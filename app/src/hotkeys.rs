//! Default keyboard map (spec 2.12). Modifiers must match exactly, so `Z`,
//! `Shift+Z`, `Ctrl+Z` and `Shift+Ctrl+Z` are all distinct.
//!
//! Every chord of the spec table is either bound in [`BINDINGS`] or listed
//! in [`RESERVED`] with the reason it does nothing yet (the feature it names
//! does not exist in Vernon); a reserved chord is never reused for anything
//! else, and the tests check the whole table for conflicts.
//!
//! `keymap.json` in the user config directory overrides the defaults: one
//! `"action_name": "Ctrl+Shift+K"` entry per action, `null` to unbind it.
//! A bad entry is reported and leaves that action on its default.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use eframe::egui::Key;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Undo,
    Redo,
    ViewUndo,
    ViewRedo,
    New,
    Open,
    Save,
    Delete,
    SelectAll,
    SelectNone,
    SelectInvert,
    ToolSelect,
    ToolMove,
    ToolRotate,
    ToolScale,
    Snap,
    AngleSnap,
    ZoomExtentsSelected,
    ZoomExtentsAll,
    ZoomExtentsAllViews,
    Maximize,
    ToggleWire,
    ToggleEdges,
    ToggleGrid,
    ViewTop,
    ViewBottom,
    ViewFront,
    ViewLeft,
    ViewPerspective,
    ViewUser,
    NodeEditor,
    AgentChat,
    Play,
    PrevFrame,
    NextFrame,
    GoStart,
    GoEnd,
    HideSelected,
    UnhideAll,
    HideUnselected,
    Isolate,
    SelectionLock,
    ZoomRegion,
    TypeIn,
    HomeView,
    OrbitMode,
    ZoomIn,
    ZoomOut,
    SelectSimilar,
    Statistics,
    Cut,
    Copy,
    Paste,
    FindInExplorer,
    ToggleGeometry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Key,
}

const fn k(key: Key) -> Chord {
    Chord {
        ctrl: false,
        shift: false,
        alt: false,
        key,
    }
}

const fn ctrl(key: Key) -> Chord {
    Chord { ctrl: true, ..k(key) }
}

const fn shift(key: Key) -> Chord {
    Chord { shift: true, ..k(key) }
}

const fn alt(key: Key) -> Chord {
    Chord { alt: true, ..k(key) }
}

pub const BINDINGS: &[(Chord, Action, &str)] = &[
    (ctrl(Key::Z), Action::Undo, "Undo scene change"),
    (ctrl(Key::Y), Action::Redo, "Redo scene change"),
    (shift(Key::Z), Action::ViewUndo, "Undo view change"),
    (shift(Key::Y), Action::ViewRedo, "Redo view change"),
    (ctrl(Key::N), Action::New, "New scene"),
    (ctrl(Key::O), Action::Open, "Open scene"),
    (ctrl(Key::S), Action::Save, "Save scene"),
    (k(Key::Delete), Action::Delete, "Delete selection"),
    (ctrl(Key::A), Action::SelectAll, "Select all"),
    (ctrl(Key::D), Action::SelectNone, "Select none"),
    (ctrl(Key::I), Action::SelectInvert, "Select invert"),
    (k(Key::Q), Action::ToolSelect, "Select tool"),
    (k(Key::W), Action::ToolMove, "Move tool"),
    (k(Key::E), Action::ToolRotate, "Rotate tool"),
    (k(Key::R), Action::ToolScale, "Scale tool"),
    (k(Key::S), Action::Snap, "Grid snap toggle"),
    (k(Key::A), Action::AngleSnap, "Angle snap toggle"),
    (k(Key::Z), Action::ZoomExtentsSelected, "Zoom extents selected"),
    (
        Chord {
            ctrl: true,
            alt: true,
            ..k(Key::Z)
        },
        Action::ZoomExtentsAll,
        "Zoom extents all",
    ),
    (
        Chord {
            ctrl: true,
            shift: true,
            ..k(Key::Z)
        },
        Action::ZoomExtentsAllViews,
        "Zoom extents in all viewports",
    ),
    (alt(Key::W), Action::Maximize, "Maximise / restore viewport"),
    (k(Key::F3), Action::ToggleWire, "Wireframe / shaded"),
    (k(Key::F4), Action::ToggleEdges, "Edged faces"),
    (k(Key::G), Action::ToggleGrid, "Home grid"),
    (k(Key::T), Action::ViewTop, "Top view"),
    (k(Key::B), Action::ViewBottom, "Bottom view"),
    (k(Key::F), Action::ViewFront, "Front view"),
    (k(Key::L), Action::ViewLeft, "Left view"),
    (k(Key::P), Action::ViewPerspective, "Perspective view"),
    (k(Key::U), Action::ViewUser, "Orthographic user view"),
    (k(Key::M), Action::NodeEditor, "Node editor"),
    (Chord { ctrl: true, shift: true, ..k(Key::J) }, Action::AgentChat, "Agent chat panel"),
    (k(Key::Slash), Action::Play, "Play / stop"),
    (k(Key::Comma), Action::PrevFrame, "Previous frame"),
    (k(Key::Period), Action::NextFrame, "Next frame"),
    (k(Key::Home), Action::GoStart, "Go to start"),
    (k(Key::End), Action::GoEnd, "Go to end"),
    (alt(Key::H), Action::HideSelected, "Hide selection"),
    (alt(Key::U), Action::UnhideAll, "Unhide all"),
    (alt(Key::I), Action::HideUnselected, "Hide unselected"),
    (alt(Key::Q), Action::Isolate, "Isolate selection / end isolation"),
    (
        Chord {
            ctrl: true,
            shift: true,
            ..k(Key::N)
        },
        Action::SelectionLock,
        "Selection lock",
    ),
    (ctrl(Key::W), Action::ZoomRegion, "Zoom region"),
    (k(Key::F12), Action::TypeIn, "Transform type-in"),
    (alt(Key::Home), Action::HomeView, "Home view"),
    (ctrl(Key::R), Action::OrbitMode, "Switch turntable / trackball orbit"),
    (k(Key::OpenBracket), Action::ZoomIn, "Zoom in"),
    (k(Key::CloseBracket), Action::ZoomOut, "Zoom out"),
    (
        Chord {
            ctrl: true,
            shift: true,
            ..k(Key::A)
        },
        Action::SelectSimilar,
        "Select objects of the same type",
    ),
    (k(Key::Num7), Action::Statistics, "Scene statistics"),
    (ctrl(Key::X), Action::Cut, "Cut selected nodes"),
    (ctrl(Key::C), Action::Copy, "Copy selected nodes"),
    (ctrl(Key::V), Action::Paste, "Paste nodes"),
    (k(Key::X), Action::FindInExplorer, "Find an object by name"),
    (shift(Key::G), Action::ToggleGeometry, "Hide / show geometry objects"),
];

/// The names `keymap.json` uses for actions.
pub const ACTION_NAMES: &[(&str, Action)] = &[
    ("undo", Action::Undo),
    ("redo", Action::Redo),
    ("view_undo", Action::ViewUndo),
    ("view_redo", Action::ViewRedo),
    ("new", Action::New),
    ("open", Action::Open),
    ("save", Action::Save),
    ("delete", Action::Delete),
    ("select_all", Action::SelectAll),
    ("select_none", Action::SelectNone),
    ("select_invert", Action::SelectInvert),
    ("tool_select", Action::ToolSelect),
    ("tool_move", Action::ToolMove),
    ("tool_rotate", Action::ToolRotate),
    ("tool_scale", Action::ToolScale),
    ("snap", Action::Snap),
    ("angle_snap", Action::AngleSnap),
    ("zoom_extents_selected", Action::ZoomExtentsSelected),
    ("zoom_extents_all", Action::ZoomExtentsAll),
    ("zoom_extents_all_views", Action::ZoomExtentsAllViews),
    ("maximize", Action::Maximize),
    ("toggle_wire", Action::ToggleWire),
    ("toggle_edges", Action::ToggleEdges),
    ("toggle_grid", Action::ToggleGrid),
    ("view_top", Action::ViewTop),
    ("view_bottom", Action::ViewBottom),
    ("view_front", Action::ViewFront),
    ("view_left", Action::ViewLeft),
    ("view_perspective", Action::ViewPerspective),
    ("view_user", Action::ViewUser),
    ("node_editor", Action::NodeEditor),
    ("agent_chat", Action::AgentChat),
    ("play", Action::Play),
    ("prev_frame", Action::PrevFrame),
    ("next_frame", Action::NextFrame),
    ("go_start", Action::GoStart),
    ("go_end", Action::GoEnd),
    ("hide_selected", Action::HideSelected),
    ("unhide_all", Action::UnhideAll),
    ("hide_unselected", Action::HideUnselected),
    ("isolate", Action::Isolate),
    ("selection_lock", Action::SelectionLock),
    ("zoom_region", Action::ZoomRegion),
    ("type_in", Action::TypeIn),
    ("home_view", Action::HomeView),
    ("orbit_mode", Action::OrbitMode),
    ("zoom_in", Action::ZoomIn),
    ("zoom_out", Action::ZoomOut),
    ("select_similar", Action::SelectSimilar),
    ("statistics", Action::Statistics),
    ("cut", Action::Cut),
    ("copy", Action::Copy),
    ("paste", Action::Paste),
    ("find_in_explorer", Action::FindInExplorer),
    ("toggle_geometry", Action::ToggleGeometry),
];

/// Spec 2.12 chords with no action yet, each with what it would drive. They
/// are held back so they cannot be taken by another command.
pub const RESERVED: &[(Chord, &str)] = &[
    (alt(Key::S), "cycle snap type"),
    (k(Key::Insert), "affect pivot only"),
    (k(Key::F5), "gizmo axis X"),
    (k(Key::F6), "gizmo axis Y"),
    (k(Key::F7), "gizmo axis Z"),
    (k(Key::F8), "cycle plane constraint"),
    (k(Key::Minus), "gizmo size down"),
    (k(Key::Equals), "gizmo size up"),
    (k(Key::Space), "lock quad set"),
    (k(Key::C), "camera view"),
    (k(Key::F2), "shade selected faces"),
    (shift(Key::F), "safe frames"),
    (alt(Key::X), "see-through display"),
    (k(Key::O), "adaptive degradation"),
    (alt(Key::Z), "zoom mode"),
    (k(Key::I), "pan mode"),
    (shift(Key::H), "hide helpers category"),
    (shift(Key::L), "hide lights category"),
    (shift(Key::C), "hide cameras category"),
    (shift(Key::S), "hide shapes category"),
    (shift(Key::W), "hide space warps category"),
    (shift(Key::P), "hide particles category"),
];

/// The chords in effect: the defaults with the user's overrides applied.
#[derive(Debug, Clone)]
pub struct Keymap {
    pub bindings: Vec<(Chord, Action, &'static str)>,
    /// One line per `keymap.json` entry that could not be applied.
    pub warnings: Vec<String>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            bindings: BINDINGS.to_vec(),
            warnings: Vec::new(),
        }
    }
}

impl Keymap {
    /// Apply the text of a `keymap.json`. Nothing in it can fail: a bad file
    /// gives the defaults and a bad entry keeps that action's default, each
    /// with a warning.
    pub fn from_json(text: &str) -> Self {
        let mut map = Self::default();
        let entries: BTreeMap<String, serde_json::Value> = match serde_json::from_str(text) {
            Ok(e) => e,
            Err(e) => {
                map.warnings.push(format!("keymap.json ignored: expected an object of action names and chords ({e})"));
                return map;
            }
        };
        // Parse everything first, so two actions can swap their chords.
        let mut wanted: Vec<(&str, Action, Option<Chord>)> = Vec::new();
        for (name, value) in &entries {
            let Some(action) = ACTION_NAMES.iter().find(|(n, _)| n == name).map(|(_, a)| *a) else {
                map.warnings.push(format!("keymap.json: unknown action '{name}'"));
                continue;
            };
            let chord = match value {
                serde_json::Value::Null => None,
                serde_json::Value::String(s) => match parse_chord(s) {
                    Ok(c) => Some(c),
                    Err(e) => {
                        map.warnings.push(format!("keymap.json: {name}: {e}"));
                        continue;
                    }
                },
                other => {
                    map.warnings.push(format!("keymap.json: {name}: expected a chord string or null, found {other}"));
                    continue;
                }
            };
            wanted.push((name, action, chord));
        }
        let defaults = std::mem::take(&mut map.bindings);
        map.bindings = defaults.iter().filter(|(_, a, _)| !wanted.iter().any(|(_, w, _)| w == a)).copied().collect();
        let mut refused = Vec::new();
        for (name, action, chord) in &wanted {
            let Some(chord) = *chord else { continue };
            let text = defaults.iter().find(|(_, a, _)| a == action).map_or("", |(_, _, t)| *t);
            let shown = chord_text(chord);
            if let Some(what) = reserved(chord) {
                map.warnings.push(format!("keymap.json: {name}: {shown} is reserved for {what}"));
                refused.push(*action);
            } else if map.bindings.iter().any(|(c, _, _)| *c == chord) {
                map.warnings.push(format!("keymap.json: {name}: {shown} is already bound"));
                refused.push(*action);
            } else {
                map.bindings.push((chord, *action, text));
            }
        }
        // A refused entry goes back to its default, unless that was taken.
        for action in refused {
            let Some(default) = defaults.iter().find(|(_, a, _)| *a == action) else { continue };
            if map.bindings.iter().any(|(c, _, _)| *c == default.0) {
                map.warnings.push(format!("keymap.json: {} now has no shortcut: its default {} was given to another action", default.2, chord_text(default.0)));
            } else {
                map.bindings.push(*default);
            }
        }
        map
    }

    /// Read `path`. A missing file is normal and gives the defaults silently.
    pub fn from_file(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_json(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => Self {
                warnings: vec![format!("keymap.json ignored: cannot read {}: {e}", path.display())],
                ..Self::default()
            },
        }
    }

    pub fn lookup(&self, chord: Chord) -> Option<Action> {
        self.bindings.iter().find(|(c, _, _)| *c == chord).map(|(_, a, _)| *a)
    }

    pub fn shortcut_for(&self, action: Action) -> String {
        self.bindings
            .iter()
            .find(|(_, a, _)| *a == action)
            .map(|(c, _, _)| chord_text(*c))
            .unwrap_or_default()
    }
}

/// Parse text like `Shift+Ctrl+Z` or `alt+f5` (the form [`chord_text`] writes).
pub fn parse_chord(text: &str) -> Result<Chord, String> {
    let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
    let bad = || format!("'{text}' is not a chord");
    let name = parts.pop().filter(|n| !n.is_empty()).ok_or_else(bad)?;
    let key = Key::from_name(name)
        .or_else(|| Key::from_name(&name.to_uppercase()))
        .or_else(|| {
            let mut c = name.chars();
            let first = c.next()?.to_uppercase().to_string();
            Key::from_name(&(first + &c.as_str().to_lowercase()))
        })
        .ok_or_else(bad)?;
    let mut chord = k(key);
    for part in parts {
        let flag = match part.to_lowercase().as_str() {
            "ctrl" | "control" | "cmd" | "command" => &mut chord.ctrl,
            "shift" => &mut chord.shift,
            "alt" | "option" => &mut chord.alt,
            _ => return Err(bad()),
        };
        if std::mem::replace(flag, true) {
            return Err(format!("'{text}' repeats a modifier"));
        }
    }
    Ok(chord)
}

/// Portable mode: a `portable.txt` beside the executable (or in its parent
/// folder, for the packaged `binernon.exe` layout) keeps settings in
/// `data/config` next to that marker instead of the per-user directory.
fn portable_config_dir(exe: &std::path::Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    [Some(dir), dir.parent()]
        .into_iter()
        .flatten()
        .find(|d| d.join("portable.txt").is_file())
        .map(|d| d.join("data").join("config"))
}

/// The config directory (no directory is created here): the portable one when
/// a `portable.txt` marker is present, otherwise the per-user directory.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| portable_config_dir(&exe)) {
        return Some(dir);
    }
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        var("APPDATA").map(|d| d.join("Vernon"))
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|d| d.join("Library/Application Support/Vernon"))
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|d| d.join(".config"))).map(|d| d.join("vernon"))
    }
}

static ACTIVE: OnceLock<Keymap> = OnceLock::new();

/// Load the user's `keymap.json` for this run; returns its warnings.
pub fn load_user_keymap() -> Vec<String> {
    let map = config_dir().map(|d| Keymap::from_file(&d.join("keymap.json"))).unwrap_or_default();
    let warnings = map.warnings.clone();
    let _ = ACTIVE.set(map);
    warnings
}

/// The keymap in effect.
pub fn active() -> &'static Keymap {
    ACTIVE.get_or_init(Keymap::default)
}

pub fn lookup(chord: Chord) -> Option<Action> {
    active().lookup(chord)
}

/// What a reserved chord would drive, if `chord` is one.
pub fn reserved(chord: Chord) -> Option<&'static str> {
    RESERVED.iter().find(|(c, _)| *c == chord).map(|(_, what)| *what)
}

pub fn chord_text(c: Chord) -> String {
    let mut s = String::new();
    if c.shift {
        s.push_str("Shift+");
    }
    if c.ctrl {
        s.push_str("Ctrl+");
    }
    if c.alt {
        s.push_str("Alt+");
    }
    s.push_str(c.key.name());
    s
}

/// The shortcut text shown next to a menu entry.
pub fn shortcut_for(action: Action) -> String {
    active().shortcut_for(action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_whole_spec_table_resolves_without_conflicts() {
        let mut seen = HashSet::new();
        for (c, _, _) in BINDINGS {
            assert!(seen.insert(*c), "duplicate chord {}", chord_text(*c));
        }
        for (c, why) in RESERVED {
            assert!(seen.insert(*c), "reserved chord {} ({why}) is also bound", chord_text(*c));
        }
        // The spec's table has 61 chords; Vernon adds B for the bottom view.
        // Each one is either bound or reserved.
        let spec: Vec<Chord> = [
            // Tools
            k(Key::Q), k(Key::W), k(Key::E), k(Key::R), k(Key::A), k(Key::S), alt(Key::S),
            k(Key::Insert), k(Key::F5), k(Key::F6), k(Key::F7), k(Key::F8), k(Key::Minus), k(Key::Equals),
            // Selection
            ctrl(Key::A), ctrl(Key::D), ctrl(Key::I),
            Chord { ctrl: true, shift: true, ..k(Key::A) },
            Chord { ctrl: true, shift: true, ..k(Key::N) },
            alt(Key::Q), alt(Key::H), alt(Key::I), alt(Key::U), k(Key::X), k(Key::Space),
            // Views
            k(Key::P), k(Key::T), k(Key::F), k(Key::L), k(Key::U), k(Key::C), alt(Key::W), alt(Key::Home),
            k(Key::F3), k(Key::F4), k(Key::F2), k(Key::G), shift(Key::F), k(Key::Num7), alt(Key::X), k(Key::O),
            shift(Key::G), shift(Key::H), shift(Key::L), shift(Key::C), shift(Key::S), shift(Key::W), shift(Key::P),
            // Zoom and pan
            k(Key::Z),
            Chord { ctrl: true, alt: true, ..k(Key::Z) },
            Chord { ctrl: true, shift: true, ..k(Key::Z) },
            alt(Key::Z), ctrl(Key::W), k(Key::I), ctrl(Key::R), k(Key::OpenBracket), k(Key::CloseBracket),
            // Undo
            ctrl(Key::Z), ctrl(Key::Y), shift(Key::Z), shift(Key::Y),
        ]
        .into();
        assert_eq!(spec.len(), 61);
        let distinct: HashSet<Chord> = spec.iter().copied().collect();
        assert_eq!(distinct.len(), 61, "the spec table repeats a chord");
        for c in &spec {
            assert!(seen.contains(c), "spec chord {} is neither bound nor reserved", chord_text(*c));
        }
        assert!(seen.contains(&k(Key::B)));
        // Reserved chords do nothing: they never resolve to an action.
        for (c, why) in RESERVED {
            assert_eq!(lookup(*c), None, "{}", chord_text(*c));
            assert_eq!(reserved(*c), Some(*why));
        }
        assert!(BINDINGS.len() >= 49, "{}", BINDINGS.len());
        assert_eq!(RESERVED.len(), 22);
        // Of the 61 spec chords, those still reserved leave 39 bound.
        assert_eq!(spec.iter().filter(|c| lookup(**c).is_some()).count(), 39);
    }

    #[test]
    fn no_conflicts_and_core_tools() {
        let mut seen = HashSet::new();
        for (c, _, _) in BINDINGS {
            assert!(seen.insert(*c), "duplicate chord {}", chord_text(*c));
        }
        assert_eq!(lookup(alt(Key::Home)), Some(Action::HomeView));
        assert_eq!(lookup(ctrl(Key::R)), Some(Action::OrbitMode));
        assert_eq!(lookup(k(Key::OpenBracket)), Some(Action::ZoomIn));
        assert_eq!(lookup(k(Key::CloseBracket)), Some(Action::ZoomOut));
        assert_eq!(lookup(k(Key::Num7)), Some(Action::Statistics));
        assert_eq!(lookup(k(Key::W)), Some(Action::ToolMove));
        assert_eq!(lookup(k(Key::E)), Some(Action::ToolRotate));
        assert_eq!(lookup(k(Key::R)), Some(Action::ToolScale));
        assert_eq!(lookup(k(Key::Q)), Some(Action::ToolSelect));
        assert_eq!(lookup(ctrl(Key::Z)), Some(Action::Undo));
        assert_eq!(lookup(shift(Key::Z)), Some(Action::ViewUndo));
        assert_eq!(lookup(k(Key::Z)), Some(Action::ZoomExtentsSelected));
        assert_eq!(shortcut_for(Action::Maximize), "Alt+W");
        assert_eq!(lookup(alt(Key::Q)), Some(Action::Isolate));
        assert_eq!(lookup(alt(Key::I)), Some(Action::HideUnselected));
        assert_eq!(lookup(ctrl(Key::W)), Some(Action::ZoomRegion));
        assert_eq!(lookup(k(Key::F12)), Some(Action::TypeIn));
        assert_eq!(shortcut_for(Action::SelectionLock), "Shift+Ctrl+N");
        assert_eq!(chord_text(Chord { ctrl: true, shift: true, ..k(Key::Z) }), "Shift+Ctrl+Z");
        assert_eq!(lookup(ctrl(Key::V)), Some(Action::Paste));
        assert_eq!(lookup(k(Key::X)), Some(Action::FindInExplorer));
        assert_eq!(lookup(shift(Key::G)), Some(Action::ToggleGeometry));
    }

    #[test]
    fn every_action_is_named_once_and_every_chord_round_trips() {
        let mut names = HashSet::new();
        let mut actions = HashSet::new();
        for (n, a) in ACTION_NAMES {
            assert!(names.insert(*n), "{n}");
            assert!(actions.insert(*a), "{n}");
        }
        for (c, a, _) in BINDINGS {
            assert!(actions.contains(a), "{a:?} has no keymap.json name");
            assert_eq!(parse_chord(&chord_text(*c)), Ok(*c));
        }
        assert_eq!(actions.len(), BINDINGS.len(), "one default chord per action");
    }

    #[test]
    fn chords_parse_loosely_but_not_wrongly() {
        assert_eq!(parse_chord(" ctrl + shift + k "), Ok(Chord { ctrl: true, shift: true, ..k(Key::K) }));
        assert_eq!(parse_chord("alt+f5"), Ok(alt(Key::F5)));
        assert_eq!(parse_chord("delete"), Ok(k(Key::Delete)));
        for bad in ["", "Ctrl", "Ctrl+", "+", "Hyper+K", "Ctrl+Ctrl+K", "NotAKey"] {
            assert!(parse_chord(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn overrides_rebind_unbind_and_swap() {
        let m = Keymap::from_json(r#"{"save": "Ctrl+Shift+K", "snap": null, "undo": "Ctrl+Y", "redo": "Ctrl+Z"}"#);
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
        assert_eq!(m.lookup(Chord { ctrl: true, shift: true, ..k(Key::K) }), Some(Action::Save));
        assert_eq!(m.lookup(ctrl(Key::S)), None, "the old chord is free");
        assert_eq!(m.lookup(k(Key::S)), None);
        assert_eq!(m.shortcut_for(Action::Snap), "");
        assert_eq!(m.lookup(ctrl(Key::Y)), Some(Action::Undo));
        assert_eq!(m.lookup(ctrl(Key::Z)), Some(Action::Redo));
        assert_eq!(m.bindings.iter().find(|(_, a, _)| *a == Action::Save).map(|b| b.2), Some("Save scene"));
    }

    #[test]
    fn invalid_keymap_entries_warn_and_keep_defaults() {
        let m = Keymap::from_json(
            r#"{"no_such_action": "K", "save": "Ctrl+Nope", "open": 5, "new": "",
                "undo": "Ctrl+Y", "delete": "F5", "select_all": "Q", "copy": "Ctrl+Shift+K"}"#,
        );
        assert_eq!(m.warnings.len(), 7, "{:?}", m.warnings);
        let has = |needle: &str| m.warnings.iter().any(|w| w.contains(needle));
        assert!(has("unknown action 'no_such_action'") && has("save:") && has("open:") && has("new:"));
        assert!(has("reserved for gizmo axis X"), "{:?}", m.warnings);
        assert!(has("Ctrl+Y is already bound") && has("Q is already bound"));
        // Every refused entry left its action on the default.
        for (c, a) in [
            (ctrl(Key::S), Action::Save),
            (ctrl(Key::O), Action::Open),
            (ctrl(Key::N), Action::New),
            (ctrl(Key::Z), Action::Undo),
            (k(Key::Delete), Action::Delete),
            (ctrl(Key::A), Action::SelectAll),
            (ctrl(Key::Y), Action::Redo),
        ] {
            assert_eq!(m.lookup(c), Some(a));
        }
        assert_eq!(m.lookup(Chord { ctrl: true, shift: true, ..k(Key::K) }), Some(Action::Copy), "a good entry beside bad ones applies");
        let mut seen = HashSet::new();
        assert!(m.bindings.iter().all(|(c, _, _)| seen.insert(*c)), "no chord bound twice");
    }

    #[test]
    fn a_refused_entry_whose_default_was_taken_ends_unbound_with_a_warning() {
        // copy refuses Q (held), and paste has taken copy's default.
        let m = Keymap::from_json(r#"{"copy": "Q", "paste": "Ctrl+C"}"#);
        assert_eq!(m.lookup(ctrl(Key::C)), Some(Action::Paste));
        assert_eq!(m.shortcut_for(Action::Copy), "");
        assert_eq!(m.warnings.len(), 2, "{:?}", m.warnings);
    }

    #[test]
    fn broken_or_missing_files_never_fail() {
        for text in ["", "not json", "[1, 2]", "\"text\"", "{\"save\": "] {
            let m = Keymap::from_json(text);
            assert_eq!(m.warnings.len(), 1, "{text}");
            assert_eq!(m.bindings.len(), BINDINGS.len());
        }
        let dir = std::env::temp_dir().join(format!("vernon_keymap_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = Keymap::from_file(&dir.join("absent.json"));
        assert!(missing.warnings.is_empty());
        assert_eq!(missing.bindings.len(), BINDINGS.len());
        let path = dir.join("keymap.json");
        std::fs::write(&path, r#"{"save": "F11"}"#).unwrap();
        assert_eq!(Keymap::from_file(&path).lookup(k(Key::F11)), Some(Action::Save));
        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(Keymap::from_file(&path).warnings.len(), 1, "not UTF-8");
        assert_eq!(Keymap::from_file(&dir).warnings.len(), 1, "a directory");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn portable_marker_moves_the_config_dir() {
        let root = std::env::temp_dir().join(format!("vernon_portable_{}", std::process::id()));
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("vernon.exe");
        assert_eq!(portable_config_dir(&exe), None, "no marker");
        std::fs::write(root.join("portable.txt"), "x").unwrap();
        assert_eq!(portable_config_dir(&exe), Some(root.join("data").join("config")), "marker in the parent");
        std::fs::write(bin.join("portable.txt"), "x").unwrap();
        assert_eq!(portable_config_dir(&exe), Some(bin.join("data").join("config")), "marker beside the exe wins");
        let _ = std::fs::remove_dir_all(&root);
    }
}
