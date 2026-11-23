//! Golden round-trip tests: every method's params and result serialize to the checked-in
//! fixture in `tests/fixtures/<method>.json`. Run with `UPDATE_FIXTURES=1` to regenerate.

use serde_json::{json, Value};
use std::collections::BTreeSet;
use tag_control::methods::*;

fn run_example(created: &str) -> Value {
    json!({
        "id": "r_20261123_0915_ab12", "project": "a1b2c3d4e5f60718", "goal": "Add OAuth login",
        "state": "planning", "conductor": {"profile": "claude", "pane": 12},
        "merge_policy": "ask", "budget_usd": 5.0, "created_at": created, "updated_at": created,
        "tasks": [{"id": "t1", "title": "API routes", "prompt": "do it", "state": "planned"}]
    })
}

fn task_example() -> Value {
    json!({"id": "t1", "title": "API routes", "prompt": "do it", "state": "planned",
           "acceptance": ["cargo test passes"]})
}

fn check<M: Method>(params: Value, result: Value, seen: &mut BTreeSet<&'static str>) {
    let p: M::Params =
        serde_json::from_value(params).unwrap_or_else(|e| panic!("{} params: {e}", M::NAME));
    let r: M::Result =
        serde_json::from_value(result).unwrap_or_else(|e| panic!("{} result: {e}", M::NAME));
    let canonical = json!({
        "method": M::NAME,
        "params": serde_json::to_value(&p).unwrap(),
        "result": serde_json::to_value(&r).unwrap(),
    });
    // Canonical form must itself round-trip.
    let p2: M::Params = serde_json::from_value(canonical["params"].clone()).unwrap();
    assert_eq!(
        serde_json::to_value(&p2).unwrap(),
        canonical["params"],
        "{}",
        M::NAME
    );
    let r2: M::Result = serde_json::from_value(canonical["result"].clone()).unwrap();
    assert_eq!(
        serde_json::to_value(&r2).unwrap(),
        canonical["result"],
        "{}",
        M::NAME
    );

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{}.json", M::NAME));
    if std::env::var("UPDATE_FIXTURES").is_ok() {
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&canonical).unwrap() + "\n",
        )
        .unwrap();
    }
    let golden: Value = serde_json::from_str(
        &std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing fixture {}", path.display())),
    )
    .unwrap();
    assert_eq!(golden, canonical, "fixture mismatch for {}", M::NAME);
    seen.insert(M::NAME);
}

#[test]
fn every_method_matches_its_golden_fixture() {
    let t = "2026-11-23T09:15:00Z";
    let mut s = BTreeSet::new();
    check::<Hello>(
        json!({"client":"claude-desktop","token":null,"protocol":1}),
        json!({"protocol":1,"tag_version":"0.2.0-beta.1","client":"claude-desktop","role":"external","tiers":["read","act"],"run_scope":null}),
        &mut s,
    );
    check::<ProjectsList>(
        json!({}),
        json!([{"id":"a1b2c3d4e5f60718","name":"tag","root":"/src/tag","active":true,"workspaces":2,"agents":3}]),
        &mut s,
    );
    check::<WorkspacesList>(
        json!({}),
        json!([{"id":0,"name":"main","panes":[1,2],"active":true}]),
        &mut s,
    );
    check::<PanesList>(
        json!({"kind":"agent"}),
        json!([{"id":3,"title":"claude","kind":"agent","profile":"claude","status":"running","visibility":"pane","cwd":"/src/tag"}]),
        &mut s,
    );
    check::<PaneRead>(
        json!({"pane":3}),
        json!({"text":"hello","truncated":false,"status":"idle"}),
        &mut s,
    );
    check::<AgentStatus>(
        json!({"pane":3}),
        json!({"status":"done","since":t,"last_hook":"TaskDone","tokens":null}),
        &mut s,
    );
    check::<UsageSummaryM>(
        json!({"run":"r_1"}),
        json!({"input":10,"output":20,"cached":5,"cost_usd":0.5,"by_profile":{"claude":{"input":10,"output":20,"cached":5,"cost_usd":0.5}}}),
        &mut s,
    );
    check::<WorktreeList>(
        json!({}),
        json!([{"name":"main","path":"/src/tag","branch":"main","is_main":true}]),
        &mut s,
    );
    check::<WorktreeDiff>(
        json!({"worktree":"r_ab12-t1"}),
        json!({"stat":" a | 1 +","diff":"+x","truncated":false}),
        &mut s,
    );
    check::<RunList>(json!({"active_only":true}), json!([run_example(t)]), &mut s);
    check::<RunGet>(
        json!({"run":"r_20261123_0915_ab12"}),
        run_example(t),
        &mut s,
    );
    check::<BrainSearch>(
        json!({"query":"jwt"}),
        json!([{"id":"pat_jwt","kind":"pattern","title":"JWT","project":null,"snippet":"…","score":1.5}]),
        &mut s,
    );
    check::<BrainGet>(
        json!({"id":"pat_jwt"}),
        json!({"id":"pat_jwt","kind":"pattern","title":"JWT","frontmatter":{"tags":["auth"]},"body":"b","links":["dec_x"],"backlinks":[]}),
        &mut s,
    );
    check::<BrainGetPattern>(
        json!({"id":"pat_jwt"}),
        json!({"pattern":{"id":"pat_jwt","kind":"pattern","title":"JWT","frontmatter":{},"body":"b","links":[],"backlinks":[]},"files":[{"path":"a.py","content":"x","truncated":false}]}),
        &mut s,
    );
    check::<EventsSubscribe>(
        json!({"kinds":["pane_status_changed"]}),
        json!({"subscribed":true}),
        &mut s,
    );
    check::<PaneSpawn>(
        json!({"profile":"agy-neon","worktree":{"name":"r_ab12-t1"},"prompt":"go","visibility":"hidden","run":"r_1","task":"t1"}),
        json!({"pane":14,"worktree":{"name":"r_ab12-t1","path":"/src/tag/.worktrees/r_ab12-t1","branch":"r_ab12-t1","is_main":false}}),
        &mut s,
    );
    check::<PaneSend>(json!({"pane":14,"text":"yes"}), json!({}), &mut s);
    check::<PaneSetVisibility>(json!({"pane":14,"visibility":"pane"}), json!({}), &mut s);
    check::<AgentWait>(
        json!({"pane":14,"until":["done"],"timeout_s":30}),
        json!({"status":"done","reason":"status"}),
        &mut s,
    );
    check::<WorktreeCreate>(
        json!({"name":"feature-x"}),
        json!({"path":"/src/tag/.worktrees/feature-x","branch":"feature-x"}),
        &mut s,
    );
    check::<TestsRun>(
        json!({"worktree":"feature-x"}),
        json!({"exit_code":0,"duration_s":1.5,"tail":"ok","commit":"abc123"}),
        &mut s,
    );
    check::<RunCreate>(
        json!({"goal":"Add OAuth login","merge_policy":"auto","budget":5.0}),
        json!({"run":run_example(t)}),
        &mut s,
    );
    check::<TaskAdd>(
        json!({"run":"r_1","title":"API routes","prompt":"do it","acceptance":["cargo test passes"]}),
        task_example(),
        &mut s,
    );
    check::<TaskUpdate>(
        json!({"run":"r_1","task":"t1","state":"approved"}),
        task_example(),
        &mut s,
    );
    check::<TaskReport>(json!({"summary":"done","status":"done"}), json!({}), &mut s);
    check::<BrainNote>(
        json!({"title":"Run notes","body":"…"}),
        json!({"id":"note_run-notes"}),
        &mut s,
    );
    check::<BrainImportPattern>(
        json!({"id":"pat_jwt","worktree":"r_ab12-t1"}),
        json!({"path":"/src/tag/.worktrees/r_ab12-t1/.tag/ref/jwt"}),
        &mut s,
    );
    check::<RunApprovePlan>(json!({"run":"r_1"}), run_example(t), &mut s);
    check::<WorktreeMerge>(
        json!({"run":"r_1","task":"t1"}),
        json!({"merged_commit":"def456","queued":false}),
        &mut s,
    );
    check::<WorktreeRemove>(json!({"worktree":"feature-x"}), json!({}), &mut s);
    check::<PaneKill>(json!({"pane":14}), json!({}), &mut s);
    check::<RunCancel>(json!({"run":"r_1"}), run_example(t), &mut s);

    let all: BTreeSet<_> = CATALOG.iter().map(|m| m.name).collect();
    assert_eq!(all, s, "every catalog method needs a golden fixture");
}

#[test]
fn empty_params_reject_unknown_fields() {
    assert!(serde_json::from_value::<Empty>(json!({"bogus": 1})).is_err());
}
