# Optional external programs

Vernon runs without any of these. Some nodes can drive an open-source program
as a **separate process** when you ask them to. None of them is bundled in
this package: several are GPL or AGPL, which is why they stay separate
downloads under their own licences. Vernon writes their input files and reads
their output files; it never links them.

To use one: install it, then set the node's **executable path** parameter to the
program and switch its **run** parameter on. With `run` off (the default) the
node only writes the input files, so you can check them or run the program
yourself. If the path is wrong the node reports it and does not start anything.

| Program | Licence | Used by | Download | Point Vernon at |
|---|---|---|---|---|
| COLMAP | BSD-3-Clause | photo-to-mesh (`capture.sfm_*`, `capture.mvs_*`) | https://colmap.github.io/ (Windows builds on the project's GitHub releases) | `colmap.exe` (or `COLMAP.bat`) |
| OpenFOAM | GPL-3.0 | CFD cases (`cfd.*`) | https://www.openfoam.com/ or https://openfoam.org/ (Windows via WSL2 or a community build) | the solver launcher script named in the node (`Allrun` files are written for you) |
| CalculiX | GPL-2.0 | FEA decks (`fea.*`) | https://www.calculix.de/ | `ccx.exe` |
| fTetWild | MPL-2.0 | tetrahedral meshing (`fea.tet_mesh`) | https://github.com/wildmeshing/fTetWild | `FloatTetwild_bin.exe` |
| OrcaSlicer / PrusaSlicer | AGPL-3.0 | slicing, G-code (`print.slice_external`) | https://github.com/SoftFever/OrcaSlicer, https://github.com/prusa3d/PrusaSlicer | the slicer's console executable |
| Klipper | GPL-3.0 | printer firmware: Vernon only writes macros for it | https://www.klipper3d.org/ | not run by Vernon; copy the generated macros to your printer |

Notes
- Check the program's own page for the current version and Windows
  availability; the links above are starting points, not endorsements.
- Keep these programs outside Vernon's install folder so that upgrading or
  uninstalling Vernon never touches them.
- Licence obligations of each program apply to you when you download and
  redistribute it. This package contains none of their code or binaries.
- The `nodes` that use them are listed in `docs\NODES.md` (look for a `run`
  parameter and an executable path).
