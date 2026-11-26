//! Seed of a fast-tier transient run: the preserved end state of an earlier
//! run of the same body, so the next hour continues the time-resolved
//! solution instead of repeating the wash-out from a steady start
//! (`docs/research/wp9-fast-tier-plan.md`, section 6).
//!
//! The checkpoint itself (copy, per-file hashes, refusal of links and changed
//! files) is `diagnostic::arms::preserve_checkpoint_in` under
//! `CheckpointSpec::fast_tier`. This module adds what a seed must prove before
//! it is used: the same body, mesh inputs, solver recipe and flow state as the
//! run it seeds, and a label that is not the run's own.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::diagnostic::arms::{preserve_checkpoint_in, verify_checkpoint_in, CheckpointSpec, CHECKPOINT_FIELDS};
use crate::diagnostic::fnv1a_file;

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn hex(h: u64) -> String {
    format!("{h:016x}")
}

/// What a checkpoint must share with the run it seeds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeedIdentity {
    /// `slant` for the generated classic body, else `stl fnv1a64 <hash>`.
    pub body: String,
    pub angle_deg: Option<f64>,
    pub np: i64,
    /// Hash of everything the mesh is built from: body, refinement plan,
    /// domain, reference geometry and mesh settings.
    pub mesh_inputs_fnv1a64: String,
    /// Hash of the solver recipe: schemes, solution controls, turbulence and
    /// transport settings, decomposition, and the control dictionary with its
    /// end time and averaging start blanked (they move with every run).
    pub recipe_fnv1a64: String,
    pub u_ms: f64,
    pub nu: f64,
    pub rho: f64,
    pub delta_t_s: f64,
    pub ctu_s: f64,
}

fn same(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-12 * a.abs().max(b.abs())
}

impl SeedIdentity {
    /// Every way `self` (the checkpoint) differs from `run`; empty when the
    /// checkpoint may seed the run.
    pub fn mismatches(&self, run: &SeedIdentity) -> Vec<String> {
        let mut out = Vec::new();
        if self.body != run.body {
            out.push(format!("body: checkpoint {}, run {}", self.body, run.body));
        }
        let angle_same = match (self.angle_deg, run.angle_deg) {
            (Some(a), Some(b)) => same(a, b),
            (None, None) => true,
            _ => false,
        };
        if !angle_same {
            out.push(format!("angle: checkpoint {:?}, run {:?}", self.angle_deg, run.angle_deg));
        }
        if self.np != run.np {
            out.push(format!("ranks: checkpoint {}, run {}", self.np, run.np));
        }
        if self.mesh_inputs_fnv1a64 != run.mesh_inputs_fnv1a64 {
            out.push(format!("mesh inputs: checkpoint {}, run {}", self.mesh_inputs_fnv1a64, run.mesh_inputs_fnv1a64));
        }
        if self.recipe_fnv1a64 != run.recipe_fnv1a64 {
            out.push(format!("recipe: checkpoint {}, run {}", self.recipe_fnv1a64, run.recipe_fnv1a64));
        }
        for (name, a, b) in [("U", self.u_ms, run.u_ms), ("nu", self.nu, run.nu), ("rho", self.rho, run.rho), ("time step", self.delta_t_s, run.delta_t_s), ("CTU", self.ctu_s, run.ctu_s)] {
            if !same(a, b) {
                out.push(format!("{name}: checkpoint {a}, run {b}"));
            }
        }
        out
    }
}

/// `slant` for the generated body, else the STL's content hash (a path is
/// not an identity: the same file may move, a different file may reuse a name).
pub fn body_key(geometry: &str) -> Result<String, String> {
    if geometry == "slant" {
        return Ok("slant".into());
    }
    let h = fnv1a_file(Path::new(geometry)).map_err(|e| format!("{geometry}: {e}"))?;
    Ok(format!("stl fnv1a64 {}", hex(h)))
}

/// Hash of the mesh inputs, given as one JSON document.
pub fn mesh_inputs_fingerprint(inputs: &Value) -> String {
    hex(fnv1a(serde_json::to_string(inputs).unwrap_or_default().as_bytes()))
}

/// Blank the entries that move with every run: the top-level `endTime` and
/// the averaging `timeStart`.
fn normalise_control(text: &str) -> String {
    text.lines()
        .map(|l| {
            let t = l.trim_start();
            for key in ["endTime", "timeStart"] {
                if t.strip_prefix(key).is_some_and(|r| r.starts_with(char::is_whitespace)) {
                    return format!("{}{key} *;", &l[..l.len() - t.len()]);
                }
            }
            l.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The solver recipe of a case directory (see [`SeedIdentity::recipe_fnv1a64`]).
pub fn recipe_fingerprint(case: &Path) -> Result<String, String> {
    let mut all = String::new();
    for f in ["system/controlDict", "system/fvSchemes", "system/fvSolution", "system/decomposeParDict", "constant/transportProperties", "constant/turbulenceProperties"] {
        let text = std::fs::read_to_string(case.join(f)).map_err(|e| format!("{f}: {e}"))?;
        let text = if f == "system/controlDict" { normalise_control(&text) } else { text };
        all += &format!("== {f}\n{text}\n");
    }
    Ok(hex(fnv1a(all.as_bytes())))
}

/// The provenance a fast-tier checkpoint carries in its manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeedRecord {
    /// Label of the run that wrote this state.
    pub label: String,
    pub identity: SeedIdentity,
    /// Simulated time at which the time-resolved solution left the steady
    /// start (s). Kept through every continuation.
    pub transient_start_s: f64,
    /// Checkpoint time (s), the written time directory.
    pub time_s: f64,
    /// Convective units since `transient_start_s`, cumulative over the chain.
    pub ctu_reached: f64,
    /// Earlier runs this state descends from, oldest first (empty when the
    /// writing run started from the steady solution).
    pub seed_chain: Vec<String>,
    /// Why the writing run ended (`pass`, or the budget stop).
    pub stop: String,
    /// Mesh row of the writing run (cells and quality).
    pub mesh: Value,
}

/// Preserve the latest written time `time_name` of every rank of `case` in
/// the empty directory `to`, with `record` as the manifest's provenance.
pub fn preserve(case: &Path, to: &Path, time_name: &str, ranks: usize, record: &SeedRecord) -> Result<Value, String> {
    let t: f64 = time_name.parse().map_err(|_| format!("not a time: {time_name}"))?;
    if !same(t, record.time_s) {
        return Err(format!("the record's time {} is not the preserved time {time_name}", record.time_s));
    }
    let provenance = serde_json::to_value(record).map_err(|e| e.to_string())?;
    preserve_checkpoint_in(case, to, time_name, &CheckpointSpec::fast_tier(ranks), provenance)
}

/// A verified fast-tier checkpoint.
#[derive(Debug, Clone)]
pub struct Seed {
    pub dir: PathBuf,
    pub time_name: String,
    pub record: SeedRecord,
    /// Per-file hashes from the manifest, relative path to digest.
    files: serde_json::Map<String, Value>,
}

impl Seed {
    /// Re-hash every file of the checkpoint at `dir` (written for `ranks`
    /// ranks) and read its record. Nothing is compared with a run yet.
    pub fn open(dir: &Path, ranks: usize) -> Result<Self, String> {
        let (time_name, doc) = verify_checkpoint_in(dir, &CheckpointSpec::fast_tier(ranks))?;
        let record: SeedRecord = serde_json::from_value(doc["provenance"].clone()).map_err(|e| format!("checkpoint record: {e}"))?;
        let t: f64 = time_name.parse().map_err(|_| format!("not a time: {time_name}"))?;
        if !same(t, record.time_s) {
            return Err(format!("the record's time {} is not the checkpoint's time {time_name}", record.time_s));
        }
        if !record.transient_start_s.is_finite() || record.time_s < record.transient_start_s {
            return Err("the checkpoint lies before its own transient start".into());
        }
        let files = doc["files"].as_object().cloned().ok_or("checkpoint file list missing")?;
        Ok(Self { dir: dir.to_path_buf(), time_name, record, files })
    }

    /// Refuse the seed unless body, angle, ranks, mesh inputs, recipe and flow
    /// state match `run`, and `label` is not this state's own run or ancestor.
    pub fn check(&self, run: &SeedIdentity, label: &str) -> Result<(), String> {
        let mut why = self.record.identity.mismatches(run);
        if self.record.label == label || self.record.seed_chain.iter().any(|l| l == label) {
            why.push(format!("label: {label} already wrote this state or one of its ancestors; give the seeded run its own label"));
        }
        if why.is_empty() {
            Ok(())
        } else {
            Err(format!("checkpoint {} refused: {}", self.dir.display(), why.join("; ")))
        }
    }

    /// Hash of the mesh the checkpoint carries: the reconstructed mesh and
    /// every rank's piece, from the verified per-file hashes.
    pub fn polymesh_fnv1a64(&self) -> String {
        let mut all = String::new();
        for (rel, digest) in &self.files {
            if rel.contains("constant/polyMesh/") {
                all += &format!("{rel} {}\n", digest.as_str().unwrap_or(""));
            }
        }
        hex(fnv1a(all.as_bytes()))
    }

    fn copy_checked(&self, rel: &str, to: &Path) -> Result<(), String> {
        let digest = self.files.get(rel).and_then(Value::as_str).ok_or_else(|| format!("{rel} is not in the checkpoint"))?;
        let dst = to.join(rel);
        std::fs::create_dir_all(dst.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::copy(self.dir.join(rel), &dst).map_err(|e| format!("{rel}: {e}"))?;
        let got = hex(fnv1a_file(&dst).map_err(|e| format!("{rel}: {e}"))?);
        if got != digest {
            return Err(format!("{rel}: the copy does not match the checkpoint"));
        }
        Ok(())
    }

    /// Write the case-level inputs of the checkpoint (settings, reconstructed
    /// mesh, initial fields, manifest) to the empty directory `to`, to be the
    /// mesh input of the seeded run's case writer.
    pub fn restore_mesh_case(&self, to: &Path) -> Result<usize, String> {
        if to.exists() && std::fs::read_dir(to).map_err(|e| e.to_string())?.next().is_some() {
            return Err(format!("{} is not empty", to.display()));
        }
        let mut n = 0;
        for rel in self.files.keys() {
            if rel == "vernon_case.json" || ["system/", "constant/", "0.orig/"].iter().any(|p| rel.starts_with(p)) {
                self.copy_checked(rel, to)?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// Install the checkpoint's decomposed mesh and restart fields into the
    /// freshly written `case`, whose reconstructed mesh must be the
    /// checkpoint's. Averaging state (mean fields and the function-object
    /// state under `uniform/`) is left behind, so the seeded run's averages
    /// cover only its own window. Returns the installed restart files.
    pub fn install(&self, case: &Path) -> Result<Vec<String>, String> {
        if !crate::diagnostic::processor_dirs(case).map_err(|e| e.to_string())?.is_empty() {
            return Err(format!("{} already has processor directories", case.display()));
        }
        let mut mesh_files = 0;
        for (rel, digest) in &self.files {
            if rel.starts_with("constant/polyMesh/") {
                let got = hex(fnv1a_file(&case.join(rel)).map_err(|e| format!("case {rel}: {e}"))?);
                if Some(got.as_str()) != digest.as_str() {
                    return Err(format!("case {rel} is not the checkpoint's mesh"));
                }
                mesh_files += 1;
            }
        }
        if mesh_files == 0 {
            return Err("the checkpoint carries no reconstructed mesh".into());
        }
        let mut installed = Vec::new();
        for n in 0..self.record.identity.np {
            let rank = format!("processor{n}/");
            let restart: Vec<String> = CHECKPOINT_FIELDS.iter().map(|f| format!("{rank}{}/{f}", self.time_name)).chain([format!("{rank}{}/uniform/time", self.time_name)]).collect();
            for rel in self.files.keys() {
                if rel.starts_with(&format!("{rank}constant/polyMesh/")) {
                    self.copy_checked(rel, case)?;
                } else if restart.contains(rel) {
                    self.copy_checked(rel, case)?;
                    installed.push(rel.clone());
                }
            }
        }
        Ok(installed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::arms::CHECKPOINT_FILE;

    const T: &str = "0.4176";

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A finished fast-tier case of `ranks` ranks with two written times.
    fn fake_case(root: &Path, ranks: usize) {
        for n in 0..ranks {
            let rank = root.join(format!("processor{n}"));
            for f in ["points", "faces", "owner", "neighbour", "boundary"] {
                write(&rank.join("constant/polyMesh").join(f), &format!("{n} {f}"));
            }
            for f in CHECKPOINT_FIELDS.iter().chain(&["UMean", "pPrime2Mean"]) {
                write(&rank.join(T).join(f), &format!("{n} {f} at {T}"));
            }
            write(&rank.join(T).join("uniform/time"), "deltaT 0.000261");
            write(&rank.join(T).join("uniform/functionObjects/functionObjectProperties"), "fieldAverage1 state");
            write(&rank.join("0.3915/U"), "an older write is not part of the checkpoint");
        }
        for f in ["points", "faces", "owner", "neighbour", "boundary"] {
            write(&root.join("constant/polyMesh").join(f), &format!("reconstructed {f}"));
        }
        write(&root.join("constant/triSurface/body.stl"), "solid body\nendsolid body\n");
        write(&root.join("system/controlDict"), "endTime         0.4176;\ndeltaT          0.000261;\nfunctions\n{\n    fieldAverage1\n    {\n        timeStart       0.0522;\n    }\n}\n");
        for f in ["system/fvSchemes", "system/fvSolution", "system/decomposeParDict", "constant/transportProperties", "constant/turbulenceProperties"] {
            write(&root.join(f), f);
        }
        write(&root.join("0.orig/U"), "initial U");
        write(&root.join("vernon_case.json"), "{\"flavour\": \"opencfd2606\"}");
        write(&root.join("logs/solver_seg01.log"), "Time = 0.000261\n");
    }

    fn identity(recipe: &str) -> SeedIdentity {
        SeedIdentity {
            body: "slant".into(),
            angle_deg: Some(25.0),
            np: 2,
            mesh_inputs_fnv1a64: mesh_inputs_fingerprint(&serde_json::json!({"dx0_m": 0.092})),
            recipe_fnv1a64: recipe.into(),
            u_ms: 40.0,
            nu: 1.5e-5,
            rho: 1.225,
            delta_t_s: 0.000261,
            ctu_s: 0.0261,
        }
    }

    fn record(case: &Path) -> SeedRecord {
        SeedRecord {
            label: "fast-slant25-a".into(),
            identity: identity(&recipe_fingerprint(case).unwrap()),
            transient_start_s: 0.0,
            time_s: 0.4176,
            ctu_reached: 16.0,
            seed_chain: vec![],
            stop: "budget".into(),
            mesh: serde_json::json!({"cells": 502944.0}),
        }
    }

    fn dirs(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = crate::util::testutil::tmp(name);
        (root.join("case"), root.join("checkpoint"), root)
    }

    #[test]
    fn fast_seed_manifest_round_trips_and_carries_only_the_latest_time() {
        let (case, to, root) = dirs("fast-seed-roundtrip");
        fake_case(&case, 2);
        let rec = record(&case);
        let doc = preserve(&case, &to, T, 2, &rec).unwrap();
        assert_eq!(doc["scope"], "wp9_fast_tier_checkpoint");
        assert_eq!(doc["time_name"], T);
        assert!(to.join(CHECKPOINT_FILE).is_file());
        let seed = Seed::open(&to, 2).unwrap();
        assert_eq!(seed.record, rec, "the record survives the round trip exactly");
        assert_eq!(seed.time_name, T);
        assert!(seed.files.keys().all(|f| !f.contains("/0.3915/")), "only the latest time");
        assert!(seed.files.contains_key("processor1/0.4176/uniform/time"));
        assert!(seed.files.contains_key("constant/polyMesh/points") && seed.files.contains_key("logs/solver_seg01.log"));
        seed.check(&rec.identity, "fast-slant25-b").unwrap();
        // Restored mesh case plus installed ranks reproduce the restart state.
        let mesh_case = root.join("restored");
        assert!(seed.restore_mesh_case(&mesh_case).unwrap() > 10);
        assert_eq!(std::fs::read_to_string(mesh_case.join("constant/polyMesh/points")).unwrap(), "reconstructed points");
        assert!(!mesh_case.join("processor0").exists() && !mesh_case.join("logs").exists());
        let installed = seed.install(&mesh_case).unwrap();
        assert_eq!(installed.len(), 2 * (CHECKPOINT_FIELDS.len() + 1));
        assert!(mesh_case.join("processor1/0.4176/phi").is_file() && mesh_case.join("processor1/constant/polyMesh/owner").is_file());
        assert!(!mesh_case.join("processor0/0.4176/UMean").exists(), "averages are not carried over");
        assert!(!mesh_case.join("processor0/0.4176/uniform/functionObjects").exists(), "nor the averaging state");
        assert!(seed.install(&mesh_case).unwrap_err().contains("already has processor"));
        assert_eq!(seed.polymesh_fnv1a64(), Seed::open(&to, 2).unwrap().polymesh_fnv1a64());
        // The wrong rank count is refused on opening.
        assert!(Seed::open(&to, 4).unwrap_err().contains("expected 4"));
    }

    #[test]
    fn fast_seed_refuses_each_mismatch() {
        let (case, to, _root) = dirs("fast-seed-mismatch");
        fake_case(&case, 2);
        let rec = record(&case);
        preserve(&case, &to, T, 2, &rec).unwrap();
        let seed = Seed::open(&to, 2).unwrap();
        let run = rec.identity.clone();
        type Change = fn(&mut SeedIdentity);
        let changes: [(&str, Change); 10] = [
            ("body", |r| r.body = "stl fnv1a64 0123456789abcdef".into()),
            ("angle", |r| r.angle_deg = Some(35.0)),
            ("ranks", |r| r.np = 16),
            ("mesh inputs", |r| r.mesh_inputs_fnv1a64 = mesh_inputs_fingerprint(&serde_json::json!({"dx0_m": 0.08}))),
            ("recipe", |r| r.recipe_fnv1a64 = "0000000000000000".into()),
            ("U", |r| r.u_ms = 30.0),
            ("nu", |r| r.nu = 1.0e-5),
            ("rho", |r| r.rho = 1.2),
            ("time step", |r| r.delta_t_s = 0.0001305),
            ("CTU", |r| r.ctu_s = 0.0262),
        ];
        for (name, change) in changes {
            let mut other = run.clone();
            change(&mut other);
            let err = seed.check(&other, "fast-slant25-b").unwrap_err();
            assert!(err.contains(&format!("{name}:")), "{name}: {err}");
            assert_eq!(rec.identity.mismatches(&other).len(), 1, "{name} is the only difference");
        }
        assert!(seed.check(&run, "fast-slant25-a").unwrap_err().contains("label"), "a run cannot seed itself");
        // A recipe change in the case files moves the fingerprint; the end
        // time and the averaging start do not.
        let base = recipe_fingerprint(&case).unwrap();
        let control = std::fs::read_to_string(case.join("system/controlDict")).unwrap();
        std::fs::write(case.join("system/controlDict"), control.replace("0.4176;", "0.8352;").replace("0.0522;", "0.4176;")).unwrap();
        assert_eq!(recipe_fingerprint(&case).unwrap(), base);
        std::fs::write(case.join("system/controlDict"), control.replace("0.000261;", "0.0001305;")).unwrap();
        assert_ne!(recipe_fingerprint(&case).unwrap(), base);
        std::fs::write(case.join("system/fvSchemes"), "changed").unwrap();
        assert_ne!(recipe_fingerprint(&case).unwrap(), base);
    }

    #[test]
    fn fast_seed_refuses_a_tampered_or_incomplete_checkpoint() {
        let (case, to, root) = dirs("fast-seed-tamper");
        fake_case(&case, 2);
        let rec = record(&case);
        preserve(&case, &to, T, 2, &rec).unwrap();
        assert!(preserve(&case, &to, T, 2, &rec).unwrap_err().contains("not empty"));
        // The record must name the preserved time.
        let mut wrong = rec.clone();
        wrong.time_s = 0.3915;
        assert!(preserve(&case, &root.join("other"), T, 2, &wrong).is_err());
        // A changed field.
        write(&to.join("processor1/0.4176/U"), "changed");
        assert!(Seed::open(&to, 2).unwrap_err().contains("changed"));
        write(&to.join("processor1/0.4176/U"), &format!("1 U at {T}"));
        Seed::open(&to, 2).unwrap();
        // An extra file, then a missing one.
        write(&to.join("system/extra"), "x");
        assert!(Seed::open(&to, 2).is_err());
        std::fs::remove_file(to.join("system/extra")).unwrap();
        std::fs::remove_file(to.join("processor0/0.4176/nut")).unwrap();
        assert!(Seed::open(&to, 2).unwrap_err().contains("nut"));
        write(&to.join("processor0/0.4176/nut"), &format!("0 nut at {T}"));
        Seed::open(&to, 2).unwrap();
        // A changed record: the provenance is not hashed, but its time must
        // match the checkpoint and the identity is what `check` compares.
        let manifest = to.join(CHECKPOINT_FILE);
        let text = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(&manifest, text.replace("\"time_s\": 0.4176", "\"time_s\": 0.5")).unwrap();
        assert!(Seed::open(&to, 2).unwrap_err().contains("time"));
        std::fs::write(&manifest, text.replace("wp9_fast_tier_checkpoint", "wp9_stage2_checkpoint")).unwrap();
        assert!(Seed::open(&to, 2).unwrap_err().contains("scope"));
        std::fs::write(&manifest, &text).unwrap();
        // The restored case's mesh must be the checkpoint's before ranks go in.
        let seed = Seed::open(&to, 2).unwrap();
        let restored = root.join("restored");
        seed.restore_mesh_case(&restored).unwrap();
        write(&restored.join("constant/polyMesh/points"), "another mesh");
        assert!(seed.install(&restored).unwrap_err().contains("not the checkpoint's mesh"));
        // A missing rank time directory in the source cannot be preserved.
        std::fs::remove_dir_all(case.join("processor1").join(T)).unwrap();
        assert!(preserve(&case, &root.join("third"), T, 2, &rec).is_err());
        // Time names that are not plain numbers are refused outright.
        assert!(crate::diagnostic::arms::checkpoint_files_in(&case, "../0.4176", &CheckpointSpec::fast_tier(2)).unwrap_err().contains("time directory"));
    }
}
