//! Client geometry types (Utils.dll `Point`/`Rect`, float based).
//!
//! RE note (GUI.dll `BorderView_c::CreateGfx` 0x10125d3b, `HLayoutNode::CalculatePreferredSize`
//! 0x1012ff61): sizes and rect extents are *inclusive extents* (pixel count − 1). `Rect::width` is
//! `r − l`; an empty size is `-1`. Layout code therefore adds `+1` when it converts to pixel counts.

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// `Rect(l, t, r, b)`; also used for border widths (`layout_borders="Rect(l,t,r,b)"`).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
}

impl Rect {
    pub const fn new(l: f32, t: f32, r: f32, b: f32) -> Self {
        Self { l, t, r, b }
    }
    pub fn width(&self) -> f32 {
        self.r - self.l
    }
    pub fn height(&self) -> f32 {
        self.b - self.t
    }
    pub fn size(&self) -> Point {
        Point::new(self.width(), self.height())
    }
    pub fn translate(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.l + dx, self.t + dy, self.r + dx, self.b + dy)
    }
    /// `Rect::Resize(dl, dt, dr, db)`: adds the deltas to the four edges.
    pub fn resize(&self, dl: f32, dt: f32, dr: f32, db: f32) -> Rect {
        Rect::new(self.l + dl, self.t + dt, self.r + dr, self.b + db)
    }
    /// Shrinks the rect by `borders` (`FUN_1012fddf`: `Resize(l, t, -r, -b)`).
    pub fn shrink(&self, bo: &Rect) -> Rect {
        self.resize(bo.l, bo.t, -bo.r, -bo.b)
    }
    pub fn floor(&self) -> Rect {
        Rect::new(self.l.floor(), self.t.floor(), self.r.floor(), self.b.floor())
    }
    /// Intersection (`Rect::operator&`).
    pub fn intersect(&self, o: &Rect) -> Rect {
        Rect::new(self.l.max(o.l), self.t.max(o.t), self.r.min(o.r), self.b.min(o.b))
    }
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.l && p.x <= self.r && p.y >= self.t && p.y <= self.b
    }
    pub fn is_empty(&self) -> bool {
        self.r < self.l || self.b < self.t
    }
}
