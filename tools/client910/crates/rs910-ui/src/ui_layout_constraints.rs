//! Pixel constraints retain fractional parent and element anchors separately.
//! Rounded float conversion and signed division preserve their mutation order.
const WIDTH_AXIS: usize = 0;
const HEIGHT_AXIS: usize = 1;
const AXIS_COUNT: usize = 2;
const EXACT_INTEGER_BOUNDARY: f32 = (1_u32 << (f32::MANTISSA_DIGITS - 1)) as f32;
const HALF_PIXEL_BIAS: f32 = f32::from_bits(0x3eff_ffff);
const FLOAT_SIGN_BIT: u32 = 1_u32 << (u32::BITS - 1);
const INTEGER_CONVERSION_LIMIT: f32 = i32::MAX as f32;

/// Fractional pixels round away from zero at a half. Values outside signed
/// conversion range produce the integer conversion sentinel.
fn pixels(value: f32) -> i32 {
    let rounded = if value.abs() < EXACT_INTEGER_BOUNDARY {
        let magnitude = (value.abs() + HALF_PIXEL_BIAS) as i32 as f32;
        f32::from_bits(magnitude.to_bits() | (value.to_bits() & FLOAT_SIGN_BIT))
    } else {
        value
    };
    if !(-INTEGER_CONVERSION_LIMIT..INTEGER_CONVERSION_LIMIT).contains(&rounded) {
        i32::MIN
    } else {
        rounded as i32
    }
}

/// Declarative conversion from a script value to an axis constraint. Mode
/// numbers and coefficients come from the selected revision's layout table.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AxisRecipe {
    pub pixel_scale: i32,
    pub fraction_scale: f32,
    pub fraction_bias: f32,
    pub fraction_subtract: bool,
    pub element_fraction: f32,
}
impl AxisRecipe {
    pub fn select(modes: &[Self], mode: i32) -> anyhow::Result<Self> {
        anyhow::ensure!(!modes.is_empty(), "missing layout mode recipes");
        let index = usize::try_from(mode)
            .unwrap_or_default()
            .min(modes.len() - 1);
        Ok(modes[index])
    }

    fn fraction(self, value: i32) -> f32 {
        let scaled = value as f32 * self.fraction_scale;
        if self.fraction_subtract {
            self.fraction_bias - scaled
        } else {
            scaled + self.fraction_bias
        }
    }

    pub fn size(self, value: i32) -> SizeAxis {
        SizeAxis {
            pixels: value.wrapping_mul(self.pixel_scale),
            parent_fraction: self.fraction(value),
        }
    }

    pub fn position(self, value: i32) -> PositionAxis {
        PositionAxis {
            pixels: value.wrapping_mul(self.pixel_scale),
            parent_fraction: self.fraction(value),
            element_fraction: self.element_fraction,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SizeAxis {
    pub pixels: i32,
    pub parent_fraction: f32,
}
impl SizeAxis {
    pub fn resolve(self, parent: i32) -> i32 {
        pixels(parent as f32 * self.parent_fraction).wrapping_add(self.pixels)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AspectConstraint {
    #[default]
    Independent,
    WidthFromHeight,
    HeightFromWidth,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ViewportInsets {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SizeConstraint {
    pub width: SizeAxis,
    pub height: SizeAxis,
    pub aspect: AspectConstraint,
}
impl SizeConstraint {
    /// Axis setters preserve a separately selected aspect constraint.
    pub fn set_modes(
        &mut self,
        recipes: &[AxisRecipe],
        values: [i32; AXIS_COUNT],
        modes: [i32; AXIS_COUNT],
    ) -> anyhow::Result<()> {
        self.width = AxisRecipe::select(recipes, modes[WIDTH_AXIS])?.size(values[WIDTH_AXIS]);
        self.height = AxisRecipe::select(recipes, modes[HEIGHT_AXIS])?.size(values[HEIGHT_AXIS]);
        Ok(())
    }

    /// An aspect failure preserves the independently computed dimension that
    /// was written before division. Viewport cropping follows both dimensions.
    pub fn apply(
        self,
        dimensions: &mut [i32; AXIS_COUNT],
        parent: [i32; AXIS_COUNT],
        ratio: [i32; AXIS_COUNT],
        viewport: Option<ViewportInsets>,
    ) -> anyhow::Result<bool> {
        let previous = *dimensions;
        let divide = |numerator: i32, denominator: i32| {
            numerator
                .checked_div(denominator)
                .ok_or_else(|| anyhow::anyhow!("invalid component aspect division"))
        };
        match self.aspect {
            AspectConstraint::WidthFromHeight => {
                dimensions[HEIGHT_AXIS] = self.height.resolve(parent[HEIGHT_AXIS]);
                dimensions[WIDTH_AXIS] = divide(
                    ratio[WIDTH_AXIS].wrapping_mul(dimensions[HEIGHT_AXIS]),
                    ratio[HEIGHT_AXIS],
                )?;
            }
            AspectConstraint::HeightFromWidth => {
                dimensions[WIDTH_AXIS] = self.width.resolve(parent[WIDTH_AXIS]);
                dimensions[HEIGHT_AXIS] = divide(
                    ratio[HEIGHT_AXIS].wrapping_mul(dimensions[WIDTH_AXIS]),
                    ratio[WIDTH_AXIS],
                )?;
            }
            AspectConstraint::Independent => {
                dimensions[WIDTH_AXIS] = self.width.resolve(parent[WIDTH_AXIS]);
                dimensions[HEIGHT_AXIS] = self.height.resolve(parent[HEIGHT_AXIS]);
            }
        }
        if let Some(viewport) = viewport {
            dimensions[WIDTH_AXIS] = dimensions[WIDTH_AXIS]
                .wrapping_sub(viewport.right)
                .wrapping_sub(viewport.left);
            dimensions[HEIGHT_AXIS] = dimensions[HEIGHT_AXIS]
                .wrapping_sub(viewport.bottom)
                .wrapping_sub(viewport.top);
        }
        Ok(previous != *dimensions)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PositionAxis {
    pub pixels: i32,
    pub parent_fraction: f32,
    pub element_fraction: f32,
}
impl PositionAxis {
    pub fn resolve(self, parent: i32, element: i32) -> i32 {
        pixels(parent as f32 * self.parent_fraction - element as f32 * self.element_fraction)
            .wrapping_add(self.pixels)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PositionConstraint {
    pub x: PositionAxis,
    pub y: PositionAxis,
}
impl PositionConstraint {
    pub fn from_modes(
        recipes: &[AxisRecipe],
        values: [i32; AXIS_COUNT],
        modes: [i32; AXIS_COUNT],
    ) -> anyhow::Result<Self> {
        Ok(Self {
            x: AxisRecipe::select(recipes, modes[WIDTH_AXIS])?.position(values[WIDTH_AXIS]),
            y: AxisRecipe::select(recipes, modes[HEIGHT_AXIS])?.position(values[HEIGHT_AXIS]),
        })
    }

    pub fn resolve(
        self,
        parent: [i32; AXIS_COUNT],
        dimensions: [i32; AXIS_COUNT],
        viewport: Option<ViewportInsets>,
    ) -> [i32; AXIS_COUNT] {
        let mut position = [
            self.x.resolve(parent[WIDTH_AXIS], dimensions[WIDTH_AXIS]),
            self.y.resolve(parent[HEIGHT_AXIS], dimensions[HEIGHT_AXIS]),
        ];
        if let Some(viewport) = viewport {
            position[WIDTH_AXIS] = position[WIDTH_AXIS].wrapping_add(viewport.left);
            position[HEIGHT_AXIS] = position[HEIGHT_AXIS].wrapping_add(viewport.top);
        }
        position
    }
}

/// Optional byte edges reserve the largest encoded byte for an absent edge.
/// Opposing edges add in byte width; both pairs must fit before either axis
/// loses space. Padding changes available space, not the position origin.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LayoutPadding {
    pub left: Option<u8>,
    pub top: Option<u8>,
    pub right: Option<u8>,
    pub bottom: Option<u8>,
}
impl LayoutPadding {
    pub fn from_edges(edges: [u8; Self::EDGE_COUNT]) -> Self {
        let edge = |index| (edges[index] != Self::ABSENT_EDGE).then_some(edges[index]);
        Self {
            left: edge(Self::LEFT_EDGE),
            top: edge(Self::TOP_EDGE),
            right: edge(Self::RIGHT_EDGE),
            bottom: edge(Self::BOTTOM_EDGE),
        }
    }

    pub fn available_space(self, dimensions: [i32; AXIS_COUNT]) -> [i32; AXIS_COUNT] {
        let horizontal = self
            .left
            .unwrap_or_default()
            .wrapping_add(self.right.unwrap_or_default());
        let vertical = self
            .top
            .unwrap_or_default()
            .wrapping_add(self.bottom.unwrap_or_default());
        if u32::from(horizontal) <= dimensions[WIDTH_AXIS] as u32
            && u32::from(vertical) <= dimensions[HEIGHT_AXIS] as u32
        {
            [
                dimensions[WIDTH_AXIS].wrapping_sub(i32::from(horizontal)),
                dimensions[HEIGHT_AXIS].wrapping_sub(i32::from(vertical)),
            ]
        } else {
            dimensions
        }
    }

    const EDGE_COUNT: usize = 4;
    const LEFT_EDGE: usize = 0;
    const TOP_EDGE: usize = 1;
    const RIGHT_EDGE: usize = 2;
    const BOTTOM_EDGE: usize = 3;
    const ABSENT_EDGE: u8 = u8::MAX;
}
