//! Server-sent events parser that accepts arbitrary byte chunks.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some((end, sep)) = find_separator(&self.buf) {
            let block = self.buf[..end].to_vec();
            self.buf.drain(..end + sep);
            if let Some(event) = parse_block(&block) {
                out.push(event);
            }
        }
        out
    }

    /// Flushes a final event that wasn't followed by a blank line.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let block = std::mem::take(&mut self.buf);
        parse_block(&block).into_iter().collect()
    }
}

/// Position and length of the first blank-line separator.
fn find_separator(buf: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < buf.len() {
        if buf[i..].starts_with(b"\r\n\r\n") {
            return Some((i, 4));
        }
        if buf[i..].starts_with(b"\n\n") || buf[i..].starts_with(b"\r\r") {
            return Some((i, 2));
        }
        i += 1;
    }
    None
}

fn parse_block(block: &[u8]) -> Option<SseEvent> {
    let text = String::from_utf8_lossy(block);
    let mut event = None;
    let mut data: Vec<&str> = Vec::new();
    for line in text.split(['\n']).map(|l| l.trim_end_matches('\r')) {
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => event = Some(value.to_string()),
            "data" => data.push(value),
            _ => {}
        }
    }
    if data.is_empty() && event.is_none() {
        return None;
    }
    Some(SseEvent {
        event,
        data: data.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAM: &str = "event: message_start\ndata: {\"a\":1}\n\n: keep-alive\n\nevent: delta\r\ndata: {\"text\":\"h\u{e9}\"}\r\n\r\ndata: line1\ndata: line2\n\ndata: [DONE]";

    fn expected() -> Vec<SseEvent> {
        vec![
            SseEvent {
                event: Some("message_start".into()),
                data: "{\"a\":1}".into(),
            },
            SseEvent {
                event: Some("delta".into()),
                data: "{\"text\":\"h\u{e9}\"}".into(),
            },
            SseEvent {
                event: None,
                data: "line1\nline2".into(),
            },
            SseEvent {
                event: None,
                data: "[DONE]".into(),
            },
        ]
    }

    #[test]
    fn parses_events_split_at_every_byte() {
        let bytes = STREAM.as_bytes();
        for cut in 0..=bytes.len() {
            let mut p = SseParser::default();
            let mut got = p.push(&bytes[..cut]);
            got.extend(p.push(&bytes[cut..]));
            got.extend(p.finish());
            assert_eq!(got, expected(), "split at {cut}");
        }
        let mut p = SseParser::default();
        let mut got: Vec<SseEvent> = bytes.iter().flat_map(|b| p.push(&[*b])).collect();
        got.extend(p.finish());
        assert_eq!(got, expected());
    }
}
