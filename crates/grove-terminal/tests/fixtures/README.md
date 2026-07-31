# grove-terminal golden fixtures

Each fixture is a raw byte dump of everything read off a PTY master (`<label>.bin`)
plus a sidecar (`<label>.meta.json`: `label`, `rows`, `cols`, `alt_screen`,
`recorded`, `how`). Bytes are never text — escape sequences must survive
round-tripping.

Recorded with `tests/capture.rs` (an `#[ignore]`d test):

```bash
GROVE_CAPTURE_CMD="tmux new-session -A -s cap" GROVE_CAPTURE_LABEL=<label> \
GROVE_CAPTURE_SECS=45 GROVE_CAPTURE_ROWS=34 GROVE_CAPTURE_COLS=120 \
  cargo test -p grove-terminal --test capture -- --ignored --nocapture --exact capture
```

All live captures were driven **headlessly**: the recorder attaches to a tmux
session in its own PTY, and a second process injects activity with
`tmux send-keys -t <session> ...`. Resize storms use `GROVE_CAPTURE_RESIZE`
(`ms:RxC;ms:RxC;...`), which resizes the PTY master itself.

## Corpus

| label | rows×cols | alt | what it exercises | how it was recorded |
| --- | --- | --- | --- | --- |
| `claude-tmux` | 34×120 | yes | agent TUI: boxed composer, spinner/status line, two prompts with streaming responses, 256-color + truecolor SGR | recorder on `tmux new-session -A -s capclaude`; `send-keys 'claude' Enter`, then "Write a haiku about terminal emulators." and "Now count from 1 to 20, one word each."; 60 s |
| `codex-tmux` | 34×120 | yes | second agent TUI: Codex banner, "esc to interrupt" working line, MCP warning banners | recorder on `tmux new-session -A -s capcodex`; `send-keys 'codex' Enter`, then one turn ("Write a haiku about terminal emulators."); 55 s. The turn was still streaming when the window closed — that is deliberate, it captures the *working* state. |
| `tmux-bare` | 34×120 | yes | plain tmux + shell: status-bar redraws, `ls -la` colors, a horizontal and a vertical split, `resize-pane`, `select-pane` | recorder on `tmux new-session -A -s capbare`; `send-keys` drove `ls -la`, `git status --short`, a 40-line echo loop, `split-window -h`, `resize-pane -L 20`, `select-pane -L`, `split-window -v`, `seq 1 30`; 22 s |
| `vim` | 34×120 | yes | heavy alt-screen + absolute cursor addressing: `vim Cargo.toml`, `:set number`, `G`/`gg`, `C-d`/`C-u`, `/workspace`, `n`, `:set nonumber`, `:q!` | recorder on `tmux new-session -A -s capvim` + `send-keys`; 25 s |
| `resize-storm` | 34×120 | yes | **alt-screen** resize regime: 24 PTY resizes alternating 24×70 / 34×120 every 400 ms while a 400-line loop streams | recorder on `tmux new-session -A -s capstorm` with `GROVE_CAPTURE_RESIZE` set to the alternating schedule; 25 s |
| `resize-storm-primary` | 34×120 | no | **primary-screen** resize regime — evidence for the asserted reflow divergence (alacritty rewraps on resize, vt100 does not) | bare shell, no tmux: `bash <scratch>/primary_stream.sh` (500 × `echo` + `sleep 0.04`) with the same 24-step `GROVE_CAPTURE_RESIZE` schedule; 22 s |
| `sgr-torture` | 24×80 | no | synthetic: all 16 ANSI named colors fg+bg (normal and bright), 256-color boundaries (0/7/8/15/16/231/232/255), truecolor `38;2;r;g;b`, bold on/off, INVERSE on/off and combined, three `\x07` bells, OSC 0/1/2 title sets, CJK + Nerd Font glyphs, `\x1b[?1049h`/`l` toggle | generated in code — `cargo test -p grove-terminal --test capture -- --ignored --nocapture generate_synthetic` |
| `activity-snippets` | 24×80 | no | the per-agent screens `src/gui/activity.rs`'s classifier keys off (Claude spinner/idle box/permission prompts, Codex working/composer/approval, a bare shell prompt, test output), each prefixed with clear + cursor-home so it parses deterministically | generated in code, same command as above |

## Post-processing

The five tmux captures were truncated at the **last** `\x1b[?1049l`, which is the
teardown tmux emits when the recorder kills the client. Without that trim every
tmux fixture would end on the *primary* screen and the alt-screen resize test
would be testing the wrong regime. Nothing else is edited.

Fixtures are capped at 2 MB by the recorder (`MAX_FIXTURE_BYTES`); long captures
are truncated rather than committed at full length.
