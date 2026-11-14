# Vernon user guide

Vernon is a 3D tool for designing 3D-printed car body kits. You start from a
scan or a mesh, and each step is a box (a **node**) you connect to the next.
Every box calculates something and writes a short list of numbers (a
**report**). Some of those numbers have limits (**gates**). A gate that fails
tells you the part is not ready for the next step.

This guide is for someone who has never used a node-graph program. It covers
the Windows package (version 0.1.0-alpha). It is pre-alpha: menus, file
formats and parameter names can still change.

The three tutorials in this folder each run on the example scene
`examples/bodykit_pipeline.vrn`:

- [Scan to print](tutorial-scan-to-print.md): check a scan against the design
  and prepare the parts for a print bed.
- [Aero check](tutorial-aero-check.md): what the airflow stage does and does
  not give you in the example.
- [Mount strength](tutorial-mount-strength.md): what the structural stage
  writes for the mounts, and where it stops.

Before you use any number from Vernon for a decision, read
[What is not validated](../VALIDATION.md#what-is-not-validated).

## 1. What you need

- Windows 10 or 11, 64-bit. No administrator rights are needed for the
  per-user install.
- Optional, only for some steps: other open-source programs (see section 9).
  Vernon works without them. It writes their input files and stops there.

## 2. Install

The package is `Vernon-0.1.0-win64.zip`, with a SHA-256 file next to it. You
can check the download against that file with `Get-FileHash` in PowerShell.

1. Unzip the file to a folder you can write to, such as `Documents`. You get
   one folder, `Vernon-0.1.0-win64`, with these parts:

   | Folder or file | What it is |
   |---|---|
   | `bin\vernon.exe` | The desktop app |
   | `bin\vernon-cli.exe` | The command-line tool (cooks scenes, lists nodes) |
   | `examples\bodykit_pipeline.vrn` | The example scene used in the tutorials |
   | `docs\` | The node catalogue (`NODES.md`) and the behaviour specs |
   | `tools\`, `agent\` | Notes on optional programs and on connecting an AI model |
   | `install.ps1`, `install.cmd` | The installer |
   | `MANIFEST.json` | Every file with its size and SHA-256 |

2. Run the installer. Either right-click `install.ps1` and choose
   **Run with PowerShell**, or double-click `install.cmd`. From a terminal:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\install.ps1
   ```

   The installer adds a **Vernon** entry to the Start Menu and makes `.vrn`
   files open in Vernon. Options:

   - `-AddToPath` puts `vernon-cli` on your user PATH.
   - `-InstallRoot DIR` changes the install folder (default
     `%LOCALAPPDATA%\Programs\Vernon`).
   - `-NoShortcut`, `-NoFileAssociation` skip those steps.

3. Or skip the installer and run Vernon from the unzipped folder. Run
   `install.ps1 -Portable`, or create an empty file named `portable.txt`
   next to `bin\`. Settings then go to `data\config` in that folder, and
   nothing is written to the registry.

To remove an installed copy, run `uninstall.ps1` or use Settings > Apps.

The package is unsigned. Windows may warn about it the first time you run it.

## 3. First launch

Start Vernon from the Start Menu, or run `bin\vernon.exe`.

With an empty scene you see a **Welcome to Vernon** panel with three buttons:

| Button | Use it to |
|---|---|
| **Open the demo (body-kit pipeline)** | Load the example scene. This is the easiest start. |
| **Import a mesh or scan (OBJ, STL, PLY, 3MF, XYZ, E57, LAS)** | Bring in your own file. |
| **Open a saved scene (.vrn)** | Open a scene you saved earlier. |

Click **Open the demo**. The scene loads and runs by itself. The viewports show a curved car panel, and the scene list on the left
fills with the names of the steps. If the window stays blank, close Vernon and
start it again with the environment variable `VERNON_RENDERER=cpu` set. That
uses the slower CPU drawing path.

If you open a scene from a file and Vernon shows **Trust This Scene?**, read
section 9 before you click anything.

## 4. The screen

| Part | Where | What it is for |
|---|---|---|
| Menu bar | Top | **File**, **Edit**, **Create**, **Modifiers**, **Pipeline**, **Views**, **Help** |
| Viewports | Centre | Four 3D views of the scene by default (the quad layout). Drag the middle mouse button to pan, hold Alt and drag with the middle button to orbit, and use the wheel to zoom about the cursor. Right-click for the viewport menu. |
| Scene list | Left | One row per object. Each row shows the name, the gate state, the stage and the last run. Click a row to select it in the viewports too. A row you click in the scene list selects its object. |
| Command panel | Right | Six tabs: **Create**, **Modify**, **Hierarchy**, **Motion**, **Display**, **Utilities**. **Modify** shows the parameters of the selected node. |
| Time bar | Bottom | Only matters for animated results. You can ignore it for the tutorials. |
| Status bar | Bottom | The last message, such as "Cook finished" or a warning. |

Windows you can open from the **Pipeline** menu:

- **Gate Dashboard**: every limit in the scene, with its value and margin.
- **Report Viewer**: the full numbers of one node.
- **Node Editor**: the graph itself, with one box per node and wires between
  them. Use it to add or change a connection.
- **Agent Chat**: an optional chat panel for an AI model. You do not need it.

**Pipeline > Cook All** runs the whole scene again. **Pipeline > Replace Scene
with Demo** clears the scene and loads the example. **Pipeline > Add Demo
Copy** adds a second copy of the example next to the first.

Keyboard shortcuts are in **Help > Keyboard Shortcuts**. The common ones are
Ctrl+Z to undo a scene change, Shift+Z to undo a view change, Ctrl+O to open
and Ctrl+S to save.

## 5. Nodes, reports and cooking

- A **node** does one job, such as reading a file, cutting a shape or
  calculating a drag estimate. It has parameters (numbers and options you can
  change in **Modify**) and inputs and outputs (the wires in the Node Editor).
- **Cooking** means running the nodes in order, from the first to the last.
  Vernon keeps the result of each node. When you change one parameter, only
  that node and the nodes after it run again.
- A node that produces numbers produces a **report**. The report appears in
  the Report Viewer, and its metrics are the values the gates check.
- The catalogue of all nodes, by stage, is in `docs/NODES.md`. The command
  `vernon-cli list` prints the same list in a terminal, and
  `vernon-cli info <node>` prints one node's inputs and parameters.

The stages, in order, are: geometry basics, capture (scans and photos), clean
and align, mesh to surface, split and print, airflow (CFD), structure (FEA),
solar and heat, flattening, and performance.

## 6. Gates, and what to do when one fails

A **gate** is a limit on one number in a report, for example "the 95th
percentile of the scan deviation is at most 0.2 mm". Each gate has a value, a
limit and a margin. A passing gate has a margin of zero or more. A failing gate
is shown as FAIL with the value, the limit and how far it is out.

When a gate fails, do this:

1. Open **Pipeline > Gate Dashboard**. It lists every gate, the stage it belongs
   to and the node that owns it. The row shows the value and the limit.
2. Click the node's name in the dashboard. Vernon selects that node and opens
   its parameters in **Modify**.
3. Read the node's message and its **Next:** hint, if it has one. Hints are
   plain steps such as "Set the File in the Modify tab, or use File > Import
   Mesh."
4. Decide which of these is true:
   - The input is wrong. For example, the scan is in the wrong units, or the
     scan has not been aligned yet. Fix the input or the step before it.
   - The limit is too strict for the job. Change the limit or the tolerance
     on purpose, and write down why. The tutorials show this with the scan
     tolerance.
   - The step does not apply to this part. Some example gates are known and
     expected, as explained below.
5. Cook again (**Pipeline > Cook All**) and check the gate.

Some gates in the example are listed as **expected**. The scene's author
recorded them: the single panel has no wheels, the built-in solid mesher is a
placeholder, and the airflow case is written but not solved. An expected gate
is not a pass. It marks a step that the example cannot finish. Read the reason
text before you rely on the node.

The command-line cook exits with one of three codes:

| Exit code | Meaning |
|---|---|
| 0 | Every node cooked and every gate passed or is listed as expected. |
| 1 | A node failed. |
| 3 | A gate failed that the scene does not list as expected. |

`--ignore-gates` makes the exit code 0 even when a gate fails. Use it only to
get the outputs of a run you are studying. It does not make a failed gate pass.

## 7. Units and axes

- All lengths are in millimetres (mm). Physics uses SI units, and every such
  value shows its unit next to it.
- The Z axis points up.
- Vernon checks the size of each imported file. A longest side under 20 mm, or
  over 50,000 mm, gets a warning that the file may be in metres or inches (or
  already in millimetres). If so, set **File unit** in the **Modify** tab to
  the right unit (`m`, `in` or `mm`).
- Scale is not checked against a real measurement. A scan that has a wrong
  scale stays wrong until you fix it with a scale bar or a known length.

## 8. Files

Vernon reads:

- Meshes and scans: OBJ, STL, PLY, 3MF, XYZ and PTS, E57, LAS.
- Scenes: `.vrn` files, read and written by **File > Open** and **File > Save**.
- Photogrammetry output and other program outputs, through their own nodes
  (see section 9).

Vernon writes:

- Meshes: OBJ, STL, PLY, 3MF and STEP (for CAD).
- 3MF print packages with their parts, from the print nodes.
- Input files for other programs: OpenFOAM case folders, CalculiX `.inp`
  decks, and the slicer and G-code inputs.
- Reports and the cooked outputs of each node, in a folder named
  `<scene>.vrn.work` next to the scene file. The command-line tool takes
  `--work DIR` to choose another folder.

A scene file keeps the node graph and parameters. It does not keep the cooked
geometry, so cook again after you open a scene.

**File > Export Selected…** writes the selected object to a file.

## 9. Programs Vernon runs, and the trust prompt

Some nodes can start other programs, or write files at paths you set. They
are:

- Nodes that call an open-source solver or mesher: the structure nodes (CalculiX),
  the solid mesher (fTetWild), the CFD solver path (OpenFOAM, through WSL), the
  photogrammetry nodes (COLMAP) and the slicer nodes.
- Two optional AI model nodes: a part generator from an image, and an
  aerodynamic preview. These run a separate model install that you point
  Vernon to with the `VERNON_AI3D_ROOT` setting. Their output is a screening
  aid, not a measurement.

Programs are not started on their own. A node that can start one has a
**Run** switch, which is off by default. Vernon only starts the program when
the switch is on and the program's executable exists. Without the program, the
node writes its input files and reports that it did not run.

Install a program and set its path in **Utilities > External Tools**. Each
external program keeps its own licence. Vernon only starts it as a separate
process.

**Trust.** A scene you open from a file can ask Vernon to start a program or
write to a path the file chose. Until you trust the scene, Vernon will not do
either thing. It holds those steps and shows the message "held until the scene
is trusted". When that happens, **Trust This Scene?** opens and lists exactly
what the scene would start or write. Read the list. Then:

- **Trust this scene** allows those actions for this scene.
- **Not now** keeps them held.

You can also choose **File > Trust Scene…** at any time. The demo scene sets no
program paths and no output paths, so opening it should not ask for trust.
Trust only matters for programs and writes to paths the scene chose.

## 10. Where to go next

- Work through [Scan to print](tutorial-scan-to-print.md) first. It uses only
  the geometry, clean, surface and print stages, which are the most complete.
- Then read [Aero check](tutorial-aero-check.md). It shows how a value that is
  entered by hand flows through to a performance number.
- Finish with [Mount strength](tutorial-mount-strength.md). It shows the
  structural inputs and where the example stops.

When a step in a tutorial does not match what you see, the tutorial is wrong
or the build is different. Check the version in the **Help > About Vernon**
window and the file `VERSION.txt` in the package.

## Glossary

- **Node**: one step in the graph.
- **Cook**: run the graph (or one node and what it needs).
- **Report**: the numbers a node writes.
- **Metric**: one number in a report.
- **Gate**: a limit on a metric, with pass or fail.
- **Expected gate**: a gate the scene's author marked as known to fail.
- **White-box node**: a node whose algorithm is a placeholder. It cooks, and it
  says so in a warning.
- **Scene**: a `.vrn` file with its nodes and parameters.
