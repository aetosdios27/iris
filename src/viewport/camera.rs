use glam::Vec2;

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub position: Vec2,
    pub zoom: f32,
    pub rotation: f32,
    pub viewport_width: u32,
    pub viewport_height: u32,
}

impl Camera {
    pub fn new() -> Self {
        Self {
            position: Vec2::ZERO,
            zoom: 1.0,
            rotation: 0.0,
            viewport_width: 1,
            viewport_height: 1,
        }
    }

    pub fn set_viewport_size(&mut self, width: u32, height: u32) {
        self.viewport_width = width.max(1);
        self.viewport_height = height.max(1);
    }

    pub fn set_rotation_degrees(&mut self, degrees: f32) {
        self.rotation = degrees.to_radians();
    }

    pub fn fit_scale(&self, image_width: f32, image_height: f32) -> [f32; 2] {
        let vw = self.viewport_width as f32;
        let vh = self.viewport_height as f32;

        if vw <= 0.0 || vh <= 0.0 || image_width <= 0.0 || image_height <= 0.0 {
            return [1.0, 1.0];
        }

        let viewport_aspect = vw / vh;

        let deg = ((self.rotation.to_degrees().round() as i32) % 360 + 360) % 360;
        let is_sideways = deg == 90 || deg == 270;

        let (eff_w, eff_h) = if is_sideways {
            (image_height, image_width)
        } else {
            (image_width, image_height)
        };

        let eff_aspect = eff_w / eff_h;
        let ratio = eff_aspect / viewport_aspect;

        if is_sideways {
            if ratio <= 1.0 {
                [1.0, ratio]
            } else {
                [1.0 / ratio, 1.0]
            }
        } else {
            if ratio <= 1.0 {
                [ratio, 1.0]
            } else {
                [1.0, 1.0 / ratio]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) {
        assert!(
            (a - b).abs() < 0.0001,
            "expected {a} ≈ {b}, diff={}",
            (a - b).abs()
        );
    }

    #[test]
    fn fit_scale_landscape_image_in_landscape_viewport() {
        let mut cam = Camera::new();
        cam.set_viewport_size(1600, 900); // 16:9
        let scale = cam.fit_scale(4000.0, 3000.0); // 4:3

        approx_eq(scale[0], 0.75);
        approx_eq(scale[1], 1.0);
    }

    #[test]
    fn fit_scale_portrait_image_in_landscape_viewport() {
        let mut cam = Camera::new();
        cam.set_viewport_size(1600, 900);
        let scale = cam.fit_scale(3000.0, 4000.0);

        approx_eq(scale[0], 0.421875);
        approx_eq(scale[1], 1.0);
    }

    #[test]
    fn fit_scale_sideways_90_swaps_axes_correctly() {
        let mut cam = Camera::new();
        cam.set_viewport_size(1600, 900);
        cam.set_rotation_degrees(90.0);

        let scale = cam.fit_scale(4000.0, 3000.0);

        approx_eq(scale[0], 1.0);
        approx_eq(scale[1], 0.421875);
    }

    #[test]
    fn fit_scale_sideways_270_swaps_axes_correctly() {
        let mut cam = Camera::new();
        cam.set_viewport_size(1600, 900);
        cam.set_rotation_degrees(270.0);

        let scale = cam.fit_scale(4000.0, 3000.0);

        approx_eq(scale[0], 1.0);
        approx_eq(scale[1], 0.421875);
    }

    #[test]
    fn fit_scale_wide_image_fits_width() {
        let mut cam = Camera::new();
        cam.set_viewport_size(1600, 900);
        let scale = cam.fit_scale(5000.0, 1000.0);

        approx_eq(scale[0], 1.0);
        approx_eq(scale[1], 0.35555556);
    }

    #[test]
    fn fit_scale_invalid_inputs_returns_identity() {
        let mut cam = Camera::new();
        cam.set_viewport_size(0, 0);

        let scale = cam.fit_scale(0.0, 0.0);
        approx_eq(scale[0], 1.0);
        approx_eq(scale[1], 1.0);
    }
}
