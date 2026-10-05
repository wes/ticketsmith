//! Drawing the ticket.
//!
//! The document lives in printer dots in FGL's frame — column along the feed,
//! row across the head. [`Layout`] is the one place that turns those into
//! window coordinates, so nothing else has to think about scaling.
//!
//! The ticket is drawn in FGL's own orientation, the long feed axis running
//! left to right. That is both how a 2" x 5.5" Boca ticket is held and how the
//! `<RC row,column>` numbers read, so the preview and the inspector agree.
//!
//! ## A note on rotated text
//!
//! GPUI's paint API can rotate SVG paths but not quads or shaped text. Rotated
//! text elements are therefore drawn with upright glyphs stepped along the true
//! rotated advance — the footprint is exact and the characters stay legible, but
//! individual glyphs are not turned the way the printer turns them. A rotation
//! chip is drawn on the element so the difference is never a surprise.

use gpui_kit::{
    App, Bounds, Hsla, Pixels, Point, SharedString, TextAlign, TextRun, Window, fill, point,
    px, size,
};

use crate::barcode;
use crate::fgl;
use crate::ticket::{Doc, Element, Kind};

// ---------------------------------------------------------------------------
// Projection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct Layout {
    /// Window pixels per printer dot.
    pub scale: f32,
    /// Window position of the ticket's (col 0, row 0) corner.
    pub origin: Point<Pixels>,
    pub len_dots: f32,
    pub wid_dots: f32,
}

/// Breathing room between the ticket and the edge of its pane.
const PAD: f32 = 28.0;

impl Layout {
    /// Fit the stock inside `bounds`, centred, preserving aspect.
    pub fn fit(doc: &Doc, bounds: Bounds<Pixels>) -> Self {
        let len = doc.length_dots() as f32;
        let wid = doc.width_dots() as f32;

        let usable_w = (f32::from(bounds.size.width) - PAD * 2.0).max(1.0);
        let usable_h = (f32::from(bounds.size.height) - PAD * 2.0).max(1.0);
        let scale = (usable_w / len).min(usable_h / wid).max(0.001);

        let origin = point(
            bounds.origin.x + px((f32::from(bounds.size.width) - len * scale) / 2.0),
            bounds.origin.y + px((f32::from(bounds.size.height) - wid * scale) / 2.0),
        );

        Self {
            scale,
            origin,
            len_dots: len,
            wid_dots: wid,
        }
    }

    pub fn to_px(&self, col: f32, row: f32) -> Point<Pixels> {
        point(
            self.origin.x + px(col * self.scale),
            self.origin.y + px(row * self.scale),
        )
    }

    /// Window point back to (column, row) in dots.
    pub fn to_dots(&self, p: Point<Pixels>) -> (f32, f32) {
        (
            (f32::from(p.x) - f32::from(self.origin.x)) / self.scale,
            (f32::from(p.y) - f32::from(self.origin.y)) / self.scale,
        )
    }

    fn rect(&self, col0: f32, row0: f32, col1: f32, row1: f32) -> Bounds<Pixels> {
        let a = self.to_px(col0.min(col1), row0.min(row1));
        let w = (col1 - col0).abs() * self.scale;
        let h = (row1 - row0).abs() * self.scale;
        Bounds {
            origin: a,
            size: size(px(w), px(h)),
        }
    }

    fn bounds_rect(&self, b: crate::ticket::Bounds) -> Bounds<Pixels> {
        self.rect(b.col0, b.row0, b.col1, b.row1)
    }
}

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

/// Deliberately fixed rather than theme-derived: this is a picture of a printed
/// object, and thermal stock does not change colour when the app switches to
/// dark mode.
pub struct Palette {
    pub stock: Hsla,
    pub ink: Hsla,
    pub margin: Hsla,
    pub grid: Hsla,
    pub select: Hsla,
    pub warn: Hsla,
    pub shadow: Hsla,
    /// Keeps the edge of the stock crisp whatever the pane behind it is doing.
    pub edge: Hsla,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            stock: hsla_from_rgb(0xfa_f9_f4, 1.0),
            ink: hsla_from_rgb(0x18_18_1c, 1.0),
            margin: hsla_from_rgb(0xd2_78_78, 0.55),
            grid: hsla_from_rgb(0x00_00_00, 0.08),
            select: hsla_from_rgb(0x2f_7d_f6, 0.95),
            warn: hsla_from_rgb(0xe0_53_3d, 0.95),
            shadow: hsla_from_rgb(0x00_00_00, 0.14),
            edge: hsla_from_rgb(0x9a_98_92, 0.9),
        }
    }
}

fn hsla_from_rgb(rgb: u32, alpha: f32) -> Hsla {
    let r = ((rgb >> 16) & 0xff) as f32 / 255.0;
    let g = ((rgb >> 8) & 0xff) as f32 / 255.0;
    let b = (rgb & 0xff) as f32 / 255.0;
    gpui_kit::Rgba { r, g, b, a: alpha }.into()
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

pub struct PaintOptions {
    pub show_grid: bool,
    pub mono_font: SharedString,
    pub palette: Palette,
}

pub fn paint(
    doc: &Doc,
    layout: &Layout,
    opts: &PaintOptions,
    window: &mut Window,
    cx: &mut App,
) {
    paint_stock(doc, layout, opts, window);

    for (index, element) in doc.elements.iter().enumerate() {
        if !element.visible {
            continue;
        }
        paint_element(doc, element, layout, opts, window, cx);
        if doc.selected == Some(index) {
            paint_selection(doc, element, layout, opts, window);
        }
    }
}

fn paint_stock(doc: &Doc, layout: &Layout, opts: &PaintOptions, window: &mut Window) {
    let full = layout.rect(0.0, 0.0, layout.len_dots, layout.wid_dots);

    // A soft drop shadow so the stock reads as a physical object.
    let mut shadow = full;
    shadow.origin.x += px(3.0);
    shadow.origin.y += px(5.0);
    window.paint_quad(fill(shadow, opts.palette.shadow));
    window.paint_quad(fill(full, opts.palette.stock));
    stroke_rect(window, full, opts.palette.edge, 1.0);

    if opts.show_grid {
        // A line every quarter inch.
        let step = doc.density.dots() as f32 / 4.0;
        let mut c = step;
        while c < layout.len_dots {
            window.paint_quad(fill(
                layout.rect(c, 0.0, c + 1.0, layout.wid_dots),
                opts.palette.grid,
            ));
            c += step;
        }
        let mut r = step;
        while r < layout.wid_dots {
            window.paint_quad(fill(
                layout.rect(0.0, r, layout.len_dots, r + 1.0),
                opts.palette.grid,
            ));
            r += step;
        }
    }

    // The printer reserves a margin it cannot print into; show where it is.
    let m = doc.margin_dots as f32;
    if m > 0.0 {
        stroke_rect(
            window,
            layout.rect(m, m, layout.len_dots - m, layout.wid_dots - m),
            opts.palette.margin,
            1.0,
        );
    }
}

fn paint_element(
    doc: &Doc,
    element: &Element,
    layout: &Layout,
    opts: &PaintOptions,
    window: &mut Window,
    cx: &mut App,
) {
    match element.kind {
        Kind::Text => paint_text(&element.text, element, layout, opts, window, cx),
        Kind::Counter => {
            // <PC> prints seven digits; show what that will look like.
            let digits = format!("{:07}", doc.start_number % 10_000_000);
            paint_text(&digits, element, layout, opts, window, cx)
        }
        Kind::Barcode => paint_barcode(element, layout, opts, window, cx),
        Kind::HLine | Kind::VLine => {
            window.paint_quad(fill(
                layout.bounds_rect(element.bounds()),
                opts.palette.ink,
            ));
        }
        Kind::Box => {
            // A box is a frame: thickness grows inward from each edge.
            let b = element.bounds();
            let t = element.thickness.max(1) as f32;
            for r in [
                layout.rect(b.col0, b.row0, b.col1, b.row0 + t),
                layout.rect(b.col0, b.row1 - t, b.col1, b.row1),
                layout.rect(b.col0, b.row0, b.col0 + t, b.row1),
                layout.rect(b.col1 - t, b.row0, b.col1, b.row1),
            ] {
                window.paint_quad(fill(r, opts.palette.ink));
            }
        }
    }
}

/// Characters are placed one box-advance apart, exactly as the printer places
/// them, rather than letting the screen font decide the spacing. That is the
/// whole point of a print preview.
fn paint_text(
    text: &str,
    element: &Element,
    layout: &Layout,
    opts: &PaintOptions,
    window: &mut Window,
    cx: &mut App,
) {
    let style = &element.style;
    let advance = style.advance_dots() as f32;
    let glyph_h = style.height_dots() as f32;
    let glyph_w = style.glyph_width_dots() as f32;
    if advance <= 0.0 || glyph_h <= 0.0 {
        return;
    }

    // Inverted text prints white on a solid block the size of the character box.
    if style.invert {
        window.paint_quad(fill(
            layout.bounds_rect(element.bounds()),
            opts.palette.ink,
        ));
    }

    let color = if style.invert {
        opts.palette.stock
    } else {
        opts.palette.ink
    };
    let font_size = fit_font_size(glyph_w, glyph_h, layout.scale, opts, window);
    let (dir_col, dir_row) = style.rotation.advance();
    let (start_col, start_row) = start_anchor(element, text);

    for (i, ch) in fgl::printable_chars(text).enumerate() {
        let step = i as f32;
        let col = start_col + dir_col * advance * step;
        let row = start_row + dir_row * advance * step;
        paint_glyph(ch, layout.to_px(col, row), font_size, color, opts, window, cx);
    }

    if style.rotation != fgl::Rotation::Nr {
        paint_rotation_chip(element, layout, opts, window);
    }
}

/// Where the first character's own top-left sits. For a centred field the run is
/// pushed along the advance axis by half the slack, the way the printer centres
/// it.
fn start_anchor(element: &Element, text: &str) -> (f32, f32) {
    let style = &element.style;
    let mut col = element.col as f32;
    let mut row = element.row as f32;

    if style.center_field > 0 {
        let field = style.center_field as f32;
        let run = style.advance_dots() as f32 * fgl::printable_len(text) as f32;
        // A string wider than the field is left justified and runs on.
        let slack = (field - run).max(0.0) / 2.0;
        let (dir_col, dir_row) = style.rotation.advance();
        col += dir_col * slack;
        row += dir_row * slack;
    }

    (col, row)
}

/// Pick a screen font size that occupies the same cell the printer's font will.
///
/// FGL's resident fonts are tall and narrow compared with a screen face, so a
/// size chosen from the glyph height alone produces letters that collide at the
/// printer's advance. Measure a representative glyph and shrink to fit the width
/// as well.
fn fit_font_size(
    glyph_w_dots: f32,
    glyph_h_dots: f32,
    scale: f32,
    opts: &PaintOptions,
    window: &Window,
) -> Pixels {
    let from_height = (glyph_h_dots * 0.78 * scale).max(1.0);
    let max_w = glyph_w_dots * scale;

    let run = TextRun {
        len: 1,
        font: gpui_kit::font(opts.mono_font.clone()),
        color: gpui_kit::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let measured = window
        .text_system()
        .shape_line("M".into(), px(from_height), &[run], None);
    let measured_w = f32::from(measured.width);

    if measured_w > max_w && measured_w > 0.0 {
        px((from_height * max_w / measured_w).max(1.0))
    } else {
        px(from_height)
    }
}

fn paint_glyph(
    ch: char,
    origin: Point<Pixels>,
    font_size: Pixels,
    color: Hsla,
    opts: &PaintOptions,
    window: &mut Window,
    cx: &mut App,
) {
    let mut buf = [0u8; 4];
    let text: SharedString = ch.encode_utf8(&mut buf).to_string().into();
    let run = TextRun {
        len: text.len(),
        font: gpui_kit::font(opts.mono_font.clone()),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(text, font_size, &[run], None);
    let _ = line.paint(origin, font_size, TextAlign::Left, None, window, cx);
}

fn paint_barcode(
    element: &Element,
    layout: &Layout,
    opts: &PaintOptions,
    window: &mut Window,
    cx: &mut App,
) {
    let settings = &element.barcode;
    let b = element.bounds();

    if settings.symbology == barcode::Symbology::Qr {
        return paint_qr_placeholder(b, layout, opts, window);
    }

    if barcode::validate(settings.symbology, &element.data).is_err() {
        // Show the space it would take up so the layout still reads.
        stroke_rect(window, layout.bounds_rect(b), opts.palette.warn, 1.0);
        return;
    }

    let pattern = barcode::encode(settings.symbology, &element.data, settings);
    if pattern.is_empty() {
        return;
    }

    let module = settings.expansion.max(1) as f32;
    let bar_len = settings.height_units.max(1) as f32 * 8.0;
    let along_col = settings.orientation == barcode::Orientation::Picket;

    for (start, len) in pattern.bar_runs() {
        let a0 = start as f32 * module;
        let a1 = a0 + len as f32 * module;
        let rect = if along_col {
            layout.rect(b.col0 + a0, b.row0, b.col0 + a1, b.row0 + bar_len)
        } else {
            layout.rect(b.col1 - bar_len, b.row0 + a0, b.col1, b.row0 + a1)
        };
        window.paint_quad(fill(rect, opts.palette.ink));
    }

    if settings.interpretation && settings.symbology.supports_interpretation() {
        paint_interpretation(element, b, bar_len, along_col, layout, opts, window, cx);
    }
}

/// The `<BI>` line the printer sets in font1 under the bars.
#[allow(clippy::too_many_arguments)]
fn paint_interpretation(
    element: &Element,
    b: crate::ticket::Bounds,
    bar_len: f32,
    along_col: bool,
    layout: &Layout,
    opts: &PaintOptions,
    window: &mut Window,
    cx: &mut App,
) {
    let data = &element.data;
    let count = fgl::printable_len(data);
    if count == 0 {
        return;
    }

    let font1 = fgl::Font::F1.metrics();
    let span = if along_col { b.width() } else { b.height() };
    let advance = span / count as f32;
    let font_size = fit_font_size(
        advance.min(font1.box_w as f32),
        font1.glyph_h as f32,
        layout.scale,
        opts,
        window,
    );

    for (i, ch) in fgl::printable_chars(data).enumerate() {
        let step = i as f32;
        let origin = if along_col {
            layout.to_px(b.col0 + advance * step, b.row0 + bar_len + 2.0)
        } else {
            layout.to_px(b.col0, b.row0 + advance * step)
        };
        paint_glyph(
            ch,
            origin,
            font_size,
            opts.palette.ink,
            opts,
            window,
            cx,
        );
    }
}

/// QR symbols are encoded in firmware, so the preview shows the exact footprint
/// with finder patterns rather than inventing modules that would not scan.
fn paint_qr_placeholder(
    b: crate::ticket::Bounds,
    layout: &Layout,
    opts: &PaintOptions,
    window: &mut Window,
) {
    let side = b.width().min(b.height());
    if side <= 0.0 {
        return;
    }
    let unit = side / 25.0;

    window.paint_quad(fill(
        layout.bounds_rect(b),
        hsla_from_rgb(0xec_eb_e6, 1.0),
    ));

    for (dc, dr) in [(0.0, 0.0), (side - unit * 7.0, 0.0), (0.0, side - unit * 7.0)] {
        let c0 = b.col0 + dc;
        let r0 = b.row0 + dr;
        // Finder pattern: filled square, light ring, filled centre.
        window.paint_quad(fill(
            layout.rect(c0, r0, c0 + unit * 7.0, r0 + unit * 7.0),
            opts.palette.ink,
        ));
        window.paint_quad(fill(
            layout.rect(c0 + unit, r0 + unit, c0 + unit * 6.0, r0 + unit * 6.0),
            opts.palette.stock,
        ));
        window.paint_quad(fill(
            layout.rect(
                c0 + unit * 2.0,
                r0 + unit * 2.0,
                c0 + unit * 5.0,
                r0 + unit * 5.0,
            ),
            opts.palette.ink,
        ));
    }
    stroke_rect(window, layout.bounds_rect(b), opts.palette.ink, 1.0);
}

fn paint_selection(
    doc: &Doc,
    element: &Element,
    layout: &Layout,
    opts: &PaintOptions,
    window: &mut Window,
) {
    let color = if doc.overflows(element) {
        opts.palette.warn
    } else {
        opts.palette.select
    };

    let mut r = layout.bounds_rect(element.bounds());
    r.origin.x -= px(2.0);
    r.origin.y -= px(2.0);
    r.size.width += px(4.0);
    r.size.height += px(4.0);
    stroke_rect(window, r, color, 1.5);

    // A dot on the element's anchor, which is the point <RC> actually names.
    let a = layout.to_px(element.col as f32, element.row as f32);
    window.paint_quad(fill(
        Bounds {
            origin: point(a.x - px(3.0), a.y - px(3.0)),
            size: size(px(6.0), px(6.0)),
        },
        color,
    ));
}

/// A small tick on elements whose glyphs the printer will turn but the preview
/// cannot.
fn paint_rotation_chip(
    element: &Element,
    layout: &Layout,
    opts: &PaintOptions,
    window: &mut Window,
) {
    let a = layout.to_px(element.col as f32, element.row as f32);
    let (dir_col, dir_row) = element.style.rotation.advance();
    let len = px(10.0);

    // A short stub pointing the way the run advances.
    let bounds = Bounds {
        origin: point(a.x, a.y),
        size: if dir_col != 0.0 {
            size(len, px(2.0))
        } else {
            size(px(2.0), len)
        },
    };
    let mut bounds = bounds;
    if dir_col < 0.0 {
        bounds.origin.x -= len;
    }
    if dir_row < 0.0 {
        bounds.origin.y -= len;
    }
    window.paint_quad(fill(bounds, opts.palette.select));
}

fn stroke_rect(window: &mut Window, r: Bounds<Pixels>, color: Hsla, thickness: f32) {
    if f32::from(r.size.width) <= 0.0 || f32::from(r.size.height) <= 0.0 {
        return;
    }
    let t = px(thickness);
    let edges: [Bounds<Pixels>; 4] = [
        Bounds {
            origin: r.origin,
            size: size(r.size.width, t),
        },
        Bounds {
            origin: point(r.origin.x, r.origin.y + r.size.height - t),
            size: size(r.size.width, t),
        },
        Bounds {
            origin: r.origin,
            size: size(t, r.size.height),
        },
        Bounds {
            origin: point(r.origin.x + r.size.width - t, r.origin.y),
            size: size(t, r.size.height),
        },
    ];
    for edge in edges {
        window.paint_quad(fill(edge, color));
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ticket::Doc;

    fn bounds(w: f32, h: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(w), px(h)),
        }
    }

    #[test]
    fn layout_scales_to_fit_the_tighter_axis() {
        let doc = Doc::default(); // 1100 x 400 dots
        // Exactly enough width for 1:1 once the padding is taken off.
        let l = Layout::fit(&doc, bounds(1100.0 + PAD * 2.0, 4000.0));
        assert!((l.scale - 1.0).abs() < 0.001);

        // Half the room on the long axis.
        let l = Layout::fit(&doc, bounds(550.0 + PAD * 2.0, 4000.0));
        assert!((l.scale - 0.5).abs() < 0.001);
    }

    #[test]
    fn dots_map_straight_through_at_unit_scale() {
        let doc = Doc::default();
        let l = Layout::fit(&doc, bounds(1100.0 + PAD * 2.0, 400.0 + PAD * 2.0));
        let a = l.to_px(0.0, 0.0);
        let b = l.to_px(100.0, 50.0);
        assert!((f32::from(b.x) - f32::from(a.x) - 100.0).abs() < 0.01);
        assert!((f32::from(b.y) - f32::from(a.y) - 50.0).abs() < 0.01);
    }

    #[test]
    fn screen_and_dot_coordinates_round_trip() {
        let doc = Doc::default();
        let l = Layout::fit(&doc, bounds(900.0, 700.0));
        let (col, row) = l.to_dots(l.to_px(321.0, 123.0));
        assert!((col - 321.0).abs() < 0.01);
        assert!((row - 123.0).abs() < 0.01);
    }

    #[test]
    fn the_ticket_is_centred_in_its_pane() {
        let doc = Doc::default();
        let pane = bounds(1600.0, 900.0);
        let l = Layout::fit(&doc, pane);

        let left = f32::from(l.origin.x);
        let right = 1600.0 - (left + l.len_dots * l.scale);
        assert!((left - right).abs() < 0.01, "left {left} right {right}");

        let top = f32::from(l.origin.y);
        let bottom = 900.0 - (top + l.wid_dots * l.scale);
        assert!((top - bottom).abs() < 0.01);
    }

    #[test]
    fn a_dot_rect_keeps_its_proportions() {
        let doc = Doc::default();
        let l = Layout::fit(&doc, bounds(1100.0 + PAD * 2.0, 400.0 + PAD * 2.0));
        let r = l.rect(10.0, 20.0, 110.0, 60.0);
        assert!((f32::from(r.size.width) - 100.0).abs() < 0.01);
        assert!((f32::from(r.size.height) - 40.0).abs() < 0.01);
    }

    #[test]
    fn centring_offsets_the_run_by_half_the_slack() {
        let mut e = Element::default();
        e.col = 100;
        e.row = 10;
        e.text = "AB".into();
        e.style.center_field = 400;

        // font3 advances 20 dots per character, so a two character run is 40
        // wide in a 400 dot field: 180 dots of slack each side.
        let (col, row) = start_anchor(&e, &e.text);
        assert!((col - 280.0).abs() < 0.001);
        assert!((row - 10.0).abs() < 0.001);
    }

    #[test]
    fn a_run_wider_than_its_field_is_not_pushed_backwards() {
        let mut e = Element::default();
        e.col = 50;
        e.text = "ABCDEFGHIJ".into();
        e.style.center_field = 40;

        let (col, _) = start_anchor(&e, &e.text);
        assert!((col - 50.0).abs() < 0.001);
    }

}
