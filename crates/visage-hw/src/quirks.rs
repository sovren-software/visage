//! Hardware quirks database.
//!
//! Maps camera vendor:product IDs to the UVC control bytes
//! needed to activate their IR emitters.

use crate::ir_emitter::EmitterConfig;

/// Known camera quirks entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CameraQuirk {
    pub vendor_id: u16,
    pub product_id: u16,
    pub name: String,
    pub emitter_config: EmitterConfig,
}

/// Look up a camera quirk by USB vendor:product ID.
pub fn lookup_quirk(_vendor_id: u16, _product_id: u16) -> Option<CameraQuirk> {
    // TODO: Load from contrib/hw/ quirks database (TOML or JSON)
    // TODO: First known entry: ASUS Zenbook 14 UM3406HA IR camera
    //       vendor=0x???? product=0x???? unit=14 selector=6
    //       control=[1,3,3,0,0,0,0,0,0]
    None
}

/// List all known camera quirks.
pub fn list_quirks() -> Vec<CameraQuirk> {
    // TODO: Load and return all entries
    Vec::new()
}
