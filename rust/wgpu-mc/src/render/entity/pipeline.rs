use crate::model::BindableTexture;
use std::sync::Arc;

pub struct EntityGroupInstancingFrame {
    ///The model for the entity
    pub vertex_buffer: Arc<wgpu::Buffer>,
    ///`EntityRenderInstance`s
    pub entity_instance_vb: Arc<wgpu::Buffer>,

    ///mat4[][] for part transforms per instance
    pub part_transform_matrices: Arc<wgpu::BindGroup>,
    ///vec2[] for offsets for mob variant textures
    pub texture_offsets: Arc<wgpu::BindGroup>,
    ///the texture
    pub texture: Arc<BindableTexture>,
    ///how many entities to draw
    pub instance_count: u32,
    ///how many vertices per entity
    pub vertex_count: u32,
}
