# ADR 0007: Project Brain (Markdown + SQLite Index)

## Context
Agents start every task without memory of the project's conventions, past decisions or code that
already solves a similar problem elsewhere. Users want that knowledge to be readable, editable,
shareable through git, and searchable by agents.

## Decision
1. **Markdown files are the source of truth (D10).** Notes, decisions, patterns and the project
   brief are `.md` files with YAML frontmatter and `[[wikilinks]]`, stored in `<repo>/.tag/brain/`
   (committable, D9) and `~/.config/tag/brain/` (global). They open in any editor and in Obsidian's
   graph view.
2. **SQLite (FTS5) is a rebuildable cache** at `<state>/brain.sqlite`, reindexed incrementally by
   mtime. Deleting it loses nothing.
3. **Patterns point at code, not copies.** `tag brain pin` records repo-relative paths and the
   commit; `brain.get_pattern` reads them with `git show <commit>:<path>` and
   `brain.import_pattern` copies them into `<worktree>/.tag/ref/<slug>/` (git-excluded) for a
   worker to adapt.
4. **Bootstrap and feedback.** `tag brain init` generates a brief from the README, manifests,
   layout and git history (never overwriting `brief.md`). The brief is injected into conductor and
   worker prompts (capped at 4 KB); agents can search and write notes over the control plane.

## Consequences
- Brains merge through normal git workflows; IDs must be unique across all roots, and duplicates
  are reported by the indexer.
- Search quality is FTS5 keyword search, not semantic search; good titles and tags matter.
- Pattern files depend on the source commit still existing in the source repository.
