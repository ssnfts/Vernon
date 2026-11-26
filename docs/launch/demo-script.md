# 90-second demo script (DRAFT)

Scene: `examples/bodykit_pipeline.vrn`, opened with the start panel's demo button. Screen
recording with voice-over. Nothing recorded, uploaded or posted. All numbers below come from a cook
of that file on 2026-11-14 (40 nodes, 0 failed, 5 expected gates). Check each number against the
screen on the day; if the build changed, the screen wins and this script is edited.

Prep: empty scene so the start panel shows. No other windows. Dark or light theme, either.

| Time | Shot | Voice-over |
|---|---|---|
| 0:00-0:08 | App opens on the start panel ("Welcome to Vernon", three buttons). Hold, then hover the first button. | "Vernon is an open-source node graph for 3D-printed car body kits. The scene is empty, so it offers a start." |
| 0:08-0:18 | Click "Open the demo (body-kit pipeline)". The graph loads and cooks by itself (panel text: "Cook runs by itself"). Quad viewports show the fender. | "The demo is a synthetic fender. It goes through scan, clean, surface, split, airflow, structure, heat and performance. Forty nodes." |
| 0:18-0:32 | Open the modifier stack / command panel on the alignment node, then the deviation node. Show the colour map on the panel. | "A noisy, misplaced copy of the panel plays the scan. It is aligned, then measured against the reference. The ninety-fifth percentile deviation is 0.0994 millimetres against a 0.2 limit, so this gate passes." |
| 0:32-0:45 | Click the bed-fit node, then the heat-deflection node, in the stack. Zoom on each report. | "Later stages check their own limits. The split parts clear the printer bed by about 66 millimetres. The surface temperature stays about 17 kelvin under the material's heat-deflection temperature." |
| 0:45-0:55 | Pipeline menu, Gate Dashboard. Window opens. Pause on the top line. | "This is the Gate Dashboard. Every report metric, its value, its limit and its margin." |
| 0:55-1:25 | Scroll to the five expected rows. Zoom on the text after "(expected: ...)". Read each in turn, briskly. | "Five gates are marked expected. The scene's author listed them, with reasons, and the dashboard shows the reason. One: the demo is a single panel, so it has no wheels. Two: the built-in solid mesher is a placeholder. Three: the airflow case is written but the solver has not run, so there are no force results. Four: there is no solver history to read forces from. Five: the drag and downforce areas in the performance table are typed in by hand until the solver has run. None of these are hidden, and anything else failing would stop the cook with an error code." |
| 1:25-1:30 | Cut to a plain title card: "Not certified engineering software. Pre-alpha." with the repository location. | "Vernon is pre-alpha and not certified engineering software. What is and is not validated is written down in the repository." |

## Shot notes
- Do not show any drag or downforce number from the demo: the airflow case is unsolved, so there is none.
- Do not say "validated", "accurate" or "safe". Say "within the limit".
- The expected-gate reasons are in the dashboard text; if its wording differs from the voice-over,
  re-record the voice-over, not the screen.
- Optional 10-second cut (if over time): drop the 0:18-0:32 shot and keep the dashboard.
- The command-line equivalent for a b-roll shot: `cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn`, ending
  with "40 nodes cooked, 0 failed" and "gates: 0 failing, 5 expected".
- Voice-over runs about 230 words; read it once with a timer before recording. It may need trimming.
