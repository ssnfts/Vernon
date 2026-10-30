//! Unsaved-changes protection: New, Open and closing the window ask to save
//! a modified scene first. Pure decision logic, driven by the UI.

/// An action that would drop the current scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    New,
    Open,
    Close,
    Demo,
}

impl Pending {
    pub fn verb(self) -> &'static str {
        match self {
            Pending::New => "starting a new scene",
            Pending::Open => "opening another scene",
            Pending::Close => "closing",
            Pending::Demo => "replacing the scene with the demo pipeline",
        }
    }
}

/// The user's answer to the save prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Save,
    Discard,
    Cancel,
}

/// What the UI should do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Carry out the action now.
    Proceed(Pending),
    /// Save to the scene's path, then carry out the action if that worked.
    SaveThen(Pending),
    /// Ask for a file name first; the action follows a successful save.
    SaveAs,
    /// Nothing to do (prompt shown or cancelled).
    Wait,
}

/// Prompt state: the action waiting on an answer, and the one waiting on a
/// Save As dialog.
#[derive(Debug, Default)]
pub struct Guard {
    pub prompt: Option<Pending>,
    pub after_save: Option<Pending>,
}

impl Guard {
    /// A request for `a`: proceeds at once on a clean scene, prompts otherwise.
    pub fn request(&mut self, a: Pending, dirty: bool) -> Step {
        if dirty {
            self.prompt = Some(a);
            Step::Wait
        } else {
            Step::Proceed(a)
        }
    }

    /// The prompt was answered.
    pub fn answer(&mut self, c: Choice, has_path: bool) -> Step {
        let Some(a) = self.prompt.take() else { return Step::Wait };
        match (c, has_path) {
            (Choice::Cancel, _) => Step::Wait,
            (Choice::Discard, _) => Step::Proceed(a),
            (Choice::Save, true) => Step::SaveThen(a),
            (Choice::Save, false) => {
                self.after_save = Some(a);
                Step::SaveAs
            }
        }
    }

    /// The Save As dialog finished: the waiting action runs only after a save.
    pub fn save_as_done(&mut self, saved: bool) -> Option<Pending> {
        let a = self.after_save.take();
        a.filter(|_| saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_scene_proceeds_dirty_scene_prompts() {
        let mut g = Guard::default();
        assert_eq!(g.request(Pending::New, false), Step::Proceed(Pending::New));
        assert!(g.prompt.is_none());
        assert_eq!(g.request(Pending::Close, true), Step::Wait);
        assert_eq!(g.prompt, Some(Pending::Close));
    }

    #[test]
    fn replacing_with_demo_asks_before_discarding_edits() {
        let mut g = Guard::default();
        assert_eq!(g.request(Pending::Demo, false), Step::Proceed(Pending::Demo));
        assert_eq!(g.request(Pending::Demo, true), Step::Wait);
        assert_eq!(g.prompt, Some(Pending::Demo));
    }

    #[test]
    fn answers() {
        let mut g = Guard::default();
        g.request(Pending::Open, true);
        assert_eq!(g.answer(Choice::Cancel, true), Step::Wait);
        assert!(g.prompt.is_none());
        g.request(Pending::Open, true);
        assert_eq!(g.answer(Choice::Discard, true), Step::Proceed(Pending::Open));
        g.request(Pending::Close, true);
        assert_eq!(g.answer(Choice::Save, true), Step::SaveThen(Pending::Close));
        // an untitled scene goes through Save As; the action waits for the save
        g.request(Pending::New, true);
        assert_eq!(g.answer(Choice::Save, false), Step::SaveAs);
        assert_eq!(g.save_as_done(false), None);
        assert!(g.after_save.is_none());
        g.request(Pending::New, true);
        g.answer(Choice::Save, false);
        assert_eq!(g.save_as_done(true), Some(Pending::New));
        // no prompt open: an answer does nothing
        assert_eq!(g.answer(Choice::Discard, true), Step::Wait);
    }
}
