//! Path-agnostic input routing for the editor shell (M2.3 deliverable 1).
//!
//! A pointer event over the shell window belongs to exactly one of two layers: the **UI** (the
//! WebView2 React panels) or the **viewport** (native wgpu picking/gizmo). This module is the
//! per-pixel decision — and nothing else. It is **pure geometry with no shell dependency**, so the
//! exact same logic carries to the single-window, DComp, or CEF shell unchanged (it's never wasted,
//! whichever composition path the gate selects).
//!
//! Model: the viewport fills the whole client area *behind* the UI. The UI reports its **occluding
//! regions** — dock panels, the toolbar, and any *floating* overlay that sits over the viewport (an
//! open dropdown, an inspector popover, a toast). A pointer routes to the UI iff it lands on an
//! occluding region; otherwise it falls through to the viewport. This is exactly what makes a
//! dropdown-over-viewport (partial overlap) route correctly: the dropdown's rect is an occluder, so
//! points inside it hit the UI while the surrounding viewport still hits the native layer.
//!
//! Regions are rectangles because editor chrome is rectangular (panels/toolbars/dropdowns/toasts).
//! For non-rectangular chrome with alpha holes, [`OcclusionMask`] is the drop-in per-pixel
//! generalization — same API, backed by a coverage bitmap instead of rects.

#![forbid(unsafe_code)]

/// A rectangle in physical (device) pixels, window-client space. Half-open: covers
/// `x..x+w`, `y..y+h`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    #[must_use]
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
    #[must_use]
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// Which layer should receive a pointer event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// The WebView2 UI (panels, toolbar, floating overlays).
    Ui,
    /// The native wgpu viewport (picking / gizmo interaction).
    Viewport,
}

/// Routes pointer events between the UI and the native viewport from the UI's current occluding
/// regions. The React side calls [`set_ui_regions`](Self::set_ui_regions) whenever its layout
/// changes (dock resize, dropdown open/close); the shell calls [`hit_test`](Self::hit_test) per
/// pointer event before forwarding it.
#[derive(Default, Clone)]
pub struct HitTester {
    ui_regions: Vec<Rect>,
}

impl HitTester {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the set of opaque UI regions. Order doesn't matter — occlusion is a union (a pixel
    /// covered by *any* UI region routes to the UI).
    pub fn set_ui_regions(&mut self, regions: Vec<Rect>) {
        self.ui_regions = regions;
    }

    /// The current occluding regions (for debugging / overlays).
    #[must_use]
    pub fn ui_regions(&self) -> &[Rect] {
        &self.ui_regions
    }

    /// Route a pointer at physical pixel `(px, py)`. UI iff it lands on an occluding region, else
    /// the native viewport. Out-of-bounds points fall through to the viewport (the window edge is
    /// the viewport's, not the UI's).
    #[must_use]
    pub fn hit_test(&self, px: i32, py: i32) -> Layer {
        if self.ui_regions.iter().any(|r| r.contains(px, py)) {
            Layer::Ui
        } else {
            Layer::Viewport
        }
    }

    /// Convenience: does this event go to the native viewport? (The hot path that must reach wgpu
    /// without crossing the JS boundary — engineering-rule invariant 4.)
    #[must_use]
    pub fn is_viewport(&self, px: i32, py: i32) -> bool {
        self.hit_test(px, py) == Layer::Viewport
    }
}

/// Per-pixel generalization for non-rectangular chrome (rounded corners, alpha holes). Same routing
/// semantics as [`HitTester`], backed by a 1-bit-per-pixel coverage mask the UI uploads on layout
/// change. Provided so the rect model isn't a dead end; the gate's panels are rectangular, so
/// [`HitTester`] is what the battery exercises.
pub struct OcclusionMask {
    width: i32,
    height: i32,
    /// Row-major; `true` = UI-occluded.
    covered: Vec<bool>,
}

impl OcclusionMask {
    #[must_use]
    pub fn new(width: i32, height: i32) -> Self {
        Self { width, height, covered: vec![false; (width.max(0) * height.max(0)) as usize] }
    }

    /// Paint a rectangle as UI-occluded (the React side composes the mask from its layout).
    pub fn cover(&mut self, r: Rect) {
        for y in r.y.max(0)..(r.y + r.h).min(self.height) {
            for x in r.x.max(0)..(r.x + r.w).min(self.width) {
                self.covered[(y * self.width + x) as usize] = true;
            }
        }
    }

    #[must_use]
    pub fn hit_test(&self, px: i32, py: i32) -> Layer {
        if px < 0 || py < 0 || px >= self.width || py >= self.height {
            return Layer::Viewport;
        }
        if self.covered[(py * self.width + px) as usize] {
            Layer::Ui
        } else {
            Layer::Viewport
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative editor layout (physical px on a 1280×800 client): left dock, top toolbar,
    /// right inspector — the rest is viewport.
    fn editor_layout() -> HitTester {
        let mut h = HitTester::new();
        h.set_ui_regions(vec![
            Rect::new(0, 0, 1280, 40),    // top toolbar (full width)
            Rect::new(0, 40, 240, 760),   // left dock
            Rect::new(1040, 40, 240, 760), // right inspector
        ]);
        h
    }

    #[test]
    fn pointer_over_panels_routes_to_ui() {
        let h = editor_layout();
        assert_eq!(h.hit_test(20, 10), Layer::Ui); // toolbar
        assert_eq!(h.hit_test(100, 400), Layer::Ui); // left dock
        assert_eq!(h.hit_test(1100, 400), Layer::Ui); // right inspector
    }

    #[test]
    fn pointer_over_viewport_gap_routes_to_viewport() {
        let h = editor_layout();
        assert_eq!(h.hit_test(640, 400), Layer::Viewport); // center viewport
        assert!(h.is_viewport(300, 200));
    }

    #[test]
    fn dropdown_over_viewport_partial_overlap() {
        // A dropdown opens from the toolbar and overhangs the viewport.
        let mut h = editor_layout();
        let mut regions = h.ui_regions().to_vec();
        let dropdown = Rect::new(400, 40, 180, 220); // floats over the viewport
        regions.push(dropdown);
        h.set_ui_regions(regions);

        assert_eq!(h.hit_test(450, 120), Layer::Ui, "inside the dropdown → UI");
        assert_eq!(h.hit_test(700, 120), Layer::Viewport, "beside the dropdown → viewport");
        assert_eq!(h.hit_test(450, 400), Layer::Viewport, "below the closed-region dropdown → viewport");
    }

    #[test]
    fn nothing_falls_through_when_closing_a_dropdown() {
        // Closing the dropdown must hand that region back to the viewport (no stuck occluder).
        let mut h = editor_layout();
        let base = h.ui_regions().to_vec();
        h.set_ui_regions({
            let mut r = base.clone();
            r.push(Rect::new(400, 40, 180, 220));
            r
        });
        assert_eq!(h.hit_test(450, 120), Layer::Ui);
        h.set_ui_regions(base); // dropdown closed
        assert_eq!(h.hit_test(450, 120), Layer::Viewport);
    }

    #[test]
    fn out_of_bounds_is_viewport() {
        let h = editor_layout();
        assert_eq!(h.hit_test(-5, 400), Layer::Viewport);
        assert_eq!(h.hit_test(5000, 400), Layer::Viewport);
    }

    #[test]
    fn empty_layout_all_viewport() {
        let h = HitTester::new();
        assert_eq!(h.hit_test(0, 0), Layer::Viewport);
        assert_eq!(h.hit_test(640, 400), Layer::Viewport);
    }

    #[test]
    fn occlusion_mask_matches_rect_semantics() {
        let mut m = OcclusionMask::new(100, 100);
        m.cover(Rect::new(10, 10, 20, 20));
        assert_eq!(m.hit_test(15, 15), Layer::Ui);
        assert_eq!(m.hit_test(5, 5), Layer::Viewport);
        assert_eq!(m.hit_test(50, 50), Layer::Viewport);
        assert_eq!(m.hit_test(-1, -1), Layer::Viewport);
    }
}
