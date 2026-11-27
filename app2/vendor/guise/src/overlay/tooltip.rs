//! `Tooltip` — a small floating label, plus the [`tooltip`] helper for gpui's
//! built-in `.tooltip(...)` attachment.

use gpui::prelude::*;
use gpui::{div, px, AnyView, App, Global, Hsla, IntoElement, Role, SharedString, Window};

use crate::devtools::Probed;
use crate::theme::{theme, ColorName, Size};

/// An optional application-wide material for tooltips.
#[derive(Clone, Copy)]
pub struct TooltipSurface {
  pub background: Hsla,
  pub border: Hsla,
  pub foreground: Hsla,
}

impl Global for TooltipSurface {}

/// The floating tooltip bubble (a gpui view). Usually built for you by
/// [`tooltip`]; construct directly only if you need a custom builder.
pub struct Tooltip {
  label: SharedString,
}

impl Tooltip {
  pub fn new(label: impl Into<SharedString>) -> Self {
    Tooltip {
      label: label.into(),
    }
  }
}

impl Render for Tooltip {
  fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
    let t = theme(cx);
    let bg = if t.scheme.is_dark() {
      t.color(ColorName::Dark, 4)
    } else {
      t.color(ColorName::Dark, 9)
    };
    let surface = cx.try_global::<TooltipSurface>().copied();
    div()
      .id("guise-tooltip")
      .accessibility_id("guise-tooltip")
      .role(Role::Tooltip)
      .aria_label(self.label.clone())
      .px(px(10.0))
      .py(px(6.0))
      .rounded(px(t.radius(Size::Sm)))
      .bg(bg.hsla())
      .text_size(px(t.font_size(Size::Sm)))
      .text_color(t.white.hsla())
      .when_some(surface, |tooltip, surface| {
        tooltip
          .rounded(px(8.))
          .border_1()
          .border_color(surface.border)
          .bg(surface.background)
          .text_color(surface.foreground)
      })
      .shadow_md()
      .child(self.label.clone())
      .probe("Tooltip")
  }
}

/// A builder for gpui's `.tooltip(...)`: attach a themed tooltip to any
/// interactive element.
///
/// ```ignore
/// div().id("hoverme").tooltip(guise::tooltip("Helpful hint"))
/// ```
pub fn tooltip(
  label: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
  let label = label.into();
  move |_window, cx| cx.new(|_cx| Tooltip::new(label.clone())).into()
}
