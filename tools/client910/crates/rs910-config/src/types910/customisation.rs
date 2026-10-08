//! Item appearance customisation (worn and head models, recolours and
//! retextures); `config_types::Item` decodes it and `entities910::appearance`
//! re-exports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Customisation {
    pub man: [i32; 3],
    pub woman: [i32; 3],
    pub man_head: [i32; 2],
    pub woman_head: [i32; 2],
    pub recolour: Option<Vec<i16>>,
    pub retexture: Option<Vec<i16>>,
}
