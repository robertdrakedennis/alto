//! The performance metric on the modern renderer: the benchmark's frames
//! really draw the model through the renderer's passes, and the timed run
//! answers draws per second.
use super::*;
use crate::frame::benchmark::{measure, run, PER_FRAME};
use rs910_toolkit::performance_metric::{Benchmark, MetricModel, MetricVertex};

/// A wall standing on the ground, drawn from both sides, scaled by `scale`
/// (0 collapses it to a point: the same draws, no pixels).
fn wall(scale: f32) -> MetricModel {
    let vertex = |x: f32, y: f32| {
        MetricVertex::new([x * scale, y * scale, 0.0], [0.0, 0.0, -1.0], 0xff80_c0e0)
    };
    MetricModel {
        vertices: vec![
            vertex(-200.0, -400.0),
            vertex(200.0, -400.0),
            vertex(0.0, 0.0),
        ],
        indices: vec![0, 1, 2, 0, 2, 1],
    }
}

fn request(model: &MetricModel, budget_ms: i64) -> Benchmark<'_> {
    Benchmark {
        model,
        canvas: [128, 96],
        near: 200.0,
        far: 9000.0,
        budget_ms,
    }
}

/// One whole frame is the 136 placements of the profiling layout, the model
/// lands on the frame (the same draws of a collapsed model leave it empty),
/// and a timed run reports a positive rate.
#[test]
#[ignore = "needs a GPU (headless wgpu device)"]
fn the_modern_benchmark_draws_and_times_frames() {
    // The real clock: the run is timed.
    let (device, queue) = crate::test_support::require_gpu();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let settings = ModernSettings::DEFAULT;
    let (model, empty) = (wall(1.0), wall(0.0));
    let drawn = run(&device, &queue, format, 4, settings, &request(&model, 0)).unwrap();
    let blank = run(&device, &queue, format, 4, settings, &request(&empty, 0)).unwrap();
    assert_eq!(
        (drawn.draws, blank.draws),
        (PER_FRAME as i64, PER_FRAME as i64)
    );
    let (a, b) = (
        read_back(&device, &queue, &drawn.frame, 4),
        read_back(&device, &queue, &blank.frame, 4),
    );
    let differing = a
        .chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(p, q)| p != q)
        .count();
    assert!(differing > 100, "the model changes {differing} pixels");
    let rate = measure(&device, &queue, format, 4, settings, &request(&model, 100)).unwrap();
    assert!(rate > 0, "{rate} draws per second");
}
