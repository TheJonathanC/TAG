//! Brain browser overlay (`Leader+B`): glue between `tag-brain` and the
//! data-only widget in `tag-ui`.

#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use crossterm::event::{KeyCode, KeyEvent};
use tag_ui::{BrainFocus, BrainPreview, BrainResult};

const MAX_RESULTS: usize = 50;

/// Open the overlay: open the brain index for the focused repo and reindex.
pub(crate) fn open_brain_overlay(state: &mut AppState) {
    state.brain_state.open();
    let repo = resolve_repo_root(&candidate_repo_dir(state));
    match crate::brain_cli::open_brain(Some(&repo)) {
        Ok((brain, _)) => {
            state.brain = Some(brain);
            refresh_brain_results(state);
        }
        Err(e) => {
            state.brain = None;
            state.brain_state.status = Some(format!("brain unavailable: {e}"));
        }
    }
}

/// Re-run the search for the current query (empty query lists documents).
pub(crate) fn refresh_brain_results(state: &mut AppState) {
    let Some(brain) = state.brain.as_ref() else {
        return;
    };
    let q = state.brain_state.query.trim().to_string();
    let results: Result<Vec<BrainResult>, tag_brain::Error> = if q.is_empty() {
        brain.all_docs().map(|docs| {
            docs.into_iter()
                .take(MAX_RESULTS)
                .map(|d| BrainResult {
                    id: d.id,
                    kind: d.kind.as_str().into(),
                    title: d.title,
                    project: d.project,
                    snippet: String::new(),
                })
                .collect()
        })
    } else {
        brain.search(&q, None, None, MAX_RESULTS).map(|hits| {
            hits.into_iter()
                .map(|h| BrainResult {
                    id: h.id,
                    kind: h.kind.as_str().into(),
                    title: h.title,
                    project: h.project,
                    snippet: h.snippet,
                })
                .collect()
        })
    };
    match results {
        Ok(r) => {
            state.brain_state.status = None;
            state.brain_state.set_results(r);
        }
        Err(e) => state.brain_state.status = Some(format!("search failed: {e}")),
    }
    load_selected_preview(state);
}

/// Preview of the highlighted result (or none).
fn load_selected_preview(state: &mut AppState) {
    let id = state.brain_state.selected_result().map(|r| r.id.clone());
    let preview = id.and_then(|id| build_preview(state, &id));
    state.brain_state.set_preview(preview);
}

fn build_preview(state: &AppState, id: &str) -> Option<BrainPreview> {
    let brain = state.brain.as_ref()?;
    let doc = brain.get(id).ok().flatten()?;
    Some(BrainPreview {
        id: doc.frontmatter.id.clone(),
        kind: doc.frontmatter.kind.as_str().into(),
        title: doc.frontmatter.title.clone(),
        project: doc.frontmatter.project.clone(),
        body: doc.body.clone(),
        links: brain.outlinks(id).unwrap_or_default(),
        backlinks: brain.backlinks(id).unwrap_or_default(),
    })
}

/// Follow `id`: show it in the preview pane.
fn navigate_to(state: &mut AppState, id: &str) {
    match build_preview(state, id) {
        Some(p) => {
            state.brain_state.status = None;
            state.brain_state.set_preview(Some(p));
        }
        None => state.brain_state.status = Some(format!("no document `{id}`")),
    }
}

pub(crate) fn handle_brain_overlay_key(state: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            state.brain_state.close();
            state.brain = None;
        }
        KeyCode::Tab | KeyCode::BackTab => state.brain_state.toggle_focus(),
        KeyCode::Up => {
            if state.brain_state.move_selection(-1) {
                load_selected_preview(state);
            }
        }
        KeyCode::Down => {
            if state.brain_state.move_selection(1) {
                load_selected_preview(state);
            }
        }
        KeyCode::Enter => {
            if state.brain_state.focus == BrainFocus::Links {
                if let Some(id) = state.brain_state.selected_link() {
                    navigate_to(state, &id);
                }
            } else if state.brain_state.preview.is_some() {
                // Enter on a result moves to its links.
                state.brain_state.focus = BrainFocus::Links;
            }
        }
        KeyCode::Backspace => {
            state.brain_state.query.pop();
            state.brain_state.focus = BrainFocus::Results;
            state.brain_state.selected = 0;
            refresh_brain_results(state);
        }
        _ => {
            if let Some(c) = text_input_char(&key) {
                state.brain_state.query.push(c);
                state.brain_state.focus = BrainFocus::Results;
                state.brain_state.selected = 0;
                refresh_brain_results(state);
            }
        }
    }
}
