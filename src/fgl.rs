//! FGL — "Friendly Ghost Language", the command language spoken by Boca Systems
//! ticket printers. Everything here emits plain ASCII; commands are wrapped in
//! `<` `>` and the text between them is what lands on the stock.
//!
//! Reference: Boca Systems FGL46/FGL26 Programming Guide, rev 16.1 (2024-06-20).
//!
//! The coordinate system trips up everybody exactly once:
//!
//! ```text
//! <RCrow,column>
//!   row    = dots DOWN from the top edge   (across the print head)
//!   column = dots RIGHT from the left edge (along the paper feed)
//! ```
//!
//! So a 2" x 5.5" ticket at 200 dpi is 400 rows tall and 1100 columns wide in
//! FGL's own head-on view — the long dimension is `column`. The guide quotes the
//! bottom-right printable corner of that stock as (383, 1049) once margins are
//! taken off.

use std::fmt::Write;

/// Raw TCP port every Boca ethernet interface listens on.
pub const RAW_TCP_PORT: u16 = 9100;

// ---------------------------------------------------------------------------
// Resident fonts
// ---------------------------------------------------------------------------

/// Glyph and box dimensions in printer dots. These do not change with head
/// density — a font3 character is 17x31 dots on a 200 dpi head and on a 300 dpi
/// head, it just comes out physically smaller on the denser one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metrics {
    pub glyph_w: u16,
    pub glyph_h: u16,
    /// Advance width. The next character starts here unless a new `<RC>` is sent.
    pub box_w: u16,
    /// Line height. A carriage return moves down by this much.
    pub box_h: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F16,
}

impl Font {
    /// Fonts offered in the UI, smallest to largest. Fonts 14 and 15 are marked
    /// "for Miltope users only" in the guide and are left out.
    pub const ALL: [Font; 14] = [
        Font::F1,
        Font::F4,
        Font::F5,
        Font::F9,
        Font::F2,
        Font::F7,
        Font::F16,
        Font::F3,
        Font::F8,
        Font::F13,
        Font::F10,
        Font::F11,
        Font::F6,
        Font::F12,
    ];

    pub fn number(self) -> u8 {
        match self {
            Font::F1 => 1,
            Font::F2 => 2,
            Font::F3 => 3,
            Font::F4 => 4,
            Font::F5 => 5,
            Font::F6 => 6,
            Font::F7 => 7,
            Font::F8 => 8,
            Font::F9 => 9,
            Font::F10 => 10,
            Font::F11 => 11,
            Font::F12 => 12,
            Font::F13 => 13,
            Font::F16 => 16,
        }
    }

    pub fn metrics(self) -> Metrics {
        let (glyph_w, glyph_h, box_w, box_h) = match self {
            Font::F1 => (5, 7, 7, 8),
            Font::F2 => (8, 16, 10, 18),
            Font::F3 => (17, 31, 20, 33),
            Font::F4 => (5, 9, 7, 11),
            Font::F5 => (5, 11, 7, 12),
            Font::F6 => (30, 52, 34, 56),
            Font::F7 => (15, 29, 20, 31),
            Font::F8 => (20, 40, 20, 33),
            Font::F9 => (13, 20, 13, 22),
            Font::F10 => (25, 41, 28, 41),
            Font::F11 => (25, 49, 26, 49),
            Font::F12 => (46, 91, 47, 91),
            Font::F13 => (20, 40, 20, 42),
            Font::F16 => (18, 31, 20, 33),
        };
        Metrics {
            glyph_w,
            glyph_h,
            box_w,
            box_h,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Font::F1 => "1  tiny 5x7",
            Font::F2 => "2  small 8x16",
            Font::F3 => "3  OCR-B 17x31",
            Font::F4 => "4  OCR-A 5x9",
            Font::F5 => "5  5x11",
            Font::F6 => "6  OCR-B large 30x52",
            Font::F7 => "7  OCR-A 15x29",
            Font::F8 => "8  Courier 20x40",
            Font::F9 => "9  OCR-B 13x20",
            Font::F10 => "10  Prestige 25x41",
            Font::F11 => "11  Script 25x49",
            Font::F12 => "12  Orator 46x91",
            Font::F13 => "13  Courier intl 20x40",
            Font::F16 => "16  Cyrillic 18x31",
        }
    }
}

// ---------------------------------------------------------------------------
// Rotation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    /// Left to right across the ticket.
    Nr,
    /// +90, reads top to bottom.
    Rr,
    /// +180, reads right to left.
    Ru,
    /// +270, reads bottom to top.
    Rl,
}

impl Rotation {
    pub const ALL: [Rotation; 4] = [Rotation::Nr, Rotation::Rr, Rotation::Ru, Rotation::Rl];

    pub fn command(self) -> &'static str {
        match self {
            Rotation::Nr => "<NR>",
            Rotation::Rr => "<RR>",
            Rotation::Ru => "<RU>",
            Rotation::Rl => "<RL>",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Rotation::Nr => "normal",
            Rotation::Rr => "rotate right",
            Rotation::Ru => "upside down",
            Rotation::Rl => "rotate left",
        }
    }

    /// Unit vector the run advances along, in (column, row) dots.
    ///
    /// The guide puts it as "facing in the direction of rotation, all characters
    /// build down and to the right of their starting points". Rotating the
    /// advance direction (+col) by the rotation gives these.
    pub fn advance(self) -> (f32, f32) {
        match self {
            Rotation::Nr => (1.0, 0.0),
            Rotation::Rr => (0.0, 1.0),
            Rotation::Ru => (-1.0, 0.0),
            Rotation::Rl => (0.0, -1.0),
        }
    }
}

// ---------------------------------------------------------------------------
// Head density
// ---------------------------------------------------------------------------

/// Boca quotes these as "200 / 300 / 600 dpi" and does its own arithmetic with
/// the round numbers (5.5" of stock = 1100 columns at 200 dpi), so we do too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Dpi200,
    Dpi300,
    Dpi600,
}

impl Density {
    pub const ALL: [Density; 3] = [Density::Dpi200, Density::Dpi300, Density::Dpi600];

    pub fn dots(self) -> u32 {
        match self {
            Density::Dpi200 => 200,
            Density::Dpi300 => 300,
            Density::Dpi600 => 600,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Density::Dpi200 => "200 dpi",
            Density::Dpi300 => "300 dpi",
            Density::Dpi600 => "600 dpi",
        }
    }
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextStyle {
    pub font: Font,
    /// `<HWheight,width>` multipliers, 1..=16.
    pub height_mult: u8,
    pub width_mult: u8,
    /// `<SD#>` divides the font down after the height/width multiply, so
    /// `<HW2,2><SD3>` gives 2/3 scale. 1 means "don't send it".
    pub scale_down: u8,
    pub rotation: Rotation,
    /// `<EI>`/`<DI>` — white on black.
    pub invert: bool,
    /// `<CTR#>` centres the string inside a field this many dots wide, starting
    /// at the element position. 0 disables it. A value wider than the ticket is
    /// clipped by the printer, which is the documented trick for centring on the
    /// ticket itself.
    pub center_field: u16,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            font: Font::F3,
            height_mult: 1,
            width_mult: 1,
            scale_down: 1,
            rotation: Rotation::Nr,
            invert: false,
            center_field: 0,
        }
    }
}

fn scaled(base: u16, mult: u8, down: u8) -> u32 {
    let down = if down == 0 { 1 } else { down as u32 };
    (base as u32 * mult.max(1) as u32) / down
}

impl TextStyle {
    /// Advance from one character cell to the next, in dots.
    pub fn advance_dots(&self) -> u32 {
        scaled(self.font.metrics().box_w, self.width_mult, self.scale_down)
    }

    /// Width of an individual glyph, in dots. Narrower than the advance for most
    /// of the resident fonts.
    pub fn glyph_width_dots(&self) -> u32 {
        scaled(self.font.metrics().glyph_w, self.width_mult, self.scale_down)
    }

    /// Height of one line of text, in dots.
    pub fn height_dots(&self) -> u32 {
        scaled(self.font.metrics().glyph_h, self.height_mult, self.scale_down)
    }

    /// Width of `text` in dots, as the printer will lay it out.
    pub fn width_dots(&self, text: &str) -> u32 {
        self.advance_dots() * printable_len(text) as u32
    }
}

/// `<` would open a command sequence and `>` would close one, so neither can
/// appear in ticket text. Anything non-printable goes too.
pub fn is_printable(c: char) -> bool {
    let b = c as u32;
    (0x20..=0x7e).contains(&b) && c != '<' && c != '>'
}

/// Characters that will actually reach the stock.
pub fn printable_chars(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars().filter(|c| is_printable(*c))
}

pub fn printable_len(text: &str) -> usize {
    printable_chars(text).count()
}

fn write_sanitized(out: &mut String, text: &str, delimiter: Option<char>) {
    for c in printable_chars(text) {
        if Some(c) == delimiter {
            continue;
        }
        out.push(c);
    }
}

/// Emit one positioned string.
pub fn write_text(out: &mut String, row: i32, col: i32, text: &str, style: &TextStyle) {
    out.push_str(style.rotation.command());
    let _ = write!(out, "<F{}>", style.font.number());
    let _ = write!(
        out,
        "<HW{},{}>",
        style.height_mult.max(1),
        style.width_mult.max(1)
    );
    if style.scale_down > 1 {
        let _ = write!(out, "<SD{}>", style.scale_down);
    }
    let _ = write!(out, "<RC{},{}>", row, col);
    if style.invert {
        out.push_str("<EI>");
    }
    if style.center_field > 0 {
        // The centring field is delimited by tildes, so a literal tilde in the
        // payload would end the field early.
        let _ = write!(out, "<CTR{}>~", style.center_field);
        write_sanitized(out, text, Some('~'));
        out.push('~');
    } else {
        write_sanitized(out, text, None);
    }
    if style.invert {
        out.push_str("<DI>");
    }
}

// ---------------------------------------------------------------------------
// Lines and boxes
// ---------------------------------------------------------------------------

/// `<LT#><BXrows,cols>` — a box `rows` dots tall and `cols` dots wide. Thickness
/// grows inward and resets to one dot after every box, so it is re-sent each time.
pub fn write_box(out: &mut String, row: i32, col: i32, rows: u16, cols: u16, thickness: u8) {
    let _ = write!(out, "<RC{},{}>", row, col);
    if thickness > 1 {
        let _ = write!(out, "<LT{}>", thickness);
    }
    let _ = write!(out, "<BX{},{}>", rows, cols);
}

/// `<LT#><HXcols>` — a horizontal rule. Thickness grows toward the bottom.
pub fn write_hline(out: &mut String, row: i32, col: i32, cols: u16, thickness: u8) {
    let _ = write!(out, "<RC{},{}>", row, col);
    if thickness > 1 {
        let _ = write!(out, "<LT{}>", thickness);
    }
    let _ = write!(out, "<HX{}>", cols);
}

/// `<LT#><VXrows>` — a vertical rule. Thickness grows toward the right.
pub fn write_vline(out: &mut String, row: i32, col: i32, rows: u16, thickness: u8) {
    let _ = write!(out, "<RC{},{}>", row, col);
    if thickness > 1 {
        let _ = write!(out, "<LT{}>", thickness);
    }
    let _ = write!(out, "<VX{}>", rows);
}

// ---------------------------------------------------------------------------
// Ticket counter
// ---------------------------------------------------------------------------

/// `<TC#######>` preloads the printer's seven digit user ticket count. Combined
/// with `<RE#>` the printer increments it per ticket at full speed, which is how
/// a run comes out numbered without re-sending the layout.
pub fn write_load_count(out: &mut String, count: u32) {
    let _ = write!(out, "<TC{:07}>", count % 10_000_000);
}

/// `<PC>` stamps the current count at the cursor. Two per ticket maximum, and
/// `<HW>` has no effect on it.
pub fn write_print_count(out: &mut String, row: i32, col: i32, font: Font, rotation: Rotation) {
    out.push_str(rotation.command());
    let _ = write!(out, "<F{}>", font.number());
    let _ = write!(out, "<RC{},{}><PC>", row, col);
}

// ---------------------------------------------------------------------------
// Print commands
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintMode {
    /// `<p>` — print and cut.
    Cut,
    /// `<q>` — print, leave the stock uncut.
    NoCut,
    /// `<z>` — print, cut and eject. Escrow printers only.
    Eject,
}

impl PrintMode {
    pub const ALL: [PrintMode; 3] = [PrintMode::Cut, PrintMode::NoCut, PrintMode::Eject];

    pub fn command(self) -> &'static str {
        match self {
            PrintMode::Cut => "<p>",
            PrintMode::NoCut => "<q>",
            PrintMode::Eject => "<z>",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PrintMode::Cut => "print + cut",
            PrintMode::NoCut => "print, no cut",
            PrintMode::Eject => "print + eject",
        }
    }
}

/// `<RE#>` asks for `qty - 1` *additional* copies, so one ticket sends nothing.
pub fn write_repeat(out: &mut String, qty: u32) {
    if qty > 1 {
        let _ = write!(out, "<RE{}>", qty - 1);
    }
}

pub fn write_print(out: &mut String, mode: PrintMode) {
    out.push_str(mode.command());
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn emit(f: impl FnOnce(&mut String)) -> String {
        let mut s = String::new();
        f(&mut s);
        s
    }

    #[test]
    fn text_emits_rotation_font_scale_then_position() {
        let out = emit(|s| write_text(s, 10, 30, "ADMIT ONE", &TextStyle::default()));
        assert_eq!(out, "<NR><F3><HW1,1><RC10,30>ADMIT ONE");
    }

    #[test]
    fn text_honours_multipliers_scale_down_and_inversion() {
        let style = TextStyle {
            font: Font::F13,
            height_mult: 2,
            width_mult: 3,
            scale_down: 3,
            rotation: Rotation::Rr,
            invert: true,
            ..Default::default()
        };
        let out = emit(|s| write_text(s, 0, 0, "VIP", &style));
        assert_eq!(out, "<RR><F13><HW2,3><SD3><RC0,0><EI>VIP<DI>");
    }

    #[test]
    fn centred_text_is_wrapped_in_tilde_delimiters() {
        let style = TextStyle {
            center_field: 4000,
            ..Default::default()
        };
        let out = emit(|s| write_text(s, 100, 0, "Main Stage", &style));
        assert_eq!(out, "<NR><F3><HW1,1><RC100,0><CTR4000>~Main Stage~");
    }

    #[test]
    fn angle_brackets_and_tildes_are_stripped_from_payload() {
        // '<' would open a command sequence; '~' would close the centring field.
        let style = TextStyle {
            center_field: 100,
            ..Default::default()
        };
        let out = emit(|s| write_text(s, 0, 0, "A~<B>", &style));
        assert_eq!(out, "<NR><F3><HW1,1><RC0,0><CTR100>~AB~");
    }

    #[test]
    fn repeat_asks_for_qty_minus_one_extra_tickets() {
        assert_eq!(emit(|s| write_repeat(s, 1)), "");
        assert_eq!(emit(|s| write_repeat(s, 25)), "<RE24>");
    }

    #[test]
    fn ticket_count_is_zero_padded_to_seven_digits() {
        assert_eq!(emit(|s| write_load_count(s, 42)), "<TC0000042>");
    }

    #[test]
    fn line_thickness_is_only_sent_when_it_differs_from_the_default() {
        assert_eq!(emit(|s| write_hline(s, 5, 5, 200, 1)), "<RC5,5><HX200>");
        assert_eq!(
            emit(|s| write_box(s, 5, 5, 10, 10, 4)),
            "<RC5,5><LT4><BX10,10>"
        );
    }

    #[test]
    fn font_metrics_match_the_font_supplement() {
        assert_eq!(Font::F3.metrics().box_w, 20);
        assert_eq!(Font::F3.metrics().box_h, 33);
        assert_eq!(Font::F12.metrics().glyph_h, 91);
    }

    #[test]
    fn scaled_text_measurement() {
        // font3 advances 20 dots; doubled that is 40 per character.
        let wide = TextStyle {
            width_mult: 2,
            ..Default::default()
        };
        assert_eq!(wide.width_dots("ABC"), 120);

        // <HW2,2><SD3> is the documented way to get 2/3 scale.
        let two_thirds = TextStyle {
            height_mult: 2,
            scale_down: 3,
            ..Default::default()
        };
        assert_eq!(two_thirds.height_dots(), 20);
    }

    #[test]
    fn unprintable_characters_do_not_count_toward_width() {
        let style = TextStyle::default();
        // The angle brackets are dropped, leaving three characters.
        assert_eq!(style.width_dots("A<B>C"), 60);
    }

    #[test]
    fn advance_directions_follow_the_rotation() {
        assert_eq!(Rotation::Nr.advance(), (1.0, 0.0));
        assert_eq!(Rotation::Rr.advance(), (0.0, 1.0));
        assert_eq!(Rotation::Ru.advance(), (-1.0, 0.0));
        assert_eq!(Rotation::Rl.advance(), (0.0, -1.0));
    }
}
