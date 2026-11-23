//! Server-sent events: the line parser, the decoder interface both protocols implement, and
//! the loop that feeds one into the other from any source of lines.

#[cfg(test)]
use std::io::BufRead;

use crate::{LlmError, StreamEvent};

/// One dispatched event.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SseEvent {
    /// The `event:` field; empty when absent.
    pub event: String,
    /// The `data:` lines joined with `\n`.
    pub data: String,
}

/// Incremental parser following the WHATWG rules, minus `id` and `retry`.
#[derive(Default)]
pub(crate) struct SseParser {
    event: String,
    data: String,
    has_data: bool,
    started: bool,
}

impl SseParser {
    /// Feeds one line without its `\n`. Returns an event when a blank line dispatches one.
    pub fn line(&mut self, line: &str) -> Option<SseEvent> {
        let mut line = line.strip_suffix('\r').unwrap_or(line);
        if !self.started {
            self.started = true;
            line = line.strip_prefix('\u{feff}').unwrap_or(line);
        }
        if line.is_empty() {
            return self.flush();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
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

    /// Dispatches a pending event (blank line or end of stream).
    pub fn flush(&mut self) -> Option<SseEvent> {
        let event = std::mem::take(&mut self.event);
        if !std::mem::take(&mut self.has_data) {
            return None;
        }
        Some(SseEvent { event, data: std::mem::take(&mut self.data) })
    }
}

/// A protocol's translation of SSE events into [`StreamEvent`]s.
pub(crate) trait Decoder {
    /// Handles one event. Returns `true` once the response is complete and `Stop` was emitted.
    fn event(&mut self, ev: &SseEvent, emit: &mut dyn FnMut(StreamEvent)) -> Result<bool, LlmError>;

    /// The body ended before [`Decoder::event`] returned `true`: emit `Stop` if the response
    /// is complete enough, or fail.
    fn eof(&mut self, emit: &mut dyn FnMut(StreamEvent)) -> Result<(), LlmError>;
}

/// Where lines come from: a reader in tests, a reader thread behind a channel on the network.
pub(crate) trait LineSource {
    /// The next line including its `\n`, or `None` at the end of the body.
    fn next_line(&mut self) -> Result<Option<Vec<u8>>, LlmError>;
}

/// Lines straight from a [`BufRead`]: canned transcripts in tests.
#[cfg(test)]
pub(crate) struct ReaderLines<R>(pub R);

#[cfg(test)]
impl<R: BufRead> LineSource for ReaderLines<R> {
    fn next_line(&mut self) -> Result<Option<Vec<u8>>, LlmError> {
        let mut buf = Vec::new();
        match self.0.read_until(b'\n', &mut buf) {
            Ok(0) => Ok(None),
            Ok(_) => Ok(Some(buf)),
            Err(e) => Err(LlmError::Network(format!("reading the response failed: {e}"))),
        }
    }
}

/// Feeds every line of `source` through the parser into `decoder` until the response is
/// complete, the body ends, or the source fails (which includes cancellation).
pub(crate) fn drive(
    source: &mut dyn LineSource,
    decoder: &mut dyn Decoder,
    emit: &mut dyn FnMut(StreamEvent),
) -> Result<(), LlmError> {
    let mut parser = SseParser::default();
    loop {
        let line = source.next_line()?;
        let ev = match &line {
            None => parser.flush(),
            Some(bytes) => {
                // Lines end at '\n', so a multi-byte character is never split here.
                let text = String::from_utf8_lossy(bytes);
                parser.line(text.strip_suffix('\n').unwrap_or(&text))
            }
        };
        if let Some(ev) = ev
            && decoder.event(&ev, emit)?
        {
            return Ok(());
        }
        if line.is_none() {
            return decoder.eof(emit);
        }
    }
}

/// Runs `decoder` over a whole body held by `reader` and collects the events.
#[cfg(test)]
pub(crate) fn decode_all(reader: impl BufRead, decoder: &mut dyn Decoder) -> Result<Vec<StreamEvent>, LlmError> {
    let mut events = Vec::new();
    drive(&mut ReaderLines(reader), decoder, &mut |e| events.push(e))?;
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::StopReason;

    #[test]
    fn events_comments_multiline_data_and_crlf() {
        let mut p = SseParser::default();
        assert_eq!(p.line("\u{feff}: keep-alive"), None);
        assert_eq!(p.line("event: ping\r"), None);
        assert_eq!(p.line("data: {\"a\":\r"), None);
        assert_eq!(p.line("data:1}"), None);
        assert_eq!(p.line("id: 7"), None);
        assert_eq!(p.line("\r"), Some(SseEvent { event: "ping".into(), data: "{\"a\":\n1}".into() }));
        // A blank line without data dispatches nothing and resets the event name.
        assert_eq!(p.line("event: x"), None);
        assert_eq!(p.line(""), None);
        assert_eq!(p.line("data"), None);
        assert_eq!(p.line(""), Some(SseEvent { event: String::new(), data: String::new() }));
        assert_eq!(p.line("data: [DONE]"), None);
        assert_eq!(p.flush(), Some(SseEvent { event: String::new(), data: "[DONE]".into() }));
    }

    /// Counts events and records their data.
    #[derive(Default)]
    struct Record(Vec<String>);

    impl Decoder for Record {
        fn event(&mut self, ev: &SseEvent, _: &mut dyn FnMut(StreamEvent)) -> Result<bool, LlmError> {
            self.0.push(ev.data.clone());
            Ok(ev.data == "stop")
        }
        fn eof(&mut self, emit: &mut dyn FnMut(StreamEvent)) -> Result<(), LlmError> {
            emit(StreamEvent::Stop(StopReason::EndTurn));
            Ok(())
        }
    }

    /// Hands out a body in fixed-size pieces so lines arrive split across reads.
    struct Trickle<'a>(&'a [u8]);

    impl std::io::Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.0.len().min(3).min(buf.len());
            buf[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Ok(n)
        }
    }

    #[test]
    fn lines_split_across_reads_and_a_last_event_without_blank_line() {
        let body = "data: h\u{e9}llo\r\n\r\n: c\ndata: a\ndata: b\n\ndata: tail";
        let reader = std::io::BufReader::with_capacity(4, Trickle(body.as_bytes()));
        let mut d = Record::default();
        let events = decode_all(reader, &mut d).unwrap();
        assert_eq!(d.0, vec!["h\u{e9}llo", "a\nb", "tail"]);
        assert_eq!(events, vec![StreamEvent::Stop(StopReason::EndTurn)]);
    }

    #[test]
    fn a_finished_decoder_stops_reading() {
        let mut d = Record::default();
        let events = decode_all("data: stop\n\ndata: more\n\n".as_bytes(), &mut d).unwrap();
        assert_eq!(d.0, vec!["stop"]);
        assert!(events.is_empty());
    }
}
