//! Conditional formats and data validation in a BIFF8 worksheet, from the [MS-XLS] CondFmt and
//! CF, CondFmt12 and CF12, CFEx, DXFN (with XFExtNoFRT) and DV structures.
//!
//! A classic CF holds a cell-value or formula rule. Excel 2007 and later add a CFEx to each one
//! with its priority, "stop if true" and rule template (top/bottom, above average, text, dates,
//! duplicates...), and write the rules a CF can't hold (colour scales, data bars, icon sets) as
//! CF12. Rule formulas are parsed formulas whose relative references are offsets from the first
//! cell of the rule's range, as GridCraft stores them.

use std::collections::HashMap;
use std::sync::Arc;

use gridcraft_core::{CellRef, MAX_ROWS, RangeRef};
use gridcraft_formula::{Expr, Reference, SheetSel};
use gridcraft_model::{
    BorderLine, Borders, CfOperator, CfRule, CfValueKind, Color, CondFormat, ErrorStyle, Fill, NumFmt, PatternType, Sheet, Style, Underline,
    Validation, ValidationKind, VertAlign,
};

use super::Budget;
use super::formula::{Links, Mode, decode};
use super::records::Cursor;
use super::styles::{color, line, num_fmt, pattern};
use crate::{IoError, ReadReport};

/// Conditional-format rules kept per sheet at most.
const MAX_RULES: usize = 65_536;
/// Ranges read from one sqref at most.
const MAX_REFS: usize = 8_192;
/// Data validations kept per sheet at most.
const MAX_VALIDATIONS: usize = 65_536;
/// Colour-scale stops and icon-set thresholds read at most (Excel uses up to 3 and 5).
const MAX_STOPS: usize = 16;

pub(super) const CONDFMT: u16 = 0x01B0;
pub(super) const CF: u16 = 0x01B1;
pub(super) const CONDFMT12: u16 = 0x0879;
pub(super) const CF12: u16 = 0x087A;
pub(super) const CFEX: u16 = 0x087B;
pub(super) const DV: u16 = 0x01BE;

/// Excel's icon sets in [MS-XLS] IconSet order.
const ICON_SETS: [&str; 17] = [
    "3Arrows",
    "3ArrowsGray",
    "3Flags",
    "3TrafficLights1",
    "3TrafficLights2",
    "3Signs",
    "3Symbols",
    "3Symbols2",
    "4Arrows",
    "4ArrowsGray",
    "4RedToBlack",
    "4Rating",
    "4TrafficLights",
    "5Arrows",
    "5ArrowsGray",
    "5Rating",
    "5Quarters",
];

/// Number formats a differential format can name: the workbook's DXF records first (Excel
/// numbers the custom formats of conditional formats on its own there), then the Format records
/// and the built-in ones.
pub(super) struct Formats<'a> {
    pub dxf: &'a HashMap<u16, String>,
    pub cells: &'a HashMap<u16, String>,
}

impl Formats<'_> {
    fn get(&self, ifmt: u16) -> NumFmt {
        match self.dxf.get(&ifmt) {
            Some(code) => NumFmt::new(code),
            None => num_fmt(ifmt, self.cells),
        }
    }
}

/// A DXF record in the workbook globals (an XFProps list): keeps a number
/// format it defines (an XFProp format id with its format string).
pub(super) fn dxf_format(c: &mut Cursor<'_, '_>, out: &mut HashMap<u16, String>) {
    let bytes = c.rest();
    let mut r = Bytes::new(&bytes);
    let mut read = || -> Option<(u16, String)> {
        r.take(16)?;
        let n = r.u16()?;
        let (mut id, mut code) = (None, None);
        for _ in 0..n {
            let kind = r.u16()?;
            let cb = usize::from(r.u16()?);
            let body = r.take(cb.checked_sub(4)?)?;
            let mut b = Bytes::new(body);
            match kind {
                0x29 => id = b.u16(),
                0x26 => {
                    let cch = usize::from(b.u16()?);
                    let units: Vec<u16> = b.take(cch.checked_mul(2)?)?.as_chunks::<2>().0.iter().map(|w| u16::from_le_bytes(*w)).collect();
                    code = Some(String::from_utf16_lossy(&units));
                }
                _ => {}
            }
        }
        Some((id?, code?))
    };
    if let Some((id, code)) = read()
        && out.len() < MAX_RULES
        && !code.is_empty()
    {
        out.insert(id, code);
    }
}

/// A little-endian reader over one record's bytes; every read is checked.
struct Bytes<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Bytes<'a> {
    fn new(b: &'a [u8]) -> Self {
        Bytes { b, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.b.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }
    fn u16(&mut self) -> Option<u16> {
        let s = self.take(2)?;
        Some(u16::from_le_bytes([*s.first()?, *s.get(1)?]))
    }
    fn u32(&mut self) -> Option<u32> {
        let s = self.take(4)?;
        Some(u32::from_le_bytes([*s.first()?, *s.get(1)?, *s.get(2)?, *s.get(3)?]))
    }
    fn f64(&mut self) -> Option<f64> {
        let a: [u8; 8] = self.take(8)?.try_into().ok()?;
        Some(f64::from_le_bytes(a))
    }
    fn vec(&mut self, n: usize) -> Option<Vec<u8>> {
        self.take(n).map(<[u8]>::to_vec)
    }
    fn remaining(&self) -> usize {
        self.b.len().saturating_sub(self.pos)
    }
}

/// Ref8U: rows and columns of one range; `None` when it is empty or outside the BIFF8 grid.
fn ref8(r: &mut Bytes<'_>) -> Option<Option<RangeRef>> {
    let (r0, r1, c0, c1) = (r.u16()?, r.u16()?, r.u16()?, r.u16()?);
    let (r0, r1, c0, c1) = (u32::from(r0), u32::from(r1), u32::from(c0), u32::from(c1));
    if r0 > r1 || c0 > c1 || c0 > 255 || r0 >= MAX_ROWS {
        return Some(None);
    }
    Some(Some(RangeRef::new(CellRef::new(r0, c0), CellRef::new(r1.min(MAX_ROWS - 1), c1.min(255)))))
}

/// SqRefU: a count and that many ranges (as many as the record holds).
fn sqref(r: &mut Bytes<'_>) -> Option<Vec<RangeRef>> {
    let n = usize::from(r.u16()?).min(r.remaining() / 8).min(MAX_REFS);
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        if let Some(range) = ref8(r)? {
            out.push(range);
        }
    }
    Some(out)
}

/// A rule threshold (CFVO) before its formula is decoded.
#[derive(Clone, Debug)]
struct Cfvo {
    kind: u8,
    rgce: Vec<u8>,
    value: f64,
}

/// CFVO: type, an optional formula, and the number when there is no formula.
fn cfvo(r: &mut Bytes<'_>) -> Option<Cfvo> {
    let kind = r.u8()?;
    let cce = usize::from(r.u16()?);
    let rgce = r.vec(cce)?;
    let value = if cce == 0 && !matches!(kind, 2 | 3) { r.f64()? } else { 0.0 };
    Some(Cfvo { kind, rgce, value: if value.is_finite() { value } else { 0.0 } })
}

/// CFColor: automatic, a palette index, RGB or a theme colour, and a tint.
fn cf_color(r: &mut Bytes<'_>, table: &[u32; 64]) -> Option<Color> {
    let kind = r.u32()?;
    let v = r.u32()?;
    let tint = r.f64()?;
    Some(match kind {
        1 => color(u16::try_from(v).unwrap_or(u16::MAX), table),
        2 => rgba(v),
        3 => theme(v, tint),
        _ => Color::Auto,
    })
}

/// LongRGBA: red, green, blue and alpha bytes.
fn rgba(v: u32) -> Color {
    let [r, g, b, _] = v.to_le_bytes();
    Color::rgb(r, g, b)
}

fn theme(index: u32, tint: f64) -> Color {
    match u8::try_from(index) {
        Ok(i) if i <= 11 => {
            let t = if tint.is_finite() { (tint * 1000.0).round().clamp(-1000.0, 1000.0) as i16 } else { 0 };
            Color::Theme(i, t)
        }
        _ => Color::Auto,
    }
}

/// What a colour scale, data bar or icon set adds to a CF12.
#[derive(Clone, Debug)]
enum Extra {
    None,
    Scale(Vec<(Cfvo, Color)>),
    Bar { min: Cfvo, max: Cfvo, color: Color, show_value: bool },
    Icons { set: u8, thresholds: Vec<Cfvo>, reverse: bool, show_value: bool },
}

/// The template part of a CF12 or CFEx: which Excel 2007 rule it is.
#[derive(Clone, Copy, Debug)]
struct Template {
    id: u16,
    params: [u8; 16],
}

/// One rule as read, before its formulas are decoded.
#[derive(Clone, Debug)]
struct Raw {
    ct: u8,
    cp: u8,
    f1: Vec<u8>,
    f2: Vec<u8>,
    style: Style,
    priority: Option<u32>,
    stop: Option<bool>,
    template: Option<Template>,
    extra: Extra,
}

/// A CFEx: what Excel 2007 adds to one classic CF.
#[derive(Clone, Debug)]
struct Ext {
    priority: u32,
    stop: bool,
    template: Template,
    style: Option<Style>,
}

/// A CondFmt or CondFmt12 and the rules that follow it.
struct Block {
    id: u16,
    cf12: bool,
    ranges: Vec<RangeRef>,
    rules: Vec<Raw>,
}

/// The rule records of one worksheet, collected while it is read.
#[derive(Default)]
pub(super) struct SheetRules {
    blocks: Vec<Block>,
    ext: HashMap<(u16, u16), Ext>,
    rules: usize,
    validations: Vec<RawDv>,
    malformed: bool,
    too_many: bool,
    /// A CFEx that says the next CF12 belongs to the classic CondFmt with this id.
    cf12_owner: Option<u16>,
}

/// One data validation as read.
struct RawDv {
    flags: u32,
    titles: [String; 4],
    f1: Vec<u8>,
    f2: Vec<u8>,
    ranges: Vec<RangeRef>,
}

impl SheetRules {
    /// A CondFmt, CF, CondFmt12, CF12, CFEx or DV record. A malformed record is skipped (and
    /// reported once); it never stops the sheet from loading.
    pub(super) fn record(
        &mut self,
        id: u16,
        c: &mut Cursor<'_, '_>,
        table: &[u32; 64],
        formats: &Formats<'_>,
        budget: &mut Budget,
    ) -> Result<(), IoError> {
        let bytes = c.rest();
        let mut r = Bytes::new(&bytes);
        let ok = match id {
            CONDFMT => self.cond_fmt(&mut r, false),
            CONDFMT12 => r.take(12).and_then(|_| self.cond_fmt(&mut r, true)),
            CF => self.cf(&mut r, table, formats),
            CF12 => self.cf12(&mut r, table, formats),
            CFEX => self.cf_ex(&mut r, table, formats),
            DV => match self.dv(&mut r) {
                Some(dv) => {
                    budget.text(dv.titles.iter().map(String::len).sum())?;
                    if self.validations.len() < MAX_VALIDATIONS {
                        self.validations.push(dv);
                    } else {
                        self.too_many = true;
                    }
                    Some(())
                }
                None => None,
            },
            _ => Some(()),
        };
        if ok.is_none() {
            self.malformed = true;
        }
        Ok(())
    }

    fn cond_fmt(&mut self, r: &mut Bytes<'_>, cf12: bool) -> Option<()> {
        let _ccf = r.u16()?;
        let id = r.u16()? >> 1;
        let _bound = ref8(r)?;
        let ranges = sqref(r)?;
        self.cf12_owner = None;
        self.blocks.push(Block { id, cf12, ranges, rules: vec![] });
        Some(())
    }

    fn push(&mut self, raw: Raw, cf12: bool) -> Option<()> {
        // A rule belongs to the CondFmt (CF) or CondFmt12 (CF12) right before it, except a CF12
        // announced by a CFEx: that one extends the CondFmt the CFEx names.
        let owner = if cf12 { self.cf12_owner.take() } else { None };
        let block = match owner {
            Some(id) => self.blocks.iter_mut().rev().find(|b| !b.cf12 && b.id == id)?,
            None => self.blocks.last_mut().filter(|b| b.cf12 == cf12)?,
        };
        if self.rules >= MAX_RULES {
            self.too_many = true;
            return Some(());
        }
        self.rules += 1;
        block.rules.push(raw);
        Some(())
    }

    fn cf(&mut self, r: &mut Bytes<'_>, table: &[u32; 64], formats: &Formats<'_>) -> Option<()> {
        let (ct, cp) = (r.u8()?, r.u8()?);
        let (cce1, cce2) = (usize::from(r.u16()?), usize::from(r.u16()?));
        let style = dxfn(r, table, formats)?;
        let f1 = r.vec(cce1)?;
        let f2 = r.vec(cce2)?;
        self.push(Raw { ct, cp, f1, f2, style, priority: None, stop: None, template: None, extra: Extra::None }, false)
    }

    fn cf12(&mut self, r: &mut Bytes<'_>, table: &[u32; 64], formats: &Formats<'_>) -> Option<()> {
        r.take(12)?;
        let (ct, cp) = (r.u8()?, r.u8()?);
        let (cce1, cce2) = (usize::from(r.u16()?), usize::from(r.u16()?));
        let style = dxfn12(r, table, formats)?;
        let f1 = r.vec(cce1)?;
        let f2 = r.vec(cce2)?;
        // fmlaActive: when the rule is active; not kept.
        let cce = usize::from(r.u16()?);
        r.take(cce)?;
        let flags = r.u8()?;
        let priority = u32::from(r.u16()?);
        let template = template(r)?;
        let extra = match ct {
            3 => gradient(r, table)?,
            4 => data_bar(r, table)?,
            6 => icon_set(r)?,
            _ => Extra::None,
        };
        let raw = Raw { ct, cp, f1, f2, style, priority: Some(priority), stop: Some(flags & 0x02 != 0), template: Some(template), extra };
        self.push(raw, true)
    }

    fn cf_ex(&mut self, r: &mut Bytes<'_>, table: &[u32; 64], formats: &Formats<'_>) -> Option<()> {
        r.take(12)?;
        let is_cf12 = r.u32()? & 1 != 0;
        let id = r.u16()?;
        if is_cf12 {
            self.cf12_owner = Some(id);
            return Some(());
        }
        let icf = r.u16()?;
        let _cp = r.u8()?;
        let template_id = r.u8()?;
        let priority = u32::from(r.u16()?);
        let flags = r.u8()?;
        let has_dxf = r.u8()? != 0;
        let style = if has_dxf { Some(dxfn12(r, table, formats)?) } else { None };
        let template = Template { id: u16::from(template_id), params: template_params(r)? };
        if self.ext.len() < MAX_RULES {
            self.ext.insert((id, icf), Ext { priority, stop: flags & 0x02 != 0, template, style });
        }
        Some(())
    }

    fn dv(&mut self, r: &mut Bytes<'_>) -> Option<RawDv> {
        let flags = r.u32()?;
        let mut titles: [String; 4] = Default::default();
        for t in titles.iter_mut() {
            *t = unicode_string(r)?;
        }
        let cce1 = usize::from(r.u16()?);
        r.take(2)?;
        let f1 = r.vec(cce1)?;
        let cce2 = usize::from(r.u16()?);
        r.take(2)?;
        let f2 = r.vec(cce2)?;
        let ranges = sqref(r)?;
        Some(RawDv { flags, titles, f1, f2, ranges })
    }

    /// Adds the sheet's conditional formats and validations, with their formulas decoded.
    pub(super) fn finish(self, links: &Links, sheet: &mut Sheet, report: &mut ReadReport) {
        let name = sheet.name.clone();
        let cx = Cx { links, sheet: &name };
        let mut skipped: Vec<&'static str> = vec![];
        let mut order = 0u32;
        let mut explicit = false;
        let mut out: Vec<CondFormat> = vec![];
        for block in &self.blocks {
            let Some(first) = block.ranges.first().map(|r| r.start) else { continue };
            for (icf, raw) in block.rules.iter().enumerate() {
                order += 1;
                let ext = if block.cf12 { None } else { u16::try_from(icf).ok().and_then(|i| self.ext.get(&(block.id, i))) };
                let style = ext.and_then(|e| e.style.clone()).unwrap_or_else(|| raw.style.clone());
                let template = ext.map(|e| e.template).or(raw.template);
                let priority = ext.map(|e| e.priority).or(raw.priority);
                explicit |= priority.is_some();
                // Excel 97-2003 applies the first rule that matches, as "stop if true" does.
                let stop = ext.map(|e| e.stop).or(raw.stop).unwrap_or(true);
                match rule(raw, template, style, first, &cx) {
                    Ok(rule) => out.push(CondFormat { ranges: block.ranges.clone(), rule, priority: priority.unwrap_or(order), stop_if_true: stop }),
                    Err(why) => {
                        if !skipped.contains(&why) {
                            skipped.push(why);
                        }
                    }
                }
            }
        }
        // Without CFEx records the file order is the priority order.
        if !explicit {
            for (i, cf) in out.iter_mut().enumerate() {
                cf.priority = u32::try_from(i + 1).unwrap_or(u32::MAX);
            }
        }
        sheet.cond_formats.extend(out);
        for raw in &self.validations {
            match validation(raw, &cx) {
                Ok(Some(v)) => sheet.validations.push(v),
                Ok(None) => {}
                Err(why) => {
                    if !skipped.contains(&why) {
                        skipped.push(why);
                    }
                }
            }
        }
        if self.malformed {
            report.warn(format!("Sheet '{}': some conditional formatting or data validation records were malformed and were skipped.", sheet.name));
        }
        if self.too_many {
            report.warn(format!("Sheet '{}': conditional formatting or data validation rules past the first {MAX_RULES} were skipped.", sheet.name));
        }
        for why in skipped {
            report.warn(format!("Sheet '{}': some conditional formatting or data validation rules use {why} and were skipped.", sheet.name));
        }
    }
}

/// XLUnicodeString; Excel writes a single NUL for an empty title or message.
fn unicode_string(r: &mut Bytes<'_>) -> Option<String> {
    let cch = usize::from(r.u16()?);
    let high = r.u8()? & 1 != 0;
    let units: Vec<u16> = if high {
        r.take(cch.checked_mul(2)?)?.as_chunks::<2>().0.iter().map(|w| u16::from_le_bytes(*w)).collect()
    } else {
        r.take(cch)?.iter().map(|&b| u16::from(b)).collect()
    };
    Some(String::from_utf16_lossy(&units).chars().filter(|c| *c != '\0').collect())
}

/// A CF12's template id and its parameters.
fn template(r: &mut Bytes<'_>) -> Option<Template> {
    let id = r.u16()?;
    Some(Template { id, params: template_params(r)? })
}

/// A byte count and the template parameters (16 bytes when present).
fn template_params(r: &mut Bytes<'_>) -> Option<[u8; 16]> {
    let n = usize::from(r.u8()?);
    let mut params = [0u8; 16];
    for (slot, b) in params.iter_mut().zip(r.take(n)?.iter()) {
        *slot = *b;
    }
    Some(params)
}

/// CFGradient: interpolation points, then the colours at them.
fn gradient(r: &mut Bytes<'_>, table: &[u32; 64]) -> Option<Extra> {
    r.take(3)?;
    let n_interp = usize::from(r.u8()?);
    let n_curve = usize::from(r.u8()?);
    r.u8()?;
    let mut points = vec![];
    for _ in 0..n_interp {
        let v = cfvo(r)?;
        r.f64()?;
        if points.len() < MAX_STOPS {
            points.push(v);
        }
    }
    let mut colors = vec![];
    for _ in 0..n_curve {
        r.f64()?;
        let c = cf_color(r, table)?;
        if colors.len() < MAX_STOPS {
            colors.push(c);
        }
    }
    Some(Extra::Scale(points.into_iter().zip(colors).collect()))
}

/// CFDatabar: flags, bar lengths, colour, then the two bounds.
fn data_bar(r: &mut Bytes<'_>, table: &[u32; 64]) -> Option<Extra> {
    r.take(3)?;
    let flags = r.u8()?;
    r.take(2)?;
    let color = cf_color(r, table)?;
    let min = cfvo(r)?;
    let max = cfvo(r)?;
    Some(Extra::Bar { min, max, color, show_value: flags & 0x02 == 0 })
}

/// CFMultistate: the icon set, its flags and one threshold per icon.
fn icon_set(r: &mut Bytes<'_>) -> Option<Extra> {
    r.take(3)?;
    let n = usize::from(r.u8()?);
    let set = r.u8()?;
    let flags = r.u8()?;
    let mut thresholds = vec![];
    for _ in 0..n {
        let v = cfvo(r)?;
        r.take(5)?;
        if thresholds.len() < MAX_STOPS {
            thresholds.push(v);
        }
    }
    Some(Extra::Icons { set, thresholds, reverse: flags & 0x04 != 0, show_value: flags & 0x01 == 0 })
}

/// DXFN12: a byte count, then a DXFN and its XFExtNoFRT (two reserved bytes when empty).
fn dxfn12(r: &mut Bytes<'_>, table: &[u32; 64], formats: &Formats<'_>) -> Option<Style> {
    let cb = usize::try_from(r.u32()?).ok()?;
    if cb == 0 {
        r.take(2)?;
        return Some(Style::default());
    }
    let body = r.take(cb)?;
    let mut b = Bytes::new(body);
    let mut parts = dxfn_parts(&mut b, table, formats)?;
    if b.remaining() > 0 {
        xf_ext(&mut b, &mut parts);
    }
    Some(parts.finish())
}

/// DXFN on its own (CF).
fn dxfn(r: &mut Bytes<'_>, table: &[u32; 64], formats: &Formats<'_>) -> Option<Style> {
    Some(dxfn_parts(r, table, formats)?.finish())
}

/// A differential format as read: the fill stays in parts until the colours are final.
struct Parts {
    style: Style,
    /// The pattern, or `None` when the format doesn't set it.
    pattern: Option<PatternType>,
    fg: Option<Color>,
    bg: Option<Color>,
}

impl Parts {
    /// As for a `dxf` in .xlsx: a solid (or unspecified) pattern fills with the background
    /// colour, or else the foreground one.
    fn finish(mut self) -> Style {
        let (fg, bg) = (self.fg.unwrap_or(Color::Auto), self.bg.unwrap_or(Color::Auto));
        self.style.fill = match self.pattern {
            None | Some(PatternType::Solid) => {
                let c = if bg != Color::Auto { bg } else { fg };
                if c == Color::Auto && self.pattern.is_none() { Fill::default() } else { Fill::solid(c) }
            }
            Some(PatternType::None) => Fill::default(),
            Some(p) => Fill { pattern: p, fg, bg },
        };
        self.style
    }
}

/// DXFN: which parts are set, then the number format, font, alignment, borders, fill and
/// protection that are present. Only the parts a conditional format can set are kept.
fn dxfn_parts(r: &mut Bytes<'_>, table: &[u32; 64], formats: &Formats<'_>) -> Option<Parts> {
    let flags = r.u32()?;
    let flags2 = r.u16()?;
    let unset = |bit: u32| flags & (1 << bit) != 0;
    let has = |bit: u32| flags & (1 << bit) != 0;
    let mut p = Parts { style: Style::default(), pattern: None, fg: None, bg: None };
    if has(25) {
        let fmt = if flags2 & 1 != 0 {
            let cb = usize::from(r.u16()?);
            let body = r.take(cb.saturating_sub(2))?;
            let mut b = Bytes::new(body);
            unicode_string(&mut b).map(|s| NumFmt::new(&s))
        } else {
            r.u8()?;
            Some(formats.get(u16::from(r.u8()?)))
        };
        if !unset(19)
            && let Some(f) = fmt
        {
            p.style.num_fmt = f;
        }
    }
    if has(26) {
        font(r, table, &mut p.style)?;
    }
    if has(27) {
        r.take(8)?;
    }
    if has(28) {
        let b1 = r.u32()?;
        let b2 = r.u32()?;
        let side = |bit: u32, l: BorderLine| if unset(bit) { BorderLine::default() } else { l };
        let mut borders = Borders {
            left: side(10, line(b1 & 0x0F, (b1 >> 16) & 0x7F, table)),
            right: side(11, line((b1 >> 4) & 0x0F, (b1 >> 23) & 0x7F, table)),
            top: side(12, line((b1 >> 8) & 0x0F, b2 & 0x7F, table)),
            bottom: side(13, line((b1 >> 12) & 0x0F, (b2 >> 7) & 0x7F, table)),
            ..Borders::default()
        };
        let diag = line((b2 >> 21) & 0x0F, (b2 >> 14) & 0x7F, table);
        if b1 & 0x4000_0000 != 0 && !unset(14) {
            borders.diag_down = diag;
        }
        if b1 & 0x8000_0000 != 0 && !unset(15) {
            borders.diag_up = diag;
        }
        p.style.borders = borders;
    }
    if has(29) {
        let a = r.u16()?;
        let b = r.u16()?;
        if !unset(16) {
            p.pattern = Some(pattern((a >> 10) & 0x3F));
        }
        if !unset(17) {
            p.fg = Some(color(b & 0x7F, table));
        }
        if !unset(18) {
            p.bg = Some(color((b >> 7) & 0x7F, table));
        }
    }
    if has(30) {
        r.take(2)?;
    }
    Some(p)
}

/// DXFFntD: the font parts that are set. As for a .xlsx `dxf`, the face and size stay the
/// default ones.
fn font(r: &mut Bytes<'_>, table: &[u32; 64], s: &mut Style) -> Option<()> {
    // Name (with its unused tail, 64 bytes), then Stxp.
    r.take(64)?;
    let _height = r.u32()?;
    let ts = r.u32()?;
    let bls = r.u16()?;
    let sss = r.u16()?;
    let uls = r.u8()?;
    r.take(3)?;
    let icv = r.u32()?;
    r.take(4)?;
    let ts_unset = r.u32()?;
    let sss_unset = r.u32()? != 0;
    let uls_unset = r.u32()? != 0;
    let bls_unset = r.u32()? != 0;
    r.take(14)?;
    let f = &mut s.font;
    if ts_unset & 0x02 == 0 {
        f.italic = ts & 0x02 != 0;
    }
    if ts_unset & 0x80 == 0 {
        f.strike = ts & 0x80 != 0;
    }
    if !bls_unset {
        f.bold = bls >= 600;
    }
    if !sss_unset {
        f.vert = match sss {
            1 => VertAlign::Superscript,
            2 => VertAlign::Subscript,
            _ => VertAlign::Baseline,
        };
    }
    if !uls_unset {
        f.underline = match uls {
            0x01 => Underline::Single,
            0x02 => Underline::Double,
            0x21 => Underline::SingleAccounting,
            0x22 => Underline::DoubleAccounting,
            _ => Underline::None,
        };
    }
    if icv != u32::MAX {
        f.color = color(u16::try_from(icv).unwrap_or(u16::MAX), table);
    }
    Some(())
}

/// XFExtNoFRT: exact (RGB or theme) colours that replace the palette ones.
fn xf_ext(r: &mut Bytes<'_>, p: &mut Parts) -> Option<()> {
    r.take(6)?;
    let n = r.u16()?;
    for _ in 0..n {
        let kind = r.u16()?;
        let cb = usize::from(r.u16()?);
        let body = r.take(cb.checked_sub(4)?)?;
        let mut b = Bytes::new(body);
        let Some(c) = full_color(&mut b) else { continue };
        let border = |l: &mut BorderLine| {
            if l.style != Default::default() {
                l.color = c;
            }
        };
        match kind {
            0x04 => p.fg = Some(c),
            0x05 => p.bg = Some(c),
            0x07 => border(&mut p.style.borders.top),
            0x08 => border(&mut p.style.borders.bottom),
            0x09 => border(&mut p.style.borders.left),
            0x0A => border(&mut p.style.borders.right),
            0x0D => p.style.font.color = c,
            _ => {}
        }
    }
    Some(())
}

/// FullColorExt: RGB or theme colours (with their tint) are taken; others are left alone.
fn full_color(r: &mut Bytes<'_>) -> Option<Color> {
    let kind = r.u16()?;
    let tint = r.u16()? as i16;
    let v = r.u32()?;
    match kind {
        2 if tint == 0 => Some(rgba(v)),
        3 => Some(theme(v, f64::from(tint) / 32767.0)),
        _ => None,
    }
}

/// A rule formula as GridCraft text (no `=`), relative to `base`.
fn formula(rgce: &[u8], base: CellRef, cx: &Cx<'_>) -> Result<String, &'static str> {
    expr(rgce, base, cx).map(|e| gridcraft_formula::print(&e))
}

/// Where rule formulas are decoded: the workbook's links and the sheet's own name.
struct Cx<'a> {
    links: &'a Links,
    sheet: &'a str,
}

/// Decodes a rule formula. Excel writes references to the rule's own sheet as 3-D references
/// in some places (colour-scale, data-bar and icon-set thresholds); they become plain ones.
fn expr(rgce: &[u8], base: CellRef, cx: &Cx<'_>) -> Result<Expr, &'static str> {
    let e = decode(rgce, &[], base, Mode::Shared, cx.links).map_err(|u| u.0)?;
    Ok(e.map(&mut |x| match x {
        Expr::Ref(Reference { sheet: SheetSel::Named(n), kind }) if n.eq_ignore_ascii_case(cx.sheet) => {
            Expr::Ref(Reference { sheet: SheetSel::Current, kind })
        }
        other => other,
    }))
}

fn threshold(v: &Cfvo, base: CellRef, cx: &Cx<'_>) -> Result<CfValueKind, &'static str> {
    let f = |rgce: &[u8]| formula(rgce, base, cx).map(CfValueKind::Formula);
    Ok(match v.kind {
        2 => CfValueKind::Min,
        3 => CfValueKind::Max,
        _ if !v.rgce.is_empty() => f(&v.rgce)?,
        4 => CfValueKind::Percent(v.value),
        5 => CfValueKind::Percentile(v.value),
        _ => CfValueKind::Number(v.value),
    })
}

fn operator(cp: u8) -> CfOperator {
    match cp {
        2 => CfOperator::NotBetween,
        3 => CfOperator::Equal,
        4 => CfOperator::NotEqual,
        5 => CfOperator::Greater,
        6 => CfOperator::Less,
        7 => CfOperator::GreaterOrEqual,
        8 => CfOperator::LessOrEqual,
        _ => CfOperator::Between,
    }
}

/// The first text constant in a formula (the text of a "contains" rule).
fn first_text(e: Expr) -> Option<String> {
    let mut found: Option<Arc<str>> = None;
    e.map(&mut |x| {
        if found.is_none()
            && let Expr::Text(t) = &x
        {
            found = Some(t.clone());
        }
        x
    });
    found.map(|t| t.to_string())
}

/// The model rule of one CF or CF12.
fn rule(raw: &Raw, template: Option<Template>, style: Style, base: CellRef, cx: &Cx<'_>) -> Result<CfRule, &'static str> {
    let style = Box::new(style);
    let p = template.map(|t| t.params).unwrap_or_default();
    let param = u16::from_le_bytes([p[0], p[1]]);
    let period = |s: &str| Ok(CfRule::TimePeriod { period: s.to_string(), style: style.clone() });
    match template.map(|t| t.id) {
        Some(0x05) => {
            let rank = u16::from_le_bytes([p[1], p[2]]);
            return Ok(CfRule::Top10 { bottom: p[0] & 1 == 0, percent: p[0] & 2 != 0, rank: u32::from(rank), style });
        }
        Some(0x07) => return Ok(CfRule::Unique { style }),
        Some(0x1B) => return Ok(CfRule::Duplicate { style }),
        Some(0x08) => {
            let text = expr(&raw.f1, base, cx).ok().and_then(first_text);
            if let Some(text) = text {
                return Ok(match param {
                    1 => CfRule::NotContainsText { text, style },
                    2 => CfRule::BeginsWith { text, style },
                    3 => CfRule::EndsWith { text, style },
                    _ => CfRule::ContainsText { text, style },
                });
            }
        }
        Some(0x09) => return Ok(CfRule::Blanks { style }),
        Some(0x0A) => return Ok(CfRule::NoBlanks { style }),
        Some(0x0B) => return Ok(CfRule::Errors { style }),
        Some(0x0C) => return Ok(CfRule::NoErrors { style }),
        Some(0x0F) => return period("today"),
        Some(0x10) => return period("tomorrow"),
        Some(0x11) => return period("yesterday"),
        Some(0x12) => return period("last7Days"),
        Some(0x13) => return period("lastMonth"),
        Some(0x14) => return period("nextMonth"),
        Some(0x15) => return period("thisWeek"),
        Some(0x16) => return period("nextWeek"),
        Some(0x17) => return period("lastWeek"),
        Some(0x18) => return period("thisMonth"),
        Some(id @ (0x19 | 0x1A | 0x1D | 0x1E)) => {
            return Ok(CfRule::AboveAverage {
                below: matches!(id, 0x1A | 0x1E),
                equal: matches!(id, 0x1D | 0x1E),
                std_dev: u8::try_from(param.min(3)).unwrap_or(0),
                style,
            });
        }
        _ => {}
    }
    match (&raw.extra, raw.ct) {
        (Extra::Scale(stops), _) => {
            let stops = stops.iter().map(|(v, c)| Ok((threshold(v, base, cx)?, *c))).collect::<Result<Vec<_>, &'static str>>()?;
            Ok(CfRule::ColorScale { stops })
        }
        (Extra::Bar { min, max, color, show_value }, _) => Ok(CfRule::DataBar {
            min: threshold(min, base, cx)?,
            max: threshold(max, base, cx)?,
            color: *color,
            gradient: true,
            show_value: *show_value,
        }),
        (Extra::Icons { set, thresholds, reverse, show_value }, _) => Ok(CfRule::IconSet {
            set: ICON_SETS.get(usize::from(*set)).copied().unwrap_or("3TrafficLights1").to_string(),
            thresholds: thresholds.iter().map(|v| threshold(v, base, cx)).collect::<Result<Vec<_>, _>>()?,
            reverse: *reverse,
            show_value: *show_value,
        }),
        (Extra::None, 1) => {
            let a = formula(&raw.f1, base, cx)?;
            let b = if raw.f2.is_empty() { None } else { Some(formula(&raw.f2, base, cx)?) };
            Ok(CfRule::CellIs { op: operator(raw.cp), a, b, style })
        }
        (Extra::None, 2) => Ok(CfRule::Expression { formula: formula(&raw.f1, base, cx)?, style }),
        _ => Err("a rule type GridCraft can't read from .xls yet"),
    }
}

/// The model validation of one DV, or `None` when it covers no cells.
fn validation(raw: &RawDv, cx: &Cx<'_>) -> Result<Option<Validation>, &'static str> {
    let Some(first) = raw.ranges.first().map(|r| r.start) else { return Ok(None) };
    let flags = raw.flags;
    let kind = match flags & 0x0F {
        1 => ValidationKind::Whole,
        2 => ValidationKind::Decimal,
        3 => ValidationKind::List,
        4 => ValidationKind::Date,
        5 => ValidationKind::Time,
        6 => ValidationKind::TextLength,
        7 => ValidationKind::Custom,
        _ => ValidationKind::Any,
    };
    let explicit_list = flags & 0x80 != 0;
    let f1 = if raw.f1.is_empty() {
        String::new()
    } else {
        let e = expr(&raw.f1, first, cx)?;
        match (kind, e) {
            // An explicit list is one text constant with the items separated by NULs.
            (ValidationKind::List, Expr::Text(t)) if explicit_list => format!("\"{}\"", t.replace('\0', ",")),
            (ValidationKind::List, e) => format!("={}", gridcraft_formula::print(&e)),
            (_, e) => gridcraft_formula::print(&e),
        }
    };
    let f2 = if raw.f2.is_empty() { None } else { Some(formula(&raw.f2, first, cx)?) };
    let [input_title, error_title, input_message, error_message] = raw.titles.clone();
    Ok(Some(Validation {
        ranges: raw.ranges.clone(),
        kind,
        op: match (flags >> 20) & 0x0F {
            1 => CfOperator::NotBetween,
            2 => CfOperator::Equal,
            3 => CfOperator::NotEqual,
            4 => CfOperator::Greater,
            5 => CfOperator::Less,
            6 => CfOperator::GreaterOrEqual,
            7 => CfOperator::LessOrEqual,
            _ => CfOperator::Between,
        },
        f1,
        f2,
        allow_blank: flags & 0x100 != 0,
        in_cell_dropdown: flags & 0x200 == 0,
        input_title,
        input_message,
        show_input: flags & 0x4_0000 != 0,
        error_title,
        error_message,
        error_style: match (flags >> 4) & 0x07 {
            1 => ErrorStyle::Warning,
            2 => ErrorStyle::Information,
            _ => ErrorStyle::Stop,
        },
        show_error: flags & 0x8_0000 != 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::super::formula::{SupBook, Xti};
    use super::*;
    use crate::tables::INDEXED;
    use gridcraft_model::{BorderStyle, Font};

    fn u16s(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn ref8(r0: u16, r1: u16, c0: u16, c1: u16) -> Vec<u8> {
        u16s(&[r0, r1, c0, c1])
    }

    /// CondFmt (or the body of a CondFmt12): rule count, id, bounds, ranges.
    fn cond_fmt(id: u16, ranges: &[[u16; 4]]) -> Vec<u8> {
        let mut p = u16s(&[ranges.len() as u16, id << 1]);
        p.extend(ref8(0, 0, 0, 0));
        p.extend(u16s(&[ranges.len() as u16]));
        for r in ranges {
            p.extend(ref8(r[0], r[1], r[2], r[3]));
        }
        p
    }

    /// The parts a DXFN sets: everything else is marked unset.
    #[derive(Default)]
    struct Dxf {
        /// Solid fill: the background colour index.
        fill: Option<u16>,
        /// Patterned fill: pattern, foreground, background.
        pattern: Option<(u16, u16, u16)>,
        /// Bold, italic, colour index.
        font: Option<(bool, bool, u32)>,
        /// Thin borders on all four sides in this colour.
        border: Option<u32>,
        /// A number format id.
        ifmt: Option<u8>,
    }

    fn dxfn(d: &Dxf) -> Vec<u8> {
        let mut flags: u32 = 0xFFFF | (1 << 16) | (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20);
        let mut body = vec![];
        if let Some(i) = d.ifmt {
            flags |= 1 << 25;
            flags &= !(1 << 19);
            body.extend([0, i]);
        }
        if let Some((bold, italic, icv)) = d.font {
            flags |= 1 << 26;
            let mut f = vec![0u8; 64];
            f.extend(u32::MAX.to_le_bytes());
            f.extend((if italic { 2u32 } else { 0 }).to_le_bytes());
            f.extend(u16s(&[if bold { 700 } else { 400 }, 0xFFFF]));
            f.extend([0xFF, 0, 0, 0]);
            f.extend(icv.to_le_bytes());
            f.extend([0; 4]);
            f.extend((!2u32).to_le_bytes()); // italic set, strikethrough unset
            f.extend(1u32.to_le_bytes());
            f.extend(1u32.to_le_bytes());
            f.extend(0u32.to_le_bytes()); // weight set
            f.extend([0; 14]);
            assert_eq!(f.len(), 118);
            body.extend(f);
        }
        if let Some(icv) = d.border {
            flags |= 1 << 28;
            flags &= !(0xF << 10);
            body.extend((0x1111u32 | (icv << 16) | (icv << 23)).to_le_bytes());
            body.extend((icv | (icv << 7)).to_le_bytes());
        }
        if let Some(bg) = d.fill {
            flags |= 1 << 29;
            flags &= !(1 << 18);
            body.extend(u16s(&[0, 0x40 | (bg << 7)]));
        }
        if let Some((fls, fg, bg)) = d.pattern {
            flags |= 1 << 29;
            flags &= !((1 << 16) | (1 << 17) | (1 << 18));
            body.extend(u16s(&[fls << 10, fg | (bg << 7)]));
        }
        [flags.to_le_bytes().to_vec(), u16s(&[0x8002]), body].concat()
    }

    fn cf(ct: u8, cp: u8, d: &Dxf, f1: &[u8], f2: &[u8]) -> Vec<u8> {
        [vec![ct, cp], u16s(&[f1.len() as u16, f2.len() as u16]), dxfn(d), f1.to_vec(), f2.to_vec()].concat()
    }

    /// CFEx for the classic rule `icf` of CondFmt `id`.
    fn cf_ex(id: u16, icf: u16, template: u8, priority: u16, stop: bool, params: &[u8]) -> Vec<u8> {
        let mut p = vec![0x7B, 0x08, 1, 0];
        p.extend([0; 8]);
        p.extend(0u32.to_le_bytes());
        p.extend(u16s(&[id, icf]));
        p.extend([0, template]);
        p.extend(u16s(&[priority]));
        p.extend([1 | if stop { 2 } else { 0 }, 0, 16]);
        let mut t = params.to_vec();
        t.resize(16, 0);
        p.extend(t);
        p
    }

    /// CFEx announcing that the next CF12 extends CondFmt `id`.
    fn cf_ex12(id: u16) -> Vec<u8> {
        [vec![0x7B, 0x08, 1, 0], vec![0; 8], 1u32.to_le_bytes().to_vec(), u16s(&[id])].concat()
    }

    /// CF12 with no differential format; `tail` is the CFGradient / CFDatabar / CFMultistate.
    fn cf12(ct: u8, f1: &[u8], priority: u16, template: u16, params: &[u8], tail: &[u8]) -> Vec<u8> {
        let mut p = vec![0x7A, 0x08, 0, 0];
        p.extend([0; 8]);
        p.extend([ct, 0]);
        p.extend(u16s(&[f1.len() as u16, 0]));
        p.extend(0u32.to_le_bytes());
        p.extend([0, 0]);
        p.extend(f1);
        p.extend(u16s(&[0]));
        p.push(1);
        p.extend(u16s(&[priority, template]));
        p.push(16);
        let mut t = params.to_vec();
        t.resize(16, 0);
        p.extend(t);
        p.extend(tail);
        p
    }

    fn cfvo(kind: u8, value: Option<f64>, rgce: &[u8]) -> Vec<u8> {
        let mut p = vec![kind];
        p.extend(u16s(&[rgce.len() as u16]));
        p.extend(rgce);
        if let Some(v) = value {
            p.extend(v.to_le_bytes());
        }
        p
    }

    fn rgb_color(rgb: u32) -> Vec<u8> {
        let [b, g, r, _] = rgb.to_le_bytes();
        [2u32.to_le_bytes().to_vec(), vec![r, g, b, 0xFF], 0f64.to_le_bytes().to_vec()].concat()
    }

    fn ptg_int(n: u16) -> Vec<u8> {
        [vec![0x1E], u16s(&[n])].concat()
    }

    /// PtgRefN: an offset from the rule's first cell (both parts relative).
    fn ptg_ref_n(dr: u16, dc: u16) -> Vec<u8> {
        [vec![0x4C], u16s(&[dr, 0xC000 | dc])].concat()
    }

    fn ptg_str(s: &str) -> Vec<u8> {
        [vec![0x17, s.len() as u8, 0], s.as_bytes().to_vec()].concat()
    }

    /// PtgRef3d to an absolute cell through XTI `ixti`.
    fn ptg_ref3d(ixti: u16, r: u16, c: u16) -> Vec<u8> {
        [vec![0x5A], u16s(&[ixti, r, c])].concat()
    }

    fn links() -> Links {
        Links {
            sheets: vec!["Data".into()],
            names: vec![],
            skipped: vec![],
            books: vec![SupBook { internal: true, addin: false, names: vec![] }],
            xti: vec![Xti { book: 0, first: 0, last: 0 }],
        }
    }

    fn run(records: &[(u16, Vec<u8>)]) -> (Sheet, ReadReport) {
        let mut rules = SheetRules::default();
        let mut budget = Budget::default();
        let (dxf, cells) = (HashMap::from([(14u16, "0.00%".to_string())]), HashMap::new());
        let formats = Formats { dxf: &dxf, cells: &cells };
        for (id, payload) in records {
            let parts = [payload.as_slice()];
            let mut c = Cursor::new(&parts);
            rules.record(*id, &mut c, &INDEXED, &formats, &mut budget).unwrap();
        }
        let mut sheet = Sheet::new("Data");
        let mut report = ReadReport::default();
        rules.finish(&links(), &mut sheet, &mut report);
        (sheet, report)
    }

    fn range(a: &str) -> RangeRef {
        RangeRef::parse(a).unwrap()
    }

    fn solid(c: u32) -> Style {
        Style { fill: Fill::solid(Color::Rgb(c)), ..Style::default() }
    }

    #[test]
    fn reads_cell_value_and_formula_rules_with_their_formats() {
        let thin = BorderLine { style: BorderStyle::Thin, color: Color::Rgb(0x0000FF) };
        let (sheet, report) = run(&[
            (CONDFMT, cond_fmt(1, &[[0, 9, 0, 0]])),
            (CF, cf(1, 5, &Dxf { fill: Some(13), ..Dxf::default() }, &ptg_int(5), &[])),
            (CF, cf(1, 1, &Dxf { font: Some((true, false, 16)), border: Some(12), ..Dxf::default() }, &ptg_int(2), &ptg_int(4))),
            (CONDFMT, cond_fmt(2, &[[1, 4, 1, 1], [1, 4, 3, 3]])),
            // B2>0, written relative to the rule's first cell; format 14 comes from a DXF record.
            (
                CF,
                cf(
                    2,
                    0,
                    &Dxf { pattern: Some((9, 10, 13)), ifmt: Some(14), ..Dxf::default() },
                    &[ptg_ref_n(0, 0), ptg_int(0), vec![0x0D]].concat(),
                    &[],
                ),
            ),
        ]);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let cfs = &sheet.cond_formats;
        assert_eq!(cfs.len(), 3);
        assert_eq!(cfs[0].ranges, vec![range("A1:A10")]);
        assert_eq!(cfs[0].rule, CfRule::CellIs { op: CfOperator::Greater, a: "5".into(), b: None, style: Box::new(solid(0xFFFF00)) });
        let style = Style {
            font: Font { bold: true, color: Color::Rgb(0x800000), ..Font::default() },
            borders: Borders { left: thin, right: thin, top: thin, bottom: thin, ..Borders::default() },
            ..Style::default()
        };
        assert_eq!(cfs[1].rule, CfRule::CellIs { op: CfOperator::Between, a: "2".into(), b: Some("4".into()), style: Box::new(style) });
        let style = Style {
            num_fmt: NumFmt::new("0.00%"),
            fill: Fill { pattern: PatternType::DarkGrid, fg: Color::Rgb(0xFF0000), bg: Color::Rgb(0xFFFF00) },
            ..Style::default()
        };
        assert_eq!(cfs[2].ranges, vec![range("B2:B5"), range("D2:D5")]);
        assert_eq!(cfs[2].rule, CfRule::Expression { formula: "B2>0".into(), style: Box::new(style) });
        // No CFEx: file order is the priority order, and the first match wins (Excel 97-2003).
        assert_eq!(cfs.iter().map(|c| (c.priority, c.stop_if_true)).collect::<Vec<_>>(), vec![(1, true), (2, true), (3, true)]);
    }

    #[test]
    fn cfex_records_give_priority_stop_and_excel_2007_rule_types() {
        let fill = Dxf { fill: Some(13), ..Dxf::default() };
        let f = |rgce: &[u8]| cf(2, 0, &fill, rgce, &[]);
        let (sheet, report) = run(&[
            (CONDFMT, cond_fmt(3, &[[0, 9, 2, 2]])),
            (CF, f(&ptg_int(1))),
            (CF, f(&ptg_int(1))),
            // LEFT(C1,2)="an": the text comes from the formula's constant.
            (CF, f(&[ptg_ref_n(0, 0), ptg_int(2), vec![0x42, 2, 0x73, 0x00], ptg_str("an"), vec![0x0B]].concat())),
            (CF, f(&ptg_int(1))),
            (CF, f(&ptg_int(1))),
            (CF, f(&ptg_int(1))),
            (CFEX, cf_ex(3, 0, 0x05, 7, false, &[0x01, 3, 0])),
            (CFEX, cf_ex(3, 1, 0x19, 2, true, &[2, 0])),
            (CFEX, cf_ex(3, 2, 0x08, 5, true, &[2, 0])),
            (CFEX, cf_ex(3, 3, 0x12, 1, true, &[])),
            (CFEX, cf_ex(3, 4, 0x1E, 3, true, &[])),
            (CFEX, cf_ex(3, 5, 0x1B, 4, false, &[])),
        ]);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let style = Box::new(solid(0xFFFF00));
        let got: Vec<_> = sheet.cond_formats.iter().map(|c| (c.priority, c.stop_if_true, c.rule.clone())).collect();
        assert_eq!(
            got,
            vec![
                (7, false, CfRule::Top10 { bottom: false, percent: false, rank: 3, style: style.clone() }),
                (2, true, CfRule::AboveAverage { below: false, equal: false, std_dev: 2, style: style.clone() }),
                (5, true, CfRule::BeginsWith { text: "an".into(), style: style.clone() }),
                (1, true, CfRule::TimePeriod { period: "last7Days".into(), style: style.clone() }),
                (3, true, CfRule::AboveAverage { below: true, equal: true, std_dev: 0, style: style.clone() }),
                (4, false, CfRule::Duplicate { style }),
            ]
        );
    }

    #[test]
    fn reads_colour_scales_data_bars_and_icon_sets() {
        let scale = [
            vec![0, 0, 0, 3, 3, 3],
            cfvo(2, None, &[]),
            0f64.to_le_bytes().to_vec(),
            cfvo(5, Some(50.0), &[]),
            0.5f64.to_le_bytes().to_vec(),
            // A threshold on the rule's own sheet, written as a 3-D reference.
            cfvo(7, None, &ptg_ref3d(0, 0, 12)),
            1f64.to_le_bytes().to_vec(),
            0f64.to_le_bytes().to_vec(),
            rgb_color(0xF8696B),
            0.5f64.to_le_bytes().to_vec(),
            rgb_color(0xFFEB84),
            1f64.to_le_bytes().to_vec(),
            rgb_color(0x63BE7B),
        ]
        .concat();
        let bar = [vec![0, 0, 0, 0x02, 0, 100], rgb_color(0x0000FF), cfvo(5, Some(10.0), &[]), cfvo(1, Some(50.0), &[])].concat();
        let mut icons = vec![0, 0, 0, 3, 7, 0x05];
        for v in [0.0, 33.0, 67.0] {
            icons.extend(cfvo(4, Some(v), &[]));
            icons.extend([1, 0, 0, 0, 0]);
        }
        let (sheet, report) = run(&[
            (CONDFMT12, [vec![0; 12], cond_fmt(1, &[[0, 9, 3, 3]])].concat()),
            (CF12, cf12(3, &[], 2, 2, &[], &scale)),
            (CONDFMT12, [vec![0; 12], cond_fmt(2, &[[0, 9, 4, 4]])].concat()),
            (CF12, cf12(4, &[], 1, 3, &[], &bar)),
            (CONDFMT12, [vec![0; 12], cond_fmt(3, &[[0, 9, 5, 5]])].concat()),
            (CF12, cf12(6, &[], 3, 4, &[], &icons)),
            // An Excel 2007 rule attached to a classic CondFmt: "contains errors" on H1:H5.
            (CONDFMT, cond_fmt(9, &[[0, 4, 7, 7]])),
            (CFEX, cf_ex12(9)),
            (CF12, cf12(2, &[ptg_ref_n(0, 0), vec![0x41, 0x03, 0x00]].concat(), 4, 0x0B, &[], &[])),
        ]);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let got: Vec<_> = sheet.cond_formats.iter().map(|c| (c.ranges.clone(), c.priority, c.rule.clone())).collect();
        assert_eq!(
            got,
            vec![
                (
                    vec![range("D1:D10")],
                    2,
                    CfRule::ColorScale {
                        stops: vec![
                            (CfValueKind::Min, Color::Rgb(0xF8696B)),
                            (CfValueKind::Percentile(50.0), Color::Rgb(0xFFEB84)),
                            (CfValueKind::Formula("$M$1".into()), Color::Rgb(0x63BE7B)),
                        ]
                    }
                ),
                (
                    vec![range("E1:E10")],
                    1,
                    CfRule::DataBar {
                        min: CfValueKind::Percentile(10.0),
                        max: CfValueKind::Number(50.0),
                        color: Color::Rgb(0x0000FF),
                        gradient: true,
                        show_value: false
                    }
                ),
                (
                    vec![range("F1:F10")],
                    3,
                    CfRule::IconSet {
                        set: "3Symbols2".into(),
                        thresholds: vec![CfValueKind::Percent(0.0), CfValueKind::Percent(33.0), CfValueKind::Percent(67.0)],
                        reverse: true,
                        show_value: false
                    }
                ),
                (vec![range("H1:H5")], 4, CfRule::Errors { style: Box::default() }),
            ]
        );
    }

    fn xlstr(s: &str) -> Vec<u8> {
        [u16s(&[s.len() as u16]), vec![0], s.as_bytes().to_vec()].concat()
    }

    fn dv(flags: u32, titles: [&str; 4], f1: &[u8], f2: &[u8], ranges: &[[u16; 4]]) -> Vec<u8> {
        let mut p = flags.to_le_bytes().to_vec();
        for t in titles {
            // Excel writes an empty title as one NUL.
            p.extend(if t.is_empty() { xlstr("\0") } else { xlstr(t) });
        }
        p.extend(u16s(&[f1.len() as u16, 0]));
        p.extend(f1);
        p.extend(u16s(&[f2.len() as u16, 0]));
        p.extend(f2);
        p.extend(u16s(&[ranges.len() as u16]));
        for r in ranges {
            p.extend(ref8(r[0], r[1], r[2], r[3]));
        }
        p
    }

    #[test]
    fn reads_data_validation() {
        let shown = 0x4_0000 | 0x8_0000;
        let area = [vec![0x25], u16s(&[0, 2, 5, 5])].concat();
        let (sheet, report) = run(&[
            (DV, dv(3 | 0x80 | 0x100 | shown, ["Pick", "Nope", "Choose", "Only a, b or c"], &ptg_str("a\0b\0c"), &[], &[[0, 4, 0, 0]])),
            (DV, dv(3 | 0x200 | shown, ["", "", "", ""], &area, &[], &[[0, 4, 1, 1]])),
            (DV, dv(1 | (1 << 4) | 0x100 | 0x8_0000 | (1 << 20), ["", "", "", "Whole"], &ptg_int(1), &ptg_int(10), &[[0, 4, 2, 2], [7, 8, 2, 3]])),
            (DV, dv(7 | (2 << 4) | 0x4_0000, ["", "", "", ""], &[ptg_ref_n(0, 1), vec![0x41, 0x80, 0x00]].concat(), &[], &[[1, 5, 6, 6]])),
            (DV, dv(6 | (7 << 20) | shown, ["", "", "", ""], &ptg_int(5), &[], &[])),
        ]);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let v = &sheet.validations;
        assert_eq!(v.len(), 4, "a validation with no cells is dropped");
        assert_eq!(
            v[0],
            Validation {
                ranges: vec![range("A1:A5")],
                kind: ValidationKind::List,
                f1: "\"a,b,c\"".into(),
                input_title: "Pick".into(),
                input_message: "Choose".into(),
                error_title: "Nope".into(),
                error_message: "Only a, b or c".into(),
                ..Validation::default()
            }
        );
        assert_eq!((v[1].f1.as_str(), v[1].in_cell_dropdown, v[1].allow_blank, v[1].input_title.as_str()), ("=$F$1:$F$3", false, false, ""));
        assert_eq!(
            (v[2].kind, v[2].op, v[2].f1.as_str(), v[2].f2.as_deref(), v[2].error_style, v[2].show_input, v[2].ranges.len()),
            (ValidationKind::Whole, CfOperator::NotBetween, "1", Some("10"), ErrorStyle::Warning, false, 2)
        );
        assert_eq!(
            (v[3].kind, v[3].f1.as_str(), v[3].error_style, v[3].show_error),
            (ValidationKind::Custom, "ISNUMBER(H2)", ErrorStyle::Information, false)
        );
    }

    #[test]
    fn hostile_rule_records_never_panic() {
        let records: Vec<(u16, Vec<u8>)> = vec![
            (CONDFMT, cond_fmt(1, &[[0, 9, 0, 0]])),
            (
                CF,
                cf(
                    1,
                    1,
                    &Dxf { font: Some((true, true, 10)), border: Some(12), fill: Some(13), ifmt: Some(14), ..Dxf::default() },
                    &ptg_int(2),
                    &ptg_int(4),
                ),
            ),
            (CFEX, cf_ex(1, 0, 0x08, 1, true, &[1, 0])),
            (CONDFMT12, [vec![0; 12], cond_fmt(2, &[[0, 9, 4, 4]])].concat()),
            (CF12, cf12(4, &[], 1, 3, &[], &[vec![0, 0, 0, 0, 0, 100], rgb_color(0xFF), cfvo(2, None, &[]), cfvo(3, None, &[])].concat())),
            (DV, dv(3 | 0x80, ["a", "b", "c", "d"], &ptg_str("x\0y"), &[], &[[0, 0, 0, 0]])),
        ];
        for (i, (_, payload)) in records.iter().enumerate() {
            for cut in 0..payload.len() {
                let mut r = records.clone();
                r[i].1.truncate(cut);
                let _ = run(&r);
            }
            for at in 0..payload.len() {
                for x in [0x00u8, 0xFF, 0x80, 0x7F] {
                    let mut r = records.clone();
                    r[i].1[at] = x;
                    let _ = run(&r);
                }
            }
        }
        // A truncated rule is reported, not fatal.
        let mut r = records.clone();
        r[1].1.truncate(20);
        let (sheet, report) = run(&r);
        assert!(report.warnings.iter().any(|w| w.contains("malformed")), "{:?}", report.warnings);
        assert_eq!(sheet.validations.len(), 1);
    }
}
