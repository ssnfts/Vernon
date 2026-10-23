//! Default keyboard map (spec 2.12). Modifiers must match exactly, so `Z`,
//! `Shift+Z`, `Ctrl+Z` and `Shift+Ctrl+Z` are all distinct.
//!
//! Every chord of the spec table is either bound in [`BINDINGS`] or listed
//! in [`RESERVED`] with the reason it does nothing yet (the feature it names
//! does not exist in Vernon); a reserved chord is never reused for anything
//! else, and the tests check the whole table for conflicts.

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
    (k(Key::X), "global search"),
    (k(Key::Space), "lock quad set"),
    (k(Key::C), "camera view"),
    (k(Key::F2), "shade selected faces"),
    (shift(Key::F), "safe frames"),
    (alt(Key::X), "see-through display"),
    (k(Key::O), "adaptive degradation"),
    (alt(Key::Z), "zoom mode"),
    (k(Key::I), "pan mode"),
    (shift(Key::G), "hide geometry category"),
    (shift(Key::H), "hide helpers category"),
    (shift(Key::L), "hide lights category"),
    (shift(Key::C), "hide cameras category"),
    (shift(Key::S), "hide shapes category"),
    (shift(Key::W), "hide space warps category"),
    (shift(Key::P), "hide particles category"),
];

pub fn lookup(chord: Chord) -> Option<Action> {
    BINDINGS.iter().find(|(c, _, _)| *c == chord).map(|(_, a, _)| *a)
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
    BINDINGS
        .iter()
        .find(|(_, a, _)| *a == action)
        .map(|(c, _, _)| chord_text(*c))
        .unwrap_or_default()
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
    }
}
