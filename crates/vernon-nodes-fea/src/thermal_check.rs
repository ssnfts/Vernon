//! Acceptance of thermal strain in the reference solver: a bar 200 x 20 x 20 mm
//! heated from the 23 degC stress-free temperature.
//!
//! Closed forms (linear elasticity, uniform temperature change `dT`):
//! - a bar free to expand grows by `alpha L dT` and carries no stress;
//! - a bar held at both ends, free sideways, carries `sigma = -E alpha dT`
//!   along its length, with `E` and `alpha` taken along the bar's axis.
//!
//! The clamped ends of the meshed bars also hold the cross-section. A held
//! end zone, unable to widen, tries to grow lengthwise by about
//! `alpha (1 + nu) dT`, and since the bar as a whole cannot grow, the middle
//! is pushed a little harder than `E alpha dT`: +3.3 % on the 200 x 20 x 20
//! bar. The effect scales with width over length, so the held closed-form
//! checks use a slender 400 x 10 x 10 bar and read the middle of it, and the
//! bending check reads the curvature away from the clamped root. The same models then go through the external
//! solver with a matching expansion card.

use std::time::Duration;

use glam::DVec3;
use vernon_core::{AttribArray, Geometry};

use crate::deck::{write_deck, DeckInput, DeckOptions};
use crate::frd::{dense_tensor6, dense_vec3, parse_frd};
use crate::linalg::von_mises;
use crate::material::{MaterialCard, RetentionTable};
use crate::mesh::{box_tets, element_centroid, tet4_to_tet10};
use crate::refsolve::{solve, RefInput, RefOutput};
use crate::solver::{find_executable, run_with_timeout, RunStatus};
use crate::types::{LoadSet, MountSet, MountSite, OrientationCard};

const LENGTH: f64 = 200.0;
const SIDE: f64 = 20.0;
const E: f64 = 2000.0;
const NU: f64 = 0.3;
const ALPHA: f64 = 1.0e-4;
const T_REF: f64 = 23.0;
const DT: f64 = 50.0;

fn iso_card() -> MaterialCard {
    let mut c = MaterialCard::trans_iso("iso", E, E, NU, NU, E / (2.0 * (1.0 + NU)));
    c.alpha = Some([ALPHA; 3]);
    c
}

/// Printed-style material: soft across the layers and expanding more along them.
fn printed_card() -> MaterialCard {
    let mut c = MaterialCard::trans_iso("printed", E, 0.6 * E, NU, 0.3, 0.25 * E);
    c.alpha = Some([0.5 * ALPHA, 0.5 * ALPHA, 1.5 * ALPHA]);
    c
}

fn mesh(n: usize, temp: impl Fn(DVec3) -> f64) -> Geometry {
    with_temp(tet4_to_tet10(&box_tets(DVec3::new(LENGTH, SIDE, SIDE), [10 * n, n, n])).unwrap(), temp)
}

/// A slender bar, 400 x 10 x 10 mm, for the held closed forms.
fn slender(temp: impl Fn(DVec3) -> f64) -> Geometry {
    with_temp(tet4_to_tet10(&box_tets(DVec3::new(2.0 * LENGTH, 0.5 * SIDE, 0.5 * SIDE), [40, 1, 1])).unwrap(), temp)
}

fn with_temp(mut m: Geometry, temp: impl Fn(DVec3) -> f64) -> Geometry {
    let t: Vec<f64> = m.points.iter().map(|p| temp(*p)).collect();
    m.point_attribs.set("temp_c", AttribArray::Float(t));
    m
}

fn nodes_at(mesh: &Geometry, x: f64) -> Vec<u32> {
    (0..mesh.points.len() as u32).filter(|&i| (mesh.points[i as usize].x - x).abs() < 1e-9).collect()
}

fn clamp(mesh: &Geometry, x: f64, name: &str) -> MountSite {
    MountSite { name: name.into(), center: [x, 0.0, 0.0], axis: [x.signum(), 0.0, 0.0], nodes: nodes_at(mesh, x), model: "fixed_patch".into(), r_w_mm: 5.0, avg_radius_mm: 5.0, bolt_d_mm: 5.0, pad_e_mpa: 5.0, pad_t_mm: 2.0 }
}

fn cantilever_mounts(mesh: &Geometry) -> MountSet {
    MountSet { sites: vec![clamp(mesh, -half(mesh), "root")] }
}

/// Half the length of a bar centred on the origin.
fn half(mesh: &Geometry) -> f64 {
    mesh.points.iter().map(|p| p.x).fold(0.0, f64::max)
}

fn both_ends(mesh: &Geometry) -> MountSet {
    let h = half(mesh);
    MountSet { sites: vec![clamp(mesh, -h, "left"), clamp(mesh, h, "right")] }
}

/// Cross-section area of a square bar centred on the X axis.
fn area(mesh: &Geometry) -> f64 {
    let ys = mesh.points.iter().map(|p| p.y).fold(0.0, f64::max);
    4.0 * ys * ys
}

fn run(mesh: &Geometry, c: &MaterialCard, id: &OrientationCard, m: &MountSet) -> RefOutput {
    solve(&RefInput { mesh, card: c, orient: id, mounts: m, load: &LoadSet::default(), max_dof: 60000, tol: 1e-12 }).unwrap()
}

fn mean_x(mesh: &Geometry, disp: &[DVec3], x: f64) -> f64 {
    let n = nodes_at(mesh, x);
    n.iter().map(|&i| disp[i as usize].x).sum::<f64>() / n.len() as f64
}

/// Mean element stress over the elements whose centre lies in the middle 40% of the bar.
fn mid_stress(mesh: &Geometry, o: &RefOutput) -> [f64; 6] {
    let h = half(mesh);
    let mid: Vec<usize> = (0..mesh.prims.len()).filter(|&e| element_centroid(mesh, e).x.abs() < 0.4 * h).collect();
    std::array::from_fn(|c| mid.iter().map(|&e| o.elem_stress[e][c]).sum::<f64>() / mid.len() as f64)
}

/// The bar axis along material axis 3 (the stacking direction).
fn stacking_along_x() -> OrientationCard {
    OrientationCard { name: "OR2".into(), axes: [[0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]], origin: [0.0; 3], rot_deg: [0.0; 3] }
}

#[test]
fn a_free_bar_grows_by_alpha_l_dt_without_stress() {
    let m = mesh(2, |_| T_REF + DT);
    let o = run(&m, &iso_card(), &OrientationCard::identity(), &cantilever_mounts(&m));
    // the 120 mm from x = -40 to the free end, clear of the clamped root
    let grown = mean_x(&m, &o.disp, LENGTH / 2.0) - mean_x(&m, &o.disp, -40.0);
    let want = ALPHA * 140.0 * DT;
    println!("free bar: 140 mm grows {grown:.5} mm, closed form {want:.5} mm, error {:+.3}%", 100.0 * (grown - want) / want);
    assert!((grown - want).abs() < 0.02 * want, "{grown} {want}");
    // the whole bar, root disturbance included, is still within 2%
    let tip = mean_x(&m, &o.disp, LENGTH / 2.0);
    println!("free bar: tip {tip:.5} mm, alpha L dT {:.5} mm", ALPHA * LENGTH * DT);
    assert!((tip - ALPHA * LENGTH * DT).abs() < 0.02 * ALPHA * LENGTH * DT, "{tip}");
    // no stress in the middle, against the fully restrained E alpha dT = 10 MPa
    let s = mid_stress(&m, &o);
    let scale = E * ALPHA * DT;
    println!("free bar: mid stress {s:?} MPa against {scale} MPa restrained");
    assert!(s.iter().all(|v| v.abs() < 0.02 * scale), "{s:?}");
    // nothing is applied, so the support carries nothing in total
    let r = o.reactions["MOUNT1"];
    assert!(r.iter().all(|v| v.abs() < 1e-6 * scale * SIDE * SIDE), "{r:?}");
}

#[test]
fn a_bar_held_at_both_ends_carries_e_alpha_dt() {
    let m = slender(|_| T_REF + DT);
    let o = run(&m, &iso_card(), &OrientationCard::identity(), &both_ends(&m));
    let s = mid_stress(&m, &o);
    let want = -E * ALPHA * DT;
    println!("held bar: mid sigma_xx {:.4} MPa, closed form {want:.4} MPa, error {:+.3}%", s[0], 100.0 * (s[0] - want) / want);
    assert!((s[0] - want).abs() < 0.02 * want.abs(), "{s:?}");
    assert!(s[1..].iter().all(|v| v.abs() < 0.02 * want.abs()), "{s:?}");
    // each end pushes back with sigma A, and the two balance
    let (l, r) = (o.reactions["MOUNT1"][0], o.reactions["MOUNT2"][0]);
    let a = area(&m);
    println!("held bar: end forces {l:.2} N and {r:.2} N, sigma A {:.2} N", want * a);
    assert!((l + r).abs() < 1e-6 * want.abs() * a);
    assert!((r.abs() - want.abs() * a).abs() < 0.02 * want.abs() * a, "{l} {r}");
    // the stubby bar shows the end effect the slender one avoids
    let stubby = mesh(2, |_| T_REF + DT);
    let s2 = mid_stress(&stubby, &run(&stubby, &iso_card(), &OrientationCard::identity(), &both_ends(&stubby)));
    println!("held bar 200 x 20 x 20: mid sigma_xx {:.4} MPa ({:+.2}%)", s2[0], 100.0 * (s2[0] - want) / want);
    assert!(s2[0] < want && s2[0] > 1.06 * want, "{}", s2[0]);
}

#[test]
fn orthotropic_expansion_follows_the_material_axes() {
    let m = slender(|_| T_REF + DT);
    let c = printed_card();
    let [a1, _, a3] = c.alpha.unwrap();
    // in-plane axis along the bar: E1 alpha1 dT
    let s = mid_stress(&m, &run(&m, &c, &OrientationCard::identity(), &both_ends(&m)));
    let want = -c.e1 * a1 * DT;
    println!("held printed bar, axis 1 along it: {:.4} MPa, closed form {want:.4} MPa", s[0]);
    assert!((s[0] - want).abs() < 0.02 * want.abs(), "{s:?}");
    // stacking axis along the bar: E3 alpha3 dT, 0.6 x 3 = 1.8 times as much
    let s3 = mid_stress(&m, &run(&m, &c, &stacking_along_x(), &both_ends(&m)));
    let want3 = -c.e3 * a3 * DT;
    println!("held printed bar, axis 3 along it: {:.4} MPa, closed form {want3:.4} MPa", s3[0]);
    assert!((s3[0] - want3).abs() < 0.02 * want3.abs(), "{s3:?}");
    // and the free bar grows by alpha3 along the stacking axis
    let o = run(&m, &c, &stacking_along_x(), &cantilever_mounts(&m));
    let grown = mean_x(&m, &o.disp, LENGTH) - mean_x(&m, &o.disp, -100.0);
    assert!((grown - a3 * 300.0 * DT).abs() < 0.02 * a3 * 300.0 * DT, "{grown}");
}

#[test]
fn retention_scales_the_restrained_stress_and_a_card_without_expansion_has_none() {
    // half the stiffness at 80 degC, linear in between: at 73 degC r = 1 - 0.5 * 50 / 57
    let m = slender(|_| T_REF + DT);
    let mut c = iso_card();
    c.retention = Some(RetentionTable { points: vec![(23.0, 1.0), (80.0, 0.5)], source: "test".into(), uncalibrated: false });
    let s = mid_stress(&m, &run(&m, &c, &OrientationCard::identity(), &both_ends(&m)));
    let want = -(1.0 - 0.5 * DT / 57.0) * E * ALPHA * DT;
    assert!((s[0] - want).abs() < 0.02 * want.abs(), "{} {want}", s[0]);
    c.alpha = None;
    let o = run(&m, &c, &OrientationCard::identity(), &both_ends(&m));
    assert!(o.max_vm() < 1e-9 && o.max_disp() < 1e-12, "{} {}", o.max_vm(), o.max_disp());
}

#[test]
fn a_through_thickness_gradient_bends_the_free_bar_to_alpha_dt_over_h() {
    // linear in z, 23 degC at the bottom to 73 at the top: curvature alpha dT / h, stress free
    let m = mesh(2, |p| T_REF + DT * (p.z + SIDE / 2.0) / SIDE);
    let o = run(&m, &iso_card(), &OrientationCard::identity(), &cantilever_mounts(&m));
    let w = |x: f64| nodes_at(&m, x).iter().map(|&i| o.disp[i as usize].z).sum::<f64>() / nodes_at(&m, x).len() as f64;
    // curvature from three stations clear of the root: w'' = 2 [slope(2,3) - slope(1,2)] / (x3 - x1)
    let (x1, x2, x3) = (-40.0, 30.0, 100.0);
    let kappa = 2.0 * ((w(x3) - w(x2)) / (x3 - x2) - (w(x2) - w(x1)) / (x2 - x1)) / (x3 - x1);
    let want = -ALPHA * DT / SIDE;
    println!("gradient: curvature {kappa:.6e} per mm, closed form {want:.6e}, error {:+.3}%", 100.0 * (kappa - want) / want);
    assert!((kappa - want).abs() < 0.02 * want.abs(), "{kappa} {want}");
    // the tip, with the clamped root's disturbance in it, is within 3% of kappa L^2 / 2
    let tip = w(LENGTH / 2.0);
    let free = want * LENGTH * LENGTH / 2.0;
    println!("gradient: tip {tip:.4} mm, kappa L^2/2 {free:.4} mm, error {:+.3}%", 100.0 * (tip - free) / free);
    assert!((tip - free).abs() < 0.03 * free.abs(), "{tip} {free}");
    // and the middle of the bar carries no stress
    let s = mid_stress(&m, &o);
    assert!(s.iter().all(|v| v.abs() < 0.02 * E * ALPHA * DT), "{s:?}");
}

/// The same model through the external solver with an expansion card. Set
/// `VERNON_CCX` or put it on the program path.
fn external(mesh: &Geometry, c: &MaterialCard, id: &OrientationCard, m: &MountSet, tag: &str) -> (Vec<DVec3>, Vec<[f64; 6]>) {
    let exe = find_executable(&std::env::var("VERNON_CCX").unwrap_or_default(), &["ccx", "ccx_2.21", "ccx_2.20", "ccx_static", "ccx_MT"]).expect("the external solver is not installed");
    let dir = std::env::temp_dir().join(format!("vernon_fea_thermal_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let opts = DeckOptions { job: "thermal".into(), thermal_strain: Some((c.alpha.unwrap(), c.t_ref_c)), ..Default::default() };
    let deck = write_deck(&DeckInput { mesh, card: c, orient: id, mounts: m, load: &LoadSet::default(), opts: &opts }).unwrap();
    std::fs::write(dir.join("thermal.inp"), &deck.text).unwrap();
    let st = run_with_timeout(&exe, &["-i".to_string(), "thermal".to_string()], &dir, &dir.join("thermal.log"), Duration::from_secs(600), &[]).unwrap();
    assert!(matches!(st, RunStatus::Exited(0)), "the solver failed, see {}", dir.display());
    let frd = parse_frd(&std::fs::read_to_string(dir.join("thermal.frd")).unwrap()).unwrap();
    let n = mesh.points.len();
    let needed = vec![true; n];
    (dense_vec3(frd.dataset("DISP").unwrap(), n, &needed).unwrap(), dense_tensor6(frd.dataset("STRESS").unwrap(), n, &needed).unwrap())
}

/// Every displacement within 2% of the largest, and every stress component on
/// the mid-span section within 2% of `stress_scale`.
fn assert_agrees(mesh: &Geometry, c: &MaterialCard, id: &OrientationCard, m: &MountSet, stress_scale: f64, tag: &str) {
    let o = run(mesh, c, id, m);
    let (u, s) = external(mesh, c, id, m, tag);
    let peak = u.iter().map(|d| d.length()).fold(0.0, f64::max);
    let worst = o.disp.iter().zip(&u).map(|(a, b)| (*a - *b).length()).fold(0.0, f64::max);
    println!("{tag}: largest displacement {:.5} mm here, {peak:.5} mm external; worst difference {:.3}% of it", o.max_disp(), 100.0 * worst / peak);
    assert!(worst < 0.02 * peak, "{tag}: displacement differs by {worst} of {peak}");
    let span = nodes_at(mesh, 0.0);
    let mut worst_s: f64 = 0.0;
    for &n in &span {
        for (a, b) in o.node_stress[n as usize].iter().zip(&s[n as usize]) {
            worst_s = worst_s.max((a - b).abs());
        }
    }
    let vm = |v: &[[f64; 6]]| span.iter().map(|&n| von_mises(&v[n as usize])).fold(0.0, f64::max);
    println!("{tag}: mid-span von Mises {:.4} MPa here, {:.4} MPa external; worst component difference {:.3}% of {stress_scale} MPa", vm(&o.node_stress), vm(&s), 100.0 * worst_s / stress_scale);
    assert!(worst_s < 0.02 * stress_scale, "{tag}: stress differs by {worst_s}");
}

#[test]
#[ignore = "needs CalculiX"]
fn heated_cantilever_agrees_with_the_external_solver_within_2_percent() {
    // printed material with the weak axis across the width, heated through the thickness
    // and softened by the heat: bending from the gradient plus growth along the bar
    let m = mesh(2, |p| T_REF + DT * (p.z + SIDE / 2.0) / SIDE);
    let mut c = printed_card();
    c.retention = Some(RetentionTable { points: vec![(23.0, 1.0), (80.0, 0.5)], source: "test".into(), uncalibrated: false });
    let mut id = OrientationCard::identity();
    id.axes = [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]];
    assert_agrees(&m, &c, &id, &cantilever_mounts(&m), E * ALPHA * DT, "heated_cantilever");
}

#[test]
#[ignore = "needs CalculiX"]
fn held_heated_bar_agrees_with_the_external_solver_within_2_percent() {
    let m = slender(|_| T_REF + DT);
    let c = printed_card();
    let o = run(&m, &c, &stacking_along_x(), &both_ends(&m));
    let want = -c.e3 * c.alpha.unwrap()[2] * DT;
    assert!((mid_stress(&m, &o)[0] - want).abs() < 0.02 * want.abs());
    assert_agrees(&m, &c, &stacking_along_x(), &both_ends(&m), want.abs(), "held_bar");
}

#[test]
#[ignore = "needs CalculiX"]
fn held_heated_bar_of_four_node_tetrahedra_agrees_with_the_external_solver_within_2_percent() {
    // constant-strain elements represent a uniform thermal strain exactly
    let mut m = box_tets(DVec3::new(LENGTH, SIDE, SIDE), [40, 4, 4]);
    let n = m.points.len();
    m.point_attribs.set("temp_c", AttribArray::Float(vec![T_REF + DT; n]));
    let c = iso_card();
    let mounts = both_ends(&m);
    let o = run(&m, &c, &OrientationCard::identity(), &mounts);
    let (u, s) = {
        let exe = find_executable(&std::env::var("VERNON_CCX").unwrap_or_default(), &["ccx"]).expect("the external solver is not installed");
        let dir = std::env::temp_dir().join(format!("vernon_fea_thermal_c3d4_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let opts = DeckOptions { job: "thermal".into(), thermal_strain: Some((c.alpha.unwrap(), c.t_ref_c)), allow_linear_tet: true, ..Default::default() };
        let deck = write_deck(&DeckInput { mesh: &m, card: &c, orient: &OrientationCard::identity(), mounts: &mounts, load: &LoadSet::default(), opts: &opts }).unwrap();
        std::fs::write(dir.join("thermal.inp"), &deck.text).unwrap();
        let st = run_with_timeout(&exe, &["-i".to_string(), "thermal".to_string()], &dir, &dir.join("thermal.log"), Duration::from_secs(600), &[]).unwrap();
        assert!(matches!(st, RunStatus::Exited(0)), "the solver failed, see {}", dir.display());
        let frd = parse_frd(&std::fs::read_to_string(dir.join("thermal.frd")).unwrap()).unwrap();
        (dense_vec3(frd.dataset("DISP").unwrap(), n, &vec![true; n]).unwrap(), dense_tensor6(frd.dataset("STRESS").unwrap(), n, &vec![true; n]).unwrap())
    };
    let peak = u.iter().map(|d| d.length()).fold(0.0, f64::max);
    let worst = o.disp.iter().zip(&u).map(|(a, b)| (*a - *b).length()).fold(0.0, f64::max);
    let mid = nodes_at(&m, 0.0);
    let (ours, theirs) = (mid.iter().map(|&i| o.node_stress[i as usize][0]).sum::<f64>() / mid.len() as f64, mid.iter().map(|&i| s[i as usize][0]).sum::<f64>() / mid.len() as f64);
    println!("c3d4 held bar: mid sigma_xx {ours:.4} MPa here, {theirs:.4} MPa external, closed form {:.4}; displacement difference {:.3}%", -E * ALPHA * DT, 100.0 * worst / peak);
    assert!(worst < 0.02 * peak && (ours - theirs).abs() < 0.02 * theirs.abs(), "{worst} {peak} {ours} {theirs}");
}

#[test]
fn the_deck_writes_an_orthotropic_expansion_card_in_solver_sized_fields() {
    let v = crate::deck::fmt_val(1.5000000000000001e-4);
    assert!(v.len() <= 20 && (v.parse::<f64>().unwrap() - 1.5e-4).abs() < 1e-16, "{v}");
    assert_eq!(crate::deck::fmt_val(1.24e-9), "1.24E-9");
    let m = slender(|_| T_REF + DT);
    let c = printed_card();
    let opts = DeckOptions { thermal_strain: Some((c.alpha.unwrap(), c.t_ref_c)), ..Default::default() };
    let deck = write_deck(&DeckInput { mesh: &m, card: &c, orient: &OrientationCard::identity(), mounts: &both_ends(&m), load: &LoadSet::default(), opts: &opts }).unwrap();
    let card = deck.text.lines().skip_while(|l| !l.starts_with("*EXPANSION")).take(2).collect::<Vec<_>>();
    assert_eq!(card[0], "*EXPANSION,TYPE=ORTHO");
    assert!(card[1].split(',').all(|f| f.len() <= 20), "{}", card[1]);
}
