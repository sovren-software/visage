# Distribution Is the Moat

> "The Linux facial authentication space is characterized by a single dominant but
> technically stagnant incumbent whose moat is documentation and packaging, not
> code quality." — Ecosystem Analysis, 2026

## The Core Insight

Howdy has 7,300+ stars and is recommended across every Linux forum, Reddit thread,
and AI assistant response — not because it is the best solution, but because it was
the first to ship distribution packages, write ArchWiki articles, and publish
tutorials. LinuxCamPAM is technically superior (dual-camera anti-spoofing, hardware
acceleration, auto IR/RGB detection) but has near-zero discoverability: no AUR
package, no ArchWiki page, no COPR, no tutorials. It does not exist in the minds of
users who need it.

**Visage must treat distribution as a first-class engineering concern from day one,
not as a release-week afterthought.**

## The Window

Three convergent factors create a narrow launch window:

1. **Howdy is stagnant.** Last stable release: September 2020. The v3.0 beta has
   been broken across distributions for years. The original maintainer is largely
   inactive.

2. **dlib is being orphaned.** Fedora 43 is dropping the dlib package that Howdy
   depends on. Fedora users running Howdy will face a hard break — and no packaged
   alternative exists. This is not a hypothetical future problem; it is happening now.

3. **Community demand is explicit and unmet.** Reddit and Arch forum threads asking
   for Howdy alternatives receive responses like: *"I'm not familiar with any other
   ongoing projects... your best option would be Howdy. If that doesn't function for
   some reason, you might be out of options."* The demand signal is clear. The supply
   is zero.

Visage enters this gap as the only Rust-native solution with no dlib dependency and
a persistent-daemon architecture. The technical differentiation is real. The question
is purely about making it discoverable.

## Distribution Priority

### Tier 1: Ship With v0.1

These must exist before any public announcement:

**1. NixOS Package (AEGIS Integration)**

Zero marginal cost — already in the AEGIS overlay. Produces a working reference
implementation that proves installability and serves as the canonical packaging
template for all other distributions.

Path: `packaging/nix/` (already scaffolded)

**2. AUR Package**

The Arch community is where technical early adopters live. The ArchWiki
`Linux_PAM` and `Biometric_authentication` pages are high-traffic discovery
surfaces. An AUR package is the entry point; an ArchWiki entry is the amplifier.

Deliverable: `packaging/aur/PKGBUILD`

**3. COPR (Fedora)**

Howdy's COPR maintainer is in a constant state of patching dlib breakage. When
Fedora 43 ships without dlib, that COPR goes dark. A working Visage COPR
published at that moment captures every Fedora user who searches for an
alternative.

Deliverable: `packaging/rpm/visage.spec`

### Tier 1.5: Partially Complete — Ubuntu .deb (not yet PPA)

**4. Ubuntu .deb Package**

The `.deb` package structure is complete as of Step 6 (2026-02-22). This includes:
- `pam-auth-update` integration (automatic PAM wiring)
- `visage setup` for model download (~182MB, SHA-256 verified)
- Hardened `visaged.service` systemd unit
- Proper install/remove/purge lifecycle via maintainer scripts

**What remains for full Tier 2 completion:**
- End-to-end install test on clean Ubuntu 24.04 VM (not yet done)
- GitHub release asset built in CI
- Launchpad PPA for `apt install visage` without building from source

Deliverable: `packaging/debian/` ✅ complete; PPA ⬜ pending

**5. GitHub Releases with Pre-Built Binaries**

`.deb`, `.rpm`, and a static PAM module `.so` with install script. Lowers the
barrier for users who will not or cannot build from source.

## Documentation That Must Precede v1.0

Distribution packages get users to the install step. Documentation closes the gap
to a working system. Both must exist before any launch announcement.

**ArchWiki Article**

The `Howdy` ArchWiki page is the #1 discovery surface for this category on Arch.
Visage needs its own article covering installation (AUR), configuration
(`/etc/visage/config.toml`), PAM stack integration, camera setup, and enrollment.
This article is more valuable than any GitHub README.

**One Tutorial Blog Post**

A single "I set up face authentication on Linux in 2026 without Howdy" post on a
credible personal or technical blog creates the SEO and social signal that drives
organic discovery. It should cover: why Howdy fails today (dlib/Fedora, no daemon,
plaintext storage), what Visage does differently, and a complete working install
walkthrough on at least one major distribution.

This post should exist before the v1.0 announcement, not after.

## Differentiation Copy

When writing documentation, release notes, or any public-facing content, lead with
the differences that matter to users evaluating alternatives:

- **No Python runtime.** Pure Rust binary — no dlib, no pip, no virtualenv.
- **Fast.** Persistent daemon with pre-loaded models. No 2-3 second cold-start
  penalty per `sudo`.
- **Secure.** Embeddings encrypted at rest. No world-readable face data files.
- **Portable.** Works with GDM, SDDM, LightDM. DE-agnostic keyring design.
- **Maintained.** Active development. Distribution packages that stay current.

## The Compounding Effect

Howdy's 7,300-star advantage is not a technical moat — it is a documentation and
packaging moat that compounds through search engines, GitHub trending, Reddit
recommendations, and AI training data. Visage cannot close that gap by being better
code. It closes it by being better distributed and documented, then letting time and
word-of-mouth do the rest.

The discovery bias works both ways: a project with distribution packages, an
ArchWiki article, and tutorials accumulates visibility in the same self-reinforcing
loop that sustains Howdy today. The difference is that Visage will have earned that
position with a technically superior foundation — daemon architecture, ONNX-native
inference, encrypted storage — rather than inheriting it from a stale 2020 codebase.

Ship the packages. Write the docs. The rest follows.
