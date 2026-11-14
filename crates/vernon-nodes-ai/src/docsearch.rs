//! `geo.ai_document_search`: offline semantic search over a folder of manuals and specs.
//!
//! The embedding model (BAAI/bge-m3, MIT) runs as a separate process
//! (`scripts/ai/docsearch_bridge.py`). Vernon lists and hashes the files itself, so the
//! cache key changes whenever a file is added, removed or edited; the model process
//! returns the best-matching text chunks, which Vernon validates and reports.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use vernon_core::report::{Metric, Severity};
use vernon_core::Report;
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::bridge::{self, BridgeError, Install};
use crate::fnv1a;

pub const DOCSEARCH_BRIDGE: &str = include_str!("../../../scripts/ai/docsearch_bridge.py");
pub const EXTENSIONS: [&str; 3] = ["md", "txt", "pdf"];
pub const MAX_FILES: usize = 2000;
pub const MAX_BYTES: u64 = 200 * 1024 * 1024;

pub fn register(reg: &mut Registry) {
    reg.add(DocumentSearchNode);
}

/// One supported file found in the folder.
#[derive(Debug, Clone, PartialEq)]
pub struct DocFile {
    /// Path relative to the folder, `/` separated.
    pub rel: String,
    pub size: u64,
    pub hash: u64,
}

/// What the folder scan found.
#[derive(Debug, Clone, PartialEq)]
pub struct Corpus {
    pub files: Vec<DocFile>,
    pub total_bytes: u64,
    pub skipped_unsupported: usize,
}

impl Corpus {
    /// Hash of the relative paths, sizes and contents.
    pub fn hash(&self) -> u64 {
        let mut s = String::from("bge-m3-corpus");
        for f in &self.files {
            s.push_str(&format!("\n{}|{}|{:016x}", f.rel, f.size, f.hash));
        }
        fnv1a(s.as_bytes())
    }
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, PathBuf, u64)>, skipped: &mut usize) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let md = std::fs::symlink_metadata(&path)?;
        if md.file_type().is_symlink() {
            *skipped += 1;
        } else if md.is_dir() {
            walk(&path, base, out, skipped)?;
        } else if md.is_file() {
            let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
            if EXTENSIONS.contains(&ext.as_str()) {
                let rel = path.strip_prefix(base).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                out.push((rel, path, md.len()));
            } else {
                *skipped += 1;
            }
        }
    }
    Ok(())
}

/// List the `.md`, `.txt` and `.pdf` files below `folder` (sorted) and hash them.
/// Refuses an empty folder, a folder without supported files, more than
/// [`MAX_FILES`] files and more than [`MAX_BYTES`] in total.
pub fn scan_folder(folder: &Path) -> Result<Corpus, String> {
    if !folder.is_dir() {
        return Err(format!("documents folder not found: {}", folder.display()));
    }
    let mut found = Vec::new();
    let mut skipped = 0usize;
    walk(folder, folder, &mut found, &mut skipped).map_err(|e| format!("cannot read {}: {e}", folder.display()))?;
    if found.is_empty() {
        return Err(if skipped == 0 {
            format!("documents folder is empty: {}", folder.display())
        } else {
            format!("no supported files in {}: only .md, .txt and .pdf are searched ({skipped} other files ignored)", folder.display())
        });
    }
    if found.len() > MAX_FILES {
        return Err(format!("too many files: {} found, the limit is {MAX_FILES}", found.len()));
    }
    let total: u64 = found.iter().map(|f| f.2).sum();
    if total > MAX_BYTES {
        return Err(format!("documents are too large: {:.1} MB, the limit is {} MB", total as f64 / 1048576.0, MAX_BYTES / 1048576));
    }
    let mut files = Vec::with_capacity(found.len());
    for (rel, path, size) in found {
        let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        files.push(DocFile { rel, size, hash: fnv1a(&bytes) });
    }
    Ok(Corpus { files, total_bytes: total, skipped_unsupported: skipped })
}

/// Hash of everything that changes the answer.
pub fn input_key(corpus_hash: u64, query: &str, top_k: i64) -> u64 {
    fnv1a(format!("bge-m3|{corpus_hash:016x}|{top_k}|{query}").as_bytes())
}

pub fn build_request(install: &Install, dir: &Path, folder: &Path, corpus: &Corpus, query: &str, top_k: i64) -> Value {
    json!({
        "ai_root": install.root.to_string_lossy(), "out_dir": dir.to_string_lossy(),
        "docs_folder": folder.to_string_lossy(), "files": corpus.files.iter().map(|f| f.rel.clone()).collect::<Vec<_>>(),
        "corpus_hash": format!("{:016x}", corpus.hash()), "query": query, "top_k": top_k,
    })
}

/// Check the `results` array of a result file and return it with the files it names.
pub fn parse_results(res: &Value, top_k: usize) -> Result<Vec<Value>, String> {
    let arr = res.get("results").and_then(Value::as_array).ok_or("result.json has no 'results' list")?;
    if arr.is_empty() {
        return Err("the search returned no results".into());
    }
    let mut out = Vec::new();
    let mut last = f64::INFINITY;
    for r in arr.iter().take(top_k) {
        let file = r.get("file").and_then(Value::as_str).ok_or("a result has no 'file'")?;
        let text = r.get("text").and_then(Value::as_str).ok_or("a result has no 'text'")?;
        let score = r.get("score").and_then(Value::as_f64).filter(|s| s.is_finite()).ok_or("a result has no valid 'score'")?;
        let idx = r.get("chunk_index").and_then(Value::as_u64).ok_or("a result has no 'chunk_index'")?;
        if score > last + 1e-9 {
            return Err("results are not sorted best first".into());
        }
        last = score;
        out.push(json!({"file": file, "chunk_index": idx, "score": score, "text": text}));
    }
    Ok(out)
}

pub struct DocumentSearchNode;

impl NodeOp for DocumentSearchNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.ai_document_search", "AI Document Search", Stage::Geometry)
            .output(PortSpec::new("results", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("manifest", DataKind::Json))
            .param(ParamSpec::path("ai_root", "AI-3D folder", &[], false).help("Folder holding the DocSearch install. Blank uses the VERNON_AI3D_ROOT environment variable."))
            .param(ParamSpec::string("wsl_distro", "WSL distribution", "AI-3D").help("Linux environment that holds the model's Python. Blank runs Python directly (Linux hosts)."))
            .param(ParamSpec::path("docs_folder", "Documents folder", &[], false).help("Folder of .md, .txt and .pdf files, searched recursively. Other files are ignored. At most 2000 files and 200 MB."))
            .param(ParamSpec::string("query", "Query", "").help("What to look for, in plain words. A question or a phrase both work."))
            .param(ParamSpec::int("top_k", "Results", 5, 1, 20).help("How many text passages to return, best first."))
            .param(ParamSpec::int("timeout_s", "Time limit", 900, 30, 7200).unit("s").help("The model process is stopped after this long."))
            .param(ParamSpec::bool("run", "Run model", false).action().help("Starts the model. Without it the node loads an earlier result for identical inputs, if one exists."))
            .help("Finds the passages of a folder of manuals, datasheets and specs that best match a question, using a local text-embedding model. PDFs are read as text only (no scanned pages). Scores are similarity, not proof that a passage is correct.")
            .emulates("semantic search over a document folder")
            .spec("docs/specs/ai-local/document-search.md")
            .external("DocSearch")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, _ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        search(ctx, p)
    }
}

fn idle(rep: Report, manifest: Value) -> Vec<Data> {
    vec![Data::json(json!({"query": "", "results": []})), Data::report(rep), Data::json(manifest)]
}

/// The body of `cook`; the real-model test calls it directly.
pub fn search(ctx: &CookContext, p: &Params) -> CookResult<Vec<Data>> {
    let query = p.str("query")?.trim().to_string();
    if query.is_empty() {
        return Err(ctx.fail("there is no query: type what to look for in the 'Query' parameter"));
    }
    let folder_s = p.str("docs_folder")?.trim().to_string();
    if folder_s.is_empty() {
        return Err(ctx.fail("no documents folder: fill the 'Documents folder' parameter"));
    }
    let folder = PathBuf::from(&folder_s);
    let top_k = p.int("top_k")?.clamp(1, 20);
    let corpus = scan_folder(&folder).map_err(|e| ctx.fail(e))?;
    let chash = corpus.hash();
    let key = input_key(chash, &query, top_k);
    let dir = ctx.work_dir.join(format!("docsearch_{key:016x}"));
    let result_path = dir.join("result.json");
    let mut rep = Report::new("AI document search");
    if !result_path.is_file() {
        if !p.bool("run")? {
            rep.note(Severity::Info, "not run: switch on 'Run model' to search these documents");
            return Ok(idle(rep, json!({"status": "not_run", "artifact_dir": dir.to_string_lossy(), "files": corpus.files.len()})));
        }
        run_model(ctx, p, &dir, &folder, &corpus, &query, top_k)?;
    }
    let res: Value = serde_json::from_slice(&std::fs::read(&result_path)?).map_err(|e| ctx.fail(format!("unreadable result.json: {e}")))?;
    let hits = parse_results(&res, top_k as usize).map_err(|e| ctx.fail(format!("{}: {e}", result_path.display())))?;
    let files_searched = res.get("files_searched").and_then(Value::as_u64).unwrap_or(corpus.files.len() as u64);
    let chunks_searched = res.get("chunks_searched").and_then(Value::as_u64).unwrap_or(0);
    rep.metric("ai_docsearch_files", Metric::new(files_searched as f64, "files"));
    rep.metric("ai_docsearch_chunks", Metric::new(chunks_searched as f64, "chunks"));
    rep.metric("ai_docsearch_results", Metric::new(hits.len() as f64, "passages"));
    if let Some(s) = hits.first().and_then(|h| h["score"].as_f64()) {
        rep.metric("ai_docsearch_top_score", Metric::new(s, "cosine similarity"));
    }
    for (k, unit) in [("seconds_load", "s"), ("seconds_index", "s"), ("seconds_query", "s"), ("peak_vram_GB", "GB"), ("peak_rss_GB", "GB")] {
        if let Some(v) = res.get(k).and_then(Value::as_f64) {
            rep.metric(format!("ai_docsearch_{}", k.to_lowercase()), Metric::new(v, unit));
        }
    }
    if let Some(sk) = res.get("skipped").and_then(Value::as_array).filter(|a| !a.is_empty()) {
        let names: Vec<&str> = sk.iter().filter_map(|s| s.get("file").and_then(Value::as_str)).collect();
        rep.note(Severity::Warning, format!("{} file(s) gave no text and were not searched: {}", sk.len(), names.join(", ")));
    }
    rep.note(
        Severity::Info,
        format!(
            "searched {files_searched} files ({chunks_searched} passages) and returned {}; scores are similarity, not correctness: check the source text",
            hits.len()
        ),
    );
    let results = json!({"query": query, "results": hits});
    let manifest = json!({
        "status": "complete", "artifact_dir": dir.to_string_lossy(), "model": res.get("model"), "revision": res.get("revision"),
        "device": res.get("device"), "docs_folder": folder_s, "query": query, "top_k": top_k,
        "files": corpus.files.len(), "files_searched": files_searched, "chunks_searched": chunks_searched,
        "corpus_bytes": corpus.total_bytes, "ignored_files": corpus.skipped_unsupported, "corpus_hash": format!("{chash:016x}"),
        "embeddings_cached": res.get("embeddings_cached"), "skipped": res.get("skipped"),
        "seconds_load": res.get("seconds_load"), "seconds_index": res.get("seconds_index"), "seconds_query": res.get("seconds_query"),
        "peak_vram_GB": res.get("peak_vram_GB"), "peak_rss_GB": res.get("peak_rss_GB"),
    });
    let _ = std::fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&manifest).unwrap_or_default());
    Ok(vec![Data::json(results), Data::report(rep), Data::json(manifest)])
}

fn run_model(ctx: &CookContext, p: &Params, dir: &Path, folder: &Path, corpus: &Corpus, query: &str, top_k: i64) -> CookResult<()> {
    let install = Install::resolve(p.str("ai_root")?, p.str("wsl_distro")?).map_err(|e| ctx.fail(e.to_string()))?;
    install.require_model_dir("DocSearch").map_err(|e| ctx.fail(e.to_string()))?;
    let request = build_request(&install, dir, folder, corpus, query, top_k);
    let timeout = Duration::from_secs(p.int("timeout_s")?.max(30) as u64);
    bridge::run_job(&install, "DocSearch", "docsearch_bridge.py", DOCSEARCH_BRIDGE, &request, dir, timeout).map_err(|e| match e {
        BridgeError::Cancelled => CookError::Other(vernon_graph::CANCELLED.into()),
        other => ctx.fail(other.to_string()),
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "docsearch_tests.rs"]
mod tests;
