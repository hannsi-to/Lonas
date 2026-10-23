// 必須の機能の判定（§8・§11 環境の判定）。機能の有無で描き方を切り替えることはしない
// wgpu 30 では multi_draw_indexed_indirect に専用の機能は要らず、INDIRECT_EXECUTION があれば使える（使えない環境では wgpu が内部で代わりに処理する）

use wgpu::{DownlevelFlags, Features};

// アダプターに求める機能
pub(crate) const REQUIRED_FEATURES: Features = Features::INDIRECT_FIRST_INSTANCE;

const REQUIRED_DOWNLEVEL: [(DownlevelFlags, &str); 3] = [
    (DownlevelFlags::VERTEX_STORAGE, "VERTEX_STORAGE（頂点シェーダーでの storage バッファ）"),
    (DownlevelFlags::COMPUTE_SHADERS, "COMPUTE_SHADERS"),
    (DownlevelFlags::INDIRECT_EXECUTION, "INDIRECT_EXECUTION（間接描画）"),
];

// 足りない機能があれば、その名前を返す
pub(crate) fn find_missing(features: Features, downlevel: DownlevelFlags) -> Option<&'static str> {
    if let Some((_, name)) = REQUIRED_DOWNLEVEL.iter().find(|(flag, _)| !downlevel.contains(*flag)) {
        return Some(name);
    }

    if !features.contains(REQUIRED_FEATURES) {
        return Some("INDIRECT_FIRST_INSTANCE（間接描画で 0 以外の first_instance）");
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_support_passes() {
        assert_eq!(find_missing(Features::INDIRECT_FIRST_INSTANCE, DownlevelFlags::all()), None);
    }

    #[test]
    fn reports_missing_capability() {
        let without_vertex_storage = DownlevelFlags::all() - DownlevelFlags::VERTEX_STORAGE;

        assert!(find_missing(Features::INDIRECT_FIRST_INSTANCE, without_vertex_storage).unwrap().starts_with("VERTEX_STORAGE"));
        assert!(find_missing(Features::empty(), DownlevelFlags::all()).unwrap().starts_with("INDIRECT_FIRST_INSTANCE"));
    }
}
