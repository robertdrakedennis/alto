use super::*;

#[derive(Default)]
struct Recording {
    loads: Vec<i32>,
    calls: Vec<i32>,
    fail: bool,
}
impl Source for Recording {
    fn image(&mut self, id: i32) -> Result<Option<Image>> {
        self.loads.push(id);
        Ok((id != 3).then(|| Image {
            graphic: id,
            size: [1, 1],
            hotspot: [0, 0],
            argb: vec![0xff804020u32 as i32],
        }))
    }
}
impl Sink for Recording {
    fn custom(&mut self, id: i32, _: &Image) -> Result<()> {
        anyhow::ensure!(!self.fail, "native cursor creation failed");
        self.calls.push(id);
        Ok(())
    }
    fn system(&mut self) -> Result<()> {
        self.calls.push(-1);
        Ok(())
    }
}
/// The cursor owner driven through the same inputs against a recording of
/// the original client; its rows are checked in below. Row: `id,customCursors,
/// native failure,currentCursor,threw|cursor type loads|native calls`
/// (`-1` = system cursor; type 3 has no sprite).
const RECORDED_CURSOR_STATE: [&str; 11] = [
    "1,1,0,1,0|[1]|[1]",
    "1,1,0,1,0|[1]|[1]",
    "2,1,0,2,0|[1, 2]|[1, 2]",
    "3,1,0,-1,0|[1, 2, 3]|[1, 2, -1]",
    "3,1,0,-1,0|[1, 2, 3, 3]|[1, 2, -1]",
    "2,0,0,-1,0|[1, 2, 3, 3]|[1, 2, -1]",
    "2,1,0,2,0|[1, 2, 3, 3, 2]|[1, 2, -1, 2]",
    "-1,1,0,-1,0|[1, 2, 3, 3, 2]|[1, 2, -1, 2, -1]",
    "-1,1,0,-1,0|[1, 2, 3, 3, 2]|[1, 2, -1, 2, -1]",
    "4,1,1,-1,1|[1, 2, 3, 3, 2, 4]|[1, 2, -1, 2, -1]",
    "1,1,0,1,0|[1, 2, 3, 3, 2, 4, 1]|[1, 2, -1, 2, -1, 1]",
];

#[test]
fn cursor_owner_reuses_restores_and_preserves_state_on_failure() -> Result<()> {
    let (mut state, mut source, mut sink) =
        (State::default(), Recording::default(), Recording::default());
    let inputs = [
        (1, true, false),
        (1, true, false),
        (2, true, false),
        (3, true, false),
        (3, true, false),
        (2, false, false),
        (2, true, false),
        (-1, true, false),
        (-1, true, false),
        (4, true, true),
        (1, true, false),
    ];
    for ((id, on, fail), recorded) in inputs.into_iter().zip(RECORDED_CURSOR_STATE) {
        sink.fail = fail;
        let threw = state.update(id, on, &mut source, &mut sink).is_err();
        let row = format!(
            "{id},{},{},{},{}|{:?}|{:?}",
            i32::from(on),
            i32::from(fail),
            state.current,
            i32::from(threw),
            source.loads,
            sink.calls
        );
        assert_eq!(row, recorded);
    }
    assert_eq!(select(4, 5, 6), 4);
    assert_eq!(select(-1, 5, 6), 5);
    assert_eq!(select(-1, -1, 6), 6);
    Ok(())
}
#[test]
fn cursor_pixels_include_padding_and_preserve_alpha() -> Result<()> {
    let d = Definition {
        graphic: 77,
        hotspot: [2, 1],
    };
    let sprite = Data {
        width: 1,
        height: 1,
        padding: [1, 1, 2, 0],
        pixels: crate::sprite_data::Pixels::Full {
            argb: vec![0x80402010u32 as i32],
            translucent: true,
        },
    };
    let image = Image::new(&d, &sprite)?;
    assert_eq!(image.size, [4, 2]);
    assert_eq!(image.hotspot, [2, 1]);
    assert_eq!(image.argb.len(), 8);
    assert_eq!(&image.rgba()[20..24], &[64, 32, 16, 128]);
    assert!(image.argb[..5].iter().all(|&p| p == 0));
    assert_eq!(
        Definition::decode(&[1, 0x80, 1, 0, 2, 2, 7, 9, 0])?,
        Definition {
            graphic: 65538,
            hotspot: [7, 9]
        }
    );
    assert!(Definition::decode(&[1, 0]).is_err());
    Ok(())
}
/// Every cached cursor definition and its decoded sprite, serialised, against
/// the frozen recording of the original client's cursor type.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn cursor_assets_match_the_recording() -> Result<()> {
    let pack = Pack::open(rs910_core::test_support::client_dir().join("../../server/data/pack"));
    let mut resources = Resources::new(pack)?;
    let mut recording = vec![];
    for (id, _) in resources.raw.clone() {
        let def = resources.definition(id as i32)?;
        let image = resources.image(id as i32)?;
        let mut values = vec![def.graphic, def.hotspot[0], def.hotspot[1]];
        if let Some(image) = image {
            values.extend(image.size);
            values.extend(image.argb);
        } else {
            values.extend([-1, -1]);
        }
        recording.extend(values.iter().flat_map(|n| n.to_be_bytes()));
    }
    rs910_core::test_support::frozen::assert_stream("cursors/assets", &recording);
    Ok(())
}
