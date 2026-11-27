use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::{Context, Task, Window};

use crate::{
    music::entity_relations::resolve_track_relation,
    ui::{
        MainView,
        playlist::navigation::{Destination, RelationTarget},
        workspace::MusicTarget,
    },
};

pub(super) struct RelationRequest {
    cancelled: Arc<AtomicBool>,
    _task: Task<()>,
}

impl Drop for RelationRequest {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl MainView {
    pub(in crate::ui) fn open_related_music(
        &mut self,
        target: RelationTarget,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.music.relation = None;
        let cancelled = Arc::new(AtomicBool::new(false));
        let stopped = cancelled.clone();
        let task = cx.spawn_in(window, async move |view, cx| {
            let worker_flag = stopped.clone();
            let (object, target) = cx
                .background_executor()
                .spawn(async move {
                    let object = resolve_track_relation(
                        &target.origin.object,
                        target.origin.profile.as_deref(),
                        target.kind,
                        &target.name,
                        &worker_flag,
                    );
                    (object, target)
                })
                .await;
            if stopped.load(Ordering::Relaxed) {
                return;
            }
            let _ = view.update_in(cx, |view, window, cx| {
                view.music.relation = None;
                let destination = match object {
                    Some(object) => Destination::Object(MusicTarget {
                        object,
                        profile: target.origin.profile,
                        title: target.name,
                        artwork: None,
                    }),
                    None => Destination::Search(target.kind, target.name),
                };
                view.follow_playlist_link(destination, window, cx);
            });
        });
        self.music.relation = Some(RelationRequest {
            cancelled,
            _task: task,
        });
    }

    pub(in crate::ui) fn cancel_related_music(&mut self) {
        self.music.relation = None;
    }
}
