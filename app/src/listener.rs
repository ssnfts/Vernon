//! The one-line command listener: `add <type>`, `cook`, `select <name>` and
//! a few more. Pure functions over [`AppState`], so they are unit-tested.

use crate::state::AppState;

pub const HELP: &str =
    "commands: add <type> | cook | select <name> | delete | bypass | undo | redo | types | objects | help";

/// Run one command line; `Ok` text goes to the log, `Err` is shown in red.
pub fn run(st: &mut AppState, line: &str) -> Result<String, String> {
    let line = line.trim();
    let (cmd, arg) = match line.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (line, ""),
    };
    match cmd.to_ascii_lowercase().as_str() {
        "" => Ok(String::new()),
        "help" | "?" => Ok(HELP.to_string()),
        "add" | "create" => {
            if arg.is_empty() {
                return Err("usage: add <type>, e.g. add box".into());
            }
            let ty = resolve_type(st, arg)?;
            let id = st.create(&ty)?;
            Ok(format!("added {} ({ty})", st.name(id)))
        }
        "cook" => {
            st.recook();
            let errors = st.graph.nodes.keys().filter(|id| st.error_of(**id).is_some()).count();
            Ok(format!("cooked {} node(s), {errors} error(s)", st.graph.nodes.len()))
        }
        "select" => {
            if arg.is_empty() {
                st.clear_selection();
                Ok("selection cleared".into())
            } else if st.select_by_name(arg) {
                Ok(format!("selected {arg}"))
            } else {
                Err(format!("no node named '{arg}'"))
            }
        }
        "delete" => {
            let n = st.selection.len();
            if n == 0 {
                return Err("nothing selected".into());
            }
            st.delete_selected();
            Ok(format!("deleted {n} node(s)"))
        }
        "bypass" => {
            let id = st.current_entry().ok_or("nothing selected")?;
            st.toggle_bypass(id);
            Ok(format!("toggled bypass of {}", st.name(id)))
        }
        "undo" => st.undo().then(|| "undone".to_string()).ok_or_else(|| "nothing to undo".into()),
        "redo" => st.redo().then(|| "redone".to_string()).ok_or_else(|| "nothing to redo".into()),
        "types" => Ok(st.registry.types().map(|i| i.id.as_str()).collect::<Vec<_>>().join(" ")),
        "objects" => Ok(st
            .objects()
            .into_iter()
            .map(|id| st.name(id).to_string())
            .collect::<Vec<_>>()
            .join(" ")),
        other => Err(format!("unknown command '{other}' ({HELP})")),
    }
}

/// Resolve a type by exact id, id suffix (`box` for `geo.box`) or label.
pub fn resolve_type(st: &AppState, s: &str) -> Result<String, String> {
    let s = s.trim();
    if st.registry.info(s).is_some() {
        return Ok(s.to_string());
    }
    let lower = s.to_ascii_lowercase();
    let matches: Vec<&str> = st
        .registry
        .types()
        .filter(|i| {
            i.id.rsplit('.').next() == Some(lower.as_str())
                || i.label.eq_ignore_ascii_case(s)
                || i.label.to_ascii_lowercase().replace([' ', '-'], "_") == lower
        })
        .map(|i| i.id.as_str())
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.to_string()),
        [] => Err(format!("unknown node type '{s}' (try: types)")),
        many => Err(format!("'{s}' is ambiguous: {}", many.join(", "))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::build_registry;

    #[test]
    fn add_select_cook() {
        let mut st = AppState::new(build_registry());
        assert!(run(&mut st, "add box").unwrap().contains("box1"));
        assert!(run(&mut st, "add Planar Cut").is_ok());
        assert_eq!(st.primary_stack().len(), 2);
        assert!(run(&mut st, "select box1").is_ok());
        assert_eq!(st.primary(), st.graph.find("box1"));
        let out = run(&mut st, "cook").unwrap();
        assert!(out.contains("2 node(s), 0 error(s)"), "{out}");
        assert!(run(&mut st, "select nothing_here").is_err());
        assert!(run(&mut st, "frobnicate").is_err());
        assert!(run(&mut st, "add").is_err());
        assert!(run(&mut st, "undo").is_ok());
        assert_eq!(run(&mut st, "").unwrap(), "");
    }

    #[test]
    fn resolves_type_names() {
        let st = AppState::new(build_registry());
        assert_eq!(resolve_type(&st, "geo.box").unwrap(), "geo.box");
        assert_eq!(resolve_type(&st, "sphere").unwrap(), "geo.sphere");
        assert_eq!(resolve_type(&st, "icp align").unwrap(), "clean.icp_align");
        assert!(resolve_type(&st, "aero_loads").unwrap_err().contains("ambiguous"));
        assert!(resolve_type(&st, "warp drive").is_err());
    }
}
