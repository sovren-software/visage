use std::path::Path;
use thiserror::Error;
use tokio_rusqlite::Connection;
use visage_core::{Embedding, FaceModel};

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use rand::rngs::OsRng;
use rand::RngCore;

const EMBEDDING_DIM: usize = 512;
const EMBEDDING_BYTE_LEN: usize = EMBEDDING_DIM * 4;

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("database error: {0}")]
    Db(#[from] tokio_rusqlite::Error),
    #[error("rusqlite error: {0}")]
    Rusqlite(#[from] rusqlite::Error),
    #[error("embedding encryption failed")]
    EncryptionFailed,
    #[error("embedding decryption failed — key mismatch or corrupted data")]
    DecryptionFailed,
    #[error("invalid embedding blob size: {0} bytes")]
    InvalidBlob(usize),
    #[error("invalid embedding dimension: {0} (expected 512)")]
    InvalidEmbeddingDim(usize),
    #[error("invalid embedding value (NaN/Inf)")]
    InvalidEmbeddingValue,
    #[error("encryption key I/O error: {0}")]
    KeyIo(#[source] std::io::Error),
}

/// SQLite-backed face model storage with AES-256-GCM encryption.
///
/// Embeddings are encrypted before storage and decrypted on retrieval.
/// A per-installation 32-byte key is generated at first use and stored at
/// `{db_dir}/.key` (mode 0600, root-readable only).
///
/// Legacy plaintext blobs (2048 bytes) are accepted transparently — they are
/// migrated to encrypted format on the next enrollment.
#[derive(Clone)]
pub struct FaceModelStore {
    conn: Connection,
    enc_key: [u8; 32],
}

impl FaceModelStore {
    /// Open (or create) the database at the given path and run migrations.
    pub async fn open(db_path: &Path) -> Result<Self, StoreError> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let enc_key = if db_path == Path::new(":memory:") {
            // In-memory DB (tests): use a fixed all-zeros key
            [0u8; 32]
        } else {
            let key_path = db_path
                .parent()
                .unwrap_or(Path::new("/var/lib/visage"))
                .join(".key");
            load_or_generate_key(&key_path)?
        };

        let conn = Connection::open(db_path).await?;

        conn.call(|conn| {
            conn.execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;
                 CREATE TABLE IF NOT EXISTS faces (
                     id TEXT PRIMARY KEY,
                     user TEXT NOT NULL,
                     label TEXT NOT NULL,
                     embedding BLOB NOT NULL,
                     model_version TEXT NOT NULL,
                     quality_score REAL NOT NULL DEFAULT 0.0,
                     pose_label TEXT NOT NULL DEFAULT 'frontal',
                     created_at TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_faces_user ON faces(user);",
            )?;
            Ok(())
        })
        .await?;

        Ok(Self { conn, enc_key })
    }

    /// Insert a new face model. Returns the generated UUID.
    pub async fn insert(
        &self,
        user: &str,
        label: &str,
        embedding: &Embedding,
        quality_score: f32,
    ) -> Result<String, StoreError> {
        let id = uuid::Uuid::new_v4().to_string();
        let model_version = embedding
            .model_version
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let created_at = chrono::Utc::now().to_rfc3339();

        // Encrypt before entering the SQLite closure
        validate_embedding_values(&embedding.values)?;
        let blob = self.encrypt_embedding(&embedding.values)?;

        let id_clone = id.clone();
        let user = user.to_string();
        let label = label.to_string();

        self.conn
            .call(move |conn| {
                conn.execute(
                    "INSERT INTO faces (id, user, label, embedding, model_version, quality_score, pose_label, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'frontal', ?7)",
                    rusqlite::params![id_clone, user, label, blob, model_version, quality_score, created_at],
                )?;
                Ok(())
            })
            .await?;

        Ok(id)
    }

    /// Get all face models for a user (the gallery for verification).
    pub async fn get_gallery_for_user(&self, user: &str) -> Result<Vec<FaceModel>, StoreError> {
        let user = user.to_string();

        // Fetch raw rows from SQLite; decrypt outside the blocking closure
        let rows: Vec<(String, String, String, Vec<u8>, String, String)> = self
            .conn
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, user, label, embedding, model_version, created_at
                     FROM faces WHERE user = ?1",
                )?;
                let rows = stmt.query_map([&user], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                })?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;

        let mut models = Vec::with_capacity(rows.len());
        for (id, user, label, blob, model_version, created_at) in rows {
            let values = self.decrypt_embedding(&blob)?;
            models.push(FaceModel {
                id,
                user,
                label,
                embedding: Embedding {
                    values,
                    model_version: Some(model_version),
                },
                created_at,
            });
        }
        Ok(models)
    }

    /// List face models for a user (metadata only, no embeddings).
    pub async fn list_by_user(&self, user: &str) -> Result<Vec<ModelInfo>, StoreError> {
        let user = user.to_string();
        self.conn
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, label, model_version, quality_score, created_at
                     FROM faces WHERE user = ?1 ORDER BY created_at",
                )?;
                let rows = stmt.query_map([&user], |row| {
                    Ok(ModelInfo {
                        id: row.get(0)?,
                        label: row.get(1)?,
                        model_version: row.get(2)?,
                        quality_score: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                })?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await
            .map_err(StoreError::from)
    }

    /// Remove a face model by ID, scoped to a user for cross-user protection.
    pub async fn remove(&self, user: &str, model_id: &str) -> Result<bool, StoreError> {
        let user = user.to_string();
        let model_id = model_id.to_string();
        self.conn
            .call(move |conn| {
                let affected = conn.execute(
                    "DELETE FROM faces WHERE id = ?1 AND user = ?2",
                    [&model_id, &user],
                )?;
                Ok(affected > 0)
            })
            .await
            .map_err(StoreError::from)
    }

    /// Count total enrolled face models across all users.
    pub async fn count_all(&self) -> Result<u64, StoreError> {
        self.conn
            .call(|conn| {
                let count: u64 =
                    conn.query_row("SELECT COUNT(*) FROM faces", [], |row| row.get(0))?;
                Ok(count)
            })
            .await
            .map_err(StoreError::from)
    }

    // ── Encryption helpers ────────────────────────────────────────────────────

    /// Encrypt embedding values with AES-256-GCM.
    ///
    /// Output: 12-byte random nonce || ciphertext || 16-byte GCM tag.
    fn encrypt_embedding(&self, values: &[f32]) -> Result<Vec<u8>, StoreError> {
        validate_embedding_values(values)?;
        let plaintext = embedding_to_bytes(values);

        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let key = Key::<Aes256Gcm>::from_slice(&self.enc_key);
        let cipher = Aes256Gcm::new(key);

        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_slice())
            .map_err(|_| StoreError::EncryptionFailed)?;

        let mut blob = Vec::with_capacity(12 + ciphertext.len());
        blob.extend_from_slice(&nonce_bytes);
        blob.extend_from_slice(&ciphertext);
        Ok(blob)
    }

    /// Decrypt an embedding blob.
    ///
    /// Accepts the legacy plaintext format (512 × 4 = 2048 bytes) and the
    /// current encrypted format (12-byte nonce + ciphertext + 16-byte GCM tag).
    fn decrypt_embedding(&self, blob: &[u8]) -> Result<Vec<f32>, StoreError> {
        const NONCE_LEN: usize = 12;

        if blob.len() == EMBEDDING_BYTE_LEN {
            // Legacy plaintext — accept transparently; re-enrolled next time
            return bytes_to_embedding_strict(blob);
        }

        if blob.len() <= NONCE_LEN {
            return Err(StoreError::InvalidBlob(blob.len()));
        }

        let (nonce_bytes, ciphertext) = blob.split_at(NONCE_LEN);
        let nonce = Nonce::from_slice(nonce_bytes);
        let key = Key::<Aes256Gcm>::from_slice(&self.enc_key);
        let cipher = Aes256Gcm::new(key);

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| StoreError::DecryptionFailed)?;

        bytes_to_embedding_strict(&plaintext)
    }
}

// ── Key management ────────────────────────────────────────────────────────────

/// Load the encryption key from disk, or generate and persist a new one.
/// Written with mode 0600 (owner-readable only).
fn load_or_generate_key(key_path: &Path) -> Result<[u8; 32], StoreError> {
    if key_path.exists() {
        let bytes = std::fs::read(key_path).map_err(StoreError::KeyIo)?;
        if bytes.len() != 32 {
            return Err(StoreError::KeyIo(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "encryption key file has wrong length ({} bytes, expected 32)",
                    bytes.len()
                ),
            )));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        tracing::debug!(path = %key_path.display(), "loaded encryption key");
        Ok(key)
    } else {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);

        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(key_path)
            .map_err(StoreError::KeyIo)?;
        f.write_all(&key).map_err(StoreError::KeyIo)?;

        tracing::info!(path = %key_path.display(), "generated new AES-256 encryption key");
        Ok(key)
    }
}

// ── Serialization helpers ─────────────────────────────────────────────────────

fn embedding_to_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for &v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

fn bytes_to_embedding_strict(bytes: &[u8]) -> Result<Vec<f32>, StoreError> {
    if bytes.len() != EMBEDDING_BYTE_LEN {
        return Err(StoreError::InvalidBlob(bytes.len()));
    }

    let mut values = Vec::with_capacity(EMBEDDING_DIM);
    for chunk in bytes.chunks_exact(4) {
        let arr: [u8; 4] = chunk
            .try_into()
            .map_err(|_| StoreError::InvalidBlob(bytes.len()))?;
        let v = f32::from_le_bytes(arr);
        if !v.is_finite() {
            return Err(StoreError::InvalidEmbeddingValue);
        }
        values.push(v);
    }

    if values.len() != EMBEDDING_DIM {
        return Err(StoreError::InvalidEmbeddingDim(values.len()));
    }

    Ok(values)
}

fn validate_embedding_values(values: &[f32]) -> Result<(), StoreError> {
    if values.len() != EMBEDDING_DIM {
        return Err(StoreError::InvalidEmbeddingDim(values.len()));
    }
    if values.iter().any(|v| !v.is_finite()) {
        return Err(StoreError::InvalidEmbeddingValue);
    }
    Ok(())
}

// ── Public types ──────────────────────────────────────────────────────────────

/// Metadata about an enrolled face model (no embedding data).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub model_version: String,
    pub quality_score: f64,
    pub created_at: String,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_roundtrip() {
        let store = FaceModelStore::open(Path::new(":memory:")).await.unwrap();

        let embedding = Embedding {
            values: (0..EMBEDDING_DIM)
                .map(|i| i as f32 / EMBEDDING_DIM as f32)
                .collect(),
            model_version: Some("w600k_r50".to_string()),
        };

        let id = store
            .insert("alice", "default", &embedding, 0.85)
            .await
            .unwrap();
        assert!(!id.is_empty());

        let gallery = store.get_gallery_for_user("alice").await.unwrap();
        assert_eq!(gallery.len(), 1);
        assert_eq!(gallery[0].id, id);
        assert_eq!(gallery[0].user, "alice");
        assert_eq!(gallery[0].label, "default");
        assert_eq!(gallery[0].embedding.values, embedding.values);
        assert_eq!(
            gallery[0].embedding.model_version.as_deref(),
            Some("w600k_r50")
        );
    }

    #[tokio::test]
    async fn test_cross_user_protection() {
        let store = FaceModelStore::open(Path::new(":memory:")).await.unwrap();

        let emb = Embedding {
            values: vec![1.0; EMBEDDING_DIM],
            model_version: None,
        };

        let id = store.insert("alice", "default", &emb, 0.9).await.unwrap();

        let bob_gallery = store.get_gallery_for_user("bob").await.unwrap();
        assert!(bob_gallery.is_empty());

        let deleted = store.remove("bob", &id).await.unwrap();
        assert!(!deleted);

        let deleted = store.remove("alice", &id).await.unwrap();
        assert!(deleted);

        let gallery = store.get_gallery_for_user("alice").await.unwrap();
        assert!(gallery.is_empty());
    }

    #[tokio::test]
    async fn test_embedding_byte_fidelity() {
        // Build a 512-dim vector with interesting values at specific positions
        let mut values = vec![0.5f32; EMBEDDING_DIM];
        values[0] = 0.0;
        values[1] = -0.0;
        values[2] = 1.0;
        values[3] = -1.0;
        values[4] = f32::MIN_POSITIVE;
        values[5] = f32::EPSILON;
        values[6] = std::f32::consts::PI;
        values[7] = 0.123456789;

        let bytes = embedding_to_bytes(&values);
        let recovered = bytes_to_embedding_strict(&bytes).unwrap();
        assert_eq!(values.len(), recovered.len());
        for (orig, rec) in values.iter().zip(recovered.iter()) {
            assert_eq!(orig.to_bits(), rec.to_bits(), "mismatch: {orig} vs {rec}");
        }
    }

    #[tokio::test]
    async fn test_strict_rejects_nan() {
        let mut values = vec![0.5f32; EMBEDDING_DIM];
        values[42] = f32::NAN;
        let bytes = embedding_to_bytes(&values);
        let err = bytes_to_embedding_strict(&bytes).unwrap_err();
        assert!(matches!(err, StoreError::InvalidEmbeddingValue));
    }

    #[tokio::test]
    async fn test_strict_rejects_infinity() {
        let mut values = vec![0.5f32; EMBEDDING_DIM];
        values[0] = f32::INFINITY;
        let bytes = embedding_to_bytes(&values);
        let err = bytes_to_embedding_strict(&bytes).unwrap_err();
        assert!(matches!(err, StoreError::InvalidEmbeddingValue));
    }

    #[tokio::test]
    async fn test_strict_rejects_wrong_length() {
        let bytes = vec![0u8; 100]; // not 2048
        let err = bytes_to_embedding_strict(&bytes).unwrap_err();
        assert!(matches!(err, StoreError::InvalidBlob(100)));
    }

    #[tokio::test]
    async fn test_validate_rejects_wrong_dimension() {
        let values = vec![0.5f32; 256]; // not 512
        let err = validate_embedding_values(&values).unwrap_err();
        assert!(matches!(err, StoreError::InvalidEmbeddingDim(256)));
    }

    #[tokio::test]
    async fn test_encryption_roundtrip() {
        let store = FaceModelStore::open(Path::new(":memory:")).await.unwrap();

        // Full 512-dim embedding to exercise the real code path
        let values: Vec<f32> = (0..512).map(|i| i as f32 / 512.0).collect();
        let emb = Embedding {
            values: values.clone(),
            model_version: Some("w600k_r50".to_string()),
        };

        let id = store.insert("alice", "test", &emb, 0.95).await.unwrap();
        let gallery = store.get_gallery_for_user("alice").await.unwrap();

        assert_eq!(gallery.len(), 1);
        assert_eq!(gallery[0].id, id);
        for (orig, rec) in values.iter().zip(gallery[0].embedding.values.iter()) {
            assert_eq!(orig.to_bits(), rec.to_bits());
        }
    }

    #[tokio::test]
    async fn test_wrong_key_fails() {
        // Encrypt with one key, try to decrypt with another — must fail
        let store1 = FaceModelStore {
            conn: tokio_rusqlite::Connection::open(Path::new(":memory:"))
                .await
                .unwrap(),
            enc_key: [1u8; 32],
        };
        let store2 = FaceModelStore {
            conn: store1.conn.clone(),
            enc_key: [2u8; 32],
        };

        let values: Vec<f32> = (0..EMBEDDING_DIM)
            .map(|i| i as f32 / EMBEDDING_DIM as f32)
            .collect();
        let blob = store1.encrypt_embedding(&values).unwrap();
        assert!(store2.decrypt_embedding(&blob).is_err());
    }

    /// Known-answer test locking the AES-256-GCM primitive against NIST GCM
    /// test case 14 (256-bit all-zero key, 96-bit all-zero IV, 16-byte all-zero
    /// plaintext). `aes-gcm`'s `encrypt` returns the ciphertext concatenated with
    /// the 16-byte GCM tag.
    ///
    /// This is the golden guard for a future `aes-gcm` version bump (e.g. the
    /// deferred 0.10 → 0.11 migration): any change that altered the primitive's
    /// output would make every embedding already on disk undecryptable, and it
    /// fails here first. AES-256-GCM is a fixed standard, so a correct 0.11
    /// implementation must reproduce these exact bytes.
    #[test]
    fn test_aes256gcm_known_answer_vector() {
        let key = Key::<Aes256Gcm>::from_slice(&[0u8; 32]);
        let cipher = Aes256Gcm::new(key);
        let nonce = Nonce::from_slice(&[0u8; 12]);
        let out = cipher.encrypt(nonce, [0u8; 16].as_slice()).unwrap();

        let expected_ciphertext = [
            0xce, 0xa7, 0x40, 0x3d, 0x4d, 0x60, 0x6b, 0x6e, 0x07, 0x4e, 0xc5, 0xd3, 0xba, 0xf3,
            0x9d, 0x18,
        ];
        let expected_tag = [
            0xd0, 0xd1, 0xc8, 0xa7, 0x99, 0x99, 0x6b, 0xf0, 0x26, 0x5b, 0x98, 0xb5, 0xd4, 0x8a,
            0xb9, 0x19,
        ];
        assert_eq!(out.len(), 32, "ciphertext+tag length changed");
        assert_eq!(
            &out[..16],
            &expected_ciphertext,
            "AES-256-GCM ciphertext drifted from NIST vector"
        );
        assert_eq!(
            &out[16..],
            &expected_tag,
            "AES-256-GCM tag drifted from NIST vector"
        );
    }

    /// Locks the on-disk embedding blob format — a 12-byte nonce prefix followed
    /// by AES-256-GCM ciphertext + 16-byte tag — and confirms AEAD authentication
    /// (any tag tamper is rejected, i.e. decryption fails closed). Together with
    /// the KAT above this guards the storage contract across `aes-gcm` upgrades.
    #[tokio::test]
    async fn test_encrypted_blob_format_and_authentication() {
        let store = FaceModelStore {
            conn: tokio_rusqlite::Connection::open(Path::new(":memory:"))
                .await
                .unwrap(),
            enc_key: [7u8; 32],
        };
        let values: Vec<f32> = (0..EMBEDDING_DIM).map(|i| i as f32 / 512.0).collect();

        let blob = store.encrypt_embedding(&values).unwrap();
        // 12-byte nonce + 2048-byte plaintext + 16-byte GCM tag.
        assert_eq!(blob.len(), 12 + EMBEDDING_BYTE_LEN + 16);

        // Round-trips bit-exactly through the current format.
        let out = store.decrypt_embedding(&blob).unwrap();
        assert_eq!(out.len(), EMBEDDING_DIM);
        for (a, b) in values.iter().zip(out.iter()) {
            assert_eq!(a.to_bits(), b.to_bits());
        }

        // Flipping the final tag byte must fail authentication (fail closed).
        let mut tampered = blob.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        assert!(store.decrypt_embedding(&tampered).is_err());
    }

    #[tokio::test]
    async fn test_list_by_user() {
        let store = FaceModelStore::open(Path::new(":memory:")).await.unwrap();

        let emb = Embedding {
            values: vec![1.0; EMBEDDING_DIM],
            model_version: Some("v1".to_string()),
        };

        store.insert("alice", "normal", &emb, 0.9).await.unwrap();
        store.insert("alice", "glasses", &emb, 0.8).await.unwrap();
        store.insert("bob", "default", &emb, 0.7).await.unwrap();

        let alice_models = store.list_by_user("alice").await.unwrap();
        assert_eq!(alice_models.len(), 2);
        assert_eq!(alice_models[0].label, "normal");
        assert_eq!(alice_models[1].label, "glasses");

        let count = store.count_all().await.unwrap();
        assert_eq!(count, 3);
    }
}
