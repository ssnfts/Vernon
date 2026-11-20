//! Renders a small set of meshes with known defects, and their intact twins, for
//! the AI vision check's second test (`real_vision_on_defect_renders` in
//! vernon-nodes-ai). `#[ignore]`: needs a GPU adapter; run with
//!   VERNON_RENDER_OUT_DIR=<folder> cargo test -p vernon-render --test defect_renders -- --ignored
//! It writes one PPM per render and `cases.json` (picture, question, expected answer).
//!
//! Flipped normals are not in the set on purpose: the viewport lights both sides
//! of every face and culls nothing, so a flipped face looks exactly like a correct
//! one in any render.

use egui::Color32;
use glam::{DQuat, DVec3};
use vernon_core::{Geometry, Prim};
use vernon_render::{Camera, DrawItem, ShadingMode, ViewDesc, ViewKind, ViewportRenderer};

const HOLE_Q: &str = "This is a render of a 3D model. Is there a hole or a missing patch in its surface? Answer with the single word YES or NO first, then one short sentence.";
const FLOAT_Q: &str = "This is a render of a 3D model. Is any part of the model floating apart from the rest, not touching it? Answer with the single word YES or NO first, then one short sentence.";
const LEGS_Q: &str = "This is a render of a 3D model of a table. How many legs does the table have? Answer with a single number.";

fn sphere(radius: f64, centre: DVec3, keep: impl Fn(DVec3) -> bool) -> Geometry {
    let (nu, nv) = (64u32, 32u32);
    let mut pts = Vec::new();
    for j in 0..=nv {
        let v = std::f64::consts::PI * j as f64 / nv as f64;
        for i in 0..nu {
            let u = std::f64::consts::TAU * i as f64 / nu as f64;
            pts.push(centre + radius * DVec3::new(v.sin() * u.cos(), v.sin() * u.sin(), v.cos()));
        }
    }
    let mut g = Geometry::from_points(pts.clone());
    let id = |i: u32, j: u32| j * nu + i % nu;
    for j in 0..nv {
        for i in 0..nu {
            for tri in [[id(i, j), id(i, j + 1), id(i + 1, j + 1)], [id(i, j), id(i + 1, j + 1), id(i + 1, j)]] {
                let c = tri.iter().map(|&k| pts[k as usize]).sum::<DVec3>() / 3.0;
                if keep((c - centre).normalize()) {
                    g.add_prim(Prim::polygon(tri.to_vec()));
                }
            }
        }
    }
    g
}

fn block(size: DVec3, at: DVec3) -> Geometry {
    let mut g = Geometry::make_box(size);
    for p in &mut g.points {
        *p += at;
    }
    g
}

fn joined(parts: &[Geometry]) -> Geometry {
    let mut g = parts[0].clone();
    for p in &parts[1..] {
        g.merge(p);
    }
    g
}

/// A body with an upright fin on its end face; `gap` moves the fin clear of the body
/// sideways (a lifted fin looks attached from a camera above it).
/// `end` is -1 or 1: which end of the body carries the fin (chosen per view so the
/// fin is on the far side of the body, where the gap can be seen).
fn body_and_fin(gap: f64, end: f64) -> Geometry {
    joined(&[
        block(DVec3::new(320.0, 160.0, 80.0), DVec3::new(0.0, 0.0, 40.0)),
        block(DVec3::new(30.0, 120.0, 140.0), DVec3::new(end * (160.0 + 15.0 + gap), 0.0, 40.0 + 70.0)),
    ])
}

/// A table top on `legs` of four corner legs.
fn table(legs: usize) -> Geometry {
    let mut parts = vec![block(DVec3::new(300.0, 200.0, 20.0), DVec3::new(0.0, 0.0, 170.0))];
    for (x, y) in [(-135.0, -85.0), (135.0, -85.0), (135.0, 85.0), (-135.0, 85.0)].into_iter().take(legs) {
        parts.push(block(DVec3::new(20.0, 20.0, 160.0), DVec3::new(x, y, 80.0)));
    }
    joined(&parts)
}

fn write_ppm(path: &std::path::Path, img: &(u32, u32, Vec<u8>)) {
    let mut out = format!("P6\n{} {}\n255\n", img.0, img.1).into_bytes();
    for px in img.2.as_chunks::<4>().0 {
        out.extend_from_slice(&px[..3]);
    }
    std::fs::write(path, out).unwrap();
}

#[test]
#[ignore = "needs a GPU adapter and VERNON_RENDER_OUT_DIR"]
fn render_defect_set() {
    let out = std::path::PathBuf::from(std::env::var("VERNON_RENDER_OUT_DIR").expect("set VERNON_RENDER_OUT_DIR"));
    std::fs::create_dir_all(&out).unwrap();
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("no GPU adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let mut r = ViewportRenderer::with_device(&device, &queue, 4);
    let mut cases = Vec::new();
    // The second view is from the opposite corner: at 90 degrees one leg of the intact
    // table hides behind another.
    for (view, turn) in [("a", 0.0f64), ("b", 180.0)] {
        let mut cam = Camera::new(ViewKind::Perspective);
        cam.rot = DQuat::from_rotation_z(turn.to_radians()) * cam.rot;
        let toward = cam.back();
        let fin_end = if turn == 0.0 { -1.0 } else { 1.0 };
        let centre = DVec3::new(0.0, 0.0, 110.0);
        let scenes: Vec<(&str, Geometry, &str, &str)> = vec![
            // Cut through the near and the far side, so the background shows through: with
            // two-sided lighting a hole in the near side alone looks like an intact ball.
            ("hole", sphere(100.0, centre, |d| d.angle_between(toward).to_degrees() > 28.0 && d.angle_between(-toward).to_degrees() > 28.0), HOLE_Q, "YES"),
            ("no_hole", sphere(100.0, centre, |_| true), HOLE_Q, "NO"),
            ("floating", body_and_fin(60.0, fin_end), FLOAT_Q, "YES"),
            ("attached", body_and_fin(0.0, fin_end), FLOAT_Q, "NO"),
            ("legs3", table(3), LEGS_Q, "3"),
            ("legs4", table(4), LEGS_Q, "4"),
        ];
        for (name, mut geo, question, expected) in scenes {
            geo.compute_normals();
            r.clear_meshes();
            let items = [DrawItem::new(r.upload(1, &geo))];
            let mut c = cam;
            c.fit(&r.items_bounds(&items), 1.0, 1.15);
            let mut v = ViewDesc::from_camera(&c, [640, 640]);
            v.background = Color32::from_rgb(205, 208, 214);
            v.show_grid = false;
            v.shading = ShadingMode::Shaded;
            r.draw(&v, &items);
            let img = r.read_pixels(0).expect("rendered");
            let file = format!("{name}_{view}.ppm");
            write_ppm(&out.join(&file), &img);
            let defect = !matches!(name, "no_hole" | "attached" | "legs4");
            cases.push(format!(
                "{{\"image\": \"{file}\", \"scene\": \"{name}\", \"view\": \"{view}\", \"defect\": {defect}, \"question\": {:?}, \"expected\": \"{expected}\"}}",
                question
            ));
        }
    }
    std::fs::write(out.join("cases.json"), format!("[\n{}\n]\n", cases.join(",\n"))).unwrap();
    assert_eq!(cases.len(), 12);
}
