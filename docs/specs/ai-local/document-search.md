# AI Document Search (`geo.ai_document_search`)

Emulates: semantic search over a document folder.

## What it does
Finds the passages of a folder of manuals, datasheets and specs that best match
a question, offline. Vernon lists the `.md`, `.txt` and `.pdf` files below the
folder (recursively), splits each into passages of about 800 characters with 100
characters of overlap (on paragraph boundaries where possible), and the
BAAI/bge-m3 model, run as a separate Python process inside the AI-3D WSL
distribution, embeds the passages and the query. Similarity is the dot product of
normalised dense vectors (cosine). Outputs: `results` (JSON
`{query, results:[{file, chunk_index, score, text}]}`, best first), `report`
(files and passages searched, top score, timings, memory) and `manifest` (JSON
with inputs, corpus hash, model revision and measurements).

Document embeddings are cached in a folder next to the node's work folder, keyed
by the corpus hash, so a second query over the same documents skips indexing.

## What it does not do
- PDFs are read with text extraction only (`pypdf`). There is no OCR: scanned or
  image-only pages give no text, and a file with no text is listed as skipped.
- Dense retrieval only: no keyword index, no re-ranking, no answer writing.
  Scores are similarity, not correctness; read the returned passage.
- Other file types (Word, HTML, images) are ignored.
- Tables and drawings inside PDFs come out as plain text in reading order.
- It never downloads anything while running.

## Licence
Model BAAI/bge-m3: MIT (568M parameters, multilingual, 8192-token context; the
bridge truncates passages at 512 tokens). Python packages `sentence-transformers`
(Apache-2.0), `torch` (BSD-3), `pypdf` (BSD-3), `numpy` (BSD-3).

## Install
`scripts/ai/install_docsearch.sh --yes [weights_dir]`, run as root inside the WSL
distribution. It prints model, licence and size, and refuses to run without
`--yes`. It creates `/opt/ai3d/DocSearch` (Python 3.11), installs the packages,
downloads only what dense retrieval needs (the PyTorch weights `pytorch_model.bin`
about 2.3 GB, tokenizer and config files; no ONNX or other duplicate formats)
into `<weights_dir>/bge-m3` (default `/mnt/e/AI-3D/DocSearch`), and writes
`installed-linux-packages.txt` and `model-revision.txt` there. Re-running reuses
what exists. Measured revision: `5617a9f61b028005a4858fdac845db406aefb181`.

## Inputs
| Parameter | Meaning |
|---|---|
| `ai_root` | AI-3D folder; blank uses `VERNON_AI3D_ROOT`. Must contain `DocSearch`. |
| `wsl_distro` | Default `AI-3D`. Blank runs Python directly (Linux). |
| `docs_folder` | Folder of documents, searched recursively. |
| `query` | What to look for; empty is an error. |
| `top_k` | 1 to 20, default 5. |
| `timeout_s` | Process is stopped after this long (default 900). |
| `run` | Action switch. Without it the node only reloads an earlier result. |

The work folder is `docsearch_<fnv1a hash of corpus, top_k and query>`. The
corpus hash covers each file's relative path, size and contents, so editing,
adding, renaming or removing a file changes it. Identical inputs reload the
saved result without running the model.

Errors are distinct: empty query, folder not set, folder not found, folder empty,
no supported files, too many files, documents too large, missing install, model
error (with the model's own message), timeout, and a result that is empty or not
sorted best first.

## Measured (single run, not a benchmark)
Corpus: this repository's `docs/specs` folder (24 files, 949 passages), RTX 4090
Laptop (16 GB), CUDA with half precision, on a PC whose weights drive was
heavily loaded by other jobs.
- Model revision `5617a9f61b028005a4858fdac845db406aefb181`.
- First query: indexing 4.3 s, query 0.78 s, model load 11.5 s (page cache
  warm; the very first cold load from the USB drive took over an hour while the
  drive was saturated), whole node call 171 s including Python start-up.
- Second query over the same folder: embeddings reused (index 0.02 s), query
  0.57 s, node call 18 s (mostly model load).
- Peak GPU memory 2.6 GB (allocated), peak resident memory 3.4 GB.
- Retrieval quality, 2 queries, judged on the top 3 files:
  - "which STEP schema variants can the exporter write": `step-export.md` is
    first (score 0.573). Correct.
  - "largest number of triangles the aero preview accepts": `ai-models.md` is
    NOT in the top 3 (it is 7th, score 0.487). The top hits (scores 0.49 to
    0.53) are mesh-tool and scanning specs. The spec says "surface path" and
    "20,000 triangles", not "aero preview", so the wording differs; the model
    ranks topic words over the number. This is a miss, not a pass.

## Limits
- At most 2000 files and 200 MB in total; larger folders are refused.
- Indexing time grows with the number of passages; the first run also pays model
  load from the weights drive, which is slow on a USB disk.
- A passage is limited to 512 tokens, so a very dense 800-character passage in a
  non-Latin script may be cut.
- Need about 1.3 GB of GPU memory in half precision; the CPU is used when no GPU
  is visible, and is much slower.
