# AI Convergence Forecast (`cfd.ai_convergence_forecast`)

Emulates: time series forecasting of solver residual histories.

## What it does

Takes one or more number series (residuals or any monitored value) and forecasts
where each may head over the next N steps, with 10 / 50 / 90 percent bands. It also
reports the first forecast step where the median falls below a target level.

## What it does not do

- It is advice only. It is not a convergence check. No acceptance gate reads it, and it
  never stops, accepts or relabels a solver run.
- The model is a general time-series model. It was not trained on CFD. A forecast is
  not evidence of convergence; use the solver's own residual and monitor criteria.
- Every output carries `"advisory": true`, `"label": "AI forecast: advisory only"` and
  `"cfd_validation": "DEFERRED"`. Report metric names start with `ai_`.

## Model and licence

`amazon/chronos-2`, Apache-2.0, about 0.5 GB. Python package `chronos-forecasting` 2.3.2
with torch 2.15.0 (versions recorded in `installed-linux-packages.txt`). Weights are
read from `<AI-3D>/Chronos/model`; the bridge sets `HF_HUB_OFFLINE=1` and never downloads.
Model revision used: `29ec3766d36d6f73f0696f85560a422f50e8498c`. It uses the GPU when
CUDA is available and the CPU otherwise.

## Install

Inside the Linux environment, as root:

    bash scripts/ai/install_chronos.sh --yes

It prints model, licence and download size, and refuses to run without `--yes`. It creates
`/opt/ai3d/Chronos` (Python 3.11, via uv), installs the packages, downloads the weights
to `<AI-3D>/Chronos/model`, and writes `installed-linux-packages.txt` and
`model-revision.txt` next to them. Re-running reuses what is there.

## Inputs

- `series` (Json, optional): `{"series": {"name": [numbers]}}` or `{"name": [numbers]}`.
- `log_path` (optional): an OpenFOAM-style solver log. Lines of the form
  `Solving for Ux, Initial residual = 0.0123, Final residual = ..., No Iterations 3` give
  one series per field, taking the first occurrence of each field inside each `Time = N` block.
- `ai_root`, `wsl_distro` (default `AI-3D`), `horizon` (default 200, at most 1024, the
  model limit), `log_transform` (default on: forecast log10, needs strictly positive
  values), `target` (default 1e-5), `timeout_s`, `run`.

Refused, each with its own message: no series; a series shorter than 32 points; a
non-finite value; more than 16 series; a value <= 0 with the log transform on.

## Outputs

- `forecast` (Json): per series the context length, `q10` / `q50` / `q90` arrays in the
  original units, and a `crossing` estimate (`crosses` with a 1-based step,
  `already_below`, or `not within horizon`), plus the advisory fields above.
- `report`: first note is the label; metrics `ai_series_count`, `ai_horizon`,
  `ai_<name>_context_length`, `ai_<name>_median_final`, `ai_<name>_crossing_step`,
  `ai_peak_vram_gb`, `ai_peak_rss_gb`, `ai_seconds_load`, `ai_seconds_infer`.
- `manifest`: model, revision, device, horizon, memory and timings.

The work folder is `forecast_<hash of series, horizon and transform>`. Without `run` the
node only reloads an earlier result for identical inputs.

## Measured check (single run)

Synthetic series `y_i = 10^(-1 - 0.004 i) * (1 + 0.05 sin(0.3 i))`, i in 0..600, horizon 200,
log transform on (RTX 4090 Laptop, CUDA):

- Median at the last step: 6.52e-5. True continuation `10^(-1 - 0.004*799)`: 6.37e-5. Ratio 1.023
  (tolerance was a factor of 3).
- q10 / q50 / q90 at the last step: 5.77e-5 / 6.52e-5 / 9.09e-5. q10 <= median <= q90 held at all 200 steps.
- Model load 5.8 s, inference 23.3 s, peak GPU memory 0.49 GB, peak process memory (RSS) 1.43 GB.
  The whole test took 215 s because the first start imports torch from a busy slow disk
  while a CFD run was active. A first smoke run of the bridge alone took 74 s to load and
  70 s to infer under the same load, so timings vary a lot on this machine.
- This is one run on one synthetic series. It shows the plumbing works and the model can
  extend a smooth log-linear decay. It says nothing about real solver residuals, which
  stall, oscillate and restart.

## Limits

- At most 1024 forecast steps and 8192 points of history (older points are dropped);
  at most 16 series, each forecast on its own.
- Series are treated as evenly spaced; the step is one logged iteration or one sample.
- Residuals that plateau, jump or change regime are exactly what a general model forecasts badly.
