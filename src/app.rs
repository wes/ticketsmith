//! The Ticketsmith window.
//!
//! Layout is three bands: a toolbar for the stock itself, a body split between
//! the preview and an inspector column, and a status line. Everything that
//! changes the ticket lives on the right; the left is only the ticket.
//!
//! ## Field plumbing
//!
//! GPUI keeps editor state in entities rather than in the render pass, so every
//! text and number field is an `Entity<InputState>` that has to be kept in step
//! with the document. Rather than name two dozen of them, they live in one array
//! indexed by [`Field`], with a single change handler and a single sync
//! direction guard. The same trick handles the dropdowns via [`Choice`].

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState, NumberInput, NumberStep};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::separator::Separator;
use gpui_kit::component::{ActiveTheme, Disableable, IndexPath, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::barcode::{self, Orientation, QrEcc, QrVersion, Symbology};
use crate::fgl::{Density, Font, PrintMode, Rotation};
use crate::preview::{self, Layout, PaintOptions, Palette};
use crate::ticket::{self, Doc, Element, Kind};
use crate::transport::{self, Target};

actions!(
    ticketsmith,
    [
        NudgeLeft,
        NudgeRight,
        NudgeUp,
        NudgeDown,
        NudgeLeftFast,
        NudgeRightFast,
        NudgeUpFast,
        NudgeDownFast,
        DeleteSelected,
        Deselect,
    ]
);

/// Scoped to the window's own key context so they cannot leak into anything
/// else, and dispatched up the focus chain, so a focused text field consumes
/// arrows and backspace before they reach here.
///
/// Delete deliberately takes a modifier. Bare backspace is one stray keystroke
/// away from removing an element whenever focus is not in a field, which is not
/// a trade worth making to save a chord.
pub fn key_bindings() -> Vec<KeyBinding> {
    const CONTEXT: Option<&str> = Some("Ticketsmith");
    vec![
        KeyBinding::new("left", NudgeLeft, CONTEXT),
        KeyBinding::new("right", NudgeRight, CONTEXT),
        KeyBinding::new("up", NudgeUp, CONTEXT),
        KeyBinding::new("down", NudgeDown, CONTEXT),
        KeyBinding::new("shift-left", NudgeLeftFast, CONTEXT),
        KeyBinding::new("shift-right", NudgeRightFast, CONTEXT),
        KeyBinding::new("shift-up", NudgeUpFast, CONTEXT),
        KeyBinding::new("shift-down", NudgeDownFast, CONTEXT),
        KeyBinding::new("cmd-backspace", DeleteSelected, CONTEXT),
        KeyBinding::new("ctrl-backspace", DeleteSelected, CONTEXT),
        KeyBinding::new("escape", Deselect, CONTEXT),
    ]
}

// ---------------------------------------------------------------------------
// Fields and choices
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    // stock
    LengthIn,
    WidthIn,
    // element
    Name,
    Content,
    Row,
    Col,
    Span,
    SpanAcross,
    Thickness,
    // text style
    WidthMult,
    HeightMult,
    ScaleDown,
    CenterField,
    // barcode
    BarHeight,
    BarExpansion,
    QrPoint,
    // output
    Host,
    Port,
    Device,
    Queue,
    Path,
    // print
    Qty,
    StartNumber,
}

impl Field {
    const ALL: [Field; 23] = [
        Field::LengthIn,
        Field::WidthIn,
        Field::Name,
        Field::Content,
        Field::Row,
        Field::Col,
        Field::Span,
        Field::SpanAcross,
        Field::Thickness,
        Field::WidthMult,
        Field::HeightMult,
        Field::ScaleDown,
        Field::CenterField,
        Field::BarHeight,
        Field::BarExpansion,
        Field::QrPoint,
        Field::Host,
        Field::Port,
        Field::Device,
        Field::Queue,
        Field::Path,
        Field::Qty,
        Field::StartNumber,
    ];

    fn index(self) -> usize {
        Field::ALL.iter().position(|f| *f == self).unwrap()
    }

    /// `None` for free text, `Some((min, max))` for a stepped number field.
    fn range(self) -> Option<(f32, f32)> {
        match self {
            Field::Name | Field::Content | Field::Host | Field::Device | Field::Queue
            | Field::Path => None,
            Field::LengthIn => Some((0.5, 24.0)),
            Field::WidthIn => Some((0.5, 9.0)),
            Field::Row => Some((-500.0, 8000.0)),
            Field::Col => Some((-500.0, 20000.0)),
            Field::Span => Some((1.0, 20000.0)),
            Field::SpanAcross => Some((1.0, 8000.0)),
            Field::Thickness => Some((1.0, 40.0)),
            Field::WidthMult | Field::HeightMult => Some((1.0, 16.0)),
            Field::ScaleDown => Some((1.0, 8.0)),
            Field::CenterField => Some((1.0, 20000.0)),
            Field::BarHeight => Some((1.0, 40.0)),
            Field::BarExpansion => Some((1.0, 9.0)),
            Field::QrPoint => Some((3.0, 16.0)),
            Field::Port => Some((1.0, 65535.0)),
            Field::Qty => Some((1.0, 100000.0)),
            Field::StartNumber => Some((0.0, 9_999_999.0)),
        }
    }

    fn step(self) -> f32 {
        match self {
            Field::LengthIn | Field::WidthIn => 0.125,
            Field::Row | Field::Col | Field::Span | Field::SpanAcross | Field::CenterField => 10.0,
            Field::Qty => 25.0,
            _ => 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Density,
    Kind,
    Font,
    Rotation,
    Symbology,
    Orientation,
    QrEcc,
    QrVersion,
    PrintMode,
    Transport,
}

impl Choice {
    const ALL: [Choice; 10] = [
        Choice::Density,
        Choice::Kind,
        Choice::Font,
        Choice::Rotation,
        Choice::Symbology,
        Choice::Orientation,
        Choice::QrEcc,
        Choice::QrVersion,
        Choice::PrintMode,
        Choice::Transport,
    ];

    fn index(self) -> usize {
        Choice::ALL.iter().position(|c| *c == self).unwrap()
    }

    fn options(self) -> Vec<SharedString> {
        fn map<T: Copy>(items: &[T], label: impl Fn(T) -> &'static str) -> Vec<SharedString> {
            items.iter().map(|t| label(*t).into()).collect()
        }
        match self {
            Choice::Density => map(&Density::ALL, Density::label),
            Choice::Kind => map(&Kind::ALL, Kind::label),
            Choice::Font => map(&Font::ALL, Font::label),
            Choice::Rotation => map(&Rotation::ALL, Rotation::label),
            Choice::Symbology => map(&Symbology::ALL, Symbology::label),
            Choice::Orientation => map(&Orientation::ALL, Orientation::label),
            Choice::QrEcc => map(&QrEcc::ALL, QrEcc::label),
            Choice::QrVersion => map(&QrVersion::ALL, QrVersion::label),
            Choice::PrintMode => map(&PrintMode::ALL, PrintMode::label),
            Choice::Transport => map(&transport::Kind::ALL, transport::Kind::label),
        }
    }
}

// ---------------------------------------------------------------------------
// The view
// ---------------------------------------------------------------------------

struct Drag {
    index: usize,
    grab_col: f32,
    grab_row: f32,
}

pub struct Ticketsmith {
    doc: Doc,
    target: Target,
    show_grid: bool,
    show_source: bool,
    status_ok: bool,
    status: SharedString,

    /// Where the preview canvas ended up last frame, captured during prepaint so
    /// mouse handlers can turn window points into printer dots.
    preview_bounds: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<Drag>,

    fields: Vec<Entity<InputState>>,
    choices: Vec<Entity<SelectState<Vec<SharedString>>>>,

    /// Set while pushing the model into the widgets, so the change events that
    /// causes do not immediately push back.
    syncing: bool,

    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl Ticketsmith {
    pub fn view(window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self::new(window, cx))
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let doc = ticket::starter();

        let mut fields = Vec::new();
        let mut subscriptions = Vec::new();
        for field in Field::ALL {
            let state = cx.new(|cx| {
                let mut state = InputState::new(window, cx);
                if let Some((min, max)) = field.range() {
                    state = state
                        .min(min as f64)
                        .max(max as f64)
                        .step(NumberStep::Fixed(field.step() as f64));
                }
                state
            });
            subscriptions.push(cx.subscribe_in(
                &state,
                window,
                move |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.field_changed(field, window, cx);
                    }
                },
            ));
            fields.push(state);
        }

        let mut choices = Vec::new();
        for choice in Choice::ALL {
            let state = cx.new(|cx| {
                SelectState::new(choice.options(), Some(IndexPath::default()), window, cx)
            });
            subscriptions.push(cx.subscribe_in(
                &state,
                window,
                move |this, _, event: &SelectEvent<Vec<SharedString>>, window, cx| {
                    if matches!(event, SelectEvent::Confirm(_)) {
                        this.choice_changed(choice, window, cx);
                    }
                },
            ));
            choices.push(state);
        }

        let mut this = Self {
            doc,
            target: Target::default(),
            show_grid: false,
            show_source: false,
            status_ok: true,
            status: "".into(),
            preview_bounds: Rc::new(Cell::new(Bounds::default())),
            drag: None,
            fields,
            choices,
            syncing: false,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };

        this.set_status(
            true,
            format!(
                "{} elements on a {:.2}\" x {:.2}\" ticket.",
                this.doc.elements.len(),
                this.doc.width_in,
                this.doc.length_in
            ),
        );
        this.sync_widgets(window, cx);
        this
    }

    fn set_status(&mut self, ok: bool, message: impl Into<SharedString>) {
        self.status_ok = ok;
        self.status = message.into();
    }

    fn field(&self, field: Field) -> &Entity<InputState> {
        &self.fields[field.index()]
    }

    fn choice(&self, choice: Choice) -> &Entity<SelectState<Vec<SharedString>>> {
        &self.choices[choice.index()]
    }

    // -----------------------------------------------------------------------
    // Model -> widgets
    // -----------------------------------------------------------------------

    fn sync_widgets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.syncing = true;

        let doc = self.doc.clone();
        let target = self.target.clone();
        let element = doc.selected_element().cloned().unwrap_or_default();

        let values: Vec<(Field, String)> = vec![
            (Field::LengthIn, format!("{:.3}", doc.length_in)),
            (Field::WidthIn, format!("{:.3}", doc.width_in)),
            (Field::Name, element.name.clone()),
            (
                Field::Content,
                if element.kind == Kind::Barcode {
                    element.data.clone()
                } else {
                    element.text.clone()
                },
            ),
            (Field::Row, element.row.to_string()),
            (Field::Col, element.col.to_string()),
            (Field::Span, element.span.to_string()),
            (Field::SpanAcross, element.span_across.to_string()),
            (Field::Thickness, element.thickness.to_string()),
            (Field::WidthMult, element.style.width_mult.to_string()),
            (Field::HeightMult, element.style.height_mult.to_string()),
            (Field::ScaleDown, element.style.scale_down.to_string()),
            (
                Field::CenterField,
                element.style.center_field.max(1).to_string(),
            ),
            (Field::BarHeight, element.barcode.height_units.to_string()),
            (Field::BarExpansion, element.barcode.expansion.to_string()),
            (Field::QrPoint, element.barcode.qr_point.to_string()),
            (Field::Host, target.host.clone()),
            (Field::Port, target.port.to_string()),
            (Field::Device, target.device.clone()),
            (Field::Queue, target.queue.clone()),
            (Field::Path, target.path.clone()),
            (Field::Qty, doc.qty.to_string()),
            (Field::StartNumber, doc.start_number.to_string()),
        ];

        for (field, value) in values {
            let state = self.field(field).clone();
            state.update(cx, |state, cx| {
                if state.value() != value.as_str() {
                    state.set_value(value, window, cx);
                }
            });
        }

        let selections: Vec<(Choice, usize)> = vec![
            (Choice::Density, index_of(&Density::ALL, doc.density)),
            (Choice::Kind, index_of(&Kind::ALL, element.kind)),
            (Choice::Font, index_of(&Font::ALL, element.style.font)),
            (
                Choice::Rotation,
                index_of(&Rotation::ALL, element.style.rotation),
            ),
            (
                Choice::Symbology,
                index_of(&Symbology::ALL, element.barcode.symbology),
            ),
            (
                Choice::Orientation,
                index_of(&Orientation::ALL, element.barcode.orientation),
            ),
            (Choice::QrEcc, index_of(&QrEcc::ALL, element.barcode.qr_ecc)),
            (
                Choice::QrVersion,
                index_of(&QrVersion::ALL, element.barcode.qr_version),
            ),
            (Choice::PrintMode, index_of(&PrintMode::ALL, doc.print_mode)),
            (
                Choice::Transport,
                index_of(&transport::Kind::ALL, target.kind),
            ),
        ];

        for (choice, index) in selections {
            let state = self.choice(choice).clone();
            state.update(cx, |state, cx| {
                state.set_selected_index(Some(IndexPath::default().row(index)), window, cx);
            });
        }

        self.syncing = false;
    }

    // -----------------------------------------------------------------------
    // Widgets -> model
    // -----------------------------------------------------------------------

    fn field_changed(&mut self, field: Field, window: &mut Window, cx: &mut Context<Self>) {
        if self.syncing {
            return;
        }
        let raw = self.field(field).read(cx).value().to_string();
        let number = raw.trim().parse::<f32>().ok();

        // Clamp anything numeric to the range the field advertises, so a typed
        // value cannot put the document somewhere the stepper could not.
        let clamped = number.map(|v| match field.range() {
            Some((min, max)) => v.clamp(min, max),
            None => v,
        });

        match field {
            Field::LengthIn => {
                if let Some(v) = clamped {
                    self.doc.length_in = v;
                }
            }
            Field::WidthIn => {
                if let Some(v) = clamped {
                    self.doc.width_in = v;
                }
            }
            Field::Host => self.target.host = raw,
            Field::Device => self.target.device = raw,
            Field::Queue => self.target.queue = raw,
            Field::Path => self.target.path = raw,
            Field::Port => {
                if let Some(v) = clamped {
                    self.target.port = v as u16;
                }
            }
            Field::Qty => {
                if let Some(v) = clamped {
                    self.doc.qty = v as u32;
                }
            }
            Field::StartNumber => {
                if let Some(v) = clamped {
                    self.doc.start_number = v as u32;
                }
            }
            _ => {
                let Some(element) = self.doc.selected_element_mut() else {
                    return;
                };
                match field {
                    Field::Name => element.name = raw,
                    Field::Content => {
                        if element.kind == Kind::Barcode {
                            element.data = raw;
                        } else {
                            element.text = raw;
                        }
                    }
                    Field::Row => {
                        if let Some(v) = clamped {
                            element.row = v as i32;
                        }
                    }
                    Field::Col => {
                        if let Some(v) = clamped {
                            element.col = v as i32;
                        }
                    }
                    Field::Span => {
                        if let Some(v) = clamped {
                            element.span = v as u16;
                        }
                    }
                    Field::SpanAcross => {
                        if let Some(v) = clamped {
                            element.span_across = v as u16;
                        }
                    }
                    Field::Thickness => {
                        if let Some(v) = clamped {
                            element.thickness = v as u8;
                        }
                    }
                    Field::WidthMult => {
                        if let Some(v) = clamped {
                            element.style.width_mult = v as u8;
                        }
                    }
                    Field::HeightMult => {
                        if let Some(v) = clamped {
                            element.style.height_mult = v as u8;
                        }
                    }
                    Field::ScaleDown => {
                        if let Some(v) = clamped {
                            element.style.scale_down = v as u8;
                        }
                    }
                    Field::CenterField => {
                        if let Some(v) = clamped
                            && element.style.center_field > 0
                        {
                            element.style.center_field = v as u16;
                        }
                    }
                    Field::BarHeight => {
                        if let Some(v) = clamped {
                            element.barcode.height_units = v as u8;
                        }
                    }
                    Field::BarExpansion => {
                        if let Some(v) = clamped {
                            element.barcode.expansion = v as u8;
                        }
                    }
                    Field::QrPoint => {
                        if let Some(v) = clamped {
                            element.barcode.qr_point = v as u8;
                        }
                    }
                    _ => unreachable!("handled above"),
                }
            }
        }

        let _ = window;
        cx.notify();
    }

    fn choice_changed(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        if self.syncing {
            return;
        }
        let Some(row) = self
            .choice(choice)
            .read(cx)
            .selected_index(cx)
            .map(|path| path.row)
        else {
            return;
        };

        match choice {
            Choice::Density => self.doc.density = pick(&Density::ALL, row),
            Choice::PrintMode => self.doc.print_mode = pick(&PrintMode::ALL, row),
            Choice::Transport => self.target.kind = pick(&transport::Kind::ALL, row),
            _ => {
                let Some(element) = self.doc.selected_element_mut() else {
                    return;
                };
                match choice {
                    Choice::Kind => element.kind = pick(&Kind::ALL, row),
                    Choice::Font => element.style.font = pick(&Font::ALL, row),
                    Choice::Rotation => element.style.rotation = pick(&Rotation::ALL, row),
                    Choice::Symbology => element.barcode.symbology = pick(&Symbology::ALL, row),
                    Choice::Orientation => {
                        element.barcode.orientation = pick(&Orientation::ALL, row)
                    }
                    Choice::QrEcc => element.barcode.qr_ecc = pick(&QrEcc::ALL, row),
                    Choice::QrVersion => element.barcode.qr_version = pick(&QrVersion::ALL, row),
                    _ => unreachable!("handled above"),
                }
            }
        }

        // Switching kinds swaps which field the content box edits.
        if choice == Choice::Kind {
            self.sync_widgets(window, cx);
        }
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // Commands
    // -----------------------------------------------------------------------

    fn select(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if self.doc.selected == index {
            return;
        }
        self.doc.selected = index;
        self.sync_widgets(window, cx);
        cx.notify();
    }

    fn add_element(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        // Drop new elements near the middle so they are never off-screen.
        let row = (self.doc.width_dots() / 2) as i32;
        let col = (self.doc.length_dots() / 4) as i32;

        let mut element = Element {
            kind,
            row,
            col,
            ..Default::default()
        };
        match kind {
            Kind::Text => element.text = "NEW TEXT".into(),
            Kind::Barcode => {
                element.data = "TKT-00001".into();
                element.barcode.height_units = 6;
            }
            Kind::HLine | Kind::VLine => {
                element.span = 300;
            }
            Kind::Box => {
                element.span = 300;
                element.span_across = 100;
            }
            Kind::Counter => {}
        }

        if self.doc.add(element) {
            self.set_status(true, format!("Added a {} element.", kind.short_label()));
        } else {
            self.set_status(
                false,
                format!("Element limit reached ({}).", ticket::MAX_ELEMENTS),
            );
        }
        self.sync_widgets(window, cx);
        cx.notify();
    }

    fn nudge(&mut self, dcol: i32, drow: i32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(element) = self.doc.selected_element_mut() else {
            return;
        };
        element.col += dcol;
        element.row += drow;
        self.sync_widgets(window, cx);
        cx.notify();
    }

    fn print(&mut self, cx: &mut Context<Self>) {
        if self.doc.elements.is_empty() {
            self.set_status(false, "Nothing to print — the ticket is empty.");
            cx.notify();
            return;
        }

        let payload = self.doc.emit();
        let target = self.target.clone();
        let verb = if target.kind == transport::Kind::File {
            "Saving"
        } else {
            "Sending"
        };
        self.set_status(true, format!("{verb} to {}…", target.describe()));
        cx.notify();

        // Sending blocks: a TCP connect to a printer that is switched off waits
        // out its timeout. Do it off the UI thread and post the result back.
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move { transport::send(&target, payload.as_bytes()) })
                .await;
            this.update(cx, |this, cx| {
                this.set_status(outcome.ok, outcome.message);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // -----------------------------------------------------------------------
    // Preview interaction
    // -----------------------------------------------------------------------

    fn layout(&self) -> Layout {
        Layout::fit(&self.doc, self.preview_bounds.get())
    }

    fn on_preview_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let layout = self.layout();
        let (col, row) = layout.to_dots(event.position);

        match self.doc.hit(col, row) {
            Some(index) => {
                let element = &self.doc.elements[index];
                self.drag = Some(Drag {
                    index,
                    grab_col: col - element.col as f32,
                    grab_row: row - element.row as f32,
                });
                self.select(Some(index), window, cx);
            }
            None => {
                self.drag = None;
                self.select(None, window, cx);
            }
        }
        cx.notify();
    }

    fn on_preview_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.drag.as_ref() else {
            return;
        };
        // The pointer can be released outside the pane, where no mouse-up
        // reaches us; the next move without a held button ends the drag.
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }

        let index = drag.index;
        let (grab_col, grab_row) = (drag.grab_col, drag.grab_row);
        let layout = self.layout();
        let (col, row) = layout.to_dots(event.position);

        if let Some(element) = self.doc.elements.get_mut(index) {
            element.col = (col - grab_col).round() as i32;
            element.row = (row - grab_row).round() as i32;
        }
        self.sync_widgets(window, cx);
        cx.notify();
    }

    fn on_preview_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            cx.notify();
        }
    }
}

fn index_of<T: PartialEq + Copy>(all: &[T], value: T) -> usize {
    all.iter().position(|t| *t == value).unwrap_or(0)
}

fn pick<T: Copy>(all: &[T], row: usize) -> T {
    all[row.min(all.len() - 1)]
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

impl Focusable for Ticketsmith {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Ticketsmith {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("Ticketsmith")
            .on_action(cx.listener(|this, _: &NudgeLeft, w, cx| this.nudge(-1, 0, w, cx)))
            .on_action(cx.listener(|this, _: &NudgeRight, w, cx| this.nudge(1, 0, w, cx)))
            .on_action(cx.listener(|this, _: &NudgeUp, w, cx| this.nudge(0, -1, w, cx)))
            .on_action(cx.listener(|this, _: &NudgeDown, w, cx| this.nudge(0, 1, w, cx)))
            .on_action(cx.listener(|this, _: &NudgeLeftFast, w, cx| this.nudge(-10, 0, w, cx)))
            .on_action(cx.listener(|this, _: &NudgeRightFast, w, cx| this.nudge(10, 0, w, cx)))
            .on_action(cx.listener(|this, _: &NudgeUpFast, w, cx| this.nudge(0, -10, w, cx)))
            .on_action(cx.listener(|this, _: &NudgeDownFast, w, cx| this.nudge(0, 10, w, cx)))
            .on_action(cx.listener(|this, _: &DeleteSelected, w, cx| {
                if let Some(index) = this.doc.selected {
                    this.doc.remove(index);
                    this.sync_widgets(w, cx);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &Deselect, w, cx| this.select(None, w, cx)))
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_toolbar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_preview(window, cx))
                    .child(self.render_sidebar(window, cx)),
            )
            .child(self.render_status(cx))
    }
}

impl Ticketsmith {
    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let problems = self.doc.problem_count();

        h_flex()
            .w_full()
            .flex_shrink_0()
            .items_center()
            .gap_3()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().title_bar)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Ticketsmith"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Boca FGL"),
            )
            .child(Separator::vertical().h(px(18.)))
            .child(labelled("width", 46., NumberInput::new(self.field(Field::WidthIn)).small().suffix(unit_suffix("in"))))
            .child(labelled("length", 46., NumberInput::new(self.field(Field::LengthIn)).small().suffix(unit_suffix("in"))))
            .child(div().w(px(112.)).child(Select::new(self.choice(Choice::Density)).small()))
            .child(
                Button::new("standard-stock")
                    .ghost()
                    .small()
                    .label("2 x 5.5")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.doc.width_in = 2.0;
                        this.doc.length_in = 5.5;
                        this.set_status(true, "Stock reset to the standard 2\" x 5.5\" ticket.");
                        this.sync_widgets(window, cx);
                        cx.notify();
                    })),
            )
            .child(Separator::vertical().h(px(18.)))
            .child(
                Checkbox::new("grid")
                    .label("grid")
                    .checked(self.show_grid)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.show_grid = *checked;
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .when(problems > 0, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(format!(
                            "{problems} element{} need attention",
                            if problems == 1 { "" } else { "s" }
                        )),
                )
            })
            .child(
                Button::new("toggle-source")
                    .ghost()
                    .small()
                    .label(if self.show_source {
                        "Hide FGL"
                    } else {
                        "Show FGL"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_source = !this.show_source;
                        cx.notify();
                    })),
            )
    }

    fn render_preview(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let doc = self.doc.clone();
        let bounds_slot = self.preview_bounds.clone();
        let show_grid = self.show_grid;
        let mono = cx.theme().mono_font_family.clone();

        v_flex()
            .flex_1()
            .min_w_0()
            // h_flex centres its children on the cross axis, so without this the
            // pane would collapse to the height of its content — which, for a
            // canvas, is nothing.
            .h_full()
            .p_4()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .rounded_lg()
                    .bg(cx.theme().muted)
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_preview_down))
                    .on_mouse_move(cx.listener(Self::on_preview_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_preview_up))
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                bounds_slot.set(bounds);
                                bounds
                            },
                            move |_, bounds, window, cx| {
                                let layout = Layout::fit(&doc, bounds);
                                let opts = PaintOptions {
                                    show_grid,
                                    mono_font: mono,
                                    palette: Palette::default(),
                                };
                                preview::paint(&doc, &layout, &opts, window, cx);
                            },
                        )
                        .size_full(),
                    ),
            )
            .when(self.show_source, |this| this.child(self.render_source(cx)))
    }

    fn render_source(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let source = self.doc.emit();
        v_flex()
            .flex_shrink_0()
            .h(px(150.))
            .p_3()
            .gap_2()
            .rounded_lg()
            .bg(cx.theme().popover)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("FGL for one job — {} bytes", source.len())),
            )
            .child(
                div()
                    .id("fgl-source")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_xs()
                    .child(source),
            )
    }

    fn render_status(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let dots = format!(
            "{} x {} dots  ({})",
            self.doc.length_dots(),
            self.doc.width_dots(),
            self.doc.density.label()
        );
        h_flex()
            .w_full()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .px_4()
            .py_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .text_xs()
            .child(
                div()
                    .text_color(if self.status_ok {
                        cx.theme().muted_foreground
                    } else {
                        cx.theme().danger
                    })
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(dots),
            )
    }
}

fn labelled(label: &'static str, width: f32, control: impl IntoElement) -> impl IntoElement {
    h_flex()
        .gap_1p5()
        .items_center()
        .child(div().text_xs().opacity(0.7).child(label))
        .child(div().w(px(width + 56.)).child(control))
}

fn unit_suffix(unit: &'static str) -> impl IntoElement {
    div().pr_2().text_xs().opacity(0.6).child(unit)
}

// ---------------------------------------------------------------------------
// Sidebar
// ---------------------------------------------------------------------------

impl Ticketsmith {
    fn render_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("sidebar")
            .w(px(380.))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(self.render_elements(cx))
            .child(self.render_inspector(window, cx))
            .child(self.render_output(cx))
            .child(self.render_print(cx))
    }

    fn render_elements(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<_> = self
            .doc
            .elements
            .iter()
            .enumerate()
            .map(|(index, element)| {
                let selected = self.doc.selected == Some(index);
                let problem = element.problem(&self.doc);
                let name = element.display_name().to_string();
                let visible = element.visible;

                h_flex()
                    .id(("element", index))
                    .w_full()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .when(selected, |this| this.bg(cx.theme().accent))
                    .child(
                        Checkbox::new(("visible", index))
                            .checked(visible)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                if let Some(element) = this.doc.elements.get_mut(index) {
                                    element.visible = *checked;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id(("name", index))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .cursor_pointer()
                            .when(problem.is_some(), |this| {
                                this.text_color(cx.theme().danger)
                            })
                            .child(name)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select(Some(index), window, cx)
                            })),
                    )
                    .child(
                        Button::new(("up", index))
                            .ghost()
                            .xsmall()
                            .label("↑")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.doc.reorder(index, -1);
                                this.sync_widgets(window, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(("down", index))
                            .ghost()
                            .xsmall()
                            .label("↓")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.doc.reorder(index, 1);
                                this.sync_widgets(window, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(("dup", index))
                            .ghost()
                            .xsmall()
                            .label("copy")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.doc.duplicate(index);
                                this.sync_widgets(window, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(("del", index))
                            .ghost()
                            .xsmall()
                            .label("×")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.doc.remove(index);
                                this.sync_widgets(window, cx);
                                cx.notify();
                            })),
                    )
            })
            .collect();

        let add_buttons: Vec<_> = Kind::ALL
            .iter()
            .map(|kind| {
                let kind = *kind;
                Button::new(("add", kind as usize))
                    .outline()
                    .xsmall()
                    .label(kind.short_label())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.add_element(kind, window, cx)
                    }))
            })
            .collect();

        section(
            "Elements",
            cx,
            v_flex()
                .gap_2()
                .child(h_flex().gap_1().flex_wrap().children(add_buttons))
                .when(rows.is_empty(), |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Nothing on the ticket yet."),
                    )
                })
                .child(v_flex().gap_0p5().children(rows)),
        )
    }

    fn render_inspector(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(element) = self.doc.selected_element().cloned() else {
            return section(
                "Inspector",
                cx,
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Select an element to edit it."),
            );
        };

        let mut body = v_flex()
            .gap_2()
            .child(field_row(
                "name",
                Input::new(self.field(Field::Name)).small(),
            ))
            .child(field_row(
                "kind",
                Select::new(self.choice(Choice::Kind)).small(),
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(field_row("column", NumberInput::new(self.field(Field::Col)).small())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(field_row("row", NumberInput::new(self.field(Field::Row)).small())),
                    ),
            );

        match element.kind {
            Kind::Text => {
                body = body
                    .child(field_row(
                        "text",
                        Input::new(self.field(Field::Content)).small(),
                    ))
                    .child(field_row(
                        "font",
                        Select::new(self.choice(Choice::Font)).small(),
                    ))
                    .child(field_row(
                        "rotation",
                        Select::new(self.choice(Choice::Rotation)).small(),
                    ))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(field_row(
                                "wide x",
                                NumberInput::new(self.field(Field::WidthMult)).small(),
                            )))
                            .child(div().flex_1().child(field_row(
                                "tall x",
                                NumberInput::new(self.field(Field::HeightMult)).small(),
                            )))
                            .child(div().flex_1().child(field_row(
                                "scale 1/",
                                NumberInput::new(self.field(Field::ScaleDown)).small(),
                            ))),
                    )
                    .child(
                        Checkbox::new("invert")
                            .label("invert (white on black)")
                            .checked(element.style.invert)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                if let Some(e) = this.doc.selected_element_mut() {
                                    e.style.invert = *checked;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Checkbox::new("centre")
                            .label("centre in a field")
                            .checked(element.style.center_field > 0)
                            .on_click(cx.listener(|this, checked: &bool, window, cx| {
                                if let Some(e) = this.doc.selected_element_mut() {
                                    // 4000 dots is wider than any stock, which is
                                    // the documented way to ask the printer to
                                    // centre on the ticket itself.
                                    e.style.center_field = if *checked { 4000 } else { 0 };
                                }
                                this.sync_widgets(window, cx);
                                cx.notify();
                            })),
                    )
                    .when(element.style.center_field > 0, |this| {
                        this.child(field_row(
                            "field width",
                            NumberInput::new(self.field(Field::CenterField)).small(),
                        ))
                    })
                    .child(measurement(
                        &self.doc,
                        element.style.width_dots(&element.text),
                        element.style.height_dots(),
                        cx,
                    ));
            }
            Kind::Counter => {
                body = body
                    .child(hint(
                        "Prints the printer's own seven digit counter. Height and width multipliers do not apply to <PC>.",
                        cx,
                    ))
                    .child(field_row(
                        "font",
                        Select::new(self.choice(Choice::Font)).small(),
                    ))
                    .child(field_row(
                        "rotation",
                        Select::new(self.choice(Choice::Rotation)).small(),
                    ));
            }
            Kind::Barcode => {
                let settings = element.barcode;
                let validation = barcode::validate(settings.symbology, &element.data);
                let footprint = barcode::footprint(&element.data, &settings);

                body = body
                    .child(field_row(
                        "data",
                        Input::new(self.field(Field::Content)).small(),
                    ))
                    .child(field_row(
                        "symbology",
                        Select::new(self.choice(Choice::Symbology)).small(),
                    ))
                    .child(hint(
                        &format!("accepts {}", settings.symbology.hint()),
                        cx,
                    ))
                    .when_some(validation.err(), |this, message| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().danger)
                                .child(message.to_string()),
                        )
                    });

                if settings.symbology == Symbology::Qr {
                    body = body
                        .child(field_row(
                            "version",
                            Select::new(self.choice(Choice::QrVersion)).small(),
                        ))
                        .child(field_row(
                            "correction",
                            Select::new(self.choice(Choice::QrEcc)).small(),
                        ))
                        .child(field_row(
                            "point size",
                            NumberInput::new(self.field(Field::QrPoint)).small(),
                        ));
                } else {
                    body = body
                        .child(field_row(
                            "orientation",
                            Select::new(self.choice(Choice::Orientation)).small(),
                        ))
                        .child(
                            h_flex()
                                .gap_2()
                                .when(settings.symbology.supports_height_units(), |this| {
                                    this.child(div().flex_1().child(field_row(
                                        "bar units",
                                        NumberInput::new(self.field(Field::BarHeight)).small(),
                                    )))
                                })
                                .child(div().flex_1().child(field_row(
                                    "narrow bar",
                                    NumberInput::new(self.field(Field::BarExpansion)).small(),
                                ))),
                        )
                        .when(settings.symbology.supports_interpretation(), |this| {
                            this.child(
                                Checkbox::new("interpretation")
                                    .label("print human readable text")
                                    .checked(settings.interpretation)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        if let Some(e) = this.doc.selected_element_mut() {
                                            e.barcode.interpretation = *checked;
                                        }
                                        cx.notify();
                                    })),
                            )
                        })
                        .when(settings.symbology.supports_wide_ratio(), |this| {
                            this.child(
                                Checkbox::new("wide-ratio")
                                    .label("3:1 wide-to-narrow ratio")
                                    .checked(settings.wide_ratio)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        if let Some(e) = this.doc.selected_element_mut() {
                                            e.barcode.wide_ratio = *checked;
                                        }
                                        cx.notify();
                                    })),
                            )
                        });
                }

                body = body
                    .child(
                        Checkbox::new("flipped")
                            .label("reverse direction")
                            .checked(settings.flipped)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                if let Some(e) = this.doc.selected_element_mut() {
                                    e.barcode.flipped = *checked;
                                }
                                cx.notify();
                            })),
                    )
                    .child(measurement(
                        &self.doc,
                        footprint.width_dots,
                        footprint.height_dots,
                        cx,
                    ))
                    .when(!settings.symbology.preview_is_exact(), |this| {
                        this.child(hint(
                            "Preview shows the exact footprint; the bar pattern is representative. The printer encodes the real symbol.",
                            cx,
                        ))
                    });
            }
            Kind::HLine | Kind::VLine => {
                body = body
                    .child(field_row(
                        "length",
                        NumberInput::new(self.field(Field::Span)).small(),
                    ))
                    .child(field_row(
                        "thickness",
                        NumberInput::new(self.field(Field::Thickness)).small(),
                    ));
            }
            Kind::Box => {
                body = body
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(field_row(
                                "width",
                                NumberInput::new(self.field(Field::Span)).small(),
                            )))
                            .child(div().flex_1().child(field_row(
                                "height",
                                NumberInput::new(self.field(Field::SpanAcross)).small(),
                            ))),
                    )
                    .child(field_row(
                        "thickness",
                        NumberInput::new(self.field(Field::Thickness)).small(),
                    ));
            }
        }

        if self.doc.overflows(&element) {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child("This element falls outside the printable area."),
            );
        }

        section("Inspector", cx, body)
    }

    fn render_output(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = self.target.kind;
        let mut body = v_flex()
            .gap_2()
            .child(Select::new(self.choice(Choice::Transport)).small())
            .child(hint(kind.blurb(), cx));

        body = match kind {
            transport::Kind::Ethernet => body
                .child(field_row(
                    "printer address",
                    Input::new(self.field(Field::Host)).small(),
                ))
                .child(field_row(
                    "port",
                    NumberInput::new(self.field(Field::Port)).small(),
                )),
            transport::Kind::Device => body.child(field_row(
                "device path",
                Input::new(self.field(Field::Device)).small(),
            )),
            transport::Kind::Spooler => body
                .child(field_row(
                    "queue name",
                    Input::new(self.field(Field::Queue)).small(),
                ))
                .when(!kind.supported_here(), |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child("Not available on this platform — use a device path."),
                    )
                }),
            transport::Kind::File => body.child(field_row(
                "file path",
                Input::new(self.field(Field::Path)).small(),
            )),
        };

        section("Where it goes", cx, body)
    }

    fn render_print(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let bytes = self.doc.emit().len();
        let saving = self.target.kind == transport::Kind::File;
        let label = format!(
            "{}  {} ×  →  {}",
            if saving { "SAVE" } else { "PRINT" },
            self.doc.qty,
            self.target.describe()
        );

        let body = v_flex()
            .gap_2()
            .child(field_row(
                "quantity",
                NumberInput::new(self.field(Field::Qty)).small(),
            ))
            .child(field_row(
                "on finish",
                Select::new(self.choice(Choice::PrintMode)).small(),
            ))
            .child(
                Checkbox::new("numbering")
                    .label("number the run")
                    .checked(self.doc.numbering)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.doc.numbering = *checked;
                        cx.notify();
                    })),
            )
            .when(self.doc.numbering, |this| {
                this.child(field_row(
                    "starting at",
                    NumberInput::new(self.field(Field::StartNumber)).small(),
                ))
                .child(hint(
                    "The printer increments its counter per ticket. Add a Number element to show it.",
                    cx,
                ))
            })
            .child(
                Checkbox::new("limit-length")
                    .label("cap printed length to the stock size")
                    .checked(self.doc.limit_print_length)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.doc.limit_print_length = *checked;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("print")
                    .primary()
                    .w_full()
                    .label(label)
                    .disabled(self.doc.elements.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.print(cx))),
            )
            .child(hint(&format!("{bytes} bytes per job"), cx));

        section("Print", cx, body)
    }
}

// ---------------------------------------------------------------------------
// Small building blocks
// ---------------------------------------------------------------------------

fn section(
    title: &'static str,
    cx: &mut Context<Ticketsmith>,
    body: impl IntoElement,
) -> AnyElement {
    v_flex()
        .w_full()
        .gap_2()
        .px_3()
        .py_3()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cx.theme().muted_foreground)
                .child(title),
        )
        .child(body)
        .into_any_element()
}

fn field_row(label: &'static str, control: impl IntoElement) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_xs().opacity(0.65).child(label))
        .child(control)
}

fn hint(text: &str, cx: &mut Context<Ticketsmith>) -> AnyElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
        .into_any_element()
}

fn measurement(
    doc: &Doc,
    width_dots: u32,
    height_dots: u32,
    cx: &mut Context<Ticketsmith>,
) -> AnyElement {
    let dpi = doc.density.dots() as f32;
    hint(
        &format!(
            "{} x {} dots   ({:.2}\" x {:.2}\")",
            width_dots,
            height_dots,
            width_dots as f32 / dpi,
            height_dots as f32 / dpi
        ),
        cx,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;

    // `use super::*` re-exports gpui_kit's `test` attribute through the glob at
    // the top of this file, which would make the `#[test]` that
    // `#[gpui_kit::test]` generates expand into itself. Naming the built-in
    // explicitly takes precedence over the glob.
    use core::prelude::v1::test;

    /// A window with the component library initialised, which the theme and
    /// input widgets need before they can be constructed.
    fn open(cx: &mut TestAppContext) -> gpui_kit::WindowHandle<Ticketsmith> {
        cx.update(|cx| gpui_kit::init(cx));
        cx.add_window(|window, cx| Ticketsmith::new(window, cx))
    }

    /// A preview pane big enough that the ticket lands at exactly 1:1, which
    /// makes every assertion below readable in printer dots.
    fn unit_scale_bounds(doc: &Doc) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(0.), px(0.)),
            size: size(
                px(doc.length_dots() as f32 + 56.),
                px(doc.width_dots() as f32 + 56.),
            ),
        }
    }

    fn mouse_down(position: Point<Pixels>) -> MouseDownEvent {
        MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }
    }

    fn mouse_move(position: Point<Pixels>, held: Option<MouseButton>) -> MouseMoveEvent {
        MouseMoveEvent {
            position,
            pressed_button: held,
            modifiers: Modifiers::default(),
        }
    }

    /// Centre of an element, in window coordinates.
    fn centre_of(this: &Ticketsmith, index: usize) -> Point<Pixels> {
        let layout = this.layout();
        let b = this.doc.elements[index].bounds();
        layout.to_px((b.col0 + b.col1) / 2.0, (b.row0 + b.row1) / 2.0)
    }

    fn index_named(this: &Ticketsmith, name: &str) -> usize {
        this.doc
            .elements
            .iter()
            .position(|e| e.name == name)
            .expect("element not in the starter ticket")
    }

    #[gpui_kit::test]
    fn the_starter_ticket_loads_with_a_selection(cx: &mut TestAppContext) {
        let handle = open(cx);
        handle
            .update(cx, |this, _, _| {
                assert_eq!(this.doc.elements.len(), 12);
                assert!(this.doc.selected.is_some());
                assert_eq!(this.doc.problem_count(), 0);
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn clicking_an_element_selects_it(cx: &mut TestAppContext) {
        let handle = open(cx);
        handle
            .update(cx, |this, window, cx| {
                this.preview_bounds.set(unit_scale_bounds(&this.doc));
                let price = index_named(this, "price");
                assert_ne!(this.doc.selected, Some(price));

                let at = centre_of(this, price);
                this.on_preview_down(&mouse_down(at), window, cx);

                assert_eq!(this.doc.selected, Some(price));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn clicking_bare_stock_clears_the_selection(cx: &mut TestAppContext) {
        let handle = open(cx);
        handle
            .update(cx, |this, window, cx| {
                this.preview_bounds.set(unit_scale_bounds(&this.doc));
                assert!(this.doc.selected.is_some());

                // Inside the border but away from every other element. The
                // border itself is a frame, so this lands on nothing.
                let empty = this.layout().to_px(700.0, 330.0);
                this.on_preview_down(&mouse_down(empty), window, cx);

                assert_eq!(this.doc.selected, None);
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn dragging_moves_the_element_by_the_distance_dragged(cx: &mut TestAppContext) {
        let handle = open(cx);
        handle
            .update(cx, |this, window, cx| {
                this.preview_bounds.set(unit_scale_bounds(&this.doc));
                let price = index_named(this, "price");
                let (col0, row0) = (
                    this.doc.elements[price].col,
                    this.doc.elements[price].row,
                );

                let grab = centre_of(this, price);
                this.on_preview_down(&mouse_down(grab), window, cx);

                let to = point(grab.x + px(120.), grab.y + px(40.));
                this.on_preview_move(&mouse_move(to, Some(MouseButton::Left)), window, cx);

                assert_eq!(this.doc.elements[price].col, col0 + 120);
                assert_eq!(this.doc.elements[price].row, row0 + 40);

                this.on_preview_up(
                    &MouseUpEvent {
                        button: MouseButton::Left,
                        position: to,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                    },
                    window,
                    cx,
                );
                assert!(this.drag.is_none());
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn releasing_outside_the_pane_ends_the_drag(cx: &mut TestAppContext) {
        let handle = open(cx);
        handle
            .update(cx, |this, window, cx| {
                this.preview_bounds.set(unit_scale_bounds(&this.doc));
                let price = index_named(this, "price");
                let grab = centre_of(this, price);
                this.on_preview_down(&mouse_down(grab), window, cx);
                assert!(this.drag.is_some());

                // A move with no button held is how a release we never saw
                // reaches us.
                this.on_preview_move(&mouse_move(grab, None), window, cx);
                assert!(this.drag.is_none());
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn dragging_does_not_move_an_unselected_element(cx: &mut TestAppContext) {
        let handle = open(cx);
        handle
            .update(cx, |this, window, cx| {
                this.preview_bounds.set(unit_scale_bounds(&this.doc));
                let event = index_named(this, "event");
                let before = this.doc.elements[event].col;

                // No mouse-down first, so there is nothing to drag.
                let somewhere = this.layout().to_px(500.0, 200.0);
                this.on_preview_move(
                    &mouse_move(somewhere, Some(MouseButton::Left)),
                    window,
                    cx,
                );

                assert_eq!(this.doc.elements[event].col, before);
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn nudging_moves_the_selection_and_updates_the_fields(cx: &mut TestAppContext) {
        let handle = open(cx);
        handle
            .update(cx, |this, window, cx| {
                let index = this.doc.selected.unwrap();
                let before = this.doc.elements[index].col;

                this.nudge(1, 0, window, cx);
                assert_eq!(this.doc.elements[index].col, before + 1);

                this.nudge(10, 0, window, cx);
                assert_eq!(this.doc.elements[index].col, before + 11);

                // The inspector's column box follows the model.
                assert_eq!(
                    this.field(Field::Col).read(cx).value(),
                    (before + 11).to_string().as_str()
                );
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn editing_a_field_writes_through_to_the_document(cx: &mut TestAppContext) {
        let handle = open(cx);
        handle
            .update(cx, |this, window, cx| {
                let index = this.doc.selected.unwrap();
                let state = this.field(Field::Content).clone();
                state.update(cx, |state, cx| {
                    state.set_value("LATE SHOW", window, cx);
                });
                this.field_changed(Field::Content, window, cx);

                assert_eq!(this.doc.elements[index].text, "LATE SHOW");
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn a_numeric_field_is_clamped_to_its_range(cx: &mut TestAppContext) {
        let handle = open(cx);
        handle
            .update(cx, |this, window, cx| {
                let state = this.field(Field::BarExpansion).clone();
                state.update(cx, |state, cx| state.set_value("99", window, cx));
                this.field_changed(Field::BarExpansion, window, cx);

                // <X#> tops out at 9 dots per narrow bar.
                let index = this.doc.selected.unwrap();
                assert_eq!(this.doc.elements[index].barcode.expansion, 9);
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn selecting_a_different_element_repopulates_the_inspector(cx: &mut TestAppContext) {
        let handle = open(cx);
        handle
            .update(cx, |this, window, cx| {
                let barcode = index_named(this, "barcode");
                this.select(Some(barcode), window, cx);

                // A barcode element edits its data through the same content box.
                assert_eq!(this.field(Field::Content).read(cx).value(), "TKT-10024");
                assert_eq!(this.field(Field::Name).read(cx).value(), "barcode");
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn changing_a_dropdown_updates_the_model(cx: &mut TestAppContext) {
        let handle = open(cx);
        handle
            .update(cx, |this, window, cx| {
                let row = index_of(&Density::ALL, Density::Dpi300);
                let state = this.choice(Choice::Density).clone();
                state.update(cx, |state, cx| {
                    state.set_selected_index(Some(IndexPath::default().row(row)), window, cx);
                });
                this.choice_changed(Choice::Density, window, cx);

                assert_eq!(this.doc.density, Density::Dpi300);
                assert_eq!(this.doc.length_dots(), 1650);
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn adding_an_element_lands_it_on_the_stock_and_selects_it(cx: &mut TestAppContext) {
        let handle = open(cx);
        handle
            .update(cx, |this, window, cx| {
                let before = this.doc.elements.len();
                this.add_element(Kind::Barcode, window, cx);

                assert_eq!(this.doc.elements.len(), before + 1);
                assert_eq!(this.doc.selected, Some(this.doc.elements.len() - 1));

                let added = this.doc.elements.last().unwrap();
                assert_eq!(added.kind, Kind::Barcode);
                assert!(!this.doc.overflows(added));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn deleting_the_selection_keeps_the_inspector_consistent(cx: &mut TestAppContext) {
        let handle = open(cx);
        handle
            .update(cx, |this, window, cx| {
                let index = this.doc.selected.unwrap();
                let before = this.doc.elements.len();

                this.doc.remove(index);
                this.sync_widgets(window, cx);

                assert_eq!(this.doc.elements.len(), before - 1);
                let selected = this.doc.selected.unwrap();
                assert_eq!(
                    this.field(Field::Name).read(cx).value(),
                    this.doc.elements[selected].name.as_str()
                );
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn switching_kind_moves_the_content_box_to_the_other_field(cx: &mut TestAppContext) {
        let handle = open(cx);
        handle
            .update(cx, |this, window, cx| {
                let index = this.doc.selected.unwrap();
                this.doc.elements[index].data = "PAYLOAD".into();

                let row = index_of(&Kind::ALL, Kind::Barcode);
                let state = this.choice(Choice::Kind).clone();
                state.update(cx, |state, cx| {
                    state.set_selected_index(Some(IndexPath::default().row(row)), window, cx);
                });
                this.choice_changed(Choice::Kind, window, cx);

                assert_eq!(this.doc.elements[index].kind, Kind::Barcode);
                assert_eq!(this.field(Field::Content).read(cx).value(), "PAYLOAD");
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn syncing_the_widgets_does_not_write_back_to_the_document(cx: &mut TestAppContext) {
        let handle = open(cx);
        handle
            .update(cx, |this, window, cx| {
                // Select an element whose name is empty, then re-sync. Without
                // the guard the resulting Change events would clobber the
                // neighbouring element's fields.
                let rule = index_named(this, "rule");
                this.select(Some(rule), window, cx);
                let before = this.doc.clone();

                this.sync_widgets(window, cx);

                assert_eq!(this.doc.elements.len(), before.elements.len());
                assert_eq!(this.doc.elements[rule].span, before.elements[rule].span);
                assert_eq!(this.doc.selected, before.selected);
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn saving_writes_the_same_bytes_the_source_panel_shows(cx: &mut TestAppContext) {
        let dir = std::env::temp_dir().join(format!("ticketsmith-ui-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.fgl");

        let handle = open(cx);
        let expected = handle
            .update(cx, |this, _, _| {
                this.target.kind = transport::Kind::File;
                this.target.path = path.to_string_lossy().into_owned();
                this.doc.qty = 12;
                this.doc.emit()
            })
            .unwrap();

        // The send itself runs on the background executor.
        handle.update(cx, |this, _, cx| this.print(cx)).unwrap();
        cx.run_until_parked();

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, expected);
        assert!(written.ends_with("<RE11><p>"));
        assert!(written.contains("MIDNIGHT ELECTRIC"));

        handle
            .update(cx, |this, _, _| assert!(this.status_ok))
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Renders a real frame for every element kind, including the paths the
    /// happy-path ticket never touches: rotated text, inverted text, a QR
    /// symbol, and a barcode whose data cannot be encoded.
    #[gpui_kit::test]
    fn every_element_kind_paints_without_panicking(cx: &mut TestAppContext) {
        let handle = open(cx);
        handle
            .update(cx, |this, window, cx| {
                this.show_grid = true;
                this.show_source = true;
                this.doc.elements.clear();

                for rotation in Rotation::ALL {
                    this.doc.elements.push(Element {
                        kind: Kind::Text,
                        row: 200,
                        col: 200,
                        text: "STUB".into(),
                        style: crate::fgl::TextStyle {
                            rotation,
                            ..Default::default()
                        },
                        ..Default::default()
                    });
                }

                this.doc.elements.push(Element {
                    kind: Kind::Text,
                    row: 60,
                    col: 60,
                    text: "VIP".into(),
                    style: crate::fgl::TextStyle {
                        invert: true,
                        center_field: 400,
                        ..Default::default()
                    },
                    ..Default::default()
                });

                for symbology in Symbology::ALL {
                    this.doc.elements.push(Element {
                        kind: Kind::Barcode,
                        row: 100,
                        col: 400,
                        data: "12345678".into(),
                        barcode: crate::barcode::Settings {
                            symbology,
                            ..Default::default()
                        },
                        ..Default::default()
                    });
                }

                // Deliberately unencodable, so the warning outline is drawn.
                this.doc.elements.push(Element {
                    kind: Kind::Barcode,
                    row: 300,
                    col: 400,
                    data: "not for i2of5".into(),
                    barcode: crate::barcode::Settings {
                        symbology: Symbology::I2of5,
                        ..Default::default()
                    },
                    ..Default::default()
                });

                for kind in [Kind::Counter, Kind::HLine, Kind::VLine, Kind::Box] {
                    this.doc.elements.push(Element {
                        kind,
                        row: 40,
                        col: 40,
                        span: 200,
                        span_across: 80,
                        ..Default::default()
                    });
                }

                this.doc.selected = Some(0);
                this.sync_widgets(window, cx);
            })
            .unwrap();

        // Drawing is where a bad rect or a missing glyph would blow up.
        cx.update_window(handle.into(), |_, window, cx| {
            _ = window.draw(cx);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn printing_an_empty_ticket_reports_instead_of_sending(cx: &mut TestAppContext) {
        let handle = open(cx);
        handle
            .update(cx, |this, _, cx| {
                this.doc.elements.clear();
                this.doc.selected = None;
                this.print(cx);
                assert!(!this.status_ok);
            })
            .unwrap();
    }
}
