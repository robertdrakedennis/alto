//! NPC type customisation. f32 scale bits retained for exact NaN payloads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Custom {
    pub salt: i64,
    pub models: Option<Vec<i32>>,
    pub scale_bits: Option<Vec<u32>>,
    pub rotation: Option<Vec<[i32; 3]>>,
    pub offset: Option<Vec<[i32; 3]>>,
    pub colours: Option<Vec<i16>>,
    pub textures: Option<Vec<i16>>,
}
