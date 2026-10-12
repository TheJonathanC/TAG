use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentStatus {
    Idle,
    Running,
    AwaitingInput,
    Done,
    Exited,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentStatus::Idle => "idle",
            AgentStatus::Running => "running",
            AgentStatus::AwaitingInput => "awaiting-input",
            AgentStatus::Done => "done",
            AgentStatus::Exited => "exited",
        }
    }
}
