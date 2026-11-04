//! Grid-convergence study, baseline-vs-kit comparison and benchmark presets.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use vernon_core::{Metric, Report, Severity};
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::coeffs::{ForceResult, force_in};
use crate::util::{FnNode, file_in, fin, json_in};

// ------------------------------------------------------------------ grid convergence index

/// Grid-convergence procedure of Celik et al. (2008) on three grids.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Gci {
    /// Fine, medium, coarse.
    pub values: [f64; 3],
    pub r21: f64,
    pub r32: f64,
    /// `phi2 - phi1` and `phi3 - phi2`.
    pub e21: f64,
    pub e32: f64,
    /// Sign change of the successive differences: no extrapolation is made.
    pub oscillatory: bool,
    /// Fine and medium are identical (nothing left to extrapolate).
    pub identical: bool,
    /// The change does not shrink with refinement: `|e21| >= |e32|`.
    #[serde(default)]
    pub diverging: bool,
    /// Observed order of convergence.
    pub p: Option<f64>,
    pub phi_ext: Option<f64>,
    /// `|phi1 - phi2| / |phi1|` in percent.
    pub ea_pct: f64,
    pub gci_fine_pct: Option<f64>,
    /// Relative difference of the extrapolated and the fine value, percent.
    pub e_ext_pct: Option<f64>,
    /// `(max - min) / |phi1|` of the three values, percent.
    pub spread_pct: f64,
}

/// Observed order by the fixed-point iteration of the procedure. `None` when it
/// does not settle on a positive finite order.
fn observed_order(ratio: f64, r21: f64, r32: f64) -> Option<f64> {
    let ln_ratio = ratio.ln();
    let mut p = ln_ratio.abs() / r21.ln();
    for _ in 0..2000 {
        let q = ((r21.powf(p) - 1.0) / (r32.powf(p) - 1.0)).ln();
        let next = (ln_ratio + q).abs() / r21.ln();
        if !next.is_finite() {
            return None;
        }
        if (next - p).abs() < 1e-12 {
            return (next > 1e-9).then_some(next);
        }
        p = next;
    }
    None
}

/// Three-grid GCI for `phi = [fine, medium, coarse]` and the linear
/// refinement ratios `r21` (medium/fine) and `r32` (coarse/medium).
pub fn grid_convergence(phi: [f64; 3], r21: f64, r32: f64) -> Result<Gci, String> {
    if phi.iter().any(|v| !v.is_finite()) {
        return Err("grid values must be finite".into());
    }
    if !(r21 > 1.0 && r32 > 1.0 && r21.is_finite() && r32.is_finite()) {
        return Err("refinement ratios must be above 1".into());
    }
    let [p1, p2, p3] = phi;
    if p1 == 0.0 {
        return Err("the fine-grid value is zero; relative errors are undefined".into());
    }
    let (e21, e32) = (p2 - p1, p3 - p2);
    let ea = ((p1 - p2) / p1).abs();
    let hi = phi.iter().copied().fold(f64::MIN, f64::max);
    let lo = phi.iter().copied().fold(f64::MAX, f64::min);
    let mut g = Gci {
        values: phi,
        r21,
        r32,
        e21,
        e32,
        ea_pct: ea * 100.0,
        spread_pct: (hi - lo) / p1.abs() * 100.0,
        ..Gci::default()
    };
    let tiny = 1e-12 * p1.abs();
    if e21.abs() <= tiny {
        g.identical = true;
        g.gci_fine_pct = Some(0.0);
        g.phi_ext = Some(p1);
        return Ok(g);
    }
    if e32.abs() <= tiny || e32 / e21 < 0.0 {
        g.oscillatory = true;
        return Ok(g);
    }
    g.diverging = e32.abs() <= e21.abs();
    if let Some(p) = observed_order(e32 / e21, r21, r32) {
        let r21p = r21.powf(p);
        let ext = (r21p * p1 - p2) / (r21p - 1.0);
        g.p = Some(p);
        g.phi_ext = Some(ext);
        g.gci_fine_pct = Some(1.25 * ea / (r21p - 1.0) * 100.0);
        g.e_ext_pct = (ext != 0.0).then(|| ((ext - p1) / ext).abs() * 100.0);
    }
    Ok(g)
}

/// Relative difference of the two finest grids in percent (the toolchain's 1-2% rule).
pub fn last_two_diff_pct(fine: f64, medium: f64) -> f64 {
    ((fine - medium) / fine).abs() * 100.0
}

/// Largest two-finest-grids change the acceptance rule may be given, percent. The project
/// note asks for the last two grids within 1 to 2 %; a looser limit would accept a mesh that
/// is not yet independent.
pub const MAX_ACCEPT_PCT: f64 = 2.0;

/// Observed orders a GCI can be believed for. Outside this the grids are not in
/// the asymptotic range, and the index they give is too small to mean anything.
const ORDER_RANGE: std::ops::RangeInclusive<f64> = 0.5..=4.0;

/// Acceptance: the two finest grids within `accept_pct`, the fine-grid GCI
/// within `gci_max_pct` and a believable observed order, with the change
/// shrinking as the grid is refined. Oscillatory or identical grids have no
/// order to judge, so the spread of all three values must stay within twice the
/// acceptance level instead.
pub fn mesh_accepted(g: &Gci, accept_pct: f64, gci_max_pct: f64) -> bool {
    let two = last_two_diff_pct(g.values[0], g.values[1]) <= accept_pct;
    if g.oscillatory || g.identical {
        return two && g.spread_pct <= 2.0 * accept_pct;
    }
    let order_ok = !g.diverging && g.p.is_some_and(|p| ORDER_RANGE.contains(&p));
    two && order_ok && g.gci_fine_pct.is_some_and(|v| v <= gci_max_pct)
}

fn study_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.mesh_study", "Mesh Convergence", Stage::Cfd)
        .input(PortSpec::optional("fine", DataKind::Json))
        .input(PortSpec::optional("medium", DataKind::Json))
        .input(PortSpec::optional("coarse", DataKind::Json))
        .output(PortSpec::new("study", DataKind::Json))
        .output(PortSpec::new("accepted", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::choice("quantity", "Quantity", "cd", &["cd", "cda", "cl", "cla"]).help("Quantity whose convergence over the three grids is judged: drag or lift coefficient, or the coefficient times area."))
        .param(ParamSpec::float("r21", "Refinement ratio medium/fine", 1.3, 1.25, 2.0).help("Linear cell-size ratio; cell count grows by r cubed"))
        .param(ParamSpec::float("r32", "Refinement ratio coarse/medium", 1.3, 1.25, 2.0).help("Linear cell-size ratio between the coarse and the medium grid."))
        .param(ParamSpec::float("accept_pct", "Two-finest-grids change", 2.0, 0.5, MAX_ACCEPT_PCT).unit("%").help("Accept when the last two grids differ by less than this. Capped at 2 %: the project note asks for the last two grids within 1 to 2 %."))
        .param(ParamSpec::float("gci_max_pct", "Fine-grid GCI limit", 3.0, 0.5, 10.0).unit("%").help("Largest acceptable grid-convergence index on the fine grid. A larger value flags the result as not mesh independent."))
        .param(ParamSpec::float("fine_value", "Fine value (manual)", 0.0, -1.0e6, 1.0e6).help("Used when the fine input is not connected; 0 means not given"))
        .param(ParamSpec::float("medium_value", "Medium value (manual)", 0.0, -1.0e6, 1.0e6).help("Value on the medium grid, used when the medium input is not connected. 0 means not given."))
        .param(ParamSpec::float("coarse_value", "Coarse value (manual)", 0.0, -1.0e6, 1.0e6).help("Value on the coarse grid, used when the coarse input is not connected. 0 means not given."))
        .help("Three-grid convergence study: observed order, extrapolated value and grid-convergence index of the chosen coefficient, plus the two-finest-grids acceptance rule. Oscillating results are flagged and not extrapolated.")
        .emulates("automotive wind-tunnel service: mesh independence check")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

/// One grid: the wired result's value, or the manual number.
struct GridIn {
    value: f64,
    placeholder: bool,
    result: Option<ForceResult>,
}

fn grid_in(ins: &[Option<Data>], idx: usize, name: &str, quantity: &str, p: &Params) -> CookResult<GridIn> {
    if json_in(ins, idx)?.is_some() {
        let r = force_in(ins, idx, name)?;
        let value = r.quantity(quantity).map_or(0.0, |(v, _)| v);
        return Ok(GridIn {
            value,
            placeholder: r.placeholder,
            result: Some(r),
        });
    }
    let manual = p.float(&format!("{name}_value"))?;
    if manual == 0.0 {
        return Err(CookError::MissingInput { index: idx, name: name.into() });
    }
    Ok(GridIn {
        value: manual,
        placeholder: false,
        result: None,
    })
}

fn study_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let quantity = p.str("quantity")?;
    let grids = [
        grid_in(ins, 0, "fine", quantity, p)?,
        grid_in(ins, 1, "medium", quantity, p)?,
        grid_in(ins, 2, "coarse", quantity, p)?,
    ];
    let mut rep = Report::new("Mesh convergence");
    if grids.iter().any(|g| g.placeholder) {
        rep.metric("data_available", Metric::new(0.0, "-").with_limit(1.0, false));
        rep.note(Severity::Warning, "a grid has no solver data yet; the study cannot be evaluated");
        let study = json!({"placeholder": true, "quantity": quantity, "pass": false});
        return Ok(vec![Data::json(study), Data::json(json!({"placeholder": true})), Data::report(rep)]);
    }
    let (accept_asked, gci_max) = (p.float("accept_pct")?, p.float("gci_max_pct")?);
    let accept = accept_asked.min(MAX_ACCEPT_PCT);
    if accept_asked > MAX_ACCEPT_PCT {
        rep.note(
            Severity::Warning,
            format!("the two-grids limit of {accept_asked} % was capped at {MAX_ACCEPT_PCT} %: a looser limit accepts meshes that are not yet independent"),
        );
    }
    let g = grid_convergence([grids[0].value, grids[1].value, grids[2].value], p.float("r21")?, p.float("r32")?).map_err(|e| ctx.fail(e))?;
    // A grid change smaller than the iteration scatter says nothing about the
    // mesh, so every wired grid has to be steadier than the acceptance level.
    let scatter = grids
        .iter()
        .filter_map(|g| g.result.as_ref()?.quantity(quantity))
        .filter(|(v, _)| *v != 0.0)
        .map(|(v, sd)| (sd / v).abs() * 100.0)
        .fold(None, |top: Option<f64>, s| Some(top.map_or(s, |t| t.max(s))));
    // Fewer than two samples give a spread of zero, which says nothing.
    let scatter_known = !grids.iter().any(|g| g.result.as_ref().is_some_and(|r| r.window < 2));
    let steady = scatter_known && scatter.is_none_or(|s| s <= accept);
    if !scatter_known {
        rep.note(
            Severity::Warning,
            "a grid's result was averaged over fewer than two iterations, so its scatter is unknown",
        );
    }
    if g.diverging {
        rep.note(
            Severity::Warning,
            "the change between grids does not shrink with refinement, so these grids show no convergence",
        );
    }
    let accepted = mesh_accepted(&g, accept, gci_max) && steady;
    let diff = last_two_diff_pct(g.values[0], g.values[1]);
    rep.metric("data_available", Metric::new(1.0, "-").with_limit(1.0, false))
        .metric("diff_fine_medium_pct", Metric::new(fin(ctx, "difference", diff)?, "%").with_limit(accept, true))
        .metric("spread_pct", Metric::new(g.spread_pct, "%"));
    if let Some(s) = scatter {
        rep.metric("iteration_scatter_pct", Metric::new(fin(ctx, "scatter", s)?, "%").with_limit(accept, true));
        if !steady {
            rep.note(
                Severity::Warning,
                format!("iteration scatter of {s:.2} % is above the {accept} % limit: the grids cannot be told apart from the run's own unsteadiness"),
            );
        }
    }
    if let Some(v) = g.gci_fine_pct {
        rep.metric("gci_fine_pct", Metric::new(v, "%").with_limit(gci_max, true));
    }
    if let Some(v) = g.p {
        rep.metric("order_p", Metric::new(v, "-"));
        if !ORDER_RANGE.contains(&v) {
            rep.note(
                Severity::Warning,
                format!("observed order {v:.2} is far from the nominal 1 to 2, so the grid-convergence index cannot be believed and the mesh is not accepted"),
            );
        }
    }
    if let Some(v) = g.phi_ext {
        rep.metric("extrapolated", Metric::new(v, "-"));
    }
    rep.metric("mesh_accepted", Metric::new(f64::from(accepted), "-").with_limit(1.0, false));
    if g.oscillatory {
        rep.note(
            Severity::Warning,
            "not monotonic: the successive differences change sign or vanish, so no order or extrapolation is reported; use the spread of the three values",
        );
    }
    if g.identical {
        rep.note(Severity::Info, "fine and medium grids agree exactly");
    }
    let mut study = serde_json::to_value(&g).unwrap_or(Value::Null);
    study["quantity"] = json!(quantity);
    study["pass"] = json!(accepted);
    study["accept_pct"] = json!(accept);
    study["gci_max_pct"] = json!(gci_max);
    let mut fine = grids[0].result.as_ref().map_or_else(|| json!({"manual": true}), ForceResult::to_json);
    fine["mesh_gci_pct"] = json!(g.gci_fine_pct);
    fine["mesh_accepted"] = json!(accepted);
    Ok(vec![Data::json(study), Data::json(fine), Data::report(rep)])
}

// ------------------------------------------------------------------ baseline vs kit

/// Verdict for a change `d` against its combined noise.
pub fn verdict(d: f64, sigma: f64, k_sigma: f64, lower_is_better: bool) -> &'static str {
    if d == 0.0 || d.abs() < k_sigma * sigma {
        "inconclusive"
    } else if (d < 0.0) == lower_is_better {
        "better"
    } else {
        "worse"
    }
}

/// Difference of a kit run against its baseline with the noise floor.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Delta {
    pub d_cda: f64,
    pub d_cda_pct: f64,
    pub sigma_cda: f64,
    pub d_cla: f64,
    pub sigma_cla: f64,
    pub d_balance_pts: Option<f64>,
    pub verdict: &'static str,
    pub verdict_downforce: &'static str,
}

/// `extra_cda` is an additional noise sd added to each run in quadrature.
#[cfg(test)]
pub fn compare(base: &ForceResult, kit: &ForceResult, k_sigma: f64, extra_cda: f64) -> Delta {
    compare_with_floor(base, kit, k_sigma, extra_cda, 0.0)
}

/// Kit against baseline. `extra_cda` is added to each run's spread in
/// quadrature, `floor_cda` is a discretisation noise floor of the
/// difference (one coarse RANS run resolves Cd only to about 0.005), added
/// once in quadrature to the CdA and ClA sigmas.
pub fn compare_with_floor(base: &ForceResult, kit: &ForceResult, k_sigma: f64, extra_cda: f64, floor_cda: f64) -> Delta {
    let sigma = |a: f64, b: f64| (a * a + b * b + 2.0 * extra_cda * extra_cda + floor_cda * floor_cda).sqrt();
    let d_cda = kit.cda - base.cda;
    let d_cla = kit.cla - base.cla;
    let (s_cda, s_cla) = (sigma(base.cda_sd, kit.cda_sd), sigma(base.cla_sd, kit.cla_sd));
    // ClA is negative for downforce: a more negative value is better
    let v_cla = match verdict(d_cla, s_cla, k_sigma, true) {
        "better" => "more downforce",
        "worse" => "less downforce",
        o => o,
    };
    Delta {
        d_cda,
        d_cda_pct: if base.cda != 0.0 { d_cda / base.cda * 100.0 } else { 0.0 },
        sigma_cda: s_cda,
        d_cla,
        sigma_cla: s_cla,
        d_balance_pts: base.balance_front_pct.zip(kit.balance_front_pct).map(|(a, b)| b - a),
        verdict: verdict(d_cda, s_cda, k_sigma, true),
        verdict_downforce: v_cla,
    }
}

fn ab_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.compare_ab", "Baseline vs Kit", Stage::Cfd)
        .input(PortSpec::new("baseline", DataKind::Json))
        .input(PortSpec::new("kit", DataKind::Json))
        .input(PortSpec::optional("benchmark", DataKind::Report))
        .output(PortSpec::new("delta", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::float("k_sigma", "Significance level", 2.0, 1.0, 4.0).unit("sigma").help("A change smaller than this many combined standard deviations is inconclusive"))
        .param(ParamSpec::float("extra_noise_cda", "Extra noise per run", 0.0, 0.0, 0.1).unit("m2").help("Added in quadrature to each run's iteration spread"))
        .param(ParamSpec::float("mesh_noise_cd", "Mesh noise floor", 0.005, 0.0, 0.05).unit("-").help("Discretisation noise of the difference in Cd, used when neither run carries a mesh-study GCI; a street kit moves Cd by only 0.005 to 0.01"))
        .help("Kit against baseline on the same recipe: changes in CdA, ClA and aero balance with iteration and mesh noise. Different flow, mesh or boundary settings are rejected. A connected benchmark must have benchmark_validated = 1 and a passing report; coefficient agreement alone does not validate the recipe. This check does not establish the full physical validation protocol.")
        .emulates("automotive wind-tunnel service: A/B comparison")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

fn rel_diff(a: f64, b: f64) -> f64 {
    if a == 0.0 { b.abs() } else { (b / a - 1.0).abs() }
}

fn ab_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let base = force_in(ins, 0, "baseline")?;
    let kit = force_in(ins, 1, "kit")?;
    let mut rep = Report::new("Baseline vs kit");
    if base.placeholder || kit.placeholder {
        rep.metric("data_available", Metric::new(0.0, "-").with_limit(1.0, false));
        rep.note(Severity::Warning, "one of the runs has no solver data yet; no comparison is made");
        let d = json!({"placeholder": true, "verdict": "inconclusive"});
        return Ok(vec![Data::json(d), Data::report(rep)]);
    }
    let same_flow = rel_diff(base.u_ms, kit.u_ms) <= 0.01 && rel_diff(base.rho, kit.rho) <= 0.01 && base.half_model == kit.half_model;
    if !same_flow {
        rep.note(
            Severity::Error,
            "baseline and kit differ in speed, density or half/full-car treatment: the difference is not attributable to the kit",
        );
    }
    let mismatches = match (&base.recipe, &kit.recipe) {
        (Some(a), Some(b)) => a.mismatches(b),
        _ => {
            rep.note(
                Severity::Warning,
                "mesh, ground/wheel, yaw and turbulence settings are not recorded for both runs; identical meshes cannot be confirmed",
            );
            vec![]
        }
    };
    if !mismatches.is_empty() {
        rep.note(
            Severity::Error,
            format!(
                "baseline and kit differ in {}: compare only runs on identical meshes and boundary treatment",
                mismatches.join(", ")
            ),
        );
    }
    let same = same_flow && mismatches.is_empty();
    rep.metric("data_available", Metric::new(1.0, "-").with_limit(1.0, false))
        .metric("same_recipe", Metric::new(f64::from(same), "-").with_limit(1.0, false));
    // protocol step 1: the recipe must have passed its benchmark first
    let validated = match ins.get(2).and_then(Option::as_ref) {
        Some(Data::Report(b)) => {
            let ok = b.metrics.get("benchmark_validated").is_some_and(|m| m.value.is_finite() && m.value == 1.0) && b.passes();
            if !ok {
                rep.note(
                    Severity::Error,
                    "the connected benchmark check has not passed: the recipe is not validated and kit deltas are not trustworthy",
                );
            }
            rep.metric("recipe_validated", Metric::new(f64::from(ok), "-").with_limit(1.0, false));
            ok
        }
        Some(_) => return Err(ctx.fail("benchmark input must be the benchmark check's report")),
        None => {
            rep.note(
                Severity::Warning,
                "no benchmark check connected: validate the recipe on a benchmark body before trusting kit deltas",
            );
            false
        }
    };
    // mesh noise of the difference: the mesh-study GCI when known, else the default floor
    let gci = base
        .mesh_gci_pct
        .into_iter()
        .chain(kit.mesh_gci_pct)
        .fold(None, |a: Option<f64>, g| Some(a.map_or(g, |a| a.max(g))));
    let floor_cda = match gci {
        Some(g) => g.abs() / 100.0 * base.cda.abs().max(kit.cda.abs()),
        None => p.float("mesh_noise_cd")? * base.a_front_m2.max(kit.a_front_m2),
    };
    let k = p.float("k_sigma")?;
    let d = compare_with_floor(&base, &kit, k, p.float("extra_noise_cda")?, floor_cda);
    rep.metric("mesh_noise_cda", Metric::new(floor_cda, "m2"));
    fin(ctx, "dCdA", d.d_cda)?;
    let significance = if d.sigma_cda > 0.0 { d.d_cda.abs() / d.sigma_cda } else { f64::INFINITY };
    let resolved = d.verdict != "inconclusive";
    rep.metric("CdA_base", Metric::new(base.cda, "m2"))
        .metric("CdA_kit", Metric::new(kit.cda, "m2"))
        .metric("dCdA", Metric::new(d.d_cda, "m2"))
        .metric("dCdA_pct", Metric::new(d.d_cda_pct, "%"))
        .metric("dClA", Metric::new(d.d_cla, "m2"))
        .metric("noise_sigma_cda", Metric::new(d.sigma_cda, "m2"))
        .metric(
            "significance",
            Metric::new(if significance.is_finite() { significance } else { 1.0e9 }, "sigma"),
        )
        .metric("delta_resolved", Metric::new(f64::from(resolved), "-").with_limit(1.0, false));
    if let Some(b) = d.d_balance_pts {
        rep.metric("balance_shift_pts", Metric::new(b, "%"));
    }
    if !resolved {
        rep.note(
            Severity::Warning,
            format!("inconclusive: the change is within {k:.1} combined standard deviations; run longer, refine the mesh or collect more iterations"),
        );
    }
    let out = json!({
        "d_cda": d.d_cda, "d_cda_pct": d.d_cda_pct, "sigma_cda": d.sigma_cda, "d_cla": d.d_cla,
        "sigma_cla": d.sigma_cla, "d_balance_pts": d.d_balance_pts, "verdict": d.verdict,
        "verdict_downforce": d.verdict_downforce, "resolved": resolved, "same_recipe": same,
        "recipe_mismatches": mismatches, "recipe_validated": validated, "mesh_noise_cda": floor_cda,
        "k_sigma": k, "cda_base": base.cda, "cda_kit": kit.cda,
    });
    Ok(vec![Data::json(out), Data::report(rep)])
}

// ------------------------------------------------------------------ validation presets

/// Validation template metadata. The wind-tunnel drag values are the ones the
/// spec cites; the Ahmed body has no absolute value here, only behaviour.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub family: &'static str,
    pub cd_exp: Option<f64>,
    pub band_pct: f64,
    pub u_ms: f64,
    pub ref_length_m: f64,
    pub ref_area_m2: f64,
    /// Share of the drag carried by slant pressure, steady-RANS behaviour to expect.
    pub slant_share_pct: Option<f64>,
    pub min_cells_million: f64,
    pub note: &'static str,
}

const DRIVAER_NOTE: &str = "published-replication baseline at 16 m/s: detailed underbody, mirrors, no engine-bay flow; moving belt and rotating wheels; half model with symmetry";
const AHMED_NOTE: &str = "slant-back bluff body; stilts carry at least 10% of the drag and must be modelled; steady RANS misses the 25-degree wake transition";

pub const PRESETS: [Preset; 5] = [
    Preset {
        id: "drivaer_fastback",
        label: "Road-car body, fastback",
        family: "drivaer",
        cd_exp: Some(0.275),
        band_pct: 5.0,
        u_ms: 16.0,
        ref_length_m: 4.6,
        ref_area_m2: 2.16,
        slant_share_pct: None,
        min_cells_million: 12.0,
        note: DRIVAER_NOTE,
    },
    Preset {
        id: "drivaer_estate",
        label: "Road-car body, estate",
        family: "drivaer",
        cd_exp: Some(0.319),
        band_pct: 5.0,
        u_ms: 16.0,
        ref_length_m: 4.6,
        ref_area_m2: 2.16,
        slant_share_pct: None,
        min_cells_million: 12.0,
        note: DRIVAER_NOTE,
    },
    Preset {
        id: "drivaer_notchback",
        label: "Road-car body, notchback",
        family: "drivaer",
        cd_exp: Some(0.277),
        band_pct: 5.0,
        u_ms: 16.0,
        ref_length_m: 4.6,
        ref_area_m2: 2.16,
        slant_share_pct: None,
        min_cells_million: 12.0,
        note: DRIVAER_NOTE,
    },
    Preset {
        id: "ahmed_25",
        label: "Slant body, 25 degrees",
        family: "ahmed",
        cd_exp: None,
        band_pct: 10.0,
        u_ms: 40.0,
        ref_length_m: 1.044,
        ref_area_m2: 0.112,
        slant_share_pct: Some(55.0),
        min_cells_million: 0.5,
        note: AHMED_NOTE,
    },
    Preset {
        id: "ahmed_35",
        label: "Slant body, 35 degrees",
        family: "ahmed",
        cd_exp: None,
        band_pct: 10.0,
        u_ms: 40.0,
        ref_length_m: 1.044,
        ref_area_m2: 0.112,
        slant_share_pct: Some(35.0),
        min_cells_million: 0.5,
        note: AHMED_NOTE,
    },
];

fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// Signed percentage error of a simulated value against the reference.
pub fn error_pct(sim: f64, reference: f64) -> f64 {
    (sim - reference) / reference * 100.0
}

/// Estate drag above both the fastback and the notchback, which stay within
/// `tol_pct` of each other.
pub fn drivaer_ordering_ok(fastback: f64, estate: f64, notchback: f64, tol_pct: f64) -> bool {
    estate > fastback && estate > notchback && last_two_diff_pct(fastback, notchback) <= tol_pct
}

fn bench_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.benchmark_check", "Benchmark Check", Stage::Cfd)
        .input(PortSpec::optional("result", DataKind::Json))
        .input(PortSpec::optional("result_b", DataKind::Json))
        .input(PortSpec::optional("result_c", DataKind::Json))
        .input(PortSpec::optional("case", DataKind::File))
        .input(PortSpec::optional("case_b", DataKind::File))
        .output(PortSpec::new("table", DataKind::Json))
        .output(PortSpec::new("preset", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::choice(
            "case_id",
            "Benchmark",
            "drivaer_fastback",
            &["drivaer_fastback", "drivaer_estate", "drivaer_notchback", "drivaer_set", "ahmed_25", "ahmed_35", "ahmed_set"],
        )
        .help("drivaer_set: fastback, estate, notchback in the three inputs. ahmed_set: 25 and 35 degrees in the first two."))
        .param(ParamSpec::float("band_pct", "Allowed drag error", 5.0, 1.0, 15.0).unit("%").help("Numerical drag comparison only. The effective band cannot exceed the preset limit; validation also requires measured mesh and solve evidence, stationarity and a verified reference mapping."))
        .param(ParamSpec::float("slant_share_pct", "Slant pressure share of drag", 0.0, 0.0, 100.0).unit("%").help("Slant-body cases, typed in by hand; 0 = not given. A connected solved case is measured instead and reported separately."))
        .param(ParamSpec::int("share_window", "Share averaging window", 1000, 1, 100_000).unit("iter").help("Final iterations of a connected case over which the slant pressure share is averaged."))
        .param(ParamSpec::float("share_band_pts", "Share tolerance", 10.0, 1.0, 30.0).unit("%").help("Largest allowed difference, in percentage points, between a computed share of the total and the benchmark share."))
        .help("coefficient_band_ok reports numerical drag agreement. benchmark_validated additionally requires measured cell count, mesh quality, two 1000-sample stationary windows, complete residuals and a verified reference mapping; within_band is its compatibility alias. Reference mappings are currently unresolved, so numerical agreement cannot pass validation. Slant-body trends remain diagnostics. This gate does not establish mesh independence or the complete physical validation protocol.")
        .emulates("automotive wind-tunnel service: validation against reference data")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

fn opt_force(ins: &[Option<Data>], idx: usize, name: &str) -> CookResult<Option<ForceResult>> {
    json_in(ins, idx)?.map(|_| force_in(ins, idx, name)).transpose()
}

/// Pressure drag on the slant surface as a percentage of the total drag of
/// all monitored surfaces of a solved case. Any missing history is an error.
fn measured_slant_share(case: &std::path::Path, window: usize) -> Result<f64, String> {
    let manifest = crate::mesh::manifest(case).ok_or("the case has no manifest")?;
    let patches: Vec<&str> = manifest["patches"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !patches.contains(&"body_slant") {
        return Err("the case has no body_slant surface".into());
    }
    let mut total = 0.0;
    let mut slant = 0.0;
    for patch in patches {
        let (pressure, all) = crate::coeffs::patch_force_x(case, patch, window)?;
        total += all;
        if patch == "body_slant" {
            slant = pressure;
        }
    }
    if total <= 0.0 {
        return Err("the total drag is not positive".into());
    }
    Ok(100.0 * slant / total)
}

/// Resolve only the carried typed replication binding against the registered
/// profile and current result; a legacy scope string cannot supply proof.
fn benchmark_reference_verified(result: &ForceResult, preset: &Preset) -> bool {
    let Some(proof) = &result.qualification else { return false };
    if proof.version != 1 || proof.solve.version != 1 || proof.solve.mesh.version != 1 {
        return false;
    }
    proof.solve.reference_binding.as_ref()
        .is_some_and(|binding| crate::reference::matches_result(binding, preset.id, result))
}

/// Roundoff tolerance for algebraic identities only, never for physical gates.
fn qualification_identity(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-12 + 1e-9 * a.abs().max(b.abs())
}

fn benchmark_min_cells(result: &ForceResult, preset: &Preset) -> u64 {
    let half = (preset.min_cells_million * 1_000_000.0).ceil() as u64;
    if result.half_model { half } else { half.saturating_mul(2) }
}

/// Recheck the recorded numbers and their relationships. The summary cannot
/// authenticate a fabricated payload or reconstruct raw residual histories.
fn benchmark_row_failures(result: &ForceResult, preset: &Preset, reference_verified: bool) -> Vec<String> {
    let mut failures = Vec::new();
    let mut require = |ok: bool, reason: &str| {
        if !ok {
            failures.push(reason.to_string());
        }
    };
    require(
        reference_verified,
        "reference mapping is unresolved: verify the experimental geometry, scale and boundary configuration before claiming benchmark validation",
    );
    require(!result.placeholder, "solver force data is absent or unusable");
    require(qualification_identity(result.u_ms, preset.u_ms), "run speed does not match the preset baseline speed");
    for (name, value) in [
        ("Cd", result.cd),
        ("CdA", result.cda),
        ("speed", result.u_ms),
        ("density", result.rho),
        ("dynamic pressure", result.q_pa),
        ("full frontal area", result.a_front_m2),
        ("reference area", result.aref_m2),
    ] {
        require(value.is_finite() && value > 0.0, &format!("{name} must be finite and positive"));
    }
    for (name, value) in [("Cd scatter", result.cd_sd), ("CdA scatter", result.cda_sd)] {
        require(value.is_finite() && value >= 0.0, &format!("{name} must be finite and nonnegative"));
    }
    require(
        result.cd_drift_pct.is_finite() && result.last_iter.is_finite(),
        "drag drift and final iteration must be finite",
    );
    require(
        qualification_identity(result.q_pa, 0.5 * result.rho * result.u_ms * result.u_ms),
        "dynamic pressure does not agree with density and speed",
    );
    require(
        qualification_identity(result.aref_m2, result.a_front_m2 * if result.half_model { 0.5 } else { 1.0 }),
        "reference area does not match the recorded half/full model",
    );
    require(
        qualification_identity(result.cda, result.cd * result.a_front_m2),
        "CdA does not agree with Cd and full frontal area",
    );
    require(
        qualification_identity(result.cda_sd, result.cd_sd * result.a_front_m2),
        "CdA scatter does not agree with Cd scatter and full frontal area",
    );
    let Some(proof) = &result.qualification else {
        require(
            false,
            "force qualification is absent: use a successful solve with measured mesh evidence and recorded normalization metadata",
        );
        return failures;
    };
    let mesh = &proof.solve.mesh;
    let convergence = &proof.convergence;
    require(
        proof.version == 1 && proof.solve.version == 1 && mesh.version == 1,
        "unsupported qualification, solve or mesh evidence version",
    );
    let fingerprint_ok = |value: &str| value.len() == 16 && value.bytes().all(|b| b.is_ascii_hexdigit());
    require(
        fingerprint_ok(&mesh.mesh_fingerprint) && fingerprint_ok(&proof.solve.input_fingerprint),
        "mesh and solve input fingerprints must each contain 16 hexadecimal characters",
    );
    let minimum = benchmark_min_cells(result, preset);
    require(
        mesh.cells >= minimum && mesh.cells > 0,
        &format!(
            "measured mesh has {} cells; this {} model requires at least {minimum}",
            mesh.cells,
            if result.half_model { "half" } else { "full" }
        ),
    );
    require(
        mesh.max_nonortho.is_finite() && (0.0..70.0).contains(&mesh.max_nonortho),
        "measured maximum non-orthogonality must be nonnegative and strictly below 70 degrees",
    );
    require(
        mesh.max_skew.is_finite() && (0.0..4.0).contains(&mesh.max_skew),
        "measured maximum skewness must be nonnegative and strictly below 4",
    );
    require(
        result.window == 1000 && proof.window == 1000 && convergence.window == 1000,
        "coefficient and convergence windows must all be exactly 1000 samples",
    );
    let enough = result.samples >= 2000;
    require(
        enough && proof.samples == result.samples && convergence.iterations == result.samples,
        "coefficient and convergence sample counts must agree and contain at least 2000 samples",
    );
    let source_matches = matches!((result.source.as_str(), proof.column.as_str()), ("forces", "Fx") | ("coefficients", "Cd"));
    require(source_matches, "force source and convergence column must agree: forces/Fx or coefficients/Cd");
    let residuals_checked = ["p", "U", "k", "omega"].iter().all(|field| convergence.residuals.contains_key(*field));
    let residuals_ok = residuals_checked && convergence.residuals.values().all(|r| r.is_finite() && *r >= 0.0 && *r <= 1e-5);
    require(residuals_checked, "complete p, U, k and omega residuals are required");
    require(residuals_ok, "every recorded residual must be finite, nonnegative and at most 1e-5");
    require(
        convergence.mean_prev.is_finite() && convergence.mean_prev > 0.0 && convergence.mean_last.is_finite() && convergence.mean_last > 0.0,
        "both convergence-window drag means must be finite and positive",
    );
    let difference = 100.0 * ((convergence.mean_last - convergence.mean_prev) / convergence.mean_last).abs();
    let stationary = convergence.window_diff_pct.is_finite()
        && (0.0..=0.2).contains(&convergence.window_diff_pct)
        && difference.is_finite()
        && difference <= 0.2
        && convergence.drift_pct.is_finite()
        && (0.0..=0.1).contains(&convergence.drift_pct);
    require(stationary, "two-window mean change must be at most 0.2% and drift at most 0.1%");
    require(
        qualification_identity(convergence.window_diff_pct, difference),
        "reported window change does not agree with the two window means",
    );
    require(
        convergence.sd_pct.is_finite() && convergence.sd_pct >= 0.0,
        "convergence scatter must be finite and nonnegative",
    );
    require(
        qualification_identity(convergence.sd_pct, 100.0 * result.cd_sd / result.cd.abs()),
        "convergence scatter does not agree with reported Cd and Cd scatter",
    );
    require(
        convergence.drift_pct + 1e-12 >= result.cd_drift_pct.abs(),
        "the two-window maximum drift is smaller than the reported final-window drag drift",
    );
    let mean_cd = match result.source.as_str() {
        "forces" => convergence.mean_last / (result.q_pa * result.aref_m2),
        "coefficients" => convergence.mean_last,
        _ => f64::NAN,
    };
    require(
        qualification_identity(mean_cd, result.cd),
        "convergence source mean does not reproduce Cd under the recorded normalization",
    );
    let converged = enough && residuals_ok && stationary;
    require(
        converged
            && convergence.converged
            && convergence.enough_iterations == enough
            && convergence.residuals_checked == residuals_checked
            && convergence.residuals_ok == residuals_ok
            && convergence.unsteady == (convergence.sd_pct > 0.5),
        "saved convergence flags are false or disagree with the numerical evidence",
    );
    failures
}

fn benchmark_row_qualified(result: &ForceResult, preset: &Preset, reference_verified: bool) -> bool {
    benchmark_row_failures(result, preset, reference_verified).is_empty()
}

fn bench_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let case = p.str("case_id")?;
    let band = p.float("band_pct")?;
    fin(ctx, "drag error band", band)?;
    if band <= 0.0 {
        return Err(ctx.fail("drag error band must be positive"));
    }
    let mut rep = Report::new("Benchmark check");
    let results = [opt_force(ins, 0, "result")?, opt_force(ins, 1, "result_b")?, opt_force(ins, 2, "result_c")?];
    let ids: &[&str] = match case {
        "drivaer_set" => &["drivaer_fastback", "drivaer_estate", "drivaer_notchback"],
        "ahmed_set" => &["ahmed_25", "ahmed_35"],
        one => &[one],
    };
    let lead = preset(ids[0]).ok_or_else(|| ctx.fail(format!("unknown benchmark '{case}'")))?;
    let preset_json = serde_json::to_value(lead).unwrap_or(Value::Null);
    let missing = results.iter().take(ids.len()).any(|r| !matches!(r, Some(r) if !r.placeholder));
    if missing {
        rep.metric("coefficient_band_ok", Metric::new(0.0, "-"))
            .metric("benchmark_validated", Metric::new(0.0, "-").with_limit(1.0, false))
            .metric("within_band", Metric::new(0.0, "-").with_limit(1.0, false));
        rep.note(
            Severity::Warning,
            "white-box placeholder: runs the benchmark template through the solver chain and compares its drag with the published wind-tunnel value; connect the solved benchmark result",
        );
        return Ok(vec![
            Data::json(json!({"rows": [], "placeholder": true})),
            Data::json(preset_json),
            Data::report(rep),
        ]);
    }
    let mut rows = Vec::new();
    let mut all_in_band = true;
    let mut all_qualified = true;
    let mut protocol_ok = true;
    let mut cds = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        let (pr, r) = (
            preset(id).ok_or_else(|| ctx.fail("unknown benchmark"))?,
            results[i].as_ref().ok_or_else(|| ctx.fail("missing result"))?,
        );
        cds.push(r.cd);
        let effective_band = band.min(pr.band_pct);
        let (err, ok) = match pr.cd_exp {
            Some(exp) => {
                let e = error_pct(r.cd, exp);
                (Some(e), r.cd.is_finite() && r.cd > 0.0 && e.is_finite() && e.abs() <= effective_band)
            }
            None => (None, false),
        };
        all_in_band &= ok;
        if let Some(e) = err {
            rep.metric(format!("cd_error_pct_{id}"), Metric::new(e.abs(), "%").with_limit(effective_band, true));
        }
        let reference_verified = benchmark_reference_verified(r, pr);
        let reference_profile = if reference_verified {
            r.qualification.as_ref().and_then(|proof| proof.solve.reference_binding.as_ref()).map(|binding| binding.profile.as_str())
        } else {
            None
        };
        let qualified = benchmark_row_qualified(r, pr, reference_verified);
        let failures = if qualified {
            Vec::new()
        } else {
            benchmark_row_failures(r, pr, reference_verified)
        };
        all_qualified &= qualified;
        for failure in &failures {
            rep.note(Severity::Warning, format!("{id}: {failure}"));
        }
        rep.metric(format!("coefficient_band_ok_{id}"), Metric::new(f64::from(ok), "-"))
            .metric(format!("row_qualified_{id}"), Metric::new(f64::from(qualified), "-"))
            .metric(format!("reference_verified_{id}"), Metric::new(f64::from(reference_verified), "-"));
        rows.push(json!({"case": id, "cd_exp": pr.cd_exp, "cd_sim": r.cd, "error_pct": err,
            "requested_band_pct": band, "band_pct": effective_band, "coefficient_band_ok": ok,
            "row_qualified": qualified, "reference_verified": reference_verified,
            "reference_profile": reference_profile,
            "reference_kind": reference_profile.map(|_| "published_replication"),
            "original_experiment_authenticated": false,
            "measured_cells": r.qualification.as_ref().map(|proof| proof.solve.mesh.cells),
            "minimum_cells": benchmark_min_cells(r, pr), "qualification_failures": failures,
            "pass": qualified && ok}));
    }
    if lead.family == "drivaer" {
        if ids.len() == 3 && !drivaer_ordering_ok(cds[0], cds[1], cds[2], 3.0) {
            protocol_ok = false;
            rep.note(
                Severity::Error,
                "drag ordering is wrong: the estate must be highest, fastback and notchback close together",
            );
        }
        protocol_ok &= all_in_band;
    } else {
        // G25 trends retain their diagnostic warnings. They cannot replace
        // measured solve evidence or a verified reference configuration.
        let (mut trends_hold, mut measured, mut unmeasured) = (true, false, false);
        if ids.len() == 2 {
            let ordering = cds[0] > cds[1];
            trends_hold &= ordering;
            rep.metric("drag_ordering_ok", Metric::new(f64::from(ordering), "-"));
            if !ordering {
                rep.note(Severity::Warning, "expected higher drag at 25 than at 35 degrees; recorded, tolerance not tightened; read this with the mesh study, since a grid can hold the wrong flow state");
            }
        }
        let window = p.int("share_window")?.max(1) as usize;
        for (i, id) in ids.iter().enumerate() {
            let Some(case) = file_in(ins, 3 + i)? else { continue };
            let want = preset(id).and_then(|pr| pr.slant_share_pct);
            match measured_slant_share(&case, window) {
                Ok(share) => {
                    rep.metric(format!("slant_share_pct_{id}"), Metric::new(share, "%"));
                    if let Some(want) = want {
                        let off = (share - want).abs();
                        rep.metric(format!("slant_share_error_pts_{id}"), Metric::new(off, "%"));
                        measured = true;
                        if off > p.float("share_band_pts")? {
                            trends_hold = false;
                            rep.note(Severity::Warning, format!("{id}: measured slant pressure share {share:.0}% against about {want:.0}% expected; read this with the mesh study before blaming the turbulence model"));
                        }
                    }
                }
                Err(e) => {
                    unmeasured = true;
                    rep.note(Severity::Warning, format!("{id}: slant pressure share not measured: {e}"));
                }
            }
        }
        let share = p.float("slant_share_pct")?;
        if let (Some(want), true) = (lead.slant_share_pct, share > 0.0) {
            let off = (share - want).abs();
            rep.metric("slant_share_error_pts", Metric::new(off, "%"));
            measured = true;
            if off > p.float("share_band_pts")? {
                trends_hold = false;
                rep.note(
                    Severity::Warning,
                    format!(
                        "slant pressure share {share:.0}% against about {want:.0}% expected; read this with the mesh study before blaming the turbulence model"
                    ),
                );
            }
        }
        protocol_ok &= measured && trends_hold && !unmeasured;
        rep.metric("slant_trends_ok", Metric::new(f64::from(protocol_ok), "-"));
        if unmeasured {
            // A share that was asked for and could not be read must not look like one that held.
            rep.metric("share_measured", Metric::new(0.0, "-").with_limit(1.0, false));
        }
        rep.note(
            Severity::Info,
            "no absolute drag reference for this case: coefficient_band_ok is false; slant trends are reported separately",
        );
    }
    let validated = all_qualified && protocol_ok && rep.passes();
    rep.metric("coefficient_band_ok", Metric::new(f64::from(all_in_band), "-"))
        .metric("benchmark_validated", Metric::new(f64::from(validated), "-").with_limit(1.0, false))
        .metric("within_band", Metric::new(f64::from(validated), "-").with_limit(1.0, false));
    if !validated {
        rep.note(
            Severity::Warning,
            "benchmark validation has not passed; coefficient agreement alone does not qualify kit comparisons",
        );
    }
    rep.note(Severity::Info, "benchmark_validated covers the recorded row evidence and reference gate only; mesh independence and the complete physical validation protocol require separate evidence");
    let reference_profile = rows.first().and_then(|row| row["reference_profile"].as_str())
        .filter(|profile| rows.iter().all(|row| row["reference_profile"].as_str() == Some(*profile)))
        .map(str::to_owned);
    if let Some(profile) = &reference_profile {
        rep.note(Severity::Info, format!("reference profile {profile}: published replication; the original experiment is not independently authenticated"));
    }
    Ok(vec![
        Data::json(json!({"rows": rows, "placeholder": false,
        "reference_profile": reference_profile,
        "reference_kind": reference_profile.as_ref().map(|_| "published_replication"),
        "original_experiment_authenticated": false,
        "coefficient_band_ok": all_in_band, "benchmark_validated": validated, "within_band": validated})),
        Data::json(preset_json),
        Data::report(rep),
    ])
}

pub(crate) fn register(reg: &mut Registry) {
    reg.add(FnNode::new(study_info, study_cook));
    reg.add(FnNode::new(ab_info, ab_cook));
    reg.add(FnNode::new(bench_info, bench_cook));
}

#[cfg(test)]
mod tests {
    use crate::types::Recipe;
    use super::*;
    use crate::util::testutil::ctx;
    use vernon_graph::ParamValue;

    fn run(info: fn() -> NodeTypeInfo, cook: crate::util::CookFn, ins: &[Option<Data>], set: &[(&str, ParamValue)]) -> CookResult<Vec<Data>> {
        let mut p = Params::from_specs(&info().params);
        for (k, v) in set {
            p.set(k, v.clone());
        }
        cook(&ctx("study"), ins, &p)
    }

    #[test]
    fn golden_celik_worked_example_g12() {
        let g = grid_convergence([6.063, 5.972, 5.863], 1.5, 4.0 / 3.0).unwrap();
        assert!((g.p.unwrap() - 1.53).abs() < 0.02, "{:?}", g.p);
        assert!((g.phi_ext.unwrap() - 6.168).abs() < 0.005, "{:?}", g.phi_ext);
        assert!((g.ea_pct - 1.50).abs() < 0.01);
        assert!((g.gci_fine_pct.unwrap() - 2.2).abs() < 0.1, "{:?}", g.gci_fine_pct);
        assert!(!g.oscillatory);
    }

    #[test]
    fn golden_drag_study_g13() {
        let g = grid_convergence([0.2825, 0.2862, 0.2925], 1.3, 1.3).unwrap();
        assert!((g.p.unwrap() - 2.03).abs() < 0.01);
        assert!((g.phi_ext.unwrap() - 0.2772).abs() < 0.0002);
        assert!((g.ea_pct - 1.31).abs() < 0.01);
        assert!((g.gci_fine_pct.unwrap() - 2.33).abs() < 0.02);
        assert!(mesh_accepted(&g, 2.0, 3.0));
    }

    #[test]
    fn golden_acceptance_rule_g14() {
        assert!((last_two_diff_pct(0.2810, 0.2780) - 1.07).abs() < 0.005);
        assert!(last_two_diff_pct(0.2810, 0.2780) < 2.0);
        assert!((last_two_diff_pct(0.295, 0.282) - 4.41).abs() < 0.005);
        assert!(last_two_diff_pct(0.295, 0.282) > 2.0);
        let bad = grid_convergence([0.295, 0.282, 0.265], 1.3, 1.3).unwrap();
        assert!(!mesh_accepted(&bad, 2.0, 3.0));
    }

    #[test]
    fn golden_oscillatory_g15_has_no_extrapolation() {
        let g = grid_convergence([0.2840, 0.2810, 0.2860], 1.3, 1.3).unwrap();
        assert!(g.oscillatory);
        assert!(g.p.is_none() && g.phi_ext.is_none() && g.gci_fine_pct.is_none());
        assert!((g.spread_pct - 1.76).abs() < 0.01);
        assert!(mesh_accepted(&g, 2.0, 3.0), "small oscillation within the spread allowance");
        assert!(!mesh_accepted(&g, 0.5, 3.0));
    }

    #[test]
    fn degenerate_grids_are_handled() {
        let same = grid_convergence([0.28, 0.28, 0.29], 1.3, 1.3).unwrap();
        assert!(same.identical && same.gci_fine_pct == Some(0.0));
        let flat_coarse = grid_convergence([0.28, 0.29, 0.29], 1.3, 1.3).unwrap();
        assert!(flat_coarse.oscillatory && flat_coarse.p.is_none());
        assert!(grid_convergence([0.0, 1.0, 2.0], 1.3, 1.3).is_err());
        assert!(grid_convergence([1.0, f64::NAN, 2.0], 1.3, 1.3).is_err());
        assert!(grid_convergence([1.0, 1.1, 1.3], 1.0, 1.3).is_err());
        // a finer grid that moves the result more than the coarse step still returns a number or a clean None
        let g = grid_convergence([0.30, 0.29, 0.288], 1.3, 1.3).unwrap();
        assert!(g.p.iter().all(|p| p.is_finite()));
    }

    fn fr(cd: f64, sd: f64) -> ForceResult {
        ForceResult {
            source: "forces".into(),
            u_ms: 30.0,
            rho: 1.225,
            a_front_m2: 2.0,
            aref_m2: 1.0,
            half_model: true,
            cd,
            cd_sd: sd,
            cda: cd * 2.0,
            cda_sd: sd * 2.0,
            window: 100,
            samples: 1000,
            ..ForceResult::default()
        }
    }

    #[test]
    fn mesh_study_node_from_three_results() {
        let ins: Vec<Option<Data>> = [0.2825, 0.2862, 0.2925].iter().map(|c| Some(Data::json(fr(*c, 0.001).to_json()))).collect();
        let out = run(study_info, study_cook, &ins, &[]).unwrap();
        let r = out[2].as_report().unwrap();
        assert!(r.passes(), "{r:?}");
        assert!((r.metrics["order_p"].value - 2.03).abs() < 0.01);
        assert_eq!(out[0].as_json().unwrap()["pass"], true);
        assert_eq!(out[1].as_json().unwrap()["mesh_accepted"], true);
        // too coarse a fine grid fails the gate
        let ins: Vec<Option<Data>> = [0.295, 0.282, 0.26].iter().map(|c| Some(Data::json(fr(*c, 0.001).to_json()))).collect();
        let out = run(study_info, study_cook, &ins, &[]).unwrap();
        assert!(!out[2].as_report().unwrap().passes());
    }

    #[test]
    fn grids_that_do_not_converge_are_not_accepted() {
        let accepted = |phi: [f64; 3], r: f64| mesh_accepted(&grid_convergence(phi, r, r).unwrap(), 2.0, 3.0);
        assert!(accepted([0.2825, 0.2862, 0.2925], 1.3));
        // Equal steps have no order at all.
        assert!(!accepted([0.300, 0.304, 0.308], 1.3));
        // The change grows with refinement.
        assert!(!accepted([0.300, 0.298, 0.297], 1.3));
        // Fine and medium agree, but the coarse grid is far away.
        assert!(!accepted([0.30, 0.30, 0.50], 1.3));
        assert!(accepted([0.30, 0.30, 0.302], 1.3));
    }

    #[test]
    fn the_order_uses_the_signed_log_when_the_ratios_differ() {
        // Celik's form: ln|e32/e21| keeps its sign before q is added.
        let g = grid_convergence([0.300, 0.2943, 0.2911], 1.2697, 1.255).unwrap();
        assert!((g.p.unwrap() - 2.15).abs() < 0.02, "{g:?}");
        assert!(g.diverging);
    }

    #[test]
    fn unknown_scatter_is_not_read_as_steady() {
        let one_sample = |c: f64| {
            let mut r = fr(c, 0.0);
            r.window = 1;
            Some(Data::json(r.to_json()))
        };
        let ins = vec![one_sample(0.2825), one_sample(0.2862), one_sample(0.2925)];
        let out = run(study_info, study_cook, &ins, &[]).unwrap();
        assert_eq!(out[0].as_json().unwrap()["pass"], false);
    }

    #[test]
    fn mesh_study_refuses_scatter_above_the_limit_and_an_unbelievable_order() {
        // The same grids pass until each run scatters more than the two-grid limit.
        let ins: Vec<Option<Data>> = [0.2825, 0.2862, 0.2925].iter().map(|c| Some(Data::json(fr(*c, 0.01).to_json()))).collect();
        let out = run(study_info, study_cook, &ins, &[]).unwrap();
        let r = out[2].as_report().unwrap();
        assert_eq!(out[0].as_json().unwrap()["pass"], false);
        assert!(r.metrics["iteration_scatter_pct"].value > 2.0, "{r:?}");
        assert!(r.notes.iter().any(|(s, m)| *s == Severity::Warning && m.contains("scatter")), "{r:?}");
        // Steady runs that agree, with an order far above what the scheme can give.
        let ins: Vec<Option<Data>> = [0.3687, 0.3709, 0.3944].iter().map(|c| Some(Data::json(fr(*c, 0.0001).to_json()))).collect();
        let out = run(
            study_info,
            study_cook,
            &ins,
            &[("r21", ParamValue::Float(1.27)), ("r32", ParamValue::Float(1.255))],
        )
        .unwrap();
        let r = out[2].as_report().unwrap();
        assert!(r.metrics["order_p"].value > 4.0 && r.metrics["diff_fine_medium_pct"].value < 2.0, "{r:?}");
        assert_eq!(out[0].as_json().unwrap()["pass"], false);
        // Typed-in values have no scatter to judge.
        let manual = [
            ("fine_value", ParamValue::Float(0.2825)),
            ("medium_value", ParamValue::Float(0.2862)),
            ("coarse_value", ParamValue::Float(0.2925)),
        ];
        let out = run(study_info, study_cook, &[], &manual).unwrap();
        assert!(!out[2].as_report().unwrap().metrics.contains_key("iteration_scatter_pct"));
        assert_eq!(out[0].as_json().unwrap()["pass"], true);
    }

    #[test]
    fn two_grid_limit_is_capped_at_two_percent() {
        // the two finest grids differ by 3.0 %: a 5 % limit would accept them, the cap does not
        let ins: Vec<Option<Data>> = [0.2800, 0.2884, 0.2975].iter().map(|c| Some(Data::json(fr(*c, 0.001).to_json()))).collect();
        let out = run(study_info, study_cook, &ins, &[("accept_pct", ParamValue::Float(5.0))]).unwrap();
        let r = out[2].as_report().unwrap();
        assert_eq!(out[0].as_json().unwrap()["accept_pct"], MAX_ACCEPT_PCT);
        assert_eq!(out[0].as_json().unwrap()["pass"], false);
        assert!(r.notes.iter().any(|(s, m)| *s == Severity::Warning && m.contains("capped")), "{r:?}");
        // inside the cap nothing is said
        let out = run(study_info, study_cook, &ins, &[("accept_pct", ParamValue::Float(2.0))]).unwrap();
        assert!(!out[2].as_report().unwrap().notes.iter().any(|(_, m)| m.contains("capped")));
    }

    #[test]
    fn mesh_study_manual_values_and_missing_inputs() {
        let manual = [
            ("fine_value", ParamValue::Float(6.063)),
            ("medium_value", ParamValue::Float(5.972)),
            ("coarse_value", ParamValue::Float(5.863)),
            ("r21", ParamValue::Float(1.5)),
            ("r32", ParamValue::Float(1.333)),
        ];
        let out = run(study_info, study_cook, &[], &manual).unwrap();
        assert!((out[0].as_json().unwrap()["p"].as_f64().unwrap() - 1.53).abs() < 0.02);
        assert!(run(study_info, study_cook, &[], &[]).is_err());
        // a placeholder grid fails closed
        let ph = ForceResult {
            placeholder: true,
            ..ForceResult::default()
        };
        let ins: Vec<Option<Data>> = (0..3).map(|_| Some(Data::json(ph.to_json()))).collect();
        let out = run(study_info, study_cook, &ins, &[]).unwrap();
        assert!(!out[2].as_report().unwrap().passes());
        assert_eq!(out[0].as_json().unwrap()["pass"], false);
    }

    #[test]
    fn golden_compare_g19_worse() {
        let (base, kit) = (fr(0.320, 0.0015), fr(0.330, 0.0015));
        let d = compare(&base, &kit, 2.0, 0.0);
        assert!((d.d_cda - 0.020).abs() < 1e-12);
        assert!((d.d_cda_pct - 3.125).abs() < 1e-9);
        assert_eq!(d.verdict, "worse");
    }

    #[test]
    fn golden_compare_g20_inconclusive_with_noise() {
        let (base, kit) = (fr(0.320, 0.006), fr(0.330, 0.006));
        let d = compare(&base, &kit, 2.0, 0.0);
        assert_eq!(d.verdict, "inconclusive");
        // the same runs with a tight spread resolve, and a drag cut reads as better
        let d = compare(&fr(0.330, 0.001), &fr(0.320, 0.001), 2.0, 0.0);
        assert_eq!(d.verdict, "better");
        // extra noise turns a resolved change inconclusive again
        assert_eq!(compare(&fr(0.330, 0.001), &fr(0.320, 0.001), 2.0, 0.02).verdict, "inconclusive");
        assert_eq!(compare(&fr(0.32, 0.0), &fr(0.32, 0.0), 2.0, 0.0).verdict, "inconclusive");
    }

    #[test]
    fn downforce_verdict_sign() {
        let mut base = fr(0.32, 0.001);
        let mut kit = fr(0.32, 0.001);
        base.cla = -0.5;
        kit.cla = -0.6;
        assert_eq!(compare(&base, &kit, 2.0, 0.0).verdict_downforce, "more downforce");
        kit.cla = -0.4;
        assert_eq!(compare(&base, &kit, 2.0, 0.0).verdict_downforce, "less downforce");
    }

    #[test]
    fn compare_node_gates_recipe_and_resolution() {
        let w = |r: ForceResult| Some(Data::json(r.to_json()));
        let out = run(ab_info, ab_cook, &[w(fr(0.320, 0.0015)), w(fr(0.340, 0.0015))], &[]).unwrap();
        let r = out[1].as_report().unwrap();
        assert!(r.passes(), "{r:?}");
        assert_eq!(out[0].as_json().unwrap()["verdict"], "worse");
        assert!((r.metrics["dCdA"].value - 0.04).abs() < 1e-12);
        // inconclusive fails the resolved gate
        let out = run(ab_info, ab_cook, &[w(fr(0.320, 0.006)), w(fr(0.330, 0.006))], &[]).unwrap();
        assert!(!out[1].as_report().unwrap().passes());
        // different recipe is rejected
        let mut other = fr(0.330, 0.0015);
        other.u_ms = 40.0;
        let out = run(ab_info, ab_cook, &[w(fr(0.320, 0.0015)), w(other)], &[]).unwrap();
        assert!(!out[1].as_report().unwrap().passes());
        assert!(run(ab_info, ab_cook, &[w(fr(0.32, 0.0))], &[]).is_err());
    }

    #[test]
    fn compare_node_applies_mesh_noise_floor() {
        let w = |r: ForceResult| Some(Data::json(r.to_json()));
        // a 0.005 Cd change with a tight iteration spread sits inside the default floor
        let out = run(ab_info, ab_cook, &[w(fr(0.320, 0.0002)), w(fr(0.325, 0.0002))], &[]).unwrap();
        assert_eq!(out[0].as_json().unwrap()["verdict"], "inconclusive");
        assert!((out[1].as_report().unwrap().metrics["mesh_noise_cda"].value - 0.01).abs() < 1e-12);
        // without the floor it would have resolved
        let out = run(
            ab_info,
            ab_cook,
            &[w(fr(0.320, 0.0002)), w(fr(0.325, 0.0002))],
            &[("mesh_noise_cd", ParamValue::Float(0.0))],
        )
        .unwrap();
        assert_eq!(out[0].as_json().unwrap()["verdict"], "worse");
        // a mesh-study GCI replaces the default floor
        let mut base = fr(0.320, 0.0002);
        base.mesh_gci_pct = Some(0.5);
        let out = run(ab_info, ab_cook, &[w(base), w(fr(0.325, 0.0002))], &[]).unwrap();
        let m = out[1].as_report().unwrap().metrics["mesh_noise_cda"].value;
        assert!((m - 0.005 * 0.65).abs() < 1e-12, "{m}");
        assert_eq!(out[0].as_json().unwrap()["verdict"], "worse");
    }

    #[test]
    fn compare_node_rejects_mismatched_mesh_or_boundary_treatment() {
        use crate::types::Recipe;
        let w = |r: ForceResult| Some(Data::json(r.to_json()));
        let recipe = Recipe {
            dx0_m: Some(0.32),
            base_cells: Some(1000.0),
            levels: Some("{\"body\":[4,5]}".into()),
            ground_mode: "moving".into(),
            wheel_mode: "rotating".into(),
            yaw_deg: 0.0,
            turbulence: "kOmegaSST".into(),
            separated_flow: false,
        };
        let (mut base, mut kit) = (fr(0.320, 0.0015), fr(0.340, 0.0015));
        base.recipe = Some(recipe.clone());
        kit.recipe = Some(recipe.clone());
        let out = run(ab_info, ab_cook, &[w(base.clone()), w(kit.clone())], &[]).unwrap();
        assert!(out[1].as_report().unwrap().passes());
        assert_eq!(out[0].as_json().unwrap()["same_recipe"], true);
        // a stationary-ground baseline against a moving-ground kit run is refused
        kit.recipe = Some(Recipe {
            ground_mode: "stationary".into(),
            ..recipe.clone()
        });
        let out = run(ab_info, ab_cook, &[w(base.clone()), w(kit.clone())], &[]).unwrap();
        assert!(!out[1].as_report().unwrap().passes());
        assert_eq!(out[0].as_json().unwrap()["recipe_mismatches"][0], "ground treatment");
        // so is a different mesh
        kit.recipe = Some(Recipe {
            levels: Some("{\"body\":[3,4]}".into()),
            ..recipe
        });
        let out = run(ab_info, ab_cook, &[w(base), w(kit)], &[]).unwrap();
        assert_eq!(out[0].as_json().unwrap()["recipe_mismatches"][0], "refinement levels");
    }

    #[test]
    fn compare_node_requires_a_passed_benchmark_when_connected() {
        let w = |r: ForceResult| Some(Data::json(r.to_json()));
        let bench = |within: f64| {
            let mut r = Report::new("Benchmark check");
            r.metric("within_band", Metric::new(within, "-").with_limit(1.0, false));
            r.metric("benchmark_validated", Metric::new(within, "-").with_limit(1.0, false));
            Some(Data::report(r))
        };
        let out = run(ab_info, ab_cook, &[w(fr(0.320, 0.0015)), w(fr(0.340, 0.0015)), bench(1.0)], &[]).unwrap();
        assert!(out[1].as_report().unwrap().passes());
        assert_eq!(out[0].as_json().unwrap()["recipe_validated"], true);
        let out = run(ab_info, ab_cook, &[w(fr(0.320, 0.0015)), w(fr(0.340, 0.0015)), bench(0.0)], &[]).unwrap();
        let r = out[1].as_report().unwrap();
        assert!(!r.passes());
        assert!(r.notes.iter().any(|(s, t)| *s == Severity::Error && t.contains("not validated")));
    }

    #[test]
    fn benchmark_golden_drivaer_g24() {
        assert!((error_pct(0.2825, 0.275) - 2.727).abs() < 0.01);
        let w = |c: f64| Some(Data::json(fr(c, 0.001).to_json()));
        let set = [("case_id", ParamValue::Str("drivaer_set".into()))];
        let out = run(bench_info, bench_cook, &[w(0.280), w(0.325), w(0.279)], &set).unwrap();
        let r = out[2].as_report().unwrap();
        assert_eq!(r.metrics["coefficient_band_ok"].value, 1.0);
        assert_eq!(r.metrics["benchmark_validated"].value, 0.0);
        assert_eq!(r.metrics["within_band"].value, 0.0);
        assert!(!r.passes(), "legacy coefficient fixtures carry no measured qualification: {r:?}");
        assert_eq!(out[0].as_json().unwrap()["rows"].as_array().unwrap().len(), 3);
        // fastback 7% high: outside the band
        let out = run(bench_info, bench_cook, &[w(0.294), w(0.325), w(0.279)], &set).unwrap();
        assert_eq!(out[2].as_report().unwrap().metrics["coefficient_band_ok"].value, 0.0);
        assert!(!out[2].as_report().unwrap().passes());
        // wrong ordering fails even when each drag is inside the band:
        // fastback and notchback 7% apart (-1.6% and +4.9% from the tunnel values)
        let out = run(bench_info, bench_cook, &[w(0.2705), w(0.325), w(0.2905)], &set).unwrap();
        let r = out[2].as_report().unwrap();
        assert_eq!(r.metrics["coefficient_band_ok"].value, 1.0);
        assert!(r.metrics.values().filter(|m| m.limit == Some(5.0)).all(|m| m.passes() == Some(true)), "{r:?}");
        assert!(!drivaer_ordering_ok(0.2705, 0.325, 0.2905, 3.0));
        assert!(!r.passes() && r.metrics["within_band"].value == 0.0);
        assert!(drivaer_ordering_ok(0.275, 0.319, 0.277, 3.0));
        assert!(!drivaer_ordering_ok(0.319, 0.275, 0.277, 3.0));
    }

    #[test]
    fn benchmark_single_case_and_ahmed_soft_checks() {
        let w = |c: f64| Some(Data::json(fr(c, 0.001).to_json()));
        let out = run(bench_info, bench_cook, &[w(0.279)], &[("case_id", ParamValue::Str("drivaer_estate".into()))]).unwrap();
        assert!(!out[2].as_report().unwrap().passes(), "estate drag 0.279 is 12% low");
        let ahmed = [("case_id", ParamValue::Str("ahmed_set".into()))];
        let out = run(bench_info, bench_cook, &[w(0.30), w(0.27)], &ahmed).unwrap();
        let r = out[2].as_report().unwrap();
        assert_eq!(r.metrics["drag_ordering_ok"].value, 1.0);
        assert_eq!(r.metrics["benchmark_validated"].value, 0.0);
        assert!(!r.passes(), "measured evidence and reference compatibility are absent");
        // The known RANS shortfall stays a warning; missing proof fails validation.
        let out = run(bench_info, bench_cook, &[w(0.26), w(0.27)], &ahmed).unwrap();
        let r = out[2].as_report().unwrap();
        assert!(!r.passes() && r.notes.iter().any(|(s, _)| *s == Severity::Warning));
        let share = [("case_id", ParamValue::Str("ahmed_25".into())), ("slant_share_pct", ParamValue::Float(30.0))];
        let out = run(bench_info, bench_cook, &[w(0.3)], &share).unwrap();
        assert!(!out[2].as_report().unwrap().passes());
        assert_eq!(out[1].as_json().unwrap()["family"], "ahmed");
    }

    #[test]
    fn benchmark_without_result_is_a_placeholder_that_fails_the_gate() {
        let out = run(bench_info, bench_cook, &[], &[]).unwrap();
        let r = out[2].as_report().unwrap();
        assert!(!r.passes());
        assert_eq!(r.metrics["benchmark_validated"].value, 0.0);
        assert_eq!(r.metrics["coefficient_band_ok"].value, 0.0);
        assert!(r.notes.iter().any(|(_, t)| t.starts_with("white-box placeholder:")));
        assert_eq!(out[1].as_json().unwrap()["id"], "drivaer_fastback");
        assert_eq!(out[1].as_json().unwrap()["cd_exp"], 0.275);
    }

    // Synthetic schema fixtures only: no real solve, source mapping or runtime
    // acceptance is claimed. Production nodes must still reject their scope.
    fn wp9_qualified_payload(cd: f64, half: bool, cells: u64, source: &str) -> Value {
        let area = 2.16;
        let aref = area * if half { 0.5 } else { 1.0 };
        let rho = 1.184;
        let speed = 16.0_f64;
        let q = 0.5 * rho * speed.powi(2);
        let mean = if source == "forces" { cd * q * aref } else { cd };
        let mut r = fr(cd, 0.001);
        r.source = source.into();
        r.half_model = half;
        r.rho = rho;
        r.u_ms = speed;
        r.a_front_m2 = area;
        r.aref_m2 = aref;
        r.q_pa = q;
        r.cda = cd * area;
        r.cda_sd = r.cd_sd * area;
        r.window = 1000;
        r.samples = 2000;
        r.last_iter = 2000.0;
        let mut payload = r.to_json();
        payload["qualification"] = json!({
            "version": 1,
            "solve": {
                "version": 1,
                "mesh": {
                    "version": 1, "cells": cells,
                    "mesh_fingerprint": "0123456789abcdef",
                    "max_nonortho": 30.0, "max_skew": 1.0
                },
                "input_fingerprint": "fedcba9876543210",
                "reference_scope": "synthetic-unit-fixture-not-a-production-reference"
            },
            "column": if source == "forces" { "Fx" } else { "Cd" },
            "window": 1000, "samples": 2000,
            "convergence": {
                "converged": true, "enough_iterations": true,
                "iterations": 2000, "window": 1000,
                "mean_prev": mean, "mean_last": mean,
                "window_diff_pct": 0.0, "drift_pct": 0.0,
                "sd_pct": 100.0 * r.cd_sd / cd, "unsteady": false,
                "residuals": {"p": 1e-6, "U": 1e-6, "k": 1e-6, "omega": 1e-6},
                "residuals_ok": true, "residuals_checked": true
            }
        });
        payload
    }

    fn wp9_fixture_qualified(payload: &Value, reference_verified: bool) -> bool {
        ForceResult::from_json(payload).is_ok_and(|r| benchmark_row_qualified(&r, preset("drivaer_fastback").unwrap(), reference_verified))
    }

    fn wp9_bench_payload(payload: Value) -> Vec<Data> {
        run(bench_info, bench_cook, &[Some(Data::json(payload))], &[]).unwrap()
    }

    #[test]
    fn wp9_replication_presets_use_16ms_without_changing_drag_targets_or_floors() {
        for (id, cd) in [("drivaer_fastback", 0.275), ("drivaer_estate", 0.319), ("drivaer_notchback", 0.277)] {
            let preset = preset(id).unwrap();
            assert_eq!(preset.cd_exp, Some(cd), "published raw-Cd target for {id}");
            assert_eq!(preset.ref_area_m2, 2.16, "frontal area for {id}");
            assert_eq!(preset.ref_length_m, 4.6);
            assert_eq!(preset.band_pct, 5.0);
            assert_eq!(preset.min_cells_million, 12.0);
            assert_eq!(preset.u_ms, 16.0, "approved published-replication speed for {id}");
        }
    }

    #[test]
    fn wp9_replication_gate_accepts_16ms_and_rejects_coherently_normalized_30ms() {
        for (id, cd) in [("drivaer_fastback", 0.275), ("drivaer_estate", 0.319), ("drivaer_notchback", 0.277)] {
            let preset = preset(id).unwrap();
            for (half, cells) in [(true, 12_000_000), (false, 24_000_000)] {
                for source in ["forces", "coefficients"] {
                    // A synthetic schema fixture with an explicit test-only
                    // reference decision; no production mapping is asserted.
                    let payload = wp9_qualified_payload(cd, half, cells, source);
                    let at_16 = ForceResult::from_json(&payload).unwrap();
                    assert_eq!(at_16.rho, 1.184);
                    assert_eq!(at_16.u_ms, 16.0);
                    assert!(benchmark_row_qualified(&at_16, preset, true), "{id}, {source}, half={half}");
                    assert!(!benchmark_row_qualified(&at_16, preset, false));
                    let mut at_30 = at_16.clone();
                    let old_q = at_30.q_pa;
                    at_30.u_ms = 30.0;
                    at_30.q_pa = 0.5 * at_30.rho * at_30.u_ms.powi(2);
                    if source == "forces" {
                        let diagnostics = &mut at_30.qualification.as_mut().unwrap().convergence;
                        diagnostics.mean_prev *= at_30.q_pa / old_q;
                        diagnostics.mean_last *= at_30.q_pa / old_q;
                        let reproduced_cd = diagnostics.mean_last / (at_30.q_pa * at_30.aref_m2);
                        assert!((reproduced_cd - cd).abs() < 1e-12, "30 m/s fixture remains coherently force-normalized");
                    }
                    assert_eq!(at_30.cd, at_16.cd, "speed does not authorize shifting the raw Cd comparison");
                    assert_eq!(at_30.cda, at_16.cda);
                    assert!(
                        !benchmark_row_qualified(&at_30, preset, true),
                        "a coherent 30 m/s solve is outside the approved 16 m/s baseline: {id}, {source}"
                    );
                }
            }
        }
    }

    #[test]
    fn wp9_replication_band_compares_raw_cd_without_a_fitted_offset() {
        for (id, target) in [("drivaer_fastback", 0.275), ("drivaer_estate", 0.319), ("drivaer_notchback", 0.277)] {
            let raw_cd = target * 1.06;
            let payload = wp9_qualified_payload(raw_cd, true, 12_000_000, "coefficients");
            let out = run(bench_info, bench_cook, &[Some(Data::json(payload))], &[("case_id", ParamValue::Str(id.into()))]).unwrap();
            let row = &out[0].as_json().unwrap()["rows"][0];
            assert_eq!(row["cd_sim"], json!(raw_cd));
            assert_eq!(row["cd_exp"], json!(target));
            assert!((row["error_pct"].as_f64().unwrap() - 6.0).abs() < 1e-12);
            assert_eq!(row["band_pct"], json!(5.0));
            assert_eq!(row["coefficient_band_ok"], false, "a 6% raw-Cd error must stay outside the unchanged 5% band");
        }
    }

    // Independently constructed unit DTOs, not measured solver outputs. The
    // geometry identifiers are captured registry entries; all sample/mesh/
    // convergence values below are synthetic consumer-test evidence only.
    fn wp9_typed_reference_unit_result(candidate: &str) -> ForceResult {
        let (fingerprint, cd) = match candidate {
            "drivaer_fastback" => ("4de3bd09d927f4dc", 0.275),
            "drivaer_estate" => ("b1a1d6d4525fcb1c", 0.319),
            "drivaer_notchback" => ("f8399fd496485c02", 0.277),
            _ => panic!("unknown unit candidate"),
        };
        let reference_binding = crate::reference::ReferenceBinding {
            version: 1,
            profile: "road_car_detailed_gs_16ms_v1".into(),
            geometry: crate::reference::GeometryBinding {
                version: 1,
                candidate: candidate.into(),
                preparation_revision: "vehicle-prep-minus-x-half-sink2-mm-v1".into(),
                tri_surface_fingerprint: fingerprint.into(),
            },
            u_ms: 16.0,
            rho: 1.184,
            nu: 1.567e-5,
            full_area_m2: 2.16,
            reference_length_m: 4.6,
            half_model: true,
            ground_mode: "moving".into(),
            wheel_mode: "rotating".into(),
            wheel_omega_rad_s: -51.0,
        };
        ForceResult {
            source: "coefficients".into(),
            u_ms: 16.0,
            rho: 1.184,
            q_pa: 0.5 * 1.184 * 16.0 * 16.0,
            a_front_m2: 2.16,
            aref_m2: 1.08,
            half_model: true,
            cd,
            cd_sd: 0.001,
            cda: cd * 2.16,
            cda_sd: 0.001 * 2.16,
            window: 1000,
            samples: 2000,
            last_iter: 2000.0,
            recipe: Some(Recipe {
                ground_mode: "moving".into(), wheel_mode: "rotating".into(),
                yaw_deg: 0.0, turbulence: "kOmegaSST I=0.250% nut/nu=10.0".into(),
                ..Recipe::default()
            }),
            qualification: Some(crate::coeffs::ForceQualification {
                version: 1,
                solve: crate::types::SolveEvidence {
                    version: 1,
                    mesh: crate::types::MeshEvidence {
                        version: 1, cells: 12_000_000,
                        mesh_fingerprint: "0123456789abcdef".into(),
                        max_nonortho: 30.0, max_skew: 1.0,
                    },
                    input_fingerprint: "fedcba9876543210".into(),
                    reference_scope: None,
                    reference_binding: Some(reference_binding),
                },
                column: "Cd".into(),
                window: 1000,
                samples: 2000,
                convergence: crate::coeffs::Convergence {
                    converged: true, enough_iterations: true, iterations: 2000, window: 1000,
                    mean_prev: cd, mean_last: cd, sd_pct: 100.0 * 0.001 / cd,
                    residuals: [("p", 1e-6), ("U", 1e-6), ("k", 1e-6), ("omega", 1e-6)]
                        .into_iter().map(|(field, value)| (field.into(), value)).collect(),
                    residuals_ok: true, residuals_checked: true,
                    ..crate::coeffs::Convergence::default()
                },
            }),
            ..ForceResult::default()
        }
    }

    #[test]
    fn wp9_reference_resolver_accepts_supported_typed_binding_for_each_candidate() {
        for candidate in ["drivaer_fastback", "drivaer_estate", "drivaer_notchback"] {
            let result = wp9_typed_reference_unit_result(candidate);
            assert!(result.qualification.as_ref().unwrap().solve.reference_scope.is_none());
            assert!(benchmark_reference_verified(&result, preset(candidate).unwrap()), "supported typed unit binding: {candidate}");
            let other = if candidate == "drivaer_fastback" { "drivaer_estate" } else { "drivaer_fastback" };
            assert!(!benchmark_reference_verified(&result, preset(other).unwrap()));
        }
    }

    #[test]
    fn wp9_reference_resolver_rejects_legacy_scope_without_typed_evidence() {
        for scope in ["road_car_detailed_gs_16ms_v1", "drivaer_fastback", "verified", ""] {
            let mut result = wp9_typed_reference_unit_result("drivaer_fastback");
            let solve = &mut result.qualification.as_mut().unwrap().solve;
            solve.reference_binding = None;
            solve.reference_scope = Some(scope.into());
            assert!(!benchmark_reference_verified(&result, preset("drivaer_fastback").unwrap()));
        }
        let mut no_proof = wp9_typed_reference_unit_result("drivaer_fastback");
        no_proof.qualification = None;
        assert!(!benchmark_reference_verified(&no_proof, preset("drivaer_fastback").unwrap()));
    }

    #[test]
    fn wp9_reference_resolver_rechecks_binding_versions_profile_and_geometry() {
        for mutation in 0..9 {
            let mut result = wp9_typed_reference_unit_result("drivaer_fastback");
            let binding = result.qualification.as_mut().unwrap().solve.reference_binding.as_mut().unwrap();
            match mutation {
                0 => binding.version = 2,
                1 => binding.profile = "unknown-profile".into(),
                2 => binding.geometry.version = 2,
                3 => binding.geometry.candidate = "drivaer_estate".into(),
                4 => binding.geometry.preparation_revision = "unregistered-preparation".into(),
                5 => binding.geometry.tri_surface_fingerprint = "0123456789abcdef".into(),
                6 => binding.u_ms = 30.0,
                7 => binding.nu = 1.5e-5,
                _ => binding.wheel_omega_rad_s = 51.0,
            }
            assert!(!benchmark_reference_verified(&result, preset("drivaer_fastback").unwrap()), "accepted typed mutation {mutation}");
        }
    }

    #[test]
    fn wp9_reference_resolver_rechecks_current_result_normalization_and_recipe() {
        for mutation in 0..11 {
            let mut result = wp9_typed_reference_unit_result("drivaer_fastback");
            match mutation {
                0 => { result.u_ms = 30.0; result.q_pa = 0.5 * result.rho * 30.0 * 30.0; }
                1 => { result.rho = 1.225; result.q_pa = 0.5 * result.rho * 16.0 * 16.0; }
                2 => result.q_pa *= 2.0,
                3 => { result.a_front_m2 = 2.0; result.aref_m2 = 1.0; result.cda = result.cd * 2.0; }
                4 => { result.half_model = false; result.aref_m2 = result.a_front_m2; }
                5 => result.recipe = None,
                6 => result.recipe.as_mut().unwrap().ground_mode = "stationary".into(),
                7 => result.recipe.as_mut().unwrap().wheel_mode = "static".into(),
                8 => result.recipe.as_mut().unwrap().yaw_deg = 1.0,
                9 => result.recipe.as_mut().unwrap().turbulence = "kOmegaSSTDDES I=0.250% nut/nu=10.0".into(),
                _ => result.placeholder = true,
            }
            assert!(!benchmark_reference_verified(&result, preset("drivaer_fastback").unwrap()), "accepted current result mutation {mutation}");
        }
    }

    #[test]
    fn wp9_reference_typed_proof_does_not_require_an_obsolete_scope_label() {
        let result = wp9_typed_reference_unit_result("drivaer_fastback");
        // Explicit unit decision for this pure gate; production obtains its
        // decision only through benchmark_reference_verified.
        let failures = benchmark_row_failures(&result, preset("drivaer_fastback").unwrap(), true);
        assert!(failures.is_empty(), "typed proof must not depend on an unrelated legacy string: {failures:?}");
    }

    #[test]
    fn wp9_reference_typed_binding_reaches_the_benchmark_consumer() {
        for candidate in ["drivaer_fastback", "drivaer_estate", "drivaer_notchback"] {
            let result = wp9_typed_reference_unit_result(candidate);
            let out = run(bench_info, bench_cook, &[Some(Data::json(result.to_json()))], &[("case_id", ParamValue::Str(candidate.into()))]).unwrap();
            let report = out[2].as_report().unwrap();
            assert_eq!(report.metrics[&format!("reference_verified_{candidate}")].value, 1.0);
            assert_eq!(report.metrics["benchmark_validated"].value, 1.0, "synthetic consumer branch: {report:?}");
            assert!(report.passes());
            let table = out[0].as_json().unwrap();
            assert_eq!(table["reference_profile"], "road_car_detailed_gs_16ms_v1");
            assert_eq!(table["reference_kind"], "published_replication");
            assert_eq!(table["original_experiment_authenticated"], false);
        }
    }

    #[test]
    fn wp9_benchmark_recorded_coarse_fastback_is_only_numerically_in_band() {
        let mut payload = wp9_qualified_payload(0.288717, true, 3_756_304, "coefficients");
        payload["qualification"]["solve"]["reference_scope"] = Value::Null;
        payload["qualification"]["convergence"]["converged"] = json!(false);
        payload["qualification"]["convergence"]["residuals"]["omega"] = json!(1e-2);
        payload["qualification"]["convergence"]["residuals_ok"] = json!(false);
        let out = wp9_bench_payload(payload);
        let report = out[2].as_report().unwrap();
        assert_eq!(report.metrics["coefficient_band_ok"].value, 1.0);
        assert_eq!(report.metrics["benchmark_validated"].value, 0.0);
        assert_eq!(report.metrics["within_band"].value, 0.0);
        assert!(!report.passes());
    }

    #[test]
    fn wp9_benchmark_pure_gate_controls_use_measured_half_and_full_cell_floors() {
        for (half, minimum) in [(true, 12_000_000), (false, 24_000_000)] {
            for source in ["forces", "coefficients"] {
                let valid = wp9_qualified_payload(0.275, half, minimum, source);
                assert!(wp9_fixture_qualified(&valid, true), "synthetic positive: half={half}, source={source}");
                assert!(!wp9_fixture_qualified(&valid, false), "unknown mapping cannot pass");
                let mut small = valid.clone();
                small["qualification"]["solve"]["mesh"]["cells"] = json!(minimum - 1);
                small["recipe"] = json!({"base_cells": 100_000_000.0, "dx0_m": 0.01,
                    "levels": "high", "ground_mode": "moving", "wheel_mode": "rotating",
                    "yaw_deg": 0.0, "turbulence": "kOmegaSST", "separated_flow": false});
                assert!(!wp9_fixture_qualified(&small, true), "budget must not replace actual cells");
            }
        }
    }

    #[test]
    fn wp9_benchmark_rejects_qualification_shape_window_and_source_mutations() {
        let valid = wp9_qualified_payload(0.275, true, 12_000_000, "coefficients");
        let changes = [
            ("/qualification", Value::Null),
            ("/qualification/version", json!(2)),
            ("/qualification/solve/version", json!(2)),
            ("/qualification/solve/mesh/version", json!(2)),
            ("/qualification/solve/mesh/cells", json!(0)),
            ("/qualification/solve/mesh/cells", json!(12_000_000.5)),
            ("/qualification/solve/mesh/mesh_fingerprint", json!("short")),
            ("/qualification/solve/input_fingerprint", json!("xxxxxxxxxxxxxxxx")),
            ("/qualification/window", json!(999)),
            ("/qualification/window", json!(1001)),
            ("/window", json!(999)),
            ("/qualification/convergence/window", json!(999)),
            ("/qualification/samples", json!(1999)),
            ("/samples", json!(2001)),
            ("/qualification/convergence/iterations", json!(2001)),
            ("/qualification/column", json!("Fx")),
            ("/qualification/column", json!("auto")),
            ("/source", json!("forces")),
            ("/source", json!("unknown")),
        ];
        for (path, value) in changes {
            let mut payload = valid.clone();
            *payload.pointer_mut(path).unwrap() = value;
            assert!(!wp9_fixture_qualified(&payload, true), "accepted mutation {path}: {payload}");
        }
    }

    #[test]
    fn wp9_benchmark_rechecks_numbers_instead_of_convergence_booleans() {
        let valid = wp9_qualified_payload(0.275, true, 12_000_000, "coefficients");
        let changes = [
            ("/qualification/convergence/residuals/k", json!(1.0001e-5)),
            ("/qualification/convergence/residuals/omega", json!(1.0001e-5)),
            ("/qualification/convergence/residuals/p", json!(-1e-6)),
            ("/qualification/convergence/residuals/U", Value::Null),
            ("/qualification/convergence/window_diff_pct", json!(0.20001)),
            ("/qualification/convergence/drift_pct", json!(0.10001)),
            ("/qualification/convergence/window_diff_pct", json!(-0.1)),
            ("/qualification/convergence/drift_pct", json!(-0.1)),
            ("/qualification/convergence/sd_pct", json!(-0.1)),
            ("/qualification/convergence/mean_last", Value::Null),
            ("/qualification/convergence/converged", json!(false)),
            ("/qualification/convergence/enough_iterations", json!(false)),
            ("/qualification/convergence/residuals_checked", json!(false)),
            ("/qualification/convergence/residuals_ok", json!(false)),
        ];
        for (path, value) in changes {
            let mut payload = valid.clone();
            *payload.pointer_mut(path).unwrap() = value;
            assert!(!wp9_fixture_qualified(&payload, true), "accepted mutation {path}: {payload}");
        }
        for field in ["p", "U", "k", "omega"] {
            let mut payload = valid.clone();
            payload["qualification"]["convergence"]["residuals"].as_object_mut().unwrap().remove(field);
            assert!(!wp9_fixture_qualified(&payload, true), "forged true booleans hid missing {field}");
        }
        let mut extra = valid.clone();
        extra["qualification"]["convergence"]["residuals"]["extra"] = json!(1e-2);
        assert!(!wp9_fixture_qualified(&extra, true), "extra bad residuals cannot be dropped");
    }

    #[test]
    fn wp9_benchmark_rejects_normalization_and_summary_payload_mutations() {
        for source in ["forces", "coefficients"] {
            let valid = wp9_qualified_payload(0.275, true, 12_000_000, source);
            let changes = [
                ("/cd", json!(0.276)),
                ("/cda", json!(0.275)),
                ("/q_pa", json!(123.0)),
                ("/aref_m2", json!(2.16)),
                ("/half_model", json!(false)),
                ("/cd_sd", json!(-0.001)),
                ("/cda_sd", json!(0.5)),
                ("/qualification/convergence/sd_pct", json!(0.0)),
                ("/qualification/convergence/mean_prev", json!(0.1)),
                ("/cd_drift_pct", json!(0.09)),
            ];
            for (path, value) in changes {
                let mut payload = valid.clone();
                *payload.pointer_mut(path).unwrap() = value;
                assert!(!wp9_fixture_qualified(&payload, true), "source={source}: accepted {path}");
            }
            let mut r = ForceResult::from_json(&valid).unwrap();
            for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                r.cd = bad;
                assert!(!benchmark_row_qualified(&r, preset("drivaer_fastback").unwrap(), true));
            }
        }
    }

    #[test]
    fn wp9_benchmark_strict_quality_boundaries_and_inclusive_stationarity_limits() {
        let valid = wp9_qualified_payload(0.275, true, 12_000_000, "coefficients");
        for (key, limit) in [("max_nonortho", 70.0), ("max_skew", 4.0)] {
            for bad in [limit, limit + 1.0, -0.01] {
                let mut payload = valid.clone();
                payload["qualification"]["solve"]["mesh"][key] = json!(bad);
                assert!(!wp9_fixture_qualified(&payload, true), "{key}={bad}");
            }
        }
        let mut boundary = valid;
        boundary["qualification"]["solve"]["mesh"]["max_nonortho"] = json!(69.999);
        boundary["qualification"]["solve"]["mesh"]["max_skew"] = json!(3.999);
        for field in ["p", "U", "k", "omega"] {
            boundary["qualification"]["convergence"]["residuals"][field] = json!(1e-5);
        }
        boundary["qualification"]["convergence"]["drift_pct"] = json!(0.1);
        boundary["cd_drift_pct"] = json!(0.1);
        // Exact means keep the independent window-difference identity satisfied.
        boundary["qualification"]["convergence"]["mean_prev"] = json!(0.275 * (1.0 - 0.002));
        let difference = 100.0 * (0.275_f64 - 0.275 * (1.0 - 0.002)).abs() / 0.275;
        boundary["qualification"]["convergence"]["window_diff_pct"] = json!(difference);
        assert!(difference <= 0.2);
        assert!(wp9_fixture_qualified(&boundary, true), "inclusive approved thresholds: {boundary}");
    }

    #[test]
    fn wp9_benchmark_unknown_reference_cannot_be_manufactured_by_payload_strings() {
        for scope in [Value::Null, json!(""), json!("unknown"), json!("drivaer_fastback"), json!("drivaer_estate")] {
            let mut payload = wp9_qualified_payload(0.275, true, 12_000_000, "coefficients");
            payload["qualification"]["solve"]["reference_scope"] = scope;
            let out = wp9_bench_payload(payload);
            let report = out[2].as_report().unwrap();
            assert_eq!(report.metrics["coefficient_band_ok"].value, 1.0);
            assert_eq!(report.metrics["benchmark_validated"].value, 0.0);
            assert_eq!(report.metrics["within_band"].value, 0.0);
            assert!(!report.passes());
        }
    }

    #[test]
    fn wp9_compare_ab_requires_exact_finite_full_validation_and_a_passing_report() {
        let compare = |report: Report| {
            run(
                ab_info,
                ab_cook,
                &[
                    Some(Data::json(fr(0.320, 0.0015).to_json())),
                    Some(Data::json(fr(0.340, 0.0015).to_json())),
                    Some(Data::report(report)),
                ],
                &[],
            )
            .unwrap()
        };
        for full in [None, Some(0.0), Some(2.0), Some(f64::NAN), Some(f64::INFINITY)] {
            let mut report = Report::new("Benchmark check");
            report.metric("within_band", Metric::new(1.0, "-"));
            report.metric("coefficient_band_ok", Metric::new(1.0, "-"));
            if let Some(value) = full {
                report.metric("benchmark_validated", Metric::new(value, "-"));
            }
            let out = compare(report);
            assert_eq!(out[0].as_json().unwrap()["recipe_validated"], false, "accepted {full:?}");
            assert!(!out[1].as_report().unwrap().passes());
        }
        let mut valid = Report::new("Synthetic full-gate unit fixture");
        valid.metric("benchmark_validated", Metric::new(1.0, "-").with_limit(1.0, false));
        valid.metric("within_band", Metric::new(1.0, "-").with_limit(1.0, false));
        assert_eq!(compare(valid.clone())[0].as_json().unwrap()["recipe_validated"], true);
        let mut error = valid.clone();
        error.note(Severity::Error, "synthetic reference-binding failure");
        assert_eq!(compare(error)[0].as_json().unwrap()["recipe_validated"], false);
        let mut failed_metric = valid;
        failed_metric.metric("mesh_ok", Metric::new(0.0, "-").with_limit(1.0, false));
        assert_eq!(compare(failed_metric)[0].as_json().unwrap()["recipe_validated"], false);
    }
}
