// LonasConfig とその中の型（§3・§8・§11）
// すべての項目に既定値を設けない。ユーザーが必ず全項目を指定する（§3 LonasConfig）

use std::collections::HashMap;

pub struct LonasConfig {
    /// 設定の関数が同値で呼ばれたときに log で警告するか（§3）
    pub redundant_call_warning: bool,
    /// インスタンス数がこのフレーム数だけ連続で増加したら警告する。None の場合は警告しない（§7）
    pub instance_growth_warning_frames: Option<u32>,
    /// 用途ごとのメモリ空間の割り当て量（§6）
    pub memory: MemoryConfig,
    /// staging バッファのチャンクの大きさ（バイト、§6 転送）
    pub staging_chunk_size: u64,
    /// Dynamic 領域が不足したときの拡張倍率（§6）
    /// 拡張後の容量 = max(現在の容量 × この値, 現在の容量 + 必要なサイズ)
    pub dynamic_growth_factor: f32,
    /// このフレーム数だけ end されなかった Object の領域を回収する（§6）
    pub gc_unused_frames: u32,
    /// Static 領域の穴の合計が Static 領域に対してこの割合（0.0〜1.0）を超えたらコンパクションする（§6）
    pub static_compaction_threshold: f32,
    /// blend() を一度も呼ばなかった Object が使うブレンド（§10）
    pub blend_when_unset: Blend,
    /// layer() を一度も呼ばなかった Object2D が属する層（§10）
    pub layer_when_unset: i32,
    /// cull_mode() を一度も呼ばなかった Object が使う背面カリング（§11）
    pub cull_mode_when_unset: CullMode,
    /// シェーダーに公開するバインドグループ・バインディング番号（§11）
    pub bindings: BindingConfig,
    /// 2D の原点・3D の座標系（§11）
    pub coordinate_system: CoordinateSystemConfig,
    /// 2D で視界の外を省く処理を行うか（§11）
    pub culling_2d: bool,
    /// 各サーフェスの最初のアンチエイリアスの方式（§11）
    /// GpuContext::new と add_surface で作るすべてのサーフェスがこの値で始まる。実行中は set_anti_aliasing で変更できる
    pub initial_anti_aliasing: AntiAliasing,
    /// 使う GPU の API（§8）
    pub backend: Backend,
    /// 各サーフェスの最初の Present mode（§8）。実行中は set_present_mode で変更できる
    pub initial_present_mode: PresentMode,
    /// TAA の細かい値（§11）
    pub taa: TaaConfig,
    /// TextureAtlas の大きさ・レイヤー数など（§11）
    pub texture_atlas: TextureAtlasConfig,
}

pub struct MemoryConfig {
    pub vertex: MemoryCapacity,
    pub index: MemoryCapacity,
    /// インスタンスは常に Dynamic のため、Dynamic の初期量のみ
    pub instance_dynamic_initial_capacity: u64,
    /// uniform の配列（DrawManager × シェーダーごと）の最初の要素数（§6 uniform の領域）
    pub uniform_initial_elements: u32,
}

pub struct MemoryCapacity {
    /// Static 領域の割り当て量（バイト）。超えると panic
    pub static_capacity: u64,
    /// Dynamic 領域の初期の割り当て量（バイト）。不足時のみ拡張
    pub dynamic_initial_capacity: u64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Blend {
    Opaque,
    Alpha,
    Additive,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum CullMode {
    None,
    Back,
    Front,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct BindingSlot {
    pub group: u32,
    pub binding: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BindingConfig {
    pub camera: BindingSlot,
    pub instances: BindingSlot,
    pub instance_order: BindingSlot,
    pub atlas_texture: BindingSlot,
    pub atlas_sampler: BindingSlot,
    pub texture_rects: BindingSlot,
    pub uniform: BindingSlot,
    pub previous_camera: BindingSlot,
    pub previous_instances: BindingSlot,
}

impl BindingConfig {
    fn slots(&self) -> [(&'static str, BindingSlot); 9] {
        [
            ("camera", self.camera),
            ("instances", self.instances),
            ("instance_order", self.instance_order),
            ("atlas_texture", self.atlas_texture),
            ("atlas_sampler", self.atlas_sampler),
            ("texture_rects", self.texture_rects),
            ("uniform", self.uniform),
            ("previous_camera", self.previous_camera),
            ("previous_instances", self.previous_instances),
        ]
    }

    pub(crate) fn max_group(&self) -> u32 {
        self.slots().iter().map(|(_, slot)| slot.group).max().unwrap_or(0)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CoordinateSystemConfig {
    pub origin_2d: Origin2D,
    pub handedness_3d: Handedness,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Origin2D {
    // 左上が原点、y は下向き
    TopLeft,
    // 左下が原点、y は上向き
    BottomLeft,
    // 中央が原点、y は上向き
    Center,
}

// 3D の手系（§11 座標系）
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Handedness {
    // カメラは -Z を向く
    RightHanded,
    // カメラは +Z を向く
    LeftHanded,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum AntiAliasing {
    None,
    Msaa { sample_count: u32 },
    Fxaa,
    Taa,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Backend {
    // wgpu が環境に合わせて選ぶ
    Auto,
    Vulkan,
    DirectX12,
    Metal,
    OpenGl,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum PresentMode {
    // 垂直同期あり。使えるものを選ぶ（FifoRelaxed → Fifo）
    AutoVsync,
    // 垂直同期なし。使えるものを選ぶ（Immediate → Mailbox → Fifo）
    AutoNoVsync,
    Fifo,
    FifoRelaxed,
    Mailbox,
    Immediate,
}

pub struct TaaConfig {
    /// 履歴を混ぜる割合の範囲（信頼度が最低のとき〜最高のとき）
    pub history_blend_min: f32,
    pub history_blend_max: f32,
    /// この深度差を超えたら履歴の信頼度を下げる
    pub depth_rejection_threshold: f32,
    /// 分散クリップの範囲の広さ（周囲の色の標準偏差の倍率）
    pub color_rejection_threshold: f32,
    /// 動きのベクトルを書き込む画像のフォーマット
    pub motion_vector_format: MotionVectorFormat,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MotionVectorFormat {
    Rg16Float,
    Rg32Float,
}

pub struct TextureAtlasConfig {
    /// 1 レイヤーの幅・高さ（画素、正方形）
    pub size: u32,
    /// 最初に作るレイヤー数
    pub initial_layers: u32,
    /// レイヤー数の上限。超えたら panic
    pub max_layers: u32,
    /// 各テクスチャの周りの余白（画素）。mip_level_count に合わせて切り上げる
    pub padding: u32,
    /// ミップマップの段数。1 ならミップマップなし
    pub mip_level_count: u32,
    /// 拡大・縮小時の補間
    pub filter: FilterMode,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FilterMode {
    Nearest,
    Linear,
}

impl LonasConfig {
    // 値の検査（GpuContext::new で行う）。不正なら panic（§3）
    // アダプターの上限による検査は、アダプターを得た後に GpuContext が行う
    pub(crate) fn validate(&self) {
        assert!(
            (0.0..=1.0).contains(&self.static_compaction_threshold),
            "static_compaction_threshold は 0.0〜1.0 の範囲で指定してください（{}）",
            self.static_compaction_threshold,
        );
        assert!(
            self.dynamic_growth_factor > 1.0,
            "dynamic_growth_factor は 1.0 より大きくしてください（{}）。拡張しても容量が増えません",
            self.dynamic_growth_factor,
        );
        assert!(
            self.staging_chunk_size > 0 && self.staging_chunk_size % 4 == 0,
            "staging_chunk_size は 0 より大きい 4 の倍数にしてください（{}）",
            self.staging_chunk_size,
        );

        let mut used: HashMap<BindingSlot, &str> = HashMap::new();

        for (name, slot) in self.bindings.slots() {
            if let Some(other) = used.insert(slot, name) {
                panic!(
                    "BindingConfig の {other} と {name} が同じ番号（group {}, binding {}）です",
                    slot.group, slot.binding,
                );
            }
        }

        let atlas = &self.texture_atlas;
        assert!(
            atlas.initial_layers > 0 && atlas.initial_layers <= atlas.max_layers,
            "texture_atlas の initial_layers は 1 以上 max_layers 以下にしてください（{} / {}）",
            atlas.initial_layers,
            atlas.max_layers,
        );

        let max_mip_levels = 32 - atlas.size.max(1).leading_zeros();
        assert!(
            atlas.mip_level_count > 0 && atlas.mip_level_count <= max_mip_levels,
            "texture_atlas の mip_level_count は 1〜{max_mip_levels} にしてください（{}）",
            atlas.mip_level_count,
        );

        let taa = &self.taa;
        assert!(
            (0.0..=1.0).contains(&taa.history_blend_min)
                && (0.0..=1.0).contains(&taa.history_blend_max)
                && taa.history_blend_min <= taa.history_blend_max,
            "taa の history_blend_min / history_blend_max は 0.0〜1.0 で、min ≦ max にしてください",
        );
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    // テスト用の設定。値は examples/test1.rs と同じ
    pub(crate) fn sample_config() -> LonasConfig {
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

    #[test]
    fn sample_config_is_valid() {
        sample_config().validate();
    }

    #[test]
    #[should_panic(expected = "static_compaction_threshold")]
    fn rejects_threshold_out_of_range() {
        LonasConfig { static_compaction_threshold: 1.5, ..sample_config() }.validate();
    }

    #[test]
    #[should_panic(expected = "dynamic_growth_factor")]
    fn rejects_growth_factor_not_above_one() {
        LonasConfig { dynamic_growth_factor: 1.0, ..sample_config() }.validate();
    }

    #[test]
    #[should_panic(expected = "staging_chunk_size")]
    fn rejects_unaligned_staging_chunk() {
        LonasConfig { staging_chunk_size: 1001, ..sample_config() }.validate();
    }

    #[test]
    #[should_panic(expected = "camera と uniform が同じ番号")]
    fn rejects_duplicate_binding_slots() {
        let mut config = sample_config();
        config.bindings.uniform = config.bindings.camera;
        config.validate();
    }

    #[test]
    #[should_panic(expected = "mip_level_count")]
    fn rejects_too_many_mip_levels() {
        let mut config = sample_config();
        config.texture_atlas.size = 256; // 作れる段数は 9
        config.texture_atlas.mip_level_count = 10;
        config.validate();
    }

    #[test]
    #[should_panic(expected = "initial_layers")]
    fn rejects_initial_layers_above_max() {
        let mut config = sample_config();
        config.texture_atlas.initial_layers = 5;
        config.validate();
    }
}
