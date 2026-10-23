# Clean-room policy

Vernon re-implements the *behaviour* of commercial tools used in the scan →
surface → print → simulate body-kit pipeline. It never contains their code.
Every contributor (human or AI) follows these rules.

## Allowed sources
1. Public documentation, help pages, manuals and tutorials published by the vendor.
2. Peer-reviewed papers, textbooks and standards (cite them in the spec).
3. Observing a legally licensed copy of a tool you own: feed it inputs you
   made yourself and record the outputs (golden tests).
4. Open-source projects, used under their licences (see *External tools*).

## Forbidden
- Decompiling, disassembling, debugging, memory-dumping or patching any
  proprietary binary, plug-in, script bundle or file in an install folder.
- Copying vendor documentation text, screenshots, icons, UI artwork, sample
  files or help strings. Specs paraphrase in our own words and cite the URL.
- Vendor trademarks in the product UI. Node labels describe what a tool does
  (`Watertight Wrap`), never whose tool it imitates. The `emulates` field in
  `NodeTypeInfo` and the specs in `docs/specs/` may name the reference tool
  for documentation only.
- Any file from a vendor's installation directory in this repository.

## Process ("two-team" separation)
1. **Spec writers** produce `docs/specs/<tool>.md` from allowed sources only.
   Each spec lists features, inputs, outputs, parameters, tolerances and the
   sources consulted.
2. **Implementers** build nodes from the spec plus published algorithms. A
   node's `spec` field points at the spec it implements.
3. **Golden tests** compare Vernon's output against recorded output of the
   reference tool on inputs we created, within stated tolerances.

## External tools
Open-source programs that already do a stage well are driven as **separate
processes**, not linked into Vernon, so Vernon's own code stays Apache-2.0:

| Tool | Licence | Used for |
|---|---|---|
| COLMAP | BSD-3-Clause | photogrammetry (`capture.sfm_*`, `capture.mvs_*`) |
| OpenFOAM | GPL-3.0 | CFD solver (`cfd.*` case writers and the optional run) |
| CalculiX | GPL-2.0 | FEA solver (`fea.*` deck writers and the optional run) |
| fTetWild | MPL-2.0 | tetrahedral meshing of watertight surfaces (`fea.tet_mesh`); driven as a process, no source copied, so the file-level copyleft never reaches Vernon's files |
| OrcaSlicer / PrusaSlicer | AGPL-3.0 | slicing and G-code (`print.slice_external`) |
| Klipper | GPL-3.0 | printer firmware (target only: Vernon writes macros, never links it) |
| CloudCompare (CLI) | GPL-2.0+ | not driven by any node: the stage-2 deviation checks are native. Listed so a user can cross-check a result by hand |
| Instant Meshes / QuadriFlow | BSD-3-Clause / MIT | not driven by any node: `surface` quad remeshing is native (`surface.quad_remesh`). Candidates for an optional external route; licences are as recalled and must be verified against each repository before a node drives either one |

Vernon writes their input files and reads their output files. It does not
copy their source code. Any new process-driven tool is added to this table,
with its licence, in the same change that adds the node.

## Dependencies and build scripts
Rust dependencies are pure Rust under MIT, Apache-2.0, BSD, Zlib, ISC,
Unicode, BSL-1.0 or CC0 (the bundled fonts carry OFL/Ubuntu licences). No
copyleft crate is linked and no dependency may compile C or C++ in its build
script (no `cc`, `cmake` or `bindgen` in the resolve for a supported target).

`Cargo.lock` lists the `cc` crate because `android-activity` and
`wayland-backend` name it as an optional or target-specific dependency. It is
not in the Windows resolve. The app turns eframe's default `wayland` and `x11`
features off, so the Wayland stack is only pulled in when a Linux build opts
in. Before shipping a Linux build run

    cargo tree -e normal,build --target x86_64-unknown-linux-gnu -i cc

and, if `cc` appears, read the build script of the crate that pulls it in. A
crate that only locates a system library (`pkg-config`, `dlopen`) is fine; one
that compiles C is not, and the feature that needs it stays off.
