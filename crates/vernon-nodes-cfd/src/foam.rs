//! OpenFOAM dictionary writers. Text is generated from the public keyword
//! descriptions only (see the spec), one function per file, deterministic.

use serde_json::Value;

use crate::types::{BcSet, Domain, PorousZone, RefGeom};
use crate::util::{num, vec3s};

/// Supported OpenFOAM flavours; file names and keywords differ between them.
pub const FLAVOURS: &[&str] = &[
    "foundation13",
    "foundation12",
    "foundation11",
    "foundation10",
    "foundation9",
    "opencfd2512",
    "opencfd2506",
    "opencfd2412",
    "opencfd2406",
    "opencfd2312",
    "opencfd2306",
    "opencfd2212",
    "opencfd2112",
    "opencfd2106",
    "opencfd2012",
    "opencfd2006",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    Foundation(u32),
    OpenCfd(u32),
}

impl Flavour {
    pub fn parse(s: &str) -> Option<Self> {
        if let Some(v) = s.strip_prefix("foundation") {
            return v.parse().ok().map(Flavour::Foundation);
        }
        s.strip_prefix("opencfd").and_then(|v| v.parse().ok()).map(Flavour::OpenCfd)
    }

    pub fn name(self) -> String {
        match self {
            Flavour::Foundation(v) => format!("foundation{v}"),
            Flavour::OpenCfd(v) => format!("opencfd{v}"),
        }
    }

    /// Kinematic viscosity file.
    pub fn physical_properties(self) -> &'static str {
        match self {
            Flavour::Foundation(v) if v >= 10 => "physicalProperties",
            _ => "transportProperties",
        }
    }

    pub fn momentum_transport(self) -> &'static str {
        match self {
            Flavour::Foundation(_) => "momentumTransport",
            Flavour::OpenCfd(_) => "turbulenceProperties",
        }
    }

    /// Path (relative to the case) of the porous-zone source file.
    pub fn fv_models_path(self) -> &'static str {
        match self {
            Flavour::Foundation(v) if v >= 10 => "constant/fvModels",
            _ => "constant/fvOptions",
        }
    }

    /// Modular solver front end (`foamRun`) instead of `simpleFoam`.
    pub fn uses_foam_run(self) -> bool {
        matches!(self, Flavour::Foundation(v) if v >= 12)
    }

    pub fn ras_key(self) -> &'static str {
        match self {
            Flavour::OpenCfd(v) if v < 2106 => "RASModel",
            _ => "model",
        }
    }

    /// Library list entry for a function-object library.
    pub fn libs(self, lib: &str) -> String {
        match self {
            Flavour::Foundation(_) => format!("(\"lib{lib}.so\")"),
            Flavour::OpenCfd(_) => format!("({lib})"),
        }
    }

    pub fn old_feature_extract(self) -> bool {
        matches!(self, Flavour::OpenCfd(2006))
    }

    /// Argv of the steady solver.
    pub fn solver_argv(self) -> Vec<String> {
        if self.uses_foam_run() {
            vec!["foamRun".into(), "-solver".into(), "incompressibleFluid".into()]
        } else {
            vec!["simpleFoam".into()]
        }
    }
}

pub fn header(class: &str, location: &str, object: &str) -> String {
    format!(
        "FoamFile\n{{\n    version     2.0;\n    format      ascii;\n    class       {class};\n    location    \"{location}\";\n    object      {object};\n}}\n\n"
    )
}

/// Minimal syntax check: FoamFile header first, balanced braces/parentheses,
/// every statement terminated. `#` directive lines are skipped.
#[cfg(test)]
pub fn check_syntax(text: &str) -> Result<(), String> {
    let mut toks: Vec<String> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '"' {
            let mut s = String::from("\"");
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                s.push(chars[i]);
                i += 1;
            }
            if i >= chars.len() {
                return Err("unterminated string".into());
            }
            i += 1;
            toks.push(s);
        } else if "{}();".contains(c) {
            toks.push(c.to_string());
            i += 1;
        } else {
            let mut s = String::new();
            while i < chars.len() && !chars[i].is_whitespace() && !"{}();\"".contains(chars[i]) {
                s.push(chars[i]);
                i += 1;
            }
            toks.push(s);
        }
    }
    if toks.first().map(String::as_str) != Some("FoamFile") {
        return Err("missing FoamFile header".into());
    }
    let mut stack: Vec<char> = Vec::new();
    let mut dangling = false;
    for t in &toks {
        let in_dict = |st: &Vec<char>| st.last().is_none_or(|&c| c == '{');
        match t.as_str() {
            "{" => {
                stack.push('{');
                dangling = false;
            }
            "}" => {
                if stack.pop() != Some('{') {
                    return Err("unbalanced '}'".into());
                }
                if dangling {
                    return Err("statement without ';' before '}'".into());
                }
                dangling = false;
            }
            "(" => stack.push('('),
            ")" => {
                if stack.pop() != Some('(') {
                    return Err("unbalanced ')'".into());
                }
                if in_dict(&stack) {
                    dangling = true;
                }
            }
            ";" => {
                if in_dict(&stack) {
                    dangling = false;
                }
            }
            _ => {
                if in_dict(&stack) {
                    dangling = true;
                }
            }
        }
    }
    if !stack.is_empty() {
        return Err("unclosed '{' or '('".into());
    }
    if dangling {
        return Err("last statement is missing ';'".into());
    }
    Ok(())
}

/// Background cells per axis: cubical cells of at most `dx0`.
pub fn base_cells(d: &Domain, dx0: f64) -> [usize; 3] {
    let s = d.size();
    [(s[0] / dx0).ceil() as usize, (s[1] / dx0).ceil() as usize, (s[2] / dx0).ceil() as usize]
}

pub fn block_mesh_dict(d: &Domain, dx0: f64) -> String {
    let (lo, hi) = (d.min_m, d.max_m);
    let n = base_cells(d, dx0);
    let mut s = header("dictionary", "system", "blockMeshDict");
    s += "convertToMeters 1;\n\nvertices\n(\n";
    for z in [lo[2], hi[2]] {
        // vertices 0-3 at the lower z, 4-7 at the upper z, counter-clockwise from (xmin, ymin)
        for (x, y) in [(lo[0], lo[1]), (hi[0], lo[1]), (hi[0], hi[1]), (lo[0], hi[1])] {
            s += &format!("    {}\n", vec3s([x, y, z]));
        }
    }
    s += ");\n\n";
    s += &format!("blocks\n(\n    hex (0 1 2 3 4 5 6 7) ({} {} {}) simpleGrading (1 1 1)\n);\n\nedges\n(\n);\n\n", n[0], n[1], n[2]);
    s += "boundary\n(\n";
    s += "    inlet\n    {\n        type patch;\n        faces ((0 4 7 3));\n    }\n";
    s += "    outlet\n    {\n        type patch;\n        faces ((1 2 6 5));\n    }\n";
    s += "    ground\n    {\n        type wall;\n        faces ((0 3 2 1));\n    }\n";
    s += "    top\n    {\n        type patch;\n        faces ((4 5 6 7));\n    }\n";
    if d.half_model {
        s += "    symmetry\n    {\n        type symmetryPlane;\n        faces ((0 1 5 4));\n    }\n";
        s += "    side\n    {\n        type patch;\n        faces ((3 7 6 2));\n    }\n";
    } else {
        s += "    side\n    {\n        type patch;\n        faces ((0 1 5 4) (3 7 6 2));\n    }\n";
    }
    s += ");\n\nmergePatchPairs\n(\n);\n";
    s
}

/// Everything the snappyHexMesh dictionary needs.
pub struct SnappySpec<'a> {
    pub plan: &'a Value,
    pub patches: Vec<String>,
    pub n_layers: u32,
    pub expansion: f64,
    pub first_layer_mode: bool,
    pub final_rel: f64,
    pub first_layer_m: f64,
    pub min_thickness_rel: f64,
    pub n_cells_between: u32,
    pub feature_angle: f64,
    pub max_global_cells: u64,
    pub max_local_cells: u64,
    pub snap_iter: u32,
    pub location: [f64; 3],
}

fn level_of(plan: &Value, patch: &str) -> (i64, i64) {
    let l = &plan["levels"];
    let get = |k: &str| l[k].as_i64().unwrap_or(5);
    if patch.starts_with("wheel") {
        (get("wheel"), get("wheel"))
    } else if patch.starts_with("kit") {
        (get("kit"), get("kit"))
    } else if patch.starts_with("underbody") {
        (get("underbody"), get("underbody"))
    } else {
        let b = &l["body"];
        (b[0].as_i64().unwrap_or(4), b[1].as_i64().unwrap_or(5))
    }
}

pub fn snappy_dict(s: &SnappySpec) -> String {
    let mut o = header("dictionary", "system", "snappyHexMeshDict");
    o += &format!(
        "castellatedMesh true;\nsnap            true;\naddLayers       {};\n\n",
        if s.n_layers > 0 { "true" } else { "false" }
    );
    let boxes = s.plan["boxes"].as_array().cloned().unwrap_or_default();
    o += "geometry\n{\n";
    for p in &s.patches {
        o += &format!("    {p}.stl\n    {{\n        type triSurfaceMesh;\n        name {p};\n    }}\n");
    }
    for b in &boxes {
        let (mn, mx) = (b["min"].clone(), b["max"].clone());
        let v = |x: &Value| vec3s([x[0].as_f64().unwrap_or(0.0), x[1].as_f64().unwrap_or(0.0), x[2].as_f64().unwrap_or(0.0)]);
        o += &format!(
            "    {}\n    {{\n        type searchableBox;\n        min {};\n        max {};\n    }}\n",
            b["name"].as_str().unwrap_or("box"),
            v(&mn),
            v(&mx)
        );
    }
    o += "}\n\ncastellatedMeshControls\n{\n";
    o += &format!(
        "    maxLocalCells {};\n    maxGlobalCells {};\n    minRefinementCells 10;\n    nCellsBetweenLevels {};\n    maxLoadUnbalance 0.1;\n",
        s.max_local_cells, s.max_global_cells, s.n_cells_between
    );
    o += "    features\n    (\n";
    for p in &s.patches {
        let (_, hi) = level_of(s.plan, p);
        o += &format!("        {{\n            file \"{p}.eMesh\";\n            level {hi};\n        }}\n");
    }
    o += "    );\n    refinementSurfaces\n    {\n";
    for p in &s.patches {
        let (lo, hi) = level_of(s.plan, p);
        o += &format!("        {p}\n        {{\n            level ({lo} {hi});\n            patchInfo\n            {{\n                type wall;\n            }}\n        }}\n");
    }
    o += &format!("    }}\n    resolveFeatureAngle {};\n    refinementRegions\n    {{\n", num(s.feature_angle));
    for b in &boxes {
        o += &format!(
            "        {}\n        {{\n            mode inside;\n            levels ((1e15 {}));\n        }}\n",
            b["name"].as_str().unwrap_or("box"),
            b["level"].as_i64().unwrap_or(3)
        );
    }
    if s.plan["underbody_mode"] == "distance" {
        let (_, ub) = level_of(s.plan, "underbody");
        o += &format!(
            "        underbody\n        {{\n            mode distance;\n            levels (({} {}));\n        }}\n",
            num(s.plan["underbody_dist_m"].as_f64().unwrap_or(0.1)),
            ub
        );
    }
    o += &format!(
        "    }}\n    locationInMesh {};\n    allowFreeStandingZoneFaces true;\n}}\n\n",
        vec3s(s.location)
    );
    o += &format!(
        "snapControls\n{{\n    nSmoothPatch 3;\n    tolerance 2.0;\n    nSolveIter {};\n    nRelaxIter 5;\n    nFeatureSnapIter 10;\n    implicitFeatureSnap false;\n    explicitFeatureSnap true;\n    multiRegionFeatureSnap false;\n}}\n\n",
        s.snap_iter
    );
    o += "addLayersControls\n{\n";
    o += &format!("    relativeSizes {};\n    expansionRatio {};\n", !s.first_layer_mode, num(s.expansion));
    if s.first_layer_mode {
        o += &format!("    firstLayerThickness {};\n    minThickness {};\n", num(s.first_layer_m), num(0.1 * s.first_layer_m));
    } else {
        o += &format!("    finalLayerThickness {};\n    minThickness {};\n", num(s.final_rel), num(s.min_thickness_rel));
    }
    o += "    layers\n    {\n";
    if s.n_layers > 0 {
        let mut roots: Vec<&str> = Vec::new();
        for p in &s.patches {
            let root = if p.starts_with("wheel") {
                "wheel_.*"
            } else if p.starts_with("kit") {
                "kit.*"
            } else if p.starts_with("underbody") {
                "underbody.*"
            } else {
                "body.*"
            };
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        for r in roots {
            o += &format!("        \"{r}\"\n        {{\n            nSurfaceLayers {};\n        }}\n", s.n_layers);
        }
    }
    o += "    }\n    nGrow 0;\n    featureAngle 130;\n    nRelaxIter 5;\n    nSmoothSurfaceNormals 1;\n    nSmoothNormals 3;\n    nSmoothThickness 10;\n    maxFaceThicknessRatio 0.5;\n    maxThicknessToMedialRatio 0.3;\n    minMedianAxisAngle 90;\n    nBufferCellsNoExtrude 0;\n    nLayerIter 50;\n    nRelaxedIter 20;\n}\n\n";
    o += "meshQualityControls\n{\n    #include \"meshQualityDict\"\n    relaxed\n    {\n        maxNonOrtho 75;\n    }\n}\n\nwriteFlags ();\n\nmergeTolerance 1e-6;\n";
    o
}

pub fn mesh_quality_dict(max_non_ortho: f64) -> String {
    let mut o = header("dictionary", "system", "meshQualityDict");
    o += &format!(
        "maxNonOrtho {};\nmaxBoundarySkewness 20;\nmaxInternalSkewness 4;\nmaxConcave 80;\nminVol 1e-13;\nminTetQuality -1e30;\nminArea -1;\nminTwist 0.05;\nminDeterminant 0.001;\nminFaceWeight 0.05;\nminVolRatio 0.01;\nminTriangleTwist -1;\nnSmoothScale 4;\nerrorReduction 0.75;\n",
        num(max_non_ortho)
    );
    o
}

pub fn surface_features_dict(f: Flavour, patches: &[String]) -> (String, &'static str) {
    if f.old_feature_extract() {
        let mut o = header("dictionary", "system", "surfaceFeatureExtractDict");
        for p in patches {
            o += &format!("{p}.stl\n{{\n    extractionMethod extractFromSurface;\n    extractFromSurfaceCoeffs\n    {{\n        includedAngle 150;\n    }}\n    writeObj no;\n}}\n");
        }
        (o, "system/surfaceFeatureExtractDict")
    } else {
        let mut o = header("dictionary", "system", "surfaceFeaturesDict");
        o += "surfaces (";
        o += &patches.iter().map(|p| format!("\"{p}.stl\"")).collect::<Vec<_>>().join(" ");
        o += ");\n\nincludedAngle 150;\n\nsubsetFeatures\n{\n    nonManifoldEdges no;\n    openEdges yes;\n}\n\nwriteObj no;\n";
        (o, "system/surfaceFeaturesDict")
    }
}

pub fn decompose_par_dict(np: u32) -> String {
    let mut o = header("dictionary", "system", "decomposeParDict");
    o += &format!("numberOfSubdomains {np};\n\nmethod scotch;\n");
    o
}

// ------------------------------------------------------------------ case files

/// Everything the solver-side files depend on.
pub struct CaseSpec<'a> {
    pub flavour: Flavour,
    pub domain: &'a Domain,
    pub rf: &'a RefGeom,
    pub bc: &'a BcSet,
    pub zones: &'a [PorousZone],
    pub u: f64,
    pub nu: f64,
    pub rho: f64,
    pub i_pct: f64,
    pub nut_ratio: f64,
    pub n_iter: u32,
    pub window: u32,
    pub residual: f64,
    pub write_interval: u32,
    pub nonortho_over_70: bool,
}

impl CaseSpec<'_> {
    pub fn k_inf(&self) -> f64 {
        1.5 * (self.i_pct / 100.0 * self.u).powi(2)
    }

    pub fn omega_inf(&self) -> f64 {
        self.k_inf() / (self.nut_ratio * self.nu)
    }

    pub fn inlet_velocity(&self) -> [f64; 3] {
        let y = self.domain.yaw_deg.to_radians();
        [self.u * y.cos(), self.u * y.sin(), 0.0]
    }

    /// Reference area for the force coefficients (spec 3.1).
    pub fn aref(&self) -> f64 {
        self.rf.aref_m2(self.domain.half_model)
    }

    pub fn cofr(&self) -> [f64; 3] {
        [self.rf.mid_wheelbase_x(), 0.0, 0.0]
    }

    fn wall_patches(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .rf
            .patches
            .iter()
            .filter(|p| !p.starts_with("wheel"))
            .map(|p| format!("{p}.*"))
            .collect();
        v.push("wheel_.*".into());
        v.dedup();
        v
    }

    fn force_patches(&self) -> String {
        let mut names: Vec<String> = self.rf.patches.iter().map(|p| format!("\"{p}.*\"")).collect();
        if names.is_empty() {
            names.push("\"body.*\"".into());
        }
        format!("({})", names.join(" "))
    }
}

fn field_header(class: &str, name: &str, dims: &str, internal: &str) -> String {
    let mut o = header(class, "0", name);
    o += &format!("dimensions      {dims};\n\ninternalField   {internal};\n\nboundaryField\n{{\n");
    o
}

fn patch_block(name: &str, body: &str) -> String {
    let q = if name.contains('*') || name.contains('.') { format!("\"{name}\"") } else { name.to_string() };
    format!("    {q}\n    {{\n{body}    }}\n")
}

pub fn u_field(c: &CaseSpec) -> String {
    let u = c.inlet_velocity();
    let uv = vec3s(u);
    let mut o = field_header("volVectorField", "U", "[0 1 -1 0 0 0 0]", &format!("uniform {uv}"), );
    o += &patch_block("inlet", &format!("        type fixedValue;\n        value uniform {uv};\n"));
    o += &patch_block("outlet", &format!("        type inletOutlet;\n        inletValue uniform (0 0 0);\n        value uniform {uv};\n"));
    o += &patch_block("top", "        type slip;\n");
    o += &patch_block("side", "        type slip;\n");
    let ground = match c.bc.ground_mode.as_str() {
        "moving" => format!("        type fixedValue;\n        value uniform {};\n", vec3s([c.bc.u_ms, 0.0, 0.0])),
        "slip" => "        type slip;\n".to_string(),
        _ => "        type noSlip;\n".to_string(),
    };
    o += &patch_block("ground", &ground);
    if c.domain.half_model {
        o += &patch_block("symmetry", "        type symmetryPlane;\n");
    }
    for w in c.wall_patches() {
        o += &patch_block(&w, "        type noSlip;\n");
    }
    if c.bc.wheel_mode == "rotating" {
        let wheels: Vec<_> = c.rf.patches.iter().filter(|p| p.starts_with("wheel")).collect();
        for wb in &c.bc.wheels {
            if !wheels.is_empty() && !wheels.contains(&&wb.name) {
                continue;
            }
            o += &patch_block(
                &format!("{}.*", wb.name),
                &format!(
                    "        type rotatingWallVelocity;\n        origin {};\n        axis {};\n        omega {};\n",
                    vec3s(wb.centre_m),
                    vec3s(c.bc.axis),
                    num(c.bc.omega_rad_s)
                ),
            );
        }
    }
    o += "}\n";
    o
}

pub fn p_field(c: &CaseSpec) -> String {
    let mut o = field_header("volScalarField", "p", "[0 2 -2 0 0 0 0]", "uniform 0");
    o += &patch_block("inlet", "        type zeroGradient;\n");
    o += &patch_block("outlet", "        type fixedValue;\n        value uniform 0;\n");
    o += &patch_block("top", "        type zeroGradient;\n");
    o += &patch_block("side", "        type zeroGradient;\n");
    o += &patch_block("ground", "        type zeroGradient;\n");
    if c.domain.half_model {
        o += &patch_block("symmetry", "        type symmetryPlane;\n");
    }
    for w in c.wall_patches() {
        o += &patch_block(&w, "        type zeroGradient;\n");
    }
    o += "}\n";
    o
}

fn turb_field(c: &CaseSpec, name: &str, dims: &str, inf: f64, wall: &str) -> String {
    let v = num(inf);
    let mut o = field_header("volScalarField", name, dims, &format!("uniform {v}"));
    o += &patch_block("inlet", &format!("        type fixedValue;\n        value uniform {v};\n"));
    o += &patch_block("outlet", &format!("        type inletOutlet;\n        inletValue uniform {v};\n        value uniform {v};\n"));
    o += &patch_block("top", "        type zeroGradient;\n");
    o += &patch_block("side", "        type zeroGradient;\n");
    let wallbc = format!("        type {wall};\n        value uniform {v};\n");
    o += &patch_block("ground", &wallbc);
    if c.domain.half_model {
        o += &patch_block("symmetry", "        type symmetryPlane;\n");
    }
    for w in c.wall_patches() {
        o += &patch_block(&w, &wallbc);
    }
    o += "}\n";
    o
}

pub fn k_field(c: &CaseSpec) -> String {
    turb_field(c, "k", "[0 2 -2 0 0 0 0]", c.k_inf(), "kqRWallFunction")
}

pub fn omega_field(c: &CaseSpec) -> String {
    turb_field(c, "omega", "[0 0 -1 0 0 0 0]", c.omega_inf(), "omegaWallFunction")
}

pub fn nut_field(c: &CaseSpec) -> String {
    let mut o = field_header("volScalarField", "nut", "[0 2 -1 0 0 0 0]", "uniform 0");
    for p in ["inlet", "outlet", "top", "side"] {
        o += &patch_block(p, "        type calculated;\n        value uniform 0;\n");
    }
    let wallbc = "        type nutUSpaldingWallFunction;\n        value uniform 0;\n";
    o += &patch_block("ground", wallbc);
    if c.domain.half_model {
        o += &patch_block("symmetry", "        type symmetryPlane;\n");
    }
    for w in c.wall_patches() {
        o += &patch_block(&w, wallbc);
    }
    o += "}\n";
    o
}

pub fn physical_properties(c: &CaseSpec) -> String {
    let mut o = header("dictionary", "constant", c.flavour.physical_properties());
    o += &format!("transportModel  Newtonian;\n\nnu              [0 2 -1 0 0 0 0] {};\n", num(c.nu));
    o
}

pub fn momentum_transport(c: &CaseSpec) -> String {
    let mut o = header("dictionary", "constant", c.flavour.momentum_transport());
    o += &format!(
        "simulationType  RAS;\n\nRAS\n{{\n    {}   kOmegaSST;\n    turbulence      on;\n    printCoeffs     on;\n}}\n",
        c.flavour.ras_key()
    );
    o
}

pub fn fv_schemes(c: &CaseSpec) -> String {
    let lap = if c.nonortho_over_70 { "limited corrected 0.5" } else { "corrected" };
    let mut o = header("dictionary", "system", "fvSchemes");
    o += "ddtSchemes\n{\n    default         steadyState;\n}\n\n";
    o += "gradSchemes\n{\n    default         Gauss linear;\n    grad(U)         cellLimited Gauss linear 1;\n}\n\n";
    o += "divSchemes\n{\n    default         none;\n    div(phi,U)      bounded Gauss linearUpwind grad(U);\n    div(phi,k)      bounded Gauss limitedLinear 1;\n    div(phi,omega)  bounded Gauss limitedLinear 1;\n    div((nuEff*dev2(T(grad(U))))) Gauss linear;\n}\n\n";
    o += &format!("laplacianSchemes\n{{\n    default         Gauss linear {lap};\n}}\n\n");
    o += &format!("interpolationSchemes\n{{\n    default         linear;\n}}\n\nsnGradSchemes\n{{\n    default         {lap};\n}}\n\nwallDist\n{{\n    method          meshWave;\n}}\n");
    o
}

pub fn fv_solution(c: &CaseSpec) -> String {
    let r = num(c.residual);
    let corr = u32::from(c.nonortho_over_70);
    let mut o = header("dictionary", "system", "fvSolution");
    o += "solvers\n{\n    p\n    {\n        solver          GAMG;\n        smoother        GaussSeidel;\n        tolerance       1e-10;\n        relTol          0.1;\n    }\n\n    \"(U|k|omega)\"\n    {\n        solver          smoothSolver;\n        smoother        symGaussSeidel;\n        tolerance       1e-10;\n        relTol          0.1;\n    }\n}\n\n";
    o += &format!("SIMPLE\n{{\n    consistent      yes;\n    nNonOrthogonalCorrectors {corr};\n    residualControl\n    {{\n        p               {r};\n        U               {r};\n        \"(k|omega)\"      {r};\n    }}\n}}\n\n");
    o += "relaxationFactors\n{\n    equations\n    {\n        U               0.9;\n        \".*\"            0.9;\n    }\n}\n";
    o
}

pub fn control_dict(c: &CaseSpec) -> String {
    let patches = c.force_patches();
    let porosity = if c.zones.is_empty() { "no" } else { "yes" };
    let (cofr, a) = (vec3s(c.cofr()), c.aref());
    let fl = |lib: &str| c.flavour.libs(lib);
    let mut o = header("dictionary", "system", "controlDict");
    if c.flavour.uses_foam_run() {
        o += "application     foamRun;\nsolver          incompressibleFluid;\n";
    } else {
        o += "application     simpleFoam;\n";
    }
    o += &format!(
        "startFrom       startTime;\nstartTime       0;\nstopAt          endTime;\nendTime         {};\ndeltaT          1;\nwriteControl    timeStep;\nwriteInterval   {};\npurgeWrite      2;\nwriteFormat     binary;\nwritePrecision  8;\ntimeFormat      general;\ntimePrecision   6;\nrunTimeModifiable true;\n\n",
        c.n_iter, c.write_interval
    );
    o += "functions\n{\n";
    o += &format!(
        "    forceCoeffs1\n    {{\n        type            forceCoeffs;\n        libs            {};\n        writeControl    timeStep;\n        writeInterval   1;\n        patches         {patches};\n        p               p;\n        U               U;\n        rho             rhoInf;\n        rhoInf          {};\n        pRef            0;\n        porosity        {porosity};\n        CofR            {cofr};\n        liftDir         (0 0 1);\n        dragDir         (1 0 0);\n        pitchAxis       (0 1 0);\n        magUInf         {};\n        lRef            {};\n        Aref            {};\n    }}\n\n",
        fl("forces"),
        num(c.rho),
        num(c.u),
        num(c.rf.length_m),
        num(a)
    );
    o += &format!(
        "    forces1\n    {{\n        type            forces;\n        libs            {};\n        writeControl    timeStep;\n        writeInterval   1;\n        patches         {patches};\n        p               p;\n        U               U;\n        rho             rhoInf;\n        rhoInf          {};\n        pRef            0;\n        porosity        {porosity};\n        CofR            {cofr};\n    }}\n\n",
        fl("forces"),
        num(c.rho)
    );
    o += &format!(
        "    yPlus1\n    {{\n        type            yPlus;\n        libs            {};\n        writeControl    writeTime;\n    }}\n\n",
        fl("fieldFunctionObjects")
    );
    o += &format!(
        "    wallShearStress1\n    {{\n        type            wallShearStress;\n        libs            {};\n        writeControl    writeTime;\n        patches         {patches};\n    }}\n\n",
        fl("fieldFunctionObjects")
    );
    o += &format!(
        "    solverInfo1\n    {{\n        type            solverInfo;\n        libs            {};\n        writeControl    timeStep;\n        writeInterval   1;\n        fields          (\".*\");\n    }}\n",
        fl("utilityFunctionObjects")
    );
    o += "}\n";
    o
}

/// Cell-zone selections (porous cores, rotating-frame cylinders).
pub fn topo_set_dict(c: &CaseSpec) -> Option<String> {
    let mut actions = String::new();
    for z in c.zones {
        if let Some([lo, hi]) = z.bbox_m {
            actions += &format!(
                "    {{\n        name    {n}Set;\n        type    cellSet;\n        action  new;\n        source  boxToCell;\n        box     {} {};\n    }}\n    {{\n        name    {n};\n        type    cellZoneSet;\n        action  new;\n        source  setToCellZone;\n        set     {n}Set;\n    }}\n",
                vec3s(lo),
                vec3s(hi),
                n = z.name
            );
        }
    }
    if c.bc.wheel_mode == "mrf" {
        let a = c.bc.axis;
        let h = 0.5 * c.bc.wheel_width_m;
        for w in &c.bc.wheels {
            let p = w.centre_m;
            let (p1, p2) = ([p[0] - a[0] * h, p[1] - a[1] * h, p[2] - a[2] * h], [p[0] + a[0] * h, p[1] + a[1] * h, p[2] + a[2] * h]);
            actions += &format!(
                "    {{\n        name    {n}Set;\n        type    cellSet;\n        action  new;\n        source  cylinderToCell;\n        p1      {};\n        p2      {};\n        radius  {};\n    }}\n    {{\n        name    {n};\n        type    cellZoneSet;\n        action  new;\n        source  setToCellZone;\n        set     {n}Set;\n    }}\n",
                vec3s(p1),
                vec3s(p2),
                num(c.bc.r_dyn_m * 1.05),
                n = w.name
            );
        }
    }
    if actions.is_empty() {
        return None;
    }
    let mut o = header("dictionary", "system", "topoSetDict");
    o += &format!("actions\n(\n{actions});\n");
    Some(o)
}

/// Porous source file (explicit Darcy-Forchheimer zones).
pub fn porous_source(c: &CaseSpec) -> Option<String> {
    let zones: Vec<&PorousZone> = c.zones.iter().filter(|z| z.bbox_m.is_some()).collect();
    if zones.is_empty() {
        return None;
    }
    let path = c.flavour.fv_models_path();
    let mut o = header("dictionary", path.split('/').next().unwrap_or("constant"), path.rsplit('/').next().unwrap_or("fvModels"));
    for z in zones {
        let e1 = z.flow_dir;
        // any vector not parallel to e1 gives the second axis
        let helper = if e1[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
        let e2 = {
            let d = e1[0] * helper[0] + e1[1] * helper[1] + e1[2] * helper[2];
            let v = [helper[0] - d * e1[0], helper[1] - d * e1[1], helper[2] - d * e1[2]];
            let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            [v[0] / n, v[1] / n, v[2] / n]
        };
        o += &format!(
            "{n}\n{{\n    type            explicitPorositySource;\n    explicitPorositySourceCoeffs\n    {{\n        selectionMode   cellZone;\n        cellZone        {n};\n        type            DarcyForchheimer;\n        DarcyForchheimerCoeffs\n        {{\n            d   {};\n            f   {};\n            coordinateSystem\n            {{\n                type    cartesian;\n                origin  (0 0 0);\n                coordinateRotation\n                {{\n                    type    axesRotation;\n                    e1      {};\n                    e2      {};\n                }}\n            }}\n        }}\n    }}\n}}\n\n",
            vec3s(z.d),
            vec3s(z.f),
            vec3s(e1),
            vec3s(e2),
            n = z.name
        );
    }
    Some(o)
}

/// Rotating-reference-frame zones for `wheel_mode = mrf`.
pub fn mrf_properties(c: &CaseSpec) -> Option<String> {
    if c.bc.wheel_mode != "mrf" || c.bc.wheels.is_empty() {
        return None;
    }
    let mut o = header("dictionary", "constant", "MRFProperties");
    for w in &c.bc.wheels {
        o += &format!(
            "MRF_{n}\n{{\n    cellZone        {n};\n    active          yes;\n    nonRotatingPatches ();\n    origin          {};\n    axis            {};\n    omega           {};\n}}\n\n",
            vec3s(w.centre_m),
            vec3s(c.bc.axis),
            num(c.bc.omega_rad_s),
            n = w.name
        );
    }
    Some(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syntax_checker_accepts_valid_and_rejects_broken() {
        let ok = format!("{}a 1;\nb {{ c 2; d (1 2 3); }}\ne ( f {{ g 1; }} );\n", header("dictionary", "system", "x"));
        check_syntax(&ok).unwrap();
        for bad in [
            format!("{}a 1\n", header("dictionary", "system", "x")),
            format!("{}b {{ c 2 }}\n", header("dictionary", "system", "x")),
            format!("{}b {{ c 2; \n", header("dictionary", "system", "x")),
            "a 1;\n".to_string(),
            format!("{}d (1 2 3;\n", header("dictionary", "system", "x")),
        ] {
            assert!(check_syntax(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn flavour_facts() {
        assert_eq!(Flavour::parse("foundation11"), Some(Flavour::Foundation(11)));
        assert_eq!(Flavour::parse("opencfd2406"), Some(Flavour::OpenCfd(2406)));
        assert_eq!(Flavour::parse("nonsense"), None);
        assert_eq!(Flavour::Foundation(9).physical_properties(), "transportProperties");
        assert_eq!(Flavour::Foundation(11).physical_properties(), "physicalProperties");
        assert_eq!(Flavour::OpenCfd(2406).momentum_transport(), "turbulenceProperties");
        assert_eq!(Flavour::OpenCfd(2006).ras_key(), "RASModel");
        assert_eq!(Flavour::Foundation(12).solver_argv()[0], "foamRun");
        assert_eq!(Flavour::Foundation(13).libs("forces"), "(\"libforces.so\")");
        assert_eq!(Flavour::OpenCfd(2312).libs("forces"), "(forces)");
        for f in FLAVOURS {
            assert_eq!(Flavour::parse(f).unwrap().name(), *f);
        }
    }
}
