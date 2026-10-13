use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

const LINE_CAP: usize = 4 * 1024 * 1024;
pub(crate) enum Line {
    Text(String),
    Oversize(usize),
}
pub(crate) struct ByteLines<R> {
    reader: BufReader<R>,
    bytes: Vec<u8>,
    length: usize,
}
impl<R: AsyncRead + Unpin> ByteLines<R> {
    pub(crate) fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            bytes: Vec::new(),
            length: 0,
        }
    }
    pub(crate) async fn next(&mut self) -> std::io::Result<Option<Line>> {
        loop {
            let available = self.reader.fill_buf().await?;
            let end = available.iter().position(|byte| *byte == b'\n');
            let count = end.unwrap_or(available.len());
            let eof = available.is_empty();
            self.length = self.length.saturating_add(count);
            if self.length <= LINE_CAP {
                self.bytes.extend_from_slice(&available[..count]);
            } else {
                self.bytes.clear();
            }
            self.reader.consume(count + usize::from(end.is_some()));
            if end.is_some() || eof {
                let length = std::mem::take(&mut self.length);
                let bytes = std::mem::take(&mut self.bytes);
                return Ok(if eof && length == 0 {
                    None
                } else if length > LINE_CAP {
                    Some(Line::Oversize(length))
                } else {
                    Some(Line::Text(String::from_utf8_lossy(&bytes).into_owned()))
                });
            }
        }
    }
}
