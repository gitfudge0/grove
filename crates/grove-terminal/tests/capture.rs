//! `#[ignore]`d fixture recorder. Spawns a command on a real PTY, records every
//! byte read off the master, and writes `tests/fixtures/<label>.bin` plus the
//! `<label>.meta.json` sidecar.
//!
//! ```text
//! GROVE_CAPTURE_CMD="tmux new-session -A -s cap" GROVE_CAPTURE_LABEL=claude-tmux \
//!   GROVE_CAPTURE_SECS=45 cargo test -p grove-terminal --test capture -- --ignored --nocapture
//! ```
//!
//! Env vars:
//! - `GROVE_CAPTURE_CMD`   (required) command line, split on whitespace
//! - `GROVE_CAPTURE_LABEL` (required) fixture label / file stem
//! - `GROVE_CAPTURE_SECS`  capture duration, default 20
//! - `GROVE_CAPTURE_ROWS` / `GROVE_CAPTURE_COLS` initial geometry, default 34x120
//! - `GROVE_CAPTURE_ALT`   `1` to mark the fixture alt-screen in the sidecar
//! - `GROVE_CAPTURE_HOW`   free-text provenance note for the sidecar
//! - `GROVE_CAPTURE_RESIZE` resize schedule `ms:RxC;ms:RxC;...` driven against
//!   the PTY master, for the resize-storm fixtures
//!
//! This is test-only tooling: it unwraps freely.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::manual_assert,
    clippy::format_push_string,
    clippy::redundant_closure
)]
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

const MAX_FIXTURE_BYTES: usize = 2 * 1024 * 1024;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn write_fixture(label: &str, bytes: &[u8], rows: u16, cols: u16, alt: bool, how: &str) {
    let dir = fixtures_dir();
    fs_err::create_dir_all(&dir).unwrap();
    let bytes = if bytes.len() > MAX_FIXTURE_BYTES {
        eprintln!(
            "capture: truncating {label} from {} to {MAX_FIXTURE_BYTES} bytes",
            bytes.len()
        );
        &bytes[..MAX_FIXTURE_BYTES]
    } else {
        bytes
    };
    fs_err::write(dir.join(format!("{label}.bin")), bytes).unwrap();
    let meta = serde_json::json!({
        "label": label,
        "rows": rows,
        "cols": cols,
        "alt_screen": alt,
        "recorded": "2026-07-31",
        "how": how,
    });
    fs_err::write(
        dir.join(format!("{label}.meta.json")),
        format!("{}\n", serde_json::to_string_pretty(&meta).unwrap()),
    )
    .unwrap();
    eprintln!("capture: wrote {label}.bin ({} bytes)", bytes.len());
}

/// Parse `ms:RxC;ms:RxC` into `(delay_from_previous, rows, cols)` steps.
fn parse_resize_schedule(s: &str) -> Vec<(u64, u16, u16)> {
    s.split(';')
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let (ms, geom) = p.split_once(':').unwrap();
            let (r, c) = geom.split_once('x').unwrap();
            (
                ms.trim().parse().unwrap(),
                r.trim().parse().unwrap(),
                c.trim().parse().unwrap(),
            )
        })
        .collect()
}

#[test]
#[ignore = "records a fixture from a live PTY; run manually"]
fn capture() {
    let cmdline = std::env::var("GROVE_CAPTURE_CMD").expect("GROVE_CAPTURE_CMD");
    let label = std::env::var("GROVE_CAPTURE_LABEL").expect("GROVE_CAPTURE_LABEL");
    let secs: u64 = env_or("GROVE_CAPTURE_SECS", "20").parse().unwrap();
    let rows: u16 = env_or("GROVE_CAPTURE_ROWS", "34").parse().unwrap();
    let cols: u16 = env_or("GROVE_CAPTURE_COLS", "120").parse().unwrap();
    let alt = env_or("GROVE_CAPTURE_ALT", "0") == "1";
    let how = env_or("GROVE_CAPTURE_HOW", &cmdline);
    let schedule = parse_resize_schedule(&env_or("GROVE_CAPTURE_RESIZE", ""));

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut parts = cmdline.split_whitespace();
    let mut cmd = CommandBuilder::new(parts.next().unwrap());
    for a in parts {
        cmd.arg(a);
    }
    cmd.env("TERM", "xterm-256color");
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }

    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let done = Arc::new(AtomicBool::new(false));

    let mut reader = pair.master.try_clone_reader().unwrap();
    let rbuf = Arc::clone(&buf);
    let rdone = Arc::clone(&done);
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut g = rbuf.lock().unwrap();
                    g.extend_from_slice(&chunk[..n]);
                    if g.len() > MAX_FIXTURE_BYTES {
                        break;
                    }
                }
            }
        }
        rdone.store(true, Ordering::SeqCst);
    });

    // `MasterPty` is `Send` but not `Sync`, so the resize schedule runs on this
    // thread, interleaved with the capture wait.
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut steps = schedule.into_iter();
    let mut next_step = steps.next();
    while Instant::now() < deadline && !done.load(Ordering::SeqCst) {
        match next_step.take() {
            Some((ms, r, c)) => {
                std::thread::sleep(Duration::from_millis(ms));
                let _ = pair.master.resize(PtySize {
                    rows: r,
                    cols: c,
                    pixel_width: 0,
                    pixel_height: 0,
                });
                next_step = steps.next();
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    let bytes = buf.lock().unwrap().clone();
    // Sidecar geometry is the *initial* size; storms replay resizes in-test.
    write_fixture(&label, &bytes, rows, cols, alt, &how);
    assert!(!bytes.is_empty(), "capture produced no bytes");
}

/// Generates the two synthetic fixtures (`sgr-torture`, `activity-snippets`).
/// No PTY involved — deterministic byte streams built in code.
#[test]
#[ignore = "regenerates committed synthetic fixtures"]
fn generate_synthetic() {
    write_fixture(
        "sgr-torture",
        &sgr_torture_bytes(),
        24,
        80,
        false,
        "generated by tests/capture.rs::generate_synthetic (sgr_torture_bytes)",
    );
    write_fixture(
        "activity-snippets",
        &activity_snippet_bytes(),
        24,
        80,
        false,
        "generated by tests/capture.rs::generate_synthetic; screens transcribed from src/gui/activity.rs",
    );
}

fn sgr_torture_bytes() -> Vec<u8> {
    let mut o = String::new();
    o.push_str("\x1b[2J\x1b[H");
    for i in 0..8 {
        o.push_str(&format!("\x1b[{}mF{i}\x1b[0m", 30 + i));
    }
    for i in 0..8 {
        o.push_str(&format!("\x1b[{}mB{i}\x1b[0m", 40 + i));
    }
    o.push_str("\r\n");
    for i in 0..8 {
        o.push_str(&format!("\x1b[{}mf{i}\x1b[0m", 90 + i));
    }
    for i in 0..8 {
        o.push_str(&format!("\x1b[{}mb{i}\x1b[0m", 100 + i));
    }
    o.push_str("\r\n");
    // 256-color boundaries: cube edges and grayscale ramp bounds.
    for idx in [0u16, 7, 8, 15, 16, 231, 232, 255] {
        o.push_str(&format!("\x1b[38;5;{idx}m#\x1b[48;5;{idx}m@\x1b[0m "));
    }
    o.push_str("\r\n");
    // Truecolor.
    for (r, g, b) in [(0, 0, 0), (255, 255, 255), (13, 37, 210), (200, 5, 99)] {
        o.push_str(&format!(
            "\x1b[38;2;{r};{g};{b}mT\x1b[48;2;{r};{g};{b}mU\x1b[0m "
        ));
    }
    o.push_str("\r\n");
    // Bold on/off, inverse on/off, and both together.
    o.push_str("\x1b[1mBOLD\x1b[22m plain \x1b[7mINVERSE\x1b[27m plain ");
    o.push_str("\x1b[1;7;31;44mBOTH\x1b[0m\r\n");
    // Bells x3.
    o.push_str("bells\x07\x07\x07\r\n");
    // OSC title sets: 0 (icon+title), 1 (icon), 2 (title).
    o.push_str("\x1b]0;first-title\x07");
    o.push_str("\x1b]1;icon-name\x07");
    o.push_str("\x1b]2;final-title\x07");
    // CJK + Nerd Font glyphs.
    o.push_str("wide: 日本語テキスト 中文字符\r\n");
    o.push_str("nerd: \u{e0b0}\u{f09b}\u{f120}\u{e725}\u{f07b} ok\r\n");
    // Alt-screen toggle with content on each side.
    o.push_str("\x1b[?1049hALT SCREEN CONTENT\r\n");
    o.push_str("\x1b[1;32malt green\x1b[0m\r\n");
    o.push_str("\x1b[?1049l");
    o.push_str("back on primary\r\n");
    o.into_bytes()
}

/// The per-agent screen snippets the iced activity classifier keys off
/// (`src/gui/activity.rs`), replayed as a deterministic byte stream. Each
/// screen is preceded by a clear + cursor-home so it parses standalone.
fn activity_snippet_bytes() -> Vec<u8> {
    let screens: &[&str] = &[
        // Claude: working spinner line.
        "\u{2726} Cogitating… (12s · ↓ 1.2k tokens · esc to interrupt)\n",
        // Claude: idle prompt box.
        "╭──────────────────────────────────────────╮\n│ > Try \"how does this work?\"              │\n╰──────────────────────────────────────────╯\n  ? for shortcuts",
        // Claude: permission prompt (needs you).
        "Do you want to make this edit to main.rs?\n❯ 1. Yes\n  2. Yes, and don't ask again\n  3. No, and tell Claude what to do differently (esc)",
        // Claude: bash permission prompt.
        "Bash command\n  cargo test\n\nDo you want to proceed?\n❯ 1. Yes\n  2. No, and tell Claude what to do differently (esc)",
        // Codex: working.
        "• Working (esc to interrupt)\n  thinking about the terminal model",
        // Codex: idle composer.
        "▌ send a message\n  ⏎ send   ⇧⏎ newline   ⌃C quit",
        // Codex: approval prompt.
        "Allow command?\n  rm -rf target\n> 1. Yes  2. Yes, don't ask again  3. No",
        // Generic shell prompt (idle, no agent).
        "user@host ~/dev/grove (main) $ ",
        // Long streaming-ish output body.
        "Running 42 tests\ntest term::tests::snapshot ... ok\ntest term::tests::resize ... ok\ntest result: ok. 42 passed; 0 failed\n",
    ];
    let mut o = Vec::new();
    for s in screens {
        o.extend_from_slice(b"\x1b[2J\x1b[H");
        for line in s.split('\n') {
            o.extend_from_slice(line.as_bytes());
            o.extend_from_slice(b"\r\n");
        }
    }
    o
}
