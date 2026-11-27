use rusqlite::{OptionalExtension, params};

use super::{Result, Store, StoreError};
use crate::{
    appearance::{Appearance, GlassPalette, Shell},
    visual::{
        AudioProfile, AudioResponse, PatternSelection, QualityChoice, SurfaceAppearance,
        VisualFlags, VisualSettings,
    },
};

pub(super) const MIGRATION: &str = "CREATE TABLE appearance (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    shell TEXT NOT NULL CHECK (shell IN ('fluid-glass', 'classic')),
    glass_palette TEXT NOT NULL CHECK (glass_palette IN
        ('auto', 'solar', 'abyss', 'ultraviolet', 'monochrome'))
) STRICT;";

/// Extends the palette list and adds independent pattern, effect and quality
/// fields. The old row keeps its shell and palette; visual defaults are added.
pub(super) const VISUAL_MIGRATION: &str = "ALTER TABLE appearance RENAME TO appearance_old;
    CREATE TABLE appearance (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        shell TEXT NOT NULL CHECK (shell IN ('fluid-glass', 'classic')),
        glass_palette TEXT NOT NULL CHECK (glass_palette IN
            ('auto', 'solar', 'abyss', 'ultraviolet', 'monochrome',
             'ember', 'acid', 'glacier', 'rosewood')),
        pattern TEXT NOT NULL DEFAULT 'auto' CHECK (pattern IN
            ('auto', 'thermal-flow', 'fluted-glass', 'interference',
             'signal-distortion', 'optical-caustics', 'light-leak',
             'gem-smoke', 'swirl', 'liquid-metal')),
        pattern_intensity INTEGER NOT NULL DEFAULT 65
            CHECK (pattern_intensity BETWEEN 0 AND 100),
        pattern_speed INTEGER NOT NULL DEFAULT 35
            CHECK (pattern_speed BETWEEN 0 AND 100),
        effects INTEGER NOT NULL DEFAULT 127
            CHECK (effects BETWEEN 0 AND 127),
        quality TEXT NOT NULL DEFAULT 'auto' CHECK (quality IN
            ('auto', 'ultra', 'high', 'balanced', 'eco', 'static'))
    ) STRICT;
    INSERT INTO appearance
        (id, shell, glass_palette, pattern, pattern_intensity, pattern_speed, effects, quality)
    SELECT id, shell, glass_palette, 'auto', 65, 35, 127, 'auto' FROM appearance_old;
    DROP TABLE appearance_old;";

const COLUMNS: &str =
    "shell, glass_palette, pattern, pattern_intensity, pattern_speed, effects, quality";
const AUDIO_COLUMNS: &str = "audio_enabled, audio_profile, audio_sensitivity, audio_channels";
const SURFACE_COLUMNS: &str = "surface_opacity, surface_blur, neutral_background";

pub(super) const AUDIO_RESPONSE_MIGRATION: &str = "
    ALTER TABLE appearance ADD COLUMN audio_enabled INTEGER NOT NULL DEFAULT 1
        CHECK (audio_enabled IN (0, 1));
    ALTER TABLE appearance ADD COLUMN audio_profile TEXT NOT NULL DEFAULT 'balanced'
        CHECK (audio_profile IN ('calm', 'balanced', 'immersive'));
    ALTER TABLE appearance ADD COLUMN audio_sensitivity INTEGER NOT NULL DEFAULT 65
        CHECK (audio_sensitivity BETWEEN 0 AND 100);
    ALTER TABLE appearance ADD COLUMN audio_channels INTEGER NOT NULL DEFAULT 63
        CHECK (audio_channels BETWEEN 0 AND 63);";

pub(super) const SURFACE_MIGRATION: &str = "
    ALTER TABLE appearance ADD COLUMN surface_opacity INTEGER NOT NULL DEFAULT 50
        CHECK (surface_opacity BETWEEN 0 AND 100);
    ALTER TABLE appearance ADD COLUMN surface_blur INTEGER NOT NULL DEFAULT 50
        CHECK (surface_blur BETWEEN 0 AND 100);
    ALTER TABLE appearance ADD COLUMN neutral_background INTEGER NOT NULL DEFAULT 0
        CHECK (neutral_background IN (0, 1));";

impl Store {
    pub(crate) fn appearance(&self) -> Result<Option<Appearance>> {
        let saved = self
            .connect()?
            .query_row(
                &format!("SELECT {COLUMNS}, {AUDIO_COLUMNS}, {SURFACE_COLUMNS} FROM appearance WHERE id = 1"),
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, bool>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, u8>(9)?,
                        row.get::<_, u8>(10)?,
                        SurfaceAppearance {
                            opacity: row.get(11)?,
                            blur: row.get(12)?,
                            neutral: row.get(13)?,
                        },
                    ))
                },
            )
            .optional()?;
        saved
            .map(
                |(
                    shell,
                    palette,
                    pattern,
                    intensity,
                    speed,
                    effects,
                    quality,
                    audio_enabled,
                    audio_profile,
                    sensitivity,
                    channels,
                    surface,
                )| {
                    Ok(Appearance {
                        shell: Shell::parse(&shell).ok_or(StoreError::InvalidData)?,
                        palette: GlassPalette::parse(&palette).ok_or(StoreError::InvalidData)?,
                        visual: VisualSettings {
                            pattern: PatternSelection::parse(&pattern)
                                .ok_or(StoreError::InvalidData)?,
                            intensity: intensity.clamp(0, 100) as f32 / 100.0,
                            speed: speed.clamp(0, 100) as f32 / 100.0,
                            effects: VisualFlags::from_bits(effects.clamp(0, 127) as u8),
                            quality: QualityChoice::parse(&quality)
                                .ok_or(StoreError::InvalidData)?,
                            audio: AudioResponse::from_stored(
                                audio_enabled,
                                AudioProfile::parse(&audio_profile)
                                    .ok_or(StoreError::InvalidData)?,
                                sensitivity,
                                channels,
                            )
                            .ok_or(StoreError::InvalidData)?,
                            surface,
                        },
                    })
                },
            )
            .transpose()
    }

    /// Writes only the shell. A concurrent palette or visual change from
    /// another window keeps its columns; the initial values are used only when
    /// the profile has no row yet.
    pub(crate) fn save_shell(&self, shell: Shell, initial_palette: GlassPalette) -> Result<()> {
        self.connect()?.execute(
            &format!(
                "INSERT INTO appearance (id, {COLUMNS}) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (id) DO UPDATE SET shell = excluded.shell"
            ),
            params![
                shell.as_str(),
                initial_palette.as_str(),
                "auto",
                65,
                35,
                127,
                "auto"
            ],
        )?;
        Ok(())
    }

    /// Writes only the palette.
    pub(crate) fn save_glass_palette(
        &self,
        palette: GlassPalette,
        initial_shell: Shell,
    ) -> Result<()> {
        self.connect()?.execute(
            &format!(
                "INSERT INTO appearance (id, {COLUMNS}) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (id) DO UPDATE SET glass_palette = excluded.glass_palette"
            ),
            params![
                initial_shell.as_str(),
                palette.as_str(),
                "auto",
                65,
                35,
                127,
                "auto"
            ],
        )?;
        Ok(())
    }

    /// Writes visual layers and audio response preferences together.
    pub(crate) fn save_visual(
        &self,
        visual: VisualSettings,
        initial_shell: Shell,
        initial_palette: GlassPalette,
    ) -> Result<()> {
        self.connect()?.execute(
            &format!(
                "INSERT INTO appearance (id, {COLUMNS}, {AUDIO_COLUMNS}, {SURFACE_COLUMNS})
                 VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT (id) DO UPDATE SET
                     pattern = excluded.pattern,
                     pattern_intensity = excluded.pattern_intensity,
                     pattern_speed = excluded.pattern_speed,
                     effects = excluded.effects,
                     quality = excluded.quality,
                     audio_enabled = excluded.audio_enabled,
                     audio_profile = excluded.audio_profile,
                     audio_sensitivity = excluded.audio_sensitivity,
                     audio_channels = excluded.audio_channels,
                     surface_opacity = excluded.surface_opacity,
                     surface_blur = excluded.surface_blur,
                     neutral_background = excluded.neutral_background"
            ),
            params![
                initial_shell.as_str(),
                initial_palette.as_str(),
                visual.pattern.as_str(),
                (visual.intensity.clamp(0., 1.) * 100.).round() as u8,
                (visual.speed.clamp(0., 1.) * 100.).round() as u8,
                visual.effects.bits(),
                visual.quality.as_str(),
                visual.audio.enabled,
                visual.audio.profile.as_str(),
                visual.audio.sensitivity(),
                visual.audio.channels(),
                visual.surface.opacity.min(100),
                visual.surface.blur.min(100),
                visual.surface.neutral,
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visual::PatternId;

    fn visual(pattern: PatternSelection, quality: QualityChoice) -> VisualSettings {
        VisualSettings {
            pattern,
            intensity: 0.72,
            speed: 0.41,
            effects: VisualFlags::from_bits(VisualFlags::GRAIN | VisualFlags::BLOOM),
            quality,
            audio: AudioResponse::from_stored(false, AudioProfile::Immersive, 21, 42).unwrap(),
            surface: SurfaceAppearance {
                opacity: 75,
                blur: 32,
                neutral: true,
            },
        }
    }

    #[test]
    fn the_saved_appearance_survives_a_restart_and_fields_do_not_clobber() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        assert_eq!(store.appearance().unwrap(), None);
        store
            .save_shell(Shell::Classic, GlassPalette::Auto)
            .unwrap();
        store
            .save_glass_palette(GlassPalette::Rosewood, Shell::FluidGlass)
            .unwrap();
        let expected = Appearance {
            shell: Shell::Classic,
            palette: GlassPalette::Rosewood,
            visual: VisualSettings::default(),
        };
        assert_eq!(
            Store::open(directory.path()).unwrap().appearance().unwrap(),
            Some(expected)
        );
        let chosen = Appearance {
            visual: visual(
                PatternSelection::Fixed(PatternId::LiquidMetal),
                QualityChoice::High,
            ),
            ..expected
        };
        store
            .save_visual(chosen.visual, Shell::FluidGlass, GlassPalette::Auto)
            .unwrap();
        assert_eq!(
            Store::open(directory.path()).unwrap().appearance().unwrap(),
            Some(chosen)
        );
        store
            .save_glass_palette(GlassPalette::Auto, Shell::FluidGlass)
            .unwrap();
        assert_eq!(
            Store::open(directory.path()).unwrap().appearance().unwrap(),
            Some(Appearance {
                palette: GlassPalette::Auto,
                ..chosen
            })
        );
    }

    #[test]
    fn an_unknown_stored_value_is_reported_as_damaged_data() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        store
            .connect()
            .unwrap()
            .execute_batch(
                "PRAGMA ignore_check_constraints = ON;
                 INSERT INTO appearance (id, shell, glass_palette)
                 VALUES (1, 'fluid-glass', 'unknown');",
            )
            .unwrap();
        assert!(matches!(store.appearance(), Err(StoreError::InvalidData)));
    }
}
