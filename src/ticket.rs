//! The ticket document: stock size, the elements on it, and the code that turns
//! the whole thing into one FGL print job.
//!
//! Positions are in printer dots throughout, in FGL's own frame:
//!
//! ```text
//! col ──────────────────────────────►  along the paper feed (the 5.5")
//! row │
//!     │   (0,0) is the top left of the stock
//!     ▼   across the print head (the 2")
//! ```

use crate::barcode::{self, Settings as BarcodeSettings};
use crate::fgl::{self, Density, PrintMode, TextStyle};

pub const MAX_ELEMENTS: usize = 64;

// ---------------------------------------------------------------------------
// Elements
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Barcode,
    /// The printer's own seven digit ticket counter, `<PC>`.
    Counter,
    HLine,
    VLine,
    Box,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Text,
        Kind::Barcode,
        Kind::Counter,
        Kind::HLine,
        Kind::VLine,
        Kind::Box,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Text => "Text",
            Kind::Barcode => "Barcode",
            Kind::Counter => "Ticket number",
            Kind::HLine => "Horizontal rule",
            Kind::VLine => "Vertical rule",
            Kind::Box => "Box",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Kind::Text => "Text",
            Kind::Barcode => "Barcode",
            Kind::Counter => "Number",
            Kind::HLine => "H rule",
            Kind::VLine => "V rule",
            Kind::Box => "Box",
        }
    }
}

/// One thing on the ticket.
///
/// Every kind shares the struct rather than carrying its own payload in an enum
/// variant, so that switching an element's kind in the inspector keeps whatever
/// you had set up for the old one.
#[derive(Debug, Clone)]
pub struct Element {
    pub kind: Kind,
    pub name: String,
    /// Dots down from the top edge.
    pub row: i32,
    /// Dots right from the left edge.
    pub col: i32,
    pub visible: bool,

    pub text: String,
    pub style: TextStyle,

    pub data: String,
    pub barcode: BarcodeSettings,

    /// Length of a rule, or the width of a box, in dots.
    pub span: u16,
    /// Height of a box, in dots.
    pub span_across: u16,
    pub thickness: u8,
}

impl Default for Element {
    fn default() -> Self {
        Self {
            kind: Kind::Text,
            name: String::new(),
            row: 0,
            col: 0,
            visible: true,
            text: String::new(),
            style: TextStyle::default(),
            data: String::new(),
            barcode: BarcodeSettings::default(),
            span: 400,
            span_across: 60,
            thickness: 2,
        }
    }
}

impl Element {
    /// A label for the element list: the given name, else the content itself.
    pub fn display_name(&self) -> &str {
        if !self.name.is_empty() {
            return &self.name;
        }
        match self.kind {
            Kind::Text => {
                if self.text.is_empty() {
                    "(empty text)"
                } else {
                    &self.text
                }
            }
            Kind::Barcode => {
                if self.data.is_empty() {
                    "(empty barcode)"
                } else {
                    &self.data
                }
            }
            Kind::Counter => "ticket number",
            Kind::HLine | Kind::VLine => "rule",
            Kind::Box => "box",
        }
    }

    /// Axis-aligned extent in dots.
    pub fn bounds(&self) -> Bounds {
        let r = self.row as f32;
        let c = self.col as f32;

        match self.kind {
            Kind::Text => oriented(
                r,
                c,
                self.style.width_dots(&self.text) as f32,
                self.style.height_dots() as f32,
                self.style.rotation,
            ),
            Kind::Counter => {
                let m = self.style.font.metrics();
                // <PC> always prints seven digits and ignores <HW>.
                oriented(
                    r,
                    c,
                    (7 * m.box_w) as f32,
                    m.glyph_h as f32,
                    self.style.rotation,
                )
            }
            Kind::Barcode => {
                let f = barcode::footprint(&self.data, &self.barcode);
                let w = f.width_dots as f32;
                let h = f.height_dots as f32;
                // The footprint already accounts for orientation, so only the
                // anchor corner changes with the rotation.
                match self.barcode.rotation() {
                    fgl::Rotation::Nr => Bounds::new(c, r, c + w, r + h),
                    fgl::Rotation::Ru => Bounds::new(c - w, r - h, c, r),
                    fgl::Rotation::Rr => Bounds::new(c - w, r, c, r + h),
                    fgl::Rotation::Rl => Bounds::new(c, r - h, c + w, r),
                }
            }
            // Line thickness grows toward the bottom (horizontal) or the right
            // (vertical); a box's thickness grows inward.
            Kind::HLine => Bounds::new(c, r, c + self.span as f32, r + self.thickness as f32),
            Kind::VLine => Bounds::new(c, r, c + self.thickness as f32, r + self.span as f32),
            Kind::Box => Bounds::new(c, r, c + self.span as f32, r + self.span_across as f32),
        }
    }

    /// Is (col, row) on this element?
    ///
    /// Boxes are frames, not fills: a border drawn around the whole ticket would
    /// otherwise swallow every click meant for the text inside it.
    pub fn hit_test(&self, col: f32, row: f32) -> bool {
        let b = self.bounds();
        if !b.contains(col, row) {
            return false;
        }
        if self.kind != Kind::Box {
            return true;
        }

        // A grab margin keeps thin borders clickable.
        let t = (self.thickness as f32).max(4.0);
        let inner = Bounds::new(b.col0 + t, b.row0 + t, b.col1 - t, b.row1 - t);
        if inner.col1 <= inner.col0 || inner.row1 <= inner.row0 {
            return true;
        }
        !inner.contains(col, row)
    }

    fn emit(&self, out: &mut String) {
        match self.kind {
            Kind::Text => fgl::write_text(out, self.row, self.col, &self.text, &self.style),
            Kind::Barcode => barcode::write(out, self.row, self.col, &self.data, &self.barcode),
            Kind::Counter => {
                fgl::write_print_count(out, self.row, self.col, self.style.font, self.style.rotation)
            }
            Kind::HLine => fgl::write_hline(out, self.row, self.col, self.span, self.thickness),
            Kind::VLine => fgl::write_vline(out, self.row, self.col, self.span, self.thickness),
            Kind::Box => fgl::write_box(
                out,
                self.row,
                self.col,
                self.span_across,
                self.span,
                self.thickness,
            ),
        }
    }

    /// The problem with this element, if any, as something to show the user.
    pub fn problem(&self, doc: &Doc) -> Option<String> {
        if !self.visible {
            return None;
        }
        if self.kind == Kind::Barcode {
            if let Err(msg) = barcode::validate(self.barcode.symbology, &self.data) {
                return Some(msg.to_string());
            }
        }
        if doc.overflows(self) {
            return Some("falls outside the printable area".to_string());
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub col0: f32,
    pub row0: f32,
    pub col1: f32,
    pub row1: f32,
}

impl Bounds {
    pub fn new(col0: f32, row0: f32, col1: f32, row1: f32) -> Self {
        Self {
            col0,
            row0,
            col1,
            row1,
        }
    }
    pub fn width(&self) -> f32 {
        self.col1 - self.col0
    }
    pub fn height(&self) -> f32 {
        self.row1 - self.row0
    }
    pub fn contains(&self, col: f32, row: f32) -> bool {
        col >= self.col0 && col <= self.col1 && row >= self.row0 && row <= self.row1
    }
}

/// Lay a `w` x `h` box down from an anchor, in the direction the rotation makes
/// text grow.
///
/// The guide puts it as "facing in the direction of rotation, all characters
/// build down and to the right of their starting points". Rotating the advance
/// direction (+col) and the glyph-height direction (+row) by the rotation gives:
///
/// ```text
/// NR   advance +col, height +row
/// RR   advance +row, height -col
/// RU   advance -col, height -row
/// RL   advance -row, height +col
/// ```
fn oriented(row: f32, col: f32, w: f32, h: f32, rot: fgl::Rotation) -> Bounds {
    match rot {
        fgl::Rotation::Nr => Bounds::new(col, row, col + w, row + h),
        fgl::Rotation::Rr => Bounds::new(col - h, row, col, row + w),
        fgl::Rotation::Ru => Bounds::new(col - w, row - h, col, row),
        fgl::Rotation::Rl => Bounds::new(col, row - w, col + h, row),
    }
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Doc {
    pub density: Density,
    /// Along the feed. The long dimension of a 2 x 5.5 ticket.
    pub length_in: f32,
    /// Across the print head.
    pub width_in: f32,
    /// Unprintable perimeter the printer reserves, drawn as a guide.
    pub margin_dots: u16,

    pub qty: u32,
    pub print_mode: PrintMode,
    /// Preload the printer's counter so a run comes out numbered.
    pub numbering: bool,
    pub start_number: u32,
    /// `<PL#>` caps the printed area. Off by default; the printer measures the
    /// stock at power-up and uses that.
    pub limit_print_length: bool,

    pub elements: Vec<Element>,
    pub selected: Option<usize>,
}

impl Default for Doc {
    fn default() -> Self {
        Self {
            density: Density::Dpi200,
            length_in: 5.5,
            width_in: 2.0,
            margin_dots: 16,
            qty: 1,
            print_mode: PrintMode::Cut,
            numbering: false,
            start_number: 1,
            limit_print_length: false,
            elements: Vec::new(),
            selected: None,
        }
    }
}

impl Doc {
    pub fn length_dots(&self) -> u32 {
        (self.length_in * self.density.dots() as f32).max(1.0) as u32
    }

    pub fn width_dots(&self) -> u32 {
        (self.width_in * self.density.dots() as f32).max(1.0) as u32
    }

    pub fn selected_element(&self) -> Option<&Element> {
        self.selected.and_then(|i| self.elements.get(i))
    }

    pub fn selected_element_mut(&mut self) -> Option<&mut Element> {
        match self.selected {
            Some(i) => self.elements.get_mut(i),
            None => None,
        }
    }

    pub fn add(&mut self, element: Element) -> bool {
        if self.elements.len() >= MAX_ELEMENTS {
            return false;
        }
        self.elements.push(element);
        self.selected = Some(self.elements.len() - 1);
        true
    }

    pub fn remove(&mut self, index: usize) {
        if index >= self.elements.len() {
            return;
        }
        self.elements.remove(index);
        self.selected = if self.elements.is_empty() {
            None
        } else {
            Some(self.selected.unwrap_or(0).min(self.elements.len() - 1))
        };
    }

    pub fn duplicate(&mut self, index: usize) {
        let Some(source) = self.elements.get(index) else {
            return;
        };
        let mut copy = source.clone();
        // Nudge it so the copy is visibly a separate thing.
        copy.row += 12;
        copy.col += 12;
        self.add(copy);
    }

    /// Move an element up or down the draw order. Later elements print later,
    /// which matters where they overlap.
    pub fn reorder(&mut self, index: usize, delta: i32) {
        let target = index as i32 + delta;
        if index >= self.elements.len() || target < 0 || target as usize >= self.elements.len() {
            return;
        }
        let target = target as usize;
        self.elements.swap(index, target);
        self.selected = Some(target);
    }

    /// True when any part of `element` falls outside the printable area.
    pub fn overflows(&self, element: &Element) -> bool {
        let b = element.bounds();
        let m = self.margin_dots as f32;
        let max_col = self.length_dots() as f32 - m;
        let max_row = self.width_dots() as f32 - m;
        b.col0 < m || b.row0 < m || b.col1 > max_col || b.row1 > max_row
    }

    /// Count of elements the printer will reject or clip.
    pub fn problem_count(&self) -> usize {
        self.elements
            .iter()
            .filter(|e| e.problem(self).is_some())
            .count()
    }

    /// Topmost element at a point, since later elements draw over earlier ones.
    pub fn hit(&self, col: f32, row: f32) -> Option<usize> {
        self.elements
            .iter()
            .enumerate()
            .rev()
            .find(|(_, e)| e.visible && e.hit_test(col, row))
            .map(|(i, _)| i)
    }

    /// The complete job: every visible element, then the repeat count, then the
    /// print command.
    ///
    /// Deliberately absent is `<CB>`. The guide is explicit that it degrades
    /// throughput and that the printer clears itself between tickets anyway.
    pub fn emit(&self) -> String {
        let mut out = String::new();
        if self.limit_print_length {
            use std::fmt::Write;
            let _ = write!(out, "<PL{}>", self.length_dots());
        }
        if self.numbering {
            fgl::write_load_count(&mut out, self.start_number);
        }
        for element in self.elements.iter().filter(|e| e.visible) {
            element.emit(&mut out);
        }
        fgl::write_repeat(&mut out, self.qty);
        fgl::write_print(&mut out, self.print_mode);
        out
    }
}

// ---------------------------------------------------------------------------
// Starter ticket
// ---------------------------------------------------------------------------

fn text_element(name: &str, row: i32, col: i32, content: &str, style: TextStyle) -> Element {
    Element {
        kind: Kind::Text,
        name: name.to_string(),
        row,
        col,
        text: content.to_string(),
        style,
        ..Default::default()
    }
}

/// A believable 2" x 5.5" event ticket at 200 dpi, laid out inside the 1050 x 383
/// printable area the guide quotes for that stock.
pub fn starter() -> Doc {
    let mut doc = Doc::default();

    doc.elements.push(Element {
        kind: Kind::Box,
        name: "border".into(),
        row: 20,
        col: 20,
        span: 1060,
        span_across: 360,
        thickness: 2,
        ..Default::default()
    });

    doc.elements.push(text_element(
        "event",
        34,
        44,
        "MIDNIGHT ELECTRIC",
        TextStyle {
            font: fgl::Font::F6,
            ..Default::default()
        },
    ));
    doc.elements.push(text_element(
        "date",
        100,
        44,
        "SAT OCT 18 2026   8:00 PM",
        TextStyle::default(),
    ));
    doc.elements.push(text_element(
        "venue",
        140,
        44,
        "THE ORPHEUM - LOS ANGELES",
        TextStyle {
            font: fgl::Font::F2,
            ..Default::default()
        },
    ));

    doc.elements.push(Element {
        kind: Kind::HLine,
        name: "rule".into(),
        row: 176,
        col: 44,
        span: 620,
        thickness: 2,
        ..Default::default()
    });

    doc.elements.push(text_element(
        "seat",
        200,
        44,
        "SEC 12   ROW H   SEAT 24",
        TextStyle::default(),
    ));
    doc.elements.push(text_element(
        "admission",
        246,
        44,
        "GENERAL ADMISSION",
        TextStyle {
            font: fgl::Font::F2,
            ..Default::default()
        },
    ));
    doc.elements.push(text_element(
        "price",
        280,
        44,
        "$45.00",
        TextStyle {
            font: fgl::Font::F6,
            ..Default::default()
        },
    ));
    doc.elements.push(text_element(
        "terms",
        352,
        44,
        "NO REFUNDS - NO EXCHANGES - ALL SALES FINAL",
        TextStyle {
            font: fgl::Font::F1,
            ..Default::default()
        },
    ));
    doc.elements.push(text_element(
        "scan label",
        120,
        748,
        "SCAN AT GATE",
        TextStyle {
            font: fgl::Font::F1,
            ..Default::default()
        },
    ));

    doc.elements.push(Element {
        kind: Kind::Barcode,
        name: "barcode".into(),
        row: 140,
        col: 748,
        data: "TKT-10024".into(),
        barcode: BarcodeSettings {
            symbology: barcode::Symbology::Code128,
            height_units: 6,
            expansion: 2,
            interpretation: true,
            ..Default::default()
        },
        ..Default::default()
    });

    doc.elements.push(Element {
        kind: Kind::Counter,
        name: "ticket number".into(),
        row: 240,
        col: 748,
        ..Default::default()
    });

    doc.selected = Some(1);
    doc
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_size_converts_to_dots_at_the_head_density() {
        let mut doc = Doc::default();
        assert_eq!(doc.length_dots(), 1100); // 5.5" x 200
        assert_eq!(doc.width_dots(), 400); // 2.0" x 200

        doc.density = Density::Dpi300;
        assert_eq!(doc.length_dots(), 1650);
        assert_eq!(doc.width_dots(), 600);
    }

    #[test]
    fn unrotated_text_grows_right_and_down_from_its_anchor() {
        let e = text_element("t", 100, 200, "AB", TextStyle::default());
        let b = e.bounds();
        assert_eq!((b.col0, b.row0), (200.0, 100.0));
        assert_eq!(b.col1, 240.0); // 2 chars x 20 dot box
        assert_eq!(b.row1, 131.0); // 31 dot glyph
    }

    #[test]
    fn rotations_move_the_body_around_the_anchor_as_the_guide_describes() {
        let rr = text_element(
            "t",
            100,
            200,
            "AB",
            TextStyle {
                rotation: fgl::Rotation::Rr,
                ..Default::default()
            },
        );
        // Rotated right reads top to bottom, so the run extends down and the
        // glyph body sits to the left of the anchor.
        let b = rr.bounds();
        assert_eq!((b.row0, b.row1), (100.0, 140.0));
        assert_eq!((b.col0, b.col1), (169.0, 200.0));

        let ru = text_element(
            "t",
            100,
            200,
            "AB",
            TextStyle {
                rotation: fgl::Rotation::Ru,
                ..Default::default()
            },
        );
        // Upside down reads right to left and builds up.
        let b = ru.bounds();
        assert_eq!((b.col0, b.col1), (160.0, 200.0));
        assert_eq!((b.row0, b.row1), (69.0, 100.0));

        let rl = text_element(
            "t",
            100,
            200,
            "AB",
            TextStyle {
                rotation: fgl::Rotation::Rl,
                ..Default::default()
            },
        );
        // Rotated left reads bottom to top.
        let b = rl.bounds();
        assert_eq!((b.row0, b.row1), (60.0, 100.0));
        assert_eq!((b.col0, b.col1), (200.0, 231.0));
    }

    #[test]
    fn overflow_detection_respects_the_margin() {
        let doc = Doc::default();
        assert!(!doc.overflows(&text_element("ok", 100, 100, "HI", TextStyle::default())));
        // Five characters of font3 from column 1060 runs off a 1100 dot ticket.
        assert!(doc.overflows(&text_element(
            "bad",
            100,
            1060,
            "HELLO",
            TextStyle::default()
        )));
        assert!(doc.overflows(&text_element("bad", 4, 100, "HI", TextStyle::default())));
    }

    #[test]
    fn a_box_is_grabbed_by_its_frame_not_its_hole() {
        let e = Element {
            kind: Kind::Box,
            row: 20,
            col: 20,
            span: 1000,
            span_across: 300,
            thickness: 2,
            ..Default::default()
        };
        assert!(e.hit_test(500.0, 21.0)); // top edge
        assert!(e.hit_test(22.0, 150.0)); // left edge
        assert!(!e.hit_test(500.0, 150.0)); // the hole, where text lives
        assert!(!e.hit_test(500.0, 400.0)); // outside entirely
    }

    #[test]
    fn other_kinds_are_hit_anywhere_inside_their_bounds() {
        let e = text_element("t", 100, 200, "HELLO", TextStyle::default());
        assert!(e.hit_test(210.0, 110.0));
        assert!(!e.hit_test(400.0, 110.0));
    }

    #[test]
    fn hit_testing_prefers_the_element_drawn_last() {
        let mut doc = Doc::default();
        doc.elements
            .push(text_element("under", 100, 100, "AAAA", TextStyle::default()));
        doc.elements
            .push(text_element("over", 100, 100, "AAAA", TextStyle::default()));
        assert_eq!(doc.hit(110.0, 110.0), Some(1));
    }

    #[test]
    fn hidden_elements_are_not_hit() {
        let mut doc = Doc::default();
        let mut e = text_element("hidden", 100, 100, "AAAA", TextStyle::default());
        e.visible = false;
        doc.elements.push(e);
        assert_eq!(doc.hit(110.0, 110.0), None);
    }

    #[test]
    fn add_duplicate_remove_and_reorder_keep_the_list_consistent() {
        let mut doc = Doc::default();
        doc.add(text_element("a", 0, 0, "A", TextStyle::default()));
        doc.add(text_element("b", 0, 0, "B", TextStyle::default()));
        assert_eq!(doc.elements.len(), 2);
        assert_eq!(doc.selected, Some(1));

        doc.duplicate(0);
        assert_eq!(doc.elements.len(), 3);
        assert_eq!(doc.elements[2].text, "A");
        assert_eq!(doc.elements[2].row, 12);

        doc.reorder(2, -1);
        assert_eq!(doc.elements[1].text, "A");
        assert_eq!(doc.elements[2].text, "B");

        doc.remove(0);
        assert_eq!(doc.elements.len(), 2);
        assert_eq!(doc.elements[0].text, "A");
    }

    #[test]
    fn removing_the_last_element_clears_the_selection() {
        let mut doc = Doc::default();
        doc.add(text_element("a", 0, 0, "A", TextStyle::default()));
        doc.remove(0);
        assert!(doc.elements.is_empty());
        assert_eq!(doc.selected, None);
    }

    #[test]
    fn the_element_list_is_bounded() {
        let mut doc = Doc::default();
        for _ in 0..MAX_ELEMENTS {
            assert!(doc.add(Element::default()));
        }
        assert!(!doc.add(Element::default()));
    }

    #[test]
    fn a_single_text_element_emits_a_complete_one_ticket_job() {
        let mut doc = Doc::default();
        doc.add(text_element("t", 10, 30, "ADMIT ONE", TextStyle::default()));
        assert_eq!(doc.emit(), "<NR><F3><HW1,1><RC10,30>ADMIT ONE<p>");
    }

    #[test]
    fn quantity_becomes_a_repeat_command_before_the_print_command() {
        let mut doc = Doc {
            qty: 50,
            ..Default::default()
        };
        doc.add(text_element("t", 0, 0, "X", TextStyle::default()));
        assert!(doc.emit().ends_with("<RE49><p>"));
    }

    #[test]
    fn numbering_preloads_the_counter_before_any_element() {
        let mut doc = Doc {
            numbering: true,
            start_number: 1001,
            ..Default::default()
        };
        doc.add(Element {
            kind: Kind::Counter,
            row: 20,
            col: 40,
            ..Default::default()
        });
        assert_eq!(doc.emit(), "<TC0001001><NR><F3><RC20,40><PC><p>");
    }

    #[test]
    fn hidden_elements_are_left_out_of_the_job() {
        let mut doc = Doc::default();
        doc.add(text_element("shown", 0, 0, "YES", TextStyle::default()));
        let mut hidden = text_element("hidden", 0, 0, "NO", TextStyle::default());
        hidden.visible = false;
        doc.add(hidden);

        let out = doc.emit();
        assert!(out.contains("YES"));
        assert!(!out.contains("NO"));
    }

    #[test]
    fn print_mode_selects_the_terminating_command() {
        let mut doc = Doc {
            print_mode: PrintMode::NoCut,
            ..Default::default()
        };
        doc.add(text_element("t", 0, 0, "X", TextStyle::default()));
        assert!(doc.emit().ends_with("<q>"));
    }

    #[test]
    fn the_starter_ticket_fits_inside_its_own_printable_area() {
        let doc = starter();
        assert!(!doc.elements.is_empty());
        for e in &doc.elements {
            assert!(
                !doc.overflows(e),
                "element {:?} overflows: {:?}",
                e.display_name(),
                e.bounds()
            );
        }
        assert_eq!(doc.problem_count(), 0);
    }

    #[test]
    fn the_starter_ticket_emits_a_job_that_ends_in_a_print_command() {
        let out = starter().emit();
        assert!(out.ends_with("<p>"));
        assert!(out.contains("MIDNIGHT ELECTRIC"));
        assert!(out.contains("^TKT-10024^"));
        assert!(out.contains("<PC>"));
    }

    #[test]
    fn a_barcode_with_unusable_data_is_counted_as_a_problem() {
        let mut doc = Doc::default();
        doc.add(Element {
            kind: Kind::Barcode,
            row: 50,
            col: 50,
            data: "12345".into(), // odd digit count
            barcode: BarcodeSettings {
                symbology: barcode::Symbology::I2of5,
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(doc.problem_count(), 1);

        doc.elements[0].data = "123456".into();
        assert_eq!(doc.problem_count(), 0);
    }

    #[test]
    fn display_name_falls_back_to_content_then_to_a_placeholder() {
        let mut e = Element::default();
        assert_eq!(e.display_name(), "(empty text)");
        e.text = "HELLO".into();
        assert_eq!(e.display_name(), "HELLO");
        e.name = "greeting".into();
        assert_eq!(e.display_name(), "greeting");
    }
}
