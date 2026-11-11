# Vernon project plan

Written 2026-11-11. This replaces the scattered "what next" notes as the one
place that says what Vernon is for, who it is for, what already exists
elsewhere, where local AI fits and what has to happen before a release.
Day-to-day progress stays in [CONTINUATION.md](CONTINUATION.md); the per-node
backlog stays in [ROADMAP.md](ROADMAP.md).

Market figures and the software survey come from web searches run on
2026-11-11. They are leads with sources, not audited numbers.

## 1. The end goal

One free, offline desktop app that takes a car-body part from **scan to
printed, bolted-on part**, and reports a number at every stage that says
whether the result can be trusted: scan deviation, bed fit, drag and lift
change, safety factor at the mounts, surface temperature against the
material's limit, lap-time change.

Three properties separate Vernon from the tools it stands beside:

1. **One graph, gated end to end.** Each stage reads the previous stage's
   report and refuses to trust a result that failed its gate.
2. **Commercially usable and local.** Apache-2.0, no account, no cloud, no
   public-project requirement. External solvers run as separate processes.
3. **Drivable by an AI agent, safely.** A typed tool layer and an MCP server
   let a local or cloud model build and cook graphs, with no shell and no file
   access outside a work folder.

## 2. Where the project stands

| Area | State on 2026-11-11 |
|---|---|
| Node catalogue | 201 nodes, 198 implemented, 3 white-box ([NODES.md](NODES.md)) |
| Pipeline demo | `examples/bodykit_pipeline.vrn` cooks all nine stages on a synthetic fender |
| Desktop app | egui/wgpu, quad viewports, node graph, agent chat panel |
| Agent | `vernon-agent` (OpenAI-compatible client, so any llama.cpp or Ollama server works) and `vernon-cli mcp` |
| AI nodes | PartCrafter (image to parts) and DoMINO (aero preview), both out of process, both labelled unvalidated |
| CFD validation (WP9) | Fine 12.06M-cell Fastback mesh passes its quality gates. Steady runs do not reach the convergence gate; the residual map shows the 1e-5 target is out of reach for this case. A DDES route is planned, starting with a 25° slant-body rehearsal ([plan](research/wp9-ddes-route-plan.md)). No validated Cd exists yet. |
| DoMINO warm start | Milestone 1 (scaling audit, units, reference parity on one DrivAerML run) is on branch `claude/domino-warmstart-m1`, not merged |
| Packaging | Windows folder, zip and installer scripts exist; no release has been published |
| Not there yet | CI, a published release, a real scanned-part walk-through, Linux and macOS testing, contributor docs |

The honest summary: the breadth is built, the proof is not. Nothing has yet
gone from a real scan to a real printed part through Vernon, and the CFD
stage has no validated number. Sections 6 and 7 are ordered around closing
those two gaps.

## 3. Who Vernon is for

Vernon is free, so "customer" here means the user whose problem it solves
well enough that they adopt it, tell others and contribute. Ranked by fit:

| # | User | Their problem today | Why Vernon fits | What they need first |
|---|---|---|---|---|
| 1 | **Maker car modders** with a printer and a hobby scanner | Design in a free CAD tier that has no simulation and is non-commercial; scan-to-surface is manual; CFD is skipped | The whole chain in one free app with pass/fail numbers | A packaged download, one real-part tutorial, sane defaults |
| 2 | **Small body-kit and composite shops** that print plugs and molds | Free CAD tiers forbid commercial use; reverse-engineering tools cost roughly €480 to €5,250; cloud CFD is per-run or public | Commercial-use licence, bed-fit split with keys, STEP out, A/B aero deltas | STEP quality on real panels, reliable split and key tools |
| 3 | **Amateur motorsport and student teams** (time attack, track day, Formula Student) | No budget; need defensible A/B aero numbers and a lap-time estimate | Validated CFD recipe, grid study, coastdown and tuft checks | The WP9 validation finished and published |
| 4 | **Hobby-scanner owners** in general | No free end-to-end scan-to-CAD tool exists; searches return only paid products | Registration, deviation gate, auto-surfacing, STEP export | Stages 1 to 3 working on real scans, without the car framing |
| 5 | **Aero-ML researchers and teachers** | Surrogates and solvers live in separate Python and config workflows with no GUI | Surrogate and OpenFOAM side by side on the same geometry, with provenance | The surrogate-versus-solver error report |
| 6 | **Agent builders** | Few CAD/CAE tools expose a safe, typed, headless tool surface | MCP server, generated agent guide, gates as machine-readable results | Stable tool schema, published binary |

Market size, for scale only. The 2026 SEMA Market Report puts US
specialty-equipment retail sales at $52.92 billion for 2025
([SEMA](https://www.sema.org/news-media/enews/2026/26/new-2026-sema-market-report-reveals-nearly-53-billion-aftermarket)).
Two reports put the global car body-kit market at about $1.7 to $1.8 billion
([Market Intelo](https://marketintelo.com/report/car-body-kit-market),
[TechSci](https://www.techsciresearch.com/report/car-body-kit-market/28761.html));
a third gives a figure about 75 times higher with a different scope, so the
category is poorly measured. No source counts small body-kit makers.

**Recommendation:** build for users 1 and 4 first (they need the least
validation and the most packaging), use user 3 as the proof audience for CFD,
and treat 2 as the group that turns Vernon from a hobby tool into a working
one. If the project ever needs income, the options that fit an Apache-2.0
tool are paid support and setup for shops, prebuilt validated case templates,
and hosted solver time. That is a later decision and none of it is planned.

## 4. What free software already exists

The full survey, with names, licences and sources, is in
[research/free-software-landscape.md](research/free-software-landscape.md).
Its findings:

- **No free tool covers the chain.** Open-source programs exist for each
  single stage (photogrammetry, point-cloud measurement, mesh repair,
  parametric CAD with solver front ends, slicing, the CFD solver itself),
  and makers lose accuracy in the hand-offs between them.
- **No free end-to-end scan-to-CAD tool exists.** Searches return only paid
  products; the neural point-cloud-to-CAD research models are
  non-commercial.
- **The real competition for hobby users is the free tier of closed tools**,
  limited by non-commercial terms, public projects or no simulation.
- **AI is arriving in three forms:** open aero surrogates with benchmarking
  code but no GUI; LLM agents that write and repair solver cases, mostly on
  cloud models; and MCP servers that let a coding agent drive open CAD
  programs to make prismatic parts from text. None has engineering gates,
  and a 2026 review judges open text-to-CAD not ready for complex
  production parts.

**Where Vernon is alone:** no surveyed free tool combines scan-to-surface,
print splitting, CFD, FEA and a local agent behind one set of numeric gates.
**Where it is behind:** every tool surveyed has users, releases and a track
record on real parts. Vernon has none of those yet.

## 5. Local AI: what to add

### Rules that do not change

- Models run **out of process**. A crash or out-of-memory kill never takes the app down.
- **Nothing downloads without a confirm** showing size, licence and destination. Weights are never bundled or committed.
- **Only MIT and Apache-2.0 models are built in.** Restricted licences are opt-in; non-commercial models are not offered.
- **AI output never passes a gate.** A prediction is labelled as one, and only a measured or solved number can satisfy an acceptance check.
- **No heavy model loads while a CFD run is active** on the same machine.

### What exists

PartCrafter and DoMINO nodes, the agent crate and chat panel, the MCP
server, and (unmerged) the first milestone of DoMINO warm start.

### What to build, in order of value

| # | Addition | What the user gets | Depends on | Size |
|---|---|---|---|---|
| A | **Resident model service** | Second and later runs start in seconds; today each DoMINO run reloads for about 145 s | Nothing | Small |
| B | **Hardware probe, model registry and recommender** (steps 2 to 4 of the [2026-11-04 plan](superpowers/plans/2026-11-04-ai-assistant-and-machine-split.md)) | The app says which models this PC can run and why | Nothing | Small |
| C | **Local chat as the default agent** | The chat panel works offline against a bundled llama.cpp server setup, with a tested model list | B | Medium |
| D | **Agent evaluation suite** | A fixed set of graph-building tasks scored per model, so "works with model X" is a measured claim | C | Medium |
| E | **Surrogate error report** | DoMINO against Vernon's own OpenFOAM result on the same shape, per field | WP9 result | Medium |
| F | **Surrogate warm start for OpenFOAM** | Solver starts from the predicted field; the saving in iterations is measured, not assumed | Warm-start branch, WP9 | Large, in progress |
| G | **Variant screening loop** | Sweep kit parameters (lip height, wing angle), rank with the surrogate, send the top few to OpenFOAM | E | Large |
| H | **Run-diagnosis assistant** | A local model reads solver logs and residual histories and proposes the next diagnostic; it cannot edit gates or relabel runs | C | Medium |
| I | **Learned multi-view reconstruction** (Apache-licensed models) beside COLMAP | A dense prior in minutes where COLMAP dense takes many hours; scale still comes from markers and passes the same gate | A, B | Medium |
| J | **Image-to-3D runners** (TripoSR for small GPUs, a quantised TRELLIS for 16 GB) and a retopology node | Concept parts from a photo, cleaned by the existing nodes | A, B | Medium |
| K | **Vision check of the viewport** | A local vision model describes what a render shows (holes, flipped normals, a part off the car) as a hint next to the numeric report | C | Small |
| L | **Fine-tuning a surrogate on Vernon's validated runs** | Predictions for kit shapes outside the stock training set | E, many solved cases | Research; feasibility on 16 GB unverified |

Local model candidates for C, D and K, from the 2026-11-11 search: a 20B
open-weight text model that fits 16 GB for tool calling, an 8B vision model
for K, and 7 to 9B models at 4-bit for the 11 GB machine. The sources are
blog roundups, so D exists to replace their claims with measurements.

Items A to D make Vernon usable offline by a non-expert. Items E to G are the
part no free tool offers: a surrogate whose error against a validated solver
is published and which speeds the solver up instead of replacing it.

## 6. Road to production

Each milestone ends in something a stranger can download or read.

| Milestone | Contents | Done when |
|---|---|---|
| **M0 Repository in order** | Work branches merged to `main` through reviewed pull requests; CI on Windows and Linux (build, test, clippy, demo cook); repository description and topics; CONTRIBUTING and issue templates; stale merged branches removed | `main` is green in CI and is the only long-lived branch |
| **M1 First alpha release** | Windows zip and installer as a GitHub Release; demo scene; library modules in [ROADMAP.md](ROADMAP.md) wired into their nodes; screenshots in the README | A new user installs and cooks the demo without building anything |
| **M2 One real part** | A real scan (openly licensed or the project's own) taken through capture, surfacing, split and print, with the measured deviation of the printed part | A written walk-through with photos and numbers |
| **M3 CFD credibility** | The WP9 DDES route: slant-body rehearsal, fine Fastback, grid study (estimated 6 to 11 weeks of machine time for a certified Fastback) | A published validation report with every gate shown |
| **M4 Local AI pack** | Items A to E above; F if the warm-start branch lands | Offline chat builds the demo graph; surrogate error is published |
| **M5 Beta** | Linux build tested; three to five outside users from groups 1 to 4 complete a task unaided; their blockers fixed | Their feedback is in the tracker and the top issues are closed |

M0, M1 and M2 need no long solver runs, so they can proceed on the second PC
while this one runs CFD for M3.

## 7. Repository state and housekeeping

Checked on 2026-11-11 against `origin` (`github.com/ssnfts/Vernon`).

| Branch | Ahead of `main` | Holds | Action |
|---|---:|---|---|
| `claude/wp9-solve-stages` | 31 | AI nodes, WP9 solver stages, diagnostics, DDES plan | Merge to `main` by pull request |
| `claude/domino-warmstart-m1` | 22 | DoMINO warm start M1, units module, car shell prep | Keep as a branch until M1 is accepted |
| `claude/wp9-stage2-diagnostics`, `claude/ai-models`, `claude/domino-scaling-semantics` | contained in the two above | | Delete after the merge |
| `qwen/t02` to `qwen/t06` | 1 each | Attempts that failed their own checks; the same modules later landed through other batches | Keep local or delete; not for the public repository |
| 30 other local branches | 0 | Already in `main` | Delete, and remove their worktrees |

Also missing from the repository: a CI workflow, `CONTRIBUTING.md`,
`SECURITY.md`, a changelog, any image, and a description or topics on GitHub.

## 8. Decisions needed from the owner

1. Merge `claude/wp9-solve-stages` into `main` now, or wait for the slant-body rehearsal?
2. Is commercial usability a firm requirement? If yes, non-commercial models stay out for good (this plan assumes yes).
3. Cloud chat models with the user's own key: offered as an option, or local only?
4. Which real part is the M2 demonstration, and can its scan be published?
5. The WP9 items already waiting: baseline v2, the steady force-based route, and the dataset download list.
