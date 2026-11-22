//! Reading a file a block at a time, for storage where a memory map is slow (a network
//! share on Linux reads a mapped file by page faults, each its own round trip).
//!
//! Blocks are kept for a while, so LibRaw jumping around a file's header costs a handful
//! of requests; while the reads run forward, more is read ahead in one request.

use std::collections::VecDeque;
use std::fs::File;

/// The unit read and kept (the shim's window size).
pub(crate) const BLOCK: usize = 64 * 1024;
/// Blocks kept (6 MB).
const KEPT: usize = 96;
/// The most read ahead in one request.
const MAX_AHEAD: usize = 4 << 20;

/// Read at `offset` without moving a cursor; `Ok(0)` at the end of the file.
fn read_some(file: &File, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_at(file, buf, offset)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::FileExt::seek_read(file, buf, offset)
    }
}

/// Fill as much of `buf` as the file has from `offset`; returns the bytes read.
fn read_full(file: &File, mut offset: u64, buf: &mut [u8]) -> usize {
    let mut done = 0;
    while done < buf.len() {
        match read_some(file, offset, &mut buf[done..]) {
            Ok(0) => break,
            Ok(n) => {
                done += n;
                offset += n as u64;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    done
}

pub(crate) struct BlockReader {
    file: File,
    size: u64,
    /// (block index, its bytes), most recently read last.
    blocks: VecDeque<(u64, Vec<u8>)>,
    /// Where the last request to the file ended, and how much the next one in line reads.
    last_end: u64,
    ahead: usize,
    /// Requests made to the file, and bytes read from it (for tests and diagnostics).
    pub(crate) requests: u64,
    pub(crate) bytes: u64,
}

impl BlockReader {
    pub(crate) fn new(file: File) -> std::io::Result<Self> {
        let size = file.metadata()?.len();
        Ok(BlockReader {
            file,
            size,
            blocks: VecDeque::new(),
            last_end: u64::MAX,
            ahead: BLOCK,
            requests: 0,
            bytes: 0,
        })
    }

    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    /// Read into `buf` from `offset`; returns the bytes read (less only at the end of the
    /// file or on an I/O error).
    pub(crate) fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> usize {
        // A big read goes straight to the file (LibRaw reading a whole strip).
        if buf.len() > BLOCK {
            let n = read_full(&self.file, offset, buf);
            self.requests += 1;
            self.bytes += n as u64;
            self.last_end = offset + n as u64;
            return n;
        }
        let mut done = 0;
        while done < buf.len() {
            let at = offset + done as u64;
            if at >= self.size {
                break;
            }
            let index = at / BLOCK as u64;
            let Some(block) = self.block(index) else { break };
            let from = (at % BLOCK as u64) as usize;
            if from >= block.len() {
                break;
            }
            let n = (block.len() - from).min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&block[from..from + n]);
            done += n;
        }
        done
    }

    /// Block `index`, from those kept or from the file (with what follows it, when the
    /// reads have been running forward).
    fn block(&mut self, index: u64) -> Option<&[u8]> {
        if let Some(i) = self.blocks.iter().position(|(b, _)| *b == index) {
            // Keep it longest.
            let hit = self.blocks.remove(i)?;
            self.blocks.push_back(hit);
            return self.blocks.back().map(|(_, d)| d.as_slice());
        }
        let start = index * BLOCK as u64;
        self.ahead = if start == self.last_end { (self.ahead * 2).min(MAX_AHEAD) } else { BLOCK };
        let want = (self.ahead as u64).min(self.size.saturating_sub(start)) as usize;
        let mut data = vec![0u8; want];
        let got = read_full(&self.file, start, &mut data);
        self.requests += 1;
        self.bytes += got as u64;
        self.last_end = start + got as u64;
        if got == 0 {
            return None;
        }
        data.truncate(got);
        // The asked-for block goes in last, so it is the one returned (and kept longest).
        let mut chunks: Vec<(u64, Vec<u8>)> =
            data.chunks(BLOCK).enumerate().map(|(i, c)| (index + i as u64, c.to_vec())).collect();
        chunks.reverse();
        for (b, d) in chunks {
            self.blocks.retain(|(have, _)| *have != b);
            self.blocks.push_back((b, d));
        }
        while self.blocks.len() > KEPT {
            self.blocks.pop_front();
        }
        self.blocks.back().map(|(_, d)| d.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn file_of(len: usize) -> (tempfile::NamedTempFile, Vec<u8>) {
        let data: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(&data).unwrap();
        f.flush().unwrap();
        (f, data)
    }

    #[test]
    fn scattered_reads_and_block_edges() {
        let (f, data) = file_of(5 * BLOCK + 1234);
        let mut r = BlockReader::new(File::open(f.path()).unwrap()).unwrap();
        assert_eq!(r.size(), data.len() as u64);
        for (offset, len) in [
            (0usize, 16usize),
            (BLOCK - 8, 16),       // across an edge
            (3 * BLOCK + 5, 4000), // elsewhere
            (10, 100),             // back again: kept
            (5 * BLOCK + 1000, 500),
            (2 * BLOCK, BLOCK),
        ] {
            let mut buf = vec![0u8; len];
            let n = r.read_at(offset as u64, &mut buf);
            let want = &data[offset..(offset + len).min(data.len())];
            assert_eq!(&buf[..n], want, "at {offset}");
        }
    }

    #[test]
    fn stops_at_the_end() {
        let (f, data) = file_of(BLOCK + 100);
        let mut r = BlockReader::new(File::open(f.path()).unwrap()).unwrap();
        let mut buf = vec![0u8; 500];
        assert_eq!(r.read_at(BLOCK as u64, &mut buf), 100);
        assert_eq!(&buf[..100], &data[BLOCK..]);
        assert_eq!(r.read_at(data.len() as u64, &mut buf), 0);
        assert_eq!(r.read_at(data.len() as u64 + 10_000, &mut buf), 0);
        // A big read past the end gets what there is.
        let mut big = vec![0u8; 4 * BLOCK];
        assert_eq!(r.read_at(50, &mut big), data.len() - 50);
        assert_eq!(&big[..data.len() - 50], &data[50..]);
    }

    #[test]
    fn kept_blocks_cost_nothing_and_forward_reads_grow() {
        let (f, data) = file_of(40 * BLOCK);
        let mut r = BlockReader::new(File::open(f.path()).unwrap()).unwrap();
        let mut buf = [0u8; 64];
        r.read_at(0, &mut buf);
        r.read_at(7 * BLOCK as u64, &mut buf);
        r.read_at(100, &mut buf);
        r.read_at(7 * BLOCK as u64 + 9, &mut buf);
        assert_eq!(r.requests, 2, "the header's blocks are read once");

        // Reading forward: each request reads more than the last.
        let mut r = BlockReader::new(File::open(f.path()).unwrap()).unwrap();
        let mut out = Vec::new();
        let mut chunk = vec![0u8; 4096];
        let mut at = 0u64;
        loop {
            let n = r.read_at(at, &mut chunk);
            if n == 0 {
                break;
            }
            out.extend_from_slice(&chunk[..n]);
            at += n as u64;
        }
        assert_eq!(out, data);
        assert!(r.requests <= 8, "40 blocks in {} requests", r.requests);
    }
}
