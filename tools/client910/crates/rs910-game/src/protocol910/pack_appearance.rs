//! Fully sourced appearance context. Title enum IDs
//! come from customization defaults, not hardcoded enums or fallback strings.
use crate::protocol910::pack_defaults as defaults_pack;
use crate::protocol910::pack_types as types_pack;
use crate::{
    cache::Pack,
    protocol910::{appearance, defaults::EntityDefaults, titles},
};
use anyhow::Result;
fn title_enum(pack: &Pack, id: i32) -> Result<titles::Enum> {
    if id < 0 {
        return Ok(titles::Enum::empty());
    }
    let index = pack.read_archive_index("enum.config")?;
    let group = (id as u32) >> 8;
    let file = (id as u32) & 255;
    if !index.group_id.contains(&group) {
        return Ok(titles::Enum::empty());
    }
    match pack.read_group("enum.config", group)?.get(&file) {
        None => Ok(titles::Enum::empty()),
        Some(bytes) => {
            titles::Enum::decode(bytes).map_err(|e| anyhow::anyhow!("title enum {id}: {e:?}"))
        }
    }
}
pub fn load_titles(pack: &Pack) -> Result<(titles::Defaults, [titles::Enum; 2])> {
    let b = defaults_pack::flat_file(pack, 12)?;
    let d = titles::Defaults::decode(&b).map_err(|e| anyhow::anyhow!("title defaults: {e:?}"))?;
    let e = [title_enum(pack, d.enums[0])?, title_enum(pack, d.enums[1])?];
    Ok((d, e))
}
pub use types_pack::Types;
pub struct Inputs {
    pub defaults: EntityDefaults,
    pub types: Types,
    pub appearance: appearance::Config,
}
pub fn load_inputs(pack: &Pack, members: bool, staff_live_override: bool) -> Result<Inputs> {
    load_inputs_with(pack, types_pack::load(pack, members)?, staff_live_override)
}
/// [`load_inputs`] with the item and NPC types already decoded.
pub fn load_inputs_with(
    pack: &Pack,
    types: types_pack::Types,
    staff_live_override: bool,
) -> Result<Inputs> {
    let defaults = defaults_pack::load(pack)?;
    let (_, enums) = load_titles(pack)?;
    let appearance = assemble(&defaults, &types, &enums, staff_live_override)?;
    Ok(Inputs {
        defaults,
        types,
        appearance,
    })
}
fn assemble(
    defaults: &EntityDefaults,
    types: &types_pack::Types,
    enums: &[titles::Enum; 2],
    staff_live_override: bool,
) -> Result<appearance::Config> {
    let mut c = appearance::Config {
        wear: vec![],
        colour_lengths: [0; 10],
        texture_lengths: [0; 10],
        items: Default::default(),
        npc_sizes: Default::default(),
        titles: Default::default(),
        default_titles: Default::default(),
        staff_live_override,
    };
    defaults
        .apply_appearance(&mut c)
        .map_err(|e| anyhow::anyhow!("appearance defaults: {e:?}"))?;
    types.install_appearance_types(&mut c);
    titles::install(&mut c, enums).map_err(|e| anyhow::anyhow!("appearance titles: {e:?}"))?;
    Ok(c)
}
