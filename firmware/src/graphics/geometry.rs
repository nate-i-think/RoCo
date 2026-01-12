/// A simple pixel-wise Rect struct which uses half-open
/// bounds: `[left, right) x [top, bottom)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    #[inline]
    pub const fn new(left: i32, top: i32, width: u32, height: u32) -> Self {
        Self { left, top, width, height }
    }

    /// Half-open right edge: left + width.
    #[inline]
    pub const fn right_exclusive(self) -> i32 {
        add_i32_u32_saturating(self.left, self.width)
    }

    /// Half-open bottom edge: top + height.
    #[inline]
    pub const fn bottom_exclusive(self) -> i32 {
        add_i32_u32_saturating(self.top, self.height)
    }

    #[inline]
    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Half-open containment: x in [left, right) and y in [top, bottom)
    #[inline]
    pub const fn contains_point(self, x: i32, y: i32) -> bool {
        if self.is_empty() {
            return false;
        }
        let right = self.right_exclusive();
        let bottom = self.bottom_exclusive();
        x >= self.left && x < right && y >= self.top && y < bottom
    }

    #[inline]
    pub const fn offset(self, x_offset: i32, y_offset: i32) -> Self {
        Self {
            left: self.left.saturating_add(x_offset),
            top: self.top.saturating_add(y_offset),
            width: self.width,
            height: self.height,
        }
    }

    /// Intersection with another rect (half-open). Returns empty
    /// rect if disjoint.
    pub const fn intersection(self, other: Self) -> Self {
        let left = max_i32(self.left, other.left);
        let top = max_i32(self.top, other.top);
        let right = min_i32(self.right_exclusive(), other.right_exclusive());
        let bottom = min_i32(self.bottom_exclusive(), other.bottom_exclusive());

        if right <= left || bottom <= top {
            return Self { left, top, width: 0, height: 0 };
        }

        // safe because we just checked right > left, bottom > top
        Self {
            left,
            top,
            width: (right - left) as u32,
            height: (bottom - top) as u32,
        }
    }

    /// Returns `true` when this `Rect` intersects `other`, false
    /// otherwise. 
    pub const fn intersects(self, other: Self) -> bool {
        let self_right = self.right_exclusive();
        let self_bottom = self.bottom_exclusive();
        let other_right = other.right_exclusive();
        let other_bottom = other.bottom_exclusive();

        self.left < other_right
            && self_right > other.left
            && self.top < other_bottom
            && self_bottom > other.top
    }

    /// Expand/shrink edges by signed amounts.
    /// Positive inset shrinks; negative inset grows.
    pub const fn inset(self, left_inset: i32, top_inset: i32, right_inset: i32, bottom_inset: i32) -> Self {
        let new_left = self.left.saturating_add(left_inset);
        let new_top = self.top.saturating_add(top_inset);

        let new_right = self.right_exclusive().saturating_sub(right_inset);
        let new_bottom = self.bottom_exclusive().saturating_sub(bottom_inset);

        if new_right <= new_left || new_bottom <= new_top {
            return Self { left: new_left, top: new_top, width: 0, height: 0 };
        }

        Self {
            left: new_left,
            top: new_top,
            width: (new_right - new_left) as u32,
            height: (new_bottom - new_top) as u32,
        }
    }
}

impl std::fmt::Display for Rect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}, {}) x [{}, {})",
            self.left,
            self.right_exclusive(),
            self.top,
            self.bottom_exclusive()
        )
    }
}

#[inline(always)]
const fn min_i32(a: i32, b: i32) -> i32 {
    if a < b { a } else { b }
}

#[inline(always)]
const fn max_i32(a: i32, b: i32) -> i32 {
    if a > b { a } else { b }
}

#[inline(always)]
const fn add_i32_u32_saturating(origin: i32, span: u32) -> i32 {
    let sum = (origin as i64) + (span as i64);
    if sum > (i32::MAX as i64) { i32::MAX }
    else if sum < (i32::MIN as i64) { i32::MIN }
    else { sum as i32 }
}