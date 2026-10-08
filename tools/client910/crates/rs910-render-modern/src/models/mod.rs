//! Models and materials: the frame's draw list from the snapshot in the
//! faithful order ([`draw_list`]), the vertex streams of floors and models
//! ([`mesh`]), the material cache ([`materials`]), the forward shading's
//! constants and BRDF parameters ([`shading`]), RT7 static locs ([`rt7`])
//! and animated entities posed as the classic models ([`rt7_anim`]), the
//! draws' bounding boxes ([`bounds`]).

pub mod bounds;
pub mod draw_list;
pub mod material_arrays;
pub mod materials;
pub mod mesh;
pub mod rt7;
pub mod rt7_anim;
pub mod shading;
