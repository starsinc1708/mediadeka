//! `Button` — the flagship interactive control, with the library's variants,
//! colors, and sizes.

use gpui::prelude::*;
use std::rc::Rc;

use gpui::{
  div, hsla, px, AccessibleAction, App, Background, BoxShadow, ClickEvent, ElementId, FocusHandle,
  FontWeight, Hsla, IntoElement, Role, SharedString, Window,
};

use crate::devtools::Probed;
use crate::input::ClickHandler;
use crate::style::{surface, ColorValue, Variant};
use crate::theme::{theme, Size};

/// A click handler shared by the pointer and the accessibility Click action.
type SharedClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A clickable button.
#[derive(IntoElement)]
pub struct Button {
  id: ElementId,
  label: SharedString,
  variant: Variant,
  color: ColorValue,
  size: Size,
  radius: Option<Size>,
  full_width: bool,
  disabled: bool,
  left_section: Option<gpui::AnyElement>,
  right_section: Option<gpui::AnyElement>,
  on_click: Option<ClickHandler>,
  accessible_name: Option<SharedString>,
  description: Option<SharedString>,
  tab_order: isize,
  focus_handle: Option<FocusHandle>,
  focus_outline: Option<Hsla>,
  backgrounds: Option<[Background; 3]>,
}

impl Button {
  pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
    Button {
      id: id.into(),
      label: label.into(),
      variant: Variant::Filled,
      color: ColorValue::default(),
      size: Size::Sm,
      radius: None,
      full_width: false,
      disabled: false,
      left_section: None,
      right_section: None,
      on_click: None,
      accessible_name: None,
      description: None,
      tab_order: 0,
      focus_handle: None,
      focus_outline: None,
      backgrounds: None,
    }
  }

  /// Name announced by screen readers when it differs from the visible label.
  pub fn accessible_name(mut self, name: impl Into<SharedString>) -> Self {
    self.accessible_name = Some(name.into());
    self
  }

  /// Extra context screen readers announce after the name.
  pub fn accessible_description(mut self, description: impl Into<SharedString>) -> Self {
    self.description = Some(description.into());
    self
  }

  /// Position in the window tab order. `0` keeps document order.
  pub fn tab_order(mut self, order: isize) -> Self {
    self.tab_order = order;
    self
  }

  /// Focus through a handle the caller owns, so it can move focus back here.
  pub fn track_focus(mut self, handle: &FocusHandle) -> Self {
    self.focus_handle = Some(handle.clone());
    self
  }

  /// Draw a two-pixel outer keyboard outline, separated from the button by its border.
  pub fn focus_outline(mut self, color: Hsla) -> Self {
    self.focus_outline = Some(color);
    self
  }

  /// Paint custom resting, hovered and pressed surfaces using the same button semantics.
  pub fn backgrounds(
    mut self,
    resting: Background,
    hovered: Background,
    pressed: Background,
  ) -> Self {
    self.backgrounds = Some([resting, hovered, pressed]);
    self
  }

  pub fn variant(mut self, variant: Variant) -> Self {
    self.variant = variant;
    self
  }

  /// Override the button color. Accepts a palette `ColorName` or any explicit
  /// color (e.g. `color!(rgba(34, 139, 230, 1))`).
  pub fn color(mut self, color: impl Into<ColorValue>) -> Self {
    self.color = color.into();
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

  pub fn full_width(mut self, full_width: bool) -> Self {
    self.full_width = full_width;
    self
  }

  pub fn disabled(mut self, disabled: bool) -> Self {
    self.disabled = disabled;
    self
  }

  /// Content shown before the label (e.g. an icon).
  pub fn left_section(mut self, section: impl IntoElement) -> Self {
    self.left_section = Some(section.into_any_element());
    self
  }

  /// Content shown after the label.
  pub fn right_section(mut self, section: impl IntoElement) -> Self {
    self.right_section = Some(section.into_any_element());
    self
  }

  pub fn on_click(
    mut self,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
  ) -> Self {
    self.on_click = Some(Box::new(handler));
    self
  }

  /// (height, horizontal padding, font size) for the size scale.
  fn metrics(&self) -> (f32, f32, f32) {
    match self.size {
      Size::Xs => (30.0, 14.0, 12.0),
      Size::Sm => (36.0, 18.0, 14.0),
      Size::Md => (42.0, 22.0, 16.0),
      Size::Lg => (50.0, 26.0, 18.0),
      Size::Xl => (60.0, 32.0, 20.0),
    }
  }
}

impl RenderOnce for Button {
  fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
    let t = theme(cx);
    let s = surface(t, self.color, self.variant);
    let (height, pad_x, font) = self.metrics();
    let radius = t.radius(self.radius.unwrap_or(t.default_radius));
    let focus = t.primary().hsla();
    let focus_gap = t.body().hsla();
    let focus_outline = self.focus_outline;
    let automation_id = self.id.to_string();
    let name = self.accessible_name.unwrap_or_else(|| self.label.clone());
    let [resting, hovered, pressed] =
      self
        .backgrounds
        .unwrap_or([s.bg.into(), s.bg_hover.into(), s.bg_hover.into()]);

    let mut el = div()
      .id(self.id)
      .accessibility_id(automation_id)
      .role(Role::Button)
      .aria_label(name)
      .flex()
      .items_center()
      .justify_center()
      .gap(px(8.0))
      .h(px(height))
      .px(px(pad_x))
      .rounded(px(radius))
      .bg(resting)
      .text_color(s.fg)
      .text_size(px(font))
      .font_weight(FontWeight::SEMIBOLD)
      // A constant 1px border keeps the keyboard ring from shifting layout.
      .border_1()
      .border_color(s.border.unwrap_or(hsla(0.0, 0.0, 0.0, 0.0)))
      .focus_visible(move |style| match focus_outline {
        Some(outline) => {
          style.border_color(focus_gap).shadow(vec![
            BoxShadow::new(px(0.), px(0.), outline).spread_radius(px(2.))
          ])
        }
        None => style.border_color(focus),
      })
      .on_key_down(crate::input::handle_tab);

    if let Some(description) = self.description {
      el = el.aria_description(description);
    }
    if let Some(handle) = &self.focus_handle {
      el = el.track_focus(handle);
    }
    if self.full_width {
      el = el.w_full();
    }
    if let Some(left) = self.left_section {
      el = el.child(left);
    }
    el = el.child(self.label);
    if let Some(right) = self.right_section {
      el = el.child(right);
    }

    let element = if self.disabled {
      el.tab_stop(false).opacity(0.6)
    } else {
      el = el.tab_index(self.tab_order).hover(move |st| st.bg(hovered));
      if self.backgrounds.is_some() {
        el = el.active(move |st| st.bg(pressed));
      }
      if let Some(handler) = self.on_click {
        // Screen readers press the button through the accessibility Click action.
        let handler: SharedClickHandler = Rc::from(handler);
        let accessible = Rc::clone(&handler);
        el = el
          .on_click(move |event, window, cx| handler(event, window, cx))
          .on_a11y_action(AccessibleAction::Click, move |_, window, cx| {
            accessible(&ClickEvent::default(), window, cx)
          });
      }
      el
    };

    element
      .probe("Button")
      .attr("variant", self.variant.label())
      .attr("size", self.size.label())
      .attr_if("disabled", self.disabled)
  }
}
