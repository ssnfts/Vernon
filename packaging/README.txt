VERNON - quickstart
====================
Open-source node-graph 3D toolkit: scan -> surface -> print -> simulate.
Windows 10/11, 64-bit. Pre-alpha. Licence: Apache-2.0 (see LICENSE, NOTICE,
THIRD_PARTY_LICENSES.md). Version and build: see VERSION.txt.

1. INSTALL (no administrator rights needed)
   Right-click install.ps1 > Run with PowerShell, or double-click install.cmd.
   From a terminal:
       powershell -ExecutionPolicy Bypass -File .\install.ps1
   Options:
       -AddToPath          put vernon-cli on your user PATH
       -InstallRoot DIR    default %LOCALAPPDATA%\Programs\Vernon
       -NoShortcut, -NoFileAssociation
   You get a Start Menu entry "Vernon" and .vrn files open in Vernon.
   Remove it all with uninstall.ps1 (or Settings > Apps > Vernon).

   PORTABLE: run  .\install.ps1 -Portable  (or create an empty portable.txt
   next to bin\). Run bin\vernon.exe straight from this folder; settings go
   to .\data\config and nothing is written to the registry or %APPDATA%.
   (Unzip somewhere writable, e.g. a USB stick or your Documents folder.)

2. OPEN THE EXAMPLE
   Start Vernon from the Start Menu, then File > Open... and choose
   examples\bodykit_pipeline.vrn. Or double-click that file once the .vrn
   association exists, or run:  bin\vernon.exe examples\bodykit_pipeline.vrn
   If the window does not open, set the environment variable
   VERNON_RENDERER=cpu and start again.

3. COOK IT FROM THE COMMAND LINE
       bin\vernon-cli.exe list                       (all node types)
       bin\vernon-cli.exe cook examples\bodykit_pipeline.vrn
   It prints one line per node. Exit code 0 = everything cooked and every
   gate passed, 1 = a node failed, 3 = a report limit (gate) failed. The
   example is expected to report 2 failing gates; that is by design.
   Outputs go to a "<file>.vrn.work" folder beside the .vrn file (use --work
   DIR to choose). Install the examples somewhere writable.

4. WHERE THE LOGS AND SETTINGS ARE
   Vernon has no log file yet: messages go to the console (stderr) of the
   program that was started, so run vernon.exe or vernon-cli.exe from a
   terminal to see them. Node results and warnings are in the app's report
   panel and in the cook output. Settings (keymap.json) live in
   %APPDATA%\Vernon, or .\data\config in portable mode.

5. OPTIONAL PROGRAMS AND AI MODELS
   tools\README.md   COLMAP, OpenFOAM, CalculiX, fTetWild, slicers, Klipper
                     (not included; separate downloads under their own licences)
   agent\README.md   connect a local or cloud model
   docs\             node catalogue (NODES.md) and the behaviour specs

Folder map: bin\ programs | examples\ | docs\ | tools\ | agent\ | setup\ (installer
helpers) | MANIFEST.json lists every file with its size and SHA-256.
