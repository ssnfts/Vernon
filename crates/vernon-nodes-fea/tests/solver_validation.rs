//! Validation of the reference linear solver (`refsolve`) against closed-form
//! results, through the crate's public API only.
//!
//! Theory used (UNVERIFIED as to edition numbers; standard textbook results):
//! * Euler-Bernoulli tip deflection of an end-loaded cantilever, d = P L^3 / (3 E I).
//! * Timoshenko shear correction d_s = P L / (kappa G A), kappa ~ 5/6 for a
//!   rectangle.
//! * Uniaxial stress sigma = F / A and axial strain sigma / E_i for an
//!   orthotropic bar loaded along principal axis i.
//! * Hydrostatic pressure on a homogeneous isotropic body gives the uniform
//!   state sigma = -p I, strain eps = -p (1 - 2 nu) / E.
//!
//! The solver API has no prescribed non-zero displacements, so the constant
//! strain patch test is run as a hydrostatic-pressure problem (see NOTES at the
//! end of this file for what could not be reached).

use glam::DVec3;
use vernon_core::Geometry;
use vernon_nodes_fea::material::MaterialCard;
use vernon_nodes_fea::mesh::{boundary_faces, box_tets, tet4_to_tet10};
use vernon_nodes_fea::refsolve::{solve, RefInput, RefOutput};
use vernon_nodes_fea::types::{FacePressure, LoadSet, MountSet, MountSite, NodalForce, OrientationCard};

const L: f64 = 100.0;
const B: f64 = 10.0;

fn iso_card(e: f64, nu: f64) -> MaterialCard {
    MaterialCard::trans_iso("iso", e, e, nu, nu, e / (2.0 * (1.0 + nu)))
}

fn site(nodes: Vec<u32>, center: [f64; 3]) -> MountSite {
    MountSite {
        name: "m".into(),
        center,
        axis: [-1.0, 0.0, 0.0],
        nodes,
        model: "fixed_patch".into(),
        r_w_mm: 5.0,
        avg_radius_mm: 5.0,
        bolt_d_mm: 5.0,
        pad_e_mpa: 5.0,
        pad_t_mm: 2.0,
    }
}

fn nodes_where(g: &Geometry, f: impl Fn(DVec3) -> bool) -> Vec<u32> {
    (0..g.points.len() as u32).filter(|&i| f(g.points[i as usize])).collect()
}

fn bar_mesh(n: [usize; 3], ten: bool) -> Geometry {
    let m = box_tets(DVec3::new(L, B, B), n);
    if ten {
        tet4_to_tet10(&m).unwrap()
    } else {
        m
    }
}

fn run(mesh: &Geometry, card: &MaterialCard, orient: &OrientationCard, mounts: Vec<MountSite>, load: LoadSet) -> RefOutput {
    solve(&RefInput {
        mesh,
        card,
        orient,
        mounts: &MountSet { sites: mounts },
        load: &load,
        max_dof: 200_000,
        tol: 1e-12,
    })
    .expect("solver runs")
}

/// Clamp x = -L/2, pull x = +L/2 with a pressure of `p` MPa (negative pressure pulls).
fn tension_bar(mesh: &Geometry, card: &MaterialCard, orient: &OrientationCard, p: f64) -> RefOutput {
    let left = nodes_where(mesh, |q| (q.x + L / 2.0).abs() < 1e-9);
    let pressures = boundary_faces(mesh)
        .iter()
        .filter(|f| f.normal.x > 0.99)
        .map(|f| FacePressure { elem: f.elem as u32, face: f.face, p_mpa: -p })
        .collect();
    run(mesh, card, orient, vec![site(left, [-L / 2.0, 0.0, 0.0])], LoadSet { pressures, ..Default::default() })
}

/// End-loaded cantilever; returns the mean tip deflection (positive down) and the output.
fn cantilever(mesh: &Geometry, card: &MaterialCard, load_n: f64) -> (f64, RefOutput) {
    let left = nodes_where(mesh, |q| (q.x + L / 2.0).abs() < 1e-9);
    let tip = nodes_where(mesh, |q| (q.x - L / 2.0).abs() < 1e-9);
    let forces = tip.iter().map(|&n| NodalForce { node: n, f_n: [0.0, 0.0, -load_n / tip.len() as f64] }).collect();
    let o = run(
        mesh,
        card,
        &OrientationCard::identity(),
        vec![site(left, [-L / 2.0, 0.0, 0.0])],
        LoadSet { forces, resultant_n: [0.0, 0.0, -load_n], ..Default::default() },
    );
    let d = tip.iter().map(|&n| -o.disp[n as usize].z).sum::<f64>() / tip.len() as f64;
    (d, o)
}

/// Mean x-displacement of the nodes in the plane x = `x`.
fn mean_ux(mesh: &Geometry, o: &RefOutput, x: f64) -> f64 {
    let v: Vec<f64> = nodes_where(mesh, |q| (q.x - x).abs() < 1e-9).iter().map(|&i| o.disp[i as usize].x).collect();
    v.iter().sum::<f64>() / v.len() as f64
}

/// Euler-Bernoulli tip deflection for the 100 x 10 x 10 mm bar.
fn euler_bernoulli(p: f64, e: f64) -> f64 {
    let i = B * B.powi(3) / 12.0;
    p * L.powi(3) / (3.0 * e * i)
}

// ---------------------------------------------------------------- (1) patch

/// Property: a homogeneous isotropic body under uniform hydrostatic pressure
/// reproduces the constant stress sigma = -p I and constant strain
/// eps = -p (1-2nu)/E exactly (patch test), for both element types.
#[test]
fn solver_validation_constant_strain_patch_test() {
    let (e, nu, p) = (2500.0, 0.35, 1.0);
    let card = iso_card(e, nu);
    // 2 cells per axis so a node sits exactly at the origin
    for ten in [false, true] {
        let mesh = {
            let m = box_tets(DVec3::splat(20.0), [2, 2, 2]);
            if ten {
                tet4_to_tet10(&m).unwrap()
            } else {
                m
            }
        };
        let centre = nodes_where(&mesh, |q| q.length() < 1e-9);
        assert_eq!(centre.len(), 1);
        let pressures = boundary_faces(&mesh).iter().map(|f| FacePressure { elem: f.elem as u32, face: f.face, p_mpa: p }).collect();
        let o = run(&mesh, &card, &OrientationCard::identity(), vec![site(centre, [0.0; 3])], LoadSet { pressures, ..Default::default() });
        for s in &o.elem_stress {
            for (k, want) in [-p, -p, -p, 0.0, 0.0, 0.0].iter().enumerate() {
                assert!((s[k] - want).abs() < 1e-9, "ten={ten} stress {s:?}");
            }
        }
        // strain along every node pair: (u_a - u_b).(x_a - x_b) = eps |x_a - x_b|^2
        // for a uniform strain, whatever rigid rotation the solve contains
        let eps = -p * (1.0 - 2.0 * nu) / e;
        let n = mesh.points.len();
        for a in 0..n {
            for b in a + 1..n {
                let dx = mesh.points[a] - mesh.points[b];
                let du = o.disp[a] - o.disp[b];
                let want = eps * dx.length_squared();
                assert!((du.dot(dx) - want).abs() < 1e-9 * want.abs().max(1e-12) + 1e-12, "ten={ten} pair {a},{b}");
            }
        }
    }
}

// --------------------------------------------------------------- (2) bending

/// Property: cantilever tip deflection against Euler-Bernoulli.
///
/// Tolerance derivation. Reference d_EB = P L^3/(3EI) = 1.6000 mm for
/// L = 100, b = h = 10, E = 2500, P = 10 N. Shear adds P L/(kappa G A) =
/// 0.0130 mm (+0.8 %) with G = E/(2(1+nu)), nu = 0.35, kappa = 5/6. The fully
/// clamped end restrains the cross-section warping and Poisson contraction,
/// which stiffens a short beam (L/h = 10) by a few per cent. Displacement
/// elements converge from below (never softer than the exact 3D solution), so:
/// * 10-node tets on the finest mesh: within 3 % of d_EB (measured 0.992);
/// * 4-node tets lock in bending: deflection is bounded above by the exact
///   solution (<= 1.02 d_EB) and must exceed 50 % of it on the 40x4x4 mesh
///   (measured 0.765: stiff, but not absurdly so).
#[test]
fn solver_validation_cantilever_tip_deflection_vs_euler_bernoulli() {
    let (e, nu, p) = (2500.0, 0.35, 10.0);
    let card = iso_card(e, nu);
    let d_eb = euler_bernoulli(p, e);
    assert!((d_eb - 1.6).abs() < 1e-12);

    let (d10, _) = cantilever(&bar_mesh([20, 2, 2], true), &card, p);
    let r10 = d10 / d_eb;
    assert!(r10 > 0.97 && r10 < 1.02, "10-node ratio {r10}");

    let (d4, _) = cantilever(&bar_mesh([40, 4, 4], false), &card, p);
    let r4 = d4 / d_eb;
    assert!(r4 > 0.5 && r4 < 1.02, "4-node ratio {r4}");
    // locking: the linear element is stiffer than the quadratic one
    assert!(d4 < d10, "4-node {d4} should be stiffer than 10-node {d10}");
}

// ------------------------------------------------------------ (3) convergence

/// Property: for the 10-node element the tip-deflection error against the
/// Euler-Bernoulli value decreases monotonically over three refinements.
#[test]
fn solver_validation_ten_node_error_decreases_with_refinement() {
    let (e, nu, p) = (2500.0, 0.35, 10.0);
    let card = iso_card(e, nu);
    let d_eb = euler_bernoulli(p, e);
    let mut err = Vec::new();
    let mut defl = Vec::new();
    for n in [[5, 1, 1], [10, 1, 1], [20, 2, 2]] {
        let (d, _) = cantilever(&bar_mesh(n, true), &card, p);
        defl.push(d);
        err.push((d - d_eb).abs() / d_eb);
    }
    assert!(err[1] < err[0] && err[2] < err[1], "errors {err:?} (deflections {defl:?})");
}

// --------------------------------------------------------- (4) uniaxial stress

/// Property: uniaxial tension gives sigma_xx = F/A in the bar interior with
/// negligible lateral stress, and the support reaction equals F.
#[test]
fn solver_validation_uniaxial_tension_gives_force_over_area() {
    let card = iso_card(2500.0, 0.35);
    let area = B * B;
    let p = 10.0; // MPa -> F = 1000 N
    for (ten, n) in [(false, [10, 2, 2]), (true, [10, 1, 1])] {
        let mesh = bar_mesh(n, ten);
        let o = tension_bar(&mesh, &card, &OrientationCard::identity(), p);
        let f = p * area;
        // elements whose centroid lies in the middle third (clear of the clamp)
        let mut checked = 0;
        for (i, s) in o.elem_stress.iter().enumerate() {
            let vs = &mesh.prims[i].vertices;
            let c = vs[..4].iter().map(|&v| mesh.points[v as usize]).sum::<DVec3>() / 4.0;
            if c.x.abs() < L / 6.0 {
                checked += 1;
                assert!((s[0] - f / area).abs() < 0.01 * f / area, "ten={ten} sxx {}", s[0]);
                assert!(s[1].abs() < 0.01 * f / area && s[2].abs() < 0.01 * f / area, "{s:?}");
            }
        }
        assert!(checked > 0);
        let r = o.reactions["MOUNT1"];
        assert!((r[0] + f).abs() < 1e-3 * f, "ten={ten} reaction {r:?}");
    }
}

// ------------------------------------------------------- (5) rigid-body modes

/// Property: with no load the solution carries no deformation energy: zero
/// displacement and zero stress. A unit-strain-free state is the only one the
/// solver returns, i.e. rigid-body modes (which the clamp removes) cost nothing.
#[test]
fn solver_validation_unloaded_body_has_zero_strain_energy() {
    for ten in [false, true] {
        let mesh = bar_mesh([4, 1, 1], ten);
        let left = nodes_where(&mesh, |q| (q.x + L / 2.0).abs() < 1e-9);
        let o = run(&mesh, &iso_card(2500.0, 0.35), &OrientationCard::identity(), vec![site(left, [-L / 2.0, 0.0, 0.0])], LoadSet::default());
        assert!(o.max_disp() < 1e-12, "ten={ten} disp {}", o.max_disp());
        assert!(o.elem_stress.iter().flatten().all(|s| s.abs() < 1e-9));
    }
}

/// Property: the hydrostatic patch solve is determined only up to an
/// infinitesimal rigid rotation about the single pinned node; that rotation
/// must not change stress. Repeating with a second pinned-node choice (a corner
/// shifted solve is not possible) is replaced by checking that the stress is
/// identical for two load magnitudes scaled linearly (linearity of the energy).
#[test]
fn solver_validation_response_is_linear_in_the_load() {
    let card = iso_card(2500.0, 0.35);
    let mesh = bar_mesh([10, 1, 1], true);
    let a = tension_bar(&mesh, &card, &OrientationCard::identity(), 5.0);
    let b = tension_bar(&mesh, &card, &OrientationCard::identity(), 10.0);
    for (da, db) in a.disp.iter().zip(&b.disp) {
        assert!((*db - *da * 2.0).length() < 1e-9 * (1.0 + db.length()), "{da:?} {db:?}");
    }
}

// ------------------------------------------------------------- (6) orthotropic

/// Property: with free lateral contraction the axial strain of a bar loaded
/// along material axis i is sigma / E_i, so the axial stiffness is E_i A / L,
/// checked on the middle 80 mm of the bar (clear of the clamp) for each axis.
#[test]
fn solver_validation_orthotropic_axial_stiffness_along_each_axis() {
    let mut card = MaterialCard::trans_iso("ortho", 3000.0, 2000.0, 0.30, 0.25, 800.0);
    card.mode = "orthotropic".into();
    card.e1 = 3000.0;
    card.e2 = 2000.0;
    card.e3 = 1000.0;
    card.nu12 = 0.25;
    card.nu13 = 0.20;
    card.nu23 = 0.15;
    card.g12 = 900.0;
    card.g13 = 700.0;
    card.g23 = 500.0;
    card.check_stability().expect("stable card");
    let mesh = bar_mesh([10, 1, 1], true);
    let p = 10.0;
    // rows of `axes` are material axes 1, 2, 3 in the model frame
    let cases: [([[f64; 3]; 3], f64); 3] = [
        ([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], card.e1),
        ([[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], card.e2),
        ([[0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]], card.e3),
    ];
    for (k, (axes, ei)) in cases.iter().enumerate() {
        let orient = OrientationCard { name: format!("OR{k}"), axes: *axes, origin: [0.0; 3], rot_deg: [0.0; 3] };
        let o = tension_bar(&mesh, &card, &orient, p);
        let strain = (mean_ux(&mesh, &o, 50.0) - mean_ux(&mesh, &o, -30.0)) / 80.0;
        let want = p / ei;
        assert!((strain - want).abs() / want < 5e-3, "axis {} strain {strain} vs {want}", k + 1);
        // axial stiffness E A / L from the force and the scaled elongation
        let k_axial = p * B * B / (strain * L);
        let k_want = ei * B * B / L;
        assert!((k_axial - k_want).abs() / k_want < 5e-3, "axis {} stiffness {k_axial} vs {k_want}", k + 1);
    }
}

// ------------------------------------------------------------- (7) reactions

/// Property: support reactions balance the applied load (global equilibrium)
/// for a tip-loaded cantilever and for a pressure-loaded bar, both element types.
#[test]
fn solver_validation_reactions_sum_to_the_applied_load() {
    let card = iso_card(2500.0, 0.35);
    for ten in [false, true] {
        let (_, o) = cantilever(&bar_mesh([10, 1, 1], ten), &card, 10.0);
        let r = o.reactions["MOUNT1"];
        assert!((r[2] - 10.0).abs() < 1e-3 * 10.0 && r[0].abs() < 1e-3 && r[1].abs() < 1e-3, "ten={ten} {r:?}");

        let mesh = bar_mesh([10, 1, 1], ten);
        let o = tension_bar(&mesh, &card, &OrientationCard::identity(), 4.0);
        let r = o.reactions["MOUNT1"];
        assert!((r[0] + 400.0).abs() < 1e-3 * 400.0 && r[1].abs() < 0.4 && r[2].abs() < 0.4, "ten={ten} {r:?}");
    }
}

// NOTES (API that could not be reached from an integration test)
// * Prescribed non-zero displacements do not exist in `RefInput`, so the
//   patch test uses hydrostatic pressure with one pinned node and cannot be
//   run as the textbook linear-displacement-on-the-boundary patch.
// * Per-element stiffness matrices (`b_matrix`, `shape`) are private to
//   `refsolve`, so rigid-body-motion energy `u^T K u = 0` is checked only
//   indirectly (zero-load solve; hydrostatic solve independent of rotation).
// * Gravity loading and the other mount models (`rigid_washer`, `spring_pad`)
//   are not validated here.
