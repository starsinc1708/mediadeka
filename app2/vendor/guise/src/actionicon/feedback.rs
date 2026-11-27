use std::time::{Duration, Instant};

use gpui::{prelude::*, App, Div, ElementId, Hsla, MouseButton, Stateful, Window};

use crate::{
  anim::{AnimValue, Easing},
  style::Surface,
};

#[derive(Clone, Copy)]
pub(super) struct Options {
  pub(super) hover: Duration,
  pub(super) press: Duration,
  pub(super) hovered_ink: Hsla,
  pub(super) pressed: Hsla,
  pub(super) hover_scale: f32,
  pub(super) press_scale: f32,
}

impl Options {
  fn scale(self, tone: Tone) -> f32 {
    match tone {
      Tone::Idle => 1.,
      Tone::Hover => self.hover_scale,
      Tone::Pressed => self.press_scale,
    }
  }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Tone {
  #[default]
  Idle,
  Hover,
  Pressed,
}

struct Transition {
  from: Hsla,
  from_scale: f32,
  since: Instant,
  duration: Duration,
}

#[derive(Default)]
struct Feedback {
  tone: Tone,
  painted: Option<(Hsla, f32)>,
  transition: Option<Transition>,
}

impl Feedback {
  fn retarget(&mut self, tone: Tone, options: Options, now: Instant) -> bool {
    if self.tone == tone {
      return false;
    }
    let duration = if self.tone == Tone::Pressed || tone == Tone::Pressed {
      options.press
    } else {
      options.hover
    };
    self.tone = tone;
    self.transition = self.painted.map(|(from, from_scale)| Transition {
      from,
      from_scale,
      since: now,
      duration,
    });
    true
  }

  fn sample(
    &mut self,
    target: Hsla,
    target_scale: f32,
    now: Instant,
    immediate: bool,
  ) -> (Hsla, bool) {
    let (color, scale, moving) = match &self.transition {
      Some(transition) if !immediate => {
        let elapsed = now.saturating_duration_since(transition.since);
        if elapsed >= transition.duration {
          (target, target_scale, false)
        } else {
          let progress =
            Easing::EaseOutCubic.apply(elapsed.as_secs_f32() / transition.duration.as_secs_f32());
          let color = AnimValue::Color(transition.from)
            .lerp(AnimValue::Color(target), progress)
            .color()
            .unwrap_or(target);
          (
            color,
            transition.from_scale + (target_scale - transition.from_scale) * progress,
            true,
          )
        }
      }
      _ => (target, if immediate { 1. } else { target_scale }, false),
    };
    if !moving {
      self.transition = None;
    }
    self.painted = Some((color, scale));
    (color, moving)
  }
}

fn background(tone: Tone, surface: Surface, pressed: Hsla) -> Hsla {
  match tone {
    Tone::Idle if surface.bg.a == 0. => surface.bg_hover.alpha(0.),
    Tone::Idle => surface.bg,
    Tone::Hover => surface.bg_hover,
    Tone::Pressed => pressed,
  }
}

pub(super) fn attach(
  element: Stateful<Div>,
  id: ElementId,
  surface: Surface,
  options: Options,
  halo: bool,
  window: &mut Window,
  cx: &mut App,
) -> (Stateful<Div>, f32, Hsla) {
  let state = window.use_keyed_state((id, "pointer-feedback"), cx, |_, _| Feedback::default());
  let immediate = cx.reduce_motion() || (options.hover.is_zero() && options.press.is_zero());
  let (color, scale, moving, ink) = state.update(cx, |feedback, _| {
    let (color, moving) = feedback.sample(
      background(feedback.tone, surface, options.pressed),
      options.scale(feedback.tone),
      Instant::now(),
      immediate,
    );
    (
      color,
      feedback.painted.map_or(1., |(_, scale)| scale),
      moving,
      if feedback.tone == Tone::Idle {
        surface.fg
      } else {
        options.hovered_ink
      },
    )
  });
  if moving {
    window.request_animation_frame();
  }

  let hover = state.clone();
  let press = state.clone();
  let release = state.clone();
  let element = if halo {
    // The halo carries the wash; the square stays transparent so the
    // command reads as a glyph with a local backing, not a filled button.
    element.text_color(ink)
  } else {
    element.bg(color).text_color(ink)
  }
  .on_hover(move |&hovered, _, cx| {
    hover.update(cx, |state, cx| {
      let tone = if hovered && state.tone == Tone::Pressed {
        Tone::Pressed
      } else if hovered {
        Tone::Hover
      } else {
        Tone::Idle
      };
      if state.retarget(tone, options, Instant::now()) {
        cx.notify();
      }
    });
  })
  .on_mouse_down(MouseButton::Left, move |_, _, cx| {
    press.update(cx, |state, cx| {
      if state.retarget(Tone::Pressed, options, Instant::now()) {
        cx.notify();
      }
    });
  })
  .on_mouse_up(MouseButton::Left, move |_, _, cx| {
    release.update(cx, |state, cx| {
      if state.tone == Tone::Pressed && state.retarget(Tone::Hover, options, Instant::now()) {
        cx.notify();
      }
    });
  })
  .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
    if state.read(cx).tone == Tone::Pressed {
      state.update(cx, |state, cx| {
        if state.retarget(Tone::Idle, options, Instant::now()) {
          cx.notify();
        }
      });
    }
  });
  (element, scale, color)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn press_interrupts_hover_from_the_last_painted_tone_and_ends_in_ninety_ms() {
    let at = Instant::now();
    let options = Options {
      hover: Duration::from_millis(140),
      press: Duration::from_millis(90),
      hovered_ink: gpui::Hsla::white(),
      pressed: gpui::hsla(0.7, 0.1, 0.3, 1.),
      hover_scale: 1.04,
      press_scale: 0.94,
    };
    let idle = gpui::hsla(0.7, 0.1, 0.2, 1.);
    let hover = gpui::hsla(0.7, 0.1, 0.6, 1.);
    let pressed = gpui::hsla(0.7, 0.1, 0.3, 1.);
    let mut feedback = Feedback::default();
    assert_eq!(
      feedback.sample(idle, options.scale(Tone::Idle), at, false),
      (idle, false)
    );
    assert!(feedback.retarget(Tone::Hover, options, at));
    let middle = at + Duration::from_millis(70);
    let (visible, moving) = feedback.sample(hover, options.scale(Tone::Hover), middle, false);
    assert!(moving && idle.l < visible.l && visible.l < hover.l);
    assert!(feedback.retarget(Tone::Pressed, options, middle));
    assert_eq!(
      feedback.sample(pressed, options.scale(Tone::Pressed), middle, false),
      (visible, true)
    );
    assert!(
      feedback
        .sample(
          pressed,
          options.scale(Tone::Pressed),
          middle + Duration::from_millis(89),
          false
        )
        .1
    );
    assert_eq!(
      feedback.sample(
        pressed,
        options.scale(Tone::Pressed),
        middle + options.press,
        false
      ),
      (pressed, false)
    );
    assert!(feedback.retarget(Tone::Hover, options, middle + options.press));
    assert_eq!(
      feedback.sample(
        hover,
        options.scale(Tone::Hover),
        middle + options.press,
        true
      ),
      (hover, false)
    );
  }

  #[test]
  fn reduced_motion_keeps_the_final_tone_without_scaling() {
    let at = Instant::now();
    let options = Options {
      hover: Duration::from_millis(140),
      press: Duration::from_millis(80),
      hovered_ink: gpui::Hsla::white(),
      pressed: gpui::hsla(0.7, 0.1, 0.3, 1.),
      hover_scale: 1.02,
      press_scale: 0.96,
    };
    let pressed = gpui::hsla(0.7, 0.1, 0.3, 1.);
    let mut feedback = Feedback::default();
    feedback.sample(pressed, options.scale(Tone::Idle), at, true);
    assert!(feedback.retarget(Tone::Pressed, options, at));
    let (color, moving) = feedback.sample(pressed, options.scale(Tone::Pressed), at, true);
    assert_eq!(color, pressed);
    assert!(!moving);
    assert_eq!(feedback.painted.map(|(_, scale)| scale), Some(1.));
  }

  #[test]
  fn hover_stops_scheduling_frames_at_its_duration() {
    let at = Instant::now();
    let options = Options {
      hover: Duration::from_millis(140),
      press: Duration::from_millis(90),
      hovered_ink: gpui::Hsla::white(),
      pressed: gpui::hsla(0.7, 0.1, 0.3, 1.),
      hover_scale: 1.04,
      press_scale: 0.94,
    };
    let hover = gpui::hsla(0.7, 0.1, 0.6, 0.15);
    let idle = hover.alpha(0.);
    let mut feedback = Feedback::default();
    feedback.sample(idle, options.scale(Tone::Idle), at, false);
    assert!(feedback.retarget(Tone::Hover, options, at));
    assert!(!feedback.retarget(Tone::Hover, options, at + Duration::from_millis(139)));
    assert!(
      feedback
        .sample(
          hover,
          options.scale(Tone::Hover),
          at + Duration::from_millis(139),
          false
        )
        .1
    );
    assert_eq!(
      feedback.sample(hover, options.scale(Tone::Hover), at + options.hover, false),
      (hover, false)
    );
    assert_eq!(
      feedback.sample(
        hover,
        options.scale(Tone::Hover),
        at + Duration::from_secs(1),
        false
      ),
      (hover, false)
    );
  }
}
