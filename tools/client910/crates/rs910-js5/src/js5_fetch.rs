//! Single-file fetch over the pack. Split out of
//! client910's `iface` (Phase 3.1): the font, sprite and defaults loaders of
//! every layer read single files through it, so it lives with the cache.

/// Single-file fetch over [`crate::cache::Pack`]:
/// single-group archives read group 0 / file `id`, otherwise group `id` /
/// file 0. Used for both `fontmetrics` and
/// `sprites`.
pub fn fetch_file(
    pack: &crate::cache::Pack,
    archive: &str,
    id: u32,
) -> anyhow::Result<Option<Vec<u8>>> {
    let index = pack
        .read_archive_index(archive)
        .map_err(|err| anyhow::anyhow!("fetch {archive} index: {err}"))?;
    let single_group = index.group_id.len() == 1;
    let (group, file) = if single_group { (0, id) } else { (id, 0) };
    match pack.read_group(archive, group) {
        Ok(files) => Ok(files.get(&file).cloned()),
        Err(crate::cache::CacheError::GroupMissing { .. })
        | Err(crate::cache::CacheError::UnknownGroup { .. }) => Ok(None),
        Err(err) => Err(anyhow::anyhow!("fetch {archive} group {group}: {err}")),
    }
}
