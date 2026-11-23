//! Draining a child's output pipe on its own thread into a head-and-tail buffer.

use std::collections::VecDeque;
use std::io::{self, Read};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{Builder, JoinHandle};

/// The start and the end of a stream; what lies between them is dropped once the stream
/// outgrows both budgets.
pub(super) struct Capture {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    head_cap: usize,
    tail_cap: usize,
    truncated: bool,
}

impl Capture {
    pub(super) fn new(head_cap: usize, tail_cap: usize) -> Self {
        Self { head: Vec::new(), tail: VecDeque::new(), head_cap, tail_cap, truncated: false }
    }

    /// Adds a chunk read from the pipe. The first `head_cap` bytes are kept for good, the rest
    /// goes through a window of the last `tail_cap` bytes.
    pub(super) fn push(&mut self, chunk: &[u8]) {
        let room = self.head_cap.saturating_sub(self.head.len()).min(chunk.len());
        let (head, rest) = chunk.split_at(room);
        self.head.extend_from_slice(head);
        if rest.is_empty() {
            return;
        }
        self.tail.extend(rest);
        if self.tail.len() > self.tail_cap {
            let excess = self.tail.len() - self.tail_cap;
            self.tail.drain(..excess);
            self.truncated = true;
        }
    }

    /// The captured text (invalid UTF-8 replaced) and whether anything was dropped.
    pub(super) fn take(&mut self) -> (String, bool) {
        let mut bytes = std::mem::take(&mut self.head);
        bytes.extend(self.tail.drain(..));
        (String::from_utf8_lossy(&bytes).into_owned(), self.truncated)
    }
}

/// A running reader thread and the buffer it fills.
pub(super) struct Reader {
    shared: Arc<Mutex<Capture>>,
    handle: JoinHandle<()>,
}

impl Reader {
    /// Starts reading `pipe` until it closes.
    pub(super) fn spawn<R>(mut pipe: R, head_cap: usize, tail_cap: usize) -> io::Result<Reader>
    where
        R: Read + Send + 'static,
    {
        let shared = Arc::new(Mutex::new(Capture::new(head_cap, tail_cap)));
        let sink = Arc::clone(&shared);
        let handle = Builder::new().name("base-process-reader".into()).spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => sink.lock().unwrap_or_else(PoisonError::into_inner).push(&buf[..n]),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        })?;
        Ok(Reader { shared, handle })
    }

    /// Whether the pipe has been read to its end.
    pub(super) fn finished(&self) -> bool {
        self.handle.is_finished()
    }

    /// What has been read so far (everything, once [`finished`](Self::finished)).
    pub(super) fn take(&self) -> (String, bool) {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}
