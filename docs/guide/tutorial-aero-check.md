# Tutorial: the aero check

**Scene:** `examples/bodykit_pipeline.vrn`, the airflow nodes and the
performance nodes that read them.
**Time:** about 30 minutes.
**You will learn:** what the airflow stage sets up from a body, what it
writes for a solver, and how a drag value you type in reaches the top-speed
number. You will also see exactly where the example stops.

Read [the user guide](README.md) sections 5 and 6 first if "node", "report"
and "gate" are new to you.

## The short version

**The example does not run an airflow solver.** It prepares the car, sizes the
virtual wind tunnel, writes a complete OpenFOAM case and then stops. The solver
step is marked as an expected failing gate. Nothing in the example measures a
drag coefficient. The drag figure the performance nodes use is a number typed
into the scene: **CdA = 0.64 m²**. Every speed and power figure below depends
on that number.

## What the airflow stage does

The airflow nodes are in the **Airflow (CFD)** stage. In this example they run
in this order:

| Node (scene name) | What it does |
|---|---|
| `air` | Air density, viscosity and dynamic pressure from temperature, humidity and speed. |
| `vehicle_prep` | Rotates the body into the flow direction and measures its size. It looks for wheels. |
| `frontal_area` | Frontal area from the body's silhouette. |
| `tunnel` | Size of the virtual tunnel around the body and its blockage. |
| `refinement` | Cell sizes and the first-layer height for the wall. |
| `mesh_dicts` | Writes the mesh setup files of the case. |
| `cfd_case` | Writes the steady turbulence model and boundary settings. |
| `cfd_solve` | Would run the solver. Not run here. |
| `cfd_forces` | Would read the force history. No history exists here. |

The performance nodes then use the air state and a vehicle:

| Node | What it does |
|---|---|
| `vehicle` | Mass, power, and the drag and downforce areas. Here they are typed in. |
| `aero_loads` | Drag and downforce at 200 km/h. |
| `top_speed` | The speed at which drag and rolling resistance use all the power. |

## What you need

- Vernon installed, or the repository with a Rust build.
- The example scene, `examples/bodykit_pipeline.vrn`.

Commands use the repository form (`cargo run -p vernon-cli --`). With the
installed package use `bin\vernon-cli.exe`.

## Step 1: check the air

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node air
```

You should see:

```text
air (cfd.air_state)
  report: Air state [PASS]
    rho = 1.0976 kg/m³
    rho_vs_isa_pct = -10.4024 %
    q = 493.907 Pa
    mach = 0.0839   (limit <= 0.3: ok)
```

The scene sets 45 °C, 30 % relative humidity and 30 m/s. The air is about
10 % less dense than the standard sea-level value. That is the expected effect
of the warm air, and it is why the tutorial reports `rho_vs_isa_pct`.

The Mach limit of 0.3 is a check that the flow is slow enough to treat as
incompressible. The example passes it.

## Step 2: check the body and look for wheels

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node vehicle_prep
```

You should see the report `Vehicle preparation [FAIL]`, and this line:

```text
wheels_found = 0 count  (limit >= 4: FAIL)
[expected] vehicle_prep / wheels_found = 0 count (limit >= 4, margin -4 count):
the example prepares a single body panel, which has no wheels
```

The scene is one body panel, so there are no wheels. The gate fails and the
scene says so. The message also says the wheel axle positions and radius are
estimates. The panel's size is measured:

```text
length_m = 0.9031 m
width_m = 0.6078 m
height_m = 0.0716 m
wheelbase_m = 0.5419 m (estimate)
```

A real car needs its wheels in the scene for this step to pass. Without them,
the wheel and ground settings in the case are not set.

Then cook the frontal area:

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node frontal_area
```

It reports `a_front_m2 = 0.0272 m²`, with `bbox_fill_ratio = 0.6253`. The
fill ratio is how much of the bounding box the silhouette covers. The case is a
half model, so its areas are for that one half.

## Step 3: the virtual tunnel

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node tunnel
```

It should report `blockage_pct = 1 %` (limit 1 %), a cross section of
`2.7201 m²`, a width of `2.1292 m`, a height of `1.2775 m` and a length of
`9.0308 m`.

Blockage is the frontal area as a share of the tunnel's cross section. The
limit of 1 % keeps the walls from pushing the flow around the body.

## Step 4: the refinement plan

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node refinement
```

It reports `Refinement plan [PASS]`, with a Reynolds number of `1.5402e6` based
on the body length, a skin friction of `0.0034`, and a first cell height of
`0.4268 mm` for a wall distance value (y+) of 30. The target is set at the
y+ = 30 height. The body cell size is 10 mm, and the finest cell is 5 mm.

## Step 5: the case files

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node mesh_dicts
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node cfd_case
```

`mesh_dicts` writes the mesh setup. Its report includes:

```text
base_cells = 464
first_layer_mm = 2.4113 mm
first_layer_yplus_estimate = 169.4716
surfaces_written = 1
ram_estimate_gb_low = 30 GB, ram_estimate_gb_high = 60 GB
```

The 30 to 60 GB memory figure is an estimate for a mesh at the 30 million cell
limit, as its note says. It is not the memory this 464 cell base mesh would
need. The first layer in this mesh setup is 2.41 mm (y+ about 169), but the
refinement plan asks for 0.43 mm (y+ 30). The example does not reconcile the
two. Treat that difference as an open item, not as a checked result.

`cfd_case` writes the steady turbulence model and the boundary settings:

```text
k_inf = 0.0084 m²/s²
omega_inf = 47.967 1/s
u_ms = 30 m/s
aref_m2 = 0.0136 m²
```

Written files are in the `.vrn.work` folder, under `n20_mesh_dicts\case` and
`n21_cfd_case\native\case`. You can open them in a text editor. They are the
complete input of an OpenFOAM run, but this example does not start one.

## Step 6: the solver is not run

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node cfd_solve
```

The report reads:

```text
Steady solve [FAIL]
iterations_read = 0 count
solve_done = 0   (limit >= 1: FAIL)
[warning] white-box placeholder: run the potential-flow initialisation and the
steady solver ... (run is off or the tools were not found)
```

That is the expected gate for this step. The node writes the case and does not
solve it. Its **run** switch is off, so no program is started.
The message is a placeholder: no solver ran, and no number from it is a result.

`cfd_forces` has the same status. It needs a force history from a solved case:

```text
data_available = 0   (limit >= 1: FAIL)
no solver history: connect a solved case or a recorded force history
```

So the example has no drag coefficient from a solver. Do not read the
`cfd_forces` numbers as results. They are only placeholders.

## Step 7: the drag number the scene actually uses

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node vehicle
```

The report reads:

```text
aero_from_solver = 0   (limit >= 1: FAIL)
CdA = 0.64 m²
downforce_area = 0.1 m²
[warning] the aero input is a placeholder with no solver data; ... the manual CdA
and ClA are used instead
```

`CdA` is drag coefficient times frontal area. Here it is 0.64 m², typed by the
scene's author, and `ClA` is 0.1 m². The gate for this is expected to fail
because nothing came from a solver.

In the app, select the `vehicle` row in the scene list and open **Modify**.
The CdA field (parameter `cda_m2`) is 0.64. This is the value to change if you want to see the
effect of drag on speed.

## Step 8: what drag does to the speed

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node aero_loads
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node top_speed
```

`aero_loads` at the 200 km/h design speed reports:

```text
design_q = 1693.7826 Pa
drag_design = 1084.0209 N
downforce_design = 169.3783 N
power_design = 60.2234 kW
```

`top_speed` reports:

```text
v_max = 263.4731 km/h   (73.187 m/s)
p_aero = 137684.1628 W
p_roll = 12315.8372 W
aero_share = 91.7894 %
```

The top speed is the point where the power of the engine (150 kW in the scene)
equals the drag and rolling losses. Most of the power goes to the air at this
speed (91.8 %). Speed depends on the drag area, so a change in `CdA` moves
the top speed.

**Try it on a copy.** Do not change the file in `examples\`. Copy the scene
to a practice folder. In the copy, change `"cda_m2"` in the node named
`vehicle` from `0.64` to `0.50`, then cook `top_speed` with the copy's path. In
our run on this PC:

```text
v_max = 285.3275 km/h   (79.2576 m/s)
dvmax_dcda_kmh_per_m2 = -183.9542 km/h per m2
```

A 0.14 m² lower drag area gives about 22 km/h more top speed in this model.
The model is simple: a fixed power, a fixed mass and rolling losses. The
change in speed is only as good as the drag area you typed in.

## What the drag number may and may not say

The only drag statement the project allows, from
[VALIDATION.md](../VALIDATION.md#cfd-what-may-be-claimed), is this sentence,
about a separate run with a different method:

> On the classic slant-body test case with a 25° slant, Vernon's fast CFD tier
> puts drag within about 7–10% of the published 1984 wind-tunnel value. It uses
> a half model of about half a million cells, and each run after the first takes
> under 35 minutes on one workstation.

That result is not in this example. This example runs no solver and produces no
drag coefficient. The 0.64 m² is an input, not a prediction. You may not say
that the example's speed or power figures are drag predictions for a car or
for any body kit. The project also says the steady runs did not converge, so
their numbers are not results.

## What this tutorial does not show

- **A solved airflow.** Steps 6 and 7 stop at the case. A solver run is a
  separate job, in WSL, with its own checks (see [the CFD runbook](../CFD-RUNBOOK.md)).
- **Wheels and ground.** The example has no wheels. The vehicle step fails
  for that reason.
- **The aerodynamic preview.** The example does not run the optional AI
  aerodynamic preview node. Its output is labelled as a prediction, and it is
  not a CFD result.
- **Any drag accuracy.** See the section above.

## Check yourself

- Which node in this example writes the solver case, and which one runs it?
- What is the difference between `CdA` in `vehicle` and a `CdA` from a solver?
- If you change the air temperature in `air`, which numbers change in
  `top_speed`?
