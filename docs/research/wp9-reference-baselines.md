# WP9 benchmark reference audit — 2026-11-02

User decision2026-11-02:adopt the documented16m/s published-replication baseline
in option2 below. This supersedes the pending choice and G24's old30m/s reference
condition, without changing its drag targets or accuracy gates. Binding to actual
local geometry and physics is still pending, so `benchmark_validated` remains
false until that evidence exists. A matching Cd or filename is insufficient.

## Distinct published configurations

| Source | Fastback experimental Cd | Estate experimental Cd | Notchback experimental Cd | Documented computational setup |
| --- | ---: | ---: | ---: | --- |
| DrivAerNet++ v2, Table8 | 0.275 detailed | 0.292 smooth | 0.246 smooth | 30m/s, rho1.184kg/m3, nu1.56e-5m2/s; moving ground, open detailed rotating wheels and mirrors |
| Existing S6 validation case, Table5 | 0.275 detailed | 0.319, variant not explicit | 0.277, variant not explicit | 16m/s, rho1.184kg/m3, nu1.567e-5m2/s; moving ground and51rad/s wheels |

The first row comes from [DrivAerNet++ v2, sections E.3.1/E.3.3 and Tables6/8](https://arxiv.org/html/2406.09624v2).
These are the paper's simulation conditions; the table does not establish the
original tunnel test speed. Its smooth estate/notchback baselines differ from
the current local all-detailed assembly.

The second row is [S6's geometry, setup and Table5](https://www.simscale.com/docs/validation-cases/validation-case-drivaer-model/).
S6 documents its own simulation but reports the experimental numbers from Heft.
Its text does not resolve the estate/notchback underbody and wheel variants.
The spec combined its targets with the first paper's different setup/error rows;
those combinations cannot currently establish a verified reference.

Later-author corroboration narrows the configuration uncertainty:
[Lyu et al.2023, official author PDF, pp.13/15/18](https://geometry.caltech.edu/pubs/LBWD%2B23.pdf)
describes detailed underbodies and mirrors on all three forms, moving ground and
rotating wheels, no engine-bay flow, and16m/s. Table1 reports the same measured
0.275/0.277/0.319 family and cites Heft2012. This supports a documented detailed
replication candidate; it is secondary evidence for the original measurements
and does not establish this archive revision or applicability at30m/s. The
authors also shift their predicted drag using experimental deviations; their
resulting error figures cannot be used as independent uncalibrated validation
bounds for Vernon. No such shift is permitted merely to make our runs pass.

[TUM's official geometry key](https://www.epc.ed.tum.de/en/aer/research-groups/automotive/drivaer/geometry/)
distinguishes rear shapes, detailed/smooth/engine-bay underbodies, wheels and
mirrors. It does not bind a component filename or archive revision to a measured
Cd. The [NWTF experiment](https://www.nwtf.ac.uk/dataset/drivaer-experimental-aerodynamic-dataset/)
uses another configuration and cannot fill that gap by name alone.

## Next bounded action

Inspect the original experimental tables/configuration key or an equivalent
official metadata record: [SAE2012-01-0168](https://saemobilus.sae.org/papers/introduction-a-new-realistic-generic-car-model-aerodynamic-investigations-2012-01-0168),
[ASME record](https://portal.fis.tum.de/en/publications/experimental-and-numerical-investigation-of-the-drivaer-model/),
or the [author's public thesis record](https://mediatum.ub.tum.de/1196144).
The current audit reached abstracts/access errors, not the complete original
tables. Do not bypass access controls, purchase access, register or contact
authors without separate authorization.

Record geometry codes/revisions, cooling state, tyres/contact, scale, air state,
Reynolds number, belt/wheel speeds, ride height, reference area, supports and
corrections, uncertainty, and exact page/table identifiers. Then bind archive and
component hashes plus preparation operations to the matching configuration.
Retain the current target family if that mapping is established; adopting the
different mixed-underbody family requires an explicit change to the handoff.

No new CFD run can recover missing historical experimental metadata. Do not
spend another long run merely to obtain a Cd closer to either table.

## Reviewable baseline choice

Before the accepted decision, G24 specified30m/s. The corroborated all-detailed replication uses
16m/s; neither source establishes that switching to30m/s preserves the measured
targets. Before accepting a new run, choose one of these reference contracts:

1. Keep the original-experiment contract and obtain an authorized copy of the
   SAE paper/configuration tables. Extract and bind the original setup first.
2. Adopt an explicitly named published-replication baseline: all-detailed F/E/N,
   mirrors, no engine-bay flow, moving ground and rotating wheels,16m/s,
   rho1.184kg/m3 and nu1.567e-5m2/s from S6, targets0.275/0.319/0.277. Bind the
   local archive components and preparation to that candidate; record remaining
   original-experiment uncertainty. Compare Vernon's raw drag without a fitted
   mean shift. Preserve the5% band,12M-half/24M-full cell floor, mesh study and
   strict convergence gates. Acceptance would be scoped to that documented
   replication baseline, not independent authentication of the2012 experiment.

Neither option starts another simulation or makes the current failed runs pass.
