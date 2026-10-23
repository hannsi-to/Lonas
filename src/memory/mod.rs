// GPU メモリの管理のうち、CPU 側で行う計算（§6）。GPU のバッファの作成・コピーは段階 5 で行う
#![allow(dead_code)] // 段階 5（GpuContext のヒープ）で使う

pub(crate) mod gc;
pub(crate) mod space;
pub(crate) mod static_stack;
pub(crate) mod tlsf;

// 変更がほとんど起こらないものと、毎フレーム変更が行われるもので管理を分ける（§6 Static / Dynamic の区別）
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MemoryUpdateType {
    Static,
    Dynamic,
}

// DrawManager* の識別子（§5 DrawManager は複数作成可能）
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ManagerId(pub u32);

// ヒープ上の領域を指すハンドル（§6 DrawManager 経由のアクセス）
// DrawManager* が ManagerId + 連番で即座に発行し、実際の位置との対応は GpuContext が持つ
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct GpuAllocation {
    pub manager: ManagerId,
    pub serial: u32,
}
