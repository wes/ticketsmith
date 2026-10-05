//! Barcodes: validation, FGL emission, and modules for the on-screen preview.
//!
//! Two different jobs live here and it is worth keeping them straight.
//!
//! [`write`] emits the FGL command. The *printer* does the real encoding —
//! check digits, Code 128 shift characters and code-set switching are all
//! calculated in firmware. What we send is a payload plus delimiters.
//!
//! [`encode`] produces bar/space modules so the preview can draw something. For
//! Code 39, Interleaved 2 of 5, UPC-A and EAN-13 those modules are the genuine
//! symbology and would scan. For Code 128 and Codabar we render a footprint of
//! exactly the right size with representative bars — see [`Pattern::exact`].
//! Sizing is what a print preview is for, and the printer still produces a
//! correct symbol either way.
//!
//! Reference: Boca Systems FGL46/FGL26 Programming Guide, rev 16.1, barcode
//! supplements (UPC, Interleaved 2of5, EAN13, Code 39, Codabar, Code 128, 2D).

use std::fmt::Write;

use crate::fgl;

pub const MAX_DATA: usize = 64;

// ---------------------------------------------------------------------------
// Symbologies
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Symbology {
    Code128,
    Code39,
    I2of5,
    Codabar,
    UpcA,
    Ean13,
    Qr,
}

impl Symbology {
    pub const ALL: [Symbology; 7] = [
        Symbology::Code128,
        Symbology::Code39,
        Symbology::I2of5,
        Symbology::Codabar,
        Symbology::UpcA,
        Symbology::Ean13,
        Symbology::Qr,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Symbology::Code128 => "Code 128",
            Symbology::Code39 => "Code 39",
            Symbology::I2of5 => "Interleaved 2 of 5",
            Symbology::Codabar => "Codabar",
            Symbology::UpcA => "UPC-A",
            Symbology::Ean13 => "EAN-13",
            Symbology::Qr => "QR",
        }
    }

    /// The letter FGL uses to select this symbology. Lower case is the "new
    /// style" that honours rotation commands; upper case ignores them.
    fn select_letter(self) -> char {
        match self {
            Symbology::Code39 => 'n',
            Symbology::Code128 => 'o',
            Symbology::I2of5 => 'f',
            Symbology::Codabar => 'c',
            Symbology::UpcA => 'u',
            Symbology::Ean13 => 'e',
            // 2D barcodes use <QR...> instead of a select letter.
            Symbology::Qr => '\0',
        }
    }

    pub fn is_2d(self) -> bool {
        self == Symbology::Qr
    }

    /// Only Code 39 and Interleaved 2 of 5 can print 3:1 wide-to-narrow.
    pub fn supports_wide_ratio(self) -> bool {
        matches!(self, Symbology::Code39 | Symbology::I2of5)
    }

    /// UPC/EAN bar heights are fixed by the symbology; the others take a unit
    /// count where each unit is an 8 dot tall bar.
    pub fn supports_height_units(self) -> bool {
        !self.is_2d()
    }

    pub fn supports_interpretation(self) -> bool {
        !self.is_2d()
    }

    /// True when [`encode`] returns the genuine symbology rather than a
    /// width-accurate stand-in.
    pub fn preview_is_exact(self) -> bool {
        matches!(
            self,
            Symbology::Code39 | Symbology::I2of5 | Symbology::UpcA | Symbology::Ean13
        )
    }

    pub fn hint(self) -> &'static str {
        match self {
            Symbology::Code39 => "0-9  A-Z  space  - . $ / + %",
            Symbology::Code128 => "any printable ASCII",
            Symbology::I2of5 => "digits, even count",
            Symbology::Codabar => "digits and  - $ : / . +",
            Symbology::UpcA => "11 digits (check digit added) or 12",
            Symbology::Ean13 => "12 digits (check digit added) or 13",
            Symbology::Qr => "any printable ASCII",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Bars stand upright; the code reads along the length of the ticket.
    Picket,
    /// Bars lie across; the code reads across the width of the ticket.
    Ladder,
}

impl Orientation {
    pub const ALL: [Orientation; 2] = [Orientation::Picket, Orientation::Ladder];

    pub fn label(self) -> &'static str {
        match self {
            Orientation::Picket => "picket fence",
            Orientation::Ladder => "ladder",
        }
    }
}

/// FGL numbers these in a surprising order — 0 is M, not L.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QrEcc {
    L,
    M,
    Q,
    H,
}

impl QrEcc {
    pub const ALL: [QrEcc; 4] = [QrEcc::L, QrEcc::M, QrEcc::Q, QrEcc::H];

    fn code(self) -> u8 {
        match self {
            QrEcc::M => 0,
            QrEcc::L => 1,
            QrEcc::H => 2,
            QrEcc::Q => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            QrEcc::L => "L  7% recovery",
            QrEcc::M => "M  15% recovery",
            QrEcc::Q => "Q  25% recovery",
            QrEcc::H => "H  30% recovery",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QrVersion {
    V2,
    V7,
    V11,
    V15,
}

impl QrVersion {
    pub const ALL: [QrVersion; 4] = [QrVersion::V2, QrVersion::V7, QrVersion::V11, QrVersion::V15];

    fn number(self) -> u8 {
        match self {
            QrVersion::V2 => 2,
            QrVersion::V7 => 7,
            QrVersion::V11 => 11,
            QrVersion::V15 => 15,
        }
    }

    /// Symbol side in modules.
    pub fn modules(self) -> u32 {
        match self {
            QrVersion::V2 => 25,
            QrVersion::V7 => 45,
            QrVersion::V11 => 61,
            QrVersion::V15 => 77,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            QrVersion::V2 => "2  25x25",
            QrVersion::V7 => "7  45x45",
            QrVersion::V11 => "11  61x61",
            QrVersion::V15 => "15  77x77",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub symbology: Symbology,
    pub orientation: Orientation,
    /// Prints the code in the opposite reading direction (RU instead of NR for
    /// picket fence, RL instead of RR for ladder).
    pub flipped: bool,
    /// `<xB#>` — bar length in 8 dot units. FGL defaults to 4, i.e. 32 dots.
    pub height_units: u8,
    /// `<X#>` — dots per narrow bar, 1..=9. Two is the usual choice for a
    /// scannable code at 200 dpi.
    pub expansion: u8,
    /// `<BI>` — human readable interpretation printed underneath, in font1.
    pub interpretation: bool,
    /// 3:1 rather than 2:1 wide-to-narrow. Code 39 and I2of5 only.
    pub wide_ratio: bool,
    pub codabar_start: char,
    pub codabar_stop: char,
    pub qr_point: u8,
    pub qr_ecc: QrEcc,
    pub qr_version: QrVersion,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            symbology: Symbology::Code128,
            orientation: Orientation::Picket,
            flipped: false,
            height_units: 4,
            expansion: 2,
            interpretation: true,
            wide_ratio: false,
            codabar_start: 'A',
            codabar_stop: 'B',
            qr_point: 6,
            qr_ecc: QrEcc::M,
            qr_version: QrVersion::V7,
        }
    }
}

impl Settings {
    fn narrow_modules(&self) -> usize {
        1
    }

    fn wide_modules(&self) -> usize {
        if self.wide_ratio && self.symbology.supports_wide_ratio() {
            3
        } else {
            2
        }
    }

    /// Rotation that matches the requested orientation. FGL only accepts NR/RU
    /// for picket fence codes and RR/RL for ladder codes; anything else the
    /// printer rejects.
    pub fn rotation(&self) -> fgl::Rotation {
        match (self.orientation, self.flipped) {
            (Orientation::Picket, false) => fgl::Rotation::Nr,
            (Orientation::Picket, true) => fgl::Rotation::Ru,
            (Orientation::Ladder, false) => fgl::Rotation::Rr,
            (Orientation::Ladder, true) => fgl::Rotation::Rl,
        }
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// `Ok(())` or a message suitable for showing next to the field.
pub type Validation = Result<(), &'static str>;

pub fn validate(sym: Symbology, data: &str) -> Validation {
    if data.is_empty() {
        return Err("no data");
    }
    if data.len() > MAX_DATA {
        return Err("too long");
    }

    match sym {
        // Lower case is accepted and upper-cased on emission; the Code 39
        // character set has no lower case at all.
        Symbology::Code39 => {
            for c in data.chars() {
                if !is_code39_char(c.to_ascii_uppercase()) {
                    return Err("Code 39 allows 0-9 A-Z space - . $ / + % only");
                }
            }
            Ok(())
        }
        Symbology::Code128 => {
            for c in data.chars() {
                if !fgl::is_printable(c) {
                    return Err("Code 128 needs printable ASCII");
                }
                if c == '^' {
                    return Err("'^' delimits the payload and cannot be encoded");
                }
            }
            Ok(())
        }
        Symbology::I2of5 => {
            if !data.chars().all(|c| c.is_ascii_digit()) {
                return Err("Interleaved 2 of 5 is numeric only");
            }
            // Digits are encoded in pairs, so an odd count cannot be represented.
            if data.len() % 2 != 0 {
                return Err("needs an even number of digits");
            }
            Ok(())
        }
        Symbology::Codabar => {
            for c in data.chars() {
                if !c.is_ascii_digit() && !"-$:/.+".contains(c) {
                    return Err("Codabar allows digits and - $ : / . + only");
                }
            }
            Ok(())
        }
        Symbology::UpcA => {
            if !data.chars().all(|c| c.is_ascii_digit()) {
                return Err("UPC-A is numeric only");
            }
            if data.len() != 11 && data.len() != 12 {
                return Err("UPC-A needs 11 or 12 digits");
            }
            Ok(())
        }
        Symbology::Ean13 => {
            if !data.chars().all(|c| c.is_ascii_digit()) {
                return Err("EAN-13 is numeric only");
            }
            if data.len() != 12 && data.len() != 13 {
                return Err("EAN-13 needs 12 or 13 digits");
            }
            Ok(())
        }
        Symbology::Qr => {
            for c in data.chars() {
                if !fgl::is_printable(c) {
                    return Err("QR text needs printable ASCII");
                }
                if c == '{' || c == '}' {
                    return Err("braces delimit the payload and cannot be encoded");
                }
            }
            Ok(())
        }
    }
}

fn is_code39_char(c: char) -> bool {
    c.is_ascii_digit() || c.is_ascii_uppercase() || " -.$/+%".contains(c)
}

/// The GTIN check digit rule, weighting 3-1-3-1... from the right of the body.
/// Covers UPC-A (11 digit body) and EAN-13 (12 digit body) alike — a UPC-A is
/// just an EAN-13 with a leading zero.
pub fn gtin_check_digit(body: &str) -> u8 {
    let mut sum = 0u32;
    let mut weight = 3u32;
    for c in body.chars().rev() {
        sum += weight * c.to_digit(10).unwrap_or(0);
        weight = if weight == 3 { 1 } else { 3 };
    }
    ((10 - (sum % 10)) % 10) as u8
}

/// The digits actually encoded, always exactly `full_len` of them.
///
/// A full-length payload is taken as given — the caller supplied the check
/// digit. Anything shorter is left-padded with zeros to the body length and
/// gets a computed check digit, because leading zeros are part of a GTIN and
/// because the emitter slices the result into fixed halves. Validation keeps
/// short data out of the UI, but `Doc::emit` does not re-validate, and a print
/// job is not the place to discover an out of bounds slice.
pub fn complete_gtin(data: &str, full_len: usize) -> String {
    let digits: String = data.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= full_len {
        return digits[..full_len].to_string();
    }

    let body_len = full_len - 1;
    let mut body = digits;
    while body.len() < body_len {
        body.insert(0, '0');
    }
    body.push(char::from(b'0' + gtin_check_digit(&body)));
    body
}

// ---------------------------------------------------------------------------
// FGL emission
// ---------------------------------------------------------------------------

pub fn write(out: &mut String, row: i32, col: i32, data: &str, s: &Settings) {
    if s.symbology == Symbology::Qr {
        return write_qr(out, row, col, data, s);
    }

    out.push_str(s.rotation().command());
    let _ = write!(out, "<RC{},{}>", row, col);
    if s.expansion > 1 {
        let _ = write!(out, "<X{}>", s.expansion);
    }
    if s.interpretation {
        out.push_str("<BI>");
    }

    // <aB#> — type letter, then P or L for orientation, then the height in 8-dot
    // units. An X between the letter and the orientation asks for 3:1.
    out.push('<');
    out.push(s.symbology.select_letter());
    if s.wide_ratio && s.symbology.supports_wide_ratio() {
        out.push('X');
    }
    out.push(match s.orientation {
        Orientation::Picket => 'P',
        Orientation::Ladder => 'L',
    });
    let _ = write!(out, "{}>", s.height_units.max(1));

    write_payload(out, data, s);
}

/// Wraps the payload in whatever start/stop characters the symbology needs.
fn write_payload(out: &mut String, data: &str, s: &Settings) {
    match s.symbology {
        Symbology::Code39 => {
            out.push('*');
            for c in data.chars() {
                out.push(c.to_ascii_uppercase());
            }
            out.push('*');
        }
        Symbology::Code128 => {
            let _ = write!(out, "^{}^", data);
        }
        Symbology::I2of5 => {
            let _ = write!(out, ":{}:", data);
        }
        Symbology::Codabar => {
            let _ = write!(out, "{}{}{}", s.codabar_start, data, s.codabar_stop);
        }
        Symbology::UpcA => {
            // J <6 digits> K <6 digits> L
            let d = complete_gtin(data, 12);
            let _ = write!(out, "J{}K{}L", &d[0..6], &d[6..12]);
        }
        Symbology::Ean13 => {
            // <parity digit> J <6 digits> K <6 digits> L
            let d = complete_gtin(data, 13);
            let _ = write!(out, "{}J{}K{}L", &d[0..1], &d[1..7], &d[7..13]);
        }
        Symbology::Qr => unreachable!("handled by write_qr"),
    }
}

fn write_qr(out: &mut String, row: i32, col: i32, data: &str, s: &Settings) {
    let _ = write!(out, "<QRV{}>", s.qr_version.number());
    out.push_str(if s.flipped {
        fgl::Rotation::Ru.command()
    } else {
        fgl::Rotation::Nr.command()
    });
    let _ = write!(out, "<RC{},{}>", row, col);
    // The guide is explicit that QR symbols must be printed at HW1,1.
    out.push_str("<HW1,1>");
    // point size, apply-tilde, encode mode (0 = binary/byte), error correction
    let _ = write!(
        out,
        "<QR{},0,0,{}>{{{}}}",
        s.qr_point.max(3),
        s.qr_ecc.code(),
        data
    );
}

// ---------------------------------------------------------------------------
// Preview modules
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Pattern {
    /// One entry per module; true is a bar.
    pub bits: Vec<bool>,
    /// False when the bars are a width-accurate stand-in rather than the real
    /// symbology. The footprint is right either way.
    pub exact: bool,
}

impl Pattern {
    fn push(&mut self, bar: bool, count: usize) {
        self.bits.extend(std::iter::repeat_n(bar, count));
    }

    fn push_bits(&mut self, s: &str) {
        for c in s.chars() {
            self.bits.push(c == '1');
        }
    }

    pub fn len(&self) -> usize {
        self.bits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bits.is_empty()
    }

    /// Runs of consecutive bars as `(start_module, length)`, so the preview can
    /// draw one quad per bar instead of one per module.
    pub fn bar_runs(&self) -> Vec<(usize, usize)> {
        let mut runs = Vec::new();
        let mut i = 0;
        while i < self.bits.len() {
            if !self.bits[i] {
                i += 1;
                continue;
            }
            let start = i;
            while i < self.bits.len() && self.bits[i] {
                i += 1;
            }
            runs.push((start, i - start));
        }
        runs
    }
}

/// Bar/space modules for the preview. `data` is assumed to have passed
/// [`validate`]; anything unexpected simply produces a shorter pattern.
pub fn encode(sym: Symbology, data: &str, s: &Settings) -> Pattern {
    let mut p = Pattern {
        bits: Vec::new(),
        exact: true,
    };
    match sym {
        Symbology::Code39 => encode_code39(&mut p, data, s),
        Symbology::I2of5 => encode_i2of5(&mut p, data, s),
        Symbology::UpcA => encode_upc_a(&mut p, data),
        Symbology::Ean13 => encode_ean13(&mut p, data),
        Symbology::Code128 => {
            p.exact = false;
            representative(&mut p, data, code128_modules(data));
        }
        Symbology::Codabar => {
            p.exact = false;
            representative(&mut p, data, codabar_modules(data, s));
        }
        Symbology::Qr => p.exact = false,
    }
    p
}

// --- Code 39 ---------------------------------------------------------------

/// Nine elements per character, alternating bar/space starting with a bar.
/// 'w' marks the three wide elements.
fn code39_pattern(c: char) -> Option<&'static str> {
    Some(match c {
        '0' => "nnnwwnwnn",
        '1' => "wnnwnnnnw",
        '2' => "nnwwnnnnw",
        '3' => "wnwwnnnnn",
        '4' => "nnnwwnnnw",
        '5' => "wnnwwnnnn",
        '6' => "nnwwwnnnn",
        '7' => "nnnwnnwnw",
        '8' => "wnnwnnwnn",
        '9' => "nnwwnnwnn",
        'A' => "wnnnnwnnw",
        'B' => "nnwnnwnnw",
        'C' => "wnwnnwnnn",
        'D' => "nnnnwwnnw",
        'E' => "wnnnwwnnn",
        'F' => "nnwnwwnnn",
        'G' => "nnnnnwwnw",
        'H' => "wnnnnwwnn",
        'I' => "nnwnnwwnn",
        'J' => "nnnnwwwnn",
        'K' => "wnnnnnnww",
        'L' => "nnwnnnnww",
        'M' => "wnwnnnnwn",
        'N' => "nnnnwnnww",
        'O' => "wnnnwnnwn",
        'P' => "nnwnwnnwn",
        'Q' => "nnnnnnwww",
        'R' => "wnnnnnwwn",
        'S' => "nnwnnnwwn",
        'T' => "nnnnwnwwn",
        'U' => "wwnnnnnnw",
        'V' => "nwwnnnnnw",
        'W' => "wwwnnnnnn",
        'X' => "nwnnwnnnw",
        'Y' => "wwnnwnnnn",
        'Z' => "nwwnwnnnn",
        '-' => "nwnnnnwnw",
        '.' => "wwnnnnwnn",
        ' ' => "nwwnnnwnn",
        '$' => "nwnwnwnnn",
        '/' => "nwnwnnnwn",
        '+' => "nwnnnwnwn",
        '%' => "nnnwnwnwn",
        '*' => "nwnnwnwnn",
        _ => return None,
    })
}

fn encode_code39(p: &mut Pattern, data: &str, s: &Settings) {
    let narrow = s.narrow_modules();
    let wide = s.wide_modules();

    // Start and stop characters are both '*'.
    emit_code39_char(p, '*', narrow, wide);
    for c in data.chars() {
        p.push(false, narrow); // inter-character gap
        emit_code39_char(p, c.to_ascii_uppercase(), narrow, wide);
    }
    p.push(false, narrow);
    emit_code39_char(p, '*', narrow, wide);
}

fn emit_code39_char(p: &mut Pattern, c: char, narrow: usize, wide: usize) {
    let Some(pattern) = code39_pattern(c) else {
        return;
    };
    for (i, e) in pattern.chars().enumerate() {
        let is_bar = i % 2 == 0;
        p.push(is_bar, if e == 'w' { wide } else { narrow });
    }
}

// --- Interleaved 2 of 5 ----------------------------------------------------

/// Five elements per digit, two of them wide. Bars come from the first digit of
/// a pair and spaces from the second, interleaved.
fn i2of5_pattern(c: char) -> Option<&'static str> {
    Some(match c {
        '0' => "nnwwn",
        '1' => "wnnnw",
        '2' => "nwnnw",
        '3' => "wwnnn",
        '4' => "nnwnw",
        '5' => "wnwnn",
        '6' => "nwwnn",
        '7' => "nnnww",
        '8' => "wnnwn",
        '9' => "nwnwn",
        _ => return None,
    })
}

fn encode_i2of5(p: &mut Pattern, data: &str, s: &Settings) {
    let narrow = s.narrow_modules();
    let wide = s.wide_modules();

    // Start: narrow bar, narrow space, narrow bar, narrow space.
    p.push(true, narrow);
    p.push(false, narrow);
    p.push(true, narrow);
    p.push(false, narrow);

    let digits: Vec<char> = data.chars().collect();
    for pair in digits.chunks_exact(2) {
        let (Some(bars), Some(spaces)) = (i2of5_pattern(pair[0]), i2of5_pattern(pair[1])) else {
            continue;
        };
        for (b, sp) in bars.chars().zip(spaces.chars()) {
            p.push(true, if b == 'w' { wide } else { narrow });
            p.push(false, if sp == 'w' { wide } else { narrow });
        }
    }

    // Stop: wide bar, narrow space, narrow bar.
    p.push(true, wide);
    p.push(false, narrow);
    p.push(true, narrow);
}

// --- UPC-A and EAN-13 ------------------------------------------------------

/// Left-hand odd-parity ("L") encodings. R is the bitwise complement and G is R
/// reversed, which is how the other two sets are derived below.
const UPC_L: [&str; 10] = [
    "0001101", "0011001", "0010011", "0111101", "0100011", "0110001", "0101111", "0111011",
    "0110111", "0001011",
];

/// Which of the left six digits use G rather than L, selected by the leading
/// digit of an EAN-13.
const EAN_PARITY: [&str; 10] = [
    "LLLLLL", "LLGLGG", "LLGGLG", "LLGGGL", "LGLLGG", "LGGLLG", "LGGGLL", "LGLGLG", "LGLGGL",
    "LGGLGL",
];

fn push_upc_digit(p: &mut Pattern, digit: char, set: char) {
    let Some(idx) = digit.to_digit(10) else { return };
    let l = UPC_L[idx as usize];
    match set {
        // L: as tabulated.
        'L' => p.push_bits(l),
        // R: complement of L.
        'R' => {
            for c in l.chars() {
                p.bits.push(c == '0');
            }
        }
        // G: R reversed.
        'G' => {
            for c in l.chars().rev() {
                p.bits.push(c == '0');
            }
        }
        _ => {}
    }
}

fn encode_upc_a(p: &mut Pattern, data: &str) {
    let d = complete_gtin(data, 12);
    if d.len() < 12 {
        return;
    }
    let chars: Vec<char> = d.chars().collect();

    p.push_bits("101"); // left guard
    for c in &chars[0..6] {
        push_upc_digit(p, *c, 'L');
    }
    p.push_bits("01010"); // centre guard
    for c in &chars[6..12] {
        push_upc_digit(p, *c, 'R');
    }
    p.push_bits("101"); // right guard
}

fn encode_ean13(p: &mut Pattern, data: &str) {
    let d = complete_gtin(data, 13);
    if d.len() < 13 {
        return;
    }
    let chars: Vec<char> = d.chars().collect();

    // The leading digit is not drawn as bars; it picks the parity pattern for
    // the left half instead.
    let Some(lead) = chars[0].to_digit(10) else {
        return;
    };
    let parity: Vec<char> = EAN_PARITY[lead as usize].chars().collect();

    p.push_bits("101");
    for (c, set) in chars[1..7].iter().zip(parity.iter()) {
        push_upc_digit(p, *c, *set);
    }
    p.push_bits("01010");
    for c in &chars[7..13] {
        push_upc_digit(p, *c, 'R');
    }
    p.push_bits("101");
}

// --- Width-accurate stand-ins ---------------------------------------------

/// Code 128 symbols are 11 modules each: a start character, one per data symbol,
/// a checksum, then a 13 module stop. The printer packs pairs of digits into code
/// set C where it can, which is what the guide means by "switches between start
/// codes B and C where appropriate".
fn code128_modules(data: &str) -> usize {
    let all_digits = !data.is_empty() && data.chars().all(|c| c.is_ascii_digit());
    let symbols = if all_digits && data.len() % 2 == 0 {
        data.len() / 2
    } else {
        data.len()
    };
    11 * (symbols + 2) + 13
}

/// Seven elements per character plus a narrow gap. Digits and `-$` carry two
/// wide elements; `:/.+` and the start/stop letters carry three.
fn codabar_modules(data: &str, s: &Settings) -> usize {
    let narrow = s.narrow_modules();
    let wide = s.wide_modules();
    // start character + payload + stop character
    let chars = data.len() + 2;
    let mut wide_heavy = 2 + data.chars().filter(|c| ":/.+".contains(*c)).count();

    let mut total = 0;
    for _ in 0..chars {
        let wides = if wide_heavy > 0 {
            wide_heavy -= 1;
            3
        } else {
            2
        };
        total += (7 - wides) * narrow + wides * wide + narrow;
    }
    total
}

/// Bars of the right total width, laid out deterministically from the payload so
/// the preview does not shimmer between frames. Used only where we do not carry
/// the real encoding table.
fn representative(p: &mut Pattern, data: &str, total_modules: usize) {
    let total = total_modules.min(1400);
    if total < 8 {
        p.push(true, total);
        return;
    }

    let mut hash: u64 = 0x9e37_79b9_7f4a_7c15;
    for b in data.bytes() {
        hash = (hash ^ b as u64).wrapping_mul(0x1000_0000_01b3);
    }

    // Guard bars at both ends, as every linear symbology has.
    p.push_bits("1011");
    while p.len() + 4 < total {
        let run = 1 + (hash & 0x3) as usize;
        hash = hash.rotate_right(2);
        let bar = (hash & 0x10) != 0;
        p.push(bar, run.min(total - 4 - p.len()));
    }
    p.push_bits("1101");
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Footprint {
    /// Dots along the ticket length (the FGL column axis).
    pub width_dots: u32,
    /// Dots across the ticket width (the FGL row axis).
    pub height_dots: u32,
}

/// Height of the human readable line, in dots: font1 plus a little air above it.
pub const INTERPRETATION_DOTS: u32 = 10;

/// Space the code occupies, including the human readable interpretation.
pub fn footprint(data: &str, s: &Settings) -> Footprint {
    if s.symbology == Symbology::Qr {
        let side = s.qr_version.modules() * s.qr_point.max(1) as u32;
        return Footprint {
            width_dots: side,
            height_dots: side,
        };
    }

    let pattern = encode(s.symbology, data, s);
    let along = pattern.len() as u32 * s.expansion.max(1) as u32;
    // Every height unit is an 8 dot bar. UPC/EAN ignore the setting and use a
    // fixed bar length, but the guide still accepts the parameter.
    let mut across = s.height_units.max(1) as u32 * 8;
    if s.interpretation && s.symbology.supports_interpretation() {
        across += INTERPRETATION_DOTS;
    }

    match s.orientation {
        Orientation::Picket => Footprint {
            width_dots: along,
            height_dots: across,
        },
        Orientation::Ladder => Footprint {
            width_dots: across,
            height_dots: along,
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn emit(data: &str, s: &Settings) -> String {
        let mut out = String::new();
        write(&mut out, 0, 10, data, s);
        out
    }

    #[test]
    fn code128_picket_fence_matches_the_guides_example_shape() {
        let s = Settings {
            symbology: Symbology::Code128,
            height_units: 5,
            ..Default::default()
        };
        assert_eq!(emit("CODE128", &s), "<NR><RC0,10><X2><BI><oP5>^CODE128^");
    }

    #[test]
    fn code39_ladder_with_a_3_to_1_ratio_inserts_the_x() {
        let s = Settings {
            symbology: Symbology::Code39,
            orientation: Orientation::Ladder,
            expansion: 1,
            height_units: 3,
            interpretation: false,
            wide_ratio: true,
            ..Default::default()
        };
        assert_eq!(emit("CODE39", &s), "<RR><RC0,10><nXL3>*CODE39*");
    }

    #[test]
    fn code39_upper_cases_the_payload() {
        let s = Settings {
            symbology: Symbology::Code39,
            expansion: 1,
            height_units: 3,
            interpretation: false,
            ..Default::default()
        };
        assert!(emit("gate-b", &s).ends_with("*GATE-B*"));
    }

    #[test]
    fn interleaved_2_of_5_uses_colon_delimiters() {
        let s = Settings {
            symbology: Symbology::I2of5,
            expansion: 1,
            height_units: 3,
            interpretation: false,
            ..Default::default()
        };
        assert_eq!(emit("123456", &s), "<NR><RC0,10><fP3>:123456:");
    }

    #[test]
    fn upc_a_splits_into_guard_delimited_halves_and_adds_the_check_digit() {
        // The guide works through this exact number: 40123456789 -> check digit 3.
        let s = Settings {
            symbology: Symbology::UpcA,
            height_units: 5,
            interpretation: false,
            ..Default::default()
        };
        assert_eq!(
            emit("40123456789", &s),
            "<NR><RC0,10><X2><uP5>J401234K567893L"
        );
    }

    #[test]
    fn ean13_keeps_the_leading_parity_digit_outside_the_guards() {
        let s = Settings {
            symbology: Symbology::Ean13,
            orientation: Orientation::Ladder,
            expansion: 1,
            height_units: 5,
            ..Default::default()
        };
        assert_eq!(
            emit("901456178012", &s),
            "<RR><RC0,10><BI><eL5>9J014561K780128L"
        );
    }

    #[test]
    fn flipped_orientations_pick_the_opposite_rotation() {
        let picket = Settings {
            symbology: Symbology::Code39,
            flipped: true,
            ..Default::default()
        };
        assert!(emit("A", &picket).starts_with("<RU>"));

        let ladder = Settings {
            symbology: Symbology::Code39,
            orientation: Orientation::Ladder,
            flipped: true,
            ..Default::default()
        };
        assert!(emit("A", &ladder).starts_with("<RL>"));
    }

    #[test]
    fn qr_pins_hw_1_1_and_wraps_the_payload_in_braces() {
        let s = Settings {
            symbology: Symbology::Qr,
            qr_point: 8,
            qr_ecc: QrEcc::Q,
            qr_version: QrVersion::V11,
            ..Default::default()
        };
        assert_eq!(
            emit("https://example.com", &s),
            "<QRV11><NR><RC0,10><HW1,1><QR8,0,0,3>{https://example.com}"
        );
    }

    #[test]
    fn short_or_dirty_gtin_data_is_padded_rather_than_panicking() {
        // Eight digits is not a UPC-A body; zero-pad to eleven and add a check
        // digit rather than slicing past the end when the halves are split.
        let d = complete_gtin("12345678", 12);
        assert_eq!(d.len(), 12);
        assert!(d.starts_with("00012345678"));
        assert_eq!(&d[11..12], &gtin_check_digit("00012345678").to_string());

        // Non-digits are dropped before padding.
        assert_eq!(complete_gtin("not for upc", 12).len(), 12);
        assert_eq!(complete_gtin("", 13).len(), 13);
    }

    #[test]
    fn emitting_a_short_upc_produces_well_formed_guards() {
        let s = Settings {
            symbology: Symbology::UpcA,
            interpretation: false,
            ..Default::default()
        };
        // "1234" becomes the body 00000001234 plus check digit 8, split into
        // guard-delimited halves.
        let out = emit("1234", &s);
        assert!(out.ends_with("J000000K012348L"), "{out}");
    }

    #[test]
    fn gtin_check_digits() {
        assert_eq!(gtin_check_digit("40123456789"), 3);
        assert_eq!(gtin_check_digit("901456178012"), 8);
    }

    #[test]
    fn validation_rejects_what_the_symbology_cannot_carry() {
        assert!(validate(Symbology::I2of5, "12345").is_err()); // odd digit count
        assert!(validate(Symbology::I2of5, "123456").is_ok());
        assert!(validate(Symbology::Code39, "lower").is_ok()); // upper-cased on emit
        assert!(validate(Symbology::Code39, "hello!").is_err());
        assert!(validate(Symbology::UpcA, "123").is_err());
        assert!(validate(Symbology::UpcA, "40123456789").is_ok());
        assert!(validate(Symbology::Code128, "a^b").is_err());
        assert!(validate(Symbology::Qr, "a{b").is_err());
        assert!(validate(Symbology::Code128, "").is_err());
    }

    #[test]
    fn upc_a_encodes_to_the_canonical_95_modules() {
        let s = Settings {
            symbology: Symbology::UpcA,
            ..Default::default()
        };
        let p = encode(Symbology::UpcA, "40123456789", &s);
        assert_eq!(p.len(), 95);
        assert!(p.exact);
        // Left guard, then the first digit '4' in L parity: 0100011.
        assert_eq!(&p.bits[0..3], &[true, false, true]);
        assert_eq!(
            &p.bits[3..10],
            &[false, true, false, false, false, true, true]
        );
    }

    #[test]
    fn ean13_also_encodes_to_95_modules() {
        let s = Settings {
            symbology: Symbology::Ean13,
            ..Default::default()
        };
        assert_eq!(encode(Symbology::Ean13, "901456178012", &s).len(), 95);
    }

    #[test]
    fn code39_pattern_starts_and_ends_with_a_bar() {
        let s = Settings {
            symbology: Symbology::Code39,
            expansion: 1,
            ..Default::default()
        };
        let p = encode(Symbology::Code39, "A", &s);
        assert!(p.exact);
        assert!(!p.is_empty());
        assert!(p.bits[0]);
        assert!(p.bits[p.len() - 1]);
    }

    #[test]
    fn representative_patterns_are_the_right_width_and_flagged_inexact() {
        let s = Settings {
            symbology: Symbology::Code128,
            expansion: 1,
            ..Default::default()
        };
        let p = encode(Symbology::Code128, "ABC", &s);
        assert!(!p.exact);
        // 3 data symbols in code set B, plus start and checksum, plus the stop.
        assert_eq!(p.len(), 11 * 5 + 13);
    }

    #[test]
    fn code128_packs_digit_pairs_into_code_set_c() {
        let s = Settings {
            symbology: Symbology::Code128,
            expansion: 1,
            ..Default::default()
        };
        // 8 digits become 4 symbols, not 8.
        assert_eq!(encode(Symbology::Code128, "12345678", &s).len(), 11 * 6 + 13);
    }

    #[test]
    fn footprint_swaps_axes_for_ladder_orientation() {
        let picket = Settings {
            symbology: Symbology::UpcA,
            height_units: 5,
            interpretation: false,
            ..Default::default()
        };
        let f = footprint("40123456789", &picket);
        assert_eq!(f.width_dots, 190); // 95 modules x 2 dots
        assert_eq!(f.height_dots, 40); // 5 units x 8 dots

        let ladder = Settings {
            orientation: Orientation::Ladder,
            ..picket
        };
        let g = footprint("40123456789", &ladder);
        assert_eq!(g.width_dots, f.height_dots);
        assert_eq!(g.height_dots, f.width_dots);
    }

    #[test]
    fn interpretation_adds_height_to_the_footprint() {
        let mut s = Settings {
            symbology: Symbology::Code39,
            expansion: 1,
            interpretation: false,
            ..Default::default()
        };
        let without = footprint("AB", &s);
        s.interpretation = true;
        assert_eq!(footprint("AB", &s).height_dots, without.height_dots + 10);
    }

    #[test]
    fn qr_footprint_is_square_and_scales_with_point_size() {
        let s = Settings {
            symbology: Symbology::Qr,
            qr_point: 6,
            qr_version: QrVersion::V7,
            ..Default::default()
        };
        let f = footprint("hello", &s);
        assert_eq!(f.width_dots, 45 * 6);
        assert_eq!(f.height_dots, f.width_dots);
    }

    #[test]
    fn bar_runs_coalesce_adjacent_modules() {
        let p = Pattern {
            bits: vec![true, true, false, true, false, false, true, true, true],
            exact: true,
        };
        assert_eq!(p.bar_runs(), vec![(0, 2), (3, 1), (6, 3)]);
    }
}
