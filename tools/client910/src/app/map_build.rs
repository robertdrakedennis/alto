//! Owned CPU scene construction, with one active build and one replaceable
//! request. The worker prepares the live scene and animation assets; current
//! variables, animation activation and acknowledgment remain on the client owner
//! after generation validation.
use super::startup::{build_game_scene, install_built_game_scene, BuiltGameScene};
use super::*;

type Completion = std::sync::mpsc::Receiver<anyhow::Result<BuiltGameScene>>;

struct Request {
    session: u64,
    map: rs910_game::entity_runtime::MapRequest,
    prefs: rs910_scene::rebuild::BuildPrefs,
    members: bool,
    event: rs910_client::session::RebuildEvent,
}

#[derive(Default)]
pub(super) struct MapBuild {
    active: Option<(Request, Completion)>,
    queued: Option<Request>,
}

/// Packet arrays retain padding; prefetch events contain only active groups
/// and may order them differently. Both describe the same requested set.
fn groups_match(groups: &[i32], count: usize, required: &[u16]) -> bool {
    let current: std::collections::BTreeSet<_> = groups.iter().take(count).copied().collect();
    let required: std::collections::BTreeSet<_> = required.iter().copied().map(i32::from).collect();
    current == required
}

impl ViewerApp {
    pub(super) fn queue_scene_build(&mut self, event: rs910_client::session::RebuildEvent) {
        let Some(session) = self.core.session.as_ref() else {
            return;
        };
        let Some(game) = session.game.as_ref() else {
            return;
        };
        let Some(map) = game.runtime.map_request.as_ref() else {
            return;
        };
        // A superseded map-load response must never start a build of an unrelated
        // current request or acknowledge the old event as the rendered map.
        if map.world.region_x != i32::from(event.rebuild.zone_x)
            || map.world.region_z != i32::from(event.rebuild.zone_z)
            || !groups_match(&map.world.groups, map.world.group_count, &event.groups)
        {
            return;
        }
        self.map_build.queued = Some(Request {
            session: session.instance_id,
            map: map.clone(),
            prefs: rs910_scene::rebuild::BuildPrefs::from_options(
                &game.ui_variables.queries.preferences.options,
            ),
            members: game.allow_members,
            event,
        });
        self.begin_rebuild_build();
        self.poll_scene_build();
    }

    pub(super) fn poll_scene_build(&mut self) {
        let completed =
            self.map_build.active.as_ref().and_then(|(_, completion)| {
                match completion.try_recv() {
                    Ok(result) => Some(result),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => {
                        Some(Err(anyhow::anyhow!("scene worker disconnected")))
                    }
                }
            });
        if let Some(completed) = completed {
            let (request, _) = self.map_build.active.take().expect("completed scene build");
            let current = self.core.session.as_ref().is_some_and(|session| {
                session.instance_id == request.session
                    && session.game.as_ref().is_some_and(|game| {
                        game.runtime.map_request.as_ref().is_some_and(|map| {
                            map.generation == request.map.generation
                                && map.world == request.map.world
                        })
                    })
            });
            if current {
                let preferences_current = self.core.session.game().is_some_and(|game| {
                    request.prefs
                        == rs910_scene::rebuild::BuildPrefs::from_options(
                            &game.ui_variables.queries.preferences.options,
                        )
                        && request.members == game.allow_members
                });
                if !preferences_current {
                    self.queue_scene_build(request.event);
                    return;
                }
                match completed {
                    Ok(built) => {
                        let mut assets = WorldAssets::synthetic().expect("empty scene assets");
                        let installed = install_built_game_scene(
                            self.core.session.game_mut().expect("current game"),
                            &self.pack,
                            &mut assets,
                            built,
                        );
                        match installed {
                            Ok(prepared) => {
                                self.core
                                    .session
                                    .as_mut()
                                    .expect("current session")
                                    .prepared_map = Some(prepared);
                                self.apply_reloaded_assets(assets, &request.event);
                                match self.finish_game_map() {
                                    Ok(()) => self.leave_rebuild_state(),
                                    Err(error) => {
                                        log::warn!("[client910] map not acknowledged: {error:#}")
                                    }
                                }
                                if let Some(session) = self.core.session.as_mut() {
                                    if session.prefetch_loading.as_ref().is_some_and(|loading| {
                                        loading.groups == request.event.groups
                                    }) {
                                        session.prefetch_loading = None;
                                    }
                                }
                            }
                            Err(error) => log::warn!(
                                "[client910] map installation retained for retry: {error:#}"
                            ),
                        }
                    }
                    Err(error) => log::warn!("[client910] scene build failed: {error:#}"),
                }
            }
        }
        if self.map_build.active.is_none() {
            if let Some(request) = self.map_build.queued.take() {
                let pack = self.pack.clone();
                let map = request.map.clone();
                let prefs = request.prefs;
                let members = request.members;
                let (sender, receiver) = mpsc::channel();
                let spawned = std::thread::Builder::new()
                    .name("client-scene-build".into())
                    .spawn(move || {
                        let result = build_game_scene(&pack, &map, &prefs, members);
                        let _ = sender.send(result);
                    });
                match spawned {
                    Ok(_) => self.map_build.active = Some((request, receiver)),
                    Err(error) => log::warn!("[client910] scene worker could not start: {error}"),
                }
            }
        }
    }
}
