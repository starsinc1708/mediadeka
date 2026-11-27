use gpui::{Context, Window};
use guise::IconName;

use crate::{
    music::{ObjectKind, ObjectRef},
    playlist::{ArtistCredit, Media, PlaylistKey, PlaylistTrack},
    providers::ProviderId,
    ui::workspace::{LocalTrackTarget, MusicTarget, Route, Target},
};

use super::{MainView, OpenPlaylist};

#[derive(Clone)]
pub(in crate::ui) enum Destination {
    Object(MusicTarget),
    LocalTrack(Box<LocalTrackTarget>),
    Playlist(Target),
    Resolve(RelationTarget),
    Search(ObjectKind, String),
}

#[derive(Clone)]
pub(in crate::ui) struct RelationTarget {
    pub(in crate::ui) origin: MusicTarget,
    pub(in crate::ui) kind: ObjectKind,
    pub(in crate::ui) name: String,
}

impl Destination {
    pub(in crate::ui) fn accessible_name(&self, name: &str) -> String {
        match self {
            Self::Object(target) => format!("Открыть · {} · {name}", target.object.kind.label()),
            Self::LocalTrack(_) => format!("Открыть · Трек · {name}"),
            Self::Playlist(_) => format!("Открыть плейлист · {name}"),
            Self::Resolve(target) => format!("Открыть · {} · {name}", target.kind.label()),
            Self::Search(kind, _) => format!("Искать · {} · {name}", kind.label()),
        }
    }
}

pub(super) fn row_prefetch_targets<'a>(
    key: &'a PlaylistKey,
    track: &'a PlaylistTrack,
) -> impl Iterator<Item = Destination> + 'a {
    album(key, track)
        .into_iter()
        .chain(artists(key, track).take(1))
        .map(|(_, destination)| destination)
}

#[derive(Clone)]
pub(in crate::ui) struct TrackLink {
    pub(in crate::ui) label: String,
    pub(in crate::ui) glyph: IconName,
    pub(in crate::ui) destination: Destination,
}

pub(in crate::ui) fn track_links<'a>(
    key: &'a PlaylistKey,
    track: &'a PlaylistTrack,
) -> impl Iterator<Item = TrackLink> + 'a {
    let recording = track_destination(key, track).map(|destination| TrackLink {
        label: "Открыть страницу трека".into(),
        glyph: IconName::AudioLines,
        destination,
    });
    let artists = artists(key, track).map(|(name, destination)| TrackLink {
        label: context_label(&destination, ObjectKind::Artist, name),
        glyph: IconName::UserRound,
        destination,
    });
    let album = album(key, track).map(|(name, destination)| TrackLink {
        label: context_label(&destination, ObjectKind::Album, name),
        glyph: IconName::ListMusic,
        destination,
    });
    recording.into_iter().chain(artists).chain(album)
}

fn context_label(destination: &Destination, kind: ObjectKind, name: &str) -> String {
    if matches!(destination, Destination::Search(..)) {
        format!("Искать · {name}")
    } else {
        format!("{} · {name}", kind.label())
    }
}

pub(in crate::ui) fn related(
    key: &PlaylistKey,
    kind: ObjectKind,
    name: &str,
    reference: Option<&ObjectRef>,
) -> Option<Destination> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some(
        match reference.filter(|object| object.kind == kind && object.is_valid()) {
            Some(object) => Destination::Object(target(key, object.clone(), name, None)),
            None => Destination::Search(kind, name.to_owned()),
        },
    )
}

pub(super) fn owner(open: &OpenPlaylist) -> Option<Destination> {
    let owner = open.playlist()?.owner.as_ref()?;
    let reference = owner.reference()?;
    let key = &open.target.key;
    let name = owner.name().trim();
    (Some(reference.provider) == key.provider()
        && reference.kind == ObjectKind::Artist
        && reference.is_valid()
        && !name.is_empty())
    .then(|| Destination::Object(target(key, reference.clone(), name, None)))
}

pub(in crate::ui) fn track_target(key: &PlaylistKey, track: &PlaylistTrack) -> Option<MusicTarget> {
    if let Some(provider) = key.provider()
        && let Some(object) = ObjectRef::native(provider, ObjectKind::Track, track.id.clone())
            .or_else(|| {
                ObjectRef::from_url(&track.id).filter(|object| {
                    object.provider == provider && object.kind == ObjectKind::Track
                })
            })
    {
        return Some(target(key, object, &track.title, track.artwork.clone()));
    }
    let object = match &track.media {
        Some(Media::Provider { provider, id }) => ObjectRef {
            provider: *provider,
            kind: ObjectKind::Track,
            id: id.clone(),
        },
        Some(Media::Url(url)) => {
            ObjectRef::from_url(url).filter(|object| object.kind == ObjectKind::Track)?
        }
        Some(Media::File(_)) => return None,
        None => return None,
    };
    object
        .is_valid()
        .then(|| target(key, object, &track.title, track.artwork.clone()))
}

pub(in crate::ui) fn track_destination(
    key: &PlaylistKey,
    track: &PlaylistTrack,
) -> Option<Destination> {
    if let Some(target) = track_target(key, track) {
        Some(Destination::Object(target))
    } else if matches!(&track.media, Some(Media::File(_) | Media::Url(_)))
        || (matches!(key, PlaylistKey::Local(_)) && !track.id.is_empty())
    {
        Some(Destination::LocalTrack(Box::new(LocalTrackTarget {
            key: key.clone(),
            track: track.clone(),
        })))
    } else {
        None
    }
}

pub(in crate::ui) fn collection(key: &PlaylistKey) -> Option<Destination> {
    match key {
        PlaylistKey::Local(id) if *id > 0 => {
            Some(Destination::Playlist(Target::local(*id, "Плейлист".into())))
        }
        PlaylistKey::Local(_) => None,
        PlaylistKey::Station {
            provider,
            profile,
            id,
        } => {
            let object = ObjectRef::native(*provider, ObjectKind::Station, id.clone())?;
            Some(Destination::Object(MusicTarget {
                object,
                profile: profile.clone(),
                title: String::new(),
                artwork: None,
            }))
        }
        PlaylistKey::Source {
            provider,
            profile,
            id,
        } => {
            if let Some((kind, id)) = id.split_once(':')
                && let Some(kind) = match kind {
                    "album" => Some(ObjectKind::Album),
                    "artist" => Some(ObjectKind::Artist),
                    "track" => Some(ObjectKind::Track),
                    _ => None,
                }
            {
                let object = ObjectRef::native(*provider, kind, id.to_owned())?;
                if kind == ObjectKind::Track {
                    return None;
                }
                return Some(Destination::Object(MusicTarget {
                    object,
                    profile: profile.clone(),
                    title: String::new(),
                    artwork: None,
                }));
            }
            Some(Destination::Playlist(Target {
                key: key.clone(),
                name: "Плейлист".into(),
                artwork: None,
                link: None,
            }))
        }
    }
}

fn target(
    key: &PlaylistKey,
    object: ObjectRef,
    title: &str,
    artwork: Option<String>,
) -> MusicTarget {
    let profile = key
        .connected_source()
        .filter(|(provider, _)| *provider == object.provider)
        .and_then(|(_, profile)| profile.map(str::to_owned));
    MusicTarget {
        object,
        profile,
        title: title.to_owned(),
        artwork,
    }
}

pub(in crate::ui) fn credits<'a>(
    names: &'a [String],
    fallback: &'a str,
    credits: &'a [ArtistCredit],
) -> impl Iterator<Item = (&'a str, Option<&'a ObjectRef>)> {
    credits
        .iter()
        .map(|credit| (credit.name.as_str(), credit.reference.as_ref()))
        .chain(
            names
                .iter()
                .filter(|_| credits.is_empty())
                .map(|name| (name.as_str(), None)),
        )
        .chain(std::iter::once((fallback, None)).filter(|_| credits.is_empty() && names.is_empty()))
}

pub(in crate::ui) fn artists<'a>(
    key: &'a PlaylistKey,
    track: &'a PlaylistTrack,
) -> impl Iterator<Item = (&'a str, Destination)> {
    credits(&track.artists, &track.artist, &track.artist_credits).filter_map(
        move |(name, reference)| {
            related_to_track(key, track, ObjectKind::Artist, name, reference)
                .map(|destination| (name, destination))
        },
    )
}

pub(in crate::ui) fn album<'a>(
    key: &PlaylistKey,
    track: &'a PlaylistTrack,
) -> Option<(&'a str, Destination)> {
    let legacy = || {
        let object = track_target(key, track)?.object;
        if object.provider != ProviderId::YandexMusic {
            return None;
        }
        let (_, album) = object.id.split_once(':')?;
        ObjectRef::native(object.provider, ObjectKind::Album, album.to_owned())
    };
    let stored = track
        .album_ref
        .as_ref()
        .filter(|object| object.kind == ObjectKind::Album && object.is_valid());
    let inferred = stored.is_none().then(legacy).flatten();
    let reference = stored.or(inferred.as_ref());
    let name = track
        .album
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| reference.map(|_| ObjectKind::Album.label()))?;
    related_to_track(key, track, ObjectKind::Album, name, reference)
        .map(|destination| (name, destination))
}

fn related_to_track(
    key: &PlaylistKey,
    track: &PlaylistTrack,
    kind: ObjectKind,
    name: &str,
    reference: Option<&ObjectRef>,
) -> Option<Destination> {
    let destination = related(key, kind, name, reference)?;
    if let Destination::Search(_, name) = &destination
        && let Some(origin) = track_target(key, track)
    {
        return Some(Destination::Resolve(RelationTarget {
            origin,
            kind,
            name: name.clone(),
        }));
    }
    Some(destination)
}

impl MainView {
    pub(in crate::ui) fn follow_playlist_link(
        &mut self,
        destination: Destination,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(&destination, Destination::Resolve(_)) {
            self.suspend_panel_for_entity(window, cx);
        }
        match destination {
            Destination::Object(target) => self.open_music_target(target, window, cx),
            Destination::LocalTrack(target) => self.navigate(Route::LocalTrack(target), window, cx),
            Destination::Playlist(target) => self.show_playlist(target, window, cx),
            Destination::Resolve(target) => self.open_related_music(target, window, cx),
            Destination::Search(kind, query) => self.submit_catalog(kind, query, window, cx),
        }
    }
}
