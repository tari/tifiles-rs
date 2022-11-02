use std::io::{Seek, SeekFrom, Write};

use super::{VariableType, MAX_DATA};

/// Custom IO error variants for writing variables.
///
/// These are returned in a `Custom` [`std::io::Error`].
#[derive(thiserror::Error, Debug)]
pub enum WriteError {
    /// Too much data was written to a variable, in excess of what can be represented in a file.
    #[error("Variable data may not exceed {} bytes but would become {0}", MAX_DATA)]
    TooLarge(usize),
    /// An illegal variable name was encountered.
    #[error("Variable name must consist only of uppercase A-Z, \u{03b8}, or after the first character 0-9")]
    InvalidName,
}

/// Writes TI variable files.
///
/// Callers must call [`close`](Writer::close) when writing is complete in order
/// to emit a valid file.
pub struct Writer<W>
where
    W: Write + Seek,
{
    w: ChecksumWriter<W>,
    data_bytes: u16,
    ty: VariableType,
}

impl<W: Write + Seek> Writer<W> {
    /// Open an output for writing.
    ///
    /// Output data gets written to the provided `W`, in the form of a variable of the provided
    /// type with the provided name. If `archived` is true, the variable will be marked for
    /// placement in archive on a calculator.
    ///
    /// If the given name is not legal for a calculator variable, this returns
    /// [`WriteError::InvalidName`].
    pub fn new(
        mut output: W,
        ty: VariableType,
        name: &str,
        archived: bool,
    ) -> std::io::Result<Self> {
        // Verify the provided name is legal, truncate to the maximum length and translate θ to the
        // θ token (which is the only non-ASCII character allowed).
        const THETA: char = '\u{03b8}';
        let mut padded_name = [0u8; 8];
        for (i, c) in name.chars().enumerate().take(padded_name.len()) {
            if !c.is_ascii_uppercase() && c != THETA && (i == 0 && c.is_ascii_digit()) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    WriteError::InvalidName,
                ));
            }
            padded_name[i] = if c == THETA { 0x5b } else { c as u8 };
        }

        // Constant header, comment, and 16-bit size of data section to follow
        let header = b"\
            **TI83F*\x1a\x0a\0\
            TI-8x variable writer by Peter Marheine   \
            \0\0\
        ";
        debug_assert_eq!(header.len(), 55);
        output.write_all(header)?;

        // Subsequent data is largely covered by the file checksum
        let mut output = ChecksumWriter::new(output);
        output.enable_checksums(true);

        // Data section: variable header size, length of data, variable type
        output.write_all(&[0xd, 0, 0, 0, ty as u8])?;
        // Name
        output.write_all(&padded_name)?;
        // Version, flags, length of data again
        output.write_all(&[0, if archived { 0x80 } else { 0 }, 0, 0])?;

        let mut out = Self {
            w: output,
            data_bytes: 0,
            ty,
        };
        if ty.has_length_prefix() {
            // Length prefix built into on actual data; counts against data length
            // in the data section header so writing it here to count against final data_bytes
            out.write_all(&[0, 0])?;
        }

        // Variable data follows, with 16-bit checksum at the end. Lengths are populated
        // on close (once we know how much data there is).
        Ok(out)
    }

    /// Finalize the variable file and return the underlying output.
    ///
    /// This must be called in order to sync assorted internal data structures out to the file.
    /// If this is not called the resulting file will appear to have no data and incorrect
    /// checksums.
    ///
    /// The writer will be positioned after all file data on success.
    pub fn close(self) -> std::io::Result<W> {
        let Self {
            mut w,
            data_bytes,
            ty,
        } = self;

        // Populate assorted length fields at offsets from file start:
        // Length of data section overall (not covered by checksum)
        w.enable_checksums(false);
        w.seek(SeekFrom::Current(-(data_bytes as i64) - 17 - 2))?;
        w.write_all(&(data_bytes + 17).to_le_bytes())?;
        w.enable_checksums(true);

        // First length in data section
        w.seek(SeekFrom::Current(2))?;
        w.write_all(&data_bytes.to_le_bytes())?;
        // Second length in data section
        w.seek(SeekFrom::Current(11))?;
        w.write_all(&data_bytes.to_le_bytes())?;

        if ty.has_length_prefix() {
            // Length embedded in data; data_bytes includes the zeroes already present
            let embedded_len = (data_bytes - 2).to_le_bytes();
            w.write_all(&embedded_len)?;
            w.seek(SeekFrom::Current(-2))?;
        }

        // Seek back to end of data section
        w.seek(SeekFrom::Current(data_bytes as i64))?;

        // All data is written; just finish with the checksum
        let ChecksumWriter {
            mut w, checksum, ..
        } = w;
        w.write_all(&checksum.to_le_bytes())?;
        Ok(w)
    }
}

impl<W: Write + Seek> Write for Writer<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Verify total data size fits in 16-bit fields where it needs to go
        if (self.data_bytes as usize).saturating_add(buf.len()) > MAX_DATA as usize {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                WriteError::TooLarge(self.data_bytes as usize + buf.len()),
            ));
        }

        // Write data to backing writer
        let written = self.w.write(buf)?;
        self.data_bytes += written as u16;

        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.w.flush()
    }
}

/// Writes data to the backing object while computing a simple checksum.
pub struct ChecksumWriter<W> {
    w: W,
    checksum: u16,
    active: bool,
}

impl<W> ChecksumWriter<W> {
    /// If true, add to the checksum for subsequent data.
    fn enable_checksums(&mut self, enable: bool) {
        self.active = enable;
    }

    /// Construct a writer that is initially inactive.
    fn new(w: W) -> Self {
        ChecksumWriter {
            w,
            checksum: 0,
            active: false,
        }
    }
}

/// Writes data to the backing `Write`r, updating the checksum if active.
impl<W: Write> Write for ChecksumWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.w.write(buf)?;
        if self.active {
            for &byte in &buf[..written] {
                self.checksum = self.checksum.wrapping_add(byte as u16);
            }
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.w.flush()
    }
}

/// Seeks within the backing `Write`r, making no other changes.
impl<W: Seek> Seek for ChecksumWriter<W> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.w.seek(pos)
    }
}

/// ChecksumWriter computes the checksum of data written through it.
#[test]
fn checksum_writer_works() {
    let mut writer = ChecksumWriter::new(Vec::<u8>::new());

    writer.checksum = 0xFF00;
    writer.enable_checksums(true);
    writer.write_all(&[255, 1, 0, 42]).unwrap();
    assert_eq!(writer.checksum, 42);

    writer.enable_checksums(false);
    writer.write_all(&[1, 2, 3, 4]).unwrap();
    assert_eq!(writer.checksum, 42);

    assert_eq!(writer.w, &[255, 1, 0, 42, 1, 2, 3, 4]);
}

/// A program file is written with exactly the correct data.
#[test]
fn empty_program_is_correct() {
    use std::io::Cursor;

    let mut buf = Vec::<u8>::new();
    let writer = Writer::new(
        Cursor::new(&mut buf),
        VariableType::ProtectedProgram,
        "A",
        true,
    )
    .unwrap();
    writer.close().unwrap();

    assert_eq!(
        &buf,
        b"**TI83F*\x1a\x0a\0\
              TI-8x variable writer by Peter Marheine   \
              \x13\0\x0d\0\x02\0\x06\
              A\0\0\0\0\0\0\0\
              \0\x80\
              \x02\x00\x00\x00\
              \xd8\x00",
    );
}
