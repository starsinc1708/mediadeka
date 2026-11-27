//! `TextInput` — a stateful single-line text field (gpui entity).
//!
//! Owns its buffer and focus; renders the shared field chrome (label, box,
//! description/error) and emits [`TextInputEvent`] on edit and submit.
//!
//! The editing surface itself lives in [`line`](super::line), which is what
//! gives the field the behaviour an `<input>` has: click and drag to select,
//! double-click a word, Tab to the next field, the clipboard, undo, IME, and
//! horizontal scrolling for values wider than the box.

use gpui::prelude::*;
use gpui::{
  accesskit, div, hsla, px, AccessibleAction, App, Context, Entity, EventEmitter, FocusHandle,
  Global, Hsla, IntoElement, KeyDownEvent, Role, SharedString, Window,
};

use super::line::{self, Line, LineEditor, LineState};
use super::{control_metrics, edit::TextEdit, Field, KeyOutcome};
use crate::devtools::ProbedAny;
use crate::reactive::Signal;
use crate::theme::{theme, ColorName, Size};

/// Emitted as the user edits or submits the field.
#[derive(Debug, Clone)]
pub enum TextInputEvent {
  /// The text changed. Carries the full new value.
  Change(String),
  /// The user pressed Enter. Carries the current value.
  Submit(String),
}

/// Optional application chrome for standalone text fields.
#[derive(Clone, Copy)]
pub struct TextInputSurface {
  pub background: Hsla,
  pub focused_background: Hsla,
  pub border: Hsla,
  pub focused_border: Hsla,
  pub radius: f32,
}

impl Global for TextInputSurface {}

/// A single-line text field. Create with `cx.new(|cx| TextInput::new(cx))`.
pub struct TextInput {
  edit: TextEdit,
  state: LineState,
  focus: FocusHandle,
  id: &'static str,
  placeholder: SharedString,
  short_placeholder: Option<SharedString>,
  accessible_name: Option<SharedString>,
  label: Option<SharedString>,
  description: Option<SharedString>,
  error: Option<SharedString>,
  idle_border: Option<Hsla>,
  size: Size,
  radius: Option<Size>,
  disabled: bool,
  read_only: bool,
  password: bool,
  bare: bool,
  embedded: bool,
  max_length: Option<usize>,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl TextInput {
  pub fn new(cx: &mut Context<Self>) -> Self {
    TextInput {
      edit: TextEdit::new(""),
      state: LineState::new(),
      // Every field is a tab stop by default, the way a form control is
      // in a browser. Ordering falls out of render order unless a host
      // sets `tab_index`.
      focus: cx.focus_handle().tab_stop(true),
      id: "guise-textinput",
      placeholder: SharedString::default(),
      short_placeholder: None,
      accessible_name: None,
      label: None,
      description: None,
      error: None,
      idle_border: None,
      size: Size::Sm,
      radius: None,
      disabled: false,
      read_only: false,
      password: false,
      bare: false,
      embedded: false,
      max_length: None,
    }
  }

  /// The element identifier, which is also the accessibility AutomationId.
  /// Without this call the field is `guise-textinput`.
  pub fn id(mut self, id: &'static str) -> Self {
    self.id = id;
    self
  }

  pub fn value(mut self, value: &str) -> Self {
    self.edit = TextEdit::new(value);
    self
  }

  pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
    self.placeholder = placeholder.into();
    self
  }

  /// A shorter placeholder for narrow fields. It is shown only while the
  /// field is narrower than 142 DIP and never becomes the accessible name.
  pub fn short_placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
    self.short_placeholder = Some(placeholder.into());
    self
  }

  /// The accessible name. The placeholder stands in when this is unset.
  pub fn accessible_name(mut self, name: impl Into<SharedString>) -> Self {
    self.accessible_name = Some(name.into());
    self
  }

  pub fn label(mut self, label: impl Into<SharedString>) -> Self {
    self.label = Some(label.into());
    self
  }

  pub fn description(mut self, description: impl Into<SharedString>) -> Self {
    self.description = Some(description.into());
    self
  }

  pub fn error(mut self, error: impl Into<SharedString>) -> Self {
    self.error = Some(error.into());
    self
  }

  /// Override the idle field boundary while keeping the theme's focus and error colors.
  pub fn idle_border(mut self, color: Hsla) -> Self {
    self.idle_border = Some(color);
    self
  }

  pub fn size(mut self, size: Size) -> Self {
    self.size = size;
    self
  }

  pub fn radius(mut self, radius: Size) -> Self {
    self.radius = Some(radius);
    self
  }

  pub fn disabled(mut self, disabled: bool) -> Self {
    self.disabled = disabled;
    self
  }

  /// Selectable and copyable, but not editable — an `<input readonly>`.
  /// Unlike [`disabled`](Self::disabled) the field still takes focus.
  pub fn read_only(mut self, read_only: bool) -> Self {
    self.read_only = read_only;
    self
  }

  pub fn password(mut self, password: bool) -> Self {
    self.password = password;
    self
  }

  /// Draw the editing surface without its own frame and background, for
  /// fields placed inside a host's container.
  pub fn bare(mut self, bare: bool) -> Self {
    self.bare = bare;
    self
  }

  /// Let a composite control draw the frame and focus ring. The editing
  /// surface keeps its own focus, text metrics and accessibility actions.
  pub fn embedded(mut self) -> Self {
    self.embedded = true;
    self
  }

  /// Whether the editing buffer is empty.
  pub fn is_empty(&self) -> bool {
    self.edit.is_empty()
  }

  /// Where the field sits in the window's Tab order.
  pub fn tab_order(mut self, order: isize) -> Self {
    self.focus = self.focus.clone().tab_index(order);
    self
  }

  /// Move keyboard focus to the field.
  pub fn focus(&self, window: &mut Window, cx: &mut App) {
    window.focus(&self.focus, cx);
  }

  /// Whether the field holds keyboard focus in this window.
  pub fn is_focused(&self, window: &Window) -> bool {
    self.focus.is_focused(window)
  }

  /// Replace the placeholder shown while the value is empty.
  pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>, cx: &mut Context<Self>) {
    self.placeholder = placeholder.into();
    cx.notify();
  }

  /// Cap the value's length in characters, like `<input maxlength>`.
  pub fn max_length(mut self, max: usize) -> Self {
    self.max_length = Some(max);
    self
  }

  /// The current text.
  pub fn text(&self) -> String {
    self.edit.text()
  }

  /// Replace the text programmatically.
  pub fn set_text(&mut self, value: &str, cx: &mut Context<Self>) {
    self.edit.set_text(value);
    cx.notify();
  }

  /// Clear through the editor so the change is undoable and emits a Change.
  pub fn clear(&mut self, cx: &mut Context<Self>) {
    if !self.edit.is_empty() {
      let whole_line = line::to_utf16(&self.edit, &(0..self.edit.len()));
      line::replace(self, Some(whole_line), "", None, cx);
    }
  }

  /// Replace the whole value for the platform's SetValue action. A field that
  /// cannot be edited ignores it.
  pub(crate) fn set_accessibility_value(&mut self, value: &str, cx: &mut Context<Self>) {
    if self.disabled || self.read_only {
      return;
    }
    self.edit.set_text(value);
    self.state.marked = None;
    self.line_changed(cx);
  }

  /// Select the whole value, as focusing a field with `<input autofocus>`
  /// and a preset value tends to.
  pub fn select_all(&mut self, cx: &mut Context<Self>) {
    self.edit.select_all();
    cx.notify();
  }

  /// Two-way bind the text to a `Signal<String>`. The signal is the source
  /// of truth: the field adopts its value now, edits write back through
  /// [`Signal::set_if_changed`], and signal writes replace the text without
  /// emitting. Equality guards on both directions prevent update loops.
  pub fn bind(entity: &Entity<TextInput>, signal: &Signal<String>, cx: &mut App) {
    let initial = signal.get(cx);
    entity.update(cx, |this, cx| this.sync_text(initial, cx));
    let sink = signal.clone();
    cx.subscribe(entity, move |_input, event: &TextInputEvent, cx| {
      if let TextInputEvent::Change(text) = event {
        sink.set_if_changed(cx, text.clone());
      }
    })
    .detach();
    let field = entity.downgrade();
    cx.observe(signal.entity(), move |observed, cx| {
      let text = observed.read(cx).clone();
      field.update(cx, |this, cx| this.sync_text(text, cx)).ok();
    })
    .detach();
  }

  /// Programmatic set: repaint without emitting an event.
  fn sync_text(&mut self, text: String, cx: &mut Context<Self>) {
    if self.edit.text() != text {
      self.edit.set_text(&text);
      cx.notify();
    }
  }

  fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
    if self.disabled {
      return;
    }
    match line::keys(self, event, window, cx) {
      KeyOutcome::Submit => {
        cx.emit(TextInputEvent::Submit(self.edit.text()));
        cx.notify();
        cx.stop_propagation();
      }
      KeyOutcome::Edited => {
        self.line_changed(cx);
        cx.stop_propagation();
      }
      // Escape (Cancel) bubbles so dialogs can close on it. Printable
      // keys pass too: the platform hands them to the input handler,
      // which is what makes IME and dead keys work.
      KeyOutcome::Cancel | KeyOutcome::Pass => {}
    }
  }
}

impl LineEditor for TextInput {
  fn edit(&self) -> &TextEdit {
    &self.edit
  }

  fn edit_mut(&mut self) -> &mut TextEdit {
    &mut self.edit
  }

  fn line(&self) -> &LineState {
    &self.state
  }

  fn line_mut(&mut self) -> &mut LineState {
    &mut self.state
  }

  fn line_focus(&self) -> &FocusHandle {
    &self.focus
  }

  fn line_masked(&self) -> bool {
    self.password
  }

  fn line_read_only(&self) -> bool {
    self.read_only || self.disabled
  }

  fn line_max_length(&self) -> Option<usize> {
    self.max_length
  }

  fn line_changed(&mut self, cx: &mut Context<Self>) {
    cx.emit(TextInputEvent::Change(self.edit.text()));
    cx.notify();
  }
}

line::line_input_handler!(TextInput);
line::line_focus_builders!(TextInput);

impl Render for TextInput {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let t = theme(cx);
    let (height, pad_x, font) = control_metrics(self.size);
    let custom = cx.try_global::<TextInputSurface>();
    let radius = match (self.radius, custom) {
      (Some(radius), _) => t.radius(radius),
      (None, Some(surface)) => surface.radius,
      (None, None) => t.radius(t.default_radius),
    };
    let focused = self.focus.is_focused(window) && !self.disabled;
    let has_error = self.error.is_some();

    let border = if has_error {
      t.color(ColorName::Red, 6).hsla()
    } else if focused {
      custom.map_or_else(|| t.primary().hsla(), |surface| surface.focused_border)
    } else {
      self
        .idle_border
        .unwrap_or_else(|| custom.map_or_else(|| t.border().hsla(), |surface| surface.border))
    };
    let dimmed = t.dimmed().hsla();
    let surface = match custom {
      Some(surface) if focused => surface.focused_background,
      Some(surface) => surface.background,
      None => t.surface().hsla(),
    };
    let name = self
      .accessible_name
      .clone()
      .unwrap_or_else(|| self.placeholder.clone());
    let value = if self.password {
      String::new()
    } else {
      self.edit.text()
    };

    let mut line = Line::new(cx.entity()).placeholder(self.placeholder.clone(), dimmed);
    if let Some(short) = self.short_placeholder.clone() {
      line = line.short_placeholder(short);
    }

    let mut field = line::wire(div().id(self.id), &self.focus, &self.state, cx)
      .on_key_down(cx.listener(Self::on_key))
      .accessibility_id(self.id)
      .role(if self.password {
        Role::PasswordInput
      } else {
        Role::TextInput
      })
      .aria_label(name)
      .aria_value(value)
      .on_a11y_action(AccessibleAction::SetValue, {
        let field = cx.entity();
        move |data, _window, cx| {
          if let Some(accesskit::ActionData::Value(value)) = data {
            field.update(cx, |field, cx| {
              field.set_accessibility_value(value, cx);
            });
          }
        }
      })
      .flex()
      .items_center()
      .w_full()
      .overflow_hidden()
      .h(px(height))
      .px(px(if self.embedded { 0.0 } else { pad_x }))
      .text_size(px(font))
      .line_height(px(font * 1.3))
      .child(div().flex_1().min_w(px(0.0)).child(line));
    // A bare field sits on the host's own surface. It keeps the constant 1px
    // border so the keyboard ring does not shift layout, and shows it only
    // while focused; an unfocused bare field has no frame and no background.
    let clear = hsla(0.0, 0.0, 0.0, 0.0);
    if !self.embedded {
      field = field
        .rounded(px(radius))
        .border_1()
        .border_color(if self.bare && !focused { clear } else { border })
        .bg(if self.bare { clear } else { surface });
    }

    let mut chrome = Field::new().child(if self.disabled {
      field.opacity(0.6)
    } else {
      field
    });
    if let Some(label) = self.label.clone() {
      chrome = chrome.label(label);
    }
    if let Some(error) = self.error.clone() {
      chrome = chrome.error(error);
    } else if let Some(description) = self.description.clone() {
      chrome = chrome.description(description);
    }
    chrome.probe_any("TextInput")
  }
}
