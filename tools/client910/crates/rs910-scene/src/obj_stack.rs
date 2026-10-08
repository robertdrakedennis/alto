//! Object-stack state that does not depend on the renderer: which objects of a
//! tile's sorted stack list it shows, its overlay raise and ground tilt, and
//! the placement of a scattered drop.

/// One object of a tile's stack list: `(index, count)`.
pub type Obj = (i32, i32);

/// The primary, secondary and tertiary objects shown for a sorted stack:
/// the head is primary; the first later object of another type is
/// secondary; every object after it of a third type overwrites tertiary, so
/// the last one wins.
pub fn select(stack: &[Obj]) -> Option<[Option<Obj>; 3]> {
    let (&primary, rest) = stack.split_first()?;
    let mut secondary = None;
    let mut tertiary = None;
    let mut iter = rest.iter();
    for &obj in iter.by_ref() {
        if obj.0 != primary.0 {
            secondary = Some(obj);
            break;
        }
    }
    if let Some(second) = secondary {
        for &obj in iter {
            if obj.0 != primary.0 && obj.0 != second.0 {
                tertiary = Some(obj);
            }
        }
    }
    Some([Some(primary), secondary, tertiary])
}

/// The lowest (most raised) overlay height of the raised entities in the
/// tile's list, then the tile's ground decoration height if that is higher.
/// The stack's raise becomes this value; 0 leaves the stack on the ground and
/// enables the tilt.
pub fn overlay_raise(raised_overlays: &[i32], ground_decoration: Option<i32>) -> i32 {
    let mut raise = 0;
    for &overlay in raised_overlays {
        if overlay < raise {
            raise = overlay;
        }
    }
    if let Some(height) = ground_decoration {
        if height > -raise {
            raise = -height;
        }
    }
    raise
}

/// The ground tilt for `radius` = the previous draw's largest model radius:
/// four fine heights at `±radius` around the stack origin give the pitch and
/// roll (`(int) (atan2 * 2607.59...) & 0x3FFF`, truncated) and the lowest
/// diagonal pair's mean minus the stack's own height as the y shift.
/// `height(dx, dz)` samples the height map of the occlusion level.
pub fn tilt(radius: i32, ground: i32, height: impl Fn(i32, i32) -> i32) -> Tilt {
    let span = radius << 1;
    let (lo, hi) = (-span / 2, span / 2);
    let south_west = height(lo, lo);
    let south_east = height(hi, lo);
    let north_west = height(lo, hi);
    let north_east = height(hi, hi);
    let angle = |a: i32, b: i32| {
        ((f64::from(a - b).atan2(f64::from(span)) * 2607.5945876176133) as i32) & 0x3fff
    };
    let (pitch, roll) = if span != 0 {
        (
            angle(south_west.min(south_east), north_west.min(north_east)),
            angle(south_west.min(north_west), south_east.min(north_east)),
        )
    } else {
        (0, 0)
    };
    let lowest_pair = (south_west + north_east).min(south_east + north_west);
    Tilt {
        pitch,
        roll,
        lift: (lowest_pair >> 1) - ground,
    }
}

/// The tilt `draw` applies before the stack translation: rotate about x by
/// `pitch`, about z by `-roll`, then translate y by `lift`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tilt {
    pub pitch: i32,
    pub roll: i32,
    pub lift: i32,
}

/// Placement of a scattered drop in 14-bit angle units: a y rotation, an x
/// translation of `(random * 0.5 + 0.5) * 256 - 128` (0..128), and a second y
/// rotation. `next` supplies the three uniform values in [0, 1).
pub fn scatter(mut next: impl FnMut() -> f64) -> [i32; 3] {
    let first = (next() * 16384.0) as i32 & 0x3fff;
    let offset = ((next() * 0.5 + 0.5) * 256.0 - 128.0) as i32;
    let second = (next() * 16384.0) as i32 & 0x3fff;
    [first, offset, second]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Including repeated primaries being skipped
    /// and the last third-type object winning tertiary.
    #[test]
    fn select_follows_sort_obj_stacks() {
        assert_eq!(select(&[]), None);
        assert_eq!(select(&[(5, 1)]), Some([Some((5, 1)), None, None]));
        assert_eq!(
            select(&[(5, 1), (5, 2), (6, 3), (5, 4), (7, 5), (6, 6), (8, 7)]),
            Some([Some((5, 1)), Some((6, 3)), Some((8, 7))])
        );
        assert_eq!(select(&[(5, 1), (5, 2)]), Some([Some((5, 1)), None, None]));
    }

    #[test]
    fn overlay_raise_takes_the_highest_surface() {
        assert_eq!(overlay_raise(&[], None), 0);
        assert_eq!(overlay_raise(&[-40, 10, -90], None), -90);
        assert_eq!(overlay_raise(&[-40], Some(60)), -60);
        assert_eq!(overlay_raise(&[-90], Some(60)), -90);
    }

    #[test]
    fn tilt_truncates_toward_zero() {
        // A slope rising 100 fine units per 128 along +x.
        let t = tilt(64, 0, |dx, _| dx * 100 / 128);
        assert_eq!(t.pitch, 0);
        // atan2(-50 - 50, 128) = -0.6633 rad -> -1729.6, truncated to -1729.
        assert_eq!(t.roll, (-1729) & 0x3fff);
        assert_eq!(t.lift, 0);
        assert_eq!(
            tilt(0, 5, |_, _| 9),
            Tilt {
                pitch: 0,
                roll: 0,
                lift: 4
            }
        );
    }

    #[test]
    fn scatter_offset_spans_zero_to_128() {
        assert_eq!(scatter(|| 0.0), [0, 0, 0]);
        let mut values = [0.25, 0.999_999, 0.5].into_iter();
        assert_eq!(scatter(|| values.next().unwrap()), [4096, 127, 8192]);
    }
}
