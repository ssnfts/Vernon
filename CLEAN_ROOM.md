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
| COLMAP | BSD-3-Clause | photogrammetry |
| CloudCompare (CLI) | GPL-2.0+ | optional reference deviation checks |
| OpenFOAM | GPL-3.0 | CFD solver |
| CalculiX | GPL-2.0 | FEA solver |
| OrcaSlicer / PrusaSlicer | AGPL-3.0 | slicing and G-code |
| Klipper | GPL-3.0 | printer firmware (target only) |

Vernon writes their input files and reads their output files. It does not
copy their source code.
