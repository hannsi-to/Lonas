// フレーム番号とフレーム中かどうか（§9）。GpuContext・DrawManager*・Object* で共有する
// 1 つの AtomicU64 の最下位ビットにフレーム中かどうか、残りにフレーム番号を入れる

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const IN_FRAME: u64 = 1;

#[derive(Clone, Debug, Default)]
pub(crate) struct FrameClock(Arc<AtomicU64>);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct FrameState {
    pub number: u64,
    pub in_frame: bool,
}

impl FrameClock {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn state(&self) -> FrameState {
        let value = self.0.load(Ordering::Acquire);

        FrameState { number: value >> 1, in_frame: value & IN_FRAME != 0 }
    }

    // フレーム番号を進めてフレームに入る
    pub(crate) fn begin(&self) {
        let state = self.state();

        if state.in_frame {
            panic!("begin_frame が二重に呼ばれました（end_frame の呼び忘れ）");
        }

        self.0.store(((state.number + 1) << 1) | IN_FRAME, Ordering::Release);
    }

    pub(crate) fn end(&self) {
        let state = self.state();

        if !state.in_frame {
            panic!("begin_frame の前に end_frame が呼ばれました");
        }

        self.0.store(state.number << 1, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_and_end_advance_frame_number() {
        let clock = FrameClock::new();
        let shared = clock.clone();

        assert_eq!(clock.state(), FrameState { number: 0, in_frame: false });
        clock.begin();
        // 複製したものからも同じ状態が見える
        assert_eq!(shared.state(), FrameState { number: 1, in_frame: true });
        clock.end();
        assert_eq!(shared.state(), FrameState { number: 1, in_frame: false });
        clock.begin();
        assert_eq!(shared.state().number, 2);
    }

    #[test]
    #[should_panic(expected = "begin_frame が二重に呼ばれました")]
    fn double_begin_panics() {
        let clock = FrameClock::new();
        clock.begin();
        clock.begin();
    }

    #[test]
    #[should_panic(expected = "begin_frame の前に end_frame が呼ばれました")]
    fn end_without_begin_panics() {
        FrameClock::new().end();
    }
}
