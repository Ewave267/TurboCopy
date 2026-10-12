use crate::fsutil;
use std::{
    fs::File,
    io::{self, ErrorKind},
    os::{fd::AsRawFd, unix::fs::FileExt},
};

const SEEK_DATA: i32 = 3;
const SEEK_HOLE: i32 = 4;

extern "C" {
    fn lseek(fd: i32, offset: i64, whence: i32) -> i64;
}

fn extent(fd: i32, offset: u64, whence: i32) -> io::Result<u64> {
    loop {
        // SAFETY: fd is owned by the caller's open File; Linux lseek accepts these constants.
        let found = unsafe { lseek(fd, offset as i64, whence) };
        if found >= 0 {
            return Ok(found as u64);
        }
        let error = io::Error::last_os_error();
        if error.kind() != ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn unsupported(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(22 | 29 | 38 | 95))
}

/// Returns false when the source filesystem cannot report data and hole extents.
pub fn copy(
    input: &File,
    output: &File,
    length: u64,
    buffer: &mut [u8],
    mut report: impl FnMut(u64),
) -> io::Result<bool> {
    if buffer.is_empty() {
        return Err(io::Error::new(ErrorKind::InvalidInput, "empty copy buffer"));
    }
    if length > i64::MAX as u64 {
        return Ok(false);
    }
    let mut position = 0;
    while position < length {
        fsutil::check_cancelled()?;
        let data = match extent(input.as_raw_fd(), position, SEEK_DATA) {
            Ok(data) => data,
            Err(error) if error.raw_os_error() == Some(6) => break, // ENXIO: trailing hole
            Err(error) if unsupported(&error) => return Ok(false),
            Err(error) => return Err(error),
        };
        if data < position {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "invalid data extent",
            ));
        }
        if data >= length {
            break;
        }
        let hole = match extent(input.as_raw_fd(), data, SEEK_HOLE) {
            Ok(hole) => hole.min(length),
            Err(error) if unsupported(&error) => return Ok(false),
            Err(error) => return Err(error),
        };
        if hole <= data {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "invalid hole extent",
            ));
        }
        position = data;
        while position < hole {
            fsutil::check_cancelled()?;
            let limit = (hole - position).min(buffer.len() as u64) as usize;
            let count = match input.read_at(&mut buffer[..limit], position) {
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                result => result?,
            };
            if count == 0 {
                return Err(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "source shrank during copying",
                ));
            }
            let mut written = 0;
            while written < count {
                fsutil::check_cancelled()?;
                let n = match output.write_at(&buffer[written..count], position + written as u64) {
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    result => result?,
                };
                if n == 0 {
                    return Err(ErrorKind::WriteZero.into());
                }
                written += n;
                report(n as u64);
            }
            position += count as u64;
        }
    }
    output.set_len(length)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{Seek, SeekFrom, Write},
        os::unix::fs::MetadataExt,
    };

    #[test]
    fn copies_sparse_extents_and_trailing_hole() {
        let dir = std::env::temp_dir().join(format!(
            "tcopy-sparse-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::create_dir(&dir).unwrap();
        let result = (|| -> io::Result<()> {
            let mut source = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(dir.join("source"))?;
            source.write_all(b"start")?;
            source.seek(SeekFrom::Start(2 * 1024 * 1024))?;
            source.write_all(b"middle")?;
            source.set_len(4 * 1024 * 1024)?;
            let target = File::create(dir.join("target"))?;
            let mut copied = 0;
            assert!(copy(
                &source,
                &target,
                source.metadata()?.len(),
                &mut [0; 4096],
                |n| copied += n
            )?);
            assert!(copied >= 11 && copied < source.metadata()?.len());
            assert_eq!(fs::read(dir.join("source"))?, fs::read(dir.join("target"))?);
            assert_eq!(source.metadata()?.len(), target.metadata()?.len());
            assert!(target.metadata()?.blocks() < target.metadata()?.len() / 512);
            Ok(())
        })();
        fs::remove_dir_all(&dir).unwrap();
        result.unwrap();
    }
}
