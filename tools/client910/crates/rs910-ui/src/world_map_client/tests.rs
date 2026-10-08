use super::*;
use crate::ui_sprites::Sprite;

/// A software canvas for the draw tests (no fonts: labels are skipped).
struct Raster {
    size: [i32; 2],
    px: Vec<i32>,
    sprites: usize,
}
impl Raster {
    fn put(&mut self, x: i32, y: i32, c: i32) {
        if x < 0 || y < 0 || x >= self.size[0] || y >= self.size[1] {
            return;
        }
        let a = (c as u32 >> 24) as i32;
        let i = (y * self.size[0] + x) as usize;
        if a == 255 {
            self.px[i] = c;
        } else if a > 0 {
            let o = self.px[i];
            let mix = |s: i32| (((c >> s & 0xFF) * a + (o >> s & 0xFF) * (255 - a)) / 255) << s;
            self.px[i] = 0xFF00_0000u32 as i32 | mix(16) | mix(8) | mix(0);
        }
    }
}
impl Canvas for Raster {
    fn sprite(&mut self, s: &Rc<Sprite>, pos: [i32; 2]) {
        self.sprites += 1;
        for y in 0..s.size[1] {
            for x in 0..s.size[0] {
                let c = s.argb[(y * s.size[0] + x) as usize];
                self.put(pos[0] + s.padding[0] + x, pos[1] + s.padding[1] + y, c);
            }
        }
    }
    fn scaled(&mut self, s: &Rc<Sprite>, r: [i32; 4]) {
        self.sprites += 1;
        for y in 0..r[3] {
            for x in 0..r[2] {
                let c =
                    s.argb[((y * s.size[1] / r[3]) * s.size[0] + x * s.size[0] / r[2]) as usize];
                self.put(r[0] + x, r[1] + y, c);
            }
        }
    }
    fn sprite_tinted(&mut self, s: &Rc<Sprite>, pos: [i32; 2], _: i32) {
        self.sprite(s, pos);
    }
    fn rotated(&mut self, s: &Rc<Sprite>, c: [f32; 2], _: i32) {
        let [w, h] = s.full_size();
        self.sprite(s, [c[0] as i32 - w / 2, c[1] as i32 - h / 2]);
    }
    fn fill(&mut self, r: [i32; 4], c: i32) {
        for y in r[1]..r[1] + r[3] {
            for x in r[0]..r[0] + r[2] {
                self.put(x, y, c);
            }
        }
    }
    fn outline(&mut self, r: [i32; 4], c: i32) {
        for x in r[0]..r[0] + r[2] {
            self.put(x, r[1], c);
            self.put(x, r[1] + r[3] - 1, c);
        }
        for y in r[1]..r[1] + r[3] {
            self.put(r[0], y, c);
            self.put(r[0] + r[2] - 1, y, c);
        }
    }
    fn line(&mut self, a: [i32; 2], b: [i32; 2], c: i32, _: [i32; 3]) {
        let n = (b[0] - a[0]).abs().max((b[1] - a[1]).abs()).max(1);
        for i in 0..=n {
            self.put(
                a[0] + (b[0] - a[0]) * i / n,
                a[1] + (b[1] - a[1]) * i / n,
                c | 0xFF00_0000u32 as i32,
            );
        }
    }
    fn polygon(&mut self, points: &[i32], c: i32) {
        if c == 0 {
            return;
        }
        for span in crate::world_map_polygon::spans(points, 0, self.size[1], None) {
            for x in span.x..=span.x + span.len {
                self.put(x, span.y, c | 0xFF00_0000u32 as i32);
            }
        }
    }
    fn measure(&mut self, _: i32, _: &str) -> Option<[i32; 2]> {
        None
    }
    fn text(&mut self, _: i32, _: &str, _: [i32; 4], _: i32, _: i32) {}
    fn text_centre(&mut self, _: &str, _: [i32; 2], _: i32) {}
}

fn surface() -> ClientWorldMap {
    let pack = crate::test_support::require_pack("client.worldmap.js5");
    let mut wm = ClientWorldMap::default();
    wm.install(&pack);
    wm.members = true;
    // Lumbridge castle courtyard: the player's own map is the surface map.
    wm.follow_player([3222, 3218], false);
    // Decode in one call (the `setMap` path) rather than 5 ms slices.
    wm.map.incremental = false;
    for _ in 0..8 {
        wm.update_loading(Some([0, 3222, 3218]), &|_| true);
    }
    assert_eq!(wm.loading, 100);
    wm
}

#[test]
fn jump_and_zoom_animate() {
    let mut wm = ClientWorldMap::default();
    wm.map.allocate([0, 0], [128, 128]);
    wm.map.zoom = 4.0;
    wm.map.target_zoom = 8.0;
    wm.position = [10, 10];
    wm.jump = [30, 11];
    wm.update([0, 0]);
    // The zoom grows by `zoom / 30`, the jump by `delta / min(8, |delta|)`.
    assert!((wm.map.zoom - (4.0 + 4.0 / 30.0)).abs() < 1e-6);
    assert_eq!(wm.position, [12, 11]);
    for _ in 0..40 {
        wm.update([0, 0]);
    }
    assert_eq!(wm.map.zoom, 8.0);
    assert_eq!((wm.position, wm.jump), ([30, 11], [-1, -1]));
    assert_eq!(wm.map.shape_size, 4);
}

#[test]
fn flashes_loop_and_expire() {
    let mut wm = ClientWorldMap::default();
    wm.set_flash_tics(2);
    wm.set_flash_loops(3);
    wm.flash_element(42);
    wm.update([0, 0]);
    assert_eq!(
        wm.flash_elements.get(&42),
        Some(&Flash { loops: 3, ticks: 1 })
    );
    wm.update([0, 0]);
    assert_eq!(
        wm.flash_elements.get(&42),
        Some(&Flash { loops: 2, ticks: 2 })
    );
    for _ in 0..4 {
        wm.update([0, 0]);
    }
    assert!(!wm.flash_elements.contains_key(&42));
    // Setting the flash tics to 0 restores the default 50.
    wm.set_flash_tics(0);
    assert_eq!(wm.flash_tics, FLASH_TICS);
    assert_eq!(
        wm.flash_alpha(Flash {
            loops: 1,
            ticks: 50
        }),
        0
    );
    assert_eq!(
        wm.flash_alpha(Flash {
            loops: 1,
            ticks: 25
        }),
        127
    );
}

#[test]
fn containers_hit_sprite_or_label_boxes() {
    let c = Container {
        element: 0,
        sprite: [10, 20, 30, 40],
        text: [0, 5, 0, 5],
    };
    assert!(c.contains(10, 30) && c.contains(20, 40) && c.contains(3, 3));
    assert!(!c.contains(21, 30) && !c.contains(6, 6));
}

#[test]
fn drag_and_click_follow_loop_interface() {
    let mut wm = ClientWorldMap::default();
    wm.map.areas.push(crate::minimap::AreaMetadata {
        id: 1,
        name: "a".into(),
        map_name: "a".into(),
        config_origin: 0,
        config_bounds: [0, 127, 0, 127],
        background: -1,
        active: true,
        config_zoom: 100,
        subareas: vec![[0, 0, 0, 127, 127, 0, 0, 127, 127]],
    });
    wm.set_map(1, -1, -1, false);
    wm.update_loading(None, &|_| true);
    wm.position = [64, 64];
    // A press 40 px right of centre at zoom 8 is 10 tiles east.
    let e = wm.component_input(
        [0, 0, 400, 300],
        [240, 150],
        true,
        true,
        Some([240, 150]),
        true,
    );
    assert_eq!(e.click, Some(74 << 14 | 64));
    assert_eq!(wm.click_state, 1);
    // A press outside the area's subareas clicks the zero coordinate.
    wm.position = [120, 64];
    let e = wm.component_input(
        [0, 0, 400, 300],
        [399, 150],
        true,
        true,
        Some([399, 150]),
        true,
    );
    assert_eq!(e.click, Some(0));
    wm.position = [64, 64];
    wm.component_input(
        [0, 0, 400, 300],
        [240, 150],
        true,
        true,
        Some([240, 150]),
        true,
    );
    // Dragging 16 px left moves the centre 4 tiles east.
    wm.component_input([0, 0, 400, 300], [224, 150], true, true, None, true);
    assert_eq!((wm.click_state, wm.position), (2, [68, 64]));
    let e = wm.component_input([0, 0, 400, 300], [224, 150], true, false, None, true);
    assert_eq!((e.show_menu, wm.click_state), (None, 0));
    // A plain release shows the menu at the press point.
    wm.component_input(
        [0, 0, 400, 300],
        [240, 150],
        true,
        true,
        Some([240, 150]),
        true,
    );
    let e = wm.component_input([0, 0, 400, 300], [240, 150], true, false, None, true);
    assert_eq!(e.show_menu, Some([240, 150]));
}

/// A toolkit change reloads the map the player was looking at.
#[test]
fn reload_after_a_toolkit_change_keeps_the_map() {
    let mut wm = ClientWorldMap::default();
    wm.map.areas.push(crate::minimap::AreaMetadata {
        id: 7,
        name: "a".into(),
        map_name: "a".into(),
        config_origin: 0,
        config_bounds: [0, 127, 0, 127],
        background: -1,
        active: true,
        config_zoom: 100,
        subareas: vec![[0, 0, 0, 127, 127, 0, 0, 127, 127]],
    });
    wm.set_map(7, -1, -1, false);
    wm.update_loading(None, &|_| true);
    assert_eq!(wm.map.metadata().map(|m| m.id), Some(7));
    wm.reload();
    wm.update_loading(None, &|_| true);
    assert_eq!(wm.map.metadata().map(|m| m.id), Some(7));
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn surface_loads_centred_on_the_player_with_elements() {
    let mut wm = surface();
    let o = wm.map.area.as_ref().unwrap().origin;
    let [x, z] = wm.display_position();
    // The player tile plus up to five tiles of jitter.
    assert!((x - 3222).abs() <= 5 && (z - 3218).abs() <= 5, "{x},{z}");
    assert!(o[0] <= 3222 && o[1] <= 3218);
    let area = wm.map.area.as_ref().unwrap();
    // Loc map elements and static ones.
    assert!(area.elements.len() > 500, "{}", area.elements.len());
    let near: Vec<_> = area
        .elements
        .iter()
        .filter(|e| (e.x + o[0] - 3222).abs() < 40 && (e.z + o[1] - 3218).abs() < 40)
        .collect();
    assert!(!near.is_empty());
    let mut raster = Raster {
        size: [800, 600],
        px: vec![0; 800 * 600],
        sprites: 0,
    };
    wm.jump_to_instant(3222, 3218);
    wm.draw(&mut raster, [0, 0, 800, 600]);
    // The decoded surface draws coloured tiles, not just a cleared canvas.
    assert!(raster.px.iter().any(|&p| p != 0));
    assert!(raster.sprites > 0);
    assert!(wm.containers.as_ref().is_some_and(|c| !c.is_empty()));
}
