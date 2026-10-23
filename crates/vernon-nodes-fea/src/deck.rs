//! Solver input deck writer (Abaqus-style keyword cards, units mm N tonne s MPa).
//!
//! Section order follows the spec: heading, nodes, elements, sets,
//! orientation, material, section, surfaces, constraints, step. Node id =
//! point index + 1 and element id = prim index + 1.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use glam::DVec3;
use vernon_core::Geometry;

use crate::material::{MaterialCard, ROOM_C};
use crate::mesh::{boundary_faces, element_size, element_volume};
use crate::types::{LoadSet, MountSet, OrientationCard};

/// Material card flavour for orthotropic materials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElasticType {
    EngineeringConstants,
    Ortho,
}

#[derive(Debug, Clone)]
pub struct DeckOptions {
    pub job: String,
    pub elastic: ElasticType,
    /// `auto` or a solver name written as `*STATIC,SOLVER=...`.
    pub solver: String,
    /// Write the face pressures as consistent nodal `*CLOAD` forces.
    pub pressure_as_cload: bool,
    /// Request integration-point stresses in the text output.
    pub ip_stress_print: bool,
    /// Uniform temperature (degC) used when the mesh has no `temp_c` attribute.
    pub temperature_c: f64,
    /// `(alpha per K, reference degC)`; `None` omits the expansion card.
    pub thermal_strain: Option<(f64, f64)>,
    pub allow_linear_tet: bool,
    pub unit_check: bool,
}

impl Default for DeckOptions {
    fn default() -> Self {
        Self {
            job: "vernon".into(),
            elastic: ElasticType::EngineeringConstants,
            solver: "auto".into(),
            pressure_as_cload: false,
            ip_stress_print: true,
            temperature_c: ROOM_C,
            thermal_strain: None,
            allow_linear_tet: false,
            unit_check: true,
        }
    }
}

pub struct DeckInput<'a> {
    pub mesh: &'a Geometry,
    pub card: &'a MaterialCard,
    pub orient: &'a OrientationCard,
    pub mounts: &'a MountSet,
    pub load: &'a LoadSet,
    pub opts: &'a DeckOptions,
}

pub struct DeckOutput {
    pub text: String,
    pub warnings: Vec<String>,
}

/// Plain number the way the deck wants it: no leading zero, a trailing dot on
/// integers, scientific notation for very small or large values.
pub fn fmt_val(v: f64) -> String {
    if v == 0.0 {
        return "0.".into();
    }
    let a = v.abs();
    if !(1e-3..1e7).contains(&a) {
        return format!("{v:E}");
    }
    if v.fract() == 0.0 {
        return format!("{v:.0}.");
    }
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0');
    strip_zero(s)
}

/// Poisson ratios keep at least two decimals (`.30`, `.35`, `.355`).
pub fn fmt_nu(v: f64) -> String {
    let s = fmt_val(v);
    match s.split_once('.') {
        Some((i, f)) if f.len() < 2 && !s.contains('E') => format!("{i}.{f:0<2}"),
        _ => s,
    }
}

fn strip_zero(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("0.") {
        format!(".{rest}")
    } else if let Some(rest) = s.strip_prefix("-0.") {
        format!("-.{rest}")
    } else {
        s.to_string()
    }
}

fn coord(v: f64) -> String {
    fmt_val(if v.abs() < 1e-12 { 0.0 } else { v })
}

fn push_ids(out: &mut String, ids: &[u32]) {
    for chunk in ids.chunks(16) {
        let line: Vec<String> = chunk.iter().map(|i| (i + 1).to_string()).collect();
        let _ = writeln!(out, "{}", line.join(","));
    }
}

/// Unit-consistency gate (G1). Returns `(errors, warnings)`.
pub fn unit_gate(mesh: &Geometry, card: &MaterialCard, load: &LoadSet) -> (Vec<String>, Vec<String>) {
    let mut err = Vec::new();
    let mut warn = Vec::new();
    for (n, e) in [("E1", card.e1), ("E2", card.e2), ("E3", card.e3)] {
        if !(50.0..=1.0e6).contains(&e) {
            err.push(format!("{n} = {e} MPa is outside 50..1e6 MPa: the deck unit system is mm, N, tonne, s, MPa (is the modulus in Pa or GPa?)"));
        }
    }
    let rho = card.density_t_mm3;
    if !(3e-10..=2e-8).contains(&rho) {
        err.push(format!("density {rho:E} tonne/mm3 is outside 0.3..20 g/cm3 (is it in kg/m3 or g/cm3?)"));
    } else if !(0.7e-9..=2.6e-9).contains(&rho) {
        warn.push(format!("density {rho:E} tonne/mm3 is unusual for a printed polymer"));
    }
    let size = mesh.bounds().size().max_element();
    if size < 1.0 {
        err.push(format!("the mesh spans only {size:.4} mm: lengths must be in millimetres (metres?)"));
    } else if size > 10_000.0 {
        warn.push(format!("the mesh spans {size:.0} mm: check the length unit"));
    }
    let pmax = load.pressures.iter().map(|p| p.p_mpa.abs()).fold(0.0, f64::max);
    if pmax > 100.0 {
        err.push(format!("a face pressure of {pmax} MPa is not plausible for an aero load (Pa instead of MPa?)"));
    } else if pmax > 5.0 {
        warn.push(format!("a face pressure of {pmax} MPa is very high for an aero load"));
    }
    if let Some(g) = load.gravity {
        if g.accel_mm_s2 > 0.0 && g.accel_mm_s2 < 100.0 {
            err.push(format!("acceleration {} mm/s2 looks like m/s2; the deck needs mm/s2 (9810 for 1 g)", g.accel_mm_s2));
        }
    }
    if !(1.0..=2000.0).contains(&card.xt) {
        warn.push(format!("in-plane strength {} MPa is outside 1..2000 MPa", card.xt));
    }
    (err, warn)
}

fn elastic_block(out: &mut String, card: &MaterialCard, opts: &DeckOptions) {
    let temps: Vec<f64> = match &card.retention {
        Some(t) if !t.points.is_empty() => t.temps(),
        _ => vec![ROOM_C],
    };
    let iso = card.mode == "isotropic_z";
    match (iso, opts.elastic) {
        (true, _) => out.push_str("*ELASTIC\n"),
        (false, ElasticType::EngineeringConstants) => out.push_str("*ELASTIC,TYPE=ENGINEERING CONSTANTS\n"),
        (false, ElasticType::Ortho) => out.push_str("*ELASTIC,TYPE=ORTHO\n"),
    }
    for t in temps {
        let c = card.stiffness_scaled(card.retention_at(t));
        if iso {
            let _ = writeln!(out, "{},{},{}", fmt_val(c.e1), fmt_nu(c.nu12), fmt_val(t));
            continue;
        }
        match opts.elastic {
            ElasticType::EngineeringConstants => {
                let _ = writeln!(
                    out,
                    "{},{},{},{},{},{},{},{},",
                    fmt_val(c.e1),
                    fmt_val(c.e2),
                    fmt_val(c.e3),
                    fmt_nu(c.nu12),
                    fmt_nu(c.nu13),
                    fmt_nu(c.nu23),
                    fmt_val(c.g12),
                    fmt_val(c.g13)
                );
                let _ = writeln!(out, "{},{}", fmt_val(c.g23), fmt_val(t));
            }
            ElasticType::Ortho => {
                let d = c.ortho_d();
                let first: Vec<String> = d[..8].iter().map(|v| fmt_val(*v)).collect();
                let _ = writeln!(out, "{},", first.join(","));
                let _ = writeln!(out, "{},{}", fmt_val(d[8]), fmt_val(t));
            }
        }
    }
}

/// Write the deck. Fails on bad meshes or a failed unit gate.
pub fn write_deck(inp: &DeckInput) -> Result<DeckOutput, String> {
    let mesh = inp.mesh;
    let opts = inp.opts;
    let nodes_per_elem = element_size(mesh)?;
    if nodes_per_elem == 4 && !opts.allow_linear_tet {
        return Err("linear tetrahedra (C3D4) are too stiff for structural use: convert to quadratic or enable 'allow linear tet'".into());
    }
    let etype = if nodes_per_elem == 10 { "C3D10" } else { "C3D4" };
    if let Some(e) = (0..mesh.prims.len()).find(|&e| element_volume(mesh, e) <= 0.0) {
        return Err(format!("element {} has a non-positive volume; run the solid mesh step again", e + 1));
    }
    let mut warnings = Vec::new();
    if opts.unit_check {
        let (errs, warns) = unit_gate(mesh, inp.card, inp.load);
        if !errs.is_empty() {
            return Err(format!("unit gate failed: {}", errs.join("; ")));
        }
        warnings.extend(warns);
    }
    inp.card.check_stability().map_err(|e| format!("material constants rejected: {e}"))?;

    let mut s = String::with_capacity(64 * 1024);
    let iso = inp.card.mode == "isotropic_z";
    s.push_str("*HEADING\n");
    let _ = writeln!(s, "{}, units mm N tonne s MPa", opts.job);

    // nodes and elements
    s.push_str("*NODE,NSET=NALL\n");
    for (i, p) in mesh.points.iter().enumerate() {
        let _ = writeln!(s, "{},{},{},{}", i + 1, coord(p.x), coord(p.y), coord(p.z));
    }
    let _ = writeln!(s, "*ELEMENT,TYPE={etype},ELSET=EALL");
    for (e, prim) in mesh.prims.iter().enumerate() {
        let ids: Vec<String> = prim.vertices.iter().map(|v| (v + 1).to_string()).collect();
        let _ = writeln!(s, "{},{}", e + 1, ids.join(","));
    }

    // rigid washers need a reference node placed off the surface
    let mut ref_nodes: BTreeMap<usize, (usize, DVec3)> = BTreeMap::new();
    let mut next_node = mesh.points.len();
    for (k, site) in inp.mounts.sites.iter().enumerate() {
        if site.model == "rigid_washer" {
            let c = site.nodes.iter().map(|&n| mesh.points[n as usize]).sum::<DVec3>() / site.nodes.len().max(1) as f64;
            let off = DVec3::from(site.axis).normalize_or_zero() * site.r_w_mm.max(1.0);
            next_node += 1;
            ref_nodes.insert(k, (next_node, c + off));
        }
    }
    if !ref_nodes.is_empty() {
        s.push_str("*NODE\n");
        for (id, p) in ref_nodes.values() {
            let _ = writeln!(s, "{},{},{},{}", id, coord(p.x), coord(p.y), coord(p.z));
        }
    }

    // mount node sets
    let mut mount_sets = Vec::new();
    for (k, site) in inp.mounts.sites.iter().enumerate() {
        if site.nodes.is_empty() {
            return Err(format!("mount '{}' has no nodes", site.name));
        }
        if let Some(bad) = site.nodes.iter().find(|&&n| n as usize >= mesh.points.len()) {
            return Err(format!("mount '{}' references node {} outside the mesh", site.name, bad + 1));
        }
        let _ = writeln!(s, "*NSET,NSET=MOUNT{}", k + 1);
        push_ids(&mut s, &site.nodes);
        mount_sets.push(format!("MOUNT{}", k + 1));
    }

    // surfaces for the pressure classes
    let mut classes: BTreeMap<i64, Vec<(u32, u8)>> = BTreeMap::new();
    let mut class_p: BTreeMap<i64, f64> = BTreeMap::new();
    for fp in &inp.load.pressures {
        let key = (fp.p_mpa * 1e12).round() as i64;
        classes.entry(key).or_default().push((fp.elem, fp.face));
        class_p.insert(key, fp.p_mpa);
    }
    let use_dsload = !opts.pressure_as_cload && !classes.is_empty() && classes.len() <= 64;
    if use_dsload {
        for (ci, faces) in classes.values().enumerate() {
            let _ = writeln!(s, "*SURFACE,NAME=SP{},TYPE=ELEMENT", ci + 1);
            for (e, f) in faces {
                let _ = writeln!(s, "{},S{}", e + 1, f);
            }
        }
    }

    // orientation, material, section
    if !iso {
        let (a, b) = inp.orient.ab();
        let _ = writeln!(s, "*ORIENTATION,NAME={}", inp.orient.name);
        let v: Vec<String> = a.iter().chain(b.iter()).map(|x| coord(*x)).collect();
        let _ = writeln!(s, "{}", v.join(","));
    }
    s.push_str("*MATERIAL,NAME=PRINT\n");
    elastic_block(&mut s, inp.card, opts);
    s.push_str("*DENSITY\n");
    let _ = writeln!(s, "{},", fmt_val(inp.card.density_t_mm3));
    if let Some((alpha, tref)) = opts.thermal_strain {
        s.push_str("*EXPANSION\n");
        let _ = writeln!(s, "{},", fmt_val(alpha));
        let _ = tref;
    }
    if iso {
        s.push_str("*SOLID SECTION,ELSET=EALL,MATERIAL=PRINT\n");
    } else {
        let _ = writeln!(s, "*SOLID SECTION,ELSET=EALL,MATERIAL=PRINT,ORIENTATION={}", inp.orient.name);
    }
    if let Some((_, tref)) = opts.thermal_strain {
        s.push_str("*INITIAL CONDITIONS,TYPE=TEMPERATURE\nNALL,");
        let _ = writeln!(s, "{}", fmt_val(tref));
    }

    // constraints
    let mut spring_elem = mesh.prims.len();
    for (k, site) in inp.mounts.sites.iter().enumerate() {
        match site.model.as_str() {
            "rigid_washer" => {
                let (ref_id, _) = ref_nodes[&k];
                let _ = writeln!(s, "*SURFACE,NAME=SW{},TYPE=NODE\nMOUNT{}", k + 1, k + 1);
                let _ = writeln!(s, "*COUPLING,REF NODE={ref_id},SURFACE=SW{},CONSTRAINT NAME=CW{}", k + 1, k + 1);
                s.push_str("*KINEMATIC\n1,3\n");
                let _ = writeln!(s, "*BOUNDARY\n{ref_id},1,6");
            }
            "spring_pad" => {
                let area = std::f64::consts::PI * site.r_w_mm * site.r_w_mm;
                let k_node = site.pad_e_mpa * area / site.pad_t_mm.max(1e-6) / site.nodes.len() as f64;
                for dof in 1..=3 {
                    let _ = writeln!(s, "*ELEMENT,TYPE=SPRING1,ELSET=SPR{}_{}", k + 1, dof);
                    for &n in &site.nodes {
                        spring_elem += 1;
                        let _ = writeln!(s, "{},{}", spring_elem, n + 1);
                    }
                    let _ = writeln!(s, "*SPRING,ELSET=SPR{}_{}\n{}\n{}", k + 1, dof, dof, fmt_val(k_node));
                }
            }
            _ => {
                let _ = writeln!(s, "*BOUNDARY\nMOUNT{},1,3", k + 1);
            }
        }
    }

    // step
    s.push_str("*STEP\n");
    if opts.solver == "auto" {
        s.push_str("*STATIC\n");
    } else {
        let _ = writeln!(s, "*STATIC,SOLVER={}", opts.solver.to_uppercase());
    }
    if mesh.point_attribs.floats("temp_c").is_ok() {
        let t = mesh.point_attribs.floats("temp_c").map_err(|e| e.to_string())?;
        s.push_str("*TEMPERATURE\n");
        for (i, v) in t.iter().enumerate() {
            let _ = writeln!(s, "{},{}", i + 1, fmt_val(*v));
        }
    } else {
        let _ = writeln!(s, "*TEMPERATURE\nNALL,{}", fmt_val(opts.temperature_c));
    }
    let l = inp.load;
    let pressure_rows = !l.pressures.is_empty() && !opts.pressure_as_cload && !use_dsload;
    if pressure_rows || l.gravity.is_some() {
        s.push_str("*DLOAD\n");
        if pressure_rows {
            for fp in &l.pressures {
                let _ = writeln!(s, "{},P{},{}", fp.elem + 1, fp.face, fmt_val(fp.p_mpa));
            }
        }
        if let Some(g) = l.gravity {
            let _ = writeln!(s, "EALL,GRAV,{},{},{},{}", fmt_val(g.accel_mm_s2), coord(g.dir[0]), coord(g.dir[1]), coord(g.dir[2]));
        }
    }
    if use_dsload {
        s.push_str("*DSLOAD\n");
        for (ci, key) in classes.keys().enumerate() {
            let _ = writeln!(s, "SP{},P,{}", ci + 1, fmt_val(class_p[key]));
        }
    }
    let mut forces = l.forces.clone();
    if opts.pressure_as_cload {
        forces.extend(crate::loads::pressure_nodal_forces(mesh, &l.pressures));
    }
    let mut cload_rows = String::new();
    for f in &forces {
        for (d, v) in f.f_n.iter().enumerate() {
            if *v != 0.0 {
                let _ = writeln!(cload_rows, "{},{},{}", f.node + 1, d + 1, fmt_val(*v));
            }
        }
    }
    if !cload_rows.is_empty() {
        s.push_str("*CLOAD\n");
        s.push_str(&cload_rows);
    }

    // output requests
    s.push_str("*NODE FILE\nU,RF\n*EL FILE\nS,E,NOE\n");
    for (k, site) in inp.mounts.sites.iter().enumerate() {
        if site.model == "rigid_washer" {
            let _ = writeln!(s, "*SECTION PRINT,SURFACE=SW{},NAME=SPR{}\nSOF", k + 1, k + 1);
        } else {
            let _ = writeln!(s, "*NODE PRINT,NSET=MOUNT{},TOTALS=ONLY\nRF", k + 1);
        }
    }
    if opts.ip_stress_print {
        s.push_str("*EL PRINT,ELSET=EALL\nS\n");
    }
    s.push_str("*EL PRINT,ELSET=EALL,TOTALS=ONLY\nEVOL\n*END STEP\n");

    if boundary_faces(mesh).is_empty() {
        warnings.push("the mesh has no boundary faces".into());
    }
    Ok(DeckOutput { text: s, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{box_tets, tet4_to_tet10};
    use crate::types::{FacePressure, Gravity, MountSite, NodalForce};

    fn mesh10() -> Geometry {
        tet4_to_tet10(&box_tets(DVec3::new(20.0, 10.0, 6.0), [2, 1, 1])).unwrap()
    }

    fn site(model: &str, mesh: &Geometry) -> MountSite {
        let nodes: Vec<u32> = mesh.points.iter().enumerate().filter(|(_, p)| p.x < -9.9).map(|(i, _)| i as u32).collect();
        MountSite {
            name: "m1".into(),
            center: [-10.0, 0.0, 0.0],
            axis: [-1.0, 0.0, 0.0],
            nodes,
            model: model.into(),
            r_w_mm: 4.0,
            avg_radius_mm: 5.0,
            bolt_d_mm: 5.0,
            pad_e_mpa: 5.0,
            pad_t_mm: 2.0,
        }
    }

    fn deck(mesh: &Geometry, mounts: &MountSet, load: &LoadSet, opts: &DeckOptions) -> Result<DeckOutput, String> {
        let card = MaterialCard::trans_iso("PRINT", 2500.0, 2000.0, 0.35, 0.30, 800.0);
        write_deck(&DeckInput { mesh, card: &card, orient: &OrientationCard::identity(), mounts, load, opts })
    }

    #[test]
    fn number_formats() {
        assert_eq!(fmt_val(2500.0), "2500.");
        assert_eq!(fmt_val(0.35), ".35");
        assert_eq!(fmt_val(925.926), "925.926");
        assert_eq!(fmt_val(23.0), "23.");
        assert_eq!(fmt_val(-0.5), "-.5");
        assert_eq!(fmt_val(1.24e-9), "1.24E-9");
        assert_eq!(fmt_nu(0.3), ".30");
        assert_eq!(fmt_nu(0.35), ".35");
        assert_eq!(fmt_nu(0.355), ".355");
    }

    #[test]
    fn g2_deck_lines_for_the_transversely_isotropic_case() {
        let mesh = mesh10();
        let mut card = MaterialCard::trans_iso("PRINT", 2500.0, 2000.0, 0.35, 0.30, 800.0);
        card.g12 = 925.926;
        let o = write_deck(&DeckInput {
            mesh: &mesh,
            card: &card,
            orient: &OrientationCard::identity(),
            mounts: &MountSet { sites: vec![site("fixed_patch", &mesh)] },
            load: &LoadSet::default(),
            opts: &DeckOptions::default(),
        })
        .unwrap();
        assert!(o.text.contains("*ELASTIC,TYPE=ENGINEERING CONSTANTS\n2500.,2500.,2000.,.35,.30,.30,925.926,800.,\n800.,23.\n"), "{}", o.text);
        assert!(o.text.contains("*DENSITY\n1.24E-9,\n"));
        assert!(o.text.contains("*ORIENTATION,NAME=OR1\n1.,0.,0.,0.,1.,0.\n"));
        assert!(o.text.contains("*SOLID SECTION,ELSET=EALL,MATERIAL=PRINT,ORIENTATION=OR1\n"));
    }

    #[test]
    fn sections_come_in_the_documented_order_and_mounts_are_fixed() {
        let mesh = mesh10();
        let mounts = MountSet { sites: vec![site("fixed_patch", &mesh)] };
        let load = LoadSet {
            pressures: vec![FacePressure { elem: 0, face: 1, p_mpa: 1.7e-3 }, FacePressure { elem: 1, face: 2, p_mpa: 1.7e-3 }],
            gravity: Some(Gravity { accel_mm_s2: 9810.0, dir: [0.0, 0.0, -1.0] }),
            forces: vec![NodalForce { node: 3, f_n: [0.0, 0.0, -2.0] }],
            ..Default::default()
        };
        let t = deck(&mesh, &mounts, &load, &DeckOptions::default()).unwrap().text;
        let order = ["*HEADING", "*NODE,NSET=NALL", "*ELEMENT,TYPE=C3D10,ELSET=EALL", "*NSET,NSET=MOUNT1", "*SURFACE,NAME=SP1", "*ORIENTATION", "*MATERIAL", "*SOLID SECTION", "*BOUNDARY\nMOUNT1,1,3", "*STEP", "*STATIC", "*TEMPERATURE", "*DLOAD", "*DSLOAD", "*CLOAD", "*NODE FILE\nU,RF", "*EL FILE\nS,E,NOE", "*NODE PRINT,NSET=MOUNT1,TOTALS=ONLY\nRF", "*END STEP"];
        let mut at = 0;
        for key in order {
            let i = t[at..].find(key).unwrap_or_else(|| panic!("{key} missing or out of order in\n{t}"));
            at += i;
        }
        assert!(t.contains("SP1,P,.0017\n") || t.contains("SP1,P,.0017"), "{t}");
        assert!(t.contains("EALL,GRAV,9810.,0.,0.,-1.\n"));
        assert!(t.contains("4,3,-2.\n"));
        // node and element counts
        let nn = t.lines().skip_while(|l| !l.starts_with("*NODE,NSET")).skip(1).take_while(|l| !l.starts_with('*')).count();
        assert_eq!(nn, mesh.points.len());
    }

    #[test]
    fn pressure_as_cload_writes_consistent_forces() {
        let mesh = mesh10();
        let load = LoadSet { pressures: vec![FacePressure { elem: 0, face: 1, p_mpa: 0.01 }], ..Default::default() };
        let mounts = MountSet { sites: vec![site("fixed_patch", &mesh)] };
        let t = deck(&mesh, &mounts, &load, &DeckOptions { pressure_as_cload: true, ..Default::default() }).unwrap().text;
        assert!(!t.contains("*DSLOAD") && !t.contains("P1,") && t.contains("*CLOAD"));
    }

    #[test]
    fn rigid_washer_and_spring_pad_cards() {
        let mesh = mesh10();
        let t = deck(&mesh, &MountSet { sites: vec![site("rigid_washer", &mesh)] }, &LoadSet::default(), &DeckOptions::default()).unwrap().text;
        let n = mesh.points.len() + 1;
        assert!(t.contains(&format!("*COUPLING,REF NODE={n},SURFACE=SW1,CONSTRAINT NAME=CW1\n*KINEMATIC\n1,3\n*BOUNDARY\n{n},1,6")), "{t}");
        assert!(t.contains("*SECTION PRINT,SURFACE=SW1,NAME=SPR1\nSOF"));
        let t = deck(&mesh, &MountSet { sites: vec![site("spring_pad", &mesh)] }, &LoadSet::default(), &DeckOptions::default()).unwrap().text;
        assert!(t.contains("*ELEMENT,TYPE=SPRING1,ELSET=SPR1_3") && t.contains("*SPRING,ELSET=SPR1_3"));
    }

    #[test]
    fn ortho_card_and_temperature_rows() {
        let mesh = mesh10();
        let mut card = MaterialCard::trans_iso("PRINT", 2500.0, 2000.0, 0.35, 0.30, 800.0);
        card.retention = Some(crate::material::RetentionTable { points: vec![(23.0, 1.0), (60.0, 0.5)], source: "t".into(), uncalibrated: false });
        let opts = DeckOptions { elastic: ElasticType::Ortho, ..Default::default() };
        let mounts = MountSet { sites: vec![site("fixed_patch", &mesh)] };
        let o = write_deck(&DeckInput { mesh: &mesh, card: &card, orient: &OrientationCard::identity(), mounts: &mounts, load: &LoadSet::default(), opts: &opts }).unwrap();
        assert!(o.text.contains("*ELASTIC,TYPE=ORTHO\n3396.28"), "{}", o.text);
        assert!(o.text.matches(",23.\n").count() + o.text.matches(",60.\n").count() >= 2);
        // stiffness at 60 degC is half
        let d_half = card.stiffness_scaled(0.5).ortho_d()[0];
        assert!(o.text.contains(&fmt_val(d_half)));
    }

    #[test]
    fn isotropic_z_mode_writes_a_plain_elastic_card() {
        let mesh = mesh10();
        let mut card = MaterialCard::trans_iso("PRINT", 2000.0, 2000.0, 0.3, 0.3, 2000.0 / 2.6);
        card.mode = "isotropic_z".into();
        let o = write_deck(&DeckInput { mesh: &mesh, card: &card, orient: &OrientationCard::identity(), mounts: &MountSet { sites: vec![site("fixed_patch", &mesh)] }, load: &LoadSet::default(), opts: &DeckOptions::default() }).unwrap();
        assert!(o.text.contains("*ELASTIC\n2000.,.30,23.\n") && !o.text.contains("*ORIENTATION"));
    }

    #[test]
    fn unit_gate_rejects_wrong_units_and_bad_meshes() {
        let mesh = mesh10();
        let mut card = MaterialCard::trans_iso("PRINT", 2.5, 2.0, 0.35, 0.3, 0.8);
        let (e, _) = unit_gate(&mesh, &card, &LoadSet::default());
        assert!(!e.is_empty());
        card = MaterialCard::trans_iso("PRINT", 2500.0, 2000.0, 0.35, 0.3, 800.0);
        card.density_t_mm3 = 1240.0;
        assert!(!unit_gate(&mesh, &card, &LoadSet::default()).0.is_empty());
        card.density_t_mm3 = 1.24e-9;
        let load = LoadSet { gravity: Some(Gravity { accel_mm_s2: 9.81, dir: [0.0, 0.0, -1.0] }), ..Default::default() };
        assert!(!unit_gate(&mesh, &card, &load).0.is_empty());
        assert!(unit_gate(&mesh, &card, &LoadSet::default()).0.is_empty());
        // linear tets are refused by default, negative volumes too
        let lin = box_tets(DVec3::splat(10.0), [1, 1, 1]);
        assert!(deck(&lin, &MountSet::default(), &LoadSet::default(), &DeckOptions::default()).is_err());
        let mut flipped = mesh.clone();
        flipped.prims[0].vertices.swap(1, 2);
        assert!(deck(&flipped, &MountSet { sites: vec![site("fixed_patch", &mesh)] }, &LoadSet::default(), &DeckOptions::default()).is_err());
    }
}
