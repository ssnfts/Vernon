# Vernon project plan

Written 2026-11-11; local AI (sections 2, 5 and 7) updated 2026-11-19. This replaces the scattered "what next" notes as the one
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

| Area | State (2026-11-11 unless dated) |
|---|---|
| Node catalogue | 210 nodes, 207 implemented, 3 white-box ([NODES.md](NODES.md)) |
| Pipeline demo | `examples/bodykit_pipeline.vrn` cooks all nine stages on a synthetic fender |
| Desktop app | egui/wgpu, quad viewports, node graph, agent chat panel |
| Agent | `vernon-agent` (OpenAI-compatible client, so any llama.cpp or Ollama server works) and `vernon-cli mcp` |
| AI nodes (2026-11-19) | Eleven, all out of process and labelled as AI output: PartCrafter (image to parts), DoMINO (aero preview and compare), and the nine local additions listed in section 5 (speech in and out, convergence forecast, document search, photo cut-out, vision check, sketch from words, print watchdog, body fit). No gate reads any of them. |
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

Added 2026-11-14 to 2026-11-17 through
[the local AI additions plan](superpowers/plans/2026-11-14-local-ai-additions.md)
(pull requests 3, 5, 7, 9 and 11), each verified once on the real model on the
reference PC. Measured state and limits are in each node's spec under
[specs/ai-local/](specs/ai-local/); the README's Local AI table summarises them.

| Node | Model, licence | Evidence so far |
|---|---|---|
| `geo.ai_text_to_speech` | Kokoro-82M, Apache-2.0 | Works on the CPU; speech round trip 15 of 15 words |
| `geo.ai_speech_to_text` | Whisper large-v3-turbo, MIT | Works; FLAC and WAV only |
| `cfd.ai_convergence_forecast` | Chronos-2, Apache-2.0 | Synthetic series only; not yet run on a real solver log |
| `geo.ai_document_search` | bge-m3, MIT | 5 of 6 topic questions over the specs; no PDF through the real model |
| `capture.ai_photo_cutout` | SAM 2.1 large, Apache-2.0 | Drawn test picture only |
| `geo.ai_vision_check` | Qwen3-VL 4B, Apache-2.0 | Drawn test pictures only; no rendered mesh yet (item K below) |
| `geo.ai_sketch_from_words` | FLUX.2 klein 4B, Apache-2.0 | One prompt; needs about 17 GB of RAM |
| `print.ai_print_watchdog` | Qwen3-VL 4B (shared with the vision check) | 20 print photos, in-sample; no printer camera feed; checks once per cook |
| `capture.ai_body_fit` | SAM 3D Body, SAM and DINOv3 licences (opt-in, not open source) | Sanity check on two photos of one person, not an accuracy test |

Every one of these needs a second, independent test before it can be called
reliable: real solver logs for the forecast, real photos for the cut-out,
rendered meshes for the vision check, a printer camera feed for the watchdog,
and a measured body for body fit.

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
| K | **Vision check of the viewport** | A local vision model describes what a render shows (holes, flipped normals, a part off the car) as a hint next to the numeric report | C | Node built (`geo.ai_vision_check`); still to do: feed it viewport renders and test on mesh defects |
| L | **Fine-tuning a surrogate on Vernon's validated runs** | Predictions for kit shapes outside the stock training set | E, many solved cases | Research; feasibility on 16 GB unverified |

Local model candidates for C, D and K, from the 2026-11-11 search: a 20B
open-weight text model that fits 16 GB for tool calling, an 8B vision model
for K (the node was built on the 4B model in half precision instead), and 7 to
9B models at 4-bit for the 11 GB machine. The sources are
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

Checked on 2026-11-19 against `origin` (`github.com/ssnfts/Vernon`). Pull
request 2 brought `main` up to date on 2026-11-12, and the local AI work landed
through pull requests 3 to 11.

| Branch | Ahead of `main` | Holds | Action |
|---|---:|---|---|
| `claude/wp9-solve-stages` | 67 | WP9 solver stages and DDES work since 2026-11-11 | Merge to `main` by pull request when the CFD work reaches a stopping point |
| `claude/domino-warmstart-m1` | 10 | DoMINO warm start M1, units module, car shell prep | Keep as a branch until M1 is accepted |
| `claude/local-ai-*`, `docs/readme-local-ai-*` | 0 | Already in `main` | Delete on GitHub |
| `qwen/t02` to `qwen/t06` (local) | 1 each | Attempts that failed their own checks; the same modules later landed through other batches | Keep local or delete; not for the public repository |

On 2026-11-12, 20 merged local worktrees and their branches were removed; the
local AI worktrees (`vernon-wt-ai*`) are next once their session is done.

Done since 2026-11-11: GitHub description and topics. Still missing: a CI
workflow, `CONTRIBUTING.md`, `SECURITY.md`, a changelog and any image.

## 8. Decisions needed from the owner

1. Merge `claude/wp9-solve-stages` into `main` now, or wait for the slant-body rehearsal?
2. Is commercial usability a firm requirement? If yes, non-commercial models stay out for good (this plan assumes yes).
3. Cloud chat models with the user's own key: offered as an option, or local only?
4. Which real part is the M2 demonstration, and can its scan be published?
5. The WP9 items already waiting: baseline v2, the steady force-based route, and the dataset download list.
6. Pictures from the printer camera (a folder of frames from real prints, good and failed) so the print watchdog can be tested on what it is meant to watch.
