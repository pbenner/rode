//! Document messages retain the ordinary wire format and physical frame bound,
//! but have no whole-file byte ceiling. Serialization streams through one frame
//! buffer rather than constructing another file-sized JSON allocation. Callers
//! must close a connection after a serialization or framing error.
use super::CHUNK;
use serde::{Serialize, de::DeserializeOwned};
use std::io::{self, BufRead, BufReader, Read, Write};

struct Writer<'a, W> {
    output: &'a mut W,
    buffer: Vec<u8>,
}
impl<W: Write> Writer<'_, W> {
    fn frame(&mut self) -> io::Result<()> {
        if !self.buffer.is_empty() {
            self.output
                .write_all(&(self.buffer.len() as u32).to_be_bytes())?;
            self.output.write_all(&self.buffer)?;
            self.buffer.clear();
        }
        Ok(())
    }
}
impl<W: Write> Write for Writer<'_, W> {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let size = bytes.len();
        while !bytes.is_empty() {
            let count = bytes.len().min(CHUNK - self.buffer.len());
            self.buffer.extend_from_slice(&bytes[..count]);
            bytes = &bytes[count..];
            if self.buffer.len() == CHUNK {
                self.frame()?;
            }
        }
        Ok(size)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.frame()?;
        self.output.flush()
    }
}

/// Serialize a document-bearing message without a total byte cap. Physical
/// frames remain bounded and compatible with the ordinary framing functions.
pub fn write_document<T: Serialize>(output: &mut impl Write, value: &T) -> io::Result<()> {
    let mut writer = Writer {
        output,
        buffer: Vec::with_capacity(CHUNK),
    };
    serde_json::to_writer(&mut writer, value).map_err(io::Error::other)?;
    writer.frame()?;
    writer.output.write_all(&0u32.to_be_bytes())?;
    writer.output.flush()
}

struct Reader<'a, R> {
    input: &'a mut R,
    remaining: usize,
    ended: bool,
}
impl<R: Read> Read for Reader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() || self.ended {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut header = [0; 4];
            self.input.read_exact(&mut header)?;
            self.remaining = u32::from_be_bytes(header) as usize;
            if self.remaining > CHUNK {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Oversized host frame",
                ));
            }
            if self.remaining == 0 {
                self.ended = true;
                return Ok(0);
            }
        }
        let count = buffer.len().min(self.remaining);
        let count = self.input.read(&mut buffer[..count])?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Truncated host message",
            ));
        }
        self.remaining -= count;
        Ok(count)
    }
}

/// Deserialize one document message, buffering only physical frames in addition
/// to its actual fields. A clean EOF is accepted only between complete messages.
pub fn read_document<T: DeserializeOwned>(input: &mut impl Read) -> io::Result<Option<T>> {
    let mut header = [0; 4];
    loop {
        match input.read(&mut header[..1]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => {
                if result? == 0 {
                    return Ok(None);
                }
                break;
            }
        }
    }
    input.read_exact(&mut header[1..])?;
    let remaining = u32::from_be_bytes(header) as usize;
    if remaining > CHUNK {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Oversized host frame",
        ));
    }
    let reader = Reader {
        input,
        remaining,
        ended: remaining == 0,
    };
    serde_json::from_reader(BufReader::with_capacity(CHUNK, reader))
        .map(Some)
        .map_err(io::Error::other)
}

struct Line<'a, R> {
    input: &'a mut R,
    ended: bool,
}
impl<R: BufRead> Read for Line<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() || self.ended {
            return Ok(0);
        }
        let buffer = self.input.fill_buf()?;
        if buffer.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Missing worker request newline",
            ));
        }
        let count = buffer.len().min(output.len());
        let count = match buffer[..count].iter().position(|byte| *byte == b'\n') {
            Some(index) => {
                self.ended = true;
                index + 1
            }
            None => count,
        };
        output[..count].copy_from_slice(&buffer[..count]);
        self.input.consume(count);
        Ok(count)
    }
}

/// Read exactly one newline-terminated JSON request without consuming the
/// following lifetime pipe. Escaped newlines inside JSON strings remain data.
pub fn read_json_line<T: DeserializeOwned>(input: &mut impl BufRead) -> io::Result<T> {
    serde_json::from_reader(BufReader::with_capacity(
        CHUNK,
        Line {
            input,
            ended: false,
        },
    ))
    .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streaming_documents_preserve_boundaries_unicode_and_control_compatibility() {
        let value = "λ 🦀\n".repeat(CHUNK);
        let mut encoded = Vec::new();
        write_document(&mut encoded, &value).unwrap();
        super::super::write(&mut encoded, &42).unwrap();
        let mut input = encoded.as_slice();
        assert_eq!(read_document::<String>(&mut input).unwrap(), Some(value));
        assert_eq!(read_document::<u32>(&mut input).unwrap(), Some(42));
        assert!(read_document::<u32>(&mut input).unwrap().is_none());
        let mut input = encoded.as_slice();
        assert!(super::super::read::<String>(&mut input).unwrap().is_some());
        assert_eq!(super::super::read::<u32>(&mut input).unwrap(), Some(42));
    }
    #[test]
    fn streaming_documents_reject_oversized_physical_frames_and_truncation() {
        assert!(
            read_document::<String>(&mut ((CHUNK + 1) as u32).to_be_bytes().as_slice()).is_err()
        );
        let mut encoded = Vec::new();
        write_document(&mut encoded, &"valid").unwrap();
        for end in 1..encoded.len() {
            assert!(read_document::<String>(&mut &encoded[..end]).is_err());
        }
    }
    #[test]
    fn worker_json_does_not_consume_its_lifetime_channel() {
        let mut input = BufReader::with_capacity(3, b"\"hello\\nworld\"\nlifeline".as_slice());
        assert_eq!(
            read_json_line::<String>(&mut input).unwrap(),
            "hello\nworld"
        );
        let mut rest = String::new();
        input.read_to_string(&mut rest).unwrap();
        assert_eq!(rest, "lifeline");
        assert!(read_json_line::<String>(&mut b"\"no newline\"".as_slice()).is_err());
    }
}
