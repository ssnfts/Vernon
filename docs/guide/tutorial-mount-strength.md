# Tutorial: mount strength

**Scene:** `examples/bodykit_pipeline.vrn`, the structure nodes (FEA) and the
inputs they take from the airflow and solar stages.
**Time:** about 30 minutes.
**You will learn:** what the structural stage needs to check a body kit's
mounts, what the example builds for that check, and where the example stops.

Read [the user guide](README.md) sections 5, 6 and 9 first. Read the whole of
"What the results are" below before you use any number in this tutorial.

## The short version

- The example prepares everything a strength check needs: the printed
  material, its loss of stiffness with heat, two mount sites, the aero load
  at two speeds, the surface temperature mapped onto the solid, and the input
  decks for a structural solver.
- **The example does not solve the decks, and it does not compute a safety
  factor.** The scene has no solver node and no safety-factor node. The last
  step of this tutorial therefore has nothing to read. See "Where the example
  stops".
- Every number here is an input or an estimate. None of it is a measured
  strength.

## What the results are

Read this before quoting a number from this tutorial.

- **Estimates, not measurements.** The material moduli and strengths in the
  preset table are placeholders until replaced by datasheet or coupon data.
  The report says so ("absolute values are placeholders").
- **The temperature curve is a placeholder.** The retention curve is marked
  `uncalibrated`. Supply a measured curve before you trust a safety factor.
- **The solid mesh is a stand-in.** The built-in mesher fills the part with
  stair-stepped voxel tetrahedra. Its gate is expected to fail. An external
  mesher gives a conformal mesh, and that is what a result would need.
- **Linear tetrahedra are stiff in bending.** Vernon's built-in reference solver
  takes four-node tetrahedra as well as ten-node ones. The project's note says
  four-node results "stay estimates that cannot pass a gate".
- **Not modelled here.** Thermal expansion, creep, layer anisotropy beyond a
  strength ratio, and bolt preload are not validated (see
  [what is not validated](../VALIDATION.md#what-is-not-validated)).
- **The aero load is an estimate.** It is dynamic pressure times the manual
  lift area, spread over the faces that face the flow. It is not a pressure
  field from a solver.

## What you need

- Vernon installed, or the repository with a Rust build.
- The example scene, `examples/bodykit_pipeline.vrn`.

Commands use the repository form (`cargo run -p vernon-cli --`). With the
installed package use `bin\vernon-cli.exe`.

## Step 1: the printed material

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node material
```

The scene picks the ASA preset. You should see:

```text
material (fea.material_fdm)
  report: Printed material ASA [PASS]
    e_inplane_mpa = 2000 MPa
    e_stack_mpa = 1600 MPa
    e_stack_over_inplane = 0.8
    xt_mpa = 40 MPa
    zt_mpa = 11.6 MPa
    strength_z_ratio = 0.29
    hdt_045_c = 100 °C
    [warning] compressive strength not given: assumed 1.2 x the tensile strength
    [warning] ... absolute values are placeholders
```

Read it this way:

- A printed part is stiffer and stronger along the print lines (in-plane) than
  across the layers (stacking direction).
- `strength_z_ratio = 0.29` means a part loaded across the layers keeps about
  29 % of its flat strength (`zt_mpa` 11.6 MPa against `xt_mpa` 40 MPa). That
  is the number that matters for a mount bolted through the part.
- The compressive strength is not given. Vernon assumes 1.2 times the tensile
  value, and says so.

## Step 2: the loss of strength with heat

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node derate
```

You should see the retention at each temperature, as a share of the 23 °C
value:

```text
retention_23c = 1
retention_40c = 0.8047
retention_60c = 0.6281
retention_85c = 0.45
retention_105c = 0.0969
uncalibrated = 1
[warning] retention curve is an uncalibrated placeholder: supply a measured
storage-modulus curve for a trustworthy safety factor
```

The curve is applied to stiffness and strength alike. At 85 °C a part keeps
45 % of its room-temperature values in this placeholder curve. The
`uncalibrated = 1` flag means that number is not a measured property.

## Step 3: the mount sites

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node mounts
```

It should report `Mount sites [PASS]` with:

```text
mount_count = 2
mount1_nodes = 25
mount2_nodes = 25
min_patch_nodes = 25   (limit >= 3: ok)
```

The scene places the two mounts at x = -300 mm and x = +300 mm, on the solid,
with a 12 mm radius parameter (`r_w_mm`) and a 5 mm bolt diameter. Each mount
is held by a patch of 25 solid nodes. The report also notes that constrained
nodes create stress singularities, so a mount is judged by the stress averaged
over a sphere, with the raw peak reported beside it.

## Step 4: the aero load

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node aero_load
```

You should see:

```text
dynamic_pressure_pa = 1693.7826 Pa
projected_area_mm2 = 209691.7703 mm²
force_requested_n = 169.3783 N
resultant_z_n = -169.3783 N
max_pressure_mpa = 8.0775e-4 MPa
conservation_error_pct = 5.5374e-13 %  (limit <= 1: ok)
```

The dynamic pressure is the one at the 200 km/h design speed (the same air
pressure as in the aero check). The force is that pressure times the manual
lift area of 0.1 m², and it pushes the body down (negative z). Spread over the
projected area, it is a pressure of about 0.0008 MPa, which is less than
1 kPa.

The conservation check is the important part here: the applied faces add up to
the requested force, to within rounding.

## Step 5: the load cases

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node load_cases
```

You should see:

```text
case_count = 2
aero_resultant_n = 169.3783 N
aero_overspeed_resultant_n = 264.6535 N
aero_overspeed_max_pressure_mpa = 0.0013 MPa
mount_count = 2   (limit >= 1: ok)
[warning] no bump load connected: the bump cases were skipped
```

There are two cases: the aero load at design speed, and the aero load at a
higher speed. A bump case (gravity and acceleration on the part) is not in the
scene, so it is skipped. The report's note says the safety factor will be the
minimum over the cases that run.

## Step 6: the temperature on the solid

The surface temperature comes from the solar and heat stage. The example maps
it onto the solid's nodes:

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node temp_map
```

You should see:

```text
temp_min_c = 82.5546 °C
temp_max_c = 82.6528 °C
temp_mean_c = 82.5874 °C
unmapped_nodes = 0
margin_to_hdt_c = 17.3472 °C  (limit >= 0: ok)
```

The part sits at about 82.6 °C in the design hour. The margin to the material's
heat-deflection temperature is 17.3 °C, using the 100 °C value from Step 1. The
margin passes, but the 100 °C is a placeholder, so the margin is too.

At 82.6 °C the derating in Step 2 lies between 0.63 (60 °C) and 0.45 (85 °C),
so roughly half of the room-temperature stiffness and strength in this
placeholder curve. This is why the temperature matters for a mount: the same
part is weaker on a hot day.

## Step 7: the solid mesh (expected failure)

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node tet_mesh
```

You should see:

```text
Solid mesh [FAIL]
nodes = 48534
elements = 19686
volume_error_pct = 1.151 %
placeholder = 1   (limit <= 0: FAIL)
[expected] tet_mesh / placeholder = 1 (limit <= 0, margin -1):
the built-in voxel mesher stands in until an external tetrahedral mesher is run
```

This failure is listed in the scene as expected. The built-in mesher's volume is
1.1 % larger than the surface's volume, and its smallest dihedral angle is
44.8 degrees, well above the limit of 5. But the mesh is stair-stepped.
A result from it is not a result for the printed part.

## Step 8: the input decks

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node fea_deck
```

It should report `Structural decks [PASS]` with:

```text
deck_count = 2
nodes = 48534
elements = 19686
mount_count = 2
unit_check_passed = 1
```

It writes two decks, `vernon_aero.inp` and `vernon_aero_overspeed.inp`, in the
`n27_fea_deck\decks` folder under the `.vrn.work` folder. The units in the deck
are millimetres, newtons, tonnes, seconds and MPa. Open the file in a text
editor and you can see the material cards with the temperature rows, the mount
sets and the loads. The decks are complete inputs. Nothing has run them.

## Where the example stops

The chain ends at the decks. The scene has:

- no node that runs a solver (`fea.solve_linear` or `fea.solve_external`), and
- no node that computes a safety factor (`fea.safety_factor`).

Because of that, this tutorial cannot show a safety factor at each mount. The
project's own gate for this is "safety factor at every mount" (README,
the structure step). The example does not check it. Nothing in this
scene passes or fails on strength.

This is an open item in the example, not a step you can finish by following
the tutorial. The tutorial does not add those nodes, because it must run on the
example as shipped. Adding a solver node is a change to the scene, and a
solver run is a separate job with its own checks. The FEA behaviour contract is
in [docs/specs/structural-fea.md](../specs/structural-fea.md).

If you run an external solver on the written decks yourself, check its result
against the bearing hand check (`fea.hand_check`) and the mesh convergence study
(`fea.mesh_study`) before you trust it. A
safety factor from this example is an estimate, and the report says so.

## What this tutorial does not show

- A safety factor, a pass or a fail at the mounts. See above.
- Bump loads (gravity, acceleration). None are connected.
- Fatigue, creep or long-term load. Creep needs the oven test plan, which
  sets the creep factor; this example keeps it at 1.
- Bolt preload, thermal expansion, layer anisotropy beyond the strength ratio.

## Check yourself

- Why does the strength of a printed part depend on the direction of the load?
- Which two numbers in this tutorial are placeholders, and which report flags
  them?
- Why is the raw stress peak at a mount not the safety factor?
