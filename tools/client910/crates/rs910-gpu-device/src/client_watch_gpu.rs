//! The device query behind the input telemetry's texture-format report: the native
//! device's compression features as the GL codes the telemetry reports.
//! Split out of `client_watch` in Phase 3.2 (wgpu stays in the GPU
//! renderer); the app hands the UI the list.
use crate::compressed_texture_format::COMPRESSED_TEXTURE_FORMATS;
/// The telemetry reports `GL_COMPRESSED_TEXTURE_FORMATS`. The native device reports block
/// compression families instead of GL enums; each family maps to the GL
/// codes that expose it (BC1-3 as S3TC, ETC2/EAC, ASTC LDR).
pub fn gl_compressed_texture_formats(features: wgpu::Features) -> Vec<i32> {
    let mut out = Vec::new();
    if features.contains(wgpu::Features::TEXTURE_COMPRESSION_BC) {
        out.extend_from_slice(&COMPRESSED_TEXTURE_FORMATS[0..4]);
    }
    if features.contains(wgpu::Features::TEXTURE_COMPRESSION_ETC2) {
        out.extend_from_slice(&COMPRESSED_TEXTURE_FORMATS[14..24]);
    }
    if features.contains(wgpu::Features::TEXTURE_COMPRESSION_ASTC) {
        out.extend_from_slice(&COMPRESSED_TEXTURE_FORMATS[24..52]);
    }
    out
}
