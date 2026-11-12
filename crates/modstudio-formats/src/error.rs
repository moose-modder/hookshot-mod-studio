use std::fmt;
use std::path::PathBuf;

/// Everything that can go wrong reading or writing the game's files.
#[derive(Debug)]
pub enum Error {
    /// A read past the end of the data: `len` bytes at `offset` in data of `size` bytes.
    Truncated {
        offset: usize,
        len: usize,
        size: usize,
    },
    /// The data doesn't start with the expected signature.
    BadMagic {
        expected: &'static str,
        found: Vec<u8>,
    },
    /// The data is well-formed but uses something this code doesn't handle (yet).
    Unsupported(String),
    /// The data contradicts itself (counts, offsets, sizes).
    Invalid(String),
    /// A zstd frame needs a dictionary that isn't loaded.
    DictionaryMissing(u32),
    /// zstd decoding failed.
    Zstd(String),
    /// A file couldn't be read or written.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated { offset, len, size } => write!(
                f,
                "data ends early: {len} bytes at 0x{offset:x}, but there are only 0x{size:x} bytes"
            ),
            Error::BadMagic { expected, found } => {
                write!(f, "not a {expected} file (starts with {found:02x?})")
            }
            Error::Unsupported(what) => write!(f, "not supported: {what}"),
            Error::Invalid(what) => write!(f, "invalid data: {what}"),
            Error::DictionaryMissing(id) => write!(
                f,
                "the zstd frame needs dictionary 0x{id:08x}, which isn't in the game's ZsDic.pack"
            ),
            Error::Zstd(what) => write!(f, "zstd: {what}"),
            Error::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub(crate) fn invalid(what: impl Into<String>) -> Error {
    Error::Invalid(what.into())
}

pub(crate) fn unsupported(what: impl Into<String>) -> Error {
    Error::Unsupported(what.into())
}
