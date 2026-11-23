//! Control methods (§4.5): names, tiers, and typed params/results.
//!
//! Every method is a zero-sized type implementing [`Method`]. [`CATALOG`] lists them all with
//! their tier and an LLM-oriented description; the server uses it for tier checks and
//! `tag mcp` uses it to build `tools/list`.

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tag_core::control::{Role, Tier};
use tag_orchestrator::{MergePolicy, ReportStatus, Run, Task, TaskState, Visibility};

/// Control protocol version spoken by this build.
pub const PROTOCOL_VERSION: u32 = 1;

/// A control method with typed params and result.
pub trait Method {
    /// Wire name, e.g. `panes.list`.
    const NAME: &'static str;
    /// Params object.
    type Params: Serialize + DeserializeOwned + JsonSchema;
    /// Result value.
    type Result: Serialize + DeserializeOwned;
}

/// Static description of a method.
#[derive(Debug, Clone, Copy)]
pub struct MethodInfo {
    pub name: &'static str,
    /// `None` only for `tag.hello`, which is always allowed.
    pub tier: Option<Tier>,
    /// Roles that may call it at all (before scope checks).
    pub roles: &'static [Role],
    /// One-paragraph description written for an LLM reader.
    pub description: &'static str,
    /// JSON Schema for the params object.
    pub schema: fn() -> schemars::schema::RootSchema,
}

const ALL: &[Role] = &[Role::External, Role::Conductor, Role::Worker];
const NOT_WORKER: &[Role] = &[Role::External, Role::Conductor];
const CONDUCTOR_ONLY: &[Role] = &[Role::Conductor];
const WORKER_ONLY: &[Role] = &[Role::Worker];

macro_rules! methods {
    ($( $ty:ident = $name:literal, $tier:expr, $roles:expr, $params:ty => $result:ty, $desc:literal; )*) => {
        $(
            #[doc = $desc]
            #[derive(Debug, Clone, Copy)]
            pub struct $ty;
            impl Method for $ty {
                const NAME: &'static str = $name;
                type Params = $params;
                type Result = $result;
            }
        )*
        /// Every control method.
        pub const CATALOG: &[MethodInfo] = &[
            $( MethodInfo {
                name: $name,
                tier: $tier,
                roles: $roles,
                description: $desc,
                schema: || schemars::schema_for!($params),
            }, )*
        ];
    };
}

/// Look up a method by wire name.
pub fn method_info(name: &str) -> Option<&'static MethodInfo> {
    CATALOG.iter().find(|m| m.name == name)
}

// ---------------------------------------------------------------------------------------------
// Shared value types
// ---------------------------------------------------------------------------------------------

/// Empty params / result object (`{}`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

/// A project known to TAG.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub root: String,
    pub active: bool,
    pub workspaces: usize,
    pub agents: usize,
}

/// A workspace (tab) and its panes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub id: usize,
    pub name: String,
    pub panes: Vec<u32>,
    pub active: bool,
}

/// A pane as seen over the control plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub id: u32,
    pub title: String,
    /// `shell`, `agent`, `tmux`, `applet`, `donut`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub visibility: Visibility,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// A git worktree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorktreeInfo {
    pub name: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub is_main: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// tag.hello
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HelloParams {
    /// Client name, e.g. `claude-desktop`, `mcp`, `ctl`.
    pub client: String,
    /// Scoped token from `TAG_CONTROL_TOKEN`, if any.
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default = "default_protocol")]
    pub protocol: u32,
}

fn default_protocol() -> u32 {
    PROTOCOL_VERSION
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloResult {
    pub protocol: u32,
    pub tag_version: String,
    pub client: String,
    pub role: Role,
    pub tiers: Vec<Tier>,
    pub run_scope: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Read methods
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectFilter {
    /// Project id or name. Defaults to all projects / the active project.
    #[serde(default)]
    pub project: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PanesListParams {
    #[serde(default)]
    pub project: Option<String>,
    /// Workspace id.
    #[serde(default)]
    pub workspace: Option<usize>,
    /// Pane kind filter: `shell`, `agent`, `tmux`, `applet`, `donut`.
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PaneReadParams {
    /// Pane id.
    pub pane: u32,
    /// Number of trailing lines to return.
    #[serde(default = "default_lines")]
    pub lines: usize,
    /// Byte cap applied from the end.
    #[serde(default = "default_read_bytes")]
    pub max_bytes: usize,
    #[serde(default = "default_true")]
    pub strip_ansi: bool,
}

fn default_lines() -> usize {
    80
}
fn default_read_bytes() -> usize {
    16384
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneReadResult {
    pub text: String,
    pub truncated: bool,
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PaneRef {
    /// Pane id.
    pub pane: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentStatusResult {
    pub status: Option<String>,
    /// When the status was last observed to change (RFC 3339).
    pub since: Option<String>,
    pub last_hook: Option<String>,
    pub tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageSummaryParams {
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub run: Option<String>,
    /// RFC 3339 timestamp or `YYYY-MM-DD`. Defaults to today.
    #[serde(default)]
    pub since: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageBucket {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageSummary {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub cost_usd: f64,
    pub by_profile: BTreeMap<String, UsageBucket>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorktreeDiffParams {
    /// Worktree name or path.
    pub worktree: String,
    #[serde(default = "default_diff_bytes")]
    pub max_bytes: usize,
    #[serde(default)]
    pub stat_only: bool,
}

fn default_diff_bytes() -> usize {
    65536
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffResult {
    pub stat: String,
    pub diff: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunListParams {
    #[serde(default)]
    pub project: Option<String>,
    /// Only runs that are not done/failed/cancelled.
    #[serde(default)]
    pub active_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunRef {
    /// Run id.
    pub run: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BrainSearchParams {
    /// Free-text query.
    pub query: String,
    #[serde(default)]
    pub project: Option<String>,
    /// `brief`, `note`, `decision` or `pattern`.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    10
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrainHit {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub project: Option<String>,
    pub snippet: String,
    pub score: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BrainGetParams {
    /// Document id. Alternatively give `project` + `kind: "brief"`.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrainDoc {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub frontmatter: serde_json::Value,
    pub body: String,
    pub links: Vec<String>,
    #[serde(default)]
    pub backlinks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BrainGetPatternParams {
    pub id: String,
    #[serde(default = "default_true")]
    pub include_files: bool,
    #[serde(default = "default_diff_bytes")]
    pub max_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatternFile {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatternResult {
    pub pattern: BrainDoc,
    pub files: Vec<PatternFile>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SubscribeParams {
    /// Event kinds to receive (`pane_status_changed`, `pane_exited`, `task_state_changed`,
    /// `run_state_changed`, `attention_added`). Default: all.
    #[serde(default)]
    pub kinds: Option<Vec<String>>,
    /// Only events for this run.
    #[serde(default)]
    pub run: Option<String>,
    /// Only events for this pane.
    #[serde(default)]
    pub pane: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubscribeResult {
    pub subscribed: bool,
}

// ---------------------------------------------------------------------------------------------
// Act methods
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorktreeSpec {
    /// Create the worktree if it doesn't exist.
    #[serde(default = "default_true")]
    pub create: bool,
    /// Worktree name (directory under `.worktrees/`, branch of the same name).
    pub name: String,
    /// Base branch/commit. Defaults to the current HEAD.
    #[serde(default)]
    pub base: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SpawnParams {
    /// Agent profile name from config (`claude`, `codex`, `agy`, ...) or `shell`.
    pub profile: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub workspace: Option<usize>,
    /// Working directory. Defaults to the worktree (if any) or the project root.
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub worktree: Option<WorktreeSpec>,
    /// Opening prompt, delivered per the profile's `prompt_delivery`.
    #[serde(default)]
    pub prompt: Option<String>,
    /// `hidden`, `card` or `pane`. Default `card`.
    #[serde(default = "default_visibility")]
    #[schemars(with = "String")]
    pub visibility: Visibility,
    /// Vault secret names to inject into the child environment. Values are never returned.
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub task: Option<String>,
}

fn default_visibility() -> Visibility {
    Visibility::Card
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpawnResult {
    pub pane: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PaneSendParams {
    pub pane: u32,
    /// Text to paste into the pane.
    pub text: String,
    /// Press Enter after the text.
    #[serde(default = "default_true")]
    pub submit: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SetVisibilityParams {
    pub pane: u32,
    /// `hidden`, `card` or `pane`.
    #[schemars(with = "String")]
    pub visibility: Visibility,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentWaitParams {
    pub pane: u32,
    /// Statuses that end the wait.
    #[serde(default = "default_until")]
    pub until: Vec<String>,
    #[serde(default = "default_wait_timeout")]
    pub timeout_s: u64,
}

fn default_until() -> Vec<String> {
    ["done", "awaiting_input", "exited", "failed"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}
fn default_wait_timeout() -> u64 {
    600
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaitResult {
    pub status: String,
    /// `status` (matched a wanted status), `already` (was already in it), or `exited`.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorktreeCreateParams {
    #[serde(default)]
    pub project: Option<String>,
    pub name: String,
    #[serde(default)]
    pub base: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorktreeCreateResult {
    pub path: String,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestsRunParams {
    /// Worktree name or path.
    pub worktree: String,
    /// Command to run. Defaults to `test_command` from `.tag/project.toml`.
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default = "default_test_timeout")]
    pub timeout_s: u64,
}

fn default_test_timeout() -> u64 {
    1800
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestRunResult {
    pub exit_code: i32,
    pub duration_s: f64,
    /// Last lines of output (ANSI-stripped, capped).
    pub tail: String,
    /// HEAD commit the tests ran against.
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunCreateParams {
    /// What the run should achieve.
    pub goal: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub conductor_profile: Option<String>,
    #[serde(default)]
    pub worker_profiles: Option<Vec<String>>,
    /// `ask` or `auto`.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub merge_policy: Option<MergePolicy>,
    /// Budget in USD (0 = unlimited).
    #[serde(default)]
    pub budget: Option<f64>,
    /// Spawn a conductor agent for the run.
    #[serde(default)]
    pub spawn_conductor: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunCreateResult {
    pub run: Run,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskAddParams {
    pub run: String,
    /// Task id; generated (`t1`, `t2`, ...) if omitted.
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    /// Full, self-contained instructions for the worker.
    pub prompt: String,
    #[serde(default)]
    pub acceptance: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Worker profile; defaults to the run's first worker profile.
    #[serde(default)]
    pub profile: Option<String>,
    /// Dependents may start once this task is approved (not yet merged).
    #[serde(default)]
    pub parallel_ok: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskUpdateParams {
    pub run: String,
    pub task: String,
    /// New state (`approved`, `changes_requested`, `skipped`, `failed`, `queued`, ...).
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub state: Option<TaskState>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub acceptance: Option<Vec<String>>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskReportParams {
    /// What was done (or what's blocking).
    pub summary: String,
    /// `done` or `blocked`.
    #[schemars(with = "String")]
    pub status: ReportStatus,
    #[serde(default)]
    pub notes: Option<String>,
    /// Ignored for scoped workers (taken from the token).
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub task: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BrainNoteParams {
    #[serde(default)]
    pub project: Option<String>,
    pub title: String,
    pub body: String,
    /// `note`, `decision` or `pattern`.
    #[serde(default = "default_note_kind")]
    pub kind: String,
    #[serde(default)]
    pub links: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_note_kind() -> String {
    "note".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdResult {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BrainImportParams {
    /// Pattern id.
    pub id: String,
    /// Worktree name or path to import into.
    pub worktree: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathResult {
    pub path: String,
}

// ---------------------------------------------------------------------------------------------
// Dangerous methods
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorktreeMergeParams {
    /// Worktree name or path. Alternatively `run` + `task`.
    #[serde(default)]
    pub worktree: Option<String>,
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub task: Option<String>,
    /// Target branch. Defaults to the main worktree's current branch.
    #[serde(default)]
    pub into: Option<String>,
    /// Require the last passing `tests.run` to match the branch HEAD.
    #[serde(default = "default_true")]
    pub require_tests: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MergeResult {
    pub merged_commit: String,
    /// True if the merge was queued for human approval instead of applied.
    #[serde(default)]
    pub queued: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorktreeRemoveParams {
    pub worktree: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunCancelParams {
    pub run: String,
    #[serde(default = "default_true")]
    pub kill_workers: bool,
}

// ---------------------------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------------------------

use Tier::{Act as A, Dangerous as D, Read as R};

methods! {
    Hello = "tag.hello", None, ALL, HelloParams => HelloResult,
        "Authenticate the connection. Must be the first call.";
    ProjectsList = "projects.list", Some(R), NOT_WORKER, Empty => Vec<ProjectInfo>,
        "List projects TAG knows about, with workspace and agent counts.";
    WorkspacesList = "workspaces.list", Some(R), NOT_WORKER, ProjectFilter => Vec<WorkspaceInfo>,
        "List workspaces (tabs) and the pane ids in each.";
    PanesList = "panes.list", Some(R), ALL, PanesListParams => Vec<PaneInfo>,
        "List panes with kind, agent profile, status (idle/running/awaiting_input/done/exited/failed), visibility, cwd and run/task binding.";
    PaneRead = "pane.read", Some(R), ALL, PaneReadParams => PaneReadResult,
        "Read the last lines of a pane's screen and scrollback as plain text (ANSI stripped, byte-capped from the end).";
    AgentStatus = "agent.status", Some(R), ALL, PaneRef => AgentStatusResult,
        "Get an agent pane's current status and when it last changed.";
    UsageSummaryM = "usage.summary", Some(R), NOT_WORKER, UsageSummaryParams => UsageSummary,
        "Token usage and estimated cost, optionally filtered by run or start time.";
    WorktreeList = "worktree.list", Some(R), NOT_WORKER, ProjectFilter => Vec<WorktreeInfo>,
        "List git worktrees of the project, with the task bound to each.";
    WorktreeDiff = "worktree.diff", Some(R), ALL, WorktreeDiffParams => DiffResult,
        "Diff of a worktree's branch against its base plus uncommitted changes: full --stat, then whole-file hunks up to max_bytes.";
    RunList = "run.list", Some(R), NOT_WORKER, RunListParams => Vec<Run>,
        "List orchestration runs.";
    RunGet = "run.get", Some(R), NOT_WORKER, RunRef => Run,
        "Get one orchestration run with all its tasks.";
    BrainSearch = "brain.search", Some(R), ALL, BrainSearchParams => Vec<BrainHit>,
        "Full-text search of the project and global brain (briefs, notes, decisions, reusable patterns). Use before designing something new.";
    BrainGet = "brain.get", Some(R), ALL, BrainGetParams => BrainDoc,
        "Get one brain document by id, or a project's brief with {project, kind: \"brief\"}.";
    BrainGetPattern = "brain.get_pattern", Some(R), ALL, BrainGetPatternParams => PatternResult,
        "Get a pattern and the source files it points to, as of the commit it was pinned at.";
    EventsSubscribe = "events.subscribe", Some(R), NOT_WORKER, SubscribeParams => SubscribeResult,
        "Subscribe this connection to state events; they arrive as `event` notifications.";
    PaneSpawn = "pane.spawn", Some(A), NOT_WORKER, SpawnParams => SpawnResult,
        "Start an agent (or shell) in a new pane, optionally in a new git worktree, with an opening prompt.";
    PaneSend = "pane.send", Some(A), NOT_WORKER, PaneSendParams => Empty,
        "Paste text into a pane and press Enter (unless submit=false). Use to answer an agent that is awaiting input.";
    PaneSetVisibility = "pane.set_visibility", Some(A), NOT_WORKER, SetVisibilityParams => Empty,
        "Show a pane as a tiled pane, as a card, or hide it. Never restarts the process.";
    AgentWait = "agent.wait", Some(A), NOT_WORKER, AgentWaitParams => WaitResult,
        "Block until an agent pane reaches one of the given statuses, or the timeout passes (Timeout error).";
    WorktreeCreate = "worktree.create", Some(A), NOT_WORKER, WorktreeCreateParams => WorktreeCreateResult,
        "Create a git worktree under .worktrees/<name> on a new branch.";
    TestsRun = "tests.run", Some(A), ALL, TestsRunParams => TestRunResult,
        "Run the project's test command in a worktree and return exit code, duration, output tail and the tested commit.";
    RunCreate = "run.create", Some(A), NOT_WORKER, RunCreateParams => RunCreateResult,
        "Create an orchestration run for a goal.";
    TaskAdd = "task.add", Some(A), CONDUCTOR_ONLY, TaskAddParams => Task,
        "Add a task to your run's plan. The prompt must be self-contained: the worker cannot see your conversation.";
    TaskUpdate = "task.update", Some(A), CONDUCTOR_ONLY, TaskUpdateParams => Task,
        "Change a task: approve it, request changes, skip it, retry it, or edit its prompt.";
    TaskReport = "task.report", Some(A), WORKER_ONLY, TaskReportParams => Empty,
        "Report your task as done (with a summary) or blocked (saying exactly what you need).";
    BrainNote = "brain.note", Some(A), ALL, BrainNoteParams => IdResult,
        "Save a note, decision or pattern to the project brain.";
    BrainImportPattern = "brain.import_pattern", Some(A), ALL, BrainImportParams => PathResult,
        "Copy a pattern's source files into <worktree>/.tag/ref/<slug>/ for reference (git-excluded).";
    RunApprovePlan = "run.approve_plan", Some(D), NOT_WORKER, RunRef => Run,
        "Approve a run's plan so workers start.";
    WorktreeMerge = "worktree.merge", Some(D), NOT_WORKER, WorktreeMergeParams => MergeResult,
        "Merge a worktree/task branch. Requires passing tests at HEAD; aborts cleanly on conflict (Conflict error lists files).";
    WorktreeRemove = "worktree.remove", Some(D), NOT_WORKER, WorktreeRemoveParams => Empty,
        "Remove a git worktree.";
    PaneKill = "pane.kill", Some(D), NOT_WORKER, PaneRef => Empty,
        "Kill a pane's process and close it.";
    RunCancel = "run.cancel", Some(D), NOT_WORKER, RunCancelParams => Run,
        "Cancel a run, optionally killing its worker panes.";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_names_are_unique_and_tiers_set() {
        let mut names: Vec<_> = CATALOG.iter().map(|m| m.name).collect();
        names.sort();
        let len = names.len();
        names.dedup();
        assert_eq!(len, names.len());
        for m in CATALOG {
            assert_eq!(m.tier.is_none(), m.name == "tag.hello", "{}", m.name);
            let schema = serde_json::to_value((m.schema)()).unwrap();
            assert_eq!(
                schema["type"], "object",
                "{} params must be an object",
                m.name
            );
        }
    }

    #[test]
    fn defaults_apply() {
        let p: PaneReadParams = serde_json::from_str(r#"{"pane":3}"#).unwrap();
        assert_eq!((p.lines, p.max_bytes, p.strip_ansi), (80, 16384, true));
        let s: SpawnParams = serde_json::from_str(r#"{"profile":"claude"}"#).unwrap();
        assert_eq!(s.visibility, Visibility::Card);
        let w: AgentWaitParams = serde_json::from_str(r#"{"pane":1}"#).unwrap();
        assert_eq!(w.timeout_s, 600);
        assert_eq!(w.until.len(), 4);
    }
}
