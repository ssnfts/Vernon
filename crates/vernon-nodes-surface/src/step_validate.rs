//! Independent bounded Part 21 syntax and topology checks for Vernon's emitted subset.
//! This is not a complete EXPRESS or ISO conformance validator.
//! Syntax is parsed for every entity; semantic checks cover the emitted planar
//! FACE_SURFACE, PLANE, POLY_LOOP, spline ADVANCED_FACE, EDGE_LOOP,
//! ORIENTED_EDGE, EDGE_CURVE, VERTEX_POINT, shell, BREP and typed shape
//! representation subset. Unrecognized metadata entities receive syntax and
//! reference checks only. Curve/surface references in checked topology must be
//! explicitly supported kinds. Geometric coincidence, intersections, units,
//! header schema semantics and full EXPRESS attribute inheritance are unchecked.
//! Limits: 128 MiB document, one million entities, 64 nested aggregate levels.
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub struct ValidationSummary {
    pub entities: usize,
    pub references: usize,
}
#[derive(Debug, Clone)]
enum Value {
    Ref(u64),
    Number(f64),
    Text,
    Enum(String),
    List(Vec<Value>),
    Omitted,
}
#[derive(Debug)]
struct Entity {
    parts: BTreeMap<String, Vec<Value>>,
}
struct Parser<'a> {
    s: &'a [u8],
    p: usize,
    refs: usize,
}
impl Parser<'_> {
    fn skip(&mut self) -> Result<(), String> {
        loop {
            while self.s.get(self.p).is_some_and(u8::is_ascii_whitespace) {
                self.p += 1;
            }
            if self.s.get(self.p..self.p + 2) != Some(b"/*") {
                return Ok(());
            }
            self.p += 2;
            while self.s.get(self.p..self.p + 2) != Some(b"*/") {
                if self.p >= self.s.len() {
                    return Err("unterminated comment".into());
                }
                self.p += 1;
            }
            self.p += 2;
        }
    }
    fn eat(&mut self, t: &[u8]) -> Result<bool, String> {
        self.skip()?;
        if self.s.get(self.p..self.p + t.len()) == Some(t) {
            self.p += t.len();
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn need(&mut self, t: &[u8]) -> Result<(), String> {
        if self.eat(t)? {
            Ok(())
        } else {
            Err(format!(
                "expected {} at byte {}",
                String::from_utf8_lossy(t),
                self.p
            ))
        }
    }
    fn word(&mut self) -> Result<String, String> {
        self.skip()?;
        let start = self.p;
        while self
            .s
            .get(self.p)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-')
        {
            self.p += 1;
        }
        if start == self.p {
            return Err(format!("expected keyword at byte {}", self.p));
        }
        Ok(String::from_utf8_lossy(&self.s[start..self.p]).into_owned())
    }
    fn id(&mut self) -> Result<u64, String> {
        self.need(b"#")?;
        let start = self.p;
        while self.s.get(self.p).is_some_and(u8::is_ascii_digit) {
            self.p += 1;
        }
        let n = std::str::from_utf8(&self.s[start..self.p])
            .unwrap_or("")
            .parse::<u64>()
            .map_err(|_| "invalid entity id")?;
        if n == 0 {
            Err("entity id zero".into())
        } else {
            Ok(n)
        }
    }
    fn list(&mut self, depth: usize) -> Result<Vec<Value>, String> {
        self.need(b"(")?;
        let mut v = vec![];
        if self.eat(b")")? {
            return Ok(v);
        }
        loop {
            v.push(self.value(depth + 1)?);
            if self.eat(b")")? {
                return Ok(v);
            }
            self.need(b",")?;
        }
    }
    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > 64 {
            return Err("nesting limit 64 exceeded".into());
        }
        self.skip()?;
        match self.s.get(self.p).copied() {
            Some(b'#') => {
                let n = self.id()?;
                self.refs += 1;
                Ok(Value::Ref(n))
            }
            Some(b'(') => Ok(Value::List(self.list(depth)?)),
            Some(b'\'') => {
                self.p += 1;
                loop {
                    match self.s.get(self.p) {
                        None => return Err("unterminated string".into()),
                        Some(b'\'') => {
                            self.p += 1;
                            if self.s.get(self.p) == Some(&b'\'') {
                                self.p += 1;
                            } else {
                                break;
                            }
                        }
                        _ => self.p += 1,
                    }
                }
                Ok(Value::Text)
            }
            Some(b'$' | b'*') => {
                self.p += 1;
                Ok(Value::Omitted)
            }
            Some(b'.') if !self.s.get(self.p + 1).is_some_and(u8::is_ascii_digit) => {
                self.p += 1;
                let start = self.p;
                while self
                    .s
                    .get(self.p)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
                {
                    self.p += 1;
                }
                let e = String::from_utf8_lossy(&self.s[start..self.p]).into_owned();
                self.need(b".")?;
                Ok(Value::Enum(e))
            }
            Some(c) if c.is_ascii_digit() || b"+-.".contains(&c) => {
                let start = self.p;
                while self
                    .s
                    .get(self.p)
                    .is_some_and(|c| c.is_ascii_digit() || b"+-.Ee".contains(c))
                {
                    self.p += 1;
                }
                let n = std::str::from_utf8(&self.s[start..self.p])
                    .unwrap_or("")
                    .parse::<f64>()
                    .map_err(|_| "invalid numeric literal")?;
                if !n.is_finite() {
                    return Err("nonfinite numeric literal".into());
                }
                Ok(Value::Number(n))
            }
            Some(c) if c.is_ascii_alphabetic() => {
                self.word()?;
                Ok(Value::List(self.list(depth)?))
            }
            _ => Err(format!("invalid value at byte {}", self.p)),
        }
    }
}
fn refs(v: &Value, out: &mut Vec<u64>) {
    match v {
        Value::Ref(n) => out.push(*n),
        Value::List(v) => {
            for a in v {
                refs(a, out)
            }
        }
        _ => {}
    }
}
fn reference(v: &Value) -> Result<u64, String> {
    if let Value::Ref(n) = v {
        Ok(*n)
    } else {
        Err("expected entity reference".into())
    }
}
fn list(v: &Value) -> Result<&[Value], String> {
    if let Value::List(v) = v {
        Ok(v)
    } else {
        Err("expected aggregate".into())
    }
}
fn number(v: &Value) -> Result<f64, String> {
    if let Value::Number(n) = v {
        Ok(*n)
    } else {
        Err("expected number".into())
    }
}
fn flag(v: &Value) -> Result<bool, String> {
    match v {
        Value::Enum(s) if s == "T" => Ok(true),
        Value::Enum(s) if s == "F" => Ok(false),
        _ => Err("expected boolean".into()),
    }
}
fn part<'a>(e: &'a Entity, name: &str) -> Result<&'a [Value], String> {
    e.parts
        .get(name)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("expected {name}"))
}
fn arg(a: &[Value], i: usize) -> Result<&Value, String> {
    a.get(i).ok_or_else(|| "missing entity attribute".into())
}
fn kind(es: &BTreeMap<u64, Entity>, v: &Value, kinds: &[&str]) -> Result<u64, String> {
    let id = reference(v)?;
    if es
        .get(&id)
        .is_some_and(|e| kinds.iter().any(|k| e.parts.contains_key(*k)))
    {
        Ok(id)
    } else {
        Err(format!("#{id}: expected {}", kinds.join(" or ")))
    }
}
fn ends(es: &BTreeMap<u64, Entity>, id: u64) -> Result<(u64, u64, u64, bool), String> {
    let a = part(&es[&id], "ORIENTED_EDGE")?;
    let edge = kind(es, arg(a, 3)?, &["EDGE_CURVE"])?;
    let e = part(&es[&edge], "EDGE_CURVE")?;
    let x = kind(es, arg(e, 1)?, &["VERTEX_POINT"])?;
    let y = kind(es, arg(e, 2)?, &["VERTEX_POINT"])?;
    let forward = flag(arg(a, 4)?)?;
    Ok(if forward {
        (x, y, edge, true)
    } else {
        (y, x, edge, false)
    })
}

pub fn validate(content: &str) -> Result<ValidationSummary, String> {
    if content.len() > 128 * 1024 * 1024 {
        return Err("document exceeds 128 MiB limit".into());
    }
    let mut p = Parser {
        s: content.as_bytes(),
        p: 0,
        refs: 0,
    };
    p.need(b"ISO-10303-21;")?;
    p.need(b"HEADER;")?;
    let mut headers = BTreeSet::new();
    while !p.eat(b"ENDSEC;")? {
        let name = p.word()?;
        p.list(0)?;
        p.need(b";")?;
        headers.insert(name);
    }
    for required in ["FILE_DESCRIPTION", "FILE_NAME", "FILE_SCHEMA"] {
        if !headers.contains(required) {
            return Err(format!("missing header {required}"));
        }
    }
    p.need(b"DATA;")?;
    let mut es = BTreeMap::new();
    while !p.eat(b"ENDSEC;")? {
        if es.len() >= 1_000_000 {
            return Err("entity limit exceeded".into());
        }
        let id = p.id()?;
        p.need(b"=")?;
        let complex = p.eat(b"(")?;
        let mut parts = BTreeMap::new();
        loop {
            let name = p.word()?;
            let a = p.list(0)?;
            if parts.insert(name, a).is_some() {
                return Err("duplicate complex entity component".into());
            }
            if !complex || p.eat(b")")? {
                break;
            }
        }
        p.need(b";")?;
        if es.insert(id, Entity { parts }).is_some() {
            return Err(format!("duplicate entity #{id}"));
        }
    }
    p.need(b"END-ISO-10303-21;")?;
    p.skip()?;
    if p.p != p.s.len() {
        return Err("trailing document data".into());
    }
    for (id, e) in &es {
        for a in e.parts.values() {
            for v in a {
                let mut rr = vec![];
                refs(v, &mut rr);
                for r in rr {
                    if !es.contains_key(&r) {
                        return Err(format!("#{id} references missing #{r}"));
                    }
                }
            }
        }
    }
    for (id, e) in &es {
        spline_check(e)?;
        control_points_check(&es, e)?;
        shell_check(&es, e)?;
        for (name, a) in &e.parts {
            match name.as_str() {
                "FACE_SURFACE" | "ADVANCED_FACE" => {
                    let advanced = name == "ADVANCED_FACE";
                    let bounds = list(arg(a, 1)?)?;
                    if bounds.is_empty() {
                        return Err("face has no bounds".into());
                    }
                    for b in bounds {
                        let bid = kind(&es, b, &["FACE_BOUND", "FACE_OUTER_BOUND"])?;
                        let bound = es[&bid].parts.values().next().unwrap();
                        kind(
                            &es,
                            arg(bound, 1)?,
                            if advanced {
                                &["EDGE_LOOP", "VERTEX_LOOP"]
                            } else {
                                &["POLY_LOOP"]
                            },
                        )?;
                        flag(arg(bound, 2)?)?;
                    }
                    kind(
                        &es,
                        arg(a, 2)?,
                        if advanced {
                            &["B_SPLINE_SURFACE", "B_SPLINE_SURFACE_WITH_KNOTS", "PLANE"]
                        } else {
                            &["PLANE"]
                        },
                    )?;
                    flag(arg(a, 3)?)?;
                }
                "POLY_LOOP" => {
                    let points = list(arg(a, 1)?)?;
                    if points.len() < 3 {
                        return Err("polygon loop requires three points".into());
                    }
                    for v in points {
                        kind(&es, v, &["CARTESIAN_POINT"])?;
                    }
                }
                "VERTEX_POINT" => {
                    kind(&es, arg(a, 1)?, &["CARTESIAN_POINT"])?;
                }
                "PLANE" => {
                    kind(&es, arg(a, 1)?, &["AXIS2_PLACEMENT_3D"])?;
                }
                "EDGE_CURVE" => {
                    kind(&es, arg(a, 1)?, &["VERTEX_POINT"])?;
                    kind(&es, arg(a, 2)?, &["VERTEX_POINT"])?;
                    kind(
                        &es,
                        arg(a, 3)?,
                        &["B_SPLINE_CURVE", "B_SPLINE_CURVE_WITH_KNOTS", "LINE"],
                    )?;
                    flag(arg(a, 4)?)?;
                }
                "EDGE_LOOP" => {
                    let edges = list(arg(a, 1)?)?;
                    if edges.is_empty() {
                        return Err("empty edge loop".into());
                    }
                    let mut pairs = vec![];
                    for edge in edges {
                        let eid = kind(&es, edge, &["ORIENTED_EDGE"])?;
                        pairs.push(ends(&es, eid)?);
                    }
                    for i in 0..pairs.len() {
                        if pairs[i].1 != pairs[(i + 1) % pairs.len()].0 {
                            return Err(format!("#{id} broken edge-loop endpoint identity"));
                        }
                    }
                }
                "FACETED_BREP" => {
                    let sid = kind(&es, arg(a, 1)?, &["CLOSED_SHELL"])?;
                    for f in list(arg(part(&es[&sid], "CLOSED_SHELL")?, 1)?)? {
                        kind(&es, f, &["FACE_SURFACE"])?;
                    }
                }
                "MANIFOLD_SOLID_BREP" => {
                    kind(&es, arg(a, 1)?, &["CLOSED_SHELL"])?;
                }
                "SHELL_BASED_SURFACE_MODEL" => {
                    for s in list(arg(a, 1)?)? {
                        kind(&es, s, &["OPEN_SHELL", "CLOSED_SHELL"])?;
                    }
                }
                "ADVANCED_BREP_SHAPE_REPRESENTATION" => {
                    for item in list(arg(a, 1)?)? {
                        kind(
                            &es,
                            item,
                            &["MANIFOLD_SOLID_BREP", "FACETED_BREP", "AXIS2_PLACEMENT_3D"],
                        )?;
                    }
                }
                "FACETED_BREP_SHAPE_REPRESENTATION" => {
                    for item in list(arg(a, 1)?)? {
                        kind(&es, item, &["FACETED_BREP", "AXIS2_PLACEMENT_3D"])?;
                    }
                }
                "MANIFOLD_SURFACE_SHAPE_REPRESENTATION" => {
                    for item in list(arg(a, 1)?)? {
                        kind(
                            &es,
                            item,
                            &["SHELL_BASED_SURFACE_MODEL", "AXIS2_PLACEMENT_3D"],
                        )?;
                    }
                }
                "OPEN_SHELL" | "CLOSED_SHELL" => {
                    let faces = list(arg(a, 1)?)?;
                    if faces.is_empty() {
                        return Err("empty shell".into());
                    }
                    let mut incidence: BTreeMap<u64, Vec<bool>> = BTreeMap::new();
                    for f in faces {
                        let fid = kind(&es, f, &["FACE_SURFACE", "ADVANCED_FACE"])?;
                        if let Some(face) = es[&fid].parts.get("ADVANCED_FACE") {
                            for b in list(arg(face, 1)?)? {
                                let bid = reference(b)?;
                                let ba = es[&bid].parts.values().next().unwrap();
                                let lid = reference(arg(ba, 1)?)?;
                                if let Some(loop_a) = es[&lid].parts.get("EDGE_LOOP") {
                                    for edge in list(arg(loop_a, 1)?)? {
                                        let (_, _, eid, orient) = ends(&es, reference(edge)?)?;
                                        incidence
                                            .entry(eid)
                                            .or_default()
                                            .push(orient ^ !flag(arg(ba, 2)?)?);
                                    }
                                }
                            }
                        }
                    }
                    if name == "CLOSED_SHELL"
                        && incidence.values().any(|v| v.len() != 2 || v[0] == v[1])
                    {
                        return Err(format!(
                            "#{id} closed shell edge incidence/orientation invalid"
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(ValidationSummary {
        entities: es.len(),
        references: p.refs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn doc(data: &str) -> String {
        format!("ISO-10303-21;HEADER;FILE_DESCRIPTION(('test'),'2;1');FILE_NAME('a''b','','',(''),(''),'','');FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));ENDSEC;DATA;{data}ENDSEC;END-ISO-10303-21;")
    }
    #[test]
    fn independent_parser_values() {
        let s=doc("/* comment */#1=(NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));#2=THING('it''s',(1,-2,3.5,1.E-2,(#1)),$,.U.);");
        let v = validate(&s).unwrap();
        assert_eq!(v.entities, 2);
        assert_eq!(v.references, 1);
    }
    #[test]
    fn malformed_documents() {
        for s in [
            doc("#1=X(#2);"),
            doc("#0=X();"),
            doc("#1=X();#1=Y();"),
            doc("#1=X('bad);"),
            doc("#1=X(1E999);"),
            doc("#1=X();").trim_end_matches("END-ISO-10303-21;").into(),
        ] {
            assert!(validate(&s).is_err(), "{s}");
        }
    }
    #[test]
    fn typed_face_rejects_polygon_advanced() {
        assert!(validate(&doc("#1=CARTESIAN_POINT('',(0.,0.,0.));#2=POLY_LOOP('',(#1,#1,#1));#3=FACE_OUTER_BOUND('',#2,.T.);#4=ADVANCED_FACE('',(#3),#1,.T.);")).is_err());
    }
    #[test]
    fn broken_loop_identity() {
        let d="#1=CARTESIAN_POINT('',(0.,0.,0.));#2=VERTEX_POINT('',#1);#3=VERTEX_POINT('',#1);#4=LINE('',#1,#1);#5=EDGE_CURVE('',#2,#3,#4,.T.);#6=ORIENTED_EDGE('',*,*,#5,.T.);#7=EDGE_LOOP('',(#6));";
        assert!(validate(&doc(d)).unwrap_err().contains("endpoint identity"));
    }
}

fn spline_check(e: &Entity) -> Result<(), String> {
    let surface = e.parts.get("B_SPLINE_SURFACE");
    let simple = e.parts.get("B_SPLINE_SURFACE_WITH_KNOTS");
    if let Some(base) = surface.or(simple) {
        let offset = usize::from(surface.is_none());
        let du = number(arg(base, offset)?)?;
        let dv = number(arg(base, offset + 1)?)?;
        let rows = list(arg(base, offset + 2)?)?;
        if rows.is_empty() {
            return Err("empty surface control grid".into());
        }
        let nv = list(&rows[0])?.len();
        for row in rows {
            if list(row)?.len() != nv {
                return Err("ragged surface control grid".into());
            }
        }
        check_degree(du, rows.len())?;
        check_degree(dv, nv)?;
        if let Some(knots) = simple {
            let start = if surface.is_some() { 0 } else { 8 };
            check_knots(arg(knots, start)?, arg(knots, start + 2)?, du, rows.len())?;
            check_knots(arg(knots, start + 1)?, arg(knots, start + 3)?, dv, nv)?;
        }
        if let Some(weights) = e.parts.get("RATIONAL_B_SPLINE_SURFACE") {
            let w = list(arg(weights, 0)?)?;
            if w.len() != rows.len() {
                return Err("surface weight row count mismatch".into());
            }
            for row in w {
                let r = list(row)?;
                if r.len() != nv {
                    return Err("surface weight column count mismatch".into());
                }
                for n in r {
                    if number(n)? <= 0.0 {
                        return Err("nonpositive rational weight".into());
                    }
                }
            }
        }
    }
    let curve = e.parts.get("B_SPLINE_CURVE");
    let simple = e.parts.get("B_SPLINE_CURVE_WITH_KNOTS");
    if let Some(base) = curve.or(simple) {
        let offset = usize::from(curve.is_none());
        let degree = number(arg(base, offset)?)?;
        let count = list(arg(base, offset + 1)?)?.len();
        check_degree(degree, count)?;
        if let Some(knots) = simple {
            let start = if curve.is_some() { 0 } else { 6 };
            check_knots(arg(knots, start)?, arg(knots, start + 1)?, degree, count)?;
        }
        if let Some(weights) = e.parts.get("RATIONAL_B_SPLINE_CURVE") {
            let w = list(arg(weights, 0)?)?;
            if w.len() != count {
                return Err("curve weight count mismatch".into());
            }
            for n in w {
                if number(n)? <= 0.0 {
                    return Err("nonpositive rational weight".into());
                }
            }
        }
    }
    Ok(())
}
fn check_degree(degree: f64, count: usize) -> Result<(), String> {
    if degree < 1.0 || degree.fract() != 0.0 || degree >= count as f64 {
        Err("invalid spline degree/control point count".into())
    } else {
        Ok(())
    }
}
fn check_knots(mult: &Value, knots: &Value, degree: f64, count: usize) -> Result<(), String> {
    let m = list(mult)?;
    let k = list(knots)?;
    if m.len() != k.len() || k.len() < 2 {
        return Err("invalid spline knot/multiplicity counts".into());
    }
    let mut sum = 0.0;
    let mut previous = f64::NEG_INFINITY;
    for (m, k) in m.iter().zip(k) {
        let n = number(m)?;
        let value = number(k)?;
        if n < 1.0 || n.fract() != 0.0 || n > degree + 1.0 || value <= previous {
            return Err("invalid spline knots or multiplicities".into());
        }
        sum += n;
        previous = value;
    }
    if sum != count as f64 + degree + 1.0 {
        return Err("spline multiplicity sum mismatch".into());
    }
    Ok(())
}

// Closed planar shells use exact point identities; advanced shells use exact edge
// identities. Geometry coincidence and general self-intersection are not checked.
fn shell_check(es: &BTreeMap<u64, Entity>, e: &Entity) -> Result<(), String> {
    let Some(faces) = e
        .parts
        .get("CLOSED_SHELL")
        .or_else(|| e.parts.get("OPEN_SHELL"))
    else {
        return Ok(());
    };
    let closed = e.parts.contains_key("CLOSED_SHELL");
    let faces = list(arg(faces, 1)?)?;
    let mut edges: BTreeMap<(u8, u64, u64), Vec<(usize, bool)>> = BTreeMap::new();
    for (i, f) in faces.iter().enumerate() {
        let fid = reference(f)?;
        let face = &es[&fid];
        let Some(a) = face
            .parts
            .get("FACE_SURFACE")
            .or_else(|| face.parts.get("ADVANCED_FACE"))
        else {
            return Err("shell references non-face".into());
        };
        for b in list(arg(a, 1)?)? {
            let bid = reference(b)?;
            let bound = es[&bid].parts.values().next().ok_or("empty bound")?;
            let orientation = flag(arg(bound, 2)?)?;
            let lid = reference(arg(bound, 1)?)?;
            if let Some(poly) = es[&lid].parts.get("POLY_LOOP") {
                let pts = list(arg(poly, 1)?)?;
                for j in 0..pts.len() {
                    let x = reference(&pts[j])?;
                    let y = reference(&pts[(j + 1) % pts.len()])?;
                    if x == y {
                        return Err("collapsed polygon edge".into());
                    }
                    edges
                        .entry((0, x.min(y), x.max(y)))
                        .or_default()
                        .push((i, (x < y) == orientation));
                }
            }
            if let Some(edge_loop) = es[&lid].parts.get("EDGE_LOOP") {
                for edge in list(arg(edge_loop, 1)?)? {
                    let (_, _, eid, o) = ends(es, reference(edge)?)?;
                    edges
                        .entry((1, eid, 0))
                        .or_default()
                        .push((i, o == orientation));
                }
            }
        }
    }
    for use_list in edges.values() {
        if use_list.len() > 2
            || (closed && use_list.len() != 2)
            || (use_list.len() == 2 && use_list[0].1 == use_list[1].1)
        {
            return Err("shell shared edge incidence/orientation invalid".into());
        }
    }
    if !faces.is_empty() {
        let mut seen = BTreeSet::from([0]);
        loop {
            let before = seen.len();
            for uses in edges.values() {
                if uses.iter().any(|(f, _)| seen.contains(f)) {
                    seen.extend(uses.iter().map(|(f, _)| *f));
                }
            }
            if seen.len() == before {
                break;
            }
        }
        if seen.len() != faces.len() {
            return Err("disconnected shell faces".into());
        }
    }
    Ok(())
}

fn control_points_check(es: &BTreeMap<u64, Entity>, e: &Entity) -> Result<(), String> {
    for (k, a) in &e.parts {
        let complex =
            e.parts.contains_key("B_SPLINE_SURFACE") || e.parts.contains_key("B_SPLINE_CURVE");
        let offset = usize::from(!complex);
        match k.as_str() {
            "B_SPLINE_SURFACE" | "B_SPLINE_SURFACE_WITH_KNOTS"
                if !a.is_empty() && (k == "B_SPLINE_SURFACE" || !complex) =>
            {
                for row in list(arg(a, offset + 2)?)? {
                    for cp in list(row)? {
                        kind(es, cp, &["CARTESIAN_POINT"])?;
                    }
                }
            }
            "B_SPLINE_CURVE" | "B_SPLINE_CURVE_WITH_KNOTS"
                if !a.is_empty() && (k == "B_SPLINE_CURVE" || !complex) =>
            {
                for cp in list(arg(a, offset + 1)?)? {
                    kind(es, cp, &["CARTESIAN_POINT"])?;
                }
            }
            "CARTESIAN_POINT" | "DIRECTION" => {
                let coords = list(arg(a, 1)?)?;
                if coords.len() != 3 {
                    return Err("expected three coordinates".into());
                }
                for c in coords {
                    number(c)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}
#[cfg(test)]
mod spline_tests {
    use super::tests::doc;
    use super::*;
    #[test]
    fn independent_simple_spline() {
        let s=doc("#1=CARTESIAN_POINT('',(0.,0.,0.));#2=B_SPLINE_SURFACE_WITH_KNOTS('',1,1,((#1,#1),(#1,#1)),.UNSPECIFIED.,.F.,.F.,.F.,(2,2),(2,2),(0.,1.),(0.,1.),.UNSPECIFIED.);");
        assert!(validate(&s).is_ok());
        assert!(validate(&s.replace("(2,2)", "(1,2)")).is_err());
        assert!(validate(&s.replace("'',1,1,", "'',2,1,")).is_err());
    }
    #[test]
    fn independent_complex_rational() {
        let s=doc("#1=CARTESIAN_POINT('',(0.,0.,0.));#2=(BOUNDED_SURFACE() B_SPLINE_SURFACE(1,1,((#1,#1),(#1,#1)),.UNSPECIFIED.,.F.,.F.,.F.) B_SPLINE_SURFACE_WITH_KNOTS((2,2),(2,2),(0.,1.),(0.,1.),.UNSPECIFIED.) GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_SURFACE(((1.,1.),(1.,1.))) REPRESENTATION_ITEM('') SURFACE());");
        assert!(validate(&s).is_ok());
        assert!(validate(&s.replace("(1.,1.)", "(0.,1.)")).is_err());
    }
    #[test]
    fn truncated_nested_and_comments() {
        for text in ["#1=X((1,2);", "#1=X(/* truncated", "#1=(X() Y();"] {
            assert!(validate(&doc(text)).is_err());
        }
    }
    #[test]
    fn representation_item_kind() {
        assert!(validate(&doc(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));#2=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#1),#1);"
        ))
        .is_err());
    }
}

#[cfg(test)]
mod shell_tests {
    use super::tests::doc;
    use super::*;
    #[test]
    fn closed_polygon_shell_requires_shared_edges() {
        let s=doc("#1=CARTESIAN_POINT('',(0.,0.,0.));#2=CARTESIAN_POINT('',(1.,0.,0.));#3=CARTESIAN_POINT('',(0.,1.,0.));#4=POLY_LOOP('',(#1,#2,#3));#5=FACE_OUTER_BOUND('',#4,.T.);#6=AXIS2_PLACEMENT_3D('',#1,$,$);#7=PLANE('',#6);#8=FACE_SURFACE('',(#5),#7,.T.);#9=CLOSED_SHELL('',(#8));");
        assert!(validate(&s).unwrap_err().contains("incidence"));
        assert!(validate(&s.replace("CLOSED_SHELL", "OPEN_SHELL")).is_ok());
    }
    #[test]
    fn nested_limit() {
        let s = doc(&format!("#1=X({}0{});", "(".repeat(70), ")".repeat(70)));
        assert!(validate(&s).unwrap_err().contains("nesting limit"));
    }
}
