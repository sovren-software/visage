# ADR 010 — Open Source Contribution Governance

**Date:** 2026-02-24
**Status:** Accepted
**Deciders:** Sovren Software core team
**Amended:** 2026-07-07 — added §9 (Problem-First PR Triage)

---

## Context

Visage is a public GitHub repository under `sovren-software/visage`, licensed MIT. Before
public promotion and community launch (Summer 2026), the project needs infrastructure to:

1. Accept and review external pull requests safely — especially for security-critical
   crates (`visaged`, `pam-visage`, `visage-core`, `visage-models`) that form the
   authentication path.
2. Provide a private disclosure channel for security vulnerabilities — Visage is a PAM
   module with root-level daemon access; public CVE disclosure without a fix is dangerous.
3. Define contribution scope boundaries — the Linux community is generous with feature
   suggestions, and Visage must stay focused on facial authentication.
4. Establish merge strategy, review expectations, and intellectual property hygiene for
   contributions.

Without this infrastructure, the first external PR would arrive with no review process,
no CI enforcement, no scope guidance, and no security disclosure path.

---

## Decision

### 1. Branch Protection on `main`

All changes to `main` must go through a pull request. Direct pushes are blocked.

**Rules enforced:**
- Require pull request before merging (1 approval minimum)
- Require `test` status check to pass (fmt + clippy + build + test)
- Block force pushes
- Restrict deletions
- No bypass list (admins use `--admin` flag for exceptional cases only)

**Rationale:** CODEOWNERS is meaningless without required reviews. CI gates are meaningless
without required status checks. Branch protection is the enforcement layer that gives the
other tools teeth.

### 2. Security Disclosure via GitHub Private Vulnerability Reporting

Security vulnerabilities are reported through GitHub's built-in Private Vulnerability
Reporting feature, documented in `SECURITY.md`.

**What we considered:**
- **Email-only** (e.g. `security@sovren.software`) — simpler but requires email
  infrastructure, PGP key management, and manual triage.
- **GitHub Security Advisories** — chosen. Zero infrastructure cost, integrates with
  GitHub's CVE database, provides private discussion space, and is the path most security
  researchers already know.

**Response timeline committed:**
| Stage | Target |
|-------|--------|
| Acknowledgment | 48 hours |
| Initial assessment | 7 days |
| Fix (critical/high) | 30 days |
| Coordinated disclosure | After fix ships; 90-day maximum |

### 3. Developer Certificate of Origin (DCO)

Contributors sign off commits with `git commit -s`, certifying the contribution is their
own work under the project's MIT license.

**What we considered:**
- **Contributor License Agreement (CLA)** — legal overhead, discourages casual
  contributors, requires CLA bot infrastructure. Appropriate for dual-licensed or
  corporate-backed projects; overkill for a single-license MIT project.
- **DCO** — chosen. Lightweight, well-understood in the Linux kernel and Rust ecosystem,
  no legal review required, no bot infrastructure. The sign-off line is sufficient for MIT.
- **Nothing** — risky. Without any IP hygiene, a contributor could later claim their code
  was submitted without authorization.

**Enforcement:** Currently honor-system with maintainer review. Automated DCO check can be
added later if contribution volume warrants it.

### 4. Merge Strategy

| Contribution type | Merge method | Rationale |
|---|---|---|
| Hardware quirks, documentation | Merge commit | Preserves contributor attribution in history |
| Bug fixes, features | Squash and merge | Clean linear history on `main` |
| Multi-crate refactors | Maintainer discretion | Depends on whether intermediate commits are meaningful |

**Rationale:** Squash-by-default keeps `main` history clean for bisection. Merge commits
for quirks and docs ensure contributors see their name in `git log` — important for a
project that depends on community hardware testing.

### 5. Code Ownership (CODEOWNERS)

`@sovren-software` is the required reviewer for all paths. Security-critical crates
(`visaged`, `pam-visage`, `visage-core`, `visage-models`) and system configuration
(`packaging/debian/`, `systemd/`, `dbus/`, `.github/`) have explicit entries.

**What we considered:**
- **Individual username** (`@ccross`) — creates a bus factor of 1.
- **GitHub team** (`@sovren-software/core`) — requires creating and maintaining a team.
- **Org-level** (`@sovren-software`) — chosen. Scales automatically as team grows.
  Any org member can review, but at least one must approve.

### 6. Issue and PR Templates

Three issue templates and one PR template standardize contribution quality:

| Template | Purpose |
|----------|---------|
| Bug report | Structured: environment, repro steps, expected/actual, logs |
| Hardware report | Mirrors Adopt-a-Laptop format — device info, test results, discovery output |
| Feature request | Includes scope-check reminder linking the out-of-scope table |
| PR template | Leads with "What problem does this solve?"; then type, change, testing evidence, DCO + quality-gate checklist |

**Rationale:** The highest-value contributions (hardware quirks) come from users who may
not be experienced OSS contributors. Templates reduce the back-and-forth from "please add
your camera VID:PID" to zero.

### 7. Automated Dependency Management (Dependabot)

Dependabot opens weekly PRs for Cargo dependency updates and GitHub Actions version bumps.
Limits: 5 Cargo PRs/week, 3 Actions PRs/week.

**Rationale:** A PAM authentication module must keep dependencies current. Dependabot
alerts were already enabled; automated PRs close the loop by making updates reviewable
and CI-tested before merge.

### 8. Review Timeline Commitments

| PR type | Target review time |
|---------|-------------------|
| Hardware quirk | 1–2 business days |
| Bug fix | 3–5 business days |
| Packaging / feature | 1–2 weeks |

**Rationale:** Published timelines set contributor expectations and reduce "is anyone
looking at this?" pings. Hardware quirks are fast-tracked because each one unlocks support
for an entire laptop model.

### 9. PR Evaluation: Problem-First Triage

*(Amendment, 2026-07-07.)* A pull request is treated as a **request to push a change** —
evaluated from the problem down, not the diff up. Every PR (external, dependabot, or
maintainer) is read through this ladder **before** line-by-line code review:

1. **What problem does it solve?** No stated problem — or a solution hunting for one — is
   asked "what's the problem here?" before its code is reviewed.
2. **Is that problem in scope?** Measured against the out-of-scope boundaries in
   `CONTRIBUTING.md`. Out-of-scope PRs are declined with the boundary, not the code.
3. **Is it the right solution?** A real, in-scope problem can still have the wrong fix —
   the bar is the simplest approach that fits the architecture and does not widen the
   auth/attack surface.
4. **Is it verifiable?** Evidence (tests, a repro, `visage discover` output) plus green CI.
5. **Governance** — DCO, security-path review, merge strategy, attribution (§§3–5 above).

**Rationale:** The other decisions here govern *how* a PR is handled (branch protection,
DCO, CI, timelines); they do not say *how it is judged*. Codifying the problem-first ladder
makes evaluation consistent and delegable — a maintainer, or an automated agent handling
routine PRs, applies the same lens, rejects/redirects early on the upper rungs (cheaper for
us, kinder and faster for the contributor), and escalates only genuine judgment calls:
scope-edge decisions, anything on the security/auth path, governance waivers, and
breaking/release-version calls. Surfaced to contributors in `CONTRIBUTING.md`
("How we evaluate a PR") and in the PR template, which now leads with
"What problem does this solve?".

---

## Consequences

### Benefits

- **PRs cannot bypass CI or review** — the authentication path is always protected.
- **Security researchers have a private channel** — no public zero-days.
- **Contributors know what to expect** — scope, timeline, merge strategy, IP requirements
  are all documented before the first external PR arrives.
- **Hardware quirk contributions are frictionless** — template + fast-track review +
  merge commit attribution.
- **Dependencies stay current automatically** — Dependabot + required CI = safe updates.

### Drawbacks

- **Single-maintainer bottleneck** — all PRs require `@sovren-software` approval. If the
  maintainer is unavailable, PRs queue. Mitigated by adding org members as the team grows.
- **Admin merge override exists** — `gh pr merge --admin` bypasses the 1-approval
  requirement. Necessary for solo-maintainer bootstrap but should be used sparingly and
  only for non-security changes.
- **DCO is not automatically enforced** — a contributor could forget the sign-off line.
  Maintainer catches this during review. Automated enforcement (e.g. `dco-bot`) can be
  added when contribution volume justifies it.
- **No CLA** — MIT license makes this acceptable, but if the project ever needs to
  relicense, DCO alone may not be sufficient. This is an acceptable trade-off for the
  current stage.

### Known Limitations

| Limitation | Impact | Mitigation |
|------------|--------|------------|
| No automated DCO enforcement | Contributor may forget sign-off | Maintainer review catches it; add bot later |
| No required CODEOWNERS review | CODEOWNERS suggests but GitHub rulesets don't require specific team review | Org-level review covers all paths |
| Admin bypass available | Maintainer can skip review for own PRs | Used only for non-security, non-code changes |
| Single approval required | One compromised reviewer could approve malicious code | Acceptable at current team size; raise to 2 when team grows |

---

## References

- [SECURITY.md](../../SECURITY.md) — vulnerability reporting policy
- [CONTRIBUTING.md](../../CONTRIBUTING.md) — contributor guide with scope, PR process, DCO
- [.github/CODEOWNERS](../../.github/CODEOWNERS) — ownership map
- [.github/pull_request_template.md](../../.github/pull_request_template.md) — PR template
- [.github/dependabot.yml](../../.github/dependabot.yml) — dependency automation config
- [Developer Certificate of Origin](https://developercertificate.org/)
