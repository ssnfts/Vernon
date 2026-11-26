# DoMINO-to-OpenFOAM Warm-Start Implementation Plan

> **For Claude / agentic workers:** Use the executing-plans skill or an equivalent task-by-task workflow. Read current repository instructions before editing. Work through the checkboxes in dependency order, with tests before behavior changes. This document is a future implementation handoff, not evidence of implemented functionality or an instruction to start a production run immediately.

**Date:** 2026-11-09, Australia/Sydney.

**Goal:** Implement and validate an optional path:
**Geometry → DoMINO volume prediction → physical-unit conversion → mapping onto the CFD mesh → boundary/continuity correction → OpenFOAM.**
Measure whether it reduces actual time to the existing CFD acceptance criteria.

**Architecture:** Preserve the current AI preview. Introduce a versioned prediction-bundle contract between the external Python inference process and Rust CFD integration. Use an external, separately compiled OpenFOAM utility for mesh-native field import, boundary evaluation and conservative flux correction. OpenFOAM independently produces all acceptance evidence. Begin with frozen pretrained weights and import only U/p; perform model adaptation later if measured evidence justifies it.

**Tech stack:** Existing Rust workspace and bounded PowerShell runner; installed PhysicsNeMo 1.3.0/PyTorch environment; installed OpenFOAM v2606/development tools; separate-process OpenFOAM utility. No C++ library is linked into Vernon.

## 1. Read this first: purpose, entry conditions and scope

The owner wants this implemented after the diagnostic testing setup is verified and before a new production trial. A passing C0 instrumentation gate does not establish a converged control recipe. D1's behavior after returning to production schemes is the important numerical evidence.

At the start of the execution session:

- [ ] Read AGENTS.md, CLEAN_ROOM.md, docs/HANDOFF.md, docs/CONTINUATION.md, docs/CFD-RUNBOOK.md and the current turbulence-diagnostic plan.
- [ ] Inspect actual C0/D1 status, phase logs, clipping-counter tables, residual/force histories and memory artifacts.
- [ ] Establish checkout/worktree, source revision, active worker lock, concurrent-writer state and process ownership.
- [ ] State which numerical recipe will be identical in the control and candidate.
- [ ] State which milestones below are currently unblocked.

Use actual artifacts rather than the previous chat's status. Do not edit tracked source while a fingerprint-bound worker is running. Reuse a suitable checkout; create isolation through native worktree tooling only if necessary.

Small offline tests and fixtures may proceed once ownership is clear. A full-mesh AI pilot requires the conversion, mapping, projection and runner gates below. A long production trial requires a concrete run specification and authorization in the execution session; retain existing authorization when applicable rather than asking repeatedly.

The intended first delivery:

- Frozen checkpoint; no retraining prerequisite.
- Registered automotive geometry at its physical scale.
- Zero-yaw, incompressible, steady k-omega SST.
- Same mesh, physics, ground/wheels, startup schedule and final numerics in both arms.
- Import internal velocity U and kinematic pressure p only.
- Keep established k/omega/nut initialization and turbulence boundary conditions.
- Stream direct predictions at identified CFD cell centers; no sparse interpolation lattice initially.
- Establish diagnostic/test-harness support first; expose product nodes after the contract is proven.
- A measured negative result is valid. Do not keep expanding scope until some acceleration claim becomes possible.

Excluded from the first delivery:

- Replacing OpenFOAM, force/residual histories, convergence gates or the mesh study with predictions.
- Guessing k and omega from predicted nut.
- Altering mesh density, physical scale, boundary conditions or the reference Cd to favor the AI arm.
- Neural intervention during each SIMPLE iteration.
- Unannounced framework upgrades, new checkpoints, resident servers or training campaigns.
- Using PartCrafter-generated geometry as the registered production benchmark vehicle.
- Quietly clipping/smoothing predictions until the experiment looks successful.

## 2. Current evidence: reverify before execution

These are handoff observations, not guarantees of future state.

| Item | Observed contract/status | Implication |
|---|---|---|
| Stage 2 | Cancelled at iteration 2546, exit 130; case retained | No final production verdict; never relabel/restart it as a certified new run |
| C0 | Instrumented original numerics, 1000 iterations | Its gate validates monitoring/reproduction, not CFD accuracy |
| D1 | Upwind k/omega through 500, then production limitedLinear 1 through 1000 | Read actual post-switch behavior, not only early absence of clipping |
| Mesh | Fastback half model, 12,056,197 cells, 16 ranks | Preserve measured mesh and decomposition |
| Reference physics | 16 m/s, rho 1.184 kg/m³, nu 1.567e-5 m²/s; moving ground, rotating wheels | Different geometry/modeling domain from the pretrained dataset |
| Model | nvidia/domino_drivaerml, revision 35b1bf1edafdaa2600d16182825890cd51c07427, checkpoint 0.501 | Preserve original checkpoint and all dependent hashes |
| Existing preview | Statistical min/max inverse; outputs labelled model-units; surface or volume slice | No verified whole-volume CFD initialization exists |
| Installed config | min_max_scaling; encode_parameters=false; reference values include 30 and 1.205 | Reference constants are not learned Reynolds-number conditioning |
| Volume outputs | Ux, Uy, Uz, pressure, turbulent viscosity | No k/omega outputs |
| Hardware | RTX 4090 Laptop, 16 GB VRAM; host about 32 GB; large solver about 21 GiB RSS | Sequence AI and large CFD work; measure aggregate host/WSL memory |

The baseline is numerical evidence, not an assumption. If D1 returns to persistent clipping and residual plateau, continue recipe/mesh diagnosis before treating a better initial guess as the cure. The pure conversion and small-fixture work can still proceed.

Bounding can redistribute values as well as enforce a floor. Do not describe every bounded cell as simply set to 1e-15. Use the proven pre-bound counter for counts, volumes, locations and extrema.

## 3. Immutable production acceptance

Load current values from docs/research/wp9-replication-baseline-v1.json. At preparation time:

| Requirement | Value |
|---|---:|
| Required residual fields | p, worst U component, k, omega |
| Final-window RMS initial residual, each field | <=1e-5 |
| Force window | 1000 samples |
| Minimum samples | 2000 |
| Adjacent-window mean change | <=0.2% |
| Absolute fitted drift within either window | <=0.1% |
| Raw Cd reference band | 5% |
| Fastback reference Cd | 0.275 |
| Minimum cells, half/full | 12M / 24M |
| Maximum non-orthogonality/skewness | <70 degrees / <4 |
| Three-grid study | Still required for full benchmark claim |
| Fitted drag shift | Forbidden |

Use the existing coeffs.rs::iteration_gate and qualification/benchmark chain. The separate generic gate in convergence.rs is not automatically the production gate. Preserve worst-component/worst-solve residual aggregation.

AI fields are initial conditions, not accepted solution evidence. Maintain cfd_validation=DEFERRED for predictions. A warm-start readiness flag must never set converged or benchmark_validated.

For this study, require both final force windows entirely after the last startup scheme change or AI intervention. With a switch at 500 and W=1000, the conservative earliest eligible endpoint is 2500. This is an additional scheduling restriction; existing thresholds remain unchanged.

Initial residual normalization depends on initialization. A large relative residual drop or a smaller first residual does not prove faster convergence.

## 4. Milestones and stop/go decisions

| Milestone | Deliverable | Gate |
|---|---|---|
| M0 | Inventory and frozen experiment design | Clear source/run ownership; diagnostic outcome understood |
| M1 | Coordinate/channel/normalization/SI contract | Independent reference parity and analytical tests |
| M2 | Streamed cell-identified volume bundles | Complete identity, chunk invariance, bounded resources |
| M3 | Native import and continuity correction | Serial/MPI manufactured fixtures and BC preservation |
| M4 | Owned-case/runner integration | Candidate fields survive staging; provenance/cancel/cleanup proven |
| M5 | Matched bounded CFD screens | No catastrophic numerical/resource regression |
| M6 | Full paired production evaluation | Both arms meet original gates and final solutions agree |
| M7 | Optional model adaptation | Converged labels, independent geometry holdouts, measured need |

M1–M4 are implementation work, M5 screens behavior, M6 establishes scoped production evidence. Completing one milestone never silently clears the next.

## 5. Source map and proposed changes

All paths in this handoff are relative to the resolved checkout root. Names marked proposed do not exist merely because they appear here.

### Existing owners

| File | Responsibility |
|---|---|
| scripts/ai/domino_bridge.py | Checkpoint loading, datapipe call, statistical inverse |
| crates/vernon-nodes-ai/src/aero.rs | Preview/slice node, resource estimates and labels |
| crates/vernon-nodes-ai/src/prep.rs | mm-to-m conversion, placement/domain checks |
| crates/vernon-nodes-ai/src/bridge.rs | Model process, discovery/progress/cancel; inspect cleanup scope |
| crates/vernon-nodes-ai/src/lib.rs | Registration/embedded scripts |
| crates/vernon-nodes-cfd/src/case.rs | solve_requested, initialization, stamps and provenance |
| crates/vernon-nodes-cfd/src/extproc.rs | Bounded external process execution |
| crates/vernon-nodes-cfd/src/util/staging.rs | Owned generations, 0.orig preference, leases/fingerprints |
| crates/vernon-nodes-cfd/src/diagnostic.rs and diagnostic/tests.rs | Existing phase/copy/counter/tool-fixture patterns; test-only module |
| crates/vernon-nodes-cfd/src/foam.rs | BC/system dictionary generation |
| crates/vernon-nodes-cfd/src/coeffs.rs | Actual convergence/qualification |
| crates/vernon-nodes-cfd/src/reference.rs and reference_record.rs | Reference binding and phase-bound proof |
| scripts/verification/verification.ps1 | Profiles, steps, source fingerprint, completion validation |
| scripts/verification/start-verification.ps1, verification-status.ps1, stop-verification.ps1 | Lifecycle |
| scripts/cfd/wp9-replication-solve-env.sh | Exact tool allowlist and memory caps |
| scripts/cfd/wp9-diagnostic-c0-gate.ps1 | Independent diagnostic validator |

### Proposed files

| File | Responsibility |
|---|---|
| docs/research/domino-warmstart-scaling-v1.json | Evidence-backed conversion audit |
| scripts/ai/domino_warmstart_scaling.py | Pure conversion functions |
| scripts/ai/test_domino_warmstart_scaling.py | No-GPU analytical unit tests |
| scripts/ai/domino_warmstart_bridge.py | Streamed volume-only inference |
| scripts/ai/test_domino_warmstart_bridge.py | Fake-predictor ordering/failure tests |
| crates/vernon-nodes-ai/src/warmstart.rs | Proposed cfd.ai_initial_fields node |
| crates/vernon-nodes-cfd/src/warmstart.rs | Bundle validation, owned preparation and initialization mode |
| crates/vernon-nodes-cfd/src/warmstart/tests.rs | Pure staging/validation regressions |
| crates/vernon-nodes-cfd/src/warmstart/fixtures.rs | Ignored actual-tool/model fixtures and paired harness |
| tools/openfoam/vernonWarmStart/vernonWarmStart.C | External native exporter/importer/projector |
| tools/openfoam/vernonWarmStart/Make/files and Make/options | Installed v2606 external build |
| scripts/cfd/build-wp9-ai-warmstart.sh | Bounded compilation before timing |
| scripts/cfd/wp9-ai-warmstart-env.sh | Narrow tool policy |
| scripts/cfd/wp9-ai-warmstart-gate.ps1 | Artifact-based readiness/comparison validator |
| scripts/cfd/test-wp9-ai-warmstart-gate.ps1 | Validator regressions |
| docs/specs/ai-cfd-initialization.md | Product contract and measured limits |

Keep the AI/CFD crates independent through an on-disk schema; do not create a circular dependency. Add shared contract types only if genuinely needed. No Python/Torch runtime is linked into Rust.

Add the external utility to CLEAN_ROOM.md's external-process/licensing inventory in the same change. Follow repository external-tool rules. Do not copy GPL implementation into Rust or link OpenFOAM into Vernon. Use vendor-neutral product labels such as “AI Initial Fields”; model names/research references belong in locations allowed by AGENTS.md.

## 6. M1 — correct scaling before training

### 6.1 Pin and audit actual artifacts

Record SHA-256 of checkpoint, architecture args, YAML, scaling_factors.pkl, download manifest, inference code, preprocessing revision and environment versions. Load pickle only from the fingerprinted trusted installed bundle.

Known recorded model hashes, to recheck:

- Surface DoMINO.0.501.mdlus: ad45e9477a7c0336c07a039ba70db7ddc9a2254d4ff94104d9c297ae7d7b662b
- Volume DoMINO.0.501.mdlus: faf54e217ca7855fa085122fb3a77b3aeae39f0a09a9b14c1652f1a6c9902882
- Recorded scaling_factors.pkl: 895b876ab7a2f32b4c714e4eafc7b2e7d94029e037bfc2950cf8c6cb2a818c2d

Create an audit row for each output channel:

1. Network channel index and name.
2. Statistical inverse.
3. Upstream nondimensional definition.
4. Dimensional reference value, unit and provenance.
5. Pressure offset/sign if relevant.
6. OpenFOAM target/dimensions.
7. Exact source revision/line or installed artifact.
8. Independent verification artifact and tolerance.
9. verified/unresolved status.

The installed checkpoint uses min/max; generic newer examples may show mean/std. Resolve differences from the exact checkpoint/generating data path. An unresolved U/p definition blocks import. Nut may remain unresolved and unused in the MVP.

### 6.2 Two distinct conversions

For the audited min/max convention:

    f_hat = (network_output + 1) / 2 * (training_max - training_min) + training_min

Validate every channel. Reject NaN/Inf, invalid extrema, wrong shapes/order and unknown normalization. Do not clamp predictions to [-1,1]; report out-of-training-range fractions. Explicitly test constant channels.

NVIDIA's same-checkpoint surface reference uses rho*U_ref^2 after inversion for stress-like outputs. This is a lead for auditing surface scaling, not automatic proof of volume scaling. Do not insert 1/2 merely because conventional dynamic pressure is rho*U_ref^2/2.

Enable the following only once the individual channel audit establishes their definitions:

    U_SI = U_ref * U_hat                         if U_hat = U/U_ref
    p_dynamic_Pa = rho * U_ref^2 * p_hat         if p_hat uses rho*U_ref^2
    p_OpenFOAM = (p_dynamic_Pa - p_reference_Pa) / rho

OpenFOAM p is gauge kinematic pressure with dimensions [0 2 -2 0 0 0 0], not absolute pascals. Align its additive gauge with the original case while preserving gradients. This is not a fitted force correction.

Do not infer nut units from its name/range. Do not import nut in the first delivery. Changing dimensional reference values does not teach Reynolds-number response to an unconditioned checkpoint.

### 6.3 Proposed pure interfaces and initial failing tests

In domino_warmstart_scaling.py implement:

    inverse_minmax(value, minimum, maximum) -> float
    velocity_to_si(value_hat, u_ref_mps) -> float
    pressure_to_kinematic(value_hat, rho_kg_m3, u_ref_mps,
                          p_reference_pa, pressure_scale) -> float

The explicit pressure enum rho_u_squared identifies the audited candidate convention. Unknown conventions fail. Functions have no Torch import or file/model side effect. The separate audit manifest controls readiness for use.

Write these tests first:

~~~python
import unittest
from domino_warmstart_scaling import (
    inverse_minmax, velocity_to_si, pressure_to_kinematic,
)

class ScalingTests(unittest.TestCase):
    def test_minmax_endpoints_and_midpoint(self):
        self.assertEqual(inverse_minmax(-1.0, -2.0, 4.0), -2.0)
        self.assertEqual(inverse_minmax(0.0, -2.0, 4.0), 1.0)
        self.assertEqual(inverse_minmax(1.0, -2.0, 4.0), 4.0)

    def test_extrapolation_is_reported_not_clamped(self):
        self.assertEqual(inverse_minmax(2.0, -2.0, 4.0), 7.0)

    def test_velocity_reference(self):
        self.assertAlmostEqual(velocity_to_si(0.8, 16.0), 12.8)

    def test_pressure_kinematic_and_no_half_factor(self):
        p = pressure_to_kinematic(
            0.2, 1.184, 16.0, 0.0, "rho_u_squared")
        self.assertAlmostEqual(p, 51.2)

    def test_pressure_reference(self):
        p = pressure_to_kinematic(
            0.2, 1.184, 16.0, 11.84, "rho_u_squared")
        self.assertAlmostEqual(p, 41.2)

    def test_unknown_convention_is_refused(self):
        with self.assertRaises(ValueError):
            pressure_to_kinematic(
                0.2, 1.184, 16.0, 0.0, "guess")
~~~

Also test zero/negative speed/density, nonfinite inputs, reversed extrema, constant channels and permutations. Use an independent analytical expected value, not the same conversion called forward/backward.

At U=16, rho=1.184, p_hat=0.2, the candidate formula produces 60.6208 Pa and 51.2 m²/s². Conventional dynamic pressure is 151.552 Pa. This catches a factor-of-two mistake. It tests software arithmetic; the real reference fixture must establish whether the formula applies to the volume checkpoint.

### 6.4 Independent reference gate

- Use a legally accessible fixed-revision DrivAerML sample and compatible reference inference/preprocessing; hash geometry, points, metadata and labels.
- A training sample may verify integration parity; it cannot establish held-out predictive accuracy.
- Compare identical point tensors/channel outputs with the reference. Preregister same-device FP32 rtol=1e-5, atol=1e-6 in nondimensional values, then document any deterministic-kernel limits rather than adjusting silently.
- Check far-field U against sample metadata, pressure gauge, frame/signs and channel order.
- Use an asymmetric manufactured field and off-center queries; a symmetric car can hide swapped axes/reflections.
- Separate implementation parity error from surrogate-versus-CFD prediction error.
- Populate domino-warmstart-scaling-v1.json with sources/hashes, formula enums, references, channel order and verified U/p status. The bridge refuses use if verification is incomplete.

## 7. Geometry/frame registration

Graph geometry uses millimeters; CFD cell centers already use meters. Convert once.

Persist:

    x_model_m = R*x_cfd_m + t
    U_model = R*U_cfd
    U_cfd = transpose(R)*U_model

Pressure is scalar up to audited scale/gauge. For current +x flow/z-up/zero-yaw, prefer identity R and an audited translation. Reject non-unit physical scale, nonzero yaw and unverified reflection in the MVP. Test a known rotation in the pure transform layer even if production supports identity only.

The preview's bounding-box-based SnapToTrainingFrame is not reliable for half-model registration: it can shift the symmetry plane. Use the actual centerline/symmetry patch, ground plane and recorded longitudinal anchor. Preserve wheel sink/contact preparation; do not substitute the lowest geometry vertex for the ground plane without evidence.

Prefer full-car surface context from the registered source assembly corresponding to the CFD half model. A half open surface being accepted by preview is not evidence of equivalent conditioning. If reconstructing full context from a half shell is necessary, test exact-plane mirroring, reflected triangle winding, removal of duplicated cut-plane faces and wheel/contact correspondence before use.

Record original CFD geometry/component hashes, AI conditioning surface hash, R/t, anchors, component mapping and decimation operations. Keep production mesh/geometry unchanged. Preserve wheel/underbody detail in the inference surface; the current <=20,000-triangle limit is not permission to discard important features. If adequate context cannot fit, record the fidelity/resource blocker.

Required tests: SI/mm; translation leaves vector unchanged; basis-vector rotation; reflection rejection; centerline and ground alignment; correct wheel centers; unchanged physical dimensions; half-model bounding-box centering rejected.

## 8. Versioned bundle and exact cell identity

Schema: vernon.ai_initial_fields.v1. Do not reuse painted-surface or slice summaries.

Export native cell centers identified by (processor_rank, local_cell_index). This identity is bound to exact rank mesh/decomposition hashes; repartitioning invalidates the bundle. Never reconstruct identity by guessed row order or nearest-neighbor search.

Declared little-endian formats:

    rank_0000.cells.u64le        N_rank local IDs
    rank_0000.centers.f64le      N_rank x 3 CFD-meter coordinates
    rank_0000.selection.u8       eligible/preserve-baseline mask
    rank_0000.raw.f32le          selected x 5: Ux Uy Uz pressure nut
    rank_0000.initial.f64le      selected x 4: Ux Uy Uz kinematic_p

Selected predictions retain explicit original cell IDs. Reordered processing/chunks must reconstruct the same mapping. Reject duplicates, missing/out-of-range IDs, wrong rows/channels/lengths, unexpected trailing bytes, nonfinite values and unknown schema.

Required manifest:

| Group | Fields |
|---|---|
| Identity | schema, run ID, UTC, source revision/fingerprint, diagnostic_only, cfd_validation=DEFERRED |
| Model | repository/revision, checkpoint/config/scaler/script hashes, environment versions |
| Geometry | original and conditioning surface hashes, preparation, transform/anchors, units |
| Mesh | global/rank counts, decomposition identity, every rank mesh-table/query hash |
| Physics | case U/rho/nu/yaw/ground/wheels/turbulence, model references, domain-shift classification |
| Scaling | audit-manifest hash, channel order, formulas, pressure gauge/reference evidence |
| Inference | seed, precision, chunking, coverage by region, out-of-range fractions |
| Mapping | direct-cell method, mask/taper definition, per-rank output hashes |
| Resources | load/inference/write timing, peak GPU/RSS, completion/cancel status |
| Payloads | path, SHA-256, byte length, dtype and shape of every file |

Write payloads to temporary owned outputs, flush/hash them and atomically publish completion metadata last. Incomplete runs are never importable. SHA-256 is trust evidence; FNV-1a alone is insufficient. Reuse permitted existing hashing paths without introducing native Windows build dependencies.

The AI process receives geometry, queries and required metadata, not the writable CFD source case. It cannot set a CFD gate.

## 9. M2 — streamed volume inference

Sequence:

1. Export exact rank-local centers/IDs/volumes/wall distances with native OpenFOAM I/O.
2. Register into the audited model frame and select eligible points.
3. Query selected fluid centers directly; retain IDs across batches.
4. Invert statistics, then perform verified dimensional conversion.
5. Gauge-align pressure and blend U/p with baseline under a fixed mask.
6. Publish rank-local bundles for the external importer.

Observed model volume box: x[-3.5,8.5], y[-2.25,2.25], z[-0.32,3.0] meters. Re-read config. CFD tunnel extends much farther. Preserve baseline outside the model domain; never extrapolate a slice or the model box across the entire tunnel.

Initial experimental blend:

    s(t) = 3*t*t - 2*t*t*t, with t clamped to [0,1]
    alpha = alpha_domain * alpha_wall
    U_candidate = (1-alpha)*U_baseline + alpha*U_prediction
    p_candidate = (1-alpha)*p_baseline + alpha*p_prediction_gauge_aligned

Starting engineering choices: domain-edge taper 0.25 m; wall taper alpha=0 at distance<=0.005 m and alpha=1 at >=0.02 m. These are preregistered experiment parameters, not validated physical constants. Record coverage and sensitivity; do not tune them on final acceptance cases.

The box-edge distance is the minimum interior distance to its six faces. Wall distance is native CFD distance. Never write AI values into solid regions or external boundary patches. Preserve baseline where selection is ineligible and declare those fractions by wheel/underbody/body/wake/far-field region.

Resource/determinism requirements:

- Start with 4096 selected volume queries per batch, then profile; this is an initial setting, not a measured capacity claim.
- Load the volume checkpoint once per preparation.
- Reuse geometry encoding only if the installed API supports it and a parity fixture proves equivalence.
- Chunking must preserve geometry context, neighbors/stencil and row identity; a naive datapipe call per chunk may change predictions.
- Compare single-batch and chunked outputs on a small case with fixed seeds and preregistered FP32 tolerance.
- Stream CPU outputs rather than retaining 12M-cell GPU tensors/activations.
- Do not apply the surface all-pairs memory fit to volume inference.
- Run large CFD and AI preparation sequentially; measure aggregate host/WSL memory.
- Confirm owned inference child exit and resource release; do not terminate a distro or unrelated process.
- OOM/cancel/error produces no importable bundle. A smaller-batch retry is an explicit new attempt.
- An AI arm that falls back entirely to ordinary initialization is reported as fallback/unavailable, never successful acceleration.

## 10. M3 — native import, BCs and flux projection

### 10.1 External utility contract

Proposed utility vernonWarmStart, built against installed v2606. Proposed operations:

    vernonWarmStart -case CASE -parallel -time 0 -operation exportCells -bundle BUNDLE
    vernonWarmStart -case CASE -parallel -time 0 -operation applyBundle -bundle BUNDLE

These are new utility operations, not existing commands. Validate argument support during implementation. Export writes native IDs/centers/volumes/wall distances. Apply validates payloads, blends internal U/p, evaluates original BCs, corrects flux and writes U/p/phi with a quality report.

Use native I/O for binary/compressed fields, label width, precision and processor coupling. Do not regex-edit binary fields or build a partial Python OpenFOAM parser. Run on the existing decomposition; avoid reconstructing the whole 12M case in one process.

### 10.2 Boundary and turbulence preservation

Preserve patch names/types/coefficients and dimensions: inlet velocity, pressure outlet, slip top/side, moving ground, body no-slip, each rotating-wheel origin/axis/omega, symmetry, processor coupling and turbulence wall functions.

Apply internal U/p only, then evaluate BCs before computing phi. Check effective loaded values, not only dictionary text. Check them again at solver read.

Keep original k/omega/nut initialization identical to control before the SST solve. The turbulence model subsequently recalculates nut normally. No relationship inferred from predicted nut may substitute for missing k/omega outputs.

### 10.3 Correction must preserve useful predicted flow

Do not call ordinary potential initialization after AI import without a specific identity/rotational-field test. A potential-flow reconstruction can overwrite the rotational wake being supplied by AI.

Implement an explicit compatible finite-volume Helmholtz correction in the external utility. Conceptual equations:

    phi_star = interpolate(U_star) dot face_area
    solve Laplacian(psi) = divergence(phi_star)
    phi_corrected = phi_star - face_flux_of_gradient(psi)
    U_corrected = U_star - gradient(psi)

For this formulation psi has units m²/s. It is a correction potential, not automatically physical kinematic pressure. Do not add it to p without an independently defined time/coefficient relationship. Retain the audited p seed; SIMPLE handles momentum-pressure coupling.

Choose correction boundary conditions that preserve prescribed inlet/wall normal flux, permit compatible pressure-outlet adjustment and retain symmetry. Provide a well-posed reference; do not impose incompatible correction values everywhere.

Use consistent face operators and MPI coupling. Reevaluate BCs, verify U–phi consistency and apply only a fixed bounded correction iteration policy if BC evaluation reintroduces divergence. Distinguish global balance from local conservation.

### 10.4 Pre-solver quality metrics

Avoid double counting processor interfaces:

    Q_in = incoming physical-inlet volumetric flow magnitude
    epsilon_global = abs(sum physical-boundary phi) / Q_in
    epsilon_local = sum_cells abs(sum outward face phi per cell) / Q_in
    deltaU_relative = volume_weighted_RMS(U_corrected-U_star) / U_inlet

Proposed preregistered engineering gates:

- Finite fields, exact identity/dimensions, complete manifest.
- epsilon_global <=1e-6.
- epsilon_local <=1e-4.
- No introduced normal leakage on impermeable patches above declared numerical tolerance.
- Original effective BC behavior preserved.

These additional initialization-quality gates are separate from production acceptance. Establish that baseline/manufactured fixtures can meet them before using them. A documented discretization problem requires an explicit reviewed change, not silent threshold adjustment to fit a prediction.

Report correction magnitude and retained wake amplitude. Flag deltaU_relative>0.25 for review in the initial study. A projection that removes the AI correction is not demonstrated AI benefit.

### 10.5 Required fixtures

| Fixture | Assertion |
|---|---|
| Uniform divergence-free channel field | Near-identity correction and preserved BCs |
| Divergence-free rotational field | Rotational structure survives |
| Deliberately divergent compatible field | Local/global continuity reaches target |
| Pressure offset | Only additive gauge changes; gradient preserved |
| Ground/wheels/symmetry | Effective BC behavior equals control |
| Known nonuniform identified payload | Values reach exact cells, serial and MPI |
| Shuffled chunks | Identical final mapping |
| Wrong decomposition | Import refused |
| Truncated/duplicate/nonfinite payload | No published candidate; source unchanged |
| Counter enabled/disabled | Existing instrumentation remains field-neutral |

Compile on fixtures before timing a large solve; record helper source/binary/compiler hashes. Do not compile opportunistically inside measured phases.

## 11. M4 — staging, solve wiring and provenance

### 11.1 Explicit initialization choice

Inspect the current solve-step builder and case.rs::solve_requested. Preserve its default behavior.

Proposed mode:

~~~rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitialisationMode {
    ExistingPotential,
    AiVelocityPressure,
}
~~~

Disabling potential initialization must not implicitly mean a valid AI bundle exists. AI mode requires validated bundle and projection receipts.

Candidate chain:

    owned prepared case/mesh copy
      -> same baseline initialization as paired control
      -> export mesh queries
      -> bounded inference/conversion
      -> internal U/p import/blend, original turbulence initialization
      -> BC evaluation and continuity correction
      -> freeze final initial-field receipt
      -> identical startup and production phases
      -> original force/residual/benchmark qualification

For large diagnostic cases, reuse the existing decomposed phase harness instead of remeshing/reconstructing. The production input geometry/mesh remain immutable.

Add an identity-bundle ablation to distinguish wrapper/projection effects from neural effects. If projection changes the baseline, record that separately; do not attribute all improvement to the model.

### 11.2 Prevent silent loss through 0.orig

Existing staging prefers 0.orig over solver-mutated 0; case input fingerprints use the original snapshot. Writing AI values only into 0 before another staging call can discard them.

Keep upstream 0.orig immutable. Apply fields at the final supported point in a new owned generation and bind that generation to an initialization receipt. If the candidate needs a new snapshot, create it deliberately with links to source hashes; never claim the old input hash for new values.

Tests must inspect the fields at the final solver process boundary, after all copies/staging/decomposition/restarts. Verify that U/p match projected hashes and are not replaced by potential initialization.

### 11.3 Evidence/caches

- Keep geometry/physics binding separate from initialization provenance.
- If current proof rejects new initial fields, expose that honestly. Add an explicit tested initialization chain before benchmark eligibility.
- Never copy old result stamps, postProcessing histories, solved markers or accepted qualification from the cancelled run.
- Receipt includes source/baseline/mesh/model/audit/transform/blend/projection hashes, ordered operations and final initial fields.
- Cache all relevant physics, decomposition, scripts/binaries, precision and masks. Geometry-only preview caching cannot satisfy this.
- Recheck payload hashes on reuse.
- run=false reads only an identical completed artifact or reports unavailable; no surprise inference/solver start.
- Failed/cancelled preparation publishes neither a solved case nor stale history.

### 11.4 Bounded runner integration

Proposed profile wp9-ai-warmstart; variants fixture, identity, control-screen, ai-screen. Add these consistently to validation sets, dispatch, environment setup, completion checks, Linux cleanup and tests.

Reuse worker lock, Windows Job Object, source fingerprints, memory sampler, timeouts and token-scoped Linux cleanup. Extend the exact tool/argument allowlist only for required utility calls. Preserve established 4 GiB solver-rank cap and aggregate memory policy unless measured evidence justifies a separate utility bound.

AI and CFD distro/user/path settings are explicit and may differ. Use typed serialized arguments; no arbitrary shell command construction. Scope child cancellation to owned run identities. Existing broad script-name cleanup is not sufficient for a new concurrent-capable path.

Record phases independently: copy/baseline, export, load, inference, conversion, import, projection, solver, postprocess, cleanup. Exit zero is insufficient: validate exactly one final marker, matching JSON and all phase receipts.

Proposed nodes:

- cfd.ai_initial_fields: geometry plus mesh-query manifest → prediction bundle and report.
- cfd.apply_initial_fields: prepared case plus bundle → new owned case and report.

Both external actions default off. Internal adapters may precede public-node exposure. Keep preview labels unchanged until their own conversion path is verified; old model-unit caches cannot silently become SI fields.

## 12. M5/M6 — matched tests and actual speedup

### 12.1 Preregister the pair

Before running, write experiment.json with source/model/mesh/baseline hashes; physics; exact startup/final dictionaries; blend/projection; durations; thresholds; sample policy and unchanged acceptance-contract hash.

Select the control recipe using the numerical diagnostics. If the missing upwind startup is implemented, verify it separately and use it in both arms. Do not simultaneously alter relaxation, linear tolerances, mesh or operating point.

Use identical instrumentation, writes, ranks and final numerics. Run sequentially on this machine. Record AC/thermal/resource conditions and speed drift. Separate genuinely shared costs from arm-specific costs.

### 12.2 Screening sequence

1. Small model reference test: units/order/parity, no CFD verdict.
2. Native identity/import/projection fixture, serial and MPI.
3. Full-mesh preparation-only screen: coverage, identity, memory, continuity and field survival, no advancement.
4. Matched 100-iteration startup smoke.
5. If healthy, matched 1000-iteration screen; compare 501–1000 with the 500 switch.

The short runs cannot certify the two-window production contract. Use fresh cases; do not silently promote a smoke continuation to production.

Measure: per-field initial RMS/trend; full residual trace; pre-bound negative/below-floor cell counts and volumes; worst locations/persistent regions; force mean/scatter/drift; y+/wall treatment in problem regions; continuity; per-phase timing and resources.

Hard stops: NaN/Inf, fatal errors, failed import/projection quality, missing counter intervals, identity mismatch, source/mesh/BC change, timeout/cancel or resource breach. Ordinary clipping is a measured outcome, not an automatic abort that prevents reproducing a clipping control.

Lower clipping alone, smaller final linear residuals alone, and a smaller first normalized residual alone do not demonstrate success.

### 12.3 Full production pair

Proceed only after evidence justifies it and the owner authorizes the concrete long-run proposal. Keep current maximum-iteration policy and full benchmark requirements. Other rear forms and the mesh study remain distinct work.

Measure:

    T_total = copy/export + load/inference + conversion/import
              + boundary/projection + solver + required postprocess
    speedup_actual = T_total_control / T_total_AI

Include potential initialization if the blended AI arm actually uses it. Report common mesh generation separately and in a from-scratch scenario. Do not make AI overhead vanish by reusing diagnostic artifacts for free.

Compute earliest eligible gate-passing prefixes from actual histories, with complete fields and both windows after recipe changes. For this acceleration study, require the crossing to persist through the next scheduled 250-iteration checkpoint. This is an additional robustness check, not a relaxed replacement gate.

If both arms execute all 7000 iterations, earlier prefix convergence is a potential saving, not measured elapsed acceleration. Genuine early stopping needs a separately tested contract that checks residuals AND both force windows; residualControl alone is insufficient. Do not change existing production stopping behavior casually as part of this feature.

### 12.4 Final-solution agreement

- Both arms independently meet unchanged production/reference gates.
- Compare Cd/Cl/moments, wheel/body force contributions and pressure distributions.
- On identical mesh, compare fields after pressure gauge alignment.
- Preregister a study Cd equivalence bound of 0.2% relative to the converged control; also report absolute delta and scatter. This is additional to the 5% reference band.
- Near-zero Cl needs an absolute bound from repeated converged controls/scatter, defined before viewing candidate results. Until that exists, mark Cl equivalence unestablished.
- Preregister field metrics/region weighting; do not drop hard regions after seeing errors.
- One pair is a pilot observation, not a universal speedup factor. Seek repeat measurements or additional held-out geometries for reproducibility.
- If only AI passes, report a promising convergence effect, not proven same-solution speedup against a failed control.
- If only control passes, retain the ordinary path.
- If both plateau, return to numerical diagnosis.
- If AI overhead exceeds savings, keep optional/disabled and document the negative result.

## 13. M7 — adapt weights to Vernon only after scaling and evidence

### 13.1 Labels and splits

Use independently converged, provenance-bound cases with explicit mesh/uncertainty qualification at desired conditions. Retain original U/p/k/omega/nut, surface pressure/shear, forces, gauge, units, BCs, air, geometry family and solver recipe. Derived samples retain original-case identity.

Stage 2's cancelled fields and C0/D1 screens are not converged steady labels. They may inform a separate stability diagnostic. Public HRLES time averages and steady SST labels must remain distinguishable.

Split by complete geometry and parent design family. Never randomly split points, partitions, snapshots or decimated copies of one vehicle between train/test. Keep train, model-selection validation and untouched final test manifests. Fit normalizers on training data only.

Include wheel/underbody/ground/wake and body-kit variations matching intended use. A small pilot supports learning curves, not broad claims. Millions of points from one case are not millions of independent examples. Three rear forms alone do not establish generalization.

Select additional cases using measured errors or calibrated uncertainty; one model's output-range check is not calibrated uncertainty.

### 13.2 Adaptation order

1. Freeze the base and train a small additive corrector.
2. If insufficient, fine-tune output decoders with geometry encoder frozen.
3. Unfreeze selected deeper blocks only if validation evidence supports it.
4. Add speed/density/viscosity/yaw conditioning as an explicit architecture-and-data change. Flipping encode_parameters cannot train conditioning or load incompatible weights correctly.
5. Add k/omega outputs only with valid labels and a measured need. Use log/positive parameterization tied to documented floors and test near-zero values.

Preserve original weights. Every adapted model gets a new identity, data/split hashes and evaluation record. NVIDIA's NIM fine-tuning recipe is for another checkpoint: use the strategy, not incompatible architecture assumptions or reported performance.

### 13.3 Losses depend on the job

Normalize loss terms separately. Area-weight surface fields and volume-weight volume fields. Correct for regional oversampling using explicit probabilities/weights.

Design-screening objective: pressure/shear accuracy, integrated Cd/Cl/moments and design-delta ranking. Force loss requires verified normal orientation, sign, area, dimensional conversion and reference conditions.

Initialization objective:

    L = lambda_U*L_U + lambda_p*L_p
        + lambda_div*L_discrete_continuity
        + lambda_BC*L_boundary
        + lambda_region*L_wheel_underbody_wake
        + lambda_turb*L_log_k_log_omega       [only with trained outputs]
        + lambda_force*L_force               [only with verified integration]

Start normalized supervised U/p terms at unit weights. Add extra terms in separate validation ablations. Tune against held-out field accuracy and actual downstream solver behavior, not the final acceptance geometry. A continuity loss does not guarantee conservation after import; retain projection.

### 13.4 Training feasibility

Proposed pilot learning-rate candidates: corrector 1e-4 and 1e-3; pretrained decoder fine-tuning 1e-5 and 1e-4. These are experimental search points, not proven optimal values.

Record optimizer/schedule/regularization/precision, sampled points, accumulation, seeds and exact frozen/trainable names. Profile one training step before scheduling epochs. Inference fitting in 16 GB does not prove training fits. Introduce mixed precision/checkpointing only with comparison; do not upgrade environments during CFD experiments.

Select checkpoints on grouped validation. Compare unchanged base, simple baseline initialization and adapted model. Repeat actual solver A/B after training. Include training time, label-generation cost and future-run break-even cost.

## 14. Test-first tasks for implementation

Each task ends with targeted verification and an independently reviewable commit when applicable. Do not push/publish without instructions. This document itself is the requested deliverable; execution is future work.

### Task 0 — Current state

- [ ] Read owners and artifacts in sections 1–2.
- [ ] Confirm lock/process/source ownership.
- [ ] Freeze actual diagnostic outcome and candidate control recipe.
- [ ] Create run-owned inventory/experiment identity without starting long work.

Acceptance: every input is locatable and identity is reproducible.

### Task 1 — Conversion audit

Files: new Python scaling module/tests and research audit JSON.

- [ ] Add section 6 analytical tests; confirm intended RED behavior.
- [ ] Implement exact interfaces and validation.
- [ ] Add malformed/channel/gauge/nonfinite tests.
- [ ] Run independent same-checkpoint reference parity.
- [ ] Populate verified U/p audit; unresolved channels remain unavailable.

Acceptance: pure tests pass and no guessed scaling reaches CFD import.

### Task 2 — Frame and bundle

Files: new Rust warmstart modules/tests, focused shared prep changes.

- [ ] Add RED tests for frame, half centerline, units and binary identity.
- [ ] Implement explicit transform/schema validation.
- [ ] Test corruption, changed decomposition, duplicate/missing IDs and checksums.
- [ ] Prove source unchanged on failure.

Acceptance: wrong/partial bundles cannot reach a writer.

### Task 3 — External native utility

Files: utility/build files, fixture policy, licensing inventory.

- [ ] Add actual-tool fixture expectations for native export/import and projection.
- [ ] Implement export/identity import before model inference.
- [ ] Implement documented projection and original-BC preservation.
- [ ] Precompile bounded; record identity.
- [ ] Run serial/four-rank fixtures including rotational preservation and cancel/error paths.

Acceptance: valid native fields and continuity; no source mutation/partial publication.

### Task 4 — Streamed inference

Files: new bridge and fake-predictor tests.

- [ ] Test shuffled chunks, exact reconstruction, cancellation and atomic completion first.
- [ ] Implement volume-only identified queries, audited conversion and masks.
- [ ] Add ignored actual-model parity/memory fixture.
- [ ] Keep unaudited nut separate/unused.

Acceptance: measured bounded complete bundle; preview timing is not extrapolated.

### Task 5 — Staging/solve chain

Files: case.rs, util/staging.rs, new CFD warmstart adapter/tests.

- [ ] Add RED field-survival tests at final solver-read boundary.
- [ ] Test upstream 0.orig immutability and stale-stamp/history rejection.
- [ ] Add explicit mode and candidate ordering.
- [ ] Preserve old default steps/dictionaries.
- [ ] Add honest initialization provenance/reference binding.
- [ ] Test failure/cancel never publishes a solved case.

Acceptance: solver reads exact projected fields with no potential overwrite.

### Task 6 — Runner and validator

Files: verification dispatch/lifecycle, new policy/gate scripts/tests.

- [ ] RED tests for profiles/variants, exact arguments, timeouts, sampling and cleanup.
- [ ] Reject zero executed tests, skips, duplicate/missing marker, mismatched JSON/hash, incomplete phase and failed cleanup.
- [ ] Implement bounded phases and source fingerprint coverage for new scripts/helper source/build identity.
- [ ] Run fixture/identity through the actual profile.

Acceptance: independently validated completion; old production profiles retain behavior.

### Task 7 — Nodes/docs/regression

Files: AI/CFD registrations/adapters, spec and generated docs.

- [ ] Test default-off actions, cache reads, unavailable dependencies, cancellation and report types.
- [ ] Keep preview labels/old-cache unit semantics accurate.
- [ ] Expose coverage/domain shift and experimental readiness.
- [ ] Regenerate catalogue/agent guide with actual generators; derive actual node counts.
- [ ] Run current AGENTS.md definition of done and bounded GUI startup.

Acceptance: software integration verified; no automatic physical-acceptance claim.

### Task 8 — Bounded full-mesh screens

- [ ] Freeze pair before execution.
- [ ] Preparation-only gate: identity, coverage, projection, resources, field survival.
- [ ] Matched 100-iteration smoke; inspect before extending.
- [ ] Matched 1000-iteration screen if justified.
- [ ] Report post-switch comparisons and all preparation costs.
- [ ] Recommend proceed/investigate/stop from evidence.

Acceptance: concrete reviewable production proposal or documented negative screen.

### Task 9 — Conditional production evaluation

- [ ] Present exact pair, recipe, bounds, cost/evidence and obtain any required execution authorization.
- [ ] Execute only authorized runs through bounded profile.
- [ ] Evaluate unchanged gates, final equivalence and actual versus potential timing.
- [ ] Save versioned report with full identity and limitations.

Acceptance: scoped demonstrated benefit or honest lack of benefit; old cancelled status unchanged.

### Task 10 — Conditional adaptation

- [ ] Data/split/normalizer leakage tests before training.
- [ ] Immutable base, corrector then limited fine-tuning.
- [ ] Independent holdouts and full import/projection/solver reevaluation.
- [ ] New model identity and cost/generalization report.

Acceptance: held-out downstream benefit; target memorization rejected.

## 15. Verification commands

Run PowerShell from resolved checkout root once corresponding proposed files exist:

~~~powershell
python -m unittest discover -s scripts/ai -p 'test_domino_warmstart_scaling.py' -v
python -m unittest discover -s scripts/ai -p 'test_domino_warmstart_bridge.py' -v
. ./scripts/dev-env.ps1
cargo test -p vernon-nodes-ai
cargo test -p vernon-nodes-cfd
cargo clippy -p vernon-nodes-ai -p vernon-nodes-cfd --all-targets -- -D warnings
pwsh -NoProfile -File scripts/cfd/test-wp9-ai-warmstart-gate.ps1
~~~

Expected: actual tests executed, no failures, no warnings. Record intended RED failures, not unrelated syntax/import breakage.

Proposed ignored actual-tool fixture names:

    warmstart::fixtures::identity_projection_preserves_rotational_flow
    warmstart::fixtures::divergent_flow_is_corrected_in_serial_and_parallel
    warmstart::fixtures::candidate_survives_staging_to_solver_read
    warmstart::fixtures::domino_chunking_matches_reference

Confirm test discovery before invoking exact names. Keep actual model/OpenFOAM fixtures ignored in ordinary workspace tests; normal tests require no GPU/WSL/install.

Use the tested bounded profile for actual fixtures/screens, not unbounded cargo test --ignored. Verify launch-script parameters from actual source. Proposed profile names are not working commands today.

Complete current AGENTS.md workspace build/tests/Clippy/demo/catalogue checks and applicable guide generation/application startup. Report actual counts, not historical 1763/1765 totals. Do not build/test heavily alongside a memory-saturated CFD solve. Freeze source before launching fingerprint-bound experiments.

## 16. Required handoff/report

Deliver:

1. Source revision, milestones and actual test/log evidence.
2. Scaling audit, independent parity and unresolved channels.
3. Geometry/query/bundle/import/projection manifests and exact field hashes.
4. Manufactured serial/MPI and failure-path results.
5. Selected control recipe and C0/D1 implications.
6. Phase-specific statuses, not one ambiguous green badge.
7. Comparable residual/force/clipping/continuity/coverage plots/tables.
8. Total/per-phase timing, peak memory, shared/arm-specific costs.
9. Production gate and final-solution agreement separately.
10. Evidence-based recommendation and remaining limits.

Use statuses precisely:

| Status | Meaning |
|---|---|
| scaling_verified | Audited channel conversion passes independent tests |
| bundle_complete | Exact identities and payloads verified |
| initialization_ready | Imported fields, BC/continuity and survival checks pass |
| screen_completed | Bounded diagnostic evidence complete |
| production_converged | Existing solver convergence gate passes |
| benchmark_validated | Additional existing benchmark requirements pass |
| acceleration_demonstrated | Actual elapsed benefit with accepted equivalent solutions and stated scope |

A status does not imply the next. Downstream solver convergence does not independently validate the raw surrogate.

## 17. References and version discipline

External references checked 2026-11-09. Pin source revisions and hashes during M1; main/latest links can drift.

- [Pinned model card](https://huggingface.co/nvidia/domino_drivaerml/blob/35b1bf1edafdaa2600d16182825890cd51c07427/README.md): outputs, DrivAerML family, time-averaged HRLES data and model terms. Exact installed config resolves normalization details.
- [Same-checkpoint NVIDIA workflow](https://github.com/NVIDIA/physicsnemo-cfd/blob/main/workflows/domino_design_sensitivities/README.md) and [implementation](https://github.com/NVIDIA/physicsnemo-cfd/blob/main/workflows/domino_design_sensitivities/main.py): surface dimensionalization/force/sensitivity audit references, not proof of Vernon's volume warm start.
- [DoMINO training example](https://docs.nvidia.com/physicsnemo/latest/physicsnemo/examples/cfd/external_aerodynamics/domino/README.html): workflow reference; defaults/API may differ from installed checkpoint.
- [NIM fine-tuning example](https://docs.nvidia.com/physicsnemo/latest/physicsnemo/examples/cfd/external_aerodynamics/domino_nim_finetuning/README.html): frozen predictor/trainable corrector strategy for another checkpoint; preliminary results do not establish Vernon's data requirement or solver speedup.
- [Potential-flow initializer](https://doc.openfoam.com/2306/tools/processing/solvers/rtm/basic/potentialFoam/) and [pressure–velocity algorithms](https://doc.openfoam.com/2312/tools/processing/solvers/pressure-velocity/): conceptual references; verify installed v2606 behavior before choosing flags/BC implementation.

Local primary sources: current AGENTS.md, CLEAN_ROOM.md, docs/specs/ai-models.md, docs/specs/automotive-cfd.md, docs/research/wp9-replication-baseline-v1.json, docs/CONTINUATION.md and owning code in section 5.

This document explicitly identifies proposed settings/engineering thresholds. They are experiment definitions, not measured upstream guarantees.

## 18. Final review checklist

- [ ] Current diagnostics and source ownership verified.
- [ ] Scaling established before training/import.
- [ ] Reference constants distinguished from learned conditioning.
- [ ] Full-car context, half centerline, ground and scale preserved.
- [ ] Exact cell identity and all hashes retained.
- [ ] Model coverage/tapers bounded and reported.
- [ ] No guessed k/omega/nut import.
- [ ] Projection preserves rotational flow and original BCs.
- [ ] Candidate survives 0.orig staging and solver-read boundary.
- [ ] Provenance, cache invalidation, cancellation and publication tested.
- [ ] Instrumentation/screen success never called production acceptance.
- [ ] Final windows use final recipe and unchanged gates.
- [ ] Actual timing distinguished from hypothetical prefix saving.
- [ ] Training labels converged and geometry holdouts independent.
- [ ] Existing production defaults and preview labels remain accurate.
- [ ] Production proposal follows completed tooling/screens and measured evidence.

## 19. Addendum (2026-11-09): proposed upgrades, pending owner review

Prepared while D1 (`wp9-diag-d1-20261109-a`) was running. Read from the installed
checkpoint (`E:\AI-3D\DoMINO\weights\domino_drivaerml_volume_checkpoint`) and the
saved PhysicsNeMo 1.3.0 references (`model-1.3.0-reference.py`,
`domino-datapipe-1.3.0-reference.py`). Items marked *verify* depend on code that
is not in those reference copies.

### 19.1 Scaling: physics-invariant audit (adds to M1, does not replace 6.4)

`global_stats.json` (volume) gives pressure_volume min -2.305, max 0.851, mean
-0.178; velocity mean (0.505, -0.001, 0.012). A stagnation maximum of 0.851 fits
p/(0.5*rho*U^2) with a gauge near -0.15 better than p/(rho*U^2), which would need
a +0.35 gauge. This is a hypothesis, not evidence. Before any import, resolve it
from the model's own predictions on the registered car:

- Far-upstream inviscid probe line: U_hat -> (1,0,0) fixes the velocity scale.
- Bernoulli fit there: p_hat + c*|U_hat|^2 = const. c = 0.5 means rho*U^2
  normalization; c = 1 means 0.5*rho*U^2. The constant is the gauge.
- Stagnation probe ahead of the nose: p_hat_max - p_hat_inf should equal c.
- Check that `scaling_factors.pkl` min/max equal `global_stats.json` min/max.
  If they differ, record which one the bridge uses.

Add `"half_rho_u_squared"` as a second explicit enum. The tests must refuse
both enums until the audit JSON marks one as verified. The far-field
least-squares gauge fit also replaces any single-point gauge alignment in 9.5.

### 19.2 Encode geometry once, then stream query chunks (makes M2 concrete)

`DoMINO.forward` separates cleanly. `geo_rep_volume(geo_centers, grid, sdf_grid)`
depends only on the geometry and the 128x64x64 grid. Each query chunk then only
needs `fc_p_vol` on its node encodings, `volume_local_geo_encodings` and
`solution_calculator_vol`. Use this split to encode once per preparation and
stream 12M cells, instead of calling the datapipe per chunk. The parity fixture
compares split and unsplit forward on the same points.

### 19.3 Determinism is not automatic (verify)

`SolutionCalculatorVolume` is built with `noise_intensity=50` and 7 sample points
per query (`num_neighbors_volume`). If it draws random perturbations in eval, as
the name suggests, then a global seed cannot make output independent of chunking,
because RNG consumption depends on batch layout. Read the installed
`solutions.py`. If it is random, either derive the perturbations from the cell ID
or use the datapipe's `deterministic` option and prove it. Repeated passes with
different seeds then give a cheap per-cell spread, which should be reported.

Parity runs must also set `torch.backends.cudnn.allow_tf32 = False`. It defaults
to True and the geometry UNet uses 3-D convolutions. TF32 alone can break the
rtol=1e-5 tolerance in 6.4.

### 19.4 Conditioning on the full car is mandatory

The node encoding uses `pos_volume_center_of_mass`, measured from the STL's
center of mass. A half shell moves that point off the centreline and corrupts
every query's encoding. Change "prefer full-car surface context" (section 7) to
**require** it. Add a test that refuses a conditioning STL whose center of mass
has |y| above a set tolerance.

### 19.5 Mirror ensemble for the half model

Predict at (x, y, z) and (x, -y, z). Average the two, flipping the sign of Uy.
This makes Uy = 0 exactly on the symmetry plane before projection, at twice the
inference cost. The per-cell mirror disagreement is a model-error indicator that
needs no labels. Report it by region. An alpha that also depends on this
disagreement is an optional, preregistered third arm, never the default.

### 19.6 Volume-only geometry fidelity

The 20,000-triangle cap and the N^2 memory fit come from surface-mode kNN and
all-pairs work. The volume datapipe does no surface kNN. It uses every STL vertex
when `sampling=False` (up to `geom_points_sample`, 300,000). Measure the
volume-only ceiling separately and use it to keep wheel and underbody detail.
Do not inherit the surface cap.

Note: the 128x64x64 grid over the volume box is about 9.4 x 7.0 x 5.2 cm. That
supports the wall taper in 9: the model cannot resolve boundary layers.

### 19.7 Measured bottleneck: turbulence, not the U/p transient

C0 clipped k in every iteration through 1000. About 600 cells per iteration were
still negative in iterations 501-1000, concentrated near the body, in the
underbody band and at the front wheel. Seeding U/p can only shorten the early
transient. If D1 shows the same persistent clipping after the switch, a U/p-only
warm start is unlikely to move the 1e-5 residual gate.

Proposed within section 1's rules (no k/omega guessed from nut): add a
**frozen-flow turbulence pre-solve** to `vernonWarmStart`. After projection, hold
U/phi fixed and run N `turbulence->correct()` passes so that k/omega relax to
the seeded flow. Give the control the same pre-solve on its own initial field as
a separate ablation, so the effect can be attributed. Preregister N and its cost
as an arm-specific phase.

### 19.8 Early stopping, so a speedup can be measured at all

Section 12.3 says that if both arms run 7000 iterations, only a potential saving
exists. Propose a tested stop contract during M4, not after M6. At each
250-iteration checkpoint, evaluate the unchanged `coeffs.rs::iteration_gate`
(residuals and both force windows, which must fall after the last recipe change).
Stop only when the gate passes and the next checkpoint confirms it. Use the same
contract in both arms. Without it, M6 cannot report elapsed acceleration.

### 19.9 Scheduling

`scripts/ai/` is inside the run fingerprint. While a fingerprint-bound run is
active, Task 1 (pure Python) and Task 2 tests must be written in a separate
worktree, not the live checkout. Merge them after the run completes.


## 20. D1 interim finding and revised DoMINO priorities (2026-11-09)

This section takes precedence over speculative recommendations in section 19 where they conflict. It records the owner's interim D1 report at approximately iteration 600, not a finished 501–1000 result. The run was still recorded as running when this note was prepared. Re-read final artifacts before assigning a final experimental conclusion.

### 20.1 What the interim experiment supports

Upwind k/omega produced no negative cells in iterations 1–500. After restarting with production limitedLinear 1, clipping returned immediately at iteration 501. Over 501–599, the reported mean counts were about 500 k cells versus C0's 603, and 87 omega cells versus C0's 91. Omega regions were similar to C0: front/rear wheels, underbody and near-body; most were 2 mm–1 cm from a wall.

This is evidence against the missing startup being a sufficient explanation or sufficient remedy. It supports investigating ongoing numerical behavior when the production transport is active. It does not isolate convection alone: local mesh geometry, near-wall treatment, turbulence source/cross-diffusion terms, gradients, linear-solve accuracy and nonlinear coupling can interact.

A count of bounded cells is not a production gate verdict. Region similarity does not prove that exactly the same cells recur. Lower counts alone do not prove convergence or force accuracy.

The reported omega RMS over 501–599 was about 13 times C0's. The switch can introduce a transient, so compare later subwindows and spike frequency before claiming a sustained deterioration. Complete the matched 501–1000 comparison, including U/p, forces, continuity, counts, affected volumes and extrema.

### 20.2 Numerical work now has priority over full-mesh AI initialization

Recommended order after D1 completes:

1. Complete location analysis using existing saved maps and native mesh/field data. Determine persistent cell identities and affected volumes, not only regional counts. Cross-reference actual wall distance, y+, layer coverage/transitions, aspect ratio, non-orthogonality, skewness, wheel contact and local gradients. Wall distance in millimeters is not itself y+.
2. Run a paired turbulence-relaxation experiment if the final evidence supports it: change only k/omega equation relaxation, for example 0.9 to 0.5, with the same U/p settings, schemes, mesh and checkpoint as its control.
3. If using the previously proposed D2 start at Stage 2 time 2000, compare it with C1 from that exact checkpoint. Do not treat a D1 restart at 500 as the matching control for a D2 start at 2000. Record any revised pair explicitly.
4. Consider targeted gradient/transport or linear-solve experiments based on the spatial and residual evidence. D3 is lower priority, not disproven by D1.
5. Evaluate each change separately; do not combine stronger relaxation, an AI seed and a new discretization in one nominal one-factor experiment.

Under-relaxation alters the iteration path; upwind alters spatial discretization. Keep their roles distinct. No production threshold is changed. Zero clipping alone still does not demonstrate accepted residuals or trustworthy forces.

### 20.3 Changes to the DoMINO implementation sequence

- **Keep the pretrained weights unchanged.** This experiment evaluates OpenFOAM numerics, not DoMINO prediction error. It supplies no supervised evidence for changing neural weights.
- **Continue M1–M4 offline/fixture work** when ownership permits: scaling, registered whole-car context, direct-cell identity, bounded inference, import, continuity correction and field-survival tests remain useful.
- **Defer M5 full-mesh AI screens and M6 production acceleration runs** while the selected baseline still shows unresolved recurring clipping and a residual plateau. A separately justified diagnostic AI-initialization experiment may be proposed later, but must not be sold as the next likely fix or as production readiness.
- **Retain U/p-only import in the first implementation.** Do not derive k/omega from nut or import additional turbulence channels merely to bypass the instability.
- **Preserve near-wall/geometry fidelity controls.** Verify full-car wheel/underbody context and actual error at relevant query locations; avoid claims that a surrogate will diagnose bad cells from its flow predictions.
- **Retain pre-bound monitoring after any AI import.** The counter remains the source of exact clipping evidence. DoMINO fields are supplemental flow estimates.
- **Do not train on clipped/cancelled diagnostic fields as accepted targets.** Use independently converged cases and grouped holdouts when adaptation becomes justified.

A U/p warm start may still change subsequent turbulence evolution through coupled production/advection. D1 does not prove that no possible initial field can help, or that warm starts can only affect the first few iterations. It substantially weakens the case for prioritizing initialization as the remedy for this observed recurring behavior. Any benefit must be measured.

### 20.4 Frozen-flow turbulence pre-solve is optional and conditional

The proposal in section 19.7 is a separate ablation, not an approved default stage. Repeated turbulence corrections with frozen U/phi can encounter the same problematic numerical operator and can continue bounding. An arbitrary number of passes can add cost without producing a consistent coupled SST solution.

Before adopting it, establish a stable turbulence discretization and measure a matched no-AI control with the same pre-solve. Record pass count, clipping, residuals, field change and cost. Never treat pre-solve positivity as production convergence. Prefer completing the relaxation/location investigation before building this extra large-case phase.

### 20.5 Qualifications to the section 19 audit proposals

- Bernoulli/stagnation probes are useful sanity checks, but a surrogate's own predictions cannot independently establish its dimensional normalization or pressure gauge. Resolve the rho*U^2 versus half-rho*U^2 definition from pinned preprocessing/metadata and independent reference parity. A fitted constant must not compensate for the wrong multiplicative scale.
- Mirror disagreement and repeated-seed spread are consistency/sensitivity diagnostics, not calibrated error bars or proof of correctness. Keep ensembles or adaptive blend weights as separately preregistered arms.
- A centered surface center of mass is a useful check but does not prove complete whole-car geometry. Verify registered component/context coverage and symmetry explicitly.
- The geometry-encoding grid spacing alone does not prove a hard pointwise resolution limit for a neural field model. Preserve the near-wall taper initially, but assess boundary-layer prediction fidelity from data rather than declaring it impossible from grid spacing.
- Early stopping remains a separately verified runner capability with both force windows and all residual requirements. Do not rush its integration to manufacture an actual speedup claim.

No solver settings, executable code, model weights or active run were changed by this documentation update.
