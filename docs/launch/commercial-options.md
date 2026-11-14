# Commercial options for an Apache-2.0 core (DRAFT, Owner's decision)

No market sizes, prices or conversion rates are given: I have no sourced figures for them. Where
a number is needed it is left for the Owner to set.

Context from the repository: the Windows package is unsigned; there is no measured validation
data beyond VALIDATION.md; the 22 reserved keyboard chords are unbound; external solvers are GPL
or AGPL programs run as separate processes (`CLEAN_ROOM.md`).

## Option A. Free core, paid signed installers and support
- **What it is.** Source stays free. Sell convenience: a code-signed Windows installer with
  updates, plus paid email support for small shops.
- **Needs.** A code-signing certificate (cost not researched), a release process that already
  exists in `scripts/release/`, a support inbox, a published support scope.
- **Effort.** Low to start; support time grows with users.
- **Risk.** Anyone may build and sign their own copy, so the offer must be convenience and
  responsiveness, not exclusivity. Support promises on safety-critical use need care because the
  software is not certified.
- **Licence / clean room.** No conflict with Apache-2.0, which permits selling copies and
  services. Do not use the project name in a way that implies an official fork's endorsement
  (Apache-2.0 grants no trademark rights; the Owner decides on a trademark policy). Bundling a
  GPL or AGPL solver inside the installer would conflict with the separate-process rule, so the
  installer must not include them. No clean-room issue.

## Option B. Paid validated template packs and tutorials
- **What it is.** Sell scenes and written or video tutorials built on Vernon's own graph: for
  example a printer-bed template, a material pack with measured retention curves, a coastdown
  and pressure-tap workflow.
- **Needs.** Measured data the Owner collects, which is also what VALIDATION.md says is missing.
  Pack content must be made from Owner-created inputs only.
- **Effort.** Medium to high: each pack needs real measurements before it can be called validated.
- **Risk.** Calling a pack "validated" before the data exists would break the project's own
  rules. A pack is only as good as its measurements and each claim must say which method and
  data it covers. Buyers may expect accuracy guarantees the software does not give.
- **Licence / clean room.** No licence conflict if the packs are separate works (scenes and
  documents may carry their own licence). Clean room: no vendor sample files, screenshots, help
  text or golden data copied; goldens recorded by the Owner from a licensed copy on self-made
  inputs are allowed by `CLEAN_ROOM.md` for tests, but vendor names stay out of user-visible
  strings and the paid material.

## Option C. Sponsorship and consulting
- **What it is.** Accept donations or sponsors (the project's own funding page), and offer paid
  work: scan-to-print setup, CFD or FEA case review, custom nodes for a team.
- **Needs.** A funding channel (account creation is the Owner's step), a consulting scope and a
  disclaimer that results are not certified engineering.
- **Effort.** Low to set up; income depends on the Owner's time.
- **Risk.** Time-for-money does not scale. Custom work for one client must not leak that
  client's data into the public repository. Professional-liability exposure on safety-critical
  parts should be checked with a lawyer.
- **Licence / clean room.** No conflict. Custom nodes written for a client can stay private or be
  contributed under Apache-2.0; the dependency and clean-room rules in `AGENTS.md` apply to
  anything merged into the repository.

## Comparison

| | A. Signed installers and support | B. Validated packs and tutorials | C. Sponsorship and consulting |
|---|---|---|---|
| Licence conflict | none | none | none |
| Clean-room conflict | none | only if vendor material is used | none |
| Needs data not yet in repo | no | yes | no |
| Effort to start | low | medium to high | low |
| Scales without Owner hours | partly | yes, once made | no |

## Recommendation (the Owner decides)
Start with C, with A added once the package is signed and a support scope is written. Both can
begin with what exists now, neither needs validation data that does not exist, and neither
asks anyone to pay for a claim the project cannot yet support. Treat B as a later step that grows
out of the measurements the Owner needs for validation anyway; it should not be sold until
there is data to stand behind it.

## Decisions for the Owner
1. Which option, or none yet.
2. A trademark policy for the "Vernon" name before any paid offer.
3. Whether to buy a signing certificate (cost not researched).
4. A written statement of what support does not cover (certified or safety-critical use).
