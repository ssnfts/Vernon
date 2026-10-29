//! Ground-truth WP7 acceptance on the actual procedural car-panel surface.
use vernon_core::{DMat4, DVec3, Geometry};
use vernon_graph::{CookContext, Data, NodeOp, ParamValue, Params};
use vernon_nodes_capture::registration::{GlobalAlign, MultiwayAlign};
use vernon_nodes_capture::util::{mat_from_json, Rng};
use vernon_nodes_geo::primitives::CarPanel;

fn context() -> CookContext {
    CookContext { node_name: "wp7".into(), work_dir: std::env::temp_dir(), frame: 0.0 }
}

#[test]
fn wp7_node_contract_is_native_and_unknown_method_is_rejected() {
    assert!(!GlobalAlign.info().whitebox);
    assert!(!MultiwayAlign.info().whitebox);
    let points = CarPanel { cols: 60, rows: 40, ..CarPanel::default() }.build();
    let mut p = Params::from_specs(&GlobalAlign.info().params);
    p.set("method", ParamValue::Str("fgr".into()));
    let error = GlobalAlign.cook(&context(), &[Some(Data::geometry(points.clone())), Some(Data::geometry(points))], &p).unwrap_err();
    assert!(error.to_string().contains("not implemented"), "{error}");
}

fn fixture() -> (Vec<Geometry>, Vec<DMat4>) {
    let panel = CarPanel { cols: 120, rows: 80, ..CarPanel::default() }.build();
    let mut pose_rng=Rng::new(21841);
    let mut poses=vec![DMat4::IDENTITY];
    for i in 1..5 {
        let axis=DVec3::new(pose_rng.gaussian(),pose_rng.gaussian(),pose_rng.gaussian()).normalize();
        let direction=DVec3::new(pose_rng.gaussian(),pose_rng.gaussian(),pose_rng.gaussian()).normalize();
        let angle=if i==4 {30.0} else {10.0+20.0*pose_rng.next_f64()};
        let distance=if i==4 {200.0} else {40.0+160.0*pose_rng.next_f64()};
        poses.push(DMat4::from_translation(direction*distance)*DMat4::from_axis_angle(axis,angle.to_radians()));
    }
    let crops = [(-450.0, 260.0, -300.0, 300.0), (-310.0, 450.0, -255.0, 300.0),
        (-400.0, 370.0, -300.0, 260.0), (-340.0, 420.0, -275.0, 300.0), (-420.0, 330.0, -300.0, 280.0)];
    let scans = crops.iter().zip(&poses).enumerate().map(|(i, (&(xmin, xmax, ymin, ymax), pose))| {
        let mut rng = Rng::new(8001 + i as u64);
        let mut points:Vec<_> = panel.points.iter().filter(|p| p.x >= xmin && p.x <= xmax && p.y >= ymin && p.y <= ymax).map(|p| {
            let noise = DVec3::new(rng.gaussian(), rng.gaussian(), rng.gaussian()) * 0.02;
            pose.transform_point3(*p + noise)
        }).collect();
        // Each scan has an independent order; no implicit point-pair indices exist.
        for k in (1..points.len()).rev() { let j=rng.below(k+1); points.swap(k,j); }
        Geometry::from_points(points)
    }).collect();
    (scans, poses)
}

fn pose_errors(actual: DMat4, expected: DMat4) -> (f64, f64) {
    let (_, ra, ta) = actual.to_scale_rotation_translation();
    let (_, rb, tb) = expected.to_scale_rotation_translation();
    let angle = (ra * rb.inverse()).to_axis_angle().1.to_degrees().abs();
    ((ta - tb).length(), angle)
}

#[test]
fn wp7_five_partial_car_panels_recover_ground_truth() {
    let (scans, source_poses) = fixture();
    let mut p = Params::from_specs(&MultiwayAlign.info().params);
    p.set("voxel", ParamValue::Float(8.0));
    p.set("dedupe_voxel", ParamValue::Float(0.0));
    let inputs: Vec<_> = scans.into_iter().map(|g| Some(Data::geometry(g))).collect();
    let output = MultiwayAlign.cook(&context(), &inputs, &p).expect("five overlapping scans align");
    let json = output[1].as_json().unwrap();
    let transforms = json["transforms"].as_array().unwrap();
    assert_eq!(transforms.len(), 5);
    for (i, matrix) in transforms.iter().enumerate() {
        let actual = mat_from_json(matrix).unwrap();
        let expected = source_poses[i].inverse();
        let (mm, deg) = pose_errors(actual, expected);
        eprintln!("WP7 scan={i} translation_error_mm={mm:.9} rotation_error_deg={deg:.9}");
        assert!(mm <= 0.5 && deg <= 0.5, "scan {i}: translation {mm} mm, rotation {deg} deg");
    }
    assert!(mat_from_json(&transforms[0]).unwrap().abs_diff_eq(DMat4::IDENTITY, 1e-12));
    let report = output[2].as_report().unwrap();
    for key in ["rmse_mm", "fitness"] {
        assert!(report.metrics[key].value.is_finite(), "{key}");
        eprintln!("WP7 {key}={}", report.metrics[key].value);
    }
    assert!(report.metrics["fitness"].value > 0.2);
    assert!(!report.notes.iter().any(|(_, note)| note.starts_with("white-box placeholder:")));
    output[0].as_geometry().unwrap().validate().unwrap();
}
