//! Webview diagnostics → the Rust log file.
//!
//! `src/lib/log.ts` emits [`CLIENT_LOG_EVENT`] for every front-end error,
//! unhandled rejection and React error-boundary failure. A packaged Tauri build
//! has no devtools, so without this listener all of that is written to a console
//! nobody can read, i.e. deleted with extra steps.
//!
//! Everything arriving here has crossed a trust boundary — it is a JSON blob
//! from a webview, on an event channel any script in that webview can post to —
//! so nothing is taken on faith:
//!
//! * the payload must be an object with a non-empty string `message`, otherwise
//!   the record is dropped (at `debug`, so a broken UI cannot spam the file with
//!   complaints about itself);
//! * control characters are folded to spaces, because a `\n` in a message would
//!   otherwise forge a second, fake log line;
//! * `message` and `detail` are truncated to a fixed number of *characters*
//!   (never bytes: that would split a UTF-8 sequence);
//! * a token-bucket-ish window bounds how many records a runaway render loop can
//!   write per interval, and reports how many it dropped instead of writing them.
//!
//! Every line goes out with the [`WEBVIEW_TARGET`] target, so the file shows
//! `[webview][ERROR] …` and the origin of a line is never ambiguous.

use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Deserialize;
use tauri::{AppHandle, Listener};

/// The event `src/lib/log.ts` emits. Read that file before changing this.
pub const CLIENT_LOG_EVENT: &str = "onyx://client-log";

/// Log target used for everything that came from the webview.
pub const WEBVIEW_TARGET: &str = "webview";

/// Longest message we will write, in characters.
const MAX_MESSAGE_CHARS: usize = 400;
/// Longest `detail` (a stack trace, usually) we will write, in characters.
const MAX_DETAIL_CHARS: usize = 1_200;
/// Rate-limit window.
const BUDGET_WINDOW: Duration = Duration::from_secs(10);
/// Records accepted per [`BUDGET_WINDOW`]. A user-visible failure produces a
/// handful; anything above this is a loop, not a diagnostic.
const BUDGET_PER_WINDOW: u32 = 40;

/// What the front end sends. Extra fields are ignored rather than rejected so a
/// future field cannot silence the whole channel; the ones we use are checked.
#[derive(Deserialize)]
struct RawRecord {
    level: Option<String>,
    message: Option<String>,
    detail: Option<String>,
}

/// A validated, bounded record ready to be logged.
#[derive(Debug, PartialEq, Eq)]
struct ClientRecord {
    level: log::Level,
    /// Message and detail already sanitised and joined.
    text: String,
}

/// Map the front end's severity onto a Rust level.
///
/// `src/lib/log.ts` only emits `warn` and `error`; the other three are accepted
/// because they cost nothing and mean exactly what they say. Anything else is
/// treated as a warning rather than dropped — losing a diagnostic is worse than
/// logging it one level away from where it belongs.
fn level_of(raw: Option<&str>) -> log::Level {
    match raw
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "error" => log::Level::Error,
        "warn" | "warning" => log::Level::Warn,
        "info" => log::Level::Info,
        "debug" => log::Level::Debug,
        "trace" => log::Level::Trace,
        _ => log::Level::Warn,
    }
}

/// Fold control characters to spaces, collapse runs of whitespace and truncate
/// to `max` characters.
fn sanitise(raw: &str, max: usize) -> String {
    let mut out = String::with_capacity(raw.len().min(max * 4) + 16);
    let mut kept = 0usize;
    let mut last_was_space = false;
    let mut truncated = false;
    for ch in raw.chars() {
        // `is_control` catches \n, \r, \t and the C1 range: a newline here would
        // forge a second log line, which is how log files get lied to.
        let ch = if ch.is_control() { ' ' } else { ch };
        if ch == ' ' {
            if last_was_space || out.is_empty() {
                continue;
            }
            last_was_space = true;
        } else {
            last_was_space = false;
        }
        if kept >= max {
            truncated = true;
            break;
        }
        out.push(ch);
        kept += 1;
    }
    while out.ends_with(' ') {
        out.pop();
    }
    if truncated {
        out.push_str(" […truncated]");
    }
    out
}

/// Validate one payload. `Err` carries why it was rejected, for a `debug` line.
fn parse(payload: &str) -> Result<ClientRecord, &'static str> {
    let raw: RawRecord = serde_json::from_str(payload).map_err(|_| "not a JSON object")?;
    let message = sanitise(
        raw.message.as_deref().unwrap_or_default(),
        MAX_MESSAGE_CHARS,
    );
    if message.is_empty() {
        return Err("empty message");
    }
    let detail = raw
        .detail
        .as_deref()
        .map(|d| sanitise(d, MAX_DETAIL_CHARS))
        .filter(|d| !d.is_empty());
    let text = match detail {
        Some(detail) => format!("{message} — {detail}"),
        None => message,
    };
    Ok(ClientRecord {
        level: level_of(raw.level.as_deref()),
        text,
    })
}

/// Sliding-window rate limiter.
#[derive(Debug, Default)]
struct Budget {
    window_start: Option<Instant>,
    used: u32,
    dropped: u32,
}

/// Whether to log this record, plus how many were silently dropped since the
/// last time anything was logged.
#[derive(Debug, PartialEq, Eq)]
struct Verdict {
    accept: bool,
    dropped_since: u32,
}

impl Budget {
    fn admit(&mut self, now: Instant) -> Verdict {
        let fresh = match self.window_start {
            Some(start) => now.duration_since(start) >= BUDGET_WINDOW,
            None => true,
        };
        if fresh {
            self.window_start = Some(now);
            self.used = 1;
            return Verdict {
                accept: true,
                dropped_since: std::mem::take(&mut self.dropped),
            };
        }
        if self.used < BUDGET_PER_WINDOW {
            self.used += 1;
            Verdict {
                accept: true,
                dropped_since: 0,
            }
        } else {
            self.dropped = self.dropped.saturating_add(1);
            Verdict {
                accept: false,
                dropped_since: 0,
            }
        }
    }
}

/// Log one raw payload. Split out from the listener so it can be tested without
/// a webview (there is no display on CI, and no window on a build machine).
fn record(budget: &Mutex<Budget>, payload: &str, now: Instant) {
    let verdict = budget.lock().admit(now);
    if verdict.dropped_since > 0 {
        log::warn!(
            target: WEBVIEW_TARGET,
            "dropped {} diagnostic record(s): the interface is logging faster than {BUDGET_PER_WINDOW} records per {} s",
            verdict.dropped_since,
            BUDGET_WINDOW.as_secs()
        );
    }
    if !verdict.accept {
        return;
    }
    match parse(payload) {
        Ok(rec) => log::log!(target: WEBVIEW_TARGET, rec.level, "{}", rec.text),
        // Detail, not a warning: a malformed payload is a front-end bug, and
        // logging it loudly is how a broken UI fills the disk.
        Err(why) => log::debug!(target: WEBVIEW_TARGET, "ignored a diagnostic record ({why})"),
    }
}

/// Start routing webview diagnostics into the log file. Returns immediately.
pub fn install(app: &AppHandle) {
    let budget = Mutex::new(Budget::default());
    app.listen(CLIENT_LOG_EVENT, move |event| {
        record(&budget, event.payload(), Instant::now());
    });
    log::debug!("listening for {CLIENT_LOG_EVENT}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_front_ends_two_levels_map_onto_rust_levels() {
        // The only two `src/lib/log.ts` can emit.
        assert_eq!(level_of(Some("warn")), log::Level::Warn);
        assert_eq!(level_of(Some("error")), log::Level::Error);
        // Case and padding are not the webview's problem.
        assert_eq!(level_of(Some(" ERROR ")), log::Level::Error);
        // Unknown or missing: kept, as a warning, rather than thrown away.
        assert_eq!(level_of(Some("catastrophe")), log::Level::Warn);
        assert_eq!(level_of(None), log::Level::Warn);
    }

    #[test]
    fn a_newline_cannot_forge_a_second_log_line() {
        let rec = parse(
            r#"{"level":"error","message":"a\n[2026-01-01][INFO] all is well","detail":null}"#,
        )
        .expect("a well-formed record");
        assert!(!rec.text.contains('\n'), "{}", rec.text);
        assert!(!rec.text.contains('\r'), "{}", rec.text);
        assert_eq!(rec.text, "a [2026-01-01][INFO] all is well");
        assert_eq!(rec.level, log::Level::Error);
    }

    #[test]
    fn a_runaway_message_is_truncated_on_a_character_boundary() {
        // 4 000 non-ASCII characters: a byte-wise truncation would split one.
        let payload = serde_json::json!({
            "level": "warn",
            "message": "é".repeat(4_000),
            "detail": "ß".repeat(20_000),
        })
        .to_string();
        let rec = parse(&payload).expect("a well-formed record");
        // Message + detail + the two truncation markers and the separator.
        assert!(
            rec.text.chars().count() < MAX_MESSAGE_CHARS + MAX_DETAIL_CHARS + 64,
            "{} characters got through",
            rec.text.chars().count()
        );
        assert!(rec.text.contains("truncated"), "{}", rec.text);
        // Still valid UTF-8 with whole characters (a `String` guarantees the
        // first, this guarantees we did not mangle the second).
        assert!(rec.text.starts_with("éé"), "{}", rec.text);
    }

    #[test]
    fn malformed_payloads_are_rejected_rather_than_logged() {
        for payload in [
            "",
            "null",
            "[]",
            "\"just a string\"",
            "{}",
            r#"{"level":"error"}"#,
            r#"{"level":"error","message":""}"#,
            // Whitespace and control characters only: nothing left after
            // sanitising, so there is nothing worth a line.
            r#"{"level":"error","message":"  \n\t "}"#,
            "{not json at all",
        ] {
            assert!(parse(payload).is_err(), "{payload:?} was accepted");
        }
        // Unknown extra fields must not reject the record: the front end may
        // grow a field before this file does.
        assert!(parse(r#"{"level":"warn","message":"hi","extra":1,"detail":null}"#).is_ok());
    }

    #[test]
    fn a_render_loop_cannot_write_without_bound() {
        let mut budget = Budget::default();
        let start = Instant::now();
        // The whole budget is accepted.
        for i in 0..BUDGET_PER_WINDOW {
            assert!(budget.admit(start).accept, "record {i} was refused");
        }
        // Everything after it, in the same window, is not.
        for _ in 0..10_000 {
            assert_eq!(
                budget.admit(start),
                Verdict {
                    accept: false,
                    dropped_since: 0
                }
            );
        }
        // The next window opens with a report of what was dropped, so the file
        // says "10 000 records were lost" instead of silently losing them.
        let verdict = budget.admit(start + BUDGET_WINDOW);
        assert!(verdict.accept);
        assert_eq!(verdict.dropped_since, 10_000);
        // ... and the count is not reported twice.
        assert_eq!(budget.admit(start + BUDGET_WINDOW).dropped_since, 0);
    }

    #[test]
    fn recording_never_panics_on_hostile_input() {
        // `record` is what the event callback runs; a panic in there would take
        // the listener down with it.
        let budget = Mutex::new(Budget::default());
        let now = Instant::now();
        for payload in [
            "",
            "{}",
            r#"{"level":42,"message":"typed wrong"}"#,
            r#"{"level":"error","message":"fine"}"#,
            r#"{"message":"\u0000\u0007 bells"}"#,
        ] {
            record(&budget, payload, now);
        }
    }
}
