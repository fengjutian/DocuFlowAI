//! DAP framing: a `Content-Length` header and the payload.
//!
//! The Debug Adapter Protocol uses the same base protocol as the Language
//! Server Protocol: every message is in a frame that says how long it is:
//!
//! ```text
//! Content-Length: 42\r\n
//! \r\n
//! {"seq":1,"type":"request","command":"initialize"}
//! ```
//!
//! The length is in **bytes**, not in characters: a message with Chinese text
//! in it is longer than its character count.

/// A frame decoder.
#[derive(Debug, Default)]
pub struct FrameReader {
    buffer: Vec<u8>,
}

impl FrameReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds bytes into the reader and returns every complete frame.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut frames = Vec::new();
        loop {
            match self.read_frame() {
                Some(Some(frame)) => frames.push(frame),
                // A malformed frame was dropped; keep reading.
                Some(None) => continue,
                None => break,
            }
        }
        frames
    }

    /// Reads one frame. `Some(None)` means a malformed frame was dropped.
    fn read_frame(&mut self) -> Option<Option<String>> {
        let separator = b"\r\n\r\n";
        let header_end = self
            .buffer
            .windows(separator.len())
            .position(|window| window == separator)?;
        let header = String::from_utf8_lossy(&self.buffer[..header_end]).to_string();
        let body_start = header_end + separator.len();

        let Some(length) = parse_content_length(&header) else {
            // A header block without a length cannot be framed; drop it so
            // the connection does not stay stuck.
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

/// Encodes a message in the framing the protocol uses.
pub fn serialize(message: &str) -> Vec<u8> {
    format!("Content-Length: {}\r\n\r\n{message}", message.len()).into_bytes()
}

/// Reads the `Content-Length` value out of a header block.
fn parse_content_length(header: &str) -> Option<usize> {
    for line in header.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            return value.trim().parse().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_frame() {
        let mut reader = FrameReader::new();
        let frames = reader.push(b"Content-Length: 7\r\n\r\n{\"a\":1}");
        assert_eq!(frames, vec!["{\"a\":1}".to_string()]);
    }

    #[test]
    fn waits_for_the_whole_payload() {
        let mut reader = FrameReader::new();
        assert!(reader.push(b"Content-Length: 7\r\n\r\n{\"a\":").is_empty());
        let frames = reader.push(b"1}");
        assert_eq!(frames, vec!["{\"a\":1}".to_string()]);
    }

    #[test]
    fn reads_two_frames_in_one_chunk() {
        let mut reader = FrameReader::new();
        let mut chunk = b"Content-Length: 7\r\n\r\n{\"a\":1}".to_vec();
        chunk.extend_from_slice(b"Content-Length: 7\r\n\r\n{\"b\":2}");
        let frames = reader.push(&chunk);
        assert_eq!(
            frames,
            vec!["{\"a\":1}".to_string(), "{\"b\":2}".to_string()]
        );
    }

    #[test]
    fn counts_the_length_in_bytes_not_characters() {
        let message = "{\"text\":\"中文\"}";
        let mut reader = FrameReader::new();
        let frames = reader.push(&serialize(message));
        assert_eq!(frames, vec![message.to_string()]);
    }

    #[test]
    fn drops_a_header_block_without_a_length() {
        let mut reader = FrameReader::new();
        let mut chunk = b"Content-Type: application/json\r\n\r\n".to_vec();
        chunk.extend_from_slice(b"Content-Length: 7\r\n\r\n{\"a\":1}");
        let frames = reader.push(&chunk);
        assert_eq!(frames, vec!["{\"a\":1}".to_string()]);
    }

    #[test]
    fn serializes_with_the_header() {
        assert_eq!(
            serialize("{\"a\":1}"),
            b"Content-Length: 7\r\n\r\n{\"a\":1}".to_vec()
        );
    }
}
