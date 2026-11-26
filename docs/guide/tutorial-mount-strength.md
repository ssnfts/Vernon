# Tutorial: mount strength

**Scenes:** `examples/bodykit_pipeline.vrn` (Part 1: the structure nodes and
the inputs they take from the airflow and solar stages) and
`examples/mount_strength.vrn` (Part 2: a bracket solved to a safety factor).
**Time:** about 45 minutes.
**You will learn:** what the structural stage needs to check a body kit's
mounts, what the example builds for that check, and where the example stops.

Read [the user guide](README.md) sections 5, 6 and 9 first. Read the whole of
"What the results are" below before you use any number in this tutorial.

## The short version

- The example prepares everything a strength check needs: the printed
  material, its loss of stiffness with heat, two mount sites, the aero load
  at two speeds, the surface temperature mapped onto the solid, and the input
  decks for a structural solver.
- **The body-kit example does not solve the decks.** Its 3 mm panel is far
  thinner than the stand-in mesher's voxels, so the built-in solver cannot use
  it (see "Where the body-kit example stops"). Part 2 therefore solves a
  separate, solid mount bracket, `examples/mount_strength.vrn`, through to a
  safety factor.
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

## Where the body-kit example stops

The body-kit chain ends at the decks. Its panel is a 3 mm shell, and the
built-in mesher fills it with 8 mm voxel tetrahedra, much thicker than the
shell. That model is too large for the built-in solver at 8 mm (145,602
unknowns against its limit of 60,000), and at 12 mm it does not converge (the
solver reports that the body may be under-constrained). A real solve of the
panel needs the external tetrahedral mesher and then either solver; the decks
from Step 8 are ready for that. If you run an external solver on them, check
its result against the bearing hand check (`fea.hand_check`) and the mesh
convergence study (`fea.mesh_study`) before you trust it.

## Part 2: solve a mount bracket to a safety factor

`examples/mount_strength.vrn` is a small solid part the built-in mesher can
represent exactly: a 120 × 32 × 40 mm ASA bracket, clamped over one 32 × 40 mm
end face (a bonded end) and loaded on its top by a wing's downforce.

```powershell
cargo run -p vernon-cli -- cook examples/mount_strength.vrn
```

It takes about half a minute. You should see `12 nodes cooked, 0 failed` and
one expected gate, the stand-in mesher. Here the stand-in is not a compromise:
the box's mesh volume error is 0 %.

The chain, node by node:

| Node | What it does | Numbers you should see |
|---|---|---|
| `solid` | ten-node tetrahedra, 8 mm | 1,800 elements, volume error 0 % |
| `mount` | clamps the end face (16 mm patch radius) | 51 nodes held |
| `wing_load` | 0.05 m² of lift at 200 km/h on the top face | 85.6 N down |
| `bump` | a 4 g bump plus gravity on the part's own mass | 3.8 N |
| `cases` | aero, aero at 1.25 × speed, bump, aero plus bump | 4 cases |
| `solve` | the built-in reference solver | worst case (overspeed): 0.149 mm deflection, 1.98 MPa peak stress |
| `results` | reads the result files back onto the mesh | volume error 0 % in every case |
| `reactions` | checks that the mount carries the load | 85.65 N in the aero case; equilibrium error 1.7e-6 |
| `safety` | safety factor by the printed-layer (Tsai-Wu) criterion | `sf_min = 10.78`, `sf_mount_min = 19.51`, both against a target of 3 |

`safety` also reports `speed_limit_kmh = 473.8`: the speed at which the
governing case would reach the target. It is the same placeholder material as
Part 1, so it is an estimate, not a rating.

**How you hold the part matters as much as its size.** The same bracket held
by an 8 mm bolt patch in the middle of the end face, instead of the whole face,
reaches only `sf_min = 2.45` (failing the target of 3), `sf_mount_min = 3.14`
and a speed limit of 226 km/h, with 4.6 times the deflection. A bolted joint
is somewhere between the two; the mount model is a choice you must justify,
not a detail. Try it: set `r_w_mm` on the `mount` node to 8 and cook again.

What Part 2 is and is not:

- It shows the whole chain running: mesh, mounts, loads, solve, read-back,
  reactions and a safety factor that a gate checks.
- The material, the temperature curve and the loads are the same estimates as
  in Part 1, and the part sits at 23 °C (no temperature map), so heat is not
  in this result.
- The built-in solver has no thermal expansion, so a model with a temperature
  field keeps its safety factor as an estimate (the report says so).

## What this tutorial does not show

- A safety factor at the body-kit panel's mounts. See "Where the body-kit
  example stops".
- Bump loads on the body-kit panel (Part 2 has one on the bracket).
- Fatigue, creep or long-term load. Creep needs the oven test plan, which
  sets the creep factor; this example keeps it at 1.
- Bolt preload, thermal expansion, layer anisotropy beyond the strength ratio.

## Check yourself

- Why does the strength of a printed part depend on the direction of the load?
- Which two numbers in this tutorial are placeholders, and which report flags
  them?
- Why is the raw stress peak at a mount not the safety factor?
- In Part 2, why does the safety factor change so much between an 8 mm bolt
  patch and a clamped end face?
