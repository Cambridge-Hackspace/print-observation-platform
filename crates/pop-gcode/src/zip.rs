//! Reading a zip archive: as much of the format as a sliced 3MF uses, and no
//! more.
//!
//! Stored and deflated entries in a single-disk archive under 4 GiB. Anything
//! else (zip64, encryption, other compression, archives split across disks)
//! is refused, not guessed at. Every size comes from the central directory,
//! is checked against the archive's length and against the caller's limit
//! before anything is decompressed, and is checked again as the entry
//! streams: an entry that decompresses to more than it says, or to anything
//! but its CRC, is refused when it does.

use std::io::{self, Read, Seek, SeekFrom};

use crate::threemf::Error;

const EOCD: u32 = 0x0605_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const LOCAL: u32 = 0x0403_4b50;
const EOCD_LEN: usize = 22;
const CENTRAL_LEN: usize = 46;
const LOCAL_LEN: usize = 30;
/// The largest central directory read: room for thousands of entries, where
/// a sliced 3MF has a few dozen.
const MAX_DIRECTORY: u64 = 4 << 20;

/// One entry, as the central directory gives it.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub name: String,
    method: u16,
    crc: u32,
    compressed: u64,
    pub size: u64,
    offset: u64,
}

pub(crate) struct Archive<R> {
    r: R,
    /// Where the central directory starts: no entry's data may run past it.
    directory: u64,
    pub entries: Vec<Entry>,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn io(e: io::Error) -> Error {
    Error::Unreadable(e.kind())
}

impl<R: Read + Seek> Archive<R> {
    pub fn open(mut r: R) -> Result<Self, Error> {
        let len = r.seek(SeekFrom::End(0)).map_err(io)?;
        // The end record is the last thing in the file, after a comment of
        // at most 64 KiB.
        let tail_len = len.min((EOCD_LEN + u16::MAX as usize) as u64);
        let mut tail = vec![0; tail_len as usize];
        r.seek(SeekFrom::Start(len - tail_len)).map_err(io)?;
        r.read_exact(&mut tail).map_err(io)?;
        // The end record whose comment runs exactly to the end of the file:
        // a signature inside a comment is not one.
        let at = (0..tail.len().saturating_sub(EOCD_LEN - 1))
            .rev()
            .find(|&i| {
                u32_at(&tail, i) == EOCD
                    && i + EOCD_LEN + u16_at(&tail, i + 20) as usize == tail.len()
            })
            .ok_or(Error::NotAnArchive)?;
        let end = &tail[at..at + EOCD_LEN];
        if u16_at(end, 4) != 0 || u16_at(end, 6) != 0 || u16_at(end, 8) != u16_at(end, 10) {
            return Err(Error::Unsupported("an archive split across disks"));
        }
        let count = u16_at(end, 10);
        let size = u64::from(u32_at(end, 12));
        let directory = u64::from(u32_at(end, 16));
        if count == u16::MAX || size == u64::from(u32::MAX) || directory == u64::from(u32::MAX) {
            return Err(Error::Unsupported("a zip64 archive"));
        }
        let end_at = len - tail_len + at as u64;
        if directory.checked_add(size) != Some(end_at) {
            return Err(Error::Corrupt(
                "the central directory is not where the archive says",
            ));
        }
        if size > MAX_DIRECTORY {
            return Err(Error::TooLarge("the archive's central directory"));
        }
        let mut dir = vec![0; size as usize];
        r.seek(SeekFrom::Start(directory)).map_err(io)?;
        r.read_exact(&mut dir).map_err(io)?;

        let mut entries = Vec::with_capacity(count as usize);
        let mut at = 0;
        for _ in 0..count {
            let h = dir
                .get(at..at + CENTRAL_LEN)
                .ok_or(Error::Corrupt("the central directory ends early"))?;
            if u32_at(h, 0) != CENTRAL {
                return Err(Error::Corrupt(
                    "an entry in the central directory is not one",
                ));
            }
            let flags = u16_at(h, 8);
            let method = u16_at(h, 10);
            let crc = u32_at(h, 16);
            let compressed = u64::from(u32_at(h, 20));
            let size = u64::from(u32_at(h, 24));
            let name_len = u16_at(h, 28) as usize;
            let extra_len = u16_at(h, 30) as usize;
            let comment_len = u16_at(h, 32) as usize;
            let offset = u64::from(u32_at(h, 42));
            let name = dir
                .get(at + CENTRAL_LEN..at + CENTRAL_LEN + name_len)
                .ok_or(Error::Corrupt("the central directory ends early"))?;
            at += CENTRAL_LEN + name_len + extra_len + comment_len;
            if at > dir.len() {
                return Err(Error::Corrupt("the central directory ends early"));
            }
            if compressed == u64::from(u32::MAX)
                || size == u64::from(u32::MAX)
                || offset == u64::from(u32::MAX)
            {
                return Err(Error::Unsupported("a zip64 archive"));
            }
            if flags & 1 != 0 {
                return Err(Error::Unsupported("an encrypted entry"));
            }
            // A name that is not UTF-8 is no entry a slicer wrote: skipped,
            // so nothing can be found by it.
            let Ok(name) = String::from_utf8(name.to_vec()) else {
                continue;
            };
            entries.push(Entry {
                name,
                method,
                crc,
                compressed,
                size,
                offset,
            });
        }
        Ok(Self {
            r,
            directory,
            entries,
        })
    }

    /// Stream `entry` into `sink`, refusing it if it says it is larger than
    /// `limit` bytes.
    pub fn read(
        &mut self,
        entry: &Entry,
        limit: u64,
        mut sink: impl FnMut(&[u8]),
    ) -> Result<(), Error> {
        if entry.size > limit {
            return Err(Error::TooLarge("an entry"));
        }
        let mut h = [0; LOCAL_LEN];
        self.r.seek(SeekFrom::Start(entry.offset)).map_err(io)?;
        self.r
            .read_exact(&mut h)
            .map_err(|_| Error::Corrupt("an entry's header runs past the archive"))?;
        if u32_at(&h, 0) != LOCAL || u16_at(&h, 8) != entry.method {
            return Err(Error::Corrupt(
                "an entry's header does not match the directory",
            ));
        }
        let name_len = u64::from(u16_at(&h, 26));
        let mut name = vec![0; name_len as usize];
        self.r
            .read_exact(&mut name)
            .map_err(|_| Error::Corrupt("an entry's header runs past the archive"))?;
        if name != entry.name.as_bytes() {
            return Err(Error::Corrupt(
                "an entry's header does not match the directory",
            ));
        }
        let data = entry.offset + LOCAL_LEN as u64 + name_len + u64::from(u16_at(&h, 28));
        if data
            .checked_add(entry.compressed)
            .is_none_or(|e| e > self.directory)
        {
            return Err(Error::Corrupt("an entry runs into the central directory"));
        }
        self.r.seek(SeekFrom::Start(data)).map_err(io)?;
        let raw = (&mut self.r).take(entry.compressed);
        let mut stream: Box<dyn Read + '_> = match entry.method {
            0 if entry.compressed == entry.size => Box::new(raw),
            0 => return Err(Error::Corrupt("a stored entry's sizes differ")),
            8 => Box::new(flate2::read::DeflateDecoder::new(raw)),
            _ => {
                return Err(Error::Unsupported(
                    "an entry compressed other than by deflate",
                ));
            }
        };
        let mut crc = crc32fast::Hasher::new();
        let mut total: u64 = 0;
        let mut buf = vec![0; 64 * 1024];
        loop {
            // One byte more than it says it holds, to see whether it does.
            let room = (entry.size - total.min(entry.size) + 1).min(buf.len() as u64) as usize;
            let n = match stream.read(&mut buf[..room]) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(Error::Corrupt("an entry does not decompress")),
            };
            total += n as u64;
            if total > entry.size {
                return Err(Error::Corrupt("an entry is longer than it says"));
            }
            crc.update(&buf[..n]);
            sink(&buf[..n]);
        }
        if total != entry.size {
            return Err(Error::Corrupt("an entry is shorter than it says"));
        }
        if crc.finalize() != entry.crc {
            return Err(Error::Corrupt("an entry's checksum does not match"));
        }
        Ok(())
    }
}
