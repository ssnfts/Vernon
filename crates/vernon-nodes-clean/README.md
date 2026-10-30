# vernon-nodes-clean

Stage 2 of the Vernon pipeline: decimation, remeshing, outlier removal,
smoothing, ICP alignment, watertight and conservative wrapping, and signed
deviation with percentile gates. Node behaviour is specified in
`docs/specs/alignment-deviation.md`, `docs/specs/conservative-wrap.md` and
`docs/specs/rhino8-mesh-tools.md`.

## Performance checks

Four tests are `#[ignore]`d because they need an optimised build. Run them with:

```powershell
cargo test --release -p vernon-nodes-clean --lib -- --ignored --nocapture --test-threads=1
```

They assert correctness at scale (triangle counts, closed output, certified
distance bounds), not a time limit, so a slow machine still passes. Read the
printed time. Recorded 2026-10-30 on an Intel i9-13900HX, Windows, GNU
toolchain, with other builds possibly running, so treat these as rough:

| Test | Workload | Time |
|---|---|---|
| `conservative::tests::wp6_release_million_triangles` | 1,000,000 source triangles to a certified wrap of 102,648 triangles; 51,326 vertices checked, none outside the distance band | 2.28 s |
| `decimate::tests::quarter_million_triangles_in_a_blink` | 250,000 to 25,000 triangles | 0.60 s |
| `distance::tests::one_million_points_signed_distance` | 1,000,000 points against a UV sphere of 128 by 64 segments | 0.07 s |
| `wrap::tests::large_grid_wrap_scales` | 1,127,120 output faces, 9,129,329 grid nodes, closed output | 0.65 s |
