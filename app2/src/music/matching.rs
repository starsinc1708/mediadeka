use std::collections::HashMap;

use crate::{
    providers::ProviderId,
    storage::{Store, StoreError},
};

use super::{ObjectKind, ObjectRef, likes::LikeIdentity, service::confirmed_music_pairs};

#[derive(Default, Eq, PartialEq)]
pub(crate) struct MatchLinks {
    pub native: Vec<(ObjectRef, ObjectRef)>,
    pub choices: Vec<(LikeIdentity, LikeIdentity)>,
}

impl MatchLinks {
    pub(super) fn load(store: &Store, region: &str, kind: ObjectKind) -> Result<Self, StoreError> {
        Ok(Self {
            native: confirmed_music_pairs(store, region, kind)?,
            choices: if kind == ObjectKind::Track {
                store.confirmed_track_pairs()?
            } else {
                Vec::new()
            },
        })
    }

    /// Every member maps to the index of the first member of its confirmed group.
    pub(super) fn groups(
        &self,
        members: impl IntoIterator<Item = (Option<ObjectRef>, Option<LikeIdentity>)>,
    ) -> Vec<usize> {
        let mut objects = HashMap::new();
        let mut identities = HashMap::new();
        let mut parents = Vec::new();
        for (object, identity) in members {
            let index = parents.len();
            parents.push(index);
            if let Some(identity) = identity
                && let Some(previous) = identities.insert(identity, index)
            {
                join(&mut parents, previous, index);
            }
            if let Some(object) = object.filter(|object| object.provider != ProviderId::Jellyfin)
                && let Some(previous) = objects.insert(native_key(object), index)
            {
                join(&mut parents, previous, index);
            }
        }
        let member_count = parents.len();
        for (a, b) in &self.native {
            // Jellyfin IDs are server-scoped. Only explicitly scoped choices can link them.
            if a.provider == ProviderId::Jellyfin || b.provider == ProviderId::Jellyfin {
                continue;
            }
            let mut node = |object: &ObjectRef| {
                *objects
                    .entry(native_key(object.clone()))
                    .or_insert_with(|| {
                        let index = parents.len();
                        parents.push(index);
                        index
                    })
            };
            let a = node(a);
            let b = node(b);
            join(&mut parents, a, b);
        }
        for (a, b) in &self.choices {
            let mut node = |identity: &LikeIdentity| {
                *identities.entry(identity.clone()).or_insert_with(|| {
                    let index = parents.len();
                    parents.push(index);
                    if let Some(object) = identity
                        .reference()
                        .filter(|object| object.provider != ProviderId::Jellyfin)
                        && let Some(previous) = objects.insert(native_key(object), index)
                    {
                        join(&mut parents, previous, index);
                    }
                    index
                })
            };
            let a = node(a);
            let b = node(b);
            join(&mut parents, a, b);
        }
        (0..member_count)
            .map(|index| root(&mut parents, index))
            .collect()
    }
}

fn native_key(mut object: ObjectRef) -> ObjectRef {
    if object.provider == ProviderId::YandexMusic
        && object.kind == ObjectKind::Track
        && let Some((id, _)) = object.id.split_once(':')
    {
        object.id = id.to_owned();
    }
    object
}

fn root(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        parents[index] = parents[parents[index]];
        index = parents[index];
    }
    index
}

fn join(parents: &mut [usize], a: usize, b: usize) {
    let a = root(parents, a);
    let b = root(parents, b);
    parents[a.max(b)] = a.min(b);
}
