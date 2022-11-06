//! B83 and B84 bundles
//!
//! Bundle files are supported by TI-Connect CE for sending multiple variables to a calculator
//! in a single operation. Some other linking software also supports them, but not universally
//! (older versions in particular don't, since bundle files are relatively new).
//!
//! ```
//! use std::io::Write;
//! use tifiles::{VariableType, bundle::{Writer, Kind}};
//!
//! # fn doit() -> Result<(), Box<dyn std::error::Error>> {
//! let outf = std::io::Cursor::new(Vec::new());
//! let mut bundle = Writer::new(Kind::B84, outf);
//!
//! // Writes to the bundle append to the most recently started var
//! bundle.start_var(VariableType::ProtectedProgram, "NOP", false)?;
//! bundle.write_all(&[0xbb, 0x6d, 0xc9])?;
//!
//! bundle.start_var(VariableType::AppVar, "GREETZ", true)?;
//! bundle.write_all(b"Hello, world!")?;
//!
//! // The bundle must be closed to be valid
//! bundle.close()?;
//! # Ok(())
//! # }
//! # doit().unwrap();
//! ```
//!
//! ## Internals
//!
//! Internally, a bundle is a zip archive containing regular variable files and two special
//! files:
//!
//!  * METADATA: a plain-text file containing several fields in the format `<name>:<value>\n`:
//!    * bundle_identifier: `TI Bundle`
//!    * bundle_format_version: 1
//!    * bundle_target_device: `83CE` or `84CE`
//!    * bundle_target_type: `CUSTOM` (presumably other values are also understood)
//!    * bundle_comments: anything you like, apparently
//!  * _CHECKSUM: the arithmetic sum of the CRC32 of each individual variable file's uncompressed
//!    data (as fed into the zip writer). This file is a single line of that CRC formatted as a hex
//!    number followed by `\r\n`.
//!
//! The order of zip entries appears to matter: variable files must come first, followed by METADATA
//! and _CHECKSUM in that order.

use std::io::{Cursor, Read, Result as IoResult, Seek, Write};
use zip::result::ZipError;
use zip::write::FileOptions;

use zip::ZipWriter;

use crate::{VariableType, Writer as VarWriter};

/// Supported bundle kinds.
///
/// A bundle of a given kind has no particular affinity with a given calculator,
/// but TI-Connect may refuse to transfer a bundle to a calculator if the bundle
/// kind does not match the calculator.
pub enum Kind {
    /// .b83, for TI-83 Premium CE
    B83,
    /// .b84, for TI-84+ CE
    B84,
}

impl Kind {
    /// Return the file extension customarily associated with a given bundle kind.
    pub fn file_extension(&self) -> &'static str {
        match self {
            Kind::B83 => "b83",
            Kind::B84 => "b84",
        }
    }

    fn metadata_device_name(&self) -> &'static str {
        match self {
            Kind::B83 => "83CE",
            Kind::B84 => "84CE",
        }
    }
}

/// Writes bundle files.
///
/// A bundle contains zero or more variables, which are written using the
/// [`Write` impl](impl std::io::Write). For each call to [`start_var`](Writer::start_var),
/// subsequent writes will append to that variable's data.
///
/// Users must call [`close`](Writer::close) when done writing all variables
/// in order to create the required metadata entries and close the archive.
pub struct Writer<W>
where
    W: Write + Seek,
{
    kind: Kind,
    zip: ZipWriter<W>,
    crc_sum: u32,
    active_var: Option<(VarWriter<Cursor<Vec<u8>>>, String)>,
}

impl<W> Writer<W>
where
    W: Write + Seek,
{
    pub fn new(kind: Kind, writer: W) -> Self {
        Writer {
            kind,
            zip: ZipWriter::new(writer),
            crc_sum: 0,
            active_var: None,
        }
    }

    /// Begin writing a variable.
    ///
    /// Subsequent writes will append to the most recently-started variable.
    /// Parameters are the same as [`write::Writer::new`](crate::write::Writer::new).
    pub fn start_var(&mut self, ty: VariableType, name: &str, archived: bool) -> IoResult<()> {
        // Finish off the previous var, if any
        self.close_var()?;
        // Make the new one active
        self.active_var = Some((
            VarWriter::new(Cursor::new(Vec::new()), ty, name, archived)?,
            format!("{}.{}", name, ty.file_extension()),
        ));
        Ok(())
    }

    fn update_crc(&mut self, data: &[u8]) {
        self.crc_sum = self.crc_sum.wrapping_add(crc32fast::hash(data));
    }

    fn close_var(&mut self) -> IoResult<()> {
        // Clear the active var and do nothing if there isn't one
        let (w, name) = match self.active_var.take() {
            Some(x) => x,
            None => return Ok(()),
        };
        // Finalize the var file; we needed to buffer it since we can't seek in the zip
        let buf = w.close()?.into_inner();
        // and we need the data to get its CRC (even though the zip writer also computes this; it's
        // hard to get back out of the zip writer)
        self.update_crc(&buf);

        // Flush buffered data out to a new file within the zip
        self.zip.start_file(name, FileOptions::default())?;
        self.zip.write_all(&buf)
    }

    /// Close the archive, returning the underlying writer.
    ///
    /// This must be called in order to make the bundle valid.
    pub fn close(mut self) -> IoResult<W> {
        self.close_var()?;

        self.zip.start_file("METADATA", FileOptions::default())?;
        let metadata_contents =
            format!(
                "bundle_identifier:TI Bundle\n\
             bundle_format_version:1\n\
             bundle_target_device:{}\n\
             bundle_target_type:CUSTOM\n\
             bundle_comments:Generated by tifiles-rs::bundle::Writer\n",
                self.kind.metadata_device_name()
            );
        self.update_crc(metadata_contents.as_bytes());
        self.zip.write_all(metadata_contents.as_bytes())?;

        self.zip.start_file("_CHECKSUM", FileOptions::default())?;
        write!(self.zip, "{:x}", self.crc_sum)?;

        match self.zip.finish() {
            Err(ZipError::Io(e)) => Err(e),
            Err(o) => unreachable!("zip.finish() can only return IO errors, but got {:?}", o),
            Ok(w) => Ok(w),
        }
    }
}

impl<W> Write for Writer<W>
where
    W: Write + Seek,
{
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        match self.active_var {
            None => {
                panic!("start_var must be called on a bundle writer before data can be written")
            }
            Some((ref mut v, _)) => v.write(buf),
        }
    }

    fn flush(&mut self) -> IoResult<()> {
        match self.active_var {
            None => {
                panic!("start_var must be called on a bundle writer before data can be flushed")
            }
            Some((ref mut v, _)) => v.flush(),
        }
    }
}

#[test]
fn crc_matches_metafile() {
    let mut w = Writer::new(Kind::B83, Cursor::new(Vec::new()));

    w.start_var(VariableType::AppVar, "A", false).unwrap();
    write!(w, "var one data").unwrap();
    w.start_var(VariableType::AppVar, "B", false).unwrap();
    write!(w, "var two data").unwrap();
    let data = w.close().unwrap().into_inner();

    let mut zip = zip::ZipArchive::new(Cursor::new(data)).unwrap();
    let mut actual_crc = 0u32;
    for i in 0..zip.len() - 1 {
        let file = zip.by_index(i).unwrap();
        assert_ne!(file.name(), "_CHECKSUM", "checksum file should be last");
        actual_crc = actual_crc.wrapping_add(file.crc32());
    }

    let mut checksum_file = zip.by_name("_CHECKSUM").unwrap();
    let mut checksum_string = String::new();
    checksum_file.read_to_string(&mut checksum_string).unwrap();

    assert_eq!(
        u32::from_str_radix(&checksum_string, 16).unwrap(),
        actual_crc,
        "Actual zip CRCs did not match CHECKSUM file"
    );
}
