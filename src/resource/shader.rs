// WGSL の検査と解析（§11 ResourceManager・シェーダーとの約束事・uniform の型の検査）

use std::collections::HashMap;

use naga::proc::Layouter;
use naga::valid::{Capabilities, ValidationFlags, Validator};
use naga::{AddressSpace, ArraySize, StorageAccess, TypeInner};

use crate::error::LonasError;

// シェーダーの中のバインディングの種類
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum BindingKind {
    Uniform,
    Storage { read_only: bool },
    // テクスチャ・サンプラーなど
    Handle,
}

// 実行時に長さが決まる配列（array<T>）の要素
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArrayElement {
    pub size: u32,
    pub stride: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct BindingInfo {
    pub kind: BindingKind,
    // 変数の型の大きさ。実行時に長さが決まる配列は要素 0 個分（WGSL の規則）
    pub size: u32,
    pub array_element: Option<ArrayElement>,
}

// 段階 6 で、BindingConfig の位置にある変数の型の大きさを検査するために使う
#[derive(Clone, Debug, Default)]
pub(crate) struct ShaderReflection {
    bindings: HashMap<(u32, u32), BindingInfo>,
}

impl ShaderReflection {
    pub(crate) fn binding(&self, group: u32, binding: u32) -> Option<BindingInfo> {
        self.bindings.get(&(group, binding)).copied()
    }
}

#[derive(Debug)]
pub(crate) struct CheckedShader {
    pub module: naga::Module,
    pub reflection: ShaderReflection,
}

// naga で WGSL を読み、検査する。誤りがあれば位置を含むメッセージで Err
pub(crate) fn check_wgsl(source: &str) -> Result<CheckedShader, LonasError> {
    let module = naga::front::wgsl::parse_str(source)
        .map_err(|error| LonasError::InvalidShader { message: error.emit_to_string(source) })?;

    // 機能の有無はアダプター次第のため、ここでは既定の範囲で検査する。最終的な検査は GPU 上で作るとき（段階 4 以降）
    Validator::new(ValidationFlags::all(), Capabilities::default())
        .validate(&module)
        .map_err(|error| LonasError::InvalidShader { message: error.emit_to_string(source) })?;

    let reflection = reflect(&module)
        .map_err(|error| LonasError::InvalidShader { message: format!("型の大きさを求められません: {error}") })?;

    Ok(CheckedShader { module, reflection })
}

fn reflect(module: &naga::Module) -> Result<ShaderReflection, naga::proc::LayoutError> {
    let mut layouter = Layouter::default();
    layouter.update(module.to_ctx())?;

    let mut bindings = HashMap::new();

    for (_, variable) in module.global_variables.iter() {
        let Some(binding) = &variable.binding else { continue };

        let kind = match variable.space {
            AddressSpace::Uniform => BindingKind::Uniform,
            AddressSpace::Storage { access } => BindingKind::Storage { read_only: !access.contains(StorageAccess::STORE) },
            _ => BindingKind::Handle,
        };
        let array_element = match module.types[variable.ty].inner {
            TypeInner::Array { base, size: ArraySize::Dynamic, stride } => {
                Some(ArrayElement { size: layouter[base].size, stride })
            }
            _ => None,
        };

        bindings.insert(
            (binding.group, binding.binding),
            BindingInfo { kind, size: layouter[variable.ty].size, array_element },
        );
    }

    Ok(ShaderReflection { bindings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wgsl::{CAMERA_SIZE, INSTANCE_2D_SIZE, INSTANCE_3D_SIZE, STRUCTS, TEXTURE_RECT_SIZE};

    #[test]
    fn invalid_wgsl_is_rejected_with_location() {
        let message = check_wgsl("fn main( {").unwrap_err().to_string();

        // 行と列の位置が入る
        assert!(message.contains(":1:"), "{message}");

        // 文法は正しいが、型が合わない
        assert!(check_wgsl("fn f() -> f32 { return 1u; }").is_err());
    }

    #[test]
    fn structs_match_gpu_sizes() {
        let source = format!(
            "{STRUCTS}
            @group(0) @binding(0) var<uniform> camera: Camera;
            @group(0) @binding(1) var<storage, read> instances_2d: array<Instance2D>;
            @group(0) @binding(2) var<storage, read> instances_3d: array<Instance3D>;
            @group(0) @binding(3) var<storage, read> texture_rects: array<TextureRect>;"
        );
        let reflection = check_wgsl(&source).unwrap().reflection;
        let element = |binding| reflection.binding(0, binding).unwrap().array_element.unwrap();

        assert_eq!(reflection.binding(0, 0).unwrap().size, CAMERA_SIZE);
        assert_eq!(element(1), ArrayElement { size: INSTANCE_2D_SIZE, stride: INSTANCE_2D_SIZE });
        assert_eq!(element(2), ArrayElement { size: INSTANCE_3D_SIZE, stride: INSTANCE_3D_SIZE });
        assert_eq!(element(3), ArrayElement { size: TEXTURE_RECT_SIZE, stride: TEXTURE_RECT_SIZE });
    }

    #[test]
    fn rust_side_layout_matches_wgsl() {
        use crate::instance::{GpuInstance2D, GpuInstance3D};

        assert_eq!(size_of::<GpuInstance2D>() as u32, INSTANCE_2D_SIZE);
        assert_eq!(size_of::<GpuInstance3D>() as u32, INSTANCE_3D_SIZE);
    }

    #[test]
    fn reflects_binding_kinds_and_uniform_array_stride() {
        let source = "
            struct Wave { time: f32, amplitude: f32, offset: vec3<f32> }
            @group(1) @binding(0) var atlas: texture_2d_array<f32>;
            @group(1) @binding(1) var atlas_sampler: sampler;
            @group(2) @binding(0) var<storage, read> uniforms: array<Wave>;
            @group(2) @binding(1) var<storage, read_write> output: array<u32>;";
        let reflection = check_wgsl(source).unwrap().reflection;

        assert_eq!(reflection.binding(1, 0).unwrap().kind, BindingKind::Handle);
        assert_eq!(reflection.binding(1, 1).unwrap().kind, BindingKind::Handle);
        assert_eq!(reflection.binding(2, 1).unwrap().kind, BindingKind::Storage { read_only: false });

        let uniforms = reflection.binding(2, 0).unwrap();
        assert_eq!(uniforms.kind, BindingKind::Storage { read_only: true });
        // vec3 の境界（16 バイト）に合わせた詰め物が入る：大きさ 32、間隔 32
        assert_eq!(uniforms.array_element, Some(ArrayElement { size: 32, stride: 32 }));
        assert_eq!(reflection.binding(3, 0), None);
    }
}
