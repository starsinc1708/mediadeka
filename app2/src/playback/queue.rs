use super::queue_resume::{QueueResumePlan, RESUME_LIMIT, ResumeEntry};
use super::{QueueEntry, Repeat};

#[derive(Default)]
pub(super) struct Queue {
    entries: Vec<Entry>,
    order: Vec<usize>,
    cursor: Option<usize>,
    shuffle: bool,
    revision: u64,
    replacement_revision: u64,
    next_entry_id: usize,
}

struct Entry {
    data: QueueEntry,
    autoplay: bool,
    id: usize,
}

#[derive(Clone, Copy)]
pub(super) enum Placement {
    Next,
    ManualEnd,
    FeedEnd,
    AutoplayEnd,
}

impl Queue {
    pub(super) fn resume_plan(&self, start: usize) -> QueueResumePlan {
        if self.order.len() <= RESUME_LIMIT {
            let entries = self
                .entries
                .iter()
                .map(|entry| ResumeEntry {
                    entry: entry.data.clone(),
                    autoplay: entry.autoplay,
                })
                .collect();
            return QueueResumePlan::new(
                entries,
                self.order.clone(),
                self.shuffle,
                0,
                self.revision,
            );
        }
        let end = (start + RESUME_LIMIT).min(self.order.len());
        let order = &self.order[start..end];
        let mut canonical = order.to_vec();
        canonical.sort_unstable();
        let entries = canonical
            .iter()
            .map(|&index| {
                let entry = &self.entries[index];
                ResumeEntry {
                    entry: entry.data.clone(),
                    autoplay: entry.autoplay,
                }
            })
            .collect();
        let order = order
            .iter()
            .map(|index| {
                canonical
                    .binary_search(index)
                    .expect("snapshotted queue index")
            })
            .collect();
        QueueResumePlan::new(entries, order, self.shuffle, start, self.revision)
    }

    pub(super) fn restore(&mut self, plan: QueueResumePlan, cursor: Option<usize>) {
        self.entries.clear();
        for entry in plan.entries {
            let entry = self.entry(entry.entry, entry.autoplay);
            self.entries.push(entry);
        }
        self.order = plan.order;
        self.cursor = cursor;
        self.shuffle = plan.shuffle;
        self.revision = self.revision.wrapping_add(1);
        self.replacement_revision = self.revision;
    }

    pub(super) fn replace(&mut self, entries: Vec<QueueEntry>, selected: usize) {
        self.replace_entries(entries, Some(selected));
    }

    pub(super) fn replace_shuffled(&mut self, entries: Vec<QueueEntry>) {
        self.shuffle = true;
        self.replace_entries(entries, None);
        self.select(0);
    }

    fn replace_entries(&mut self, entries: Vec<QueueEntry>, selected: Option<usize>) {
        self.entries.clear();
        self.cursor = None;
        for (index, entry) in entries.into_iter().enumerate() {
            if !entry.is_playable() {
                continue;
            }
            if Some(index) == selected {
                self.cursor = Some(self.entries.len());
            }
            let entry = self.entry(entry, false);
            self.entries.push(entry);
        }
        self.order = (0..self.entries.len()).collect();
        if self.shuffle {
            // Keep a clicked entry first. Without an explicit entry, shuffle the whole queue.
            if let Some(selected) = self.cursor {
                self.order.swap(0, selected);
                self.cursor = Some(0);
            }
            self.reorder_upcoming();
        }
        self.revision = self.revision.wrapping_add(1);
        self.replacement_revision = self.revision;
    }

    pub(super) fn insert(&mut self, incoming: Vec<QueueEntry>, placement: Placement) {
        let upcoming = self.cursor.map_or(0, |cursor| cursor + 1);
        let end = self.order.len();
        let index = match placement {
            Placement::Next => upcoming,
            Placement::ManualEnd => self.order[upcoming..]
                .iter()
                .position(|&index| self.entries[index].autoplay)
                .map_or(end, |position| upcoming + position),
            Placement::FeedEnd | Placement::AutoplayEnd => end,
        };
        let incoming: Vec<_> = incoming
            .into_iter()
            .filter(QueueEntry::is_playable)
            .map(|data| self.entry(data, matches!(placement, Placement::AutoplayEnd)))
            .collect();
        if incoming.is_empty() {
            return;
        }
        let canonical = if matches!(placement, Placement::Next) {
            self.cursor.map_or(0, |cursor| self.order[cursor] + 1)
        } else {
            self.order.get(index).copied().unwrap_or(self.entries.len())
        };
        let insert_count = incoming.len();
        for item in &mut self.order {
            if *item >= canonical {
                *item += insert_count;
            }
        }
        self.entries.splice(canonical..canonical, incoming);
        self.order
            .splice(index..index, canonical..canonical + insert_count);
        self.revision = self.revision.wrapping_add(1);
    }

    fn entry(&mut self, data: QueueEntry, autoplay: bool) -> Entry {
        let id = self.next_entry_id;
        self.next_entry_id = id.wrapping_add(1);
        Entry { data, autoplay, id }
    }

    pub(super) fn at(&self, position: usize) -> Option<(usize, &QueueEntry)> {
        let entry = &self.entries[*self.order.get(position)?];
        Some((entry.id, &entry.data))
    }

    fn upcoming_entry(&self, revision: u64, id: usize, position: usize) -> Option<usize> {
        if self.revision != revision || self.cursor.is_some_and(|cursor| position <= cursor) {
            return None;
        }
        let entry = *self.order.get(position)?;
        (self.entries[entry].id == id).then_some(entry)
    }

    /// Reorder upcoming entries without moving the cursor or restarting playback.
    /// Commit the same relative placement to the canonical order, which is used
    /// when the listener later turns shuffle off.
    pub(super) fn move_upcoming(
        &mut self,
        revision: u64,
        id: usize,
        source: usize,
        target: usize,
    ) -> bool {
        let upcoming = self.cursor.map_or(0, |index| index + 1);
        if source == target || target < upcoming || target >= self.order.len() {
            return false;
        }

        let Some(from) = self.upcoming_entry(revision, id, source) else {
            return false;
        };
        let anchor = self.order[target];
        let to = (anchor - usize::from(from < anchor)) + usize::from(source < target);
        if from != to {
            let entry = self.entries.remove(from);
            self.entries.insert(to, entry);
            for index in &mut self.order {
                *index = if *index == from {
                    to
                } else if (from < *index) && (*index <= to) {
                    *index - 1
                } else if (to <= *index) && (*index < from) {
                    *index + 1
                } else {
                    *index
                };
            }
        }
        let index = self.order.remove(source);
        self.order.insert(target, index);
        self.revision = self.revision.wrapping_add(1);
        true
    }

    pub(super) fn remove_upcoming(&mut self, revision: u64, id: usize, position: usize) -> bool {
        let Some(entry) = self.upcoming_entry(revision, id, position) else {
            return false;
        };
        self.entries.remove(entry);
        self.order.remove(position);
        for index in &mut self.order {
            if *index > entry {
                *index -= 1;
            }
        }
        self.revision = self.revision.wrapping_add(1);
        true
    }

    pub(super) fn clear_upcoming(&mut self, revision: u64) -> bool {
        let retained = self.cursor.map_or(0, |cursor| cursor + 1);
        if self.revision != revision || retained == self.order.len() {
            return false;
        }

        // Keep the playback prefix and compact its canonical entries in their
        // original order. Shuffle can make the two orders different.
        self.order.truncate(retained);
        let mut remap = vec![None; self.entries.len()];
        for &index in &self.order {
            remap[index] = Some(0);
        }
        let mut next_index = 0;
        let mut indices = remap.iter_mut();
        self.entries.retain(|_| {
            let index = indices.next().expect("one mapping per canonical entry");
            if let Some(retained_index) = index {
                *retained_index = next_index;
                next_index += 1;
                true
            } else {
                false
            }
        });
        for index in &mut self.order {
            *index = remap[*index].expect("retained playback position");
        }
        self.revision = self.revision.wrapping_add(1);
        true
    }

    pub(super) fn entries(&self) -> impl ExactSizeIterator<Item = &QueueEntry> {
        self.order.iter().map(|&index| &self.entries[index].data)
    }

    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn next_entry_id(&self) -> usize {
        self.next_entry_id
    }

    pub(super) fn replacement_revision(&self) -> u64 {
        self.replacement_revision
    }

    pub(super) fn current(&self) -> Option<&QueueEntry> {
        self.cursor
            .map(|cursor| &self.entries[self.order[cursor]].data)
    }

    pub(super) fn current_index(&self) -> Option<usize> {
        self.cursor
    }

    pub(super) fn upcoming(&self, repeat: Repeat) -> impl Iterator<Item = &QueueEntry> {
        let start = self.cursor.map_or(0, |cursor| cursor + 1);
        let tail = self.order.iter().skip(start);
        let wrap = usize::from(repeat == Repeat::All) * self.cursor.unwrap_or_default();
        tail.chain(self.order.iter().take(wrap))
            .take(2)
            .filter(move |_| repeat != Repeat::One)
            .map(|&index| &self.entries[index].data)
    }

    pub(super) fn select(&mut self, index: usize) -> bool {
        if index >= self.order.len() {
            return false;
        }
        self.cursor = Some(index);
        true
    }

    pub(super) fn next(&self, repeat: bool) -> Option<usize> {
        let next = self.cursor.map_or(0, |cursor| cursor + 1);
        if next < self.order.len() {
            Some(next)
        } else {
            (repeat && !self.order.is_empty()).then_some(0)
        }
    }

    pub(super) fn previous(&self, repeat: bool) -> Option<usize> {
        self.cursor?.checked_sub(1).or_else(|| {
            if repeat {
                self.order.len().checked_sub(1)
            } else {
                None
            }
        })
    }

    pub(super) fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub(super) fn toggle_shuffle(&mut self) {
        self.shuffle = !self.shuffle;
        self.reorder_upcoming();
        self.revision = self.revision.wrapping_add(1);
    }

    fn reorder_upcoming(&mut self) {
        let start = self.cursor.map_or(0, |cursor| cursor + 1);
        let upcoming = &mut self.order[start..];
        if self.shuffle {
            let mut choices = vec![0; upcoming.len().saturating_sub(1) * size_of::<u64>()];
            if getrandom::fill(&mut choices).is_ok() {
                for (end, bytes) in (1..upcoming.len()).rev().zip(choices.as_chunks::<8>().0) {
                    let random = u64::from_le_bytes(*bytes);
                    upcoming.swap(end, (random % (end as u64 + 1)) as usize);
                }
            }
        } else {
            upcoming.sort_unstable();
        }
        if upcoming.iter().any(|&index| self.entries[index].autoplay) {
            upcoming.sort_by_key(|&index| self.entries[index].autoplay);
        }
    }
}
