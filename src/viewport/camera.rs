use glam::{Mat4, Vec2, Vec3};

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub position: Vec2,
    pub zoom: f32,
    pub aspect_ratio: f32,
}

impl Camera {
    pub fn new() -> Self {
        Self {
            position: Vec2::ZERO,
            zoom: 1.0,
            aspect_ratio: 1.0,
        }
    }

    pub fn set_viewport_size(&mut self, width: u32, height: u32) {
        if height > 0 {
            self.aspect_ratio = width as f32 / height as f32;
        }
    }

    pub fn build_view_projection_matrix(&self) -> Mat4 {
        // Orthographic projection: Map camera view to screen coordinates
        let projection =
            Mat4::orthographic_rh(-self.aspect_ratio, self.aspect_ratio, -1.0, 1.0, -1.0, 1.0);

        // View matrix: The inverse of the camera's position
        let view = Mat4::from_scale(Vec3::new(self.zoom, self.zoom, 1.0))
            * Mat4::from_translation(Vec3::new(-self.position.x, -self.position.y, 0.0));

        projection * view
    }
}
