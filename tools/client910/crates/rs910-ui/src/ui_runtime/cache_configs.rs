//! The config decoders `Engine::load_cache` installs
//! (`SETUP_CONFIG_DECODERS`,).
use super::Engine;
use crate::cache::Pack;
use anyhow::Result;
use std::collections::BTreeMap;

/// The cache decodes [`Engine::load_cache_configs`] installs, as owned `Send`
/// data so the loading stage can decode them off the UI thread while the
/// loading screen keeps drawing (the loading screen renderer runs on its own
/// thread).
pub struct CacheConfigs {
    quickchat: crate::ui_social::QuickChatStore,
    wordpack: Result<crate::wordpack::Huffman>,
    inv_varbits: Result<std::sync::Arc<crate::scenery_varbits::Inputs>>,
    /// `show_single_option_menu != 0` and
    graphics: Result<(bool, i32)>,
    inv: Result<crate::config::InvStore>,
    bas: Result<BTreeMap<i32, crate::protocol910::bas_types::Bas>>,
    params: Result<
        Vec<(
            i32,
            std::result::Result<native910::config::ParamConfig, String>,
        )>,
    >,
    cam2_default: Result<bool>,
    objs: Result<crate::config::ObjStore>,
    locs: Result<crate::config::LocStore>,
    npcs: Result<crate::config::NpcStore>,
    seqs: Result<crate::config::SeqStore>,
    minimenu: Result<crate::ui_defaults::MiniMenuDefaults>,
    db: Result<crate::ui_db::Tables>,
    skills: Result<crate::ui_stats::SkillDefaults>,
}

impl CacheConfigs {
    pub fn decode(pack: &Pack) -> Self {
        Self::decode_with(pack, rs910_config::login_configs::LoginConfigs::read(pack))
    }

    /// [`Self::decode`] decoding the config archives `shared` has read (the
    /// game owner decoded its types from the same read) and taking its
    /// decoded locs and variable bindings.
    pub fn decode_with(pack: &Pack, shared: rs910_config::login_configs::LoginConfigs) -> Self {
        let graphics = (|| -> Result<_> {
            let bytes = crate::js5_fetch::fetch_file(pack, "defaults", 3)?
                .ok_or_else(|| anyhow::anyhow!("graphics defaults missing"))?;
            let defaults = crate::protocol910::defaults::Graphics::decode(&bytes)
                .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            Ok((
                defaults.value.scalars.show_single_option_menu != 0,
                defaults.value.scalars.performancemetricsmodel,
            ))
        })();
        let params = pack
            .read_group("config", 11)
            .map_err(anyhow::Error::from)
            .map(|files| {
                files
                    .iter()
                    .map(|(file_id, bytes)| {
                        (
                            *file_id as i32,
                            native910::config::decode_param(bytes).map_err(|e| format!("{e:?}")),
                        )
                    })
                    .collect()
            });
        Self {
            quickchat: crate::ui_social::QuickChatStore::load(pack),
            wordpack: crate::wordpack::Huffman::from_pack(pack),
            inv_varbits: shared.varbits,
            graphics,
            inv: crate::config::InvStore::load(pack),
            bas: crate::ui_bas::load(pack),
            params,
            cam2_default: crate::avatar::GraphicsDefaults::load(pack).map(|d| d.cam2_default),
            objs: decode_shared(&shared.objs, |r| {
                crate::config::ObjStore::from_records(&r.files)
            }),
            locs: shared.locs.map(|(store, _)| store),
            npcs: decode_shared(&shared.npcs, |r| {
                crate::config::NpcStore::from_records(&r.files)
            }),
            seqs: decode_shared(&shared.seqs, |r| {
                crate::config::SeqStore::from_records(&r.files)
            }),
            minimenu: crate::ui_defaults::MiniMenuDefaults::load(pack),
            db: crate::ui_db::Tables::load(pack),
            skills: crate::ui_stats::SkillDefaults::load(pack),
        }
    }
}

/// One interface store decoded from a shared archive read; a failed read is
/// the store's failure.
fn decode_shared<T>(
    records: &Result<crate::protocol910::pack_types::Records>,
    decode: fn(&crate::protocol910::pack_types::Records) -> Result<T>,
) -> Result<T> {
    match records {
        Ok(records) => decode(records),
        Err(error) => Err(anyhow::anyhow!("{error:#}")),
    }
}

impl Engine {
    /// Load cache-backed query tables (`INVTYPE` sizes + param defaults).
    /// Failures are logged; the corresponding commands stay terminal so the
    /// replay report names the missing owner instead of guessing zeros.
    pub fn load_cache(&mut self, pack: &Pack) {
        self.load_cache_configs(pack);
        self.load_world_map(pack);
    }
    /// The config decoders of [`Engine::load_cache`] (`SETUP_CONFIG_DECODERS`,
    /// The config decoders of [`Engine::load_cache`] (`SETUP_CONFIG_DECODERS`,
    pub fn load_cache_configs(&mut self, pack: &Pack) {
        self.install_cache_configs(CacheConfigs::decode(pack));
    }
    /// [`Self::load_cache`] decoding the config archives `shared` has read.
    pub fn load_cache_with(
        &mut self,
        pack: &Pack,
        shared: rs910_config::login_configs::LoginConfigs,
    ) {
        self.install_cache_configs(CacheConfigs::decode_with(pack, shared));
        self.load_world_map(pack);
    }
    /// Install decoded [`CacheConfigs`] (the cheap half of
    /// [`Self::load_cache_configs`]; the decode may run on a worker thread).
    pub fn install_cache_configs(&mut self, c: CacheConfigs) {
        self.configs.quickchat = Some(c.quickchat);
        match c.wordpack {
            Ok(huffman) => self.configs.wordpack = Some(huffman),
            Err(error) => log::warn!("[client910] chat Huffman coder unavailable: {error:#}"),
        }
        match c.inv_varbits {
            Ok(defs) => self.configs.inv_varbits = Some(defs),
            Err(error) => log::warn!("[client910] inventory varbits: {error:#}"),
        }
        match c.graphics {
            Ok((show_single_option_menu, performance_metrics_model)) => {
                self.menu.show_single_option_menu = show_single_option_menu;
                self.builtins.performance_metrics_model = Some(performance_metrics_model);
            }
            Err(error) => log::warn!("[client910] menu graphics defaults: {error:#}"),
        }
        match c.inv {
            Ok(store) => {
                self.configs.inv_sizes.clear();
                for id in 0..65536 {
                    if let Some(inv) = store.get(id) {
                        self.configs.inv_sizes.insert(id as i32, inv.size as i32);
                    }
                }
                self.game_host.inv_types = Some(store);
                // Sparse fallback: InvStore only holds present files; absent
                // ids stay missing and fail loudly in the trap.
            }
            Err(error) => log::warn!("[client910] inv types unavailable: {error:#}"),
        }
        match c.bas {
            Ok(defs) => self.configs.bas = Some(defs),
            Err(error) => log::warn!("[client910] BAS definitions unavailable: {error:#}"),
        }
        match c.params {
            Ok(params) => {
                for (file_id, entry) in params {
                    match entry {
                        Ok(entry) => {
                            self.configs.params.insert(file_id, entry);
                        }
                        Err(error) => {
                            log::warn!("[client910] param {} undecodable: {error}", file_id);
                        }
                    }
                }
            }
            Err(error) => log::warn!("[client910] param types unavailable: {error:#}"),
        }
        match c.cam2_default {
            Ok(default) => self.camera.cam2 = crate::ui_cam2::Cam2::new(default),
            Err(error) => log::warn!("[client910] graphics defaults unavailable: {error:#}"),
        }
        match c.objs {
            Ok(store) => self.configs.objs = Some(std::rc::Rc::new(store)),
            Err(error) => log::warn!("[client910] obj types unavailable: {error:#}"),
        }
        match c.locs {
            Ok(store) => self.configs.locs = Some(std::rc::Rc::new(store)),
            Err(error) => log::warn!("[client910] loc types unavailable: {error:#}"),
        }
        match c.npcs {
            Ok(store) => self.configs.npcs = Some(std::rc::Rc::new(store)),
            Err(error) => log::warn!("[client910] npc types unavailable: {error:#}"),
        }
        match c.seqs {
            Ok(store) => self.configs.seqs = Some(std::rc::Rc::new(store)),
            Err(error) => log::warn!("[client910] seq types unavailable: {error:#}"),
        }
        match c.minimenu {
            Ok(defaults) => self.configs.minimenu = Some(defaults),
            Err(error) => log::warn!("[client910] minimenu defaults unavailable: {error:#}"),
        }
        match c.db {
            Ok(tables) => self.configs.db = tables,
            Err(error) => log::warn!("[client910] db tables unavailable: {error:#}"),
        }
        match c.skills {
            Ok(skills) => self.configs.skills = Some(skills),
            Err(error) => log::warn!("[client910] skill definitions unavailable: {error:#}"),
        }
    }
}
