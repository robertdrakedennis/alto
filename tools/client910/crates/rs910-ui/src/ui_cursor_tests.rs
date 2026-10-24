use super::*;

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
