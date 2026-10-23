//! The node registry the app runs with. Today it holds the local demo nodes;
//! the stage crates' `register` functions are added here when integrated.

use vernon_graph::Registry;

pub fn build_registry() -> Registry {
    let mut reg = Registry::new();
    crate::demo_nodes::register(&mut reg);
    reg
}

/// Vendor names that must never reach a user-visible string (CLEAN_ROOM.md).
#[cfg(test)]
pub const PROTECTED_NAMES: &[&str] = &[
    "Rhino", "Fusion", "Autodesk", "3ds Max", "3dsMax", "QUICKSURFACE", "Quicksurface", "Geomagic", "ZEISS", "Zeiss",
    "AirShaper", "SimScale", "Ansys", "OptimumLap", "Plasticity", "Polycam", "Ladybug", "CloudCompare", "Houdini",
    "SideFX", "ViewCube", "Slate", "MAXScript",
];

/// Returns the first protected name found in `text`, if any.
#[cfg(test)]
pub fn find_protected(text: &str) -> Option<&'static str> {
    PROTECTED_NAMES.iter().copied().find(|n| text.contains(n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vernon_graph::Stage;

    #[test]
    fn every_stage_has_a_node() {
        let reg = build_registry();
        for stage in Stage::ALL {
            assert!(!reg.by_stage(stage).is_empty(), "{stage:?} has no node");
        }
        assert!(reg.types().all(|i| i.id.starts_with("demo.")));
    }

    #[test]
    fn no_vendor_names_in_user_visible_node_strings() {
        let reg = build_registry();
        for info in reg.types() {
            let mut texts = vec![info.label.clone(), info.help.clone(), info.stage.label().to_string()];
            for p in &info.params {
                texts.push(p.label.clone());
                texts.push(p.help.clone());
                texts.extend(p.unit.clone());
            }
            for port in info.inputs.iter().chain(&info.outputs) {
                texts.push(port.name.clone());
            }
            for t in texts {
                assert_eq!(find_protected(&t), None, "{}: '{t}'", info.id);
            }
        }
    }

    #[test]
    fn no_vendor_names_in_app_string_literals() {
        // Scan quoted literals in the app sources, skipping this file's list.
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.file_name().is_some_and(|n| n == "registry.rs") {
                    continue;
                }
                let src = std::fs::read_to_string(&p).unwrap();
                for (n, line) in src.lines().enumerate() {
                    if line.trim_start().starts_with("//") || line.contains(".emulates(") {
                        continue;
                    }
                    for lit in line.split('"').skip(1).step_by(2) {
                        assert_eq!(find_protected(lit), None, "{}:{}: {lit}", p.display(), n + 1);
                    }
                }
            }
        }
    }
}
