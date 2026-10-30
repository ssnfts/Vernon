//! Bounded validation of an image tree before an external photo process runs.

use std::path::Path;

const MAX_ENTRIES: usize = 32_768;
const MAX_DEPTH: usize = 32;
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "bmp"];

fn check_cancelled() -> Result<(), String> {
    if vernon_core::process::cancel_requested() {
        Err("photo input enumeration was cancelled".into())
    } else {
        Ok(())
    }
}

fn inspect(path: &Path) -> Result<std::fs::Metadata, String> {
    check_cancelled()?;
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("cannot inspect image input {}: {e}", path.display()))?;
    let linked = metadata.file_type().is_symlink();
    #[cfg(windows)]
    let linked = {
        use std::os::windows::fs::MetadataExt;
        // Directory junctions and other reparse points are not all symlinks.
        linked || metadata.file_attributes() & 0x400 != 0
    };
    if linked {
        return Err(format!(
            "image input {} is a link or reparse point; use an ordinary local image tree",
            path.display()
        ));
    }
    Ok(metadata)
}

/// Count supported regular image files after checking the whole input tree.
/// Root depth is zero; all descendant entries, including non-images, consume
/// the shared entry budget. Links are rejected rather than skipped because
/// the external extractor would otherwise still see and follow them.
pub(crate) fn count_images_checked(dir: &Path) -> Result<usize, String> {
    if !inspect(dir)?.is_dir() {
        return Err(format!("image input root {} is not a directory", dir.display()));
    }
    let mut pending = vec![(dir.to_path_buf(), 0usize)];
    let mut entries = 0usize;
    let mut images = 0usize;
    while let Some((directory, depth)) = pending.pop() {
        check_cancelled()?;
        // Inspect again before opening a previously queued directory.
        if !inspect(&directory)?.is_dir() {
            return Err(format!("image input {} is no longer a directory", directory.display()));
        }
        let listing = std::fs::read_dir(&directory)
            .map_err(|e| format!("cannot read image directory {}: {e}", directory.display()))?;
        let mut paths = Vec::new();
        for entry in listing {
            check_cancelled()?;
            let entry = entry.map_err(|e| format!("cannot read image directory entry in {}: {e}", directory.display()))?;
            if entries == MAX_ENTRIES {
                return Err(format!("image input exceeds the {MAX_ENTRIES} entry limit"));
            }
            entries += 1;
            paths.push(entry.path());
        }
        paths.sort();
        let mut directories = Vec::new();
        for path in paths {
            let metadata = inspect(&path)?;
            if metadata.is_dir() {
                if depth == MAX_DEPTH {
                    return Err(format!("image input exceeds the directory depth limit of {MAX_DEPTH}: {}", path.display()));
                }
                directories.push((path, depth + 1));
            } else if metadata.is_file() {
                if path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| {
                    IMAGE_EXTENSIONS.iter().any(|supported| ext.eq_ignore_ascii_case(supported))
                }) {
                    images += 1;
                }
            } else {
                return Err(format!("image input {} is a special file; only regular files and directories are supported", path.display()));
            }
        }
        // A stack reverses order, so queue reverse-sorted children.
        pending.extend(directories.into_iter().rev());
    }
    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn fixture(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "vernon-photo-inputs-{name}-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn counts_nested_regular_images() {
        let dir = fixture("nested");
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("one.jpg"), []).unwrap();
        std::fs::write(dir.join("a/two.png"), []).unwrap();
        std::fs::write(dir.join("a/b/three.jpeg"), []).unwrap();
        std::fs::write(dir.join("a/b/notes.txt"), []).unwrap();
        assert_eq!(count_images_checked(&dir).unwrap(), 3);
    }

    #[test]
    fn extensions_are_case_insensitive_and_exact() {
        let dir = fixture("extensions");
        for file in ["a.JPG", "b.jPeG", "c.PNG", "d.TIF", "e.TIFF", "f.BMP", "g.jpg.bak", "h.webp", "no_extension"] {
            std::fs::write(dir.join(file), []).unwrap();
        }
        assert_eq!(count_images_checked(&dir).unwrap(), 6);
    }

    #[test]
    fn permits_depth_32_and_rejects_depth_33() {
        let dir = fixture("depth");
        let mut child = dir.clone();
        for _ in 0..32 {
            child.push("d");
        }
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join("image.jpg"), []).unwrap();
        assert_eq!(count_images_checked(&dir).unwrap(), 1);
        std::fs::create_dir(child.join("d")).unwrap();
        assert!(count_images_checked(&dir).unwrap_err().contains("depth"));
    }

    #[test]
    fn missing_directory_is_an_error() {
        let dir = fixture("missing").join("absent");
        let error = count_images_checked(&dir).unwrap_err();
        assert!(error.contains("cannot inspect"), "{error}");
    }

    #[test]
    fn root_regular_file_is_an_error() {
        let dir = fixture("root-file");
        let file = dir.join("image.jpg");
        std::fs::write(&file, []).unwrap();
        assert!(count_images_checked(&file).unwrap_err().contains("directory"));
    }

    #[test]
    fn observes_cook_cancellation_before_traversal() {
        let dir = fixture("cancel");
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let error = vernon_core::process::with_cancel(flag, || count_images_checked(&dir)).unwrap_err();
        assert!(error.contains("cancelled"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_root_and_child_symlink_cycles() {
        let dir = fixture("symlink");
        let link = dir.join("loop");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(count_images_checked(&dir).unwrap_err().contains("link"));
        assert!(count_images_checked(&link).unwrap_err().contains("link"));
    }

    #[cfg(windows)]
    #[test]
    fn rejects_root_and_child_junction_cycles() {
        use std::os::windows::process::CommandExt;
        let dir = fixture("junction");
        let link = dir.join("loop");
        let output = std::process::Command::new("cmd.exe")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&dir)
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        if !output.status.success() {
            let detail = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            let lower = detail.to_ascii_lowercase();
            if lower.contains("access is denied") || lower.contains("privilege") || lower.contains("not permitted") {
                eprintln!("SKIP junction fixture: this environment denied creation: {detail}");
                return;
            }
            panic!("creating a local test junction failed: {detail}");
        }
        let child_error = count_images_checked(&dir).unwrap_err();
        let root_error = count_images_checked(&link).unwrap_err();
        // Remove only the junction itself. The fixture stays in the temp area.
        std::fs::remove_dir(&link).unwrap();
        assert!(child_error.contains("reparse") || child_error.contains("link"), "{child_error}");
        assert!(root_error.contains("reparse") || root_error.contains("link"), "{root_error}");
    }
}
