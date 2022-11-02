use std::io::{Error, Read};

use super::VariableType;

#[derive(thiserror::Error, Debug)]
pub enum ReadError {
    #[error("File signature should be (\"**TI83F*\", 1a, 0a, 0), but was {0:?}")]
    InvalidSignature([u8; 11]),
    #[error("Variable header reports length {0}, which is unrecognized")]
    UnknownHeaderLength(u16),
    #[error("Variable data length fields disagree: {0} != {1}")]
    DataLengthMismatch(u16, u16),
    #[error("Variable type {0:#x} is not recognized")]
    UnrecognizedType(u8),
}

impl Into<std::io::Error> for ReadError {
    fn into(self) -> Error {
        std::io::Error::new(std::io::ErrorKind::Other, self)
    }
}

pub struct Reader<R>
where
    R: Read,
{
    input: ChecksumReader<std::io::Take<R>>,
    comment: [u8; 42],
    ty: VariableType,
    name: [u8; 8],
    archived: bool,
    data_len: u16,
}

fn read8<R: Read>(mut r: R) -> std::io::Result<u8> {
    let mut buf = [0u8];
    r.read_exact(&mut buf)?;
    Ok(buf[0])
}

fn read16<R: Read>(mut r: R) -> std::io::Result<u16> {
    let mut buf = [0u8; 2];
    r.read_exact(&mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

impl<R> Reader<R>
where
    R: Read,
{
    pub fn new(mut r: R) -> std::io::Result<Self> {
        let mut signature = [0u8; 11];
        r.read_exact(&mut signature)?;
        if &signature != b"**TI83F*\x1a\x0a\0" {
            return Err(ReadError::InvalidSignature(signature).into());
        }

        let mut comment = [0u8; 42];
        r.read_exact(&mut comment)?;

        let data_section_len = read16(&mut r)?;

        // Begin data section. All data from here until final checksum is checksummed,
        // and the data section length tells us how much data we can read.
        let mut r = ChecksumReader {
            r: r.take(data_section_len as u64),
            checksum: 0,
        };

        let entry_header_len = read16(&mut r)?;
        if ![11, 13].contains(&entry_header_len) {
            return Err(ReadError::UnknownHeaderLength(entry_header_len).into());
        }

        let mut data_len = read16(&mut r)?;
        if data_len + entry_header_len + 4 != data_section_len {
            return Err(ReadError::DataLengthMismatch(
                data_len + entry_header_len + 4,
                data_section_len,
            )
            .into());
        }

        let ty = match VariableType::try_from(read8(&mut r)?) {
            Ok(ty) => ty,
            Err(e) => return Err(ReadError::UnrecognizedType(e.number).into()),
        };

        let mut name = [0u8; 8];
        r.read_exact(&mut name)?;

        let archived = if entry_header_len == 13 {
            let _version = read8(&mut r)?;
            let flag = read8(&mut r)?;
            flag & 0x80 != 0
        } else {
            false
        };

        let data_len2 = read16(&mut r)?;
        if data_len != data_len2 {
            return Err(ReadError::DataLengthMismatch(data_len, data_len2).into());
        }

        if ty.has_length_prefix() {
            // Inner length excludes the length field itself
            let inner_len = read16(&mut r)?;
            if data_len != inner_len + 2 {
                return Err(ReadError::DataLengthMismatch(data_len, inner_len).into());
            }
            // Reported length excludes the length prefix because we handle that
            data_len -= 2;
        }

        debug_assert_eq!(
            r.r.limit(),
            data_len as u64,
            "remaining data to take should be equal to var data size"
        );

        Ok(Reader {
            input: r,
            comment,
            ty,
            name,
            archived,
            data_len,
        })
    }

    /// Return the number of bytes of variable data this reader contains.
    ///
    /// This value is constant for any given input data.
    pub fn len(&self) -> u16 {
        self.data_len
    }

    /// Get the type of the variable returned via this reader.
    pub fn ty(&self) -> VariableType {
        self.ty
    }

    /// Get the contained variable's name.
    pub fn name(&self) -> &[u8] {
        self.name.as_slice()
    }

    /// Return whether the contained variable is marked as archived.
    pub fn is_archived(&self) -> bool {
        self.archived
    }

    /// Return the file's comment.
    pub fn comment(&self) -> &[u8] {
        self.comment.as_slice()
    }

    /// Finish reading the input, dropping unread data.
    ///
    /// Returns `Ok` if the file checksum is valid, `Err` otherwise. Any data that wasn't read by
    /// the user is used to verify the checksum but is not returned. If the checksum is not valid,
    /// the output tuple's first value is the computed checksum and the second is the checksum
    /// included in the file.
    ///
    /// The reader will be positioned after all file data on success.
    pub fn finish(mut self) -> std::io::Result<Result<R, FinishError<R>>> {
        // Read to end of data
        loop {
            let mut buf = [0u8; 256];
            if self.input.read(&mut buf[..])? == 0 {
                break;
            }
        }

        // Stop computing checksums of read data and compare with file
        let ChecksumReader { r, checksum } = self.input;
        let mut input = r.into_inner();
        let file_checksum = read16(&mut input)?;

        if checksum != file_checksum {
            Ok(Err(FinishError {
                r: input,
                computed_checksum: checksum,
                read_checksum: file_checksum,
            }))
        } else {
            Ok(Ok(input))
        }
    }
}

impl<R: Read> Read for Reader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // input is a Take so we can't overread, and checksums include everything:
        // do nothing but delegate to the underlying reader.
        self.input.read(buf)
    }
}

struct ChecksumReader<R> {
    r: R,
    checksum: u16,
}

impl<R> Read for ChecksumReader<R>
where
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.r.read(buf)?;
        for &b in &buf[..n] {
            self.checksum = self.checksum.wrapping_add(b as u16);
        }
        Ok(n)
    }
}

#[derive(thiserror::Error, Debug)]
#[error("File checksum was {read_checksum:#x} but read data checksummed to {computed_checksum:#x}")]
pub struct FinishError<R> {
    r: R,
    computed_checksum: u16,
    read_checksum: u16,
}

impl<R> FinishError<R> {
    pub fn into_reader(self) -> R {
        self.r
    }
}

#[test]
fn reads_empty_appvar() {
    const DATA: &'static [u8] = b"**TI83F*\x1a\x0a\0Created by SourceCoder 3 - sc.cemetech.net\
                                  \x13\0\x0d\0\x02\0\x15A\0\0\0\0\0\0\0\0\0\x02\0\0\0\x67\0";

    let mut reader = Reader::new(DATA).unwrap();
    assert_eq!(reader.len(), 0);
    assert_eq!(reader.ty(), VariableType::AppVar);
    assert_eq!(reader.name(), b"A\0\0\0\0\0\0\0");
    assert!(!reader.is_archived());
    assert_eq!(
        reader.comment(),
        b"Created by SourceCoder 3 - sc.cemetech.net"
    );

    let mut contents = vec![];
    reader.read_to_end(&mut contents).unwrap();
    assert!(contents.is_empty());

    reader.finish().unwrap().expect("checksum should be valid");
}
