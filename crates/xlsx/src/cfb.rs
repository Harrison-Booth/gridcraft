//! Just enough of the Compound File Binary (CFB/OLE2) container to tell what a non-zip
//! spreadsheet file is: Excel wraps password-protected workbooks in a CFB holding an
//! `EncryptedPackage` stream, while Excel 97-2003 `.xls` (and other legacy Office files) are
//! CFB files without one. Every read is bounds-checked and every loop bounded.

/// The CFB signature at the start of the file.
pub(crate) const SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

const END_OF_CHAIN: u32 = 0xFFFF_FFFE;
/// Sector ids at or above this are markers (free, end of chain, FAT/DIFAT sector), not sectors.
const MAX_REG_SECT: u32 = 0xFFFF_FFFA;
/// Directory sectors followed at most (4096 entries with 512-byte sectors).
const MAX_DIR_SECTORS: usize = 1024;
/// DIFAT sectors followed at most.
const MAX_DIFAT_SECTORS: usize = 1024;

/// Stream names that mark an encrypted (password-protected) OOXML package.
const ENCRYPTED_STREAMS: [&str; 2] = ["EncryptedPackage", "EncryptionInfo"];

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([*s.first()?, *s.get(1)?]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([*s.first()?, *s.get(1)?, *s.get(2)?, *s.get(3)?]))
}

/// Whether the compound file holds an encryption stream. Walks the directory through the FAT;
/// when the structure can't be followed (truncated or malformed), falls back to scanning the
/// bytes for the UTF-16LE stream names.
pub(crate) fn is_encrypted(bytes: &[u8]) -> bool {
    match directory_names(bytes) {
        Some(names) => names.iter().any(|n| ENCRYPTED_STREAMS.iter().any(|e| n.eq_ignore_ascii_case(e))),
        None => ENCRYPTED_STREAMS.iter().any(|e| contains_utf16(bytes, e)),
    }
}

fn contains_utf16(hay: &[u8], needle: &str) -> bool {
    let n: Vec<u8> = needle.encode_utf16().flat_map(u16::to_le_bytes).collect();
    !n.is_empty() && hay.windows(n.len()).any(|w| w == n.as_slice())
}

/// Names of every directory entry, or `None` when the directory can't be read.
fn directory_names(b: &[u8]) -> Option<Vec<String>> {
    if !b.starts_with(&SIGNATURE) {
        return None;
    }
    let shift = u16_at(b, 30)?;
    if shift != 9 && shift != 12 {
        return None;
    }
    let size = 1usize << shift;
    let per = size / 4;
    let sector = |id: u32| -> Option<&[u8]> {
        let start = (id as usize).checked_add(1)?.checked_mul(size)?;
        b.get(start..start.checked_add(size)?)
    };
    // The DIFAT: the FAT's sector ids, 109 in the header then chained DIFAT sectors.
    let mut difat: Vec<u32> = (0..109).filter_map(|i| u32_at(b, 76 + i * 4)).filter(|&s| s < MAX_REG_SECT).collect();
    let mut next = u32_at(b, 68)?;
    for _ in 0..MAX_DIFAT_SECTORS {
        if next >= MAX_REG_SECT {
            break;
        }
        let s = sector(next)?;
        difat.extend((0..per - 1).filter_map(|i| u32_at(s, i * 4)).filter(|&x| x < MAX_REG_SECT));
        next = u32_at(s, (per - 1) * 4)?;
    }
    let fat_next = |id: u32| -> Option<u32> {
        let idx = id as usize;
        let fat_sector = *difat.get(idx / per)?;
        u32_at(sector(fat_sector)?, (idx % per) * 4)
    };
    let mut names = Vec::new();
    let mut id = u32_at(b, 48)?;
    for _ in 0..MAX_DIR_SECTORS {
        if id == END_OF_CHAIN {
            return Some(names);
        }
        if id >= MAX_REG_SECT {
            return None;
        }
        let s = sector(id)?;
        for e in s.as_chunks::<128>().0 {
            let len = usize::from(u16_at(e, 64)?);
            if !(2..=64).contains(&len) {
                continue;
            }
            let units: Vec<u16> = (0..(len - 2) / 2).filter_map(|i| u16_at(e, i * 2)).collect();
            names.push(String::from_utf16_lossy(&units));
        }
        id = fat_next(id)?;
    }
    // A directory longer than the bound (or a cyclic chain): report what was read.
    Some(names)
}

/// Directory entry type of a stream object ([MS-CFB] 2.6.1).
const STREAM_OBJECT: u8 = 2;
/// "No entry" in the directory tree (NOSTREAM).
const NO_STREAM: u32 = 0xFFFF_FFFF;
/// Streams larger than this are refused (the format allows more, a workbook never needs it).
const MAX_STREAM: u64 = 1 << 30;

/// One directory entry ([MS-CFB] 2.6).
struct DirEntry {
    name: String,
    kind: u8,
    left: u32,
    right: u32,
    child: u32,
    start: u32,
    size: u64,
}

/// A parsed compound file: the FAT, the mini FAT and the directory, all bounds-checked.
/// Implemented from Microsoft's public [MS-CFB] specification.
pub(crate) struct Compound<'a> {
    bytes: &'a [u8],
    sector_size: usize,
    mini_sector_size: usize,
    mini_cutoff: u64,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    entries: Vec<DirEntry>,
}

impl<'a> Compound<'a> {
    /// Parses the header, FAT, mini FAT and directory. Errors on anything that can't be followed.
    pub(crate) fn open(b: &'a [u8]) -> Result<Compound<'a>, String> {
        let bad = |m: &str| format!("damaged compound file: {m}");
        if !b.starts_with(&SIGNATURE) {
            return Err(bad("missing signature"));
        }
        let major = u16_at(b, 26).ok_or_else(|| bad("truncated header"))?;
        let shift = u16_at(b, 30).ok_or_else(|| bad("truncated header"))?;
        if shift != 9 && shift != 12 {
            return Err(bad("unsupported sector size"));
        }
        let mini_shift = u16_at(b, 32).ok_or_else(|| bad("truncated header"))?;
        if mini_shift != 6 {
            return Err(bad("unsupported mini sector size"));
        }
        let sector_size = 1usize << shift;
        let per = sector_size / 4;
        // Sectors that physically exist after the header sector: every chain is bounded by it.
        let sectors = (b.len() / sector_size).saturating_sub(1);
        let sector = |id: u32| -> Option<&[u8]> {
            let start = (id as usize).checked_add(1)?.checked_mul(sector_size)?;
            b.get(start..start.checked_add(sector_size)?)
        };
        // DIFAT: 109 FAT sector ids in the header, then chained DIFAT sectors.
        let mut fat_sectors: Vec<u32> = (0..109).filter_map(|i| u32_at(b, 76 + i * 4)).filter(|&s| s < MAX_REG_SECT).collect();
        let mut next = u32_at(b, 68).ok_or_else(|| bad("truncated header"))?;
        let mut hops = 0usize;
        while next < MAX_REG_SECT {
            hops += 1;
            if hops > sectors.min(MAX_DIFAT_SECTORS * 64) {
                return Err(bad("DIFAT chain loops"));
            }
            let s = sector(next).ok_or_else(|| bad("DIFAT sector out of range"))?;
            fat_sectors.extend((0..per - 1).filter_map(|i| u32_at(s, i * 4)).filter(|&x| x < MAX_REG_SECT));
            next = u32_at(s, (per - 1) * 4).ok_or_else(|| bad("truncated DIFAT sector"))?;
        }
        if fat_sectors.len() > sectors {
            return Err(bad("more FAT sectors than the file holds"));
        }
        let mut fat = Vec::with_capacity(fat_sectors.len().saturating_mul(per));
        for id in fat_sectors {
            let s = sector(id).ok_or_else(|| bad("FAT sector out of range"))?;
            fat.extend((0..per).filter_map(|i| u32_at(s, i * 4)));
        }
        let mut cf = Compound { bytes: b, sector_size, mini_sector_size: 64, mini_cutoff: 4096, fat, mini_fat: vec![], entries: vec![] };
        if let Some(c) = u32_at(b, 56) {
            cf.mini_cutoff = u64::from(c);
        }
        // Directory.
        let dir_start = u32_at(b, 48).ok_or_else(|| bad("truncated header"))?;
        let dir = cf.read_chain(dir_start, u64::MAX)?;
        for e in dir.as_chunks::<128>().0 {
            let len = usize::from(u16_at(e, 64).unwrap_or(0)).min(64);
            let units: Vec<u16> = (0..len.saturating_sub(2) / 2).filter_map(|i| u16_at(e, i * 2)).collect();
            let mut size = u32_at(e, 120).map(u64::from).unwrap_or(0);
            if major >= 4 {
                size |= u64::from(u32_at(e, 124).unwrap_or(0)) << 32;
            }
            cf.entries.push(DirEntry {
                name: String::from_utf16_lossy(&units),
                kind: e.get(66).copied().unwrap_or(0),
                left: u32_at(e, 68).unwrap_or(NO_STREAM),
                right: u32_at(e, 72).unwrap_or(NO_STREAM),
                child: u32_at(e, 76).unwrap_or(NO_STREAM),
                start: u32_at(e, 116).unwrap_or(END_OF_CHAIN),
                size,
            });
        }
        if cf.entries.first().is_none_or(|r| r.kind != 5) {
            return Err(bad("missing root entry"));
        }
        // Mini FAT.
        let mini_start = u32_at(b, 60).ok_or_else(|| bad("truncated header"))?;
        if mini_start < MAX_REG_SECT {
            let raw = cf.read_chain(mini_start, u64::MAX)?;
            cf.mini_fat = (0..raw.len() / 4).filter_map(|i| u32_at(&raw, i * 4)).collect();
        }
        Ok(cf)
    }

    /// Follows a FAT chain from `start`, reading at most `limit` bytes. A chain longer than the
    /// FAT (so a cycle) or pointing outside the file is an error. The file may end inside the
    /// chain's last sector (some writers don't pad it, and Excel opens those files): that sector
    /// reads short, and the caller checks the stream's declared size against what was read.
    fn read_chain(&self, start: u32, limit: u64) -> Result<Vec<u8>, String> {
        let limit = usize::try_from(limit.min(MAX_STREAM)).unwrap_or(usize::MAX);
        let mut out = Vec::new();
        let mut id = start;
        let mut seen = vec![false; self.fat.len()];
        let mut cut = false;
        while id != END_OF_CHAIN && out.len() < limit {
            if cut {
                return Err("damaged compound file: sector out of range (the file may be truncated)".into());
            }
            if id >= MAX_REG_SECT {
                return Err("damaged compound file: broken sector chain".into());
            }
            match seen.get_mut(id as usize) {
                Some(v) if !*v => *v = true,
                Some(_) => return Err("damaged compound file: sector chain loops".into()),
                None => return Err("damaged compound file: sector missing from the FAT".into()),
            }
            let start = (id as usize).checked_add(1).and_then(|i| i.checked_mul(self.sector_size));
            let s = start
                .and_then(|s| self.bytes.get(s..s.checked_add(self.sector_size)?.min(self.bytes.len())))
                .ok_or("damaged compound file: sector out of range (the file may be truncated)")?;
            cut = s.len() < self.sector_size;
            out.extend_from_slice(s);
            id = *self.fat.get(id as usize).ok_or("damaged compound file: sector missing from the FAT")?;
        }
        Ok(out)
    }

    /// Follows a mini FAT chain inside the mini stream container. Like `read_chain`, the
    /// container may end inside the chain's last mini sector.
    fn read_mini_chain(&self, container: &[u8], start: u32, limit: usize) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let mut id = start;
        let mut seen = vec![false; self.mini_fat.len()];
        let mut cut = false;
        while id != END_OF_CHAIN && out.len() < limit {
            if cut {
                return Err("damaged compound file: mini sector out of range".into());
            }
            if id >= MAX_REG_SECT {
                return Err("damaged compound file: broken mini sector chain".into());
            }
            match seen.get_mut(id as usize) {
                Some(v) if !*v => *v = true,
                Some(_) => return Err("damaged compound file: mini sector chain loops".into()),
                None => return Err("damaged compound file: mini sector missing from the mini FAT".into()),
            }
            let at = (id as usize).checked_mul(self.mini_sector_size);
            let s = at
                .and_then(|a| container.get(a..a.checked_add(self.mini_sector_size)?.min(container.len())))
                .ok_or("damaged compound file: mini sector out of range")?;
            cut = s.len() < self.mini_sector_size;
            out.extend_from_slice(s);
            id = *self.mini_fat.get(id as usize).ok_or("damaged compound file: mini sector missing from the mini FAT")?;
        }
        Ok(out)
    }

    /// The contents of the stream called `name` (case-insensitive) directly under the root
    /// storage, or `None` when there is no such stream.
    /// The directory entry of the stream called `name` (case-insensitive) directly under the
    /// root storage.
    fn root_entry(&self, name: &str) -> Result<Option<&DirEntry>, String> {
        let Some(root) = self.entries.first() else { return Ok(None) };
        // Walk the root's children (a red-black tree through left/right siblings), bounded.
        let mut stack = vec![root.child];
        let mut seen = 0usize;
        while let Some(i) = stack.pop() {
            if i == NO_STREAM {
                continue;
            }
            seen += 1;
            if seen > self.entries.len() {
                return Err("damaged compound file: directory tree loops".into());
            }
            let Some(e) = self.entries.get(i as usize) else { continue };
            if e.kind == STREAM_OBJECT && e.name.eq_ignore_ascii_case(name) {
                return Ok(Some(e));
            }
            stack.push(e.left);
            stack.push(e.right);
        }
        Ok(None)
    }

    /// Whether the root storage holds a stream called `name`, without reading it.
    pub(crate) fn has_root_stream(&self, name: &str) -> bool {
        matches!(self.root_entry(name), Ok(Some(_)))
    }

    /// The contents of the stream called `name` (case-insensitive) directly under the root
    /// storage, or `None` when there is no such stream.
    pub(crate) fn root_stream(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let Some(root) = self.entries.first() else { return Ok(None) };
        let found = self.root_entry(name)?;
        let Some(e) = found else { return Ok(None) };
        if e.size > MAX_STREAM {
            return Err("the workbook stream is larger than 1 GiB".into());
        }
        let size = usize::try_from(e.size).map_err(|_| "stream too large".to_string())?;
        let mut data = if e.size < self.mini_cutoff {
            let container = self.read_chain(root.start, root.size)?;
            self.read_mini_chain(&container, e.start, size)?
        } else {
            self.read_chain(e.start, e.size)?
        };
        if data.len() < size {
            return Err("damaged compound file: a stream is shorter than its declared size (the file may be truncated)".into());
        }
        data.truncate(size);
        Ok(Some(data))
    }
}

// ---- writing -------------------------------------------------------------------------------

/// Sector-id markers in the FAT ([MS-CFB] 2.1).
const FREE_SECT: u32 = 0xFFFF_FFFF;
const FAT_SECT: u32 = 0xFFFF_FFFD;
const DIF_SECT: u32 = 0xFFFF_FFFC;
/// The header holds the first 109 FAT sector ids; DIFAT sectors hold the rest.
const HEADER_DIFAT: usize = 109;
const WRITE_SECTOR: usize = 512;
const WRITE_MINI: usize = 64;
/// Streams smaller than this live in the mini stream.
const MINI_CUTOFF: usize = 4096;

/// A version 3 compound file (512-byte sectors) holding `streams` directly under the root
/// storage, written from the public [MS-CFB] specification. Streams under 4096 bytes go in the
/// mini stream, larger ones in regular sectors; the FAT grows DIFAT sectors as needed, so the
/// only limit is the 1 GiB a stream may hold when read back. Layout: header, FAT, DIFAT,
/// directory, mini FAT, mini stream, then each large stream in order.
pub(crate) fn write(streams: &[(&str, &[u8])]) -> Result<Vec<u8>, String> {
    let to32 = |n: usize| u32::try_from(n).map_err(|_| "the compound file is too large".to_string());
    let total: usize = streams.iter().map(|(_, d)| d.len()).fold(0usize, usize::saturating_add);
    if total as u64 > MAX_STREAM {
        return Err("the file is larger than the 1 GiB a compound file stream may hold".into());
    }
    for (name, _) in streams {
        if name.is_empty() || name.encode_utf16().count() > 31 {
            return Err(format!("a compound file stream name must be 1 to 31 characters: {name:?}"));
        }
    }
    let is_mini = |d: &[u8]| d.len() < MINI_CUTOFF;

    // The mini stream: small streams in 64-byte mini sectors, chained by the mini FAT.
    let mut container = vec![];
    let mut mini_fat: Vec<u32> = vec![];
    let mut mini_starts = vec![];
    for (_, d) in streams {
        if !is_mini(d) || d.is_empty() {
            mini_starts.push(END_OF_CHAIN);
            continue;
        }
        let first = mini_fat.len();
        let n = d.len().div_ceil(WRITE_MINI);
        for k in 0..n {
            mini_fat.push(if k + 1 == n { END_OF_CHAIN } else { to32(first + k + 1)? });
        }
        mini_starts.push(to32(first)?);
        container.extend_from_slice(d);
        container.resize((first + n) * WRITE_MINI, 0);
    }

    // Regular-sector blobs, in order: directory, mini FAT, mini stream, then each large stream.
    let dir_len = (streams.len() + 1).div_ceil(4) * WRITE_SECTOR;
    let mut blobs: Vec<&[u8]> = vec![&[], &[], &container];
    let mini_fat_bytes: Vec<u8> = mini_fat.iter().flat_map(|v| v.to_le_bytes()).collect();
    if let Some(b) = blobs.get_mut(1) {
        *b = &mini_fat_bytes;
    }
    blobs.extend(streams.iter().map(|(_, d)| if is_mini(d) { &[][..] } else { *d }));
    let mut counts: Vec<usize> = blobs.iter().map(|b| b.len().div_ceil(WRITE_SECTOR)).collect();
    if let Some(c) = counts.first_mut() {
        *c = dir_len / WRITE_SECTOR;
    }
    let body: usize = counts.iter().sum();

    // FAT and DIFAT sectors are counted in the sectors the FAT maps: grow both to a fixed point.
    let per = WRITE_SECTOR / 4;
    let mut fat_sectors = 1usize;
    let difat_sectors = loop {
        let difat = fat_sectors.saturating_sub(HEADER_DIFAT).div_ceil(per - 1);
        if fat_sectors * per >= fat_sectors + difat + body {
            break difat;
        }
        fat_sectors += 1;
    };
    let mut fat = vec![FREE_SECT; fat_sectors * per];
    let mut starts = vec![];
    fat.iter_mut().take(fat_sectors).for_each(|s| *s = FAT_SECT);
    fat.iter_mut().skip(fat_sectors).take(difat_sectors).for_each(|s| *s = DIF_SECT);
    let mut next = fat_sectors + difat_sectors;
    for &n in &counts {
        if n == 0 {
            starts.push(END_OF_CHAIN);
            continue;
        }
        starts.push(to32(next)?);
        for k in 0..n {
            let v = if k + 1 == n { END_OF_CHAIN } else { to32(next + k + 1)? };
            if let Some(slot) = fat.get_mut(next + k) {
                *slot = v;
            }
        }
        next += n;
    }
    let start = |i: usize| starts.get(i).copied().unwrap_or(END_OF_CHAIN);

    let sectors = 1 + fat_sectors + difat_sectors + body;
    let mut out = vec![0u8; WRITE_SECTOR * sectors];
    let put16 = |b: &mut [u8], at: usize, v: u16| {
        if let Some(s) = b.get_mut(at..at + 2) {
            s.copy_from_slice(&v.to_le_bytes());
        }
    };
    let put32 = |b: &mut [u8], at: usize, v: u32| {
        if let Some(s) = b.get_mut(at..at + 4) {
            s.copy_from_slice(&v.to_le_bytes());
        }
    };
    let at = |sector: usize| WRITE_SECTOR * (1 + sector);

    // Directory: the root (owning the mini stream), the streams as a balanced binary tree in
    // [MS-CFB] name order (shorter names first, then by upper-case code units), all black.
    let mut dir = vec![0u8; dir_len];
    for k in 0..dir_len / 128 {
        for off in [68, 72, 76] {
            put32(&mut dir, k * 128 + off, NO_STREAM);
        }
    }
    let key = |name: &str| -> (usize, Vec<u16>) {
        let upper: Vec<u16> = name.to_uppercase().encode_utf16().collect();
        (name.encode_utf16().count(), upper)
    };
    let mut order: Vec<usize> = (0..streams.len()).collect();
    order.sort_by_key(|&i| streams.get(i).map(|(n, _)| key(n)));
    let mut links = vec![(NO_STREAM, NO_STREAM); streams.len()];
    // Bottom-up: each sorted span's middle entry has its halves' middles as children.
    let tree_root = balance(&order, &mut links);
    let mut entry = |k: usize, name: &str, kind: u8, start: u32, size: usize, left: u32, right: u32, child: u32| -> Result<(), String> {
        let e = k * 128;
        let units: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        if let Some(s) = dir.get_mut(e..e + units.len()) {
            s.copy_from_slice(&units);
        }
        put16(&mut dir, e + 64, u16::try_from(units.len() + 2).map_err(|_| "a stream name is too long".to_string())?);
        if let Some(b) = dir.get_mut(e + 66) {
            *b = kind;
        }
        if let Some(b) = dir.get_mut(e + 67) {
            *b = 1; // black
        }
        put32(&mut dir, e + 68, left);
        put32(&mut dir, e + 72, right);
        put32(&mut dir, e + 76, child);
        put32(&mut dir, e + 116, start);
        put32(&mut dir, e + 120, to32(size)?);
        Ok(())
    };
    entry(0, "Root Entry", 5, start(2), container.len(), NO_STREAM, NO_STREAM, tree_root)?;
    for (k, (name, data)) in streams.iter().enumerate() {
        let s = if is_mini(data) { mini_starts.get(k).copied().unwrap_or(END_OF_CHAIN) } else { start(3 + k) };
        let (left, right) = links.get(k).copied().unwrap_or((NO_STREAM, NO_STREAM));
        entry(k + 1, name, 2, s, data.len(), left, right, NO_STREAM)?;
    }
    if let Some(b) = blobs.first_mut() {
        *b = &dir;
    }

    // Header.
    if let Some(s) = out.get_mut(..8) {
        s.copy_from_slice(&SIGNATURE);
    }
    put16(&mut out, 24, 0x3E); // minor version
    put16(&mut out, 26, 3); // major version
    put16(&mut out, 28, 0xFFFE); // byte order
    put16(&mut out, 30, 9); // 512-byte sectors
    put16(&mut out, 32, 6); // 64-byte mini sectors
    put32(&mut out, 44, to32(fat_sectors)?);
    put32(&mut out, 48, start(0));
    put32(&mut out, 56, to32(MINI_CUTOFF)?);
    put32(&mut out, 60, start(1)); // first mini FAT sector (END when there is none)
    put32(&mut out, 64, to32(counts.get(1).copied().unwrap_or(0))?);
    put32(&mut out, 68, if difat_sectors > 0 { to32(fat_sectors)? } else { END_OF_CHAIN });
    put32(&mut out, 72, to32(difat_sectors)?);
    for i in 0..HEADER_DIFAT {
        put32(&mut out, 76 + i * 4, if i < fat_sectors { to32(i)? } else { FREE_SECT });
    }
    // DIFAT sectors: 127 FAT sector ids each, then the next DIFAT sector.
    for d in 0..difat_sectors {
        let base = at(fat_sectors + d);
        for j in 0..per - 1 {
            let fat_id = HEADER_DIFAT + d * (per - 1) + j;
            put32(&mut out, base + j * 4, if fat_id < fat_sectors { to32(fat_id)? } else { FREE_SECT });
        }
        let next_difat = if d + 1 < difat_sectors { to32(fat_sectors + d + 1)? } else { END_OF_CHAIN };
        put32(&mut out, base + (per - 1) * 4, next_difat);
    }
    for (i, v) in fat.iter().enumerate() {
        put32(&mut out, at(0) + i * 4, *v);
    }
    for (blob, s) in blobs.iter().zip(&starts) {
        if !blob.is_empty() {
            let from = at(*s as usize);
            if let Some(dst) = out.get_mut(from..from + blob.len()) {
                dst.copy_from_slice(blob);
            }
        }
    }
    Ok(out)
}

/// Links the entries `sorted` (stream indices in name order) into a balanced binary tree:
/// `links[i]` gets entry i's (left, right) siblings as directory ids (stream index + 1). Returns
/// the tree's root id. The depth is log2 of the stream count, which the caller chooses.
fn balance(sorted: &[usize], links: &mut [(u32, u32)]) -> u32 {
    if sorted.is_empty() {
        return NO_STREAM;
    }
    let mid = sorted.len() / 2;
    let (Some(lo), Some(hi)) = (sorted.get(..mid), sorted.get(mid + 1..)) else { return NO_STREAM };
    let left = balance(lo, links);
    let right = balance(hi, links);
    let Some(&me) = sorted.get(mid) else { return NO_STREAM };
    if let Some(l) = links.get_mut(me) {
        *l = (left, right);
    }
    u32::try_from(me + 1).unwrap_or(NO_STREAM)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal valid compound file (512-byte sectors: FAT at sector 0, directory at sector 1)
    /// whose directory holds a root entry plus the given stream names.
    pub(crate) fn build(streams: &[&str]) -> Vec<u8> {
        let mut b = vec![0u8; 512 * 3];
        b[..8].copy_from_slice(&SIGNATURE);
        b[24..26].copy_from_slice(&0x3Eu16.to_le_bytes()); // minor version
        b[26..28].copy_from_slice(&3u16.to_le_bytes()); // major version
        b[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes()); // byte order
        b[30..32].copy_from_slice(&9u16.to_le_bytes()); // sector shift
        b[32..34].copy_from_slice(&6u16.to_le_bytes()); // mini sector shift
        b[44..48].copy_from_slice(&1u32.to_le_bytes()); // FAT sectors
        b[48..52].copy_from_slice(&1u32.to_le_bytes()); // first directory sector
        b[60..64].copy_from_slice(&END_OF_CHAIN.to_le_bytes()); // no mini FAT
        b[68..72].copy_from_slice(&END_OF_CHAIN.to_le_bytes()); // no DIFAT sectors
        for i in 0..109 {
            b[76 + i * 4..80 + i * 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        }
        b[76..80].copy_from_slice(&0u32.to_le_bytes()); // FAT lives in sector 0
        // FAT (sector 0, at byte 512): sector 0 is a FAT sector, sector 1 ends its chain.
        for i in 0..128 {
            b[512 + i * 4..516 + i * 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        }
        b[512..516].copy_from_slice(&0xFFFF_FFFDu32.to_le_bytes());
        b[516..520].copy_from_slice(&END_OF_CHAIN.to_le_bytes());
        // Directory (sector 1, at byte 1024).
        for (k, name) in std::iter::once("Root Entry").chain(streams.iter().copied()).take(4).enumerate() {
            let e = 1024 + k * 128;
            let units: Vec<u16> = name.encode_utf16().take(31).collect();
            for (i, u) in units.iter().enumerate() {
                b[e + i * 2..e + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
            }
            b[e + 64..e + 66].copy_from_slice(&((units.len() as u16 + 1) * 2).to_le_bytes());
            b[e + 66] = if k == 0 { 5 } else { 2 };
        }
        b
    }

    #[test]
    fn reads_directory_names() {
        let b = build(&["EncryptionInfo", "EncryptedPackage"]);
        assert_eq!(directory_names(&b), Some(vec!["Root Entry".into(), "EncryptionInfo".into(), "EncryptedPackage".into()]));
        assert!(is_encrypted(&b));
        assert!(!is_encrypted(&build(&["Workbook", "\u{5}SummaryInformation"])));
    }

    #[test]
    fn hostile_headers_never_panic() {
        let mut b = build(&["Workbook"]);
        for at in [30usize, 48, 68, 76, 512, 516, 1024 + 64] {
            for v in [0u32, 1, 7, 0xFFFF_FFFA, 0xFFFF_FFFF, 0x7FFF_FFFF] {
                let mut c = b.clone();
                c[at..at + 4].copy_from_slice(&v.to_le_bytes());
                let _ = is_encrypted(&c);
                for cut in [0usize, 8, 100, 600, 1100] {
                    let _ = is_encrypted(&c[..cut.min(c.len())]);
                }
            }
        }
        // A directory chain that loops back on itself stops at the bound.
        b[516..520].copy_from_slice(&1u32.to_le_bytes());
        assert!(!is_encrypted(&b));
    }

    /// `compound_file` with one regular-sector stream of `len` bytes (FAT in sector 0, directory
    /// in sector 1, the stream from sector 2), cut right after the stream's last byte, the way
    /// some writers leave the final sector unpadded.
    fn unpadded(len: usize) -> (Vec<u8>, Vec<u8>) {
        let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        let mut file = crate::testing::compound_file(&[("Workbook", &data)]);
        file.truncate(512 * 3 + len);
        (file, data)
    }

    #[test]
    fn an_unpadded_last_sector_still_reads() {
        let (file, data) = unpadded(5000);
        assert_ne!(file.len() % 512, 0);
        let cf = Compound::open(&file).unwrap();
        assert_eq!(cf.root_stream("Workbook").unwrap().unwrap(), data);
        // Bytes the stream needs that the file doesn't hold are still damage.
        let cf = Compound::open(&file[..file.len() - 1]).unwrap();
        assert!(cf.root_stream("Workbook").is_err());
    }

    #[test]
    fn an_unpadded_last_mini_sector_still_reads() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let mut file = crate::testing::compound_file(&[("Workbook", &data)]);
        // Header, FAT, directory, mini FAT, then the mini stream holding the stream's bytes.
        file.truncate(512 * 4 + data.len());
        let cf = Compound::open(&file).unwrap();
        assert_eq!(cf.root_stream("Workbook").unwrap().unwrap(), data);
        let cf = Compound::open(&file[..file.len() - 1]).unwrap();
        assert!(cf.root_stream("Workbook").is_err());
    }

    #[test]
    fn a_cut_sector_inside_a_chain_is_damage() {
        let (mut file, _) = unpadded(5000);
        // The cut sector (11) points on to the directory sector and the stream claims those
        // bytes too: the partial sector would splice 392 bytes into the middle of the stream.
        file[512 + 11 * 4..512 + 12 * 4].copy_from_slice(&1u32.to_le_bytes());
        file[1024 + 128 + 120..1024 + 128 + 124].copy_from_slice(&5512u32.to_le_bytes());
        let cf = Compound::open(&file).unwrap();
        assert!(cf.root_stream("Workbook").is_err());
    }

    #[test]
    fn a_cut_mini_sector_inside_a_chain_is_damage() {
        let small = [7u8; 100];
        let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let mut file = crate::testing::compound_file(&[("Small", &small), ("Workbook", &data)]);
        // "Small" holds mini sectors 0-1, "Workbook" 2-17; the mini stream starts at sector 3.
        file.truncate(512 * 4 + 128 + data.len());
        assert_eq!(Compound::open(&file).unwrap().root_stream("Workbook").unwrap().unwrap(), data);
        // The cut mini sector (17) points on to sector 0 and the stream claims those bytes too.
        file[1536 + 17 * 4..1536 + 18 * 4].copy_from_slice(&0u32.to_le_bytes());
        file[1024 + 2 * 128 + 120..1024 + 2 * 128 + 124].copy_from_slice(&1064u32.to_le_bytes());
        let cf = Compound::open(&file).unwrap();
        assert!(cf.root_stream("Workbook").is_err());
    }
}
