// 一定フレーム end されなかった Object* の領域の回収（§6 解放：GC 方式・回収の通知）
// Object* は GpuContext を知らないため Drop で解放できない。最後に end されたフレームを記録し、期限切れを回収する

use std::collections::HashMap;

use super::{GpuAllocation, ManagerId};

#[derive(Debug, Default)]
pub(crate) struct GcTracker {
    last_used_frame: HashMap<GpuAllocation, u64>,
    // 回収したハンドルを、発行元の DrawManager* に渡すまで溜めておく
    reclaimed: HashMap<ManagerId, Vec<GpuAllocation>>,
}

impl GcTracker {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    // そのフレームで end されたことを記録する
    pub(crate) fn touch(&mut self, handle: GpuAllocation, frame: u64) {
        self.last_used_frame.insert(handle, frame);
    }

    // 明示的に解放したハンドルは追いかけない
    pub(crate) fn forget(&mut self, handle: GpuAllocation) {
        self.last_used_frame.remove(&handle);
    }

    // unused_frames フレームの間 end されなかったハンドルを回収して返す
    // 呼び出し側はヒープの領域を解放する。回収したハンドルは DrawManager* ごとに溜まる
    pub(crate) fn collect(&mut self, current_frame: u64, unused_frames: u32) -> Vec<GpuAllocation> {
        let expired: Vec<GpuAllocation> = self
            .last_used_frame
            .iter()
            .filter(|&(_, &last)| current_frame - last >= unused_frames as u64)
            .map(|(&handle, _)| handle)
            .collect();

        for handle in &expired {
            self.last_used_frame.remove(handle);
            self.reclaimed.entry(handle.manager).or_default().push(*handle);
        }

        expired
    }

    // その DrawManager* が end_frame に渡されたときに、溜めていた回収済みのハンドルを渡す
    pub(crate) fn take_reclaimed(&mut self, manager: ManagerId) -> Vec<GpuAllocation> {
        self.reclaimed.remove(&manager).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(manager: u32, serial: u32) -> GpuAllocation {
        GpuAllocation { manager: ManagerId(manager), serial }
    }

    #[test]
    fn collects_exactly_after_unused_frames() {
        let mut gc = GcTracker::new();
        gc.touch(handle(0, 0), 10);

        // フレーム 11〜129 の 119 フレームでは、まだ回収しない
        assert!(gc.collect(129, 120).is_empty());
        // 120 フレーム end されなかったので回収する
        assert_eq!(gc.collect(130, 120), vec![handle(0, 0)]);
        // 一度回収したものは二度回収しない
        assert!(gc.collect(500, 120).is_empty());
    }

    #[test]
    fn touch_postpones_collection() {
        let mut gc = GcTracker::new();
        gc.touch(handle(0, 0), 10);
        gc.touch(handle(0, 0), 100);

        assert!(gc.collect(130, 120).is_empty());
        assert_eq!(gc.collect(220, 120), vec![handle(0, 0)]);
    }

    #[test]
    fn forgotten_handles_are_not_collected() {
        let mut gc = GcTracker::new();
        gc.touch(handle(0, 0), 0);
        gc.forget(handle(0, 0));

        assert!(gc.collect(1000, 1).is_empty());
    }

    #[test]
    fn reclaimed_handles_wait_per_manager() {
        let mut gc = GcTracker::new();
        gc.touch(handle(0, 0), 0);
        gc.touch(handle(1, 0), 0);
        gc.touch(handle(1, 1), 0);
        gc.collect(10, 5);

        let mut reclaimed = gc.take_reclaimed(ManagerId(1));
        reclaimed.sort_by_key(|h| h.serial);
        assert_eq!(reclaimed, vec![handle(1, 0), handle(1, 1)]);
        // 渡した後は空になる。別の DrawManager* の分は残る
        assert!(gc.take_reclaimed(ManagerId(1)).is_empty());
        assert_eq!(gc.take_reclaimed(ManagerId(0)), vec![handle(0, 0)]);
    }
}
