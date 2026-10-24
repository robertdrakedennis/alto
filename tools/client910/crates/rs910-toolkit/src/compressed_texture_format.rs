//! The compressed texture formats: the GL codes a toolkit
//! reports and the telemetry report serialises by serial id. Split out of
//! client910's `client_watch` in Phase 3.2: the GPU renderer maps device
//! features to these codes below the UI.
/// Compressed texture format GL codes by serial id.
pub const COMPRESSED_TEXTURE_FORMATS: [i32; 52] = [
    33776, 33777, 33778, 33779, 35728, 35729, 35730, 35731, 35732, 35733, 35734, 35735, 35736,
    35737, 37492, 37493, 37494, 37495, 37496, 37497, 37488, 37489, 37490, 37491, 37808, 37809,
    37810, 37811, 37812, 37813, 37814, 37815, 37816, 37817, 37818, 37819, 37820, 37821, 37840,
    37841, 37842, 37843, 37844, 37845, 37846, 37847, 37848, 37849, 37850, 37851, 37852, 37853,
];
