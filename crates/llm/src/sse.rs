//! Minimal Server-Sent Events reader plus the protocol-agnostic streaming driver.

use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{StopReason, StreamEvent};

/// One dispatched SSE event.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SseEvent {
    /// `event:` field; empty when absent (default type "message").
    pub event: String,
    /// `data:` lines joined with `\n`.
    pub data: String,
}

/// Incremental SSE line parser (per the WHATWG spec, minus `id`/`retry`).
#[derive(Default)]
pub(crate) struct SseParser {
    event: String,
    data: String,
    has_data: bool,
}

impl SseParser {
    /// Feed one line (without the trailing newline). Returns an event when a blank line
    /// dispatches one.
    pub fn line(&mut self, line: &str) -> Option<SseEvent> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            return self.flush();
        }
        if line.starts_with(':') {
            return None; // comment / keep-alive
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => self.event = value.to_string(),
            "data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            _ => {}
        }
        None
    }

    /// Dispatch any pending event (blank line or end of stream).
    pub fn flush(&mut self) -> Option<SseEvent> {
        let event = std::mem::take(&mut self.event);
        if !std::mem::take(&mut self.has_data) {
            return None;
        }
        Some(SseEvent {
            event,
            data: std::mem::take(&mut self.data),
        })
    }
}

/// Protocol-specific translation of SSE events into [`StreamEvent`]s.
pub(crate) trait Decoder {
    /// Handle one SSE event. Returns `true` once the turn is complete and
    /// `Stop` has been emitted.
    fn event(&mut self, ev: &SseEvent, emit: &mut dyn FnMut(StreamEvent)) -> anyhow::Result<bool>;

    /// The body ended before [`Decoder::event`] returned `true`: emit the final `Stop`
    /// if the stream is complete enough, otherwise return an error.
    fn eof(&mut self, emit: &mut dyn FnMut(StreamEvent)) -> anyhow::Result<()>;
}

/// Read an SSE body line by line, feeding `decoder`, until the turn completes, the body
/// ends, or `cancel` is set (then emits `Stop(Cancelled)` and returns `Ok`).
pub(crate) fn drive(
    mut reader: impl BufRead,
    decoder: &mut dyn Decoder,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(StreamEvent),
) -> anyhow::Result<()> {
    let mut parser = SseParser::default();
    let mut buf = Vec::with_capacity(4096);
    loop {
        if cancel.load(Ordering::Relaxed) {
            emit(StreamEvent::Stop(StopReason::Cancelled));
            return Ok(());
        }
        buf.clear();
        let n = match reader.read_until(b'\n', &mut buf) {
            Ok(n) => n,
            Err(_) if cancel.load(Ordering::Relaxed) => {
                emit(StreamEvent::Stop(StopReason::Cancelled));
                return Ok(());
            }
            Err(e) => return Err(anyhow::Error::new(e).context("reading response stream")),
        };
        let ev = if n == 0 {
            parser.flush()
        } else {
            let line = String::from_utf8_lossy(&buf);
            parser.line(line.strip_suffix('\n').unwrap_or(&line))
        };
        if let Some(ev) = ev
            && decoder.event(&ev, emit)?
        {
            return Ok(());
        }
        if n == 0 {
            return decoder.eof(emit);
        }
    }
}

/// Test helper: run `decoder` over a fixed list of SSE lines and collect the events.
#[cfg(test)]
pub(crate) fn decode_lines(
    decoder: &mut dyn Decoder,
    lines: &[&str],
) -> anyhow::Result<Vec<StreamEvent>> {
    let body = lines.join("\n");
    let mut events = Vec::new();
    drive(
        std::io::Cursor::new(body),
        decoder,
        &AtomicBool::new(false),
        &mut |e| events.push(e),
    )?;
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_comments_and_multiline_data() {
        let mut p = SseParser::default();
        assert_eq!(p.line(": keep-alive"), None);
        assert_eq!(p.line("event: ping"), None);
        assert_eq!(p.line("data: {\"a\":"), None);
        assert_eq!(p.line("data:1}\r"), None);
        assert_eq!(
            p.line(""),
            Some(SseEvent {
                event: "ping".into(),
                data: "{\"a\":\n1}".into()
            })
        );
        // Blank lines without data dispatch nothing; event name resets.
        assert_eq!(p.line("event: x"), None);
        assert_eq!(p.line(""), None);
        assert_eq!(p.line("data: [DONE]"), None);
        assert_eq!(
            p.flush(),
            Some(SseEvent {
                event: String::new(),
                data: "[DONE]".into()
            })
        );
    }

    struct Count(usize);
    impl Decoder for Count {
        fn event(&mut self, _: &SseEvent, _: &mut dyn FnMut(StreamEvent)) -> anyhow::Result<bool> {
            self.0 += 1;
            Ok(false)
        }
        fn eof(&mut self, emit: &mut dyn FnMut(StreamEvent)) -> anyhow::Result<()> {
            emit(StreamEvent::Stop(StopReason::EndTurn));
            Ok(())
        }
    }

    #[test]
    fn cancel_stops_before_reading() {
        let mut d = Count(0);
        let mut events = Vec::new();
        let cancel = AtomicBool::new(true);
        drive(
            std::io::Cursor::new("data: x\n\n"),
            &mut d,
            &cancel,
            &mut |e| events.push(e),
        )
        .unwrap();
        assert_eq!(d.0, 0);
        assert_eq!(events, vec![StreamEvent::Stop(StopReason::Cancelled)]);
    }

    #[test]
    fn last_event_without_trailing_blank_line_is_dispatched() {
        let mut d = Count(0);
        let events = decode_lines(&mut d, &["data: a", "", "data: b"]).unwrap();
        assert_eq!(d.0, 2);
        assert_eq!(events, vec![StreamEvent::Stop(StopReason::EndTurn)]);
    }
}
