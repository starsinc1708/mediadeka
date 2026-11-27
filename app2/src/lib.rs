mod appearance;
mod integrations;
mod music;
mod playback;
pub mod playlist;
pub mod providers;
mod soloist;
mod state;
mod storage;
mod telemetry;
mod ui;
mod visual;

#[cfg(all(windows, feature = "capture"))]
pub use soloist::seed_soloist_startup_capture;

pub use ui::{application, open_main_window};

#[cfg(feature = "capture")]
pub use appearance::{GlassPalette, Shell};
#[cfg(feature = "capture")]
pub use ui::{
    CaptureLayout, MainView, open_capture_window, seed_playlist_capture,
    seed_playlist_source_capture,
};
#[cfg(feature = "capture")]
pub use visual::{
    PatternId, PatternSelection, QualityChoice, SurfaceAppearance, VisualFlags, VisualSettings,
};
