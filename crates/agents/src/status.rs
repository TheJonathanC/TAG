use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};
use tag_core::config::DoneSignal;
use tag_core::event::HookEvent;

use crate::profile::AgentProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentStatus {
    Idle,
    Running,
    AwaitingInput,
    Done,
    Exited,
    /// Process exited non-zero, or explicitly marked failed (e.g. a worker reported `blocked`).
    Failed,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentStatus::Idle => "idle",
            AgentStatus::Running => "running",
            AgentStatus::AwaitingInput => "awaiting-input",
            AgentStatus::Done => "done",
            AgentStatus::Exited => "exited",
            AgentStatus::Failed => "failed",
        }
    }
}

const TAIL_CAP: usize = 4096;

/// Small state machine deciding an agent's status from its profile's `done_signal`s.
///
/// Time is passed in explicitly so tests need no sleeping. A detector built from a profile
/// with no `done_signal` is *disabled* and keeps the legacy behavior (the pane applies hook
/// events directly); it only reports process exit.
///
/// Exit semantics: a non-zero exit is always `Failed`. A zero (or unknown) exit is `Done`
/// when `exit` is among the signals (or the agent is already `Done`), otherwise `Exited`.
pub struct CompletionDetector {
    hook: bool,
    osc: bool,
    idle: Option<Duration>,
    regex: Option<Regex>,
    exit: bool,
    awaiting: Vec<Regex>,
    enabled: bool,
    status: AgentStatus,
    last_output: Option<Instant>,
    output_since_input: bool,
    input_pending: bool,
    tail: String,
    in_osc: Option<Vec<u8>>,
}

impl CompletionDetector {
    pub fn new(
        signals: &[DoneSignal],
        idle_done_secs: u64,
        done_regex: Option<&str>,
        awaiting_patterns: &[String],
    ) -> Self {
        let has = |s: DoneSignal| signals.contains(&s);
        Self {
            hook: has(DoneSignal::Hook),
            osc: has(DoneSignal::Osc),
            idle: (has(DoneSignal::Idle) && idle_done_secs > 0)
                .then(|| Duration::from_secs(idle_done_secs)),
            regex: if has(DoneSignal::Regex) {
                done_regex.and_then(|r| Regex::new(r).ok())
            } else {
                None
            },
            exit: has(DoneSignal::Exit),
            awaiting: awaiting_patterns
                .iter()
                .filter_map(|p| Regex::new(p).ok())
                .collect(),
            enabled: !signals.is_empty(),
            status: AgentStatus::Running,
            last_output: None,
            output_since_input: false,
            input_pending: false,
            tail: String::new(),
            in_osc: None,
        }
    }

    pub fn disabled() -> Self {
        Self::new(&[], 0, None, &[])
    }

    pub fn from_profile(p: &AgentProfile) -> Self {
        Self::new(
            &p.done_signal,
            p.idle_done_secs,
            p.done_regex.as_deref(),
            &p.status_patterns,
        )
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn status(&self) -> AgentStatus {
        self.status
    }

    fn set(&mut self, st: AgentStatus) -> Option<AgentStatus> {
        if self.status == st {
            None
        } else {
            self.status = st;
            Some(st)
        }
    }

    fn awaiting_matches(&self) -> bool {
        self.awaiting.iter().any(|r| r.is_match(&self.tail))
    }

    /// Feed PTY output. Returns the new status if it changed.
    pub fn on_output(&mut self, data: &[u8], now: Instant) -> Option<AgentStatus> {
        if !self.enabled || data.is_empty() {
            return None;
        }
        let (visible, notified) = self.scan(data);
        let mut changed = None;
        if !visible.is_empty() {
            self.last_output = Some(now);
            self.output_since_input = true;
            self.tail.push_str(&visible);
            if self.tail.len() > TAIL_CAP {
                let mut cut = self.tail.len() - TAIL_CAP;
                while !self.tail.is_char_boundary(cut) {
                    cut += 1;
                }
                self.tail.drain(..cut);
            }
            // Output resuming after new input resumes work.
            if self.input_pending && matches!(self.status, AgentStatus::Done | AgentStatus::Idle) {
                self.input_pending = false;
                changed = self.set(AgentStatus::Running);
            }
        }
        if self.status == AgentStatus::Running {
            let regex_hit = self.regex.as_ref().is_some_and(|r| r.is_match(&self.tail));
            let osc_hit = self.osc && notified && self.output_since_input;
            if regex_hit || (osc_hit && !self.awaiting_matches()) {
                self.tail.clear();
                changed = self.set(AgentStatus::Done);
            }
        }
        changed
    }

    /// The user (or orchestrator) sent input to the agent.
    pub fn on_input(&mut self, _now: Instant) {
        if !self.enabled {
            return;
        }
        self.output_since_input = false;
        self.input_pending = true;
        self.tail.clear();
        if self.status == AgentStatus::AwaitingInput {
            self.status = AgentStatus::Running;
        }
    }

    pub fn on_hook(&mut self, ev: &HookEvent) -> Option<AgentStatus> {
        match ev {
            HookEvent::NeedsPermission | HookEvent::NeedsInput => {
                self.set(AgentStatus::AwaitingInput)
            }
            HookEvent::TaskDone if self.hook => self.set(AgentStatus::Done),
            HookEvent::Error => self.set(AgentStatus::Idle),
            HookEvent::Exited => self.set(AgentStatus::Exited),
            _ => None,
        }
    }

    /// Periodic check for the idle signal.
    pub fn tick(&mut self, now: Instant) -> Option<AgentStatus> {
        let idle = self.idle?;
        if self.status != AgentStatus::Running || !self.output_since_input {
            return None;
        }
        let last = self.last_output?;
        if now.saturating_duration_since(last) >= idle && !self.awaiting_matches() {
            return self.set(AgentStatus::Done);
        }
        None
    }

    pub fn on_exit(&mut self, code: Option<i32>) -> Option<AgentStatus> {
        match code {
            Some(c) if c != 0 => self.set(AgentStatus::Failed),
            _ if self.exit || self.status == AgentStatus::Done => self.set(AgentStatus::Done),
            _ => self.set(AgentStatus::Exited),
        }
    }

    /// Splits raw bytes into visible text (ANSI stripped) and whether a notification
    /// (bare BEL or OSC 9 / OSC 777;notify) was seen. Other OSC sequences, including their
    /// BEL terminators, are not notifications.
    fn scan(&mut self, data: &[u8]) -> (String, bool) {
        let mut out = Vec::with_capacity(data.len());
        let mut notified = false;
        let mut i = 0;
        while i < data.len() {
            if let Some(buf) = self.in_osc.as_mut() {
                let b = data[i];
                if b == 0x07 || (b == 0x1b && data.get(i + 1) == Some(&b'\\')) {
                    if buf.starts_with(b"9;") || buf.starts_with(b"777;notify") {
                        notified = true;
                    }
                    self.in_osc = None;
                    i += if b == 0x07 { 1 } else { 2 };
                } else {
                    if buf.len() < 64 {
                        buf.push(b);
                    }
                    i += 1;
                }
                continue;
            }
            match data[i] {
                0x07 => {
                    notified = true;
                    i += 1;
                }
                0x1b => match data.get(i + 1) {
                    Some(b']') => {
                        self.in_osc = Some(Vec::new());
                        i += 2;
                    }
                    Some(b'[') => {
                        i += 2;
                        while i < data.len() && !(0x40..=0x7e).contains(&data[i]) {
                            i += 1;
                        }
                        i += 1;
                    }
                    Some(_) => i += 2,
                    None => i += 1,
                },
                b => {
                    out.push(b);
                    i += 1;
                }
            }
        }
        (String::from_utf8_lossy(&out).into_owned(), notified)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(signals: &[DoneSignal], idle: u64, re: Option<&str>) -> CompletionDetector {
        CompletionDetector::new(signals, idle, re, &["(?i)permission".to_string()])
    }

    #[test]
    fn failed_as_str() {
        assert_eq!(AgentStatus::Failed.as_str(), "failed");
    }

    #[test]
    fn disabled_is_inert() {
        let mut d = CompletionDetector::disabled();
        let t = Instant::now();
        assert_eq!(d.on_output(b"hi\x07", t), None);
        assert_eq!(d.tick(t + Duration::from_secs(999)), None);
        assert!(!d.enabled());
        assert_eq!(d.on_exit(Some(1)), Some(AgentStatus::Failed));
    }

    #[test]
    fn hook_signal() {
        let mut d = det(&[DoneSignal::Hook], 0, None);
        assert_eq!(
            d.on_hook(&HookEvent::NeedsInput),
            Some(AgentStatus::AwaitingInput)
        );
        assert_eq!(d.on_hook(&HookEvent::TaskDone), Some(AgentStatus::Done));
        let mut d = det(&[DoneSignal::Osc], 0, None);
        assert_eq!(d.on_hook(&HookEvent::TaskDone), None);
    }

    #[test]
    fn osc_and_bell() {
        let t = Instant::now();
        let mut d = det(&[DoneSignal::Osc], 0, None);
        // a bell without prior output does not count
        assert_eq!(d.on_output(b"\x07", t), None);
        assert_eq!(d.on_output(b"working\n", t), None);
        assert_eq!(d.on_output(b"\x07", t), Some(AgentStatus::Done));

        let mut d = det(&[DoneSignal::Osc], 0, None);
        d.on_output(b"x", t);
        // a title OSC terminated by BEL is not a notification
        assert_eq!(d.on_output(b"\x1b]0;title\x07", t), None);
        assert_eq!(
            d.on_output(b"\x1b]9;finished\x07", t),
            Some(AgentStatus::Done)
        );
    }

    #[test]
    fn osc_suppressed_when_awaiting_pattern() {
        let t = Instant::now();
        let mut d = det(&[DoneSignal::Osc], 0, None);
        assert_eq!(d.on_output(b"Need permission to run\n\x07", t), None);
    }

    #[test]
    fn idle_signal() {
        let t = Instant::now();
        let mut d = det(&[DoneSignal::Idle], 10, None);
        assert_eq!(d.tick(t + Duration::from_secs(100)), None); // no output yet
        d.on_output(b"work", t);
        assert_eq!(d.tick(t + Duration::from_secs(9)), None);
        assert_eq!(d.tick(t + Duration::from_secs(10)), Some(AgentStatus::Done));
        // new input then output resumes
        d.on_input(t + Duration::from_secs(11));
        assert_eq!(d.tick(t + Duration::from_secs(100)), None);
        assert_eq!(
            d.on_output(b"more", t + Duration::from_secs(12)),
            Some(AgentStatus::Running)
        );
        assert_eq!(d.tick(t + Duration::from_secs(22)), Some(AgentStatus::Done));
    }

    #[test]
    fn idle_not_when_awaiting_input() {
        let t = Instant::now();
        let mut d = det(&[DoneSignal::Idle, DoneSignal::Hook], 5, None);
        d.on_output(b"x", t);
        d.on_hook(&HookEvent::NeedsPermission);
        assert_eq!(d.tick(t + Duration::from_secs(60)), None);
        assert_eq!(d.status(), AgentStatus::AwaitingInput);
    }

    #[test]
    fn regex_signal_strips_ansi() {
        let t = Instant::now();
        let mut d = det(&[DoneSignal::Regex], 0, Some(r"TASK COMPLETE"));
        assert_eq!(d.on_output(b"TASK COM", t), None);
        assert_eq!(d.on_output(b"PLETE \x1b[0m\n", t), Some(AgentStatus::Done));
        let mut d = det(&[DoneSignal::Regex], 0, Some(r"done"));
        assert_eq!(
            d.on_output(b"\x1b[32mdo\x1b[0mne", t),
            Some(AgentStatus::Done)
        );
    }

    #[test]
    fn exit_signal() {
        let mut d = det(&[DoneSignal::Exit], 0, None);
        assert_eq!(d.on_exit(Some(0)), Some(AgentStatus::Done));
        let mut d = det(&[DoneSignal::Exit], 0, None);
        assert_eq!(d.on_exit(Some(2)), Some(AgentStatus::Failed));
        let mut d = det(&[DoneSignal::Osc], 0, None);
        assert_eq!(d.on_exit(Some(0)), Some(AgentStatus::Exited));
    }
}
