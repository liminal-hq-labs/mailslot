// A `MakeWriter` wrapper that removes secrets from every line before it is written.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::io::{self, Write};

use tracing_subscriber::fmt::MakeWriter;

/// Wraps a [`MakeWriter`] so that every complete line passes through
/// [`mailslot_core::redact::redact`].
pub struct Redacting<M>(M);

impl<M> Redacting<M> {
	/// Wraps `inner`.
	pub fn new(inner: M) -> Self {
		Self(inner)
	}
}

impl<'a, M: MakeWriter<'a>> MakeWriter<'a> for Redacting<M> {
	type Writer = RedactingWriter<M::Writer>;

	fn make_writer(&'a self) -> Self::Writer {
		RedactingWriter {
			inner: self.0.make_writer(),
			pending: Vec::new(),
		}
	}
}

/// The writer made by [`Redacting`]. It buffers up to the end of a line.
pub struct RedactingWriter<W: Write> {
	inner: W,
	pending: Vec<u8>,
}

impl<W: Write> RedactingWriter<W> {
	fn emit(&mut self, upto: usize) -> io::Result<()> {
		let chunk: Vec<u8> = self.pending.drain(..upto).collect();
		let text = String::from_utf8_lossy(&chunk);
		self.inner
			.write_all(mailslot_core::redact::redact(&text).as_bytes())
	}
}

impl<W: Write> Write for RedactingWriter<W> {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		self.pending.extend_from_slice(buf);
		if let Some(pos) = self.pending.iter().rposition(|b| *b == b'\n') {
			self.emit(pos + 1)?;
		}
		Ok(buf.len())
	}

	fn flush(&mut self) -> io::Result<()> {
		let all = self.pending.len();
		self.emit(all)?;
		self.inner.flush()
	}
}

impl<W: Write> Drop for RedactingWriter<W> {
	fn drop(&mut self) {
		let _ = self.flush();
	}
}

#[cfg(test)]
mod tests {
	use std::sync::{Arc, Mutex};

	use tracing_subscriber::layer::SubscriberExt;

	use super::*;

	#[derive(Clone, Default)]
	struct Sink(Arc<Mutex<Vec<u8>>>);

	impl Write for Sink {
		fn write(&mut self, b: &[u8]) -> io::Result<usize> {
			self.0.lock().unwrap().extend_from_slice(b);
			Ok(b.len())
		}
		fn flush(&mut self) -> io::Result<()> {
			Ok(())
		}
	}

	impl<'a> MakeWriter<'a> for Sink {
		type Writer = Sink;
		fn make_writer(&'a self) -> Sink {
			self.clone()
		}
	}

	#[test]
	fn the_token_never_reaches_the_log_output() {
		let token = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCd";
		let sink = Sink::default();
		let subscriber = tracing_subscriber::registry().with(
			tracing_subscriber::fmt::layer()
				.with_ansi(false)
				.with_writer(Redacting::new(sink.clone())),
		);
		tracing::subscriber::with_default(subscriber, || {
			tracing::info!(path = %format!("/{token}/photo.jpg"), "request");
			tracing::warn!("link mailslot://10.0.0.2:8931/{token}?tls=1 failed token={token}");
		});
		let out = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
		assert!(out.contains("request"), "{out}");
		assert!(!out.contains(token), "{out}");
		assert!(out.contains(mailslot_core::redact::REDACTED));
	}
}
