//! Shader validation (always runs) and a headless GPU render (`#[ignore]`:
//! needs an adapter; run with `cargo test -p vernon-render -- --ignored`).

use egui::Color32;
use glam::DVec3;
use vernon_core::{Geometry, Prim};
use vernon_render::{
    Camera, DrawItem, ShadingMode, ViewDesc, ViewKind, ViewportRenderer, COLOR_FORMAT, SHADER_WGSL,
};

#[test]
fn shader_parses_and_validates() {
    let module = naga::front::wgsl::parse_str(SHADER_WGSL).expect("WGSL parses");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .expect("WGSL validates");
    for name in [
        "vs_mesh", "fs_mesh", "vs_edge", "vs_curve", "vs_point", "fs_flat", "fs_id",
    ] {
        assert!(
            module.entry_points.iter().any(|e| e.name == name),
            "missing entry point {name}"
        );
    }
}

#[test]
fn offscreen_format_is_non_srgb() {
    // egui's native-texture path expects a non-sRGB format.
    assert_eq!(COLOR_FORMAT, wgpu::TextureFormat::Rgba8Unorm);
}

fn headless() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        force_fallback_adapter: false,
        compatible_surface: None,
    }))
    .ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

/// Two boxes side by side: the ID pass names each, the nearer wins, and the
/// occluder hides from picking what it hides on screen.
#[test]
#[ignore = "needs a GPU or software adapter"]
fn id_pass_and_overlay() {
    let Some((device, queue)) = headless() else {
        eprintln!("no adapter available; skipping");
        return;
    };
    let mut r = ViewportRenderer::with_device(&device, &queue, 4);
    let at = |x: f64, z: f64| {
        let mut g = Geometry::make_box(DVec3::splat(100.0));
        for p in &mut g.points {
            *p += DVec3::new(x, 0.0, z);
        }
        g
    };
    let a = r.upload(1, &at(-150.0, 100.0));
    let b = r.upload(2, &at(150.0, 100.0));
    let behind = r.upload(3, &at(-150.0, -100.0));
    let items = [DrawItem::new(a), DrawItem::new(b), DrawItem::new(behind)];
    let mut cam = Camera::new(ViewKind::Top);
    cam.target = DVec3::ZERO;
    cam.half_height = 300.0;
    let size = [200, 200];
    let mut view = ViewDesc::from_camera(&cam, size);
    view.show_grid = false;
    let px = |x: f64| [(100.0 + x / 3.0) as u32, 100];
    assert_eq!(r.pick(&view, &items, px(-150.0), 0), Some(0), "nearer box wins");
    assert_eq!(r.pick(&view, &items, px(150.0), 0), Some(1));
    assert_eq!(r.pick(&view, &items, px(0.0), 0), None, "empty space");
    assert_eq!(r.pick(&view, &items, px(95.0), 8), Some(1), "tolerance reaches the box");
    assert_eq!(r.pick(&view, &items[2..], px(-150.0), 0), Some(0));
    view.occluder = Some([
        glam::Vec3::new(-1e3, -1e3, 0.0),
        glam::Vec3::new(1e3, -1e3, 0.0),
        glam::Vec3::new(1e3, 1e3, 0.0),
        glam::Vec3::new(-1e3, 1e3, 0.0),
    ]);
    assert_eq!(r.pick(&view, &items[2..], px(-150.0), 0), None, "under the ground");

    // The overlay stays on top of a box that is nearer than it.
    view.occluder = None;
    view.background = Color32::BLACK;
    view.overlay_lines = vec![
        vernon_render::PointVertex { pos: [-150.0, -100.0, -500.0], color: [0.0, 1.0, 0.0] },
        vernon_render::PointVertex { pos: [-150.0, 100.0, -500.0], color: [0.0, 1.0, 0.0] },
    ];
    r.draw(&view, &items);
    let img = r.read_pixels(0).unwrap();
    // The line runs along a pixel boundary, so MSAA may split it over two columns.
    let line = (49..=50)
        .map(|x| pixel(&img, x, 100))
        .any(|p| p[1] > p[0] + 40 && p[1] > p[2] + 40);
    assert!(line, "overlay line drawn over the box");
}

fn pixel(img: &(u32, u32, Vec<u8>), x: u32, y: u32) -> [u8; 4] {
    let i = ((y * img.0 + x) * 4) as usize;
    [img.2[i], img.2[i + 1], img.2[i + 2], img.2[i + 3]]
}

#[test]
#[ignore = "needs a GPU or software adapter"]
fn renders_a_box_headless() {
    let Some((device, queue)) = headless() else {
        eprintln!("no adapter available; skipping");
        return;
    };
    for samples in [1, 4] {
        let mut r = ViewportRenderer::with_device(&device, &queue, samples);
        let mut geo = Geometry::make_box(DVec3::new(100.0, 100.0, 100.0));
        geo.compute_normals();
        let mesh = r.upload(1, &geo);
        assert_eq!(r.mesh_stats(mesh).unwrap().triangles, 12);

        // Generation-keyed cache: same generation does not re-upload.
        let n = r.upload_count();
        assert_eq!(
            r.upload_versioned(7, 1, &geo),
            (vernon_render::MeshHandle(7), true)
        );
        assert!(!r.upload_versioned(7, 1, &geo).1);
        assert!(r.upload_versioned(7, 2, &geo).1);
        assert_eq!(r.upload_count(), n + 2);
        assert!(r.remove_mesh(7));

        let items = [DrawItem::new(mesh)];
        let mut cam = Camera::new(ViewKind::Perspective);
        cam.fit(&r.items_bounds(&items), 1.0, 1.05);
        let mut view = ViewDesc::from_camera(&cam, [128, 128]);
        view.background = Color32::from_rgb(10, 20, 30);
        view.show_grid = false;
        for shading in [
            ShadingMode::Shaded,
            ShadingMode::ShadedEdges,
            ShadingMode::Wireframe,
            ShadingMode::Points,
        ] {
            view.shading = shading;
            r.draw(&view, &items);
            let img = r.read_pixels(0).expect("target exists");
            assert_eq!((img.0, img.1), (128, 128));
            assert_eq!(
                pixel(&img, 0, 0)[..3],
                [10, 20, 30],
                "{shading:?} corner is background"
            );
            if shading == ShadingMode::Shaded {
                let c = pixel(&img, 64, 64);
                assert_ne!(c[..3], [10, 20, 30], "centre is covered by the box");
                assert_eq!(c[3], 255);
                // Headlight shading gives the visible faces different tones.
                let mut tones = std::collections::BTreeSet::new();
                for i in 0..128 * 128 {
                    let p = [img.2[i * 4], img.2[i * 4 + 1], img.2[i * 4 + 2]];
                    if p != [10, 20, 30] {
                        tones.insert(p);
                    }
                }
                assert!(
                    tones.len() >= 3,
                    "flat faces should differ in tone: {}",
                    tones.len()
                );
            }
            let covered = (0..128 * 128)
                .filter(|i| img.2[i * 4..i * 4 + 3] != [10, 20, 30])
                .count();
            assert!(covered > 0, "{shading:?} drew nothing");
        }

        // Resize keeps working and reallocates the target.
        view.size = [64, 32];
        view.shading = ShadingMode::Shaded;
        r.draw(&view, &items);
        let img = r.read_pixels(0).unwrap();
        assert_eq!((img.0, img.1), (64, 32));

        // Grid, curves and points draw without validation errors.
        let mut lines = Geometry::from_points(vec![DVec3::ZERO, DVec3::X * 50.0, DVec3::Y * 50.0]);
        lines.add_prim(Prim::polyline(vec![0, 1, 2]));
        let lh = r.upload(2, &lines);
        let mut view = ViewDesc::from_camera(&cam, [96, 96]);
        view.show_grid = true;
        r.draw(&view, &[DrawItem::new(mesh), DrawItem::new(lh)]);
        assert!(r.read_pixels(0).is_some());

        assert!(r.remove_mesh(2));
        assert!(!r.remove_mesh(2));
        r.clear_meshes();
        assert!(!r.has_mesh(1));
    }
}
