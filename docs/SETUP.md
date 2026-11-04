# Setting up Vernon on a new Windows PC

This takes a fresh Windows 10/11 x64 machine to a passing build (parts 1–3).
Parts 4–6 then set up the WSL + OpenFOAM environment and the DrivAer data
needed for the CFD validation stages in [CFD-RUNBOOK.md](CFD-RUNBOOK.md).
Run everything from a normal, **unelevated** PowerShell 7 window unless a
step says otherwise.

## Hardware for the CFD stages

The app and the tests run on any recent PC. The fine-grid CFD work needs:

| | Minimum | Why |
|---|---|---|
| RAM | 32 GB | The 12M-cell mesh peaked at 20.7 GB inside WSL. The memory policy needs at least 23 GiB available in WSL. |
| CPU | 16+ cores | The solver runs as 16 MPI ranks. The reference machine is an i9-13900HX (24 cores, 32 threads). |
| Disk | 150 GB free | Each sizing run copies about 1 GB of surfaces. A solver run writes several 12M-cell time steps. |
| Power | Mains, sleep disabled | Sleep suspends the solver, and the wall-clock timeout then kills the run. |

## 1. Tools

```powershell
winget install Git.Git
winget install Microsoft.PowerShell
winget install Rustlang.Rustup
winget install BrechtSanders.WinLibs.POSIX.UCRT
```

Open a new PowerShell 7 window (`pwsh`) so the new PATH applies, then:

```powershell
rustup toolchain install stable-x86_64-pc-windows-gnu
rustup default stable-x86_64-pc-windows-gnu
```

## 2. Clone

Clone into an **ordinary folder** on a local drive. Don't use a junction,
symlink, network share or OneDrive folder: the CFD input checks refuse linked
paths. External tools (WSL distro, OpenFOAM records) go into a `tools` folder
*next to* the checkout, so pick a parent folder with room:

```powershell
mkdir C:\dev; cd C:\dev
git clone https://github.com/ssnfts/Vernon.git vernon
cd vernon
.\scripts\setup\setup-dev.ps1 -WriteCargoConfig
```

`setup-dev.ps1` installs nothing. It reports what is missing and, with
`-WriteCargoConfig`, writes `~/.cargo/config.toml` so the GNU target links with
WinLibs' `gcc` and `dlltool`. It only does this when that file doesn't exist yet.

## 3. Build and test

```powershell
. .\scripts\dev-env.ps1        # in every new shell
cargo build --workspace
cargo test --workspace         # expect about 1543 passed, 0 failed, 35 ignored
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn   # 40 nodes, 0 failing, 5 expected gates
cargo run -p vernon            # opens the app
```

Or run the same queue as a background job with saved logs:

```powershell
.\scripts\verification\test-verification.ps1 -LibraryChecksOnly
.\scripts\verification\start-verification.ps1 -Profile full -RunId full-YYYYMMDD-a -JobTimeoutSeconds 2400
.\scripts\verification\verification-status.ps1 -Json
```

Ignored tests need external programs or data and are run explicitly through
the verification profiles.

## 4. WSL, Ubuntu 24.04 and OpenFOAM v2606 (CFD only)

The setup scripts write their records and the distro into `..\tools\wsl`
(beside the checkout). They never restart Windows.

1. **WSL runtime.** Check `wsl --version`. If it reports 3.0.3 or newer, skip
   to step 2. Otherwise run the following, approve the UAC prompt, and restart
   Windows yourself if it asks:

   ```powershell
   .\scripts\setup\setup-wp9-wsl.ps1
   ```

   It installs Microsoft's signed WSL 3.0.3 MSI, checking its pinned hash and
   Authenticode signature.
2. **Project distro.** This registers `Vernon-Ubuntu-24.04` under
   `..\tools\wsl\ubuntu-24.04` for the current user:

   ```powershell
   .\scripts\setup\setup-wp9-ubuntu.ps1
   ```

3. **OpenFOAM.** Two phases. `inspect` reads the signed OpenCFD repository and
   records the exact candidate package in `..\tools\wsl\openfoam\`. Review
   `candidate.txt` and `install-simulation.txt` there, then run `install`. It
   installs exactly that candidate and creates the unprivileged `vernon` Linux
   user (its password stays locked).

   ```powershell
   .\scripts\setup\setup-wp9-openfoam.ps1 -Mode inspect
   .\scripts\setup\setup-wp9-openfoam.ps1 -Mode install
   ```

   The reference machine used `openfoam2606` version `2606.0~rc2-1`. If the
   repository now offers a later 2606 build, note the version in your run
   records. The install accepts any `2606*` candidate.
4. **Smoke test.** A 320-cell duct, which proves the tools run; this isn't
   validation. Replace `C:\dev\vernon` with your checkout, keeping
   `/mnt/<drive letter>/...`:

   ```powershell
   wsl -d Vernon-Ubuntu-24.04 -u vernon -- bash /mnt/c/dev/vernon/scripts/cfd/wp9-cfd-smoke.sh /mnt/c/dev/vernon/scratch/wp9-setup/duct-smoke-1
   ```

## 5. WSL memory

By default WSL gets half the host's RAM. The fine mesh needs more. Create
`%USERPROFILE%\.wslconfig` (a 32 GB host leaves Windows about 6 GB, so close
large apps during runs):

```ini
[wsl2]
memory=26GB
```

Then run `wsl --shutdown`, and check with
`wsl -d Vernon-Ubuntu-24.04 -u vernon -- grep MemAvailable /proc/meminfo`.
It must show at least 24117248 kB (23 GiB). Policy and live-environment
probes (no CFD runs):

```powershell
$repo = '/mnt/c/dev/vernon'   # your checkout as a WSL path
wsl -d Vernon-Ubuntu-24.04 -u vernon -- bash --noprofile --norc "$repo/scripts/cfd/test-wp9-sizing-policy.sh" "$repo/scripts/cfd/wp9-replication-sizing-env.sh"
wsl -d Vernon-Ubuntu-24.04 -u vernon -- env -i HOME=/home/vernon PATH=/usr/bin:/bin bash --noprofile --norc "$repo/scripts/cfd/test-wp9-sizing-environment.sh" "$repo/scripts/cfd/wp9-replication-sizing-env.sh"
.\scripts\cfd\test-wp9-sizing-cleanup.ps1
```

Expected: `passed=27 failed=0`, `passed=24 failed=0 blocked=0`, and `PASS`.

## 6. DrivAer surfaces

The validation uses TUM's DrivAer STL archive (`STL_complete.zip`, 590,018,283
bytes). TUM gives it out after registration at the Chair of Aerodynamics and
Fluid Mechanics DrivAer page. Download it yourself; it is licensed to you, and
must never be committed or redistributed. Then:

```powershell
.\scripts\setup\prepare-drivaer-data.ps1 -Archive D:\downloads\STL_complete.zip
```

This checks the archive's SHA-256. It then extracts the eight parts the study
uses (body, detailed underbody, the three rear ends, wheels and mirror) into
`scratch\wp9-data\tum-drivaer\extracted`, and checks each against
[docs/research/wp9-replication-baseline-v1.json](research/wp9-replication-baseline-v1.json).
It prints the `VERNON_CAR_DIR` value to use.

You're ready for [CFD-RUNBOOK.md](CFD-RUNBOOK.md).
