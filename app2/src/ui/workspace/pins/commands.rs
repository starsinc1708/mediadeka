use gpui::Context;

use crate::storage::{NewPin, StoreError};

use super::{MainView, PinId, PinnedKind, PinnedObject, Pins, Store, pin_span};

const PIN_FAILURE: &str = "Не удалось изменить закреплённое.";

#[derive(Clone)]
pub(in crate::ui) struct PinRequest {
    pub(in crate::ui) id: PinId,
    pub(super) title: String,
    pub(super) artwork: Option<String>,
    pub(super) link: Option<String>,
}

impl PinRequest {
    pub(in crate::ui) fn with_title(mut self, title: &str) -> Self {
        self.title = title.to_owned();
        self
    }

    pub(in crate::ui::workspace) fn into_storage(self) -> NewPin {
        let remote = self.id.provider.is_some();
        NewPin {
            id: self.id,
            title: remote.then_some(self.title),
            artwork: if remote { self.artwork } else { None },
            link: if remote { self.link } else { None },
        }
    }
}

#[derive(Clone)]
pub(super) enum PinCommand {
    Pin(PinRequest),
    Unpin(PinId),
    Move(PinId, bool),
    MoveTo(PinId, i64),
}

impl PinCommand {
    fn toggle_id(&self) -> Option<&PinId> {
        match self {
            Self::Pin(pin) => Some(&pin.id),
            Self::Unpin(id) => Some(id),
            Self::Move(..) | Self::MoveTo(..) => None,
        }
    }

    fn project(&self, items: &mut Vec<PinnedObject>) {
        match self {
            Self::Pin(pin) if items.iter().all(|item| item.id != pin.id) => {
                items.push(PinnedObject {
                    id: pin.id.clone(),
                    title: pin.title.clone(),
                    artwork: pin.artwork.clone(),
                    link: pin.link.clone(),
                    position: items.len() as i64,
                });
            }
            Self::Unpin(id) => items.retain(|pin| &pin.id != id),
            _ => {}
        }
    }

    fn execute(self) -> (Result<(), StoreError>, Option<Vec<PinnedObject>>) {
        let store = match Store::open_default() {
            Ok(store) => store,
            Err(error) => return (Err(error), None),
        };
        let result = match self {
            Self::Pin(pin) => store.pin_object(&pin.into_storage()),
            Self::Unpin(id) => store.unpin_object(&id),
            Self::Move(id, up) => store.move_pin(&id, up),
            Self::MoveTo(id, position) => store.move_pin_to(&id, position),
        };
        let items = match store.pinned_objects() {
            Ok(items) => Some(items),
            Err(error) => {
                eprintln!("Не удалось прочитать закреплённое Mediadeka: {error}");
                None
            }
        };
        (result.map(|_| ()), items)
    }
}

impl Pins {
    fn projected(&self) -> Vec<PinnedObject> {
        let mut items = self.committed.clone();
        for command in &self.pending {
            command.project(&mut items);
        }
        for (index, pin) in items.iter_mut().enumerate() {
            pin.position = index as i64;
        }
        items
    }
}

impl MainView {
    pub(in crate::ui) fn toggle_pin(&mut self, pin: PinRequest, cx: &mut Context<Self>) {
        if self.pin_pending(&pin.id) {
            return;
        }
        let command = if self.pins.contains(&pin.id) {
            PinCommand::Unpin(pin.id)
        } else {
            PinCommand::Pin(pin)
        };
        self.queue_pin(command, cx);
    }

    fn pin_pending(&self, id: &PinId) -> bool {
        self.pins
            .pending
            .iter()
            .any(|command| command.toggle_id() == Some(id))
    }

    pub(in crate::ui) fn pin_status(&self, id: &PinId) -> Option<&'static str> {
        self.pin_pending(id)
            .then_some("Сохраняем изменение закрепления…")
    }

    pub(super) fn queue_pin(&mut self, command: PinCommand, cx: &mut Context<Self>) {
        self.player
            .update(cx, |player, cx| player.clear_notice(PIN_FAILURE, cx));
        let departing = match &command {
            PinCommand::Unpin(id) if id.kind == PinnedKind::Playlist => Some(id.clone()),
            _ => None,
        };
        let toggle = command.toggle_id().is_some();
        self.pins.pending.push_back(command);
        if toggle {
            let items = self.pins.projected();
            if let Some(id) = departing.as_ref()
                && self.uses_fluid_glass()
                && !self.reduced_motion
                && !cx.reduce_motion()
            {
                self.pins.animate_departure(id, &items);
            }
            self.pins.replace(items, cx);
            cx.notify();
        }
        if self.pins.write.is_some() {
            return;
        }

        // A startup read must not overwrite a more recent committed change.
        self.pins.load = None;
        let task = cx.spawn(async move |view, cx| {
            while let Ok(Some(next)) = view.update(cx, |view, _| view.pins.pending.front().cloned())
            {
                let (result, items) = cx
                    .background_executor()
                    .spawn(async move { next.execute() })
                    .await;
                if view
                    .update(cx, |view, cx| view.finish_pin_change(result, items, cx))
                    .is_err()
                {
                    return;
                }
            }
            let _ = view.update(cx, |view, cx| {
                view.pins.write = None;
                if std::mem::take(&mut view.pins.refresh_after_write) {
                    view.refresh_pins(cx);
                }
            });
        });
        self.pins.write = Some(task);
    }

    fn finish_pin_change(
        &mut self,
        result: Result<(), StoreError>,
        items: Option<Vec<PinnedObject>>,
        cx: &mut Context<Self>,
    ) {
        let command = self
            .pins
            .pending
            .pop_front()
            .expect("a queued pin command completed");
        if let Some(items) = items {
            self.pins.committed = items;
        } else {
            if result.is_ok() {
                // The write committed. Reuse that known change until the full snapshot can be read.
                command.project(&mut self.pins.committed);
            }
            self.pins.refresh_after_write = true;
        }
        if let Err(error) = result {
            eprintln!("Не удалось изменить закреплённое Mediadeka: {error}");
            if let PinCommand::Unpin(id) = &command {
                self.pins.exits.forget(pin_span(id));
            }
            self.player.update(cx, |player, cx| {
                player.show_notice(PIN_FAILURE.into(), cx);
            });
        }
        self.pins.replace(self.pins.projected(), cx);
        cx.notify();
    }
}
