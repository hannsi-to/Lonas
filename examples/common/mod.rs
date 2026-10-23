// examples で共有する設定。LonasConfig は既定値を持たないため、すべての項目を書く（§3）

use lonas::lonas::{
    AntiAliasing, Backend, BindingConfig, BindingSlot, Blend, CoordinateSystemConfig, CullMode, FilterMode, Handedness,
    LonasConfig, MemoryCapacity, MemoryConfig, MotionVectorFormat, Origin2D, PresentMode, TaaConfig, TextureAtlasConfig,
};

pub fn config() -> LonasConfig {
    LonasConfig {
        redundant_call_warning: true,
        instance_growth_warning_frames: Some(60),
        memory: MemoryConfig {
            vertex: MemoryCapacity { static_capacity: 1 << 20, dynamic_initial_capacity: 1 << 20 },
            index: MemoryCapacity { static_capacity: 1 << 20, dynamic_initial_capacity: 1 << 20 },
            instance_dynamic_initial_capacity: 1 << 20,
            uniform_initial_elements: 64,
        },
        staging_chunk_size: 1 << 20,
        dynamic_growth_factor: 1.5,
        gc_unused_frames: 120,
        static_compaction_threshold: 0.25,
        blend_when_unset: Blend::Opaque,
        layer_when_unset: 0,
        cull_mode_when_unset: CullMode::Back,
        bindings: BindingConfig {
            camera: BindingSlot { group: 0, binding: 0 },
            instances: BindingSlot { group: 0, binding: 1 },
            instance_order: BindingSlot { group: 0, binding: 2 },
            atlas_texture: BindingSlot { group: 1, binding: 0 },
            atlas_sampler: BindingSlot { group: 1, binding: 1 },
            texture_rects: BindingSlot { group: 1, binding: 2 },
            uniform: BindingSlot { group: 2, binding: 0 },
            previous_camera: BindingSlot { group: 3, binding: 0 },
            previous_instances: BindingSlot { group: 3, binding: 1 },
        },
        coordinate_system: CoordinateSystemConfig {
            origin_2d: Origin2D::TopLeft,
            handedness_3d: Handedness::RightHanded,
        },
        culling_2d: true,
        initial_anti_aliasing: AntiAliasing::None,
        backend: Backend::Auto,
        initial_present_mode: PresentMode::AutoVsync,
        taa: TaaConfig {
            history_blend_min: 0.1,
            history_blend_max: 0.95,
            depth_rejection_threshold: 0.01,
            color_rejection_threshold: 1.25,
            motion_vector_format: MotionVectorFormat::Rg16Float,
        },
        texture_atlas: TextureAtlasConfig {
            size: 2048,
            initial_layers: 1,
            max_layers: 4,
            padding: 2,
            mip_level_count: 4,
            filter: FilterMode::Linear,
        },
    }
}
