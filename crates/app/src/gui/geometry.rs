//! Pure screen geometry: which monitor gets the launcher and where on it.
//!
//! All rectangles are in logical points with the origin at the top-left of
//! the primary monitor and y growing downwards (the convention winit uses
//! for window positions). AppKit reports bottom-left coordinates; [`flip_y`]
//! converts.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect { x, y, w, h }
    }

    /// Half-open containment so adjacent monitors never both claim a point.
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// Squared distance from the point to the rectangle (0 when inside).
    pub fn distance_sq(&self, px: f64, py: f64) -> f64 {
        let dx = (self.x - px).max(0.0).max(px - (self.x + self.w));
        let dy = (self.y - py).max(0.0).max(py - (self.y + self.h));
        dx * dx + dy * dy
    }
}

/// Convert an AppKit rect (origin bottom-left of the primary monitor) to a
/// top-left-origin rect.
pub fn flip_y(primary_height: f64, r: Rect) -> Rect {
    Rect::new(r.x, primary_height - (r.y + r.h), r.w, r.h)
}

/// Convert an AppKit point (e.g. `NSEvent.mouseLocation`) likewise.
pub fn flip_point_y(primary_height: f64, y: f64) -> f64 {
    primary_height - y
}

/// Index of the monitor containing the mouse; if it is in a gap between
/// monitors, the nearest one. `None` only when there are no monitors.
pub fn pick_monitor(monitors: &[Rect], mouse: (f64, f64)) -> Option<usize> {
    if let Some(i) = monitors.iter().position(|m| m.contains(mouse.0, mouse.1)) {
        return Some(i);
    }
    monitors
        .iter()
        .enumerate()
        .min_by(|a, b| {
            a.1.distance_sq(mouse.0, mouse.1)
                .total_cmp(&b.1.distance_sq(mouse.0, mouse.1))
        })
        .map(|(i, _)| i)
}

/// Where the window's top-left goes on `monitor`: horizontally centred,
/// `y_frac` of the monitor height from its top. A window wider than the
/// monitor is pinned to the monitor's left edge.
pub fn origin_on(monitor: Rect, window_width: f64, y_frac: f64) -> (f64, f64) {
    let x = monitor.x + ((monitor.w - window_width) / 2.0).max(0.0);
    (x, monitor.y + monitor.h * y_frac)
}

/// Monitor rects + mouse point -> window top-left position.
pub fn launcher_position(monitors: &[Rect], mouse: (f64, f64), window_width: f64, y_frac: f64) -> Option<(f64, f64)> {
    pick_monitor(monitors, mouse).map(|i| origin_on(monitors[i], window_width, y_frac))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Primary 1440x900 at the origin, external 1920x1080 to its right and
    // raised so its bottom edge is level with the primary's (AppKit: y = 0).
    fn two_monitors() -> Vec<Rect> {
        let primary_h = 900.0;
        let primary = flip_y(primary_h, Rect::new(0.0, 0.0, 1440.0, 900.0));
        let external = flip_y(primary_h, Rect::new(1440.0, 0.0, 1920.0, 1080.0));
        vec![primary, external]
    }

    #[test]
    fn flip_y_maps_appkit_to_top_left() {
        let ms = two_monitors();
        assert_eq!(ms[0], Rect::new(0.0, 0.0, 1440.0, 900.0));
        // the taller external display extends above the primary: negative y
        assert_eq!(ms[1], Rect::new(1440.0, -180.0, 1920.0, 1080.0));
        assert_eq!(flip_point_y(900.0, 100.0), 800.0);
    }

    #[test]
    fn mouse_on_each_monitor() {
        let ms = two_monitors();
        assert_eq!(pick_monitor(&ms, (100.0, 100.0)), Some(0));
        assert_eq!(pick_monitor(&ms, (2000.0, 0.0)), Some(1));
        assert_eq!(pick_monitor(&ms, (2000.0, -100.0)), Some(1));
    }

    #[test]
    fn shared_edge_belongs_to_the_right_hand_monitor() {
        let ms = two_monitors();
        assert_eq!(pick_monitor(&ms, (1439.9, 10.0)), Some(0));
        assert_eq!(pick_monitor(&ms, (1440.0, 10.0)), Some(1));
    }

    #[test]
    fn mouse_in_a_gap_uses_nearest() {
        let ms = vec![Rect::new(0.0, 0.0, 100.0, 100.0), Rect::new(200.0, 0.0, 100.0, 100.0)];
        assert_eq!(pick_monitor(&ms, (130.0, 50.0)), Some(0));
        assert_eq!(pick_monitor(&ms, (180.0, 50.0)), Some(1));
        assert_eq!(pick_monitor(&ms, (-500.0, -500.0)), Some(0));
    }

    #[test]
    fn no_monitors_no_position() {
        assert_eq!(pick_monitor(&[], (0.0, 0.0)), None);
        assert_eq!(launcher_position(&[], (0.0, 0.0), 700.0, 0.2), None);
    }

    #[test]
    fn centred_horizontally_and_a_fifth_down() {
        let ms = two_monitors();
        let (x, y) = launcher_position(&ms, (2000.0, 0.0), 700.0, 0.2).unwrap();
        assert_eq!(x, 1440.0 + (1920.0 - 700.0) / 2.0);
        assert!((y - (-180.0 + 1080.0 * 0.2)).abs() < 1e-9);
        let (x0, y0) = launcher_position(&ms, (10.0, 10.0), 700.0, 0.2).unwrap();
        assert_eq!((x0, y0), (370.0, 180.0));
    }

    #[test]
    fn window_wider_than_monitor_is_pinned_left() {
        let (x, _) = origin_on(Rect::new(50.0, 0.0, 600.0, 400.0), 900.0, 0.2);
        assert_eq!(x, 50.0);
    }
}
