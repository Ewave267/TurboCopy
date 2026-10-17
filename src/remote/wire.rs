use crate::fsutil;
use std::{
    ffi::{OsStr, OsString},
    fs::Metadata,
    io::{self, BufReader, Read, Write},
    net::TcpStream,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
    },
    path::PathBuf,
    time::Duration,
};

pub const MAGIC: &[u8; 8] = b"TCOPY\0\0\x02";
pub const MAX_FIELD: usize = 8192;
pub const MAX_RANGE: u64 = 64 << 20;
pub const STAT: u8 = 1;
pub const STAT_AT: u8 = 2;
pub const LIST: u8 = 3;
pub const NEXT: u8 = 4;
pub const CLOSE_LIST: u8 = 5;
pub const DIRECTORY: u8 = 6;
pub const CLOSE_DIR: u8 = 7;
pub const FINISH_DIR: u8 = 8;
pub const BEGIN: u8 = 9;
pub const WRITE_RANGE: u8 = 10;
pub const COMMIT: u8 = 11;
pub const ABORT: u8 = 12;
pub const READ_RANGE: u8 = 13;
pub const READ_AT: u8 = 14;
pub const LINK: u8 = 15;
pub const ANCESTOR: u8 = 16;
pub const PATH_ANCESTOR: u8 = 17;
pub const SYNC_METADATA: u8 = 18;

// Coalesce metadata frames; large data buffers bypass the small output buffer.
pub struct Stream {
    input: BufReader<TcpStream>,
    output: Vec<u8>,
}
impl Stream {
    pub fn new(socket: TcpStream) -> Self {
        Self {
            input: BufReader::new(socket),
            output: Vec::with_capacity(8192),
        }
    }
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.input.get_ref().set_read_timeout(timeout)
    }
}
impl Read for Stream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.input.read(bytes)
    }
}
impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() >= 8192 {
            self.flush()?;
            return self.input.get_mut().write(bytes);
        }
        if self.output.len() + bytes.len() > 8192 {
            self.flush()?;
        }
        self.output.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.input.get_mut().write_all(&self.output)?;
        self.output.clear();
        self.input.get_mut().flush()
    }
}

pub fn byte(input: &mut impl Read) -> io::Result<u8> {
    let mut bytes = [0];
    input.read_exact(&mut bytes)?;
    Ok(bytes[0])
}
pub fn number(input: &mut impl Read) -> io::Result<u64> {
    let mut bytes = [0; 8];
    input.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}
pub fn put_number(output: &mut impl Write, value: u64) -> io::Result<()> {
    output.write_all(&value.to_le_bytes())
}
pub fn bytes(input: &mut impl Read) -> io::Result<Vec<u8>> {
    let size = number(input)?;
    if size > MAX_FIELD as u64 {
        return Err(fsutil::invalid("oversized SSH protocol field"));
    }
    let mut result = vec![0; size as usize];
    input.read_exact(&mut result)?;
    Ok(result)
}
pub fn put_bytes(output: &mut impl Write, value: &[u8]) -> io::Result<()> {
    if value.len() > MAX_FIELD {
        return Err(fsutil::invalid("oversized SSH protocol field"));
    }
    put_number(output, value.len() as u64)?;
    output.write_all(value)
}
pub fn path(input: &mut impl Read) -> io::Result<PathBuf> {
    let bytes = bytes(input)?;
    if bytes.is_empty() || bytes.contains(&0) {
        return Err(fsutil::invalid("invalid SSH path"));
    }
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}
pub fn name(input: &mut impl Read) -> io::Result<OsString> {
    let value = path(input)?.into_os_string();
    if value.as_bytes().contains(&b'/') || value == ".." {
        return Err(fsutil::invalid("invalid child name"));
    }
    Ok(value)
}
pub fn put_path(output: &mut impl Write, value: &OsStr) -> io::Result<()> {
    put_bytes(output, value.as_bytes())
}
pub fn status(input: &mut (impl Read + Write)) -> io::Result<()> {
    input.flush()?;
    match byte(input)? {
        0 => Ok(()),
        1 => Err(io::Error::other(
            String::from_utf8_lossy(&bytes(input)?).into_owned(),
        )),
        _ => Err(fsutil::invalid("invalid SSH response status")),
    }
}
pub fn ok(output: &mut impl Write) -> io::Result<()> {
    output.write_all(&[0])
}
pub fn error(output: &mut impl Write, error: &io::Error) -> io::Result<()> {
    output.write_all(&[1])?;
    let text = error.to_string();
    put_bytes(output, &text.as_bytes()[..text.len().min(MAX_FIELD)])
}

#[derive(Clone, Debug, Default)]
pub struct Info {
    pub kind: u8, // 0 missing, 1 regular, 2 directory, 3 link, 4 special
    pub size: u64,
    pub mode: u32,
    pub dev: u64,
    pub ino: u64,
    pub mtime: i64,
    pub mtime_ns: i64,
    pub ctime: i64,
    pub ctime_ns: i64,
    pub path: PathBuf,
    pub link: PathBuf,
}
impl Info {
    pub fn new(metadata: &Metadata, path: PathBuf) -> Self {
        Self {
            kind: if metadata.is_file() {
                1
            } else if metadata.is_dir() {
                2
            } else if metadata.file_type().is_symlink() {
                3
            } else {
                4
            },
            size: metadata.len(),
            mode: metadata.mode(),
            dev: metadata.dev(),
            ino: metadata.ino(),
            mtime: metadata.mtime(),
            mtime_ns: metadata.mtime_nsec(),
            ctime: metadata.ctime(),
            ctime_ns: metadata.ctime_nsec(),
            path,
            link: PathBuf::new(),
        }
    }
    pub fn unchanged(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.dev == other.dev
            && self.ino == other.ino
            && self.size == other.size
            && self.mtime == other.mtime
            && self.mtime_ns == other.mtime_ns
            && self.ctime == other.ctime
            && self.ctime_ns == other.ctime_ns
    }
    pub fn write(&self, output: &mut impl Write) -> io::Result<()> {
        output.write_all(&[self.kind])?;
        for value in [
            self.size,
            self.mode as u64,
            self.dev,
            self.ino,
            self.mtime as u64,
            self.mtime_ns as u64,
            self.ctime as u64,
            self.ctime_ns as u64,
        ] {
            put_number(output, value)?;
        }
        put_path(output, self.path.as_os_str())?;
        put_path(output, self.link.as_os_str())
    }
    pub fn read(input: &mut impl Read) -> io::Result<Self> {
        let kind = byte(input)?;
        let result = Self {
            kind,
            size: number(input)?,
            mode: number(input)?
                .try_into()
                .map_err(|_| fsutil::invalid("invalid mode"))?,
            dev: number(input)?,
            ino: number(input)?,
            mtime: number(input)? as i64,
            mtime_ns: number(input)? as i64,
            ctime: number(input)? as i64,
            ctime_ns: number(input)? as i64,
            path: PathBuf::from(OsString::from_vec(bytes(input)?)),
            link: PathBuf::from(OsString::from_vec(bytes(input)?)),
        };
        if kind > 4
            || !(0..1_000_000_000).contains(&result.mtime_ns)
            || !(0..1_000_000_000).contains(&result.ctime_ns)
            || result.size > i64::MAX as u64
            || result.path.as_os_str().as_bytes().contains(&0)
            || result.link.as_os_str().as_bytes().contains(&0)
        {
            return Err(fsutil::invalid("invalid SSH metadata"));
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_oversized_truncated_and_invalid_fields() {
        assert!(bytes(&mut &u64::MAX.to_le_bytes()[..]).is_err());
        assert!(number(&mut &[0; 7][..]).is_err());
        for text in [b"../escape".as_slice(), b"..", b"a/b", b"a\0b"] {
            let mut encoded = Vec::new();
            put_bytes(&mut encoded, text).unwrap();
            assert!(name(&mut encoded.as_slice()).is_err());
        }
    }
    #[test]
    fn metadata_round_trip_preserves_raw_paths_and_signed_times() {
        let info = Info {
            kind: 1,
            size: 7,
            mtime: -1,
            path: PathBuf::from(OsString::from_vec(vec![b'a', 255])),
            ..Info::default()
        };
        let mut encoded = Vec::new();
        info.write(&mut encoded).unwrap();
        let decoded = Info::read(&mut encoded.as_slice()).unwrap();
        assert!(info.unchanged(&decoded));
        assert_eq!(info.path, decoded.path);
    }

    #[test]
    fn rejects_truncated_and_out_of_range_metadata() {
        let mut encoded = Vec::new();
        Info {
            kind: 1,
            path: PathBuf::from("source"),
            ..Info::default()
        }
        .write(&mut encoded)
        .unwrap();
        for length in 0..encoded.len() {
            assert!(Info::read(&mut &encoded[..length]).is_err());
        }
        // Wire offsets for size, mode and the two nanosecond fields.
        for (offset, value) in [
            (1, u64::MAX),
            (9, u64::MAX),
            (41, 1_000_000_000),
            (57, 1_000_000_000),
        ] {
            let mut invalid = encoded.clone();
            invalid[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            assert!(Info::read(&mut invalid.as_slice()).is_err());
        }
        encoded[0] = 5;
        assert!(Info::read(&mut encoded.as_slice()).is_err());
    }
}
