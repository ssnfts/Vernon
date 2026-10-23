//! Default keyboard map (spec 2.12, Vernon subset). Modifiers must match
//! exactly, so `Z`, `Shift+Z`, `Ctrl+Z` and `Shift+Ctrl+Z` are all distinct.

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
];

pub fn lookup(chord: Chord) -> Option<Action> {
    BINDINGS.iter().find(|(c, _, _)| *c == chord).map(|(_, a, _)| *a)
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
    fn no_conflicts_and_core_tools() {
        let mut seen = HashSet::new();
        for (c, _, _) in BINDINGS {
            assert!(seen.insert(*c), "duplicate chord {}", chord_text(*c));
        }
        assert_eq!(lookup(k(Key::W)), Some(Action::ToolMove));
        assert_eq!(lookup(k(Key::E)), Some(Action::ToolRotate));
        assert_eq!(lookup(k(Key::R)), Some(Action::ToolScale));
        assert_eq!(lookup(k(Key::Q)), Some(Action::ToolSelect));
        assert_eq!(lookup(ctrl(Key::Z)), Some(Action::Undo));
        assert_eq!(lookup(shift(Key::Z)), Some(Action::ViewUndo));
        assert_eq!(lookup(k(Key::Z)), Some(Action::ZoomExtentsSelected));
        assert_eq!(shortcut_for(Action::Maximize), "Alt+W");
        assert_eq!(chord_text(Chord { ctrl: true, shift: true, ..k(Key::Z) }), "Shift+Ctrl+Z");
    }
}
