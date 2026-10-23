//! Text and JSON tables: marker lists and scale-bar / reference-length lists.

use serde_json::Value;

/// One scale bar or reference length between two markers.
#[derive(Debug, Clone, PartialEq)]
pub struct BarSpec {
    pub a: String,
    pub b: String,
    /// True length at the reference temperature, mm.
    pub length: f64,
    pub sigma: Option<f64>,
    /// Linear expansion coefficient, 1/K.
    pub alpha: Option<f64>,
    /// Temperature at measurement, degrees C.
    pub temp_c: Option<f64>,
}

/// Split a data line: commas / semicolons / tabs when present (keeping empty
/// fields), otherwise whitespace.
fn fields(line: &str) -> Vec<&str> {
    if line.contains([',', ';', '\t']) {
        line.split([',', ';', '\t']).map(str::trim).collect()
    } else {
        line.split_whitespace().collect()
    }
}

fn data_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#') && !l.starts_with("//"))
}

fn num(field: &str, line: usize, what: &str) -> Result<f64, String> {
    field
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("line {line}: '{field}' is not a number ({what})"))
}

fn opt_num(f: Option<&&str>, line: usize, what: &str) -> Result<Option<f64>, String> {
    match f.copied() {
        None | Some("") => Ok(None),
        Some(s) => num(s, line, what).map(Some),
    }
}

/// Policy for repeated marker ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicatePolicy {
    Error,
    KeepFirst,
    KeepLast,
}

/// Parsed marker list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MarkerList {
    pub ids: Vec<String>,
    /// Coordinates in the file's own unit (not yet converted).
    pub coords: Vec<[f64; 3]>,
    pub sigma: Vec<Option<f64>>,
}

/// Parse `id, x, y, z [, sigma]` rows. A first row whose x field is not a
/// number is taken as a header and skipped.
pub fn parse_marker_list(text: &str, policy: DuplicatePolicy) -> Result<MarkerList, String> {
    let mut out = MarkerList::default();
    let mut first = true;
    for (ln, line) in data_lines(text) {
        let f = fields(line);
        if first {
            first = false;
            if f.len() >= 2 && f[1].parse::<f64>().is_err() {
                continue;
            }
        }
        if f.len() < 4 {
            return Err(format!("line {ln}: expected 'id, x, y, z [, sigma]'"));
        }
        let id = f[0].to_string();
        if id.is_empty() {
            return Err(format!("line {ln}: empty marker id"));
        }
        let c = [num(f[1], ln, "x")?, num(f[2], ln, "y")?, num(f[3], ln, "z")?];
        let sigma = opt_num(f.get(4), ln, "sigma")?;
        if let Some(pos) = out.ids.iter().position(|x| *x == id) {
            match policy {
                DuplicatePolicy::Error => {
                    return Err(format!("line {ln}: duplicate marker id '{id}'"));
                }
                DuplicatePolicy::KeepFirst => {}
                DuplicatePolicy::KeepLast => {
                    out.coords[pos] = c;
                    out.sigma[pos] = sigma;
                }
            }
            continue;
        }
        out.ids.push(id);
        out.coords.push(c);
        out.sigma.push(sigma);
    }
    if out.ids.is_empty() {
        return Err("no marker rows found".into());
    }
    Ok(out)
}

/// Parse `a, b, length [, sigma [, alpha [, temp_c]]]` rows.
pub fn parse_bar_list(text: &str) -> Result<Vec<BarSpec>, String> {
    let mut out = Vec::new();
    let mut first = true;
    for (ln, line) in data_lines(text) {
        let f = fields(line);
        if first {
            first = false;
            if f.len() >= 3 && f[2].parse::<f64>().is_err() {
                continue;
            }
        }
        if f.len() < 3 {
            return Err(format!("line {ln}: expected 'id_a, id_b, length [, sigma, alpha, temp_c]'"));
        }
        out.push(BarSpec {
            a: f[0].to_string(),
            b: f[1].to_string(),
            length: num(f[2], ln, "length")?,
            sigma: opt_num(f.get(3), ln, "sigma")?,
            alpha: opt_num(f.get(4), ln, "alpha")?,
            temp_c: opt_num(f.get(5), ln, "temp_c")?,
        });
    }
    Ok(out)
}

fn json_id(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Parse an array of objects `{a, b, length, sigma?, alpha?, temp_c?}`
/// (`length_mm` and `T` are accepted aliases).
pub fn bars_from_json(v: &Value) -> Result<Vec<BarSpec>, String> {
    let arr = v
        .as_array()
        .or_else(|| v.get("bars").and_then(Value::as_array))
        .or_else(|| v.get("lengths").and_then(Value::as_array))
        .ok_or("expected a JSON array of {a, b, length} objects")?;
    arr.iter()
        .enumerate()
        .map(|(i, o)| {
            let get = |keys: &[&str]| keys.iter().find_map(|k| o.get(*k));
            let a = get(&["a", "id_a"]).and_then(json_id);
            let b = get(&["b", "id_b"]).and_then(json_id);
            let length = get(&["length", "length_mm", "L"]).and_then(Value::as_f64);
            match (a, b, length) {
                (Some(a), Some(b), Some(length)) => Ok(BarSpec {
                    a,
                    b,
                    length,
                    sigma: get(&["sigma", "sigma_mm"]).and_then(Value::as_f64),
                    alpha: get(&["alpha"]).and_then(Value::as_f64),
                    temp_c: get(&["temp_c", "T"]).and_then(Value::as_f64),
                }),
                _ => Err(format!("row {i}: needs 'a', 'b' and a numeric 'length'")),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn marker_csv_with_header_comments_and_duplicates() {
        let text = "# survey\nid,x,y,z,sigma\nA,0,0,0,0.02\nB;1000;0;0\nC 0 500 0 0.05\n";
        let m = parse_marker_list(text, DuplicatePolicy::Error).unwrap();
        assert_eq!(m.ids, ["A", "B", "C"]);
        assert_eq!(m.coords[2], [0.0, 500.0, 0.0]);
        assert_eq!(m.sigma, [Some(0.02), None, Some(0.05)]);
        let dup = "A,0,0,0\nA,1,1,1\n";
        assert!(parse_marker_list(dup, DuplicatePolicy::Error).is_err());
        assert_eq!(parse_marker_list(dup, DuplicatePolicy::KeepFirst).unwrap().coords[0], [0.0; 3]);
        assert_eq!(parse_marker_list(dup, DuplicatePolicy::KeepLast).unwrap().coords[0], [1.0; 3]);
        assert!(parse_marker_list("A,0,x,0\n", DuplicatePolicy::Error).is_err());
        assert!(parse_marker_list("# nothing\n", DuplicatePolicy::Error).is_err());
    }

    #[test]
    fn bar_csv_and_json_agree() {
        let csv = "a,b,length,sigma,alpha,temp_c\n1,2,1000,0.01,1.2e-6,25\n2,3,500\n";
        let bars = parse_bar_list(csv).unwrap();
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].alpha, Some(1.2e-6));
        assert_eq!(bars[1].sigma, None);
        let js = json!([{"a": 1, "b": "2", "length": 1000.0, "sigma": 0.01, "alpha": 1.2e-6, "temp_c": 25.0},
                         {"a": 2, "b": 3, "length_mm": 500.0}]);
        assert_eq!(bars_from_json(&js).unwrap(), bars);
        assert!(bars_from_json(&json!([{"a": 1}])).is_err());
        assert!(bars_from_json(&json!("x")).is_err());
    }
}
