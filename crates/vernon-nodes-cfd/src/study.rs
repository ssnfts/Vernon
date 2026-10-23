//! Grid-convergence study, baseline-vs-kit comparison and benchmark presets.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use vernon_core::{Metric, Report, Severity};
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::coeffs::{force_in, ForceResult};
use crate::util::{fin, json_in, FnNode};

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
    let ln_ratio = ratio.ln().abs();
    let mut p = ln_ratio / r21.ln();
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

/// Acceptance: the two finest grids within `accept_pct` and the fine-grid GCI
/// within `gci_max_pct`. Without a GCI (oscillatory) the spread of all three
/// values must stay within twice the acceptance level instead.
pub fn mesh_accepted(g: &Gci, accept_pct: f64, gci_max_pct: f64) -> bool {
    let two = last_two_diff_pct(g.values[0], g.values[1]) <= accept_pct;
    let gci_ok = match g.gci_fine_pct {
        Some(v) => v <= gci_max_pct,
        None => g.spread_pct <= 2.0 * accept_pct,
    };
    two && gci_ok
}

fn study_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.mesh_study", "Mesh Convergence", Stage::Cfd)
        .input(PortSpec::optional("fine", DataKind::Json))
        .input(PortSpec::optional("medium", DataKind::Json))
        .input(PortSpec::optional("coarse", DataKind::Json))
        .output(PortSpec::new("study", DataKind::Json))
        .output(PortSpec::new("accepted", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::choice("quantity", "Quantity", "cd", &["cd", "cda", "cl", "cla"]))
        .param(ParamSpec::float("r21", "Refinement ratio medium/fine", 1.3, 1.25, 2.0).help("Linear cell-size ratio; cell count grows by r cubed"))
        .param(ParamSpec::float("r32", "Refinement ratio coarse/medium", 1.3, 1.25, 2.0))
        .param(ParamSpec::float("accept_pct", "Two-finest-grids change", 2.0, 0.5, 5.0).unit("%").help("Accept when the last two grids differ by less than this"))
        .param(ParamSpec::float("gci_max_pct", "Fine-grid GCI limit", 3.0, 0.5, 10.0).unit("%"))
        .param(ParamSpec::float("fine_value", "Fine value (manual)", 0.0, -1.0e6, 1.0e6).help("Used when the fine input is not connected; 0 means not given"))
        .param(ParamSpec::float("medium_value", "Medium value (manual)", 0.0, -1.0e6, 1.0e6))
        .param(ParamSpec::float("coarse_value", "Coarse value (manual)", 0.0, -1.0e6, 1.0e6))
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
        return Ok(GridIn { value, placeholder: r.placeholder, result: Some(r) });
    }
    let manual = p.float(&format!("{name}_value"))?;
    if manual == 0.0 {
        return Err(CookError::MissingInput { index: idx, name: name.into() });
    }
    Ok(GridIn { value: manual, placeholder: false, result: None })
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
    let (accept, gci_max) = (p.float("accept_pct")?, p.float("gci_max_pct")?);
    let g = grid_convergence([grids[0].value, grids[1].value, grids[2].value], p.float("r21")?, p.float("r32")?)
        .map_err(|e| ctx.fail(e))?;
    let accepted = mesh_accepted(&g, accept, gci_max);
    let diff = last_two_diff_pct(g.values[0], g.values[1]);
    rep.metric("data_available", Metric::new(1.0, "-").with_limit(1.0, false))
        .metric("diff_fine_medium_pct", Metric::new(fin(ctx, "difference", diff)?, "%").with_limit(accept, true))
        .metric("spread_pct", Metric::new(g.spread_pct, "%"));
    if let Some(v) = g.gci_fine_pct {
        rep.metric("gci_fine_pct", Metric::new(v, "%").with_limit(gci_max, true));
    }
    if let Some(v) = g.p {
        rep.metric("order_p", Metric::new(v, "-"));
        if !(0.5..=4.0).contains(&v) {
            rep.note(Severity::Warning, format!("observed order {v:.2} is far from the nominal 1 to 2; treat the extrapolation with care"));
        }
    }
    if let Some(v) = g.phi_ext {
        rep.metric("extrapolated", Metric::new(v, "-"));
    }
    rep.metric("mesh_accepted", Metric::new(f64::from(accepted), "-").with_limit(1.0, false));
    if g.oscillatory {
        rep.note(Severity::Warning, "not monotonic: the successive differences change sign or vanish, so no order or extrapolation is reported; use the spread of the three values");
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
pub fn compare(base: &ForceResult, kit: &ForceResult, k_sigma: f64, extra_cda: f64) -> Delta {
    let sigma = |a: f64, b: f64| (a * a + b * b + 2.0 * extra_cda * extra_cda).sqrt();
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
        .output(PortSpec::new("delta", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::float("k_sigma", "Significance level", 2.0, 1.0, 4.0).unit("sigma").help("A change smaller than this many combined standard deviations is inconclusive"))
        .param(ParamSpec::float("extra_noise_cda", "Extra noise per run", 0.0, 0.0, 0.1).unit("m2").help("Added in quadrature to each run's iteration spread, e.g. a mesh-study uncertainty"))
        .help("Kit against baseline on the same recipe: change in CdA, ClA and aero balance with a noise floor from the iteration spread. Runs with different speed, density or half/full treatment are rejected.")
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
    let same = rel_diff(base.u_ms, kit.u_ms) <= 0.01 && rel_diff(base.rho, kit.rho) <= 0.01 && base.half_model == kit.half_model;
    rep.metric("data_available", Metric::new(1.0, "-").with_limit(1.0, false))
        .metric("same_recipe", Metric::new(f64::from(same), "-").with_limit(1.0, false));
    if !same {
        rep.note(Severity::Error, "baseline and kit differ in speed, density or half/full-car treatment: the difference is not attributable to the kit");
    }
    let k = p.float("k_sigma")?;
    let d = compare(&base, &kit, k, p.float("extra_noise_cda")?);
    fin(ctx, "dCdA", d.d_cda)?;
    let significance = if d.sigma_cda > 0.0 { d.d_cda.abs() / d.sigma_cda } else { f64::INFINITY };
    let resolved = d.verdict != "inconclusive";
    rep.metric("CdA_base", Metric::new(base.cda, "m2"))
        .metric("CdA_kit", Metric::new(kit.cda, "m2"))
        .metric("dCdA", Metric::new(d.d_cda, "m2"))
        .metric("dCdA_pct", Metric::new(d.d_cda_pct, "%"))
        .metric("dClA", Metric::new(d.d_cla, "m2"))
        .metric("noise_sigma_cda", Metric::new(d.sigma_cda, "m2"))
        .metric("significance", Metric::new(if significance.is_finite() { significance } else { 1.0e9 }, "sigma"))
        .metric("delta_resolved", Metric::new(f64::from(resolved), "-").with_limit(1.0, false));
    if let Some(b) = d.d_balance_pts {
        rep.metric("balance_shift_pts", Metric::new(b, "%"));
    }
    if !resolved {
        rep.note(Severity::Warning, format!("inconclusive: the change is within {k:.1} combined standard deviations; run longer, refine the mesh or collect more iterations"));
    }
    let out = json!({
        "d_cda": d.d_cda, "d_cda_pct": d.d_cda_pct, "sigma_cda": d.sigma_cda, "d_cla": d.d_cla,
        "sigma_cla": d.sigma_cla, "d_balance_pts": d.d_balance_pts, "verdict": d.verdict,
        "verdict_downforce": d.verdict_downforce, "resolved": resolved, "same_recipe": same,
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

const DRIVAER_NOTE: &str = "fastback, notchback and estate rear ends; moving belt and rotating wheels; half model with symmetry";
const AHMED_NOTE: &str = "slant-back bluff body; stilts carry at least 10% of the drag and must be modelled; steady RANS misses the 25-degree wake transition";

pub const PRESETS: [Preset; 5] = [
    Preset { id: "drivaer_fastback", label: "Road-car body, fastback", family: "drivaer", cd_exp: Some(0.275), band_pct: 5.0, u_ms: 30.0, ref_length_m: 4.6, ref_area_m2: 2.16, slant_share_pct: None, min_cells_million: 12.0, note: DRIVAER_NOTE },
    Preset { id: "drivaer_estate", label: "Road-car body, estate", family: "drivaer", cd_exp: Some(0.319), band_pct: 5.0, u_ms: 30.0, ref_length_m: 4.6, ref_area_m2: 2.16, slant_share_pct: None, min_cells_million: 12.0, note: DRIVAER_NOTE },
    Preset { id: "drivaer_notchback", label: "Road-car body, notchback", family: "drivaer", cd_exp: Some(0.277), band_pct: 5.0, u_ms: 30.0, ref_length_m: 4.6, ref_area_m2: 2.16, slant_share_pct: None, min_cells_million: 12.0, note: DRIVAER_NOTE },
    Preset { id: "ahmed_25", label: "Slant body, 25 degrees", family: "ahmed", cd_exp: None, band_pct: 10.0, u_ms: 40.0, ref_length_m: 1.044, ref_area_m2: 0.112, slant_share_pct: Some(55.0), min_cells_million: 0.5, note: AHMED_NOTE },
    Preset { id: "ahmed_35", label: "Slant body, 35 degrees", family: "ahmed", cd_exp: None, band_pct: 10.0, u_ms: 40.0, ref_length_m: 1.044, ref_area_m2: 0.112, slant_share_pct: Some(35.0), min_cells_million: 0.5, note: AHMED_NOTE },
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
        .param(ParamSpec::float("band_pct", "Allowed drag error", 5.0, 1.0, 15.0).unit("%").help("The recipe is trusted for kit runs only inside this band"))
        .param(ParamSpec::float("slant_share_pct", "Slant pressure share of drag", 0.0, 0.0, 100.0).unit("%").help("Slant-body cases, from the solver run; 0 = not given"))
        .param(ParamSpec::float("share_band_pts", "Share tolerance", 10.0, 1.0, 30.0).unit("%"))
        .help("Compares a benchmark run with the published wind-tunnel drag of the road-car body family (and the slant-body trends) and reports whether the recipe is within its error band. Kit runs should stay disabled until this passes.")
        .emulates("automotive wind-tunnel service: validation against reference data")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

fn opt_force(ins: &[Option<Data>], idx: usize, name: &str) -> CookResult<Option<ForceResult>> {
    json_in(ins, idx)?.map(|_| force_in(ins, idx, name)).transpose()
}

fn bench_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let case = p.str("case_id")?;
    let band = p.float("band_pct")?;
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
        rep.metric("within_band", Metric::new(0.0, "-").with_limit(1.0, false));
        rep.note(
            Severity::Warning,
            "white-box placeholder: runs the benchmark template through the solver chain and compares its drag with the published wind-tunnel value; connect the solved benchmark result",
        );
        return Ok(vec![Data::json(json!({"rows": [], "placeholder": true})), Data::json(preset_json), Data::report(rep)]);
    }
    let mut rows = Vec::new();
    let mut all_in_band = true;
    let mut cds = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        let (pr, r) = (preset(id).ok_or_else(|| ctx.fail("unknown benchmark"))?, results[i].as_ref().ok_or_else(|| ctx.fail("missing result"))?);
        cds.push(r.cd);
        let (err, ok) = match pr.cd_exp {
            Some(exp) => {
                let e = error_pct(r.cd, exp);
                (Some(e), e.abs() <= band)
            }
            None => (None, true),
        };
        all_in_band &= ok;
        if let Some(e) = err {
            rep.metric(format!("cd_error_pct_{id}"), Metric::new(e.abs(), "%").with_limit(band, true));
        }
        rows.push(json!({"case": id, "cd_exp": pr.cd_exp, "cd_sim": r.cd, "error_pct": err, "band_pct": band, "pass": ok}));
    }
    if lead.family == "drivaer" {
        if ids.len() == 3 && !drivaer_ordering_ok(cds[0], cds[1], cds[2], 3.0) {
            all_in_band = false;
            rep.note(Severity::Error, "drag ordering is wrong: the estate must be highest, fastback and notchback close together");
        }
        rep.metric("within_band", Metric::new(f64::from(all_in_band), "-").with_limit(1.0, false));
    } else {
        rep.metric("within_band", Metric::new(1.0, "-").with_limit(1.0, false));
        if ids.len() == 2 {
            let ordering = cds[0] > cds[1];
            rep.metric("drag_ordering_ok", Metric::new(f64::from(ordering), "-"));
            if !ordering {
                rep.note(Severity::Warning, "expected higher drag at 25 than at 35 degrees; recorded as a known steady-RANS limitation, tolerance not tightened");
            }
        }
        let share = p.float("slant_share_pct")?;
        if let (Some(want), true) = (lead.slant_share_pct, share > 0.0) {
            let off = (share - want).abs();
            rep.metric("slant_share_error_pts", Metric::new(off, "%"));
            if off > p.float("share_band_pts")? {
                rep.note(Severity::Warning, format!("slant pressure share {share:.0}% against about {want:.0}% expected; steady RANS under-predicts the base-pressure drop"));
            }
        }
        rep.note(Severity::Info, "no absolute drag reference for this case: the trend is recorded, not gated");
    }
    if !all_in_band {
        rep.note(Severity::Warning, "recipe is outside its error band: kit runs should stay disabled");
    }
    Ok(vec![Data::json(json!({"rows": rows, "placeholder": false})), Data::json(preset_json), Data::report(rep)])
}

pub(crate) fn register(reg: &mut Registry) {
    reg.add(FnNode::new(study_info, study_cook));
    reg.add(FnNode::new(ab_info, ab_cook));
    reg.add(FnNode::new(bench_info, bench_cook));
}

#[cfg(test)]
mod tests {
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
        ForceResult { source: "forces".into(), u_ms: 30.0, rho: 1.225, a_front_m2: 2.0, aref_m2: 1.0, half_model: true, cd, cd_sd: sd, cda: cd * 2.0, cda_sd: sd * 2.0, window: 100, samples: 1000, ..ForceResult::default() }
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
    fn mesh_study_manual_values_and_missing_inputs() {
        let manual = [("fine_value", ParamValue::Float(6.063)), ("medium_value", ParamValue::Float(5.972)), ("coarse_value", ParamValue::Float(5.863)), ("r21", ParamValue::Float(1.5)), ("r32", ParamValue::Float(1.333))];
        let out = run(study_info, study_cook, &[], &manual).unwrap();
        assert!((out[0].as_json().unwrap()["p"].as_f64().unwrap() - 1.53).abs() < 0.02);
        assert!(run(study_info, study_cook, &[], &[]).is_err());
        // a placeholder grid fails closed
        let ph = ForceResult { placeholder: true, ..ForceResult::default() };
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
        let out = run(ab_info, ab_cook, &[w(fr(0.320, 0.0015)), w(fr(0.330, 0.0015))], &[]).unwrap();
        let r = out[1].as_report().unwrap();
        assert!(r.passes(), "{r:?}");
        assert_eq!(out[0].as_json().unwrap()["verdict"], "worse");
        assert!((r.metrics["dCdA"].value - 0.02).abs() < 1e-12);
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
    fn benchmark_golden_drivaer_g24() {
        assert!((error_pct(0.2825, 0.275) - 2.727).abs() < 0.01);
        let w = |c: f64| Some(Data::json(fr(c, 0.001).to_json()));
        let set = [("case_id", ParamValue::Str("drivaer_set".into()))];
        let out = run(bench_info, bench_cook, &[w(0.280), w(0.325), w(0.279)], &set).unwrap();
        let r = out[2].as_report().unwrap();
        assert!(r.passes(), "{r:?}");
        assert_eq!(out[0].as_json().unwrap()["rows"].as_array().unwrap().len(), 3);
        // fastback 7% high: outside the band
        let out = run(bench_info, bench_cook, &[w(0.294), w(0.325), w(0.279)], &set).unwrap();
        assert!(!out[2].as_report().unwrap().passes());
        // wrong ordering fails even when each drag is inside the band:
        // fastback and notchback 7% apart (-1.6% and +4.9% from the tunnel values)
        let out = run(bench_info, bench_cook, &[w(0.2705), w(0.325), w(0.2905)], &set).unwrap();
        let r = out[2].as_report().unwrap();
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
        assert!(r.passes() && r.metrics["drag_ordering_ok"].value == 1.0);
        // the known RANS shortfall is recorded, never a failure
        let out = run(bench_info, bench_cook, &[w(0.26), w(0.27)], &ahmed).unwrap();
        let r = out[2].as_report().unwrap();
        assert!(r.passes() && r.notes.iter().any(|(s, _)| *s == Severity::Warning));
        let share = [("case_id", ParamValue::Str("ahmed_25".into())), ("slant_share_pct", ParamValue::Float(30.0))];
        let out = run(bench_info, bench_cook, &[w(0.3)], &share).unwrap();
        assert!(out[2].as_report().unwrap().passes());
        assert_eq!(out[1].as_json().unwrap()["family"], "ahmed");
    }

    #[test]
    fn benchmark_without_result_is_a_placeholder_that_fails_the_gate() {
        let out = run(bench_info, bench_cook, &[], &[]).unwrap();
        let r = out[2].as_report().unwrap();
        assert!(!r.passes());
        assert!(r.notes.iter().any(|(_, t)| t.starts_with("white-box placeholder:")));
        assert_eq!(out[1].as_json().unwrap()["id"], "drivaer_fastback");
        assert_eq!(out[1].as_json().unwrap()["cd_exp"], 0.275);
    }
}
