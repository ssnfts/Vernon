//! Versioned binding to the explicitly scoped published-replication profile.
//! Geometry entries record independently captured prepared-input identities;
//! registration does not establish physical benchmark acceptance.

use std::path::Path;

use serde::{Deserialize, Serialize};
use vernon_graph::{CookContext, CookResult};

use crate::coeffs::ForceResult;
use crate::foam::CaseSpec;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeometryBinding {
    pub version: u32,
    pub candidate: String,
    pub preparation_revision: String,
    pub tri_surface_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceBinding {
    pub version: u32,
    pub profile: String,
    pub geometry: GeometryBinding,
    pub u_ms: f64,
    pub rho: f64,
    pub nu: f64,
    pub full_area_m2: f64,
    pub reference_length_m: f64,
    pub half_model: bool,
    pub ground_mode: String,
    pub wheel_mode: String,
    pub wheel_omega_rad_s: f64,
}

// No case-name or test-token entries may stand in for captured real geometry.
struct RegisteredGeometry<'a> {
    version: u32,
    candidate: &'a str,
    preparation_revision: &'a str,
    tri_surface_fingerprint: &'a str,
    wheel_fr_m: [f64; 3],
    wheel_rr_m: [f64; 3],
}

// Captured identities and centres from geometry.prepared_cases in
// docs/research/wp9-replication-baseline-v1.json; no filename-only inference.
const REGISTERED_GEOMETRY: &[RegisteredGeometry<'static>] = &[
    RegisteredGeometry {
        version: 1,
        candidate: "drivaer_fastback",
        preparation_revision: "vehicle-prep-minus-x-half-sink2-mm-v1",
        tri_surface_fingerprint: "4de3bd09d927f4dc",
        wheel_fr_m: [0.8148519935, 0.760269436, 0.31650019599999996],
        wheel_rr_m: [3.601033365, 0.7634309750000001, 0.316500243],
    },
    RegisteredGeometry {
        version: 1,
        candidate: "drivaer_estate",
        preparation_revision: "vehicle-prep-minus-x-half-sink2-mm-v1",
        tri_surface_fingerprint: "b1a1d6d4525fcb1c",
        wheel_fr_m: [0.8148519935, 0.760269436, 0.31650019599999996],
        wheel_rr_m: [3.601033365, 0.7634309750000001, 0.316500243],
    },
    RegisteredGeometry {
        version: 1,
        candidate: "drivaer_notchback",
        preparation_revision: "vehicle-prep-minus-x-half-sink2-mm-v1",
        tri_surface_fingerprint: "f8399fd496485c02",
        wheel_fr_m: [0.8148519935, 0.760269436, 0.31650019599999996],
        wheel_rr_m: [3.601033365, 0.7634309750000001, 0.316500243],
    },
];
const PROFILE: &str = "road_car_detailed_gs_16ms_v1";

fn same(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-12 + 1e-9 * a.abs().max(b.abs())
}

fn known_candidate(candidate: &str) -> bool {
    matches!(candidate, "drivaer_fastback" | "drivaer_estate" | "drivaer_notchback")
}

fn valid_registration(entry: &RegisteredGeometry<'_>) -> bool {
    entry.version == 1
        && known_candidate(entry.candidate)
        && !entry.preparation_revision.trim().is_empty()
        && entry.tri_surface_fingerprint.len() == 16
        && entry.tri_surface_fingerprint.bytes().all(|b| b.is_ascii_hexdigit())
        && entry.wheel_fr_m.iter().chain(&entry.wheel_rr_m).all(|value| value.is_finite())
}

fn registered_entry<'r, 's>(geometry: &GeometryBinding, registry: &'r [RegisteredGeometry<'s>]) -> Option<&'r RegisteredGeometry<'s>> {
    registry.iter().find(|entry| {
        valid_registration(entry)
            && geometry.version == entry.version
            && geometry.candidate == entry.candidate
            && geometry.preparation_revision == entry.preparation_revision
            && geometry.tri_surface_fingerprint == entry.tri_surface_fingerprint
    })
}

fn wheel_layout_matches(spec: &CaseSpec<'_>, entry: &RegisteredGeometry<'_>) -> bool {
    if spec.bc.wheels.len() != 2 || !same(spec.bc.r_dyn_m, 16.0 / 51.0) {
        return false;
    }
    for (name, expected) in [("wheel_fr", entry.wheel_fr_m), ("wheel_rr", entry.wheel_rr_m)] {
        let mut wheels = spec.bc.wheels.iter().filter(|wheel| wheel.name == name);
        let Some(wheel) = wheels.next() else { return false };
        if wheels.next().is_some() || !wheel.centre_m.iter().zip(expected).all(|(actual, target)| same(*actual, target)) {
            return false;
        }
        if !spec
            .rf
            .wheel_centres_m
            .get(name)
            .is_some_and(|centre| centre.iter().zip(expected).all(|(actual, target)| same(*actual, target)))
        {
            return false;
        }
    }
    true
}

fn reference_patches_match(spec: &CaseSpec<'_>) -> bool {
    let mut seen = [false; 5];
    for patch in &spec.rf.patches {
        let index = match patch.as_str() {
            "body" => 0,
            "wheel_fr" => 1,
            "wheel_rr" => 2,
            // Default references can include the absent left half. These
            // literal names do not replace either required right wheel.
            "wheel_fl" => 3,
            "wheel_rl" => 4,
            _ => return false,
        };
        if seen[index] {
            return false;
        }
        seen[index] = true;
    }
    seen[..3].iter().all(|present| *present)
}

fn domain_matches(spec: &CaseSpec<'_>) -> bool {
    let domain = spec.domain;
    let reference = spec.rf;
    if !domain.min_m.iter().chain(&domain.max_m).all(|value| value.is_finite())
        || ![
            domain.yaw_deg,
            domain.blockage_pct,
            domain.cross_section_m2,
            domain.ref_length_m,
            domain.up_l,
            domain.down_l,
            reference.nose_x_m,
            reference.tail_x_m,
        ]
        .iter()
        .all(|value| value.is_finite())
        || reference.tail_x_m <= reference.nose_x_m
        || domain.up_l < 3.0
        || domain.down_l < 6.0
        || domain.blockage_pct <= 0.0
        || domain.blockage_pct > 1.0
        || domain.cross_section_m2 <= 0.0
        || !same(domain.min_m[1], 0.0)
        || !same(domain.min_m[2], 0.0)
    {
        return false;
    }
    let spans = domain.size();
    if !spans.iter().all(|span| span.is_finite() && *span > 0.0) {
        return false;
    }
    // Cross-section and blockage describe the full tunnel, although this
    // profile solves only its y >= 0 half. The normalization length need not
    // equal the prepared vehicle's nose-to-tail span.
    let cross_section = 2.0 * spans[1] * spans[2];
    same(domain.cross_section_m2, cross_section)
        && same(domain.blockage_pct, 100.0 * reference.a_front_m2 / cross_section)
        && same(domain.min_m[0], reference.nose_x_m - domain.up_l * reference.length_m)
        && same(domain.max_m[0], reference.tail_x_m + domain.down_l * reference.length_m)
}

pub(crate) fn geometry_binding(ctx: &CookContext, case: &Path) -> CookResult<Option<GeometryBinding>> {
    geometry_binding_with_registry(ctx, case, REGISTERED_GEOMETRY)
}

fn geometry_binding_with_registry(ctx: &CookContext, case: &Path, registry: &[RegisteredGeometry<'_>]) -> CookResult<Option<GeometryBinding>> {
    // Read even with an empty registry: cancellation and read errors are not
    // silently reclassified as a successful unknown-geometry lookup.
    let fingerprint = crate::util::tri_surface_fingerprint(ctx, case)?;
    let mut candidates = registry
        .iter()
        .filter(|entry| valid_registration(entry) && entry.tri_surface_fingerprint == fingerprint);
    let Some(entry) = candidates.next() else { return Ok(None) };
    // Ambiguous identities cannot select a candidate by registry ordering.
    if candidates.next().is_some() {
        return Ok(None);
    }
    Ok(Some(GeometryBinding {
        version: entry.version,
        candidate: entry.candidate.into(),
        preparation_revision: entry.preparation_revision.into(),
        tri_surface_fingerprint: fingerprint,
    }))
}

pub(crate) fn match_case_protocol(geometry: &GeometryBinding, spec: &CaseSpec<'_>, solver_mode: &str) -> Option<ReferenceBinding> {
    match_case_protocol_with_registry(geometry, spec, solver_mode, REGISTERED_GEOMETRY)
}

fn match_case_protocol_with_registry(
    geometry: &GeometryBinding,
    spec: &CaseSpec<'_>,
    solver_mode: &str,
    registry: &[RegisteredGeometry<'_>],
) -> Option<ReferenceBinding> {
    let entry = registered_entry(geometry, registry)?;
    if !wheel_layout_matches(spec, entry)
        || !reference_patches_match(spec)
        || !domain_matches(spec)
        || solver_mode != "steady_sst"
        || spec.flavour != crate::foam::Flavour::OpenCfd(2606)
        || !spec.zones.is_empty()
        || spec.nonortho_over_70
        || !spec.domain.half_model
        || !spec.rf.half_model
        || !same(spec.domain.yaw_deg, 0.0)
        || !same(spec.domain.ref_length_m, 4.6)
        || !same(spec.rf.length_m, 4.6)
        || !same(spec.rf.a_front_m2, 2.16)
        || !same(spec.u, 16.0)
        || !same(spec.nu, 1.567e-5)
        || !same(spec.rho, 1.184)
        || !same(spec.i_pct, 0.25)
        || !same(spec.nut_ratio, 10.0)
        || spec.bc.ground_mode != "moving"
        || spec.bc.belt_start_x_m.is_some()
        || spec.bc.wheel_mode != "rotating"
        || !same(spec.bc.u_ms, 16.0)
        || !same(spec.bc.omega_rad_s, -51.0)
        || !spec.bc.axis.iter().zip([0.0, 1.0, 0.0]).all(|(a, b)| same(*a, b))
    {
        return None;
    }
    Some(ReferenceBinding {
        version: 1,
        profile: PROFILE.into(),
        geometry: geometry.clone(),
        u_ms: spec.u,
        rho: spec.rho,
        nu: spec.nu,
        full_area_m2: spec.rf.a_front_m2,
        reference_length_m: spec.rf.length_m,
        half_model: true,
        ground_mode: spec.bc.ground_mode.clone(),
        wheel_mode: spec.bc.wheel_mode.clone(),
        wheel_omega_rad_s: spec.bc.omega_rad_s,
    })
}

pub(crate) fn matches_result(binding: &ReferenceBinding, preset_id: &str, result: &ForceResult) -> bool {
    matches_result_with_registry(binding, preset_id, result, REGISTERED_GEOMETRY)
}

/// Validate the supported profile and registered geometry without requiring a
/// result. Case-record readers still verify current inputs independently.
pub fn binding_supported(binding: &ReferenceBinding) -> bool {
    binding_supported_with_registry(binding, REGISTERED_GEOMETRY)
}

fn binding_supported_with_registry(binding: &ReferenceBinding, registry: &[RegisteredGeometry<'_>]) -> bool {
    binding.version == 1
        && binding.profile == PROFILE
        && registered_entry(&binding.geometry, registry).is_some()
        && same(binding.u_ms, 16.0)
        && same(binding.rho, 1.184)
        && same(binding.nu, 1.567e-5)
        && same(binding.full_area_m2, 2.16)
        && same(binding.reference_length_m, 4.6)
        && binding.half_model
        && binding.ground_mode == "moving"
        && binding.wheel_mode == "rotating"
        && same(binding.wheel_omega_rad_s, -51.0)
}

fn matches_result_with_registry(binding: &ReferenceBinding, preset_id: &str, result: &ForceResult, registry: &[RegisteredGeometry<'_>]) -> bool {
    let Some(recipe) = &result.recipe else { return false };
    binding_supported_with_registry(binding, registry)
        && binding.geometry.candidate == preset_id
        && !result.placeholder
        && same(result.u_ms, binding.u_ms)
        && same(result.rho, binding.rho)
        && same(result.q_pa, 0.5 * binding.rho * binding.u_ms * binding.u_ms)
        && same(result.a_front_m2, binding.full_area_m2)
        && same(result.aref_m2, 0.5 * binding.full_area_m2)
        && result.half_model
        && result.cd.is_finite()
        && result.cd > 0.0
        && same(result.cda, result.cd * binding.full_area_m2)
        && recipe.ground_mode == binding.ground_mode
        && recipe.wheel_mode == binding.wheel_mode
        && same(recipe.yaw_deg, 0.0)
        && recipe.turbulence == "kOmegaSST I=0.250% nut/nu=10.0"
        && !recipe.separated_flow
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foam::Flavour;
    use crate::types::{BcSet, Domain, Recipe, RefGeom, WheelBc};
    use crate::util::testutil::ctx;

    const PROFILE: &str = "road_car_detailed_gs_16ms_v1";
    const WHEEL_FR_M: [f64; 3] = [0.8148519935, 0.760269436, 0.31650019599999996];
    const WHEEL_RR_M: [f64; 3] = [3.601033365, 0.7634309750000001, 0.316500243];

    fn synthetic_geometry(candidate: &str) -> GeometryBinding {
        GeometryBinding {
            version: 1,
            candidate: candidate.into(),
            preparation_revision: "synthetic-unit-preparation-v1".into(),
            // Independent shared triSurface framing fixture, not real CAD.
            tri_surface_fingerprint: "67577c1e3392b746".into(),
        }
    }

    fn registration(geometry: &GeometryBinding) -> RegisteredGeometry<'_> {
        RegisteredGeometry {
            version: geometry.version,
            candidate: &geometry.candidate,
            preparation_revision: &geometry.preparation_revision,
            tri_surface_fingerprint: &geometry.tri_surface_fingerprint,
            wheel_fr_m: WHEEL_FR_M,
            wheel_rr_m: WHEEL_RR_M,
        }
    }

    fn protocol(geometry: &GeometryBinding, spec: &CaseSpec<'_>, solver: &str) -> Option<ReferenceBinding> {
        let registered = synthetic_geometry(&geometry.candidate);
        match_case_protocol_with_registry(geometry, spec, solver, &[registration(&registered)])
    }

    fn matched(binding: &ReferenceBinding, candidate: &str, result: &ForceResult) -> bool {
        let registered = synthetic_geometry("drivaer_fastback");
        matches_result_with_registry(binding, candidate, result, &[registration(&registered)])
    }

    fn binding(candidate: &str) -> ReferenceBinding {
        ReferenceBinding {
            version: 1,
            profile: PROFILE.into(),
            geometry: synthetic_geometry(candidate),
            u_ms: 16.0,
            rho: 1.184,
            nu: 1.567e-5,
            full_area_m2: 2.16,
            reference_length_m: 4.6,
            half_model: true,
            ground_mode: "moving".into(),
            wheel_mode: "rotating".into(),
            wheel_omega_rad_s: -51.0,
        }
    }

    fn inputs() -> (RefGeom, Domain, BcSet) {
        let mut reference = RefGeom::reference_car();
        reference.wheel_centres_m.insert("wheel_fr".into(), WHEEL_FR_M);
        reference.wheel_centres_m.insert("wheel_rr".into(), WHEEL_RR_M);
        let domain = crate::tunnel::size_domain(&reference, 1.0, 3.0, 6.0, 5.0 / 3.0, true, 0.0);
        let boundary = BcSet {
            ground_mode: "moving".into(),
            wheel_mode: "rotating".into(),
            u_ms: 16.0,
            omega_rad_s: -51.0,
            r_dyn_m: 16.0 / 51.0,
            axis: [0.0, 1.0, 0.0],
            wheel_width_m: 0.22,
            belt_start_x_m: None,
            wheels: ["wheel_fr", "wheel_rr"]
                .iter()
                .map(|name| WheelBc {
                    name: (*name).into(),
                    centre_m: reference.wheel_centres_m[*name],
                })
                .collect(),
        };
        (reference, domain, boundary)
    }

    fn spec<'a>(reference: &'a RefGeom, domain: &'a Domain, boundary: &'a BcSet) -> CaseSpec<'a> {
        CaseSpec {
            flavour: Flavour::OpenCfd(2606),
            domain,
            rf: reference,
            bc: boundary,
            zones: &[],
            u: 16.0,
            nu: 1.567e-5,
            rho: 1.184,
            i_pct: 0.25,
            nut_ratio: 10.0,
            n_iter: 3000,
            window: 1000,
            residual: 1e-5,
            write_interval: 500,
            nonortho_over_70: false,
            monitor_patches: Vec::new(),
        }
    }

    fn prepared_case(name: &str) -> (CookContext, std::path::PathBuf) {
        let context = ctx(name);
        let case = context.work_dir.join("case");
        let surfaces = case.join("constant/triSurface");
        std::fs::create_dir_all(&surfaces).unwrap();
        for name in ["body", "wheel_fr", "wheel_rr"] {
            std::fs::write(surfaces.join(format!("{name}.stl")), format!("solid {name}\nendsolid {name}\n")).unwrap();
        }
        (context, case)
    }

    fn result() -> ForceResult {
        ForceResult {
            source: "coefficients".into(),
            u_ms: 16.0,
            rho: 1.184,
            q_pa: 0.5 * 1.184 * 16.0 * 16.0,
            a_front_m2: 2.16,
            aref_m2: 1.08,
            half_model: true,
            cd: 0.275,
            cda: 0.275 * 2.16,
            recipe: Some(Recipe {
                ground_mode: "moving".into(),
                wheel_mode: "rotating".into(),
                yaw_deg: 0.0,
                turbulence: "kOmegaSST I=0.250% nut/nu=10.0".into(),
                ..Recipe::default()
            }),
            ..ForceResult::default()
        }
    }

    #[test]
    fn injected_geometry_registry_matches_content_without_creating_sidecars() {
        let (context, case) = prepared_case("reference-registered");
        let registered = synthetic_geometry("drivaer_fastback");
        let actual = geometry_binding_with_registry(&context, &case, &[registration(&registered)]).unwrap();
        assert_eq!(actual, Some(registered));
        assert!(!case.join("vernon_replication.json").exists());
        assert!(!case.join("constant/.vernon-mesh-evidence.json").exists());
    }

    #[test]
    fn production_captured_registry_never_accepts_names_or_synthetic_tokens() {
        assert_eq!(REGISTERED_GEOMETRY.len(), 3);
        assert!(REGISTERED_GEOMETRY.iter().all(|entry| entry.tri_surface_fingerprint != "67577c1e3392b746"));
        let (context, case) = prepared_case("drivaer_fastback-road_car_detailed_gs_16ms_v1");
        assert!(geometry_binding(&context, &case).unwrap().is_none());
        let (reference, domain, boundary) = inputs();
        assert!(match_case_protocol(&synthetic_geometry("drivaer_fastback"), &spec(&reference, &domain, &boundary), "steady_sst").is_none());
        assert!(!matches_result(&binding("drivaer_fastback"), "drivaer_fastback", &result()));
        assert!(!binding_supported(&binding("drivaer_fastback")));
    }

    #[test]
    fn registry_rejects_changed_surfaces_and_invalid_entries() {
        let (context, case) = prepared_case("reference-changed");
        let registered = synthetic_geometry("drivaer_fastback");
        std::fs::write(case.join("constant/triSurface/body.stl"), "solid edit\nendsolid edit\n").unwrap();
        assert!(geometry_binding_with_registry(&context, &case, &[registration(&registered)]).unwrap().is_none());
        std::fs::write(case.join("constant/triSurface/body.stl"), "solid body\nendsolid body\n").unwrap();
        for mode in 0..4 {
            let mut invalid = registered.clone();
            match mode {
                0 => invalid.version = 2,
                1 => invalid.candidate = "unknown".into(),
                2 => invalid.preparation_revision.clear(),
                _ => invalid.tri_surface_fingerprint = "bad".into(),
            }
            assert!(geometry_binding_with_registry(&context, &case, &[registration(&invalid)]).unwrap().is_none());
        }
    }

    #[test]
    fn geometry_read_errors_and_cancellation_are_not_unknown_geometry() {
        let context = ctx("reference-errors");
        assert!(geometry_binding(&context, &context.work_dir.join("missing")).is_err());
        let (context, case) = prepared_case("reference-cancel");
        let cancelled = vernon_core::process::with_cancel(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)), || {
            geometry_binding(&context, &case)
        });
        assert!(cancelled.is_err());
    }

    #[test]
    fn exact_case_protocol_binds_each_registered_candidate() {
        let (reference, domain, boundary) = inputs();
        let case = spec(&reference, &domain, &boundary);
        for candidate in ["drivaer_fastback", "drivaer_estate", "drivaer_notchback"] {
            assert_eq!(protocol(&synthetic_geometry(candidate), &case, "steady_sst"), Some(binding(candidate)));
        }
    }

    #[test]
    fn typed_records_still_require_exact_registry_membership() {
        let registered = synthetic_geometry("drivaer_fastback");
        let registry = [registration(&registered)];
        let (reference, domain, boundary) = inputs();
        let case = spec(&reference, &domain, &boundary);
        for mode in 0..3 {
            let mut forged = binding("drivaer_fastback");
            match mode {
                0 => forged.geometry.candidate = "drivaer_estate".into(),
                1 => forged.geometry.preparation_revision = "another-nonempty-revision".into(),
                _ => forged.geometry.tri_surface_fingerprint = "0123456789abcdef".into(),
            }
            assert!(match_case_protocol_with_registry(&forged.geometry, &case, "steady_sst", &registry).is_none());
            assert!(!matches_result_with_registry(&forged, &forged.geometry.candidate, &result(), &registry));
        }
    }

    #[test]
    fn protocol_rejects_edited_flow_solver_and_turbulence() {
        let (reference, domain, boundary) = inputs();
        let geometry = synthetic_geometry("drivaer_fastback");
        for mode in 0..10 {
            let mut case = spec(&reference, &domain, &boundary);
            match mode {
                0 => case.u = 30.0,
                1 => case.rho = 1.225,
                2 => case.nu = 1.5e-5,
                3 => case.i_pct = 0.5,
                4 => case.nut_ratio = 5.0,
                5 => case.flavour = Flavour::Foundation(11),
                6 => case.flavour = Flavour::OpenCfd(2512),
                7 => case.u = f64::NAN,
                8 => case.rho = f64::INFINITY,
                _ => case.nonortho_over_70 = true,
            }
            assert!(protocol(&geometry, &case, "steady_sst").is_none(), "edited case {mode}");
        }
        let case = spec(&reference, &domain, &boundary);
        for solver in ["transient_ddes", "unknown", ""] {
            assert!(protocol(&geometry, &case, solver).is_none());
        }
        let zones = [crate::types::PorousZone {
            name: "added-core".into(),
            bbox_m: Some([[0.0; 3], [1.0; 3]]),
            flow_dir: [1.0, 0.0, 0.0],
            d: [1.0; 3],
            f: [1.0; 3],
        }];
        let mut with_zones = spec(&reference, &domain, &boundary);
        with_zones.zones = &zones;
        assert!(protocol(&geometry, &with_zones, "steady_sst").is_none());
    }

    #[test]
    fn protocol_rejects_changed_scale_half_yaw_and_wheel_treatment() {
        let geometry = synthetic_geometry("drivaer_fastback");
        for mode in 0..12 {
            let (mut reference, mut domain, mut boundary) = inputs();
            match mode {
                0 => reference.a_front_m2 = 2.0,
                1 => reference.length_m = 1.84,
                2 => domain.half_model = false,
                3 => reference.half_model = false,
                4 => domain.yaw_deg = 1.0,
                5 => boundary.ground_mode = "stationary".into(),
                6 => boundary.wheel_mode = "static".into(),
                7 => boundary.omega_rad_s = 51.0,
                8 => boundary.axis = [1.0, 0.0, 0.0],
                9 => boundary.u_ms = 30.0,
                10 => boundary.omega_rad_s = f64::NAN,
                _ => domain.ref_length_m = 1.84,
            }
            assert!(
                protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst").is_none(),
                "edited boundary/reference {mode}"
            );
        }
    }

    #[test]
    fn protocol_wheel_layout_requires_exactly_two_distinct_registered_right_names() {
        let geometry = synthetic_geometry("drivaer_fastback");
        for mutation in ["missing", "extra", "duplicate", "unknown", "left", "pattern"] {
            let (reference, domain, mut boundary) = inputs();
            match mutation {
                "missing" => {
                    boundary.wheels.pop();
                }
                "extra" => boundary.wheels.push(WheelBc {
                    name: "wheel_fl".into(),
                    centre_m: WHEEL_FR_M,
                }),
                "duplicate" => boundary.wheels[1] = boundary.wheels[0].clone(),
                "unknown" => boundary.wheels[0].name = "front_right".into(),
                "left" => boundary.wheels[1].name = "wheel_rl".into(),
                _ => boundary.wheels[0].name = "wheel_fr.*".into(),
            }
            assert!(
                protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst").is_none(),
                "accepted {mutation} wheel layout"
            );
        }
    }

    #[test]
    fn protocol_wheel_centres_bind_boundary_and_reference_to_registry() {
        let geometry = synthetic_geometry("drivaer_fastback");
        for mutation in [
            "boundary_x",
            "boundary_y",
            "boundary_z",
            "swapped",
            "nonfinite",
            "reference",
            "both",
            "missing_reference",
        ] {
            let (mut reference, domain, mut boundary) = inputs();
            match mutation {
                "boundary_x" => boundary.wheels[0].centre_m[0] += 0.001,
                "boundary_y" => boundary.wheels[1].centre_m[1] += 0.001,
                "boundary_z" => boundary.wheels[0].centre_m[2] += 0.001,
                "swapped" => boundary.wheels[0].centre_m = WHEEL_RR_M,
                "nonfinite" => boundary.wheels[0].centre_m[0] = f64::NAN,
                "reference" => reference.wheel_centres_m.get_mut("wheel_rr").unwrap()[0] += 0.001,
                "both" => {
                    boundary.wheels[0].centre_m[0] += 0.001;
                    reference.wheel_centres_m.get_mut("wheel_fr").unwrap()[0] = boundary.wheels[0].centre_m[0];
                }
                _ => {
                    reference.wheel_centres_m.shift_remove("wheel_fr");
                }
            }
            assert!(
                protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst").is_none(),
                "accepted {mutation} centres"
            );
        }
    }

    #[test]
    fn protocol_wheel_axis_and_dynamic_radius_are_fixed_finite_profile_inputs() {
        let geometry = synthetic_geometry("drivaer_fastback");
        for radius in [0.0, -16.0 / 51.0, 0.4, f64::NAN, f64::INFINITY] {
            let (reference, domain, mut boundary) = inputs();
            boundary.r_dyn_m = radius;
            assert!(
                protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst").is_none(),
                "accepted radius {radius}"
            );
        }
        for axis in [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0; 3], [0.0, f64::NAN, 0.0]] {
            let (reference, domain, mut boundary) = inputs();
            boundary.axis = axis;
            assert!(
                protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst").is_none(),
                "accepted axis {axis:?}"
            );
        }
    }

    #[test]
    fn protocol_registered_wheels_are_matched_by_name_in_either_order() {
        let geometry = synthetic_geometry("drivaer_fastback");
        let (reference, domain, mut boundary) = inputs();
        boundary.wheels.reverse();
        assert_eq!(
            protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst"),
            Some(binding("drivaer_fastback"))
        );
    }

    #[test]
    fn wp9_protocol_safety_requires_each_body_and_right_wheel_patch() {
        let geometry = synthetic_geometry("drivaer_fastback");
        for missing in ["all", "body", "wheel_fr", "wheel_rr"] {
            let (mut reference, domain, boundary) = inputs();
            reference.patches = ["body", "wheel_fr", "wheel_rr"]
                .into_iter()
                .filter(|name| missing != "all" && *name != missing)
                .map(str::to_owned)
                .collect();
            assert!(
                protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst").is_none(),
                "accepted reference with missing {missing} patches"
            );
        }
    }

    #[test]
    fn wp9_protocol_safety_rejects_unknown_and_pattern_patch_selectors() {
        let geometry = synthetic_geometry("drivaer_fastback");
        for selector in ["", "body.*", "wheel_.*", "wheel_fr.*", "body|wheel_fr", "\"body\"", "underbody"] {
            let (mut reference, domain, boundary) = inputs();
            reference.patches = ["body", "wheel_fr", "wheel_rr", selector]
                .into_iter()
                .map(str::to_owned)
                .collect();
            assert!(
                protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst").is_none(),
                "accepted extra patch selector {selector:?}"
            );
        }
    }

    #[test]
    fn wp9_protocol_safety_rejects_duplicate_reference_patches() {
        let geometry = synthetic_geometry("drivaer_fastback");
        for duplicate in ["body", "wheel_fr", "wheel_rr", "wheel_fl", "wheel_rl"] {
            let (mut reference, domain, boundary) = inputs();
            reference.patches.push(duplicate.into());
            assert!(
                protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst").is_none(),
                "accepted duplicate reference patch {duplicate}"
            );
        }
    }

    #[test]
    fn wp9_protocol_safety_accepts_literal_half_patches_and_optional_default_left_names() {
        let geometry = synthetic_geometry("drivaer_fastback");
        for names in [
            vec!["body", "wheel_fr", "wheel_rr"],
            vec!["wheel_rr", "body", "wheel_fr"],
            vec!["body", "wheel_fl", "wheel_fr", "wheel_rl", "wheel_rr"],
            vec!["wheel_rr", "wheel_rl", "wheel_fr", "wheel_fl", "body"],
            vec!["wheel_fl", "wheel_rr", "body", "wheel_fr"],
            vec!["wheel_rr", "wheel_rl", "body", "wheel_fr"],
        ] {
            let (mut reference, domain, boundary) = inputs();
            reference.patches = names.iter().map(|name| (*name).into()).collect();
            assert_eq!(
                protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst"),
                Some(binding("drivaer_fastback")),
                "rejected permitted literal patches {names:?}"
            );
        }
    }

    #[test]
    fn wp9_protocol_safety_requires_whole_ground_movement_without_belt_start() {
        let geometry = synthetic_geometry("drivaer_fastback");
        for start in [-2.3, 0.0, 1.0, f64::NAN, f64::INFINITY] {
            let (reference, domain, mut boundary) = inputs();
            boundary.belt_start_x_m = Some(start);
            assert!(
                protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst").is_none(),
                "accepted optional belt start {start}"
            );
        }
    }

    #[test]
    fn wp9_protocol_safety_rejects_short_coherent_upstream_or_downstream_domains() {
        let geometry = synthetic_geometry("drivaer_fastback");
        let (reference, _, boundary) = inputs();
        for (up, down) in [(2.9, 6.0), (3.0, 5.9), (0.0, 6.0), (3.0, -1.0)] {
            let domain = crate::tunnel::size_domain(&reference, 1.0, up, down, 5.0 / 3.0, true, 0.0);
            assert!(
                protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst").is_none(),
                "accepted upstream {up} L, downstream {down} L"
            );
        }
    }

    #[test]
    fn wp9_protocol_safety_rejects_coherent_blockage_above_one_percent() {
        let geometry = synthetic_geometry("drivaer_fastback");
        let (reference, _, boundary) = inputs();
        for blockage in [1.0001, 1.5, 2.0] {
            let domain = crate::tunnel::size_domain(&reference, blockage, 3.0, 6.0, 5.0 / 3.0, true, 0.0);
            assert!(
                protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst").is_none(),
                "accepted blockage {blockage}%"
            );
        }
    }

    #[test]
    fn wp9_protocol_safety_rejects_nonfinite_domain_components() {
        let geometry = synthetic_geometry("drivaer_fastback");
        for field in 0..12 {
            for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                let (reference, mut domain, boundary) = inputs();
                match field {
                    0..=2 => domain.min_m[field] = invalid,
                    3..=5 => domain.max_m[field - 3] = invalid,
                    6 => domain.up_l = invalid,
                    7 => domain.down_l = invalid,
                    8 => domain.blockage_pct = invalid,
                    9 => domain.cross_section_m2 = invalid,
                    10 => domain.ref_length_m = invalid,
                    _ => domain.yaw_deg = invalid,
                }
                assert!(
                    protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst").is_none(),
                    "accepted nonfinite domain field {field}: {invalid}"
                );
            }
        }
    }

    #[test]
    fn wp9_protocol_safety_rejects_incoherent_domain_bounds_and_metrics() {
        let geometry = synthetic_geometry("drivaer_fastback");
        for mutation in [
            "inlet", "outlet", "upstream", "downstream", "area", "blockage", "half_width", "height",
            "zero_length", "zero_width", "zero_height", "reverse_x", "reverse_y", "reverse_z",
            "zero_blockage", "negative_blockage", "zero_area", "negative_area", "nose", "tail", "reversed_car",
        ] {
            let (mut reference, mut domain, boundary) = inputs();
            match mutation {
                "inlet" => domain.min_m[0] += 0.1,
                "outlet" => domain.max_m[0] -= 0.1,
                "upstream" => domain.up_l += 1.0,
                "downstream" => domain.down_l += 1.0,
                "area" => domain.cross_section_m2 *= 0.5,
                "blockage" => domain.blockage_pct *= 0.5,
                "half_width" => domain.max_m[1] *= 0.5,
                "height" => domain.max_m[2] *= 2.0,
                "zero_length" => domain.max_m[0] = domain.min_m[0],
                "zero_width" => domain.max_m[1] = domain.min_m[1],
                "zero_height" => domain.max_m[2] = domain.min_m[2],
                "reverse_x" => domain.max_m[0] = domain.min_m[0] - 1.0,
                "reverse_y" => domain.max_m[1] = domain.min_m[1] - 1.0,
                "reverse_z" => domain.max_m[2] = domain.min_m[2] - 1.0,
                "zero_blockage" => domain.blockage_pct = 0.0,
                "negative_blockage" => domain.blockage_pct = -1.0,
                "zero_area" => domain.cross_section_m2 = 0.0,
                "negative_area" => domain.cross_section_m2 = -216.0,
                "nose" => reference.nose_x_m = f64::NAN,
                "tail" => reference.tail_x_m = f64::INFINITY,
                _ => reference.tail_x_m = reference.nose_x_m - 1.0,
            }
            assert!(
                protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst").is_none(),
                "accepted incoherent domain/reference {mutation}"
            );
        }
    }

    #[test]
    fn wp9_protocol_safety_requires_symmetry_and_ground_at_zero() {
        let geometry = synthetic_geometry("drivaer_fastback");
        for (axis, offset) in [(1, -1.0), (1, 0.1), (2, -0.1), (2, 0.1)] {
            let (reference, mut domain, boundary) = inputs();
            // Keep each extent and the cross-section unchanged: only the
            // symmetry/ground plane placement is changed.
            domain.min_m[axis] += offset;
            domain.max_m[axis] += offset;
            assert!(
                protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst").is_none(),
                "accepted offset axis {axis}: {offset}"
            );
        }
    }

    #[test]
    fn wp9_protocol_safety_allows_default_or_larger_coherent_domains() {
        let geometry = synthetic_geometry("drivaer_fastback");
        let (reference, _, boundary) = inputs();
        for (blockage, up, down, aspect) in [(1.0, 3.0, 6.0, 5.0 / 3.0), (0.5, 4.0, 8.0, 2.0), (0.9, 3.0, 6.0, 1.0)] {
            let domain = crate::tunnel::size_domain(&reference, blockage, up, down, aspect, true, 0.0);
            assert_eq!(
                protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst"),
                Some(binding("drivaer_fastback")),
                "rejected coherent domain {blockage}%/{up}L/{down}L/aspect{aspect}"
            );
        }
    }

    #[test]
    fn wp9_protocol_safety_uses_reference_length_without_requiring_equal_body_span() {
        let geometry = synthetic_geometry("drivaer_fastback");
        for nose in [0.0, 2.0] {
            let (mut reference, _, boundary) = inputs();
            // Captured prepared geometry is slightly longer than the 4.6 m
            // normalization length; inlet/outlet multiples still use 4.6 m.
            reference.nose_x_m = nose;
            reference.tail_x_m = nose + 4.612826835;
            let domain = crate::tunnel::size_domain(&reference, 1.0, 3.0, 6.0, 5.0 / 3.0, true, 0.0);
            assert_eq!(
                protocol(&geometry, &spec(&reference, &domain, &boundary), "steady_sst"),
                Some(binding("drivaer_fastback"))
            );
        }
    }

    #[test]
    fn result_matches_only_the_same_candidate_normalization_and_recipe() {
        let original = result();
        let profile = binding("drivaer_fastback");
        assert!(matched(&profile, "drivaer_fastback", &original));
        assert!(!matched(&profile, "drivaer_estate", &original));
        for mode in 0..12 {
            let mut edited = original.clone();
            match mode {
                0 => edited.u_ms = 30.0,
                1 => edited.rho = 1.225,
                2 => edited.q_pa *= 2.0,
                3 => edited.a_front_m2 = 2.0,
                4 => edited.aref_m2 = 2.16,
                5 => edited.half_model = false,
                6 => edited.recipe = None,
                7 => edited.recipe.as_mut().unwrap().ground_mode = "stationary".into(),
                8 => edited.recipe.as_mut().unwrap().wheel_mode = "mrf".into(),
                9 => edited.recipe.as_mut().unwrap().yaw_deg = 2.0,
                10 => edited.recipe.as_mut().unwrap().turbulence = "kOmegaSSTDDES I=0.250% nut/nu=10.0".into(),
                _ => edited.placeholder = true,
            }
            assert!(!matched(&profile, "drivaer_fastback", &edited), "edited result {mode}");
        }
    }

    #[test]
    fn result_binding_rejects_unknown_profile_version_and_edited_physics() {
        for mode in 0..10 {
            let mut edited = binding("drivaer_fastback");
            match mode {
                0 => edited.version = 2,
                1 => edited.profile = "unknown".into(),
                2 => edited.geometry.version = 2,
                3 => edited.geometry.candidate = "unknown".into(),
                4 => edited.geometry.preparation_revision.clear(),
                5 => edited.geometry.tri_surface_fingerprint = "bad".into(),
                6 => edited.nu = 1.5e-5,
                7 => edited.reference_length_m = 1.84,
                8 => edited.wheel_omega_rad_s = 51.0,
                _ => edited.u_ms = 30.0,
            }
            assert!(!matched(&edited, "drivaer_fastback", &result()), "edited binding {mode}");
        }
    }

    #[test]
    fn binding_json_is_versioned_and_rejects_unknown_fields() {
        let original = binding("drivaer_fastback");
        let mut json = serde_json::to_value(&original).unwrap();
        assert_eq!(serde_json::from_value::<ReferenceBinding>(json.clone()).unwrap(), original);
        json["verified"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ReferenceBinding>(json).is_err());
    }
}
