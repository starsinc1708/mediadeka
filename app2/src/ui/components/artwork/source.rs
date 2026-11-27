use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use gpui::Image;
use image::DynamicImage;

use super::{
    ArtworkSource, decode,
    disk::{self, Rendition, SourceRecord},
    http::{self, Response, Validators},
};

#[derive(Clone)]
pub(super) struct Revalidation {
    pub(super) path: PathBuf,
    pub(super) generation: u64,
    pub(super) receipt: disk::Receipt,
}

pub(super) struct Ready {
    pub(super) image: Image,
    pub(super) decoded: DynamicImage,
    pub(super) content: disk::ContentId,
    pub(super) revalidation: Option<Revalidation>,
}

pub(super) enum Refresh {
    Same(Option<Revalidation>),
    Changed(Ready),
    Failed,
}

impl ArtworkSource {
    fn rendition(&self, rendition: Rendition) -> Option<&str> {
        match rendition {
            Rendition::Larger => self.larger.as_deref(),
            Rendition::Original => Some(&self.url),
        }
    }

    fn candidates(&self) -> impl Iterator<Item = (Rendition, &str)> {
        [Rendition::Larger, Rendition::Original]
            .into_iter()
            .filter_map(|variant| self.rendition(variant).map(|url| (variant, url)))
    }
}

pub(super) fn load(
    source: &ArtworkSource,
    profile: Option<&Path>,
    cancelled: &AtomicBool,
) -> Option<Ready> {
    let path =
        profile.and_then(|directory| disk::path(directory, &source.url, source.larger.as_deref()));
    let location = path
        .as_deref()
        .and_then(|path| Some((path, disk::generation(path)?)));
    let stored = path.as_deref().and_then(disk::read_entry);
    if let Some(entry) = &stored
        && let Some(decoded) = decode::decode(&entry.image)
    {
        return Some(Ready {
            image: entry.image.clone(),
            decoded,
            content: entry.receipt.content,
            revalidation: location.map(|(path, generation)| Revalidation {
                path: path.to_owned(),
                generation,
                receipt: entry.receipt.clone(),
            }),
        });
    }
    let prior = stored.as_ref().map(|entry| &entry.receipt);
    for (rendition, url) in source.candidates() {
        if cancelled.load(Ordering::Acquire) {
            return None;
        }
        let Some(Response::Image {
            image,
            validators,
            no_store,
        }) = http::download(url, None)
        else {
            continue;
        };
        let Some(decoded) = decode::decode(&image) else {
            continue;
        };
        let content = disk::image_content(&image)?;
        let revalidation = if let Some((path, generation)) = location {
            let source = (!no_store).then_some(SourceRecord {
                rendition,
                validators,
            });
            match disk::accept_image(path, generation, prior, &image, source, cancelled) {
                Ok(Some(receipt)) if !no_store => Some(Revalidation {
                    path: path.to_owned(),
                    generation,
                    receipt,
                }),
                Ok(Some(_)) | Err(_) => None,
                Ok(None) => return None,
            }
        } else {
            None
        };
        return Some(Ready {
            image,
            decoded,
            content,
            revalidation,
        });
    }
    None
}

pub(super) fn refresh(
    source: &ArtworkSource,
    before: &Revalidation,
    cancelled: &AtomicBool,
) -> Refresh {
    refresh_with(source, before, cancelled, http::download)
}

fn refresh_with(
    source: &ArtworkSource,
    before: &Revalidation,
    cancelled: &AtomicBool,
    fetch: impl Fn(&str, Option<&Validators>) -> Option<Response>,
) -> Refresh {
    if cancelled.load(Ordering::Acquire) {
        return Refresh::Failed;
    }
    if let Some(saved) = &before.receipt.source {
        let Some(url) = source.rendition(saved.rendition) else {
            return Refresh::Failed;
        };
        let Some(response) = fetch(url, Some(&saved.validators)) else {
            return Refresh::Failed;
        };
        return apply(before, saved.rendition, response, cancelled);
    }
    for (rendition, url) in source.candidates() {
        if let Some(response) = fetch(url, None) {
            let result = apply(before, rendition, response, cancelled);
            if !matches!(result, Refresh::Failed) {
                return result;
            }
        }
    }
    Refresh::Failed
}

fn apply(
    before: &Revalidation,
    rendition: Rendition,
    response: Response,
    cancelled: &AtomicBool,
) -> Refresh {
    if cancelled.load(Ordering::Acquire) {
        return Refresh::Failed;
    }
    let (path, generation, prior) = (&before.path, before.generation, &before.receipt);
    match response {
        Response::NotModified {
            validators,
            no_store,
        } => {
            match disk::accept_not_modified(
                path, generation, prior, validators, no_store, cancelled,
            ) {
                Ok(Some(receipt)) => Refresh::Same((!no_store).then(|| Revalidation {
                    path: path.clone(),
                    generation,
                    receipt,
                })),
                _ => Refresh::Failed,
            }
        }
        Response::Image {
            image,
            validators,
            no_store,
        } => {
            let Some(content) = disk::image_content(&image) else {
                return Refresh::Failed;
            };
            let decoded = if content == prior.content {
                None
            } else {
                let Some(decoded) = decode::decode(&image) else {
                    return Refresh::Failed;
                };
                Some(decoded)
            };
            let source = (!no_store).then_some(SourceRecord {
                rendition,
                validators,
            });
            let Ok(Some(receipt)) =
                disk::accept_image(path, generation, Some(prior), &image, source, cancelled)
            else {
                return Refresh::Failed;
            };
            let revalidation = (!no_store).then(|| Revalidation {
                path: path.clone(),
                generation,
                receipt,
            });
            match decoded {
                Some(decoded) => Refresh::Changed(Ready {
                    image,
                    decoded,
                    content,
                    revalidation,
                }),
                None => Refresh::Same(revalidation),
            }
        }
    }
}

#[cfg(test)]
mod tests;
