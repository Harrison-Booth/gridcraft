//! Pictures of a `.xls` workbook, from the public [MS-XLS] and [MS-ODRAW] specifications:
//!
//! - the picture store: the MsoDrawingGroup record (2.4.158) holds an OfficeArtBStoreContainer
//!   whose OfficeArtFBSE entries (2.2.32) each hold one OfficeArtBlip (2.2.23 to 2.2.31);
//! - the placed pictures: every MsoDrawing record (2.4.157) of a worksheet holds shapes whose
//!   property table names the store entry (`pib`, 1-based) and whose ClientAnchor (MS-XLS
//!   2.5.19) gives the cells the picture covers and the offsets inside them.
//!
//! PNG, JPEG, TIFF and DIB pictures are kept as they are (a DIB gets the bitmap file header it
//! lacks). Metafile pictures (EMF, WMF, PICT) are deflate-compressed and are skipped.

use std::sync::Arc;

use gridcraft_core::CellRef;
use gridcraft_model::{Anchor, AnchorMode, Image, Sheet};

use super::Budget;
use crate::IoError;

const FBSE: u16 = 0xF007;
const FSP: u16 = 0xF00A;
const FOPT: u16 = 0xF00B;
const CLIENT_ANCHOR: u16 = 0xF010;

/// Shape type of a picture frame (msosptPictureFrame).
const SPT_PICTURE: u16 = 0x004B;
/// Property ids: the picture's store index and its alternative text.
const PIB: u16 = 0x0104;
const WZ_DESCRIPTION: u16 = 0x0381;

/// Bytes of an FBSE before its name and embedded blip.
const FBSE_HEAD: usize = 36;
/// Nesting of OfficeArt containers followed at most.
const MAX_DEPTH: usize = 16;
/// Pictures placed on one sheet at most (each placement walks the sheet's row heights).
const MAX_PICTURES: usize = 2_000;

/// One picture of the store, shared by every shape that shows it.
pub(super) struct Blip {
    data: Arc<Vec<u8>>,
    mime: &'static str,
}

/// The workbook's pictures, one entry per FBSE in order (`None` for one that can't be read), so
/// that a shape's `pib` minus one indexes it.
pub(super) type BlipStore = Vec<Option<Blip>>;

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    let b = d.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes(b.try_into().ok()?))
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    let b = d.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(b.try_into().ok()?))
}

/// UTF-16 text up to its first NUL.
fn utf16z(b: &[u8]) -> String {
    let units: Vec<u16> = (0..b.len() / 2).filter_map(|i| u16_at(b, i * 2)).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}

/// One OfficeArt record (MS-ODRAW 2.2.1): a container (`ver` 0xF) or an atom.
struct Rec<'a> {
    ver: u8,
    inst: u16,
    kind: u16,
    body: &'a [u8],
}

/// Calls `f` for every record of `data`, containers before their children. A record that claims
/// more bytes than are left is cut short.
fn walk<'a>(data: &'a [u8], depth: usize, f: &mut dyn FnMut(&Rec<'a>)) {
    let mut at = 0usize;
    while let (Some(ver_inst), Some(kind), Some(len)) = (u16_at(data, at), u16_at(data, at + 2), u32_at(data, at + 4)) {
        let start = at + 8;
        let end = start.saturating_add(len as usize).min(data.len());
        let body = data.get(start..end).unwrap_or(&[]);
        let rec = Rec { ver: (ver_inst & 0xF) as u8, inst: ver_inst >> 4, kind, body };
        f(&rec);
        if rec.ver == 0xF && depth < MAX_DEPTH {
            walk(body, depth + 1, f);
        }
        at = end;
    }
}

/// A DIB (a bitmap without its file header) as a BMP file.
fn dib_to_bmp(dib: &[u8]) -> Option<Vec<u8>> {
    let head = u32_at(dib, 0)? as usize;
    if head < 40 {
        return None;
    }
    let bits = u16_at(dib, 14)?;
    let compression = u32_at(dib, 16)?;
    let used = u32_at(dib, 32)? as usize;
    let palette = if used > 0 {
        used
    } else if bits <= 8 {
        1usize << bits
    } else {
        0
    };
    // BI_BITFIELDS after a plain header: three colour masks before the pixels.
    let masks = if head == 40 && compression == 3 { 12 } else { 0 };
    let offset = head.checked_add(masks)?.checked_add(palette.checked_mul(4)?)?;
    if offset > dib.len() {
        return None;
    }
    let mut bmp = Vec::with_capacity(dib.len() + 14);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&u32::try_from(dib.len() + 14).ok()?.to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&u32::try_from(offset + 14).ok()?.to_le_bytes());
    bmp.extend_from_slice(dib);
    Some(bmp)
}

/// The picture inside an FBSE body, or `None` when there is none or it isn't a supported kind.
fn fbse(body: &[u8]) -> Option<Blip> {
    let name = usize::from(*body.get(33)?);
    let blip = body.get(FBSE_HEAD.checked_add(name)?..)?;
    let (ver_inst, kind, len) = (u16_at(blip, 0)?, u16_at(blip, 2)?, u32_at(blip, 4)? as usize);
    let payload = blip.get(8..8usize.saturating_add(len).min(blip.len()))?;
    // One 16-byte UID, or two when the instance is odd, then a tag byte, then the picture.
    let skip = if (ver_inst >> 4) & 1 == 1 { 33 } else { 17 };
    let image = payload.get(skip..)?;
    let (data, mime) = match kind {
        0xF01E => (image.to_vec(), "image/png"),
        0xF01D => (image.to_vec(), "image/jpeg"),
        0xF029 => (image.to_vec(), "image/tiff"),
        0xF01F => (dib_to_bmp(image)?, "image/bmp"),
        _ => return None,
    };
    if data.is_empty() {
        return None;
    }
    Some(Blip { data: Arc::new(data), mime })
}

/// Reads the picture store from the MsoDrawingGroup record.
pub(super) fn blip_store(parts: &[&[u8]], budget: &mut Budget) -> Result<BlipStore, IoError> {
    let data = parts.concat();
    let mut store = BlipStore::new();
    walk(&data, 0, &mut |r| {
        if r.kind == FBSE {
            store.push(fbse(r.body));
        }
    });
    for blip in store.iter().flatten() {
        budget.text(blip.data.len())?;
    }
    Ok(store)
}

/// A picture on a sheet: its store index, its ClientAnchor (flags, then column, offset in
/// 1/1024 of the cell's width, row and offset in 1/256 of its height, for the top-left and the
/// bottom-right corner) and its alternative text.
pub(super) struct Placed {
    pib: u32,
    anchor: [u16; 9],
    alt: String,
}

struct Pending {
    spt: u16,
    pib: u32,
    anchor: Option<[u16; 9]>,
    alt: String,
}

fn flush(shape: Option<Pending>, out: &mut Vec<Placed>) {
    if let Some(p) = shape
        && p.spt == SPT_PICTURE
        && p.pib > 0
        && let Some(anchor) = p.anchor
        && out.len() < MAX_PICTURES
    {
        out.push(Placed { pib: p.pib, anchor, alt: p.alt });
    }
}

/// The picture index and alternative text in an OfficeArtFOPT: the property table (six bytes
/// each), then the data of the complex properties in table order.
fn options(r: &Rec<'_>, shape: &mut Pending) {
    let n = usize::from(r.inst).min(r.body.len() / 6);
    let mut complex = r.body.get(n * 6..).unwrap_or(&[]);
    for i in 0..n {
        let (Some(id), Some(val)) = (u16_at(r.body, i * 6), u32_at(r.body, i * 6 + 2)) else { break };
        if id & 0x8000 != 0 {
            let Some((bytes, rest)) = complex.split_at_checked(val as usize) else { break };
            complex = rest;
            if id & 0x3FFF == WZ_DESCRIPTION {
                shape.alt = utf16z(bytes);
            }
        } else if id & 0x3FFF == PIB && id & 0x4000 != 0 {
            shape.pib = val;
        }
    }
}

fn client_anchor(body: &[u8]) -> Option<[u16; 9]> {
    let mut a = [0u16; 9];
    for (i, v) in a.iter_mut().enumerate() {
        *v = u16_at(body, i * 2)?;
    }
    Some(a)
}

/// Gathers the picture shapes of one MsoDrawing record (with its `Continue` records).
pub(super) fn collect(parts: &[&[u8]], out: &mut Vec<Placed>) {
    let data = parts.concat();
    let mut shape: Option<Pending> = None;
    walk(&data, 0, &mut |r| match r.kind {
        FSP => {
            flush(shape.take(), out);
            shape = Some(Pending { spt: r.inst, pib: 0, anchor: None, alt: String::new() });
        }
        FOPT => {
            if let Some(p) = shape.as_mut() {
                options(r, p);
            }
        }
        CLIENT_ANCHOR => {
            if let Some(p) = shape.as_mut() {
                p.anchor = client_anchor(r.body);
            }
        }
        _ => {}
    });
    flush(shape, out);
}

/// Adds the pictures to the sheet, sized from the cells they cover. Returns how many it placed.
pub(super) fn place(shapes: &[Placed], blips: &BlipStore, sheet: &mut Sheet) -> usize {
    let mut placed = 0;
    for p in shapes {
        let Some(Some(blip)) = (p.pib as usize).checked_sub(1).and_then(|i| blips.get(i)) else { continue };
        let [flags, col_l, dx_l, row_t, dy_t, col_r, dx_r, row_b, dy_b] = p.anchor;
        let (col_l, col_r) = (u32::from(col_l.min(255)), u32::from(col_r.min(255)));
        let (row_t, row_b) = (u32::from(row_t), u32::from(row_b));
        let x = |col: u32, dx: u16| sheet.col_left(col) + f64::from(dx.min(1023)) / 1024.0 * f64::from(sheet.col_width(col));
        let y = |row: u32, dy: u16| sheet.row_top(row) + f64::from(dy.min(255)) / 256.0 * f64::from(sheet.row_height(row));
        let (x1, y1, x2, y2) = (x(col_l, dx_l), y(row_t, dy_t), x(col_r, dx_r), y(row_b, dy_b));
        let anchor = Anchor {
            cell: CellRef::new(row_t, col_l),
            dx: (x1 - sheet.col_left(col_l)) as f32,
            dy: (y1 - sheet.row_top(row_t)) as f32,
            width: (x2 - x1).max(0.0) as f32,
            height: (y2 - y1).max(0.0) as f32,
            // Excel writes 0 (move and size with cells), 2 (move but don't size) and 3 (don't
            // move or size).
            mode: match flags & 3 {
                0 => AnchorMode::MoveAndSize,
                3 => AnchorMode::Absolute,
                _ => AnchorMode::MoveOnly,
            },
        };
        sheet.images.push(Image { id: 0, anchor, data: blip.data.clone(), mime: blip.mime.to_string(), alt: p.alt.clone() });
        placed += 1;
    }
    placed
}
