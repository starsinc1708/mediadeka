//! `Chip` — a selectable pill (controlled).

use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
  div, px, AccessibleAction, AnyElement, App, ClickEvent, ElementId, FontWeight, IntoElement, Role,
  SharedString, Toggled, Window,
};

use crate::devtools::Probed;
use crate::input::ClickHandler;
use crate::reactive::Binding;
use crate::style::ColorValue;
use crate::theme::{theme, Size};

/// A click handler shared by the pointer and the accessibility Click action.
type SharedClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A selectable chip. Controlled: pass `checked` and a
/// change handler via `cx.listener`, or two-way bind with [`Chip::bind`].
#[derive(IntoElement)]
pub struct Chip {
  id: ElementId,
  label: SharedString,
  checked: bool,
  color: ColorValue,
  size: Size,
  binding: Option<Binding<bool>>,
  on_change: Option<ClickHandler>,
  left_section: Option<AnyElement>,
  accessible_name: Option<SharedString>,
  tab_order: isize,
  tab: bool,
}

impl Chip {
  pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
    Chip {
      id: id.into(),
      label: label.into(),
      checked: false,
      color: ColorValue::default(),
      size: Size::Md,
      binding: None,
      on_change: None,
      left_section: None,
      accessible_name: None,
      tab_order: 0,
      tab: false,
    }
  }

  pub fn checked(mut self, checked: bool) -> Self {
    self.checked = checked;
    self
  }

  pub fn color(mut self, color: impl Into<ColorValue>) -> Self {
    self.color = color.into();
    self
  }

  pub fn size(mut self, size: Size) -> Self {
    self.size = size;
    self
  }

  /// Content shown before the label (e.g. an icon or a brand mark).
  pub fn left_section(mut self, section: impl IntoElement) -> Self {
    self.left_section = Some(section.into_any_element());
    self
  }

  /// Name announced by screen readers when it differs from the visible label.
  pub fn accessible_name(mut self, name: impl Into<SharedString>) -> Self {
    self.accessible_name = Some(name.into());
    self
  }

  /// Position in the window tab order. `0` keeps document order.
  pub fn tab_order(mut self, order: isize) -> Self {
    self.tab_order = order;
    self
  }

  /// Present the chip as one tab of a tab list: screen readers hear a
  /// selected tab instead of a checkbox, and no check mark is drawn.
  pub fn as_tab(mut self) -> Self {
    self.tab = true;
    self
  }

  /// Two-way bind the checked state. Overrides `checked`; clicks write the
  /// toggled value back through the binding, then run any `on_change`.
  pub fn bind(mut self, binding: Binding<bool>) -> Self {
    self.binding = Some(binding);
    self
  }

  pub fn on_change(
    mut self,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
  ) -> Self {
    self.on_change = Some(Box::new(handler));
    self
  }

  fn metrics(&self) -> (f32, f32, f32) {
    match self.size {
      Size::Xs => (24.0, 10.0, 11.0),
      Size::Sm => (28.0, 12.0, 12.0),
      Size::Md => (32.0, 16.0, 14.0),
      Size::Lg => (38.0, 20.0, 16.0),
      Size::Xl => (44.0, 24.0, 18.0),
    }
  }
}

impl RenderOnce for Chip {
  fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
    let t = theme(cx);
    let (height, pad_x, font) = self.metrics();
    let accent = self.color.accent(t);
    let checked = self.binding.as_ref().map_or(self.checked, |b| b.get(cx));

    let (bg, fg, border) = if checked {
      (self.color.soft(t), accent, accent)
    } else {
      (t.surface().hsla(), t.text().hsla(), t.border().hsla())
    };
    let hover_bg = t.surface_hover().hsla();
    // A checked chip already wears the accent border, so the keyboard ring
    // uses the text color to stay visible in both states.
    let focus = t.text().hsla();
    let automation_id = self.id.to_string();
    let name = self.accessible_name.unwrap_or_else(|| self.label.clone());

    let mut el = div()
      .id(self.id)
      .accessibility_id(automation_id)
      .aria_label(name)
      .flex()
      .items_center()
      .gap(px(6.0))
      .h(px(height))
      .px(px(pad_x))
      .rounded(px(height))
      .border_1()
      .border_color(border)
      .bg(bg)
      .text_color(fg)
      .text_size(px(font))
      .font_weight(FontWeight::MEDIUM);
    el = if self.tab {
      el.role(Role::Tab).aria_selected(checked)
    } else {
      el.role(Role::CheckBox).aria_toggled(if checked {
        Toggled::True
      } else {
        Toggled::False
      })
    };
    if checked {
      if !self.tab {
        el = el.child(SharedString::new_static("\u{2713}"));
      }
    } else {
      el = el.hover(move |s| s.bg(hover_bg));
    }
    if let Some(left) = self.left_section {
      el = el.child(left);
    }
    el = el.child(self.label);
    if self.binding.is_some() || self.on_change.is_some() {
      let binding = self.binding;
      let handler = self.on_change;
      let next = !checked;
      let toggle: SharedClickHandler = Rc::new(move |ev, window, cx| {
        if let Some(binding) = &binding {
          binding.set(cx, next);
        }
        if let Some(handler) = &handler {
          handler(ev, window, cx);
        }
      });
      let accessible = Rc::clone(&toggle);
      el = el
        .tab_index(self.tab_order)
        .focus_visible(move |style| style.border_color(focus))
        .on_key_down(crate::input::handle_tab)
        .on_click(move |ev, window, cx| toggle(ev, window, cx))
        // Screen readers press the chip through the accessibility Click action.
        .on_a11y_action(AccessibleAction::Click, move |_, window, cx| {
          accessible(&ClickEvent::default(), window, cx)
        });
    }
    el.probe("Chip")
      .attr("size", self.size.label())
      .attr_if("checked", self.checked)
  }
}
