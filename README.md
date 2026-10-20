# Vernon

Open-source, node-based 3D toolkit for the **scan → surface → print → simulate**
workflow used to design 3D-printed car body kits. Vernon has a familiar
DCC-style interface (quad viewports, command panel, modifier stack) and a
procedural node graph underneath.

Status: **pre-alpha / white-box**. Every pipeline tool exists as a node with
real inputs, outputs and parameters; many still run placeholder algorithms
(`whitebox: true`) while their clean-room implementations are written.

## Pipeline stages
| Stage | What Vernon does |
|---|---|
| 1 Capture | Import scans (PLY/OBJ/STL/XYZ), drive COLMAP photogrammetry, scale correction |
| 2 Clean / Align | Decimate, ICP alignment, watertight wrap, signed deviation with percentile report |
| 3 Mesh → Surface | Quad remesh, SubD, auto-surfacing, mesh sections, curve fitting, STEP hand-off |
| 4 Split / Print | Body-line splitting, dovetail/dowel keys, grid splitting, bed fitting, 3MF, slicer hand-off |
| 5 Airflow (CFD) | OpenFOAM case generation with a validated automotive protocol, CdA/ClA, Cp maps |
| 6 Structure (FEA) | CalculiX decks with anisotropic print properties, safety factor at mounts |
| 7 Solar / Heat | Incident radiation from a sky model, surface temperature vs heat-deflection temperature |
| 7b Flatten | Doubly-curved panel flattening with stretch maps |
| 8 Performance | Point-mass lap model, ΔCdA/ΔClA, pressure-tap and coastdown validation |

## Building (Windows, GNU toolchain)
```powershell
. .\scripts\dev-env.ps1
cargo run -p vernon
```

## Clean room
Vernon is a clean-room project. Read [CLEAN_ROOM.md](CLEAN_ROOM.md) before contributing.

## Licence
Apache-2.0. External open-source solvers are run as separate processes under their own licences.
