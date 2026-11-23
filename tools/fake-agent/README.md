# fake-agent

Scripted stand-in for AI agent CLIs (claude/codex/agy) used by TAG end-to-end
tests. Run it inside a PTY.

    fake-agent --script script.txt ["prompt"]

The script comes from `--script <path>`, else env `FAKE_AGENT_SCRIPT` (path),
else env `FAKE_AGENT_SCRIPT_TEXT` (inline text). A trailing positional argument
prints `PROMPT: <prompt>` first.

## Commands (one per line; `#` comments and blank lines ignored)

| Command | Effect |
| --- | --- |
| `print <text>` | print line; `\e` becomes ESC |
| `wait_input <regex>` | read stdin lines until one matches, echo `INPUT: <line>`; EOF exits 3 |
| `write <path> <text>` | write file relative to cwd (parents created); `\n` becomes newline |
| `commit <msg>` | `git add -A && git commit -m <msg>` in cwd; failure exits 4 |
| `bel` | write BEL (`\x07`) |
| `osc9 <text>` | write `ESC ] 9 ; <text> BEL` |
| `notify <event>` | run `$FAKE_AGENT_NOTIFY_BIN` (default `cockpit-notify`) `--event <event>`; prints `NOTIFY_FAILED` on error |
| `ctl <method> <json>` | run `$FAKE_AGENT_TAG_BIN` (default `tag`) `ctl call <method> <json>`; stdout lines printed as `CTL: ...`; failure prints `CTL_ERROR: <stderr>` |
| `ctl_save <var> <method> <json>` | like `ctl`, storing trimmed stdout in `<var>`; if `<var>` is `name.a.b`, stdout is parsed as JSON and `result["a"]["b"]` is stored as `name` |
| `sleep <ms>` | sleep |
| `exit <code>` | exit |

`${var}` and `${env:NAME}` are substituted in the arguments of later lines.
End of script without `exit` exits 0.

## Example

    # announce, do work, commit, notify
    print \e[1mworking\e[0m
    wait_input ^(y|yes)$
    write src/hello.txt hello\nworld
    commit add hello
    ctl_save pane.pane pane.spawn {"cmd":"echo hi"}
    ctl pane.send {"pane":"${pane}","text":"x"}
    osc9 done
    notify agent-done
    exit 0
