//! Tuft analysis numerics: reading tuft angle series (CSV or JSON), classifying each tuft,
//! mapping the classes onto a surface and summarising them per region.
//!
//! A tuft is a short yarn taped to the body. Its angle against the local flow direction says
//! whether the flow there is attached (aligned and steady), unsteady (aligned but fluttering),
//! separated (off the flow or wildly fluttering) or reversed (pointing back upstream). Nothing
//! here finds tufts in photographs; the angles are measured elsewhere and supplied.

use std::collections::{BTreeMap, HashMap};

use glam::DVec3;
use serde_json::Value;
use vernon_core::Geometry;

use crate::cp::{circular_stats_deg, classify_tuft, closest_on_triangle, TuftClass, TuftThresholds};

/// One tuft: where it sits (mm), the flow direction there and its measured angles (degrees).
#[derive(Debug, Clone, PartialEq)]
pub struct Tuft {
    pub id: String,
    pub pos: DVec3,
    pub flow_deg: f64,
    pub angles: Vec<f64>,
}

/// Tufts read from a file, plus notes about anything odd in it.
#[derive(Debug, Clone, Default)]
pub struct Parsed {
    pub tufts: Vec<Tuft>,
    pub notes: Vec<String>,
}

/// Read a CSV of tuft samples. The first row names the columns (case-insensitive):
/// `x`, `y`, `z` (mm) and `angle_deg` are required; `id` and `flow_deg` (the local flow
/// direction in the same frame as the angles, default 0) are optional. One row per tuft per
/// frame: rows with the same `id` (or, without an id column, the same position) are the
/// samples of one tuft. `#` lines and blank lines are skipped.
pub fn parse_csv(text: &str) -> Result<Parsed, String> {
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim_start_matches('\u{feff}').trim()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'));
    let (_, header) = lines.next().ok_or("the tuft CSV is empty")?;
    let names: Vec<String> = header.split(',').map(|s| s.trim().to_ascii_lowercase()).collect();
    let find = |keys: &[&str]| names.iter().position(|n| keys.contains(&n.as_str()));
    let need = |what: &str, keys: &[&str]| {
        find(keys).ok_or_else(|| format!("the tuft CSV has no {what} column (found: {})", names.join(", ")))
    };
    let (cx, cy, cz) = (need("x (mm)", &["x", "x_mm"])?, need("y (mm)", &["y", "y_mm"])?, need("z (mm)", &["z", "z_mm"])?);
    let ca = need("angle_deg", &["angle_deg", "angle", "theta_deg", "angles_deg"])?;
    let cflow = find(&["flow_deg", "flow"]);
    let cid = find(&["id", "tuft", "tuft_id", "name"]);

    let mut tufts: Vec<Tuft> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut moved = 0usize;
    for (n, line) in lines {
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        let num = |c: usize, what: &str| -> Result<f64, String> {
            f.get(c)
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("line {n}: {what} is missing or not a finite number"))
        };
        let pos = DVec3::new(num(cx, "x")?, num(cy, "y")?, num(cz, "z")?);
        let angle = num(ca, "angle_deg")?;
        let flow = match cflow {
            Some(c) => num(c, "flow_deg")?,
            None => 0.0,
        };
        let id = cid.and_then(|c| f.get(c)).filter(|s| !s.is_empty()).map(|s| (*s).to_string());
        let key = id.clone().unwrap_or_else(|| format!("{:.3},{:.3},{:.3}", pos.x, pos.y, pos.z));
        match index.get(&key) {
            Some(&i) => {
                if tufts[i].pos.distance(pos) > 1.0 {
                    moved += 1;
                }
                tufts[i].angles.push(angle);
            }
            None => {
                index.insert(key, tufts.len());
                tufts.push(Tuft {
                    id: id.unwrap_or_else(|| format!("tuft{}", tufts.len() + 1)),
                    pos,
                    flow_deg: flow,
                    angles: vec![angle],
                });
            }
        }
    }
    if tufts.is_empty() {
        return Err("the tuft CSV has no data rows".into());
    }
    let mut notes = Vec::new();
    if moved > 0 {
        notes.push(format!(
            "{moved} CSV rows repeat a tuft id at a position more than 1 mm from its first row; the first position is used"
        ));
    }
    let single = tufts.iter().filter(|t| t.angles.len() < 2).count();
    if single > 0 {
        notes.push(format!(
            "{single} tufts have a single angle sample, so their flutter (std-dev) cannot be judged: they are classified on the offset from the flow alone"
        ));
    }
    Ok(Parsed { tufts, notes })
}

/// Read tufts from the JSON form `{"tufts": [{id, x, y, z, flow_deg, angles_deg: [...]}]}`
/// (or the bare list). As in the CSV, `x`, `y` and `z` (mm) are required numbers: a tuft with
/// no position is refused, not put at the origin. `flow_deg` may be left out (0); if it is
/// there it must be a number.
pub fn parse_json(v: &Value) -> Result<Parsed, String> {
    let list = v.get("tufts").and_then(Value::as_array).or_else(|| v.as_array()).ok_or("tufts: expected a list")?;
    let tufts = list
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let num = |k: &str| -> Result<f64, String> {
                t.get(k)
                    .and_then(Value::as_f64)
                    .filter(|x| x.is_finite())
                    .ok_or_else(|| format!("tuft {i}: {k} is missing or not a finite number"))
            };
            let angles: Vec<f64> = t
                .get("angles_deg")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("tuft {i}: missing angles_deg"))?
                .iter()
                .filter_map(Value::as_f64)
                .collect();
            if angles.is_empty() {
                return Err(format!("tuft {i}: no angles"));
            }
            if angles.iter().any(|a| !a.is_finite()) {
                return Err(format!("tuft {i}: non-finite angle"));
            }
            let pos = DVec3::new(num("x")?, num("y")?, num("z")?);
            let flow_deg = if t.get("flow_deg").is_some() { num("flow_deg")? } else { 0.0 };
            Ok(Tuft {
                id: t.get("id").and_then(Value::as_str).map(String::from).unwrap_or_else(|| format!("tuft{}", i + 1)),
                pos,
                flow_deg,
                angles,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if tufts.is_empty() {
        return Err("tufts: the list is empty".into());
    }
    Ok(Parsed { tufts, notes: Vec::new() })
}

/// The verdict on one tuft.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Verdict {
    pub class: TuftClass,
    /// Circular mean of the angles, degrees.
    pub mean_deg: f64,
    /// Mean offset from the flow direction, degrees in (-180, 180].
    pub offset_deg: f64,
    /// Circular standard deviation of the angles, degrees.
    pub sd_deg: f64,
}

pub fn judge(t: &Tuft, th: &TuftThresholds) -> Verdict {
    let (class, offset_deg, sd_deg) = classify_tuft(&t.angles, t.flow_deg, th);
    Verdict { class, mean_deg: circular_stats_deg(&t.angles).0, offset_deg, sd_deg }
}

/// Which surface face each tuft sits on and which tuft each face takes its class from.
#[derive(Debug, Clone, Default)]
pub struct SurfaceMap {
    /// Per tuft: nearest face and the distance to it (mm), if within reach.
    pub tuft_face: Vec<Option<(usize, f64)>>,
    /// Per face: nearest tuft and the distance to it (mm), if within reach.
    pub face_tuft: Vec<Option<(usize, f64)>>,
}

/// Match tufts and faces by the distance from the tuft to the face (closest point on its
/// triangles). A face takes the class of its nearest tuft within `reach_mm`; farther faces
/// stay unobserved, so no class is extrapolated across the surface. Ties go to the lowest index.
pub fn map_to_surface(geo: &Geometry, tufts: &[Tuft], reach_mm: f64) -> SurfaceMap {
    let mut best_face = vec![(f64::INFINITY, usize::MAX); tufts.len()];
    let mut best_tuft = vec![(f64::INFINITY, usize::MAX); geo.prims.len()];
    for (pi, t) in geo.triangulated() {
        let (a, b, c) = (geo.points[t[0] as usize], geo.points[t[1] as usize], geo.points[t[2] as usize]);
        for (ti, tuft) in tufts.iter().enumerate() {
            let d = closest_on_triangle(tuft.pos, a, b, c).0.distance(tuft.pos);
            if d < best_face[ti].0 {
                best_face[ti] = (d, pi);
            }
            if d < best_tuft[pi].0 {
                best_tuft[pi] = (d, ti);
            }
        }
    }
    let within = |&(d, i): &(f64, usize)| (i != usize::MAX && d <= reach_mm).then_some((i, d));
    SurfaceMap { tuft_face: best_face.iter().map(within).collect(), face_tuft: best_tuft.iter().map(within).collect() }
}

/// The region label of every face from the attribute `name` (integer, float or string).
/// `None` when the surface has no such attribute.
pub fn face_regions(geo: &Geometry, name: &str) -> Option<Vec<String>> {
    use vernon_core::AttribArray;
    match geo.prim_attribs.get(name)? {
        AttribArray::Int(v) => Some(v.iter().map(|x| x.to_string()).collect()),
        AttribArray::Float(v) => Some(v.iter().map(|x| format!("{}", x.round() as i64)).collect()),
        AttribArray::Str(v) => Some(v.clone()),
        AttribArray::Vec3(_) => None,
    }
}

/// Tuft counts of one region.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RegionStat {
    pub name: String,
    pub tufts: usize,
    pub attached: usize,
    pub unsteady: usize,
    pub separated: usize,
    pub reversed: usize,
}

impl RegionStat {
    /// Share of the region's tufts that are separated (class 1), percent. Reversed tufts are not
    /// counted here; they have their own share, `reversed_pct`.
    pub fn separated_pct(&self) -> f64 {
        self.share_pct(self.separated)
    }

    /// Share of the region's tufts that are reversed (class 2), percent.
    pub fn reversed_pct(&self) -> f64 {
        self.share_pct(self.reversed)
    }

    fn share_pct(&self, count: usize) -> f64 {
        if self.tufts == 0 {
            0.0
        } else {
            100.0 * count as f64 / self.tufts as f64
        }
    }
}

/// Count the verdicts per region label (`None` = the tuft belongs to no region and is left
/// out). Regions come out sorted by name.
pub fn region_stats(labels: &[Option<String>], verdicts: &[Verdict]) -> Vec<RegionStat> {
    let mut by: BTreeMap<String, RegionStat> = BTreeMap::new();
    for (label, v) in labels.iter().zip(verdicts) {
        let Some(name) = label else { continue };
        let s = by.entry(name.clone()).or_insert_with(|| RegionStat { name: name.clone(), ..RegionStat::default() });
        s.tufts += 1;
        match v.class {
            TuftClass::Attached => s.attached += 1,
            TuftClass::Unsteady => s.unsteady += 1,
            TuftClass::Separated => s.separated += 1,
            TuftClass::Reversed => s.reversed += 1,
        }
    }
    by.into_values().collect()
}

/// A region label made safe for a metric name: letters, digits and underscores. Different
/// names can give the same label (`a-b` and `a_b`); use [`metric_labels`] to keep them apart.
pub fn metric_label(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    if s.is_empty() {
        "_".into()
    } else {
        s
    }
}

/// Metric labels for a list of region names, all different. A name whose [`metric_label`] is
/// already taken by an earlier name gets a numeric suffix (`a_b_2`, then `a_b_3`, ...), so no
/// region's metrics overwrite another's. Also returns one warning per name that had to be
/// renamed, naming both regions and the label they share.
pub fn metric_labels(names: &[String]) -> (Vec<String>, Vec<String>) {
    let mut holder: HashMap<String, &str> = HashMap::new();
    let mut labels = Vec::with_capacity(names.len());
    let mut warnings = Vec::new();
    for name in names {
        let base = metric_label(name);
        let mut label = base.clone();
        let mut k = 2;
        while holder.contains_key(&label) {
            label = format!("{base}_{k}");
            k += 1;
        }
        if label != base {
            warnings.push(format!(
                "regions '{}' and '{name}' have the same metric key part '{base}': the metrics of '{name}' are reported under '{label}'",
                holder[&base]
            ));
        }
        holder.insert(label.clone(), name);
        labels.push(label);
    }
    (labels, warnings)
}
