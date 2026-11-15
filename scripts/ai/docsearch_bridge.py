"""Vernon <-> document search bridge (BAAI/bge-m3 dense embeddings). Runs inside the AI-3D WSL environment.

Usage: python docsearch_bridge.py --request request.json
Reads the request, splits the listed .md/.txt/.pdf files into about 800 character chunks,
embeds them (cached in a sibling folder keyed by the corpus hash), embeds the query and
writes the best chunks to result.json in the request's out_dir. stdout carries one JSON
object per line: progress events, then exactly one `result` or `error` event.
Never downloads anything: it is fully offline. PDFs use text extraction only (no OCR).
"""
import argparse
import json
import os
import sys
import time
import traceback
from pathlib import Path

CHUNK = 800
OVERLAP = 100


def emit(event, **kw):
    print(json.dumps({"event": event, **kw}), flush=True)


def fail(code, message):
    emit("error", code=code, message=message)
    sys.exit(2)


def linux_path(v):
    if isinstance(v, str) and len(v) > 2 and v[1] == ":" and v[2] in "\\/":
        return "/mnt/" + v[0].lower() + "/" + v[3:].replace("\\", "/")
    return v


def chunk_text(text):
    """Split into about CHUNK characters on paragraph boundaries, with OVERLAP characters carried over."""
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    paras = []
    for p in text.split("\n\n"):
        p = p.strip()
        while len(p) > CHUNK:
            cut = p.rfind(" ", CHUNK // 2, CHUNK)
            cut = cut if cut > 0 else CHUNK
            paras.append(p[:cut].strip())
            p = p[cut:].strip()
        if p:
            paras.append(p)
    chunks, cur = [], ""
    for p in paras:
        if cur and len(cur) + 2 + len(p) > CHUNK:
            chunks.append(cur)
            tail = cur[-OVERLAP:]
            sp = tail.find(" ")
            tail = tail[sp + 1:] if 0 <= sp < len(tail) - 1 else tail
            cur = (tail + "\n\n" + p) if len(tail) + 2 + len(p) <= CHUNK + OVERLAP else p
        else:
            cur = (cur + "\n\n" + p) if cur else p
    if cur.strip():
        chunks.append(cur)
    return chunks


def read_file(path):
    if path.suffix.lower() == ".pdf":
        from pypdf import PdfReader
        return "\n\n".join((pg.extract_text() or "") for pg in PdfReader(str(path)).pages)
    return path.read_text(encoding="utf-8", errors="replace")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--request", required=True)
    req = json.loads(Path(linux_path(ap.parse_args().request)).read_text(encoding="utf-8"))
    root = Path(linux_path(req["ai_root"]))
    weights = root / "DocSearch"
    model_dir = weights / "bge-m3"
    out = Path(linux_path(req["out_dir"]))
    out.mkdir(parents=True, exist_ok=True)
    docs = Path(linux_path(req["docs_folder"]))
    files = list(req["files"])
    query = str(req["query"]).strip()
    top_k = int(req["top_k"])
    corpus = str(req["corpus_hash"])
    if not query:
        fail("empty_query", "the query is empty")
    if not docs.is_dir():
        fail("docs_missing", f"documents folder not found: {docs}")
    if not model_dir.is_dir():
        fail("weights_missing", f"model folder not found: {model_dir}")
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    os.environ["HF_HOME"] = str(weights)
    emit("progress", stage="import", fraction=0.05, message="importing torch and sentence-transformers")
    try:
        import numpy as np
        import torch
        from sentence_transformers import SentenceTransformer
    except Exception as e:  # noqa
        fail("import_failed", f"{type(e).__name__}: {e}")
    device = "cuda" if torch.cuda.is_available() else "cpu"
    emit("progress", stage="load", fraction=0.15, message=f"loading bge-m3 on {device}")
    t0 = time.time()
    try:
        model = SentenceTransformer(str(model_dir), device=device)
        if device == "cuda":
            model.half()
        model.max_seq_length = 512
    except Exception as e:  # noqa
        fail("load_failed", f"{type(e).__name__}: {e}")
    t_load = time.time() - t0

    cache = out.parent / f"docsearch_cache_{corpus}"
    emb_path, meta_path = cache / "embeddings.npy", cache / "chunks.json"
    t1 = time.time()
    cached = emb_path.is_file() and meta_path.is_file()
    skipped = []
    if cached:
        emit("progress", stage="index", fraction=0.5, message="using cached document embeddings")
        chunks = json.loads(meta_path.read_text(encoding="utf-8"))
        emb = np.load(str(emb_path))
        skipped = chunks.get("skipped", []) if isinstance(chunks, dict) else []
        chunks = chunks["chunks"]
    else:
        chunks = []
        for i, rel in enumerate(files):
            try:
                text = read_file(docs / rel)
            except Exception as e:  # noqa
                skipped.append({"file": rel, "reason": f"{type(e).__name__}: {e}"})
                continue
            parts = chunk_text(text)
            if not parts:
                skipped.append({"file": rel, "reason": "no extractable text"})
            for j, c in enumerate(parts):
                chunks.append({"file": rel, "chunk_index": j, "text": c})
            emit("progress", stage="read", fraction=0.2 + 0.1 * (i + 1) / max(1, len(files)), message=f"read {rel}")
        if not chunks:
            fail("no_text", "no text could be extracted from any file (scanned PDFs need OCR, which is not supported)")
        emit("progress", stage="index", fraction=0.35, message=f"embedding {len(chunks)} chunks")
        try:
            emb = model.encode([c["text"] for c in chunks], batch_size=16, normalize_embeddings=True,
                               convert_to_numpy=True, show_progress_bar=False)
        except Exception as e:  # noqa
            fail("embed_failed", f"{type(e).__name__}: {e}")
        emb = np.asarray(emb, dtype=np.float32)
        cache.mkdir(parents=True, exist_ok=True)
        np.save(str(emb_path), emb)
        meta_path.write_text(json.dumps({"chunks": chunks, "skipped": skipped}), encoding="utf-8")
    t_index = time.time() - t1

    emit("progress", stage="query", fraction=0.9, message="searching")
    t2 = time.time()
    try:
        q = model.encode([query], normalize_embeddings=True, convert_to_numpy=True, show_progress_bar=False)
    except Exception as e:  # noqa
        fail("embed_failed", f"{type(e).__name__}: {e}")
    scores = emb @ np.asarray(q[0], dtype=np.float32)
    order = np.argsort(-scores)[:top_k]
    results = [{"file": chunks[i]["file"], "chunk_index": chunks[i]["chunk_index"],
                "score": float(scores[i]), "text": chunks[i]["text"]} for i in order]
    t_query = time.time() - t2

    rev = ""
    rf = weights / "model-revision.txt"
    if rf.is_file():
        rev = rf.read_text().strip()
    import resource
    peak_vram = torch.cuda.max_memory_allocated() / 1073741824.0 if device == "cuda" else 0.0
    res = {"model": "BAAI/bge-m3", "revision": rev, "device": device, "query": query,
           "results": results, "files_searched": len({c["file"] for c in chunks}), "chunks_searched": len(chunks),
           "skipped": skipped, "embeddings_cached": cached,
           "seconds_load": t_load, "seconds_index": t_index, "seconds_query": t_query,
           "peak_vram_GB": peak_vram,
           "peak_rss_GB": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1048576.0}
    (out / "result.json").write_text(json.dumps(res, indent=2), encoding="utf-8")
    emit("result", **res)


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception as e:  # noqa
        traceback.print_exc(file=sys.stderr)
        fail("exception", f"{type(e).__name__}: {e}")
