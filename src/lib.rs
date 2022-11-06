//! TI file formats
//!
//! [`Reader`]s read the data contained in calculator variable files, and [`Writer`]s write data to
//! calculator variable files. The meaning of data in any given file depends on the
//! [`VariableType`].
//!
//! Refer to the [TI link protocol & file format
//! guide](https://www.ticalc.org/archives/files/fileinfo/247/24750.html)
//! for details on file formats.
use num_enum::TryFromPrimitive;

#[cfg(feature = "bundles")]
pub mod bundle;
pub mod read;
pub mod write;

pub use read::Reader;
pub use write::Writer;

/// Types of variables
///
/// These values correspond to `*Obj` constants from ti83plus.inc, and match the type byte
/// stored in the VAT on a calculator.
#[repr(u8)]
#[derive(Debug, PartialEq, Eq, Clone, Copy, TryFromPrimitive)]
pub enum VariableType {
    Real = 0,             // 8xn
    List = 1,             // 8xl
    Matrix = 2,           // 8xm
    Equation = 3,         // 8xy
    String = 4,           // 8xs
    Program = 5,          // 8xp
    ProtectedProgram = 6, // also 8xp
    Picture = 7,          // 8xi
    GDB = 8,              // 8xd
    Unknown = 9,
    UnknownEquation = 0xa,
    NewEquation = 0xb,
    Complex = 0xc,     // 8xc
    ComplexList = 0xd, // also 8xl
    Undefined = 0xe,
    Window = 0xf,
    Zoom = 0x10,       // 8xz (ZSto)
    TableSetup = 0x11, // 8xt (TblRng)
    LCD = 0x12,
    Backup = 0x13,
    // AppObj=0x14 never appears in the VAT, and 8xk files use the "flash" format
    AppVar = 0x15, // 8xv
    TemporaryProgram = 0x16,
    Group = 0x17, // 8xg
}

impl VariableType {
    fn has_length_prefix(&self) -> bool {
        use VariableType::*;
        match self {
            Equation | String | GDB | Program | ProtectedProgram | Picture | Window
            | TableSetup | AppVar => true,
            Real | List | Matrix | Complex | ComplexList => false,
            x => unimplemented!("Format for variable type {:?} is unknown", x),
        }
    }

    /// Return the customary file extension associated with a file of a given variable type.
    pub fn file_extension(&self) -> &'static str {
        use VariableType::*;
        match self {
            Real => "8xn",
            Complex => "8xc",
            List | ComplexList => "8xl",
            Matrix => "8xm",
            Equation => "8xy",
            String => "8xs",
            Program | ProtectedProgram => "8xp",
            Picture => "8xi",
            GDB => "8xd",
            Zoom => "8xz",
            TableSetup => "8xt",
            AppVar => "8xv",
            Group => "8xg",
            t => todo!("File extension for type {:?} isn't yet known", t),
        }
    }
}

/// The maximum amount of data that can be stored in a file.
///
/// Variable data has 17 bytes of overhead and the overall data section size is 16 bits, so any more
/// than this overflows the mandatory length fields.
const MAX_DATA: u16 = u16::MAX - 17;

#[test]
fn round_trip_is_lossless() {
    use std::io::{Cursor, Read, Write};

    let mut ref_data = [0u8; 256];
    for (x, b) in ref_data.iter_mut().enumerate() {
        *b = x as u8;
    }

    let mut file_data = vec![];
    {
        let mut writer = Writer::new(
            Cursor::new(&mut file_data),
            VariableType::Program,
            "ABC123",
            false,
        )
        .unwrap();
        writer.write_all(&ref_data).unwrap();
        writer.close().unwrap();
    }

    let mut reader = Reader::new(&*file_data).unwrap();
    assert_eq!(reader.name(), b"ABC123\0\0");
    assert!(!reader.is_archived());

    let mut read_data = vec![];
    reader.read_to_end(&mut read_data).unwrap();
    reader.finish().unwrap().expect("checksum should be valid");

    assert_eq!(ref_data, &*read_data);
}
