//! ACP framing over stdio.
//!
//! The agent endpoint speaks newline-delimited JSON, but it also accepts
//! LSP-style `Content-Length` framing and answers in whatever framing it
//! detected from the first frame it read. A client therefore only has to
//! choose one framing and keep using it.
//!
//! This implementation always *writes* newline-delimited JSON and *reads*
//! either framing, detecting which one the peer used from its first frame.
//! That matches the agent endpoint: it detects from what we send, so if we
//! send ndjson it will answer in ndjson.

/// The framing used to encode frames on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framing {
    /// One JSON object per line.
    Ndjson,
    /// A `Content-Length` header followed by the payload.
    ContentLength,
}

/// A frame decoder that accepts both framings.
#[derive(Debug, Default)]
pub struct FrameReader {
    buffer: Vec<u8>,
    framing: Option<Framing>,
}

impl FrameReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// The framing detected so far, if any frame has been decoded.
    pub fn framing(&self) -> Option<Framing> {
        self.framing
    }

    /// Feeds bytes into the reader and returns every complete frame.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut frames = Vec::new();

        loop {
            if self.framing.is_none() {
                match self.detect() {
                    Some(framing) => self.framing = Some(framing),
                    // Not enough bytes to tell yet.
                    None => break,
                }
            }
            match self.read_frame() {
                Some(Some(frame)) => frames.push(frame),
                // A malformed header block was dropped; keep reading.
                Some(None) => continue,
                None => break,
            }
        }

        frames
    }

    /// Decides which framing the peer is using.
    ///
    /// The agent endpoint looks at the first 32 bytes: a `Content-Length`
    /// header means that framing, anything else that starts with `{` means
    /// ndjson. The same rule is applied here so both sides agree.
    fn detect(&self) -> Option<Framing> {
        let preview_len = self.buffer.len().min(32);
        let preview = String::from_utf8_lossy(&self.buffer[..preview_len]).to_string();

        let trimmed = preview.trim_start();
        if trimmed.len() != preview.len() {
            // Leading whitespace is allowed before a JSON object, but the
            // header check below is case-insensitive and position tolerant.
        }
        let header_like = trimmed
            .split(|c: char| c == ':' || c.is_whitespace())
            .next()
            .unwrap_or("")
            .eq_ignore_ascii_case("content-length");
        if header_like {
            return Some(Framing::ContentLength);
        }
        if !trimmed.starts_with('{') {
            // Neither a header nor JSON yet: wait for more bytes, unless a
            // newline shows the peer is sending something unparseable.
            if !self.buffer.contains(&b'\n') {
                return None;
            }
            return Some(Framing::Ndjson);
        }
        Some(Framing::Ndjson)
    }

    /// Reads one frame. `Some(None)` means a malformed frame was dropped.
    fn read_frame(&mut self) -> Option<Option<String>> {
        match self.framing {
            Some(Framing::ContentLength) => self.read_content_length_frame(),
            _ => self.read_line_frame(),
        }
    }

    fn read_line_frame(&mut self) -> Option<Option<String>> {
        let newline = self.buffer.iter().position(|byte| *byte == b'\n')?;
        let line: Vec<u8> = self.buffer.drain(..=newline).collect();
        let text = String::from_utf8_lossy(&line[..line.len() - 1]).trim().to_owned();
        if text.is_empty() {
            return Some(None);
        }
        Some(Some(text))
    }

    fn read_content_length_frame(&mut self) -> Option<Option<String>> {
        let header_end = find_header_end(&self.buffer)?;
        let (header_len, body_start) = header_end;

        let header = String::from_utf8_lossy(&self.buffer[..header_len]).to_string();
        let length = parse_content_length(&header);

        let Some(length) = length else {
            // A malformed header block cannot be framed; drop it so the
            // connection does not stay stuck.
            self.buffer.drain(..body_start);
            return Some(None);
        };

        if self.buffer.len() < body_start + length {
            return None;
        }
        let payload: Vec<u8> = self
            .buffer
            .drain(..body_start + length)
            .skip(body_start)
            .collect();
        let text = String::from_utf8_lossy(&payload).trim().to_owned();
        if text.is_empty() {
            return Some(None);
        }
        Some(Some(text))
    }
}

/// Encodes a message in the given framing.
pub fn serialize(message: &str, framing: Framing) -> Vec<u8> {
    match framing {
        Framing::Ndjson => format!("{message}\n").into_bytes(),
        Framing::ContentLength => {
            format!("Content-Length: {}\r\n\r\n{}", message.len(), message).into_bytes()
        }
    }
}

/// Finds the end of a `Content-Length` header block.
///
/// Returns the header length (excluding the separator) and the offset where
/// the payload starts.
fn find_header_end(buffer: &[u8]) -> Option<(usize, usize)> {
    let separator = b"\r\n\r\n";
    let position = find_bytes(buffer, separator)?;
    Some((position, position + separator.len()))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Reads the `Content-Length` value out of a header block.
fn parse_content_length(header: &str) -> Option<usize> {
    for line in header.lines() {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            return value.trim().parse::<usize>().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_newline_delimited_frames() {
        let mut reader = FrameReader::new();
        let frames = reader.push(b"{\"a\":1}\n{\"b\":2}\n");
        assert_eq!(frames, vec!["{\"a\":1}".to_string(), "{\"b\":2}".to_string()]);
        assert_eq!(reader.framing(), Some(Framing::Ndjson));
    }

    #[test]
    fn waits_for_a_complete_line() {
        let mut reader = FrameReader::new();
        assert!(reader.push(b"{\"a\":").is_empty());
        let frames = reader.push(b"1}\n");
        assert_eq!(frames, vec!["{\"a\":1}".to_string()]);
    }

    #[test]
    fn reads_content_length_frames() {
        let mut reader = FrameReader::new();
        let payload = "{\"a\":1}";
        let header = format!("Content-Length: {}\r\n\r\n", payload.len());
        let mut chunk = header.into_bytes();
        chunk.extend_from_slice(payload.as_bytes());

        let frames = reader.push(&chunk);
        assert_eq!(frames, vec![payload.to_string()]);
        assert_eq!(reader.framing(), Some(Framing::ContentLength));
    }

    #[test]
    fn counts_content_length_in_bytes_not_characters() {
        let mut reader = FrameReader::new();
        let payload = "{\"text\":\"中文\"}";
        let byte_len = payload.len();
        let header = format!("Content-Length: {byte_len}\r\n\r\n");
        let mut chunk = header.into_bytes();
        chunk.extend_from_slice(payload.as_bytes());

        let frames = reader.push(&chunk);
        assert_eq!(frames, vec![payload.to_string()]);
    }

    #[test]
    fn waits_for_the_full_content_length_payload() {
        let mut reader = FrameReader::new();
        let payload = "{\"a\":1}";
        let header = format!("Content-Length: {}\r\n\r\n", payload.len());
        let mut chunk = header.into_bytes();
        chunk.extend_from_slice(&payload.as_bytes()[..3]);

        assert!(reader.push(&chunk).is_empty());
        let frames = reader.push(&payload.as_bytes()[3..]);
        assert_eq!(frames, vec![payload.to_string()]);
    }

    #[test]
    fn serializes_in_both_framings() {
        assert_eq!(
            serialize("{\"a\":1}", Framing::Ndjson),
            b"{\"a\":1}\n".to_vec()
        );
        assert_eq!(
            serialize("{\"a\":1}", Framing::ContentLength),
            b"Content-Length: 7\r\n\r\n{\"a\":1}".to_vec()
        );
    }

    #[test]
    fn drops_a_malformed_header_block_instead_of_stalling() {
        let mut reader = FrameReader::new();
        let payload = "{\"a\":1}";
        let mut chunk = b"Content-Length: abc\r\n\r\n".to_vec();
        chunk.extend_from_slice(format!("Content-Length: {}\r\n\r\n", payload.len()).as_bytes());
        chunk.extend_from_slice(payload.as_bytes());

        // The malformed block is dropped, the framing stays content-length,
        // and the next well-formed frame is still read.
        let frames = reader.push(&chunk);
        assert_eq!(frames, vec![payload.to_string()]);
    }
}
