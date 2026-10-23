//! Time slider logic (spec 2.8 and 3.10): segment, playback modes and the
//! time display formats. Frames drive study sweeps, not keyframe animation.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayMode {
    Loop,
    Once,
    PingPong,
}

impl PlayMode {
    pub const ALL: [PlayMode; 3] = [PlayMode::Loop, PlayMode::Once, PlayMode::PingPong];

    pub fn label(self) -> &'static str {
        match self {
            PlayMode::Loop => "Loop",
            PlayMode::Once => "Play once",
            PlayMode::PingPong => "Ping-pong",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeFormat {
    Frames,
    Timecode,
    FramesTicks,
    MinSecTicks,
}

impl TimeFormat {
    pub const ALL: [TimeFormat; 4] = [
        TimeFormat::Frames,
        TimeFormat::Timecode,
        TimeFormat::FramesTicks,
        TimeFormat::MinSecTicks,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TimeFormat::Frames => "Frames",
            TimeFormat::Timecode => "Min:Sec:Frame",
            TimeFormat::FramesTicks => "Frame:Ticks",
            TimeFormat::MinSecTicks => "Min:Sec:Ticks",
        }
    }
}

pub const TICKS_PER_SECOND: i64 = 4800;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeConfig {
    pub start: i64,
    pub end: i64,
    pub fps: u32,
    pub speed: f64,
    pub mode: PlayMode,
    pub format: TimeFormat,
}

impl Default for TimeConfig {
    fn default() -> Self {
        Self {
            start: 0,
            end: 100,
            fps: 30,
            speed: 1.0,
            mode: PlayMode::Loop,
            format: TimeFormat::Frames,
        }
    }
}

impl TimeConfig {
    pub fn length(&self) -> i64 {
        self.end - self.start
    }

    pub fn frame_count(&self) -> i64 {
        self.length() + 1
    }

    pub fn seconds(&self, frame: i64) -> f64 {
        frame as f64 / self.fps.max(1) as f64
    }

    pub fn clamp(&self, frame: i64) -> i64 {
        frame.clamp(self.start, self.end.max(self.start))
    }

    /// Frame reached `n` frames after starting playback at `from`, and
    /// whether playback continues.
    pub fn advance(&self, from: i64, n: i64) -> (i64, bool) {
        let len = self.length();
        if len <= 0 {
            return (self.start, false);
        }
        let rel = (from - self.start) + n.max(0);
        match self.mode {
            PlayMode::Loop => (self.start + rel.rem_euclid(len + 1), true),
            PlayMode::Once => {
                if rel >= len {
                    (self.end, false)
                } else {
                    (self.start + rel, true)
                }
            }
            PlayMode::PingPong => {
                let m = rel.rem_euclid(2 * len);
                (self.start + if m <= len { m } else { 2 * len - m }, true)
            }
        }
    }

    /// Whole frames elapsed after `seconds` of wall-clock playback.
    pub fn frames_elapsed(&self, seconds: f64) -> i64 {
        (seconds.max(0.0) * self.fps as f64 * self.speed).floor() as i64
    }

    pub fn format(&self, frame: i64) -> String {
        format_frame(frame, self.fps, self.format)
    }

    /// Slider caption such as `25/100`.
    pub fn slider_text(&self, frame: i64) -> String {
        format!("{} / {}", self.format(frame), self.format(self.end))
    }
}

pub fn format_frame(f: i64, fps: u32, fmt: TimeFormat) -> String {
    let fps = fps.max(1) as i64;
    let s = f.div_euclid(fps);
    let r = f - s * fps;
    match fmt {
        TimeFormat::Frames => f.to_string(),
        TimeFormat::Timecode => format!("{}:{}:{}", s.div_euclid(60), s.rem_euclid(60), r),
        TimeFormat::FramesTicks => format!("{f}:0"),
        TimeFormat::MinSecTicks => format!(
            "{}:{}:{}",
            s.div_euclid(60),
            s.rem_euclid(60),
            r * TICKS_PER_SECOND / fps
        ),
    }
}

/// Playback state driven by the UI clock.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Playback {
    pub playing: bool,
    pub frame: i64,
    started_at: f64,
    started_frame: i64,
}

impl Playback {
    pub fn play(&mut self, now: f64) {
        self.playing = true;
        self.started_at = now;
        self.started_frame = self.frame;
    }

    pub fn stop(&mut self) {
        self.playing = false;
    }

    pub fn toggle(&mut self, now: f64) {
        if self.playing {
            self.stop()
        } else {
            self.play(now)
        }
    }

    /// Advance from the wall clock; returns true when the frame changed.
    pub fn tick(&mut self, cfg: &TimeConfig, now: f64) -> bool {
        if !self.playing {
            return false;
        }
        let n = cfg.frames_elapsed(now - self.started_at);
        let (f, go_on) = cfg.advance(self.started_frame, n);
        self.playing = go_on;
        let changed = f != self.frame;
        self.frame = f;
        changed
    }

    pub fn set_frame(&mut self, cfg: &TimeConfig, f: i64, now: f64) {
        self.frame = cfg.clamp(f);
        self.started_frame = self.frame;
        self.started_at = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_time() {
        let c = TimeConfig::default();
        assert_eq!(c.frame_count(), 101);
        assert!((c.seconds(100) - 3.3333).abs() < 1e-3);
        assert!((c.seconds(45) - 1.5).abs() < 1e-12);
        let c24 = TimeConfig { fps: 24, ..c };
        assert!((c24.seconds(36) - 1.5).abs() < 1e-12);
        let c25 = TimeConfig { fps: 25, ..c };
        assert!((c25.seconds(100) - 4.0).abs() < 1e-12);
        assert_eq!(format_frame(35, 30, TimeFormat::Frames), "35");
        assert_eq!(format_frame(35, 30, TimeFormat::Timecode), "0:1:5");
        assert_eq!(format_frame(35, 30, TimeFormat::FramesTicks), "35:0");
        assert_eq!(format_frame(35, 30, TimeFormat::MinSecTicks), "0:1:800");
        assert_eq!(c.slider_text(25), "25 / 100");
    }

    #[test]
    fn golden_ping_pong() {
        let c = TimeConfig {
            end: 10,
            mode: PlayMode::PingPong,
            ..Default::default()
        };
        let seq: Vec<i64> = (0..=25).map(|n| c.advance(0, n).0).collect();
        let want: Vec<i64> = (0..=10).chain((0..=9).rev()).chain(1..=5).collect();
        assert_eq!(seq, want);
    }

    #[test]
    fn loop_and_once() {
        let c = TimeConfig::default();
        assert_eq!(c.advance(100, 1), (0, true));
        let once = TimeConfig {
            mode: PlayMode::Once,
            ..c
        };
        assert_eq!(once.advance(90, 20), (100, false));
        // Virtual clock: 10,000 frames of playback never leave the segment.
        let mut p = Playback::default();
        p.play(0.0);
        for i in 0..10_000 {
            p.tick(&c, i as f64 / 30.0);
            assert!((0..=100).contains(&p.frame));
        }
    }
}
