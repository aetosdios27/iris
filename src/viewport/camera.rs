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

        // Normalize rotation to 0..360 degrees
        let deg = ((self.rotation.to_degrees().round() as i32) % 360 + 360) % 360;
        let is_sideways = deg == 90 || deg == 270;

        // After rotation, the effective image dimensions change:
        // - 0°/180°: width and height stay the same
        // - 90°/270°: width and height swap
        let (eff_w, eff_h) = if is_sideways {
            (image_height, image_width)
        } else {
            (image_width, image_height)
        };

        let eff_aspect = eff_w / eff_h;

        // After rotation in the shader:
        // - 0°/180°: visual_width ∝ scale_x, visual_height ∝ scale_y (normal)
        // - 90°/270°: visual_width ∝ scale_y, visual_height ∝ scale_x (swapped)
        //
        // We compute the ratio to fit the effective image into the viewport,
        // then assign scale_x and scale_y accordingly.

        let ratio = eff_aspect / viewport_aspect;

        if is_sideways {
            // Rotation swaps axes: scale_y controls visual width, scale_x controls visual height
            if ratio <= 1.0 {
                // Effective image is taller than viewport → fit to height
                // visual_height fills viewport (scale_x = 1.0)
                // visual_width is smaller (scale_y = ratio)
                [1.0, ratio]
            } else {
                // Effective image is wider than viewport → fit to width
                // visual_width fills viewport (scale_y = 1.0)
                // visual_height is smaller (scale_x = 1/ratio)
                [1.0 / ratio, 1.0]
            }
        } else {
            // No swap: scale_x controls visual width, scale_y controls visual height
            if ratio <= 1.0 {
                // Image taller → fit to height
                [ratio, 1.0]
            } else {
                // Image wider → fit to width
                [1.0, 1.0 / ratio]
            }
        }
    }
}
