//! Plain-words next steps for node errors and import feedback. The node's own
//! message says what went wrong; `next_step` adds what to do about it.

/// What to try for a node error, matched on the text of the message.
pub fn next_step(error: &str) -> Option<&'static str> {
    let e = error.to_ascii_lowercase();
    let rules: &[(&[&str], &str)] = &[
        (&["no file path set"], "Set the File in the Modify tab, or use File > Import Mesh."),
        (
            &["unsupported file extension"],
            "Use OBJ, STL, PLY, 3MF, XYZ/PTS, E57 or LAS, or export your file to one of these first.",
        ),
        (
            &["os error 2", "os error 3", "cannot find the file", "cannot find the path", "no such file"],
            "The file is not at that path. Check the spelling, or pick it again in the Modify tab.",
        ),
        (
            &["os error 5", "permission denied", "access is denied"],
            "Vernon may not read that file. Close the program that has it open, or copy the file to another folder.",
        ),
        (
            &["held until the scene is trusted"],
            "Choose File > Trust Scene after checking what this scene would run or write.",
        ),
        (
            &["not connected"],
            "Wire the previous node's output to this input in the Node Editor, or add the missing step.",
        ),
        (
            &["executable", "program not found", "not found on path"],
            "Install the program and set its path in Utilities > External Tools. Without it Vernon only writes the input files.",
        ),
        (
            &["cannot read", "invalid", "malformed", "unexpected end", "parse"],
            "The file may be damaged or in another format than its extension says. Re-export it from the program that made it.",
        ),
    ];
    rules.iter().find(|(keys, _)| keys.iter().any(|k| e.contains(k))).map(|(_, hint)| *hint)
}

/// A line about the size of freshly imported geometry, and whether the size
/// looks like a unit mix-up. Sizes are in millimetres.
pub fn import_size_note(name: &str, size_mm: [f64; 3]) -> (String, bool) {
    let longest = size_mm.iter().cloned().fold(0.0_f64, f64::max);
    let dims = format!("{:.1} x {:.1} x {:.1} mm", size_mm[0], size_mm[1], size_mm[2]);
    if longest <= 0.0 {
        return (
            format!("{name}: imported, but it has no extent ({dims}). The file may be empty or hold a single point."),
            true,
        );
    }
    if longest < 20.0 {
        return (
            format!("{name}: {dims}. That is tiny; if the file is in metres or inches, set File unit to m or in in the Modify tab."),
            true,
        );
    }
    if longest > 50_000.0 {
        return (
            format!("{name}: {dims}. That is huge; if the file is already in millimetres, set File unit to mm in the Modify tab."),
            true,
        );
    }
    (format!("{name}: imported, {dims}. Wrong size? Change File unit in the Modify tab."), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_and_bad_extension_get_a_next_step() {
        assert!(next_step("cannot read 'a.obj': The system cannot find the file specified. (os error 2)")
            .unwrap()
            .contains("not at that path"));
        assert!(next_step("unsupported file extension in 'a.xyzzy'").unwrap().contains("OBJ"));
        assert!(next_step("something unrelated").is_none());
    }

    #[test]
    fn unit_mixups_are_flagged() {
        assert!(import_size_note("a", [1.0, 2.0, 0.5]).1);
        assert!(!import_size_note("a", [400.0, 90.0, 50.0]).1);
        assert!(import_size_note("a", [9e5, 1.0, 1.0]).1);
        assert!(import_size_note("a", [0.0, 0.0, 0.0]).1);
    }
}
