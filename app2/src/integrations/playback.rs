use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::{
    playback::{ListeningPhase, ListeningSample, SoundStarted},
    playlist::PlaylistTrack,
};

use super::{
    Preferences, Service,
    discord::{Activity, Discord},
    scrobble::Play,
    submission::{Delivery, Kind, Row},
};

const MAX_START_DRIFT_SECONDS: u64 = 2;

pub(super) struct Listening {
    play: Option<Current>,
    discord: Discord,
    shown: Option<(Presence, String)>,
}

impl Default for Listening {
    fn default() -> Self {
        Self {
            play: None,
            discord: Discord::new(),
            shown: None,
        }
    }
}

struct Current {
    generation: u64,
    track: PlaylistTrack,
    submission: Option<Play>,
    at: SystemTime,
    sample: Option<ListeningSample>,
    reports: [Reports; Service::ALL.len()],
    timing: Option<(u64, i64)>,
}

#[derive(Default)]
struct Reports {
    current: bool,
    earned: bool,
}

#[derive(Clone, Copy, PartialEq)]
struct Presence {
    generation: u64,
    private: bool,
    timing: Option<(i64, Option<i64>)>,
}

impl Listening {
    pub(super) fn started(&mut self, sound: &SoundStarted) {
        let track = sound.entry.track.clone();
        let at = SystemTime::now();
        let title = track.title.trim();
        let artist = track.artist.trim();
        let submission = (!title.is_empty() && !artist.is_empty()).then(|| Play {
            title: title.to_owned(),
            artist: artist.to_owned(),
            album: track.album.clone(),
            duration: Duration::ZERO,
            at,
        });
        self.play = Some(Current {
            generation: sound.generation,
            track,
            submission,
            at,
            sample: None,
            reports: Default::default(),
            timing: None,
        });
    }

    pub(super) fn sample(&mut self, sample: Option<ListeningSample>) {
        if let Some(current) = &mut self.play {
            current.sample = sample.filter(|sample| sample.generation == current.generation);
        }
    }

    pub(super) fn reconcile(
        &mut self,
        preferences: &Preferences,
        rows: &mut [Row],
        deliveries: &async_channel::Sender<Delivery>,
    ) {
        let mut wanted = None;
        if let Some(current) = &mut self.play
            && let Some(sample) = current.sample
        {
            current.report(sample, rows, deliveries);
            wanted = current.presence(sample, preferences);
        }
        let application = &preferences.discord_application_id;
        let changed = match (&self.shown, wanted) {
            (Some((previous, id)), Some(wanted)) => *previous != wanted || id != application,
            (None, None) => false,
            _ => true,
        };
        if changed {
            let activity = wanted
                .as_ref()
                .zip(self.play.as_ref())
                .map(|(presence, current)| {
                    if presence.private {
                        Activity::anonymous(
                            application,
                            presence.timing.map(|(started, _)| started),
                        )
                    } else {
                        Activity::track(application, &current.track, presence.timing)
                    }
                });
            self.discord.publish(activity);
            self.shown = wanted.map(|presence| (presence, application.clone()));
        }
    }
}

impl Current {
    fn report(
        &mut self,
        sample: ListeningSample,
        rows: &mut [Row],
        deliveries: &async_channel::Sender<Delivery>,
    ) {
        if !rows.iter().any(Row::enabled) {
            return;
        }
        let Some(play) = &mut self.submission else {
            return;
        };
        play.duration = sample
            .duration
            .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
            .unwrap_or_default();
        let earned = play.earned(sample.listened);
        for (row, reports) in rows
            .iter_mut()
            .zip(&mut self.reports)
            .filter(|(row, _)| row.enabled())
        {
            if matches!(
                sample.phase,
                ListeningPhase::Playing | ListeningPhase::Paused
            ) && !reports.current
            {
                reports.current = row.dispatch(Kind::NowPlaying, play, deliveries);
            }
            if earned && reports.current && !reports.earned {
                reports.earned = row.dispatch(Kind::Scrobble, play, deliveries);
            }
        }
    }

    fn presence(&mut self, sample: ListeningSample, preferences: &Preferences) -> Option<Presence> {
        if sample.phase != ListeningPhase::Playing {
            self.timing = None;
        }
        if !preferences.discord_enabled
            || !matches!(
                sample.phase,
                ListeningPhase::Playing | ListeningPhase::Paused
            )
            || sample.phase == ListeningPhase::Paused && !preferences.discord_paused
        {
            return None;
        }
        let timing = (sample.phase == ListeningPhase::Playing).then(|| {
            if preferences.discord_private {
                return (unix_seconds(self.at), None);
            }
            let measured = unix_seconds(SystemTime::now()).saturating_sub(sample.position as i64);
            let started = match self.timing {
                Some((revision, at))
                    if revision == sample.seek_revision
                        && at.abs_diff(measured) <= MAX_START_DRIFT_SECONDS =>
                {
                    at
                }
                _ => measured,
            };
            self.timing = Some((sample.seek_revision, started));
            let ends = sample
                .duration
                .filter(|seconds| *seconds > 0.)
                .map(|seconds| started.saturating_add(seconds as i64));
            (started, ends)
        });
        Some(Presence {
            generation: sample.generation,
            private: preferences.discord_private,
            timing,
        })
    }
}

fn unix_seconds(time: SystemTime) -> i64 {
    i64::try_from(
        time.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX)
}
