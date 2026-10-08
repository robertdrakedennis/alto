//! Disk adapter for the defaults archive: flat files by id, through the pack
//! reader. A group is never read as "its first file".
use crate::{
    cache::Pack,
    types910::defaults::{EntityDefaults, Graphics, Wear},
};
use anyhow::{Context, Result};
pub fn flat_file(pack: &Pack, id: u32) -> Result<Vec<u8>> {
    let index = pack.read_archive_index("defaults")?;
    let (group, file) = if index.group_id.last() == Some(&0) {
        (0, id)
    } else {
        anyhow::ensure!(index.group_id.contains(&id), "defaults group {id} absent");
        anyhow::ensure!(
            index.file_count_for_group(id)? == 1 && index.file_id_for_group_index(id, 0)? == 0,
            "defaults group {id} is not a flat file"
        );
        (id, 0)
    };
    pack.read_group("defaults", group)?
        .remove(&file)
        .with_context(|| format!("defaults {group}/{file} absent"))
}
pub fn load(pack: &Pack) -> Result<EntityDefaults> {
    let graphics_bytes = flat_file(pack, 3)?;
    let wear_bytes = flat_file(pack, 6)?;
    let graphics = Graphics::decode(&graphics_bytes)
        .map_err(|e| anyhow::anyhow!("graphics defaults: {e:?}"))?
        .value;
    let wear = Wear::decode(&wear_bytes)
        .map_err(|e| anyhow::anyhow!("wear defaults: {e:?}"))?
        .value;
    Ok(EntityDefaults {
        graphics,
        wear,
        graphics_bytes,
        wear_bytes,
    })
}
