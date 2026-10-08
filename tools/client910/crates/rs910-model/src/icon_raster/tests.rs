//! Regression tests for the icon rasteriser.
//!
//! The pixel-exact contract is pinned two ways: `rs910-scene`'s item-icon
//! goldens draw real items, and this module draws seeded random scenes (every
//! fill mode, translucency level, corner order, clipping case and tie) and
//! compares a digest of each finished canvas with
//! `fixtures/icon-goldens/raster-scenes.txt`. The digests were recorded from
//! the rasteriser this one replaced, so the two agree on every scene.

use super::*;

/// A small deterministic generator (xorshift64*), so scenes never depend on
/// a library's version.
pub(super) struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e3779b97f4a7c15) | 1)
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545f4914f6cdd1d)
    }

    pub fn below(&mut self, n: u32) -> u32 {
        (self.next() >> 33) as u32 % n
    }

    pub fn between(&mut self, low: f32, high: f32) -> f32 {
        low + (self.next() >> 40) as f32 / (1u64 << 24) as f32 * (high - low)
    }
}

pub(super) struct TextureSpec {
    pub id: i32,
    pub size: i32,
    pub texels: Option<Vec<i32>>,
    pub average_colour: i32,
    pub alpha: u32,
    pub alpha_threshold: i32,
    pub repeat: bool,
}

pub(super) enum Fill {
    Flat {
        colour: i32,
        level: i32,
    },
    Palette {
        index: [i32; 3],
        level: i32,
    },
    Rgb {
        colours: [i32; 3],
        level: i32,
    },
    Textured {
        material: i32,
        w: [f32; 3],
        uv: [[f32; 2]; 3],
        light: [i32; 3],
    },
}

pub(super) struct Triangle {
    pub corners: [[f32; 3]; 3],
    pub fill: Fill,
}

pub(super) struct Scene {
    pub width: i32,
    pub height: i32,
    pub background: Vec<i32>,
    pub textures: Vec<TextureSpec>,
    pub triangles: Vec<Triangle>,
}

fn coordinate(rng: &mut Rng, extent: i32, style: u32) -> f32 {
    let extent = extent as f32;
    match style {
        0..=3 => rng.between(-6.0, extent + 6.0),
        4 | 5 => rng.below(extent as u32 + 9) as f32 - 4.0,
        6 => (rng.below(extent as u32 * 2 + 17) as f32 - 8.0) * 0.5,
        7 => rng.between(-60.0, extent + 60.0),
        8 => extent - 0.75 + rng.between(0.0, 1.5),
        _ => rng.between(-1.5, 1.5),
    }
}

pub(super) fn random_scene(seed: u64) -> Scene {
    let mut rng = Rng::new(seed);
    let (width, height) = [(36, 32), (36, 32), (36, 32), (20, 24), (64, 40)][rng.below(5) as usize];
    let background = (0..width * height)
        .map(|_| {
            if rng.below(4) == 0 {
                0
            } else {
                rng.next() as i32
            }
        })
        .collect();
    let textures = (0..rng.below(5))
        .map(|n| {
            let size = [4, 8, 16][rng.below(3) as usize];
            let texels = (rng.below(5) != 0).then(|| {
                (0..size * size)
                    .map(|_| (rng.next() as i32 & 0xffffff) | (rng.below(256) as i32) << 24)
                    .collect()
            });
            TextureSpec {
                id: n as i32 * 3 + 1,
                size,
                texels,
                average_colour: rng.below(65536) as i32,
                alpha: rng.below(3),
                alpha_threshold: rng.below(256) as i32,
                repeat: rng.below(2) == 0,
            }
        })
        .collect::<Vec<_>>();
    let mut triangles = Vec::new();
    for _ in 0..4 + rng.below(9) {
        let style = rng.below(10);
        let small = rng.below(4) == 0;
        let (centre_x, centre_y) = (
            coordinate(&mut rng, width, style),
            coordinate(&mut rng, height, style),
        );
        let same_depth = rng.below(3) == 0;
        let base_depth = rng.between(0.05, 2.0);
        let mut corners = [[0.0f32; 3]; 3];
        // Shapes that stress the sorting and clipping rules: two corners
        // above the canvas, two below it, and collinear corners (equal
        // slopes).
        let shape = rng.below(9);
        let (start_x, start_y) = (
            rng.below(width as u32 + 6) as f32 - 3.0,
            rng.below(height as u32 + 6) as f32 - 3.0,
        );
        let (step_x, step_y) = (rng.below(9) as f32 - 4.0, rng.below(9) as f32 - 4.0);
        let far = rng.below(3) as f32 + 1.0;
        for (n, corner) in corners.iter_mut().enumerate() {
            let (x, y) = match shape {
                6 => (
                    coordinate(&mut rng, width, style),
                    if n < 2 {
                        rng.between(-15.0, 0.0)
                    } else {
                        rng.between(-2.0, height as f32 + 8.0)
                    },
                ),
                7 => (
                    coordinate(&mut rng, width, style),
                    if n < 2 {
                        height as f32 + rng.between(0.0, 12.0)
                    } else {
                        rng.between(-2.0, height as f32 + 2.0)
                    },
                ),
                8 => (
                    start_x + step_x * n as f32 * far,
                    start_y + step_y * n as f32 * far,
                ),
                _ if small => (
                    centre_x + rng.between(-3.0, 3.0),
                    centre_y + rng.between(-3.0, 3.0),
                ),
                _ => (
                    coordinate(&mut rng, width, style),
                    coordinate(&mut rng, height, style),
                ),
            };
            let depth = if same_depth {
                base_depth
            } else {
                rng.between(0.05, 2.0)
            };
            *corner = [x, y, depth];
        }
        let translucent = rng.below(3) == 0;
        let level = if !translucent {
            0
        } else if rng.below(5) == 0 {
            254
        } else {
            rng.below(254) as i32 + 1
        };
        let fill = match rng.below(8) {
            0..=1 => Fill::Flat {
                colour: rng.next() as i32,
                level,
            },
            2..=3 => Fill::Palette {
                index: std::array::from_fn(|_| rng.below(65536) as i32),
                level,
            },
            4 => Fill::Rgb {
                colours: std::array::from_fn(|_| rng.next() as i32),
                level,
            },
            _ if !textures.is_empty() => {
                let material = textures[rng.below(textures.len() as u32) as usize].id;
                let clear = rng.below(3) == 0;
                Fill::Textured {
                    material,
                    w: std::array::from_fn(|_| rng.between(0.5, 4.0)),
                    uv: std::array::from_fn(|_| [rng.between(-0.5, 1.5), rng.between(-0.5, 1.5)]),
                    light: std::array::from_fn(|_| {
                        let alpha = if clear {
                            rng.below(256)
                        } else {
                            255 - level.min(253) as u32
                        };
                        (alpha << 24 | rng.below(0x1000000)) as i32
                    }),
                }
            }
            _ => Fill::Flat {
                colour: rng.next() as i32,
                level,
            },
        };
        triangles.push(Triangle { corners, fill });
    }
    Scene {
        width,
        height,
        background,
        textures,
        triangles,
    }
}

/// Draws a scene with the rasteriser, returning the final pixels and the depth
/// buffer's bit patterns.
pub(super) fn draw(scene: &Scene) -> (Vec<i32>, Vec<u32>) {
    let mut raster = IconRaster::new(scene.width, scene.height);
    raster.canvas.pixels.copy_from_slice(&scene.background);
    for texture in &scene.textures {
        raster.add_texture(
            texture.id,
            IconTexture {
                texels: texture.texels.clone(),
                size: texture.size,
                average_colour: texture.average_colour,
                alpha: match texture.alpha {
                    0 => TextureAlpha::Opaque,
                    1 => TextureAlpha::Cutout,
                    _ => TextureAlpha::Blended,
                },
                alpha_threshold: texture.alpha_threshold,
                repeat: texture.repeat,
            },
        );
    }
    for triangle in &scene.triangles {
        let points = triangle
            .corners
            .map(|[x, y, depth]| ScreenPoint { x, y, depth });
        match &triangle.fill {
            Fill::Flat { colour, level } => {
                raster.fill_flat(points, *colour, Translucency::from_level(*level));
            }
            Fill::Palette { index, level } => {
                raster.fill_palette_shaded(points, *index, Translucency::from_level(*level));
            }
            Fill::Rgb { colours, level } => {
                raster.fill_rgb_shaded(points, *colours, Translucency::from_level(*level));
            }
            Fill::Textured {
                material,
                w,
                uv,
                light,
            } => {
                let textured = std::array::from_fn(|i| TexturedPoint {
                    at: points[i],
                    w: w[i],
                    u: uv[i][0],
                    v: uv[i][1],
                    light: light[i],
                });
                raster.fill_textured(textured, *material);
            }
        }
    }
    (
        raster.canvas.pixels,
        raster.canvas.depth.iter().map(|d| d.to_bits()).collect(),
    )
}

pub(super) fn digest(pixels: &[i32], depth: &[u32]) -> u64 {
    let words = pixels
        .iter()
        .map(|&p| p as u32)
        .chain(depth.iter().copied());
    words.fold(0xcbf29ce484222325, |hash, word| {
        (hash ^ u64::from(word)).wrapping_mul(0x100000001b3)
    })
}

/// Scenes recorded in the fixture.
pub(super) const RECORDED_SCENES: u64 = 3000;

fn fixture_path() -> std::path::PathBuf {
    rs910_core::test_support::client_dir().join("fixtures/icon-goldens/raster-scenes.txt")
}

#[test]
fn random_scenes_match_the_recorded_digests() {
    let recorded = std::fs::read_to_string(fixture_path()).expect("raster-scenes.txt");
    let mut differing = Vec::new();
    let mut count = 0;
    for (seed, line) in recorded.lines().filter(|l| !l.starts_with('#')).enumerate() {
        let (pixels, depth) = draw(&random_scene(seed as u64));
        if line != format!("{:016x}", digest(&pixels, &depth)) {
            differing.push(seed);
        }
        count += 1;
    }
    assert_eq!(
        count, RECORDED_SCENES,
        "the fixture holds one digest per scene"
    );
    assert!(differing.is_empty(), "scenes differ: {differing:?}");
}

#[test]
#[ignore = "rewrites the recorded digests; only after an intended pixel change"]
fn regenerate_scene_digests() {
    let mut text = String::from(
        "# FNV-1a digest of each seeded random scene's pixels and depth (icon_raster/tests.rs)\n",
    );
    for seed in 0..RECORDED_SCENES {
        let (pixels, depth) = draw(&random_scene(seed));
        text.push_str(&format!("{:016x}\n", digest(&pixels, &depth)));
    }
    std::fs::write(fixture_path(), text).expect("write raster-scenes.txt");
}

fn point(x: f32, y: f32, depth: f32) -> ScreenPoint {
    ScreenPoint { x, y, depth }
}

#[test]
fn the_nearer_face_wins_whatever_the_draw_order() {
    let triangle = |depth| {
        [
            point(2.0, 2.0, depth),
            point(30.0, 2.0, depth),
            point(2.0, 28.0, depth),
        ]
    };
    let mut near_first = IconRaster::new(36, 32);
    near_first.fill_flat(triangle(0.5), 0xff0000ff_u32 as i32, Translucency::OPAQUE);
    near_first.fill_flat(triangle(1.5), 0xffff0000_u32 as i32, Translucency::OPAQUE);
    let mut far_first = IconRaster::new(36, 32);
    far_first.fill_flat(triangle(1.5), 0xffff0000_u32 as i32, Translucency::OPAQUE);
    far_first.fill_flat(triangle(0.5), 0xff0000ff_u32 as i32, Translucency::OPAQUE);
    assert_eq!(near_first.pixels(), far_first.pixels());
    assert_eq!(near_first.pixels()[5 * 36 + 5], 0xff0000ff_u32 as i32);
    assert_eq!(near_first.pixels()[0], 0, "outside the face stays clear");
}

#[test]
fn a_translucent_face_keeps_its_share_of_the_background() {
    let mut raster = IconRaster::new(36, 32);
    let face = [
        point(0.0, 0.0, 1.0),
        point(36.0, 0.0, 1.0),
        point(0.0, 32.0, 1.0),
    ];
    raster.fill_flat(face, 0x00ffffff, Translucency::OPAQUE);
    let behind = [
        point(0.0, 0.0, 0.5),
        point(36.0, 0.0, 0.5),
        point(0.0, 32.0, 0.5),
    ];
    // Level 128 keeps half of the (white) background and adds half of black.
    raster.fill_flat(behind, 0, Translucency::from_level(128));
    assert_eq!(raster.pixels()[4 * 36 + 4] & 0xffffff, 0x7f7f7f);
}
