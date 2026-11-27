//! `ActionIcon` — a square icon-only button.

mod feedback;

use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
  accesskit::Toggled, div, hsla, px, transparent_black, AccessibleAction, AnyElement, App,
  ClickEvent, ElementId, FocusHandle, IntoElement, Pixels, Role, SharedString, Window,
};

use crate::devtools::Probed;
use crate::icon::Glyph;
use crate::input::ClickHandler;
use crate::style::{icon_size, surface, ColorValue, Surface, Variant};
use crate::theme::{theme, ColorName, Size};

/// A click handler shared by the pointer and the accessibility Click action.
type SharedClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A compact, square icon button.
#[derive(IntoElement)]
pub struct ActionIcon {
  id: ElementId,
  icon: Glyph,
  label: Option<SharedString>,
  variant: Variant,
  color: ColorValue,
  surface_style: Option<Surface>,
  size: Size,
  pixel_size: Option<Pixels>,
  radius: Option<Size>,
  disabled: bool,
  on_click: Option<ClickHandler>,
  glyph_size: Option<Pixels>,
  tab_order: isize,
  focus_handle: Option<FocusHandle>,
  description: Option<SharedString>,
  toggled: Option<bool>,
  feedback: Option<feedback::Options>,
  feedback_scale: Option<(f32, f32)>,
  glyph_halo: bool,
  glyph_content: Option<AnyElement>,
  tooltip: bool,
}

impl ActionIcon {
  pub fn new(id: impl Into<ElementId>, icon: impl Into<Glyph>) -> Self {
    ActionIcon {
      id: id.into(),
      icon: icon.into(),
      label: None,
      variant: Variant::Subtle,
      color: ColorValue::Named(ColorName::Gray),
      surface_style: None,
      size: Size::Md,
      pixel_size: None,
      radius: None,
      disabled: false,
      on_click: None,
      glyph_size: None,
      tab_order: 0,
      focus_handle: None,
      description: None,
      toggled: None,
      feedback: None,
      feedback_scale: None,
      glyph_halo: false,
      glyph_content: None,
      tooltip: true,
    }
  }

  /// Give the icon-only control a human-readable name and hover tooltip.
  /// Screen readers announce it as the button's name.
  pub fn label(mut self, label: impl Into<SharedString>) -> Self {
    self.label = Some(label.into());
    self
  }

  pub fn variant(mut self, variant: Variant) -> Self {
    self.variant = variant;
    self
  }

  pub fn color(mut self, color: impl Into<ColorValue>) -> Self {
    self.color = color.into();
    self
  }

  /// Supply explicit colors while keeping the action, focus, and feedback behavior.
  pub fn surface_style(mut self, style: Surface) -> Self {
    self.surface_style = Some(style);
    self
  }

  pub fn size(mut self, size: Size) -> Self {
    self.size = size;
    self
  }

  /// Set both the visible square and the hit target to an exact size.
  pub fn pixel_size(mut self, size: Pixels) -> Self {
    self.pixel_size = Some(size);
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

  /// Draw the glyph at an exact size instead of half the button.
  pub fn glyph_size(mut self, size: Pixels) -> Self {
    self.glyph_size = Some(size);
    self
  }

  /// Use stateful glyph geometry while retaining the action's input and focus behavior.
  pub fn glyph_content(mut self, glyph: impl IntoElement) -> Self {
    self.glyph_content = Some(glyph.into_any_element());
    self
  }

  /// Hide the hint while a popup attached to this action owns the pointer.
  pub fn tooltip_enabled(mut self, enabled: bool) -> Self {
    self.tooltip = enabled;
    self
  }

  /// Position in the window tab order. `0` keeps document order.
  pub fn tab_order(mut self, order: isize) -> Self {
    self.tab_order = order;
    self
  }

  /// Extra context screen readers announce after the name, such as why the
  /// control is unavailable.
  pub fn accessible_description(mut self, description: impl Into<SharedString>) -> Self {
    self.description = Some(description.into());
    self
  }

  /// Preserve a button's focus identity across renders.
  pub fn track_focus(mut self, handle: &FocusHandle) -> Self {
    self.focus_handle = Some(handle.clone());
    self
  }

  /// Expose the state of a toggle and mark the active state with a dot.
  pub fn toggled(mut self, toggled: bool) -> Self {
    self.toggled = Some(toggled);
    self
  }

  /// Opt into an interruptible background tint for pointer hover and press.
  /// Zero durations apply the same states immediately.
  pub fn pointer_feedback(
    mut self,
    hover: Duration,
    press: Duration,
    hovered_ink: gpui::Hsla,
    pressed: gpui::Hsla,
  ) -> Self {
    self.feedback = Some(feedback::Options {
      hover,
      press,
      hovered_ink,
      pressed,
      hover_scale: 1.04,
      press_scale: 0.94,
    });
    self
  }

  /// Override the glyph scale used by [`Self::pointer_feedback`]. The button
  /// and its hit target keep their size; only the glyph is transformed.
  pub fn pointer_scale(mut self, hover: f32, press: f32) -> Self {
    self.feedback_scale = Some((hover, press));
    self
  }

  /// Draw the hover/press wash as a small rounded halo behind the glyph
  /// instead of filling the whole square. Used by the Fluid Glass transport.
  pub fn glyph_halo(mut self, enabled: bool) -> Self {
    self.glyph_halo = enabled;
    self
  }

  pub fn on_click(
    mut self,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
  ) -> Self {
    self.on_click = Some(Box::new(handler));
    self
  }
}

impl RenderOnce for ActionIcon {
  fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
    let t = theme(cx);
    let s = self
      .surface_style
      .unwrap_or_else(|| surface(t, self.color, self.variant));
    let dim = self.pixel_size.unwrap_or_else(|| px(icon_size(self.size)));
    let radius = t.radius(self.radius.unwrap_or(t.default_radius));
    // A filled control already wears the accent, so the keyboard ring uses
    // the text color to stay visible on every variant.
    let focus = t.text().hsla();
    let automation_id = self.id.to_string();
    let feedback = self.feedback.filter(|_| !self.disabled).map(|mut options| {
      if let Some((hover_scale, press_scale)) = self.feedback_scale {
        options.hover_scale = hover_scale;
        options.press_scale = press_scale;
      }
      (self.id.clone(), options)
    });

    let mut el = div()
      .id(self.id)
      .accessibility_id(automation_id)
      .role(Role::Button)
      .w(dim)
      .h(dim)
      .flex()
      .items_center()
      .justify_center()
      .relative()
      .rounded(px(radius))
      .bg(s.bg)
      .text_color(s.fg)
      .text_size(self.glyph_size.unwrap_or(dim * 0.5))
      // A constant 1px border keeps the keyboard ring from shifting layout.
      .border_1()
      .border_color(s.border.unwrap_or(hsla(0.0, 0.0, 0.0, 0.0)))
      .focus_visible(move |style| style.border_color(focus))
      .on_key_down(crate::input::handle_tab);
    if let Some(toggled) = self.toggled {
      el = el.aria_toggled(Toggled::from(toggled)).when(toggled, |el| {
        el.relative().child(
          div()
            .absolute()
            .bottom(px(1.))
            .left(px((f32::from(dim) - 5.) * 0.5))
            .size(px(3.))
            .rounded_full()
            .bg(s.fg),
        )
      });
    }
    if let Some(label) = &self.label {
      el = el.aria_label(label.clone());
    }
    if let Some(description) = self.description {
      el = el.aria_description(description);
    }

    let label = self.label;
    let mut glyph_scale = 1.;
    let mut halo_color = transparent_black();
    let element = if self.disabled {
      el.opacity(0.5)
    } else {
      el = el.tab_index(self.tab_order);
      if let Some((id, options)) = feedback {
        (el, glyph_scale, halo_color) =
          feedback::attach(el, id, s, options, self.glyph_halo, window, cx);
      } else {
        let hover_bg = s.bg_hover;
        el = el.hover(move |st| st.bg(hover_bg));
      }
      if let Some(handle) = &self.focus_handle {
        el = el.track_focus(handle);
      }
      if let Some(handler) = self.on_click {
        // Screen readers press the control through the accessibility Click action.
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

    let glyph = self
      .glyph_content
      .unwrap_or_else(|| self.icon.into_any_element());
    let halo = self.glyph_halo.then(|| {
      let halo_size = px((f32::from(dim) - 8.).max(18.));
      div()
        .absolute()
        .top(px(4.))
        .left(px(4.))
        .size(halo_size)
        .rounded_full()
        .bg(halo_color)
    });
    let element = element.children(halo).child(
      div()
        .flex_none()
        .when(glyph_scale != 1., |glyph| glyph.layer_scale(glyph_scale))
        .child(glyph),
    );
    let element = element.when_some(label.clone().filter(|_| self.tooltip), |element, label| {
      element.tooltip(crate::overlay::tooltip(label))
    });

    element
      .probe("ActionIcon")
      .attr("variant", self.variant.label())
      .attr("size", self.size.label())
      .attr_opt("label", label)
      .attr_if("disabled", self.disabled)
  }
}
