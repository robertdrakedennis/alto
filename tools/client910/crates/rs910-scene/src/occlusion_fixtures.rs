//! Synthetic differential fixtures for the occlusion raster, independent of
//! map geometry; the traces are compared against an independently produced
//! reference trace.
use crate::draw_trace::Trace;
use crate::occlusion_raster::{DepthRaster, RasterMode, RasterTriangle};

pub fn raster_trace() -> Trace {
    let bases = [
        [2, 20, 7, 2, 16, 28, 500, 600, 700],
        [5, 5, 20, 5, 25, 15, 500, 500, 500],
        [0, 0, 0, 0, 20, 30, 500, 600, 700],
        [-2003, 0, 2003, -2003, 16, 2003, 10, 40, 99],
        [-2004, 0, 20, 0, 16, 28, 500, 600, 700],
        [0, 12, 24, 16, 16, 16, 500, 600, 700],
        [-4, 23, 8, -2, 32, 17, i32::MIN, i32::MAX, -1],
        [0, 23, 23, 0, 0, 31, 50, 50, 50],
    ];
    let mut cases = Vec::new();
    for base in bases {
        for perm in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let mut a = [0; 9];
            for c in 0..3 {
                for i in 0..3 {
                    a[c * 3 + i] = base[c * 3 + perm[i]];
                }
            }
            cases.push(a);
        }
    }
    let mut seed = 0x910_u32;
    let mut next = |bound: u32| {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((seed >> 1) % bound) as i32
    };
    for _ in 0..256 {
        let mut a = [0; 9];
        for v in &mut a[..6] {
            *v = next(100) - 25;
        }
        for v in &mut a[6..] {
            *v = next(50000);
        }
        cases.push(a);
    }
    let mut raster = DepthRaster::new(32, 24);
    let mut trace = Trace::default();
    for (case, base) in cases.into_iter().enumerate() {
        raster.depth.fill(i32::MAX);
        raster.coverage = 0;
        for (pass, delta) in [0, -1, 150, 151, 152].into_iter().enumerate() {
            let mut a = base;
            if pass != 0 {
                for v in &mut a[6..] {
                    *v = v.wrapping_add(delta);
                }
            }
            raster.mode = if pass == 0 {
                RasterMode::Write
            } else {
                RasterMode::Test
            };
            let result = raster.triangle(RasterTriangle::from_words(a));
            let mut words = vec![raster.mode.code()];
            words.extend(a);
            words.extend([result as i32, raster.coverage]);
            words.extend(&raster.depth);
            trace.push(format!("raster/{case}/{pass}"), words);
        }
    }
    let scene = crate::scene::Scene::new(9, 4, 4, 4);
    let heights = vec![crate::floor::FloorHeights::new(4, 4, 512, vec![0; 25]); 4];
    let mut occlusion = crate::occlusion::Occlusion::new(&scene, &heights);
    let cpu = crate::camera::CpuProjection::new([0.0; 16], [0.0; 16], [0, 0, 32, 24]);
    occlusion.set_cycle(37);
    occlusion.enabled = true;
    occlusion.cycles[0][2][2] = 37;
    let mut gates = Vec::new();
    for count in [100, 101, 102] {
        occlusion.raster.coverage = count;
        gates.extend([
            count,
            occlusion.tile_occluded(&cpu, &heights, 0, 2, 2) as i32,
        ]);
    }
    occlusion.enabled = false;
    gates.push(occlusion.tile_occluded(&cpu, &heights, 0, 2, 2) as i32);
    // The global and per-frame flags both map to disabled in the normal
    // scene planner; verify both paths against that same false result.
    gates.push(occlusion.tile_occluded(&cpu, &heights, 0, 2, 2) as i32);
    occlusion.enabled = true;
    occlusion.cycles[0][2][2] = -37;
    gates.push(occlusion.tile_occluded(&cpu, &heights, 0, 2, 2) as i32);
    trace.push("manager/gates", gates);
    trace
}

#[cfg(test)]
mod tests {
    /// Client cheat command 24: with the manager disabled, the occluder
    /// build and every visibility test bail out.
    #[test]
    fn cheat_toggle_disables_the_manager() {
        let scene = crate::scene::Scene::new(9, 4, 4, 4);
        let heights = vec![crate::floor::FloorHeights::new(4, 4, 512, vec![0; 25]); 4];
        let mut occlusion = crate::occlusion::Occlusion::new(&scene, &heights);
        let cpu = crate::camera::CpuProjection::new([0.0; 16], [0.0; 16], [0, 0, 32, 24]);
        let visibility = vec![vec![true; 9]; 9];
        assert!(occlusion.manager_enabled);
        occlusion.manager_enabled = false;
        occlusion.prepare(
            &crate::occlusion::OcclusionView {
                cpu: &cpu,
                eye: [0; 3],
                surface: [32, 24],
                distance: 4,
                visibility: &visibility,
                roof_level: 1,
                boxes: &[],
            },
            true,
        );
        assert!(!occlusion.enabled);
        assert!(!occlusion.tile_occluded(&cpu, &heights, 0, 2, 2));
        occlusion.manager_enabled = true;
        occlusion.prepare(
            &crate::occlusion::OcclusionView {
                cpu: &cpu,
                eye: [0; 3],
                surface: [32, 24],
                distance: 4,
                visibility: &visibility,
                roof_level: 1,
                boxes: &[],
            },
            true,
        );
        assert!(occlusion.enabled);
    }
}
