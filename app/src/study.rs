//! Study sweep on the time slider (spec 4.1 Motion tab): one node parameter
//! runs from `v0` at the segment start to `v1` at its end, so each frame is
//! one sample (a speed, a yaw angle, a load case). Every sample remembers
//! whether its cook solved, failed or went stale after a later edit, which
//! the track bar shows.

use std::collections::BTreeMap;

use vernon_graph::NodeId;

use crate::timeline::TimeConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleState {
    /// Cooked without errors and every gate passed.
    Solved,
    /// A node failed or a gate did not pass.
    Failed,
    /// Solved or failed before the scene or the sweep changed.
    Stale,
}

#[derive(Debug, Clone, Default)]
pub struct Study {
    pub enabled: bool,
    pub node: Option<NodeId>,
    pub param: String,
    pub v0: f64,
    pub v1: f64,
    pub samples: BTreeMap<i64, SampleState>,
    /// Frame whose value was applied last.
    applied: Option<i64>,
    /// Frame whose cook result is awaited.
    waiting: Option<i64>,
    /// Scene revision the samples belong to.
    revision: u64,
}

impl Study {
    /// Parameter value at `frame`: linear from `v0` (segment start) to `v1`
    /// (segment end).
    pub fn value_at(&self, cfg: &TimeConfig, frame: i64) -> f64 {
        let len = cfg.length();
        if len <= 0 {
            return self.v0;
        }
        let t = (cfg.clamp(frame) - cfg.start) as f64 / len as f64;
        self.v0 + (self.v1 - self.v0) * t
    }

    pub fn active(&self) -> bool {
        self.enabled && self.node.is_some() && !self.param.is_empty()
    }

    /// The value to apply when the frame moved since the last application
    /// (or the sweep was redefined); `None` otherwise.
    pub fn due(&mut self, cfg: &TimeConfig, frame: i64) -> Option<f64> {
        if !self.active() || self.applied == Some(frame) {
            return None;
        }
        self.applied = Some(frame);
        self.waiting = Some(frame);
        Some(self.value_at(cfg, frame))
    }

    /// The sweep definition changed: apply again and mark every sample stale.
    pub fn redefine(&mut self) {
        self.applied = None;
        self.waiting = None;
        self.mark_stale();
    }

    pub fn mark_stale(&mut self) {
        for s in self.samples.values_mut() {
            *s = SampleState::Stale;
        }
    }

    /// Record the finished cook of the awaited frame.
    pub fn record(&mut self, ok: bool) {
        let Some(f) = self.waiting.take() else { return };
        self.samples.insert(f, if ok { SampleState::Solved } else { SampleState::Failed });
    }

    /// Note the scene edit count after the sweep wrote its value, so its own
    /// parameter writes do not count as outside edits.
    pub fn applied_revision(&mut self, revision: u64) {
        self.revision = revision;
    }

    /// An outside edit: when the scene's edit count moved past the samples',
    /// they are stale.
    pub fn observe(&mut self, revision: u64) {
        if self.revision != 0 && revision != self.revision {
            self.mark_stale();
            self.revision = revision;
            // the current frame must be cooked again with the new scene
            self.applied = None;
        }
    }

    pub fn state(&self, frame: i64) -> Option<SampleState> {
        self.samples.get(&frame).copied()
    }

    pub fn awaiting(&self) -> bool {
        self.waiting.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn study() -> Study {
        Study { enabled: true, node: Some(3), param: "u_ms".into(), v0: 20.0, v1: 40.0, ..Default::default() }
    }

    #[test]
    fn frames_map_linearly_onto_the_sweep() {
        let cfg = TimeConfig { start: 0, end: 10, ..Default::default() };
        let s = study();
        assert_eq!(s.value_at(&cfg, 0), 20.0);
        assert_eq!(s.value_at(&cfg, 5), 30.0);
        assert_eq!(s.value_at(&cfg, 10), 40.0);
        // outside the segment the end values hold
        assert_eq!(s.value_at(&cfg, 50), 40.0);
    }

    #[test]
    fn samples_are_solved_failed_and_go_stale() {
        let cfg = TimeConfig { start: 0, end: 10, ..Default::default() };
        let mut s = study();
        assert_eq!(s.due(&cfg, 2), Some(24.0));
        // the same frame is applied once
        assert_eq!(s.due(&cfg, 2), None);
        s.applied_revision(5);
        s.observe(5);
        s.record(true);
        assert_eq!(s.state(2), Some(SampleState::Solved));
        assert_eq!(s.due(&cfg, 3), Some(26.0));
        s.applied_revision(6);
        s.record(false);
        assert_eq!(s.state(3), Some(SampleState::Failed));
        assert_eq!(s.state(4), None);
        // an outside edit makes every sample stale and the frame cooks again
        s.observe(9);
        assert_eq!(s.state(2), Some(SampleState::Stale));
        assert_eq!(s.state(3), Some(SampleState::Stale));
        assert_eq!(s.due(&cfg, 3), Some(26.0));
        // redefining the sweep does the same
        s.applied_revision(10);
        s.record(true);
        s.redefine();
        assert_eq!(s.state(3), Some(SampleState::Stale));
        // a disabled sweep drives nothing
        s.enabled = false;
        assert_eq!(s.due(&cfg, 4), None);
    }
}
