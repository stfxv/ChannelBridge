//! One atomic snapshot for channel/mode/mute/preview/stream-generation changes.
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Mono,
    Stereo,
    Mix,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    left: usize,
    right: usize,
    mode: Mode,
    muted: bool,
}
impl Selection {
    /// Accepts one-based channels in `1..=512` and stores zero-based indices.
    /// Mono uses `left` for both channels and ignores `right`. Returns `None`
    /// when a channel used by the selected mode is outside that range.
    #[must_use]
    pub fn new(left: u32, right: u32, mode: Mode, muted: bool) -> Option<Self> {
        if !(1..=512).contains(&left) {
            return None;
        }
        let right = if mode == Mode::Mono { left } else { right };
        if !(1..=512).contains(&right) {
            return None;
        }
        Some(Self {
            left: left as usize - 1,
            right: right as usize - 1,
            mode,
            muted,
        })
    }
    #[must_use]
    pub fn left(&self) -> usize {
        self.left
    }
    #[must_use]
    pub fn right(&self) -> usize {
        self.right
    }
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }
    #[must_use]
    pub fn muted(&self) -> bool {
        self.muted
    }
    #[must_use]
    pub fn available(&self, channels: usize) -> bool {
        self.left < channels && self.right < channels
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Routing {
    pub selection: Selection,
    pub sending: bool,
    pub overview: bool,
    pub generation: u64,
}
const GENERATION_SHIFT: u32 = 23;
const MAX_GENERATION: u64 = u64::MAX >> GENERATION_SHIFT;
impl Routing {
    #[must_use]
    fn encode(self) -> u64 {
        let mode = match self.selection.mode {
            Mode::Mono => 0,
            Mode::Stereo => 1,
            Mode::Mix => 2,
        };
        self.selection.left as u64
            | ((self.selection.right as u64) << 9)
            | (mode << 18)
            | (u64::from(self.selection.muted) << 20)
            | (u64::from(self.overview) << 21)
            | (u64::from(self.sending) << 22)
            | (self.generation << GENERATION_SHIFT)
    }
    #[must_use]
    fn decode(word: u64) -> Self {
        Self {
            selection: Selection {
                left: (word & 511) as usize,
                right: ((word >> 9) & 511) as usize,
                mode: match (word >> 18) & 3 {
                    1 => Mode::Stereo,
                    2 => Mode::Mix,
                    _ => Mode::Mono,
                },
                muted: word & (1 << 20) != 0,
            },
            overview: word & (1 << 21) != 0,
            sending: word & (1 << 22) != 0,
            generation: word >> GENERATION_SHIFT,
        }
    }
}

pub struct AtomicRouting {
    word: AtomicU64,
}
impl AtomicRouting {
    #[must_use]
    pub fn new(selection: Selection) -> Self {
        Self {
            word: AtomicU64::new(
                Routing {
                    selection,
                    sending: false,
                    overview: false,
                    generation: 1,
                }
                .encode(),
            ),
        }
    }
    #[must_use]
    pub fn load(&self) -> Routing {
        Routing::decode(self.word.load(Ordering::Acquire))
    }

    /// Control-thread update. Source discontinuities can also change the word;
    /// retry only a bounded number of times, preserving their newer generation.
    #[must_use]
    pub fn configure(
        &self,
        selection: Selection,
        sending: bool,
        overview: bool,
    ) -> Option<Routing> {
        for _ in 0..8 {
            let previous = self.word.load(Ordering::Acquire);
            let current = Routing::decode(previous);
            let generation = current
                .generation
                .checked_add(u64::from(current.sending != sending))?;
            if generation > MAX_GENERATION {
                return None;
            }
            let next = Routing {
                selection,
                sending,
                overview,
                generation,
            };
            if self
                .word
                .compare_exchange(previous, next.encode(), Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(next);
            }
        }
        None
    }

    /// Single attempt from capture on a clock/FIFO discontinuity. No RT retry.
    #[must_use]
    pub fn advance_generation(&self, expected: Routing) -> Option<Routing> {
        let generation = expected.generation.checked_add(1)?;
        if generation > MAX_GENERATION {
            return None;
        }
        let next = Routing {
            generation,
            ..expected
        };
        self.word
            .compare_exchange(
                expected.encode(),
                next.encode(),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .ok()
            .map(|_| next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mode_edits_are_atomic_but_start_stop_retires_old_audio() {
        let mono = Selection::new(512, 999, Mode::Mono, false)
            .expect("mono uses left channel 512 and ignores the saved right channel");
        assert!(!mono.available(511));
        let settings = AtomicRouting::new(mono);
        let start = settings
            .configure(mono, /* sending */ true, /* overview */ false)
            .expect("fixture selection is encodable and generation space is not exhausted");
        let stereo = Selection::new(10, 16, Mode::Stereo, true)
            .expect("fixture channel numbers are nonzero and within the selection encoding");
        let changed = settings
            .configure(stereo, /* sending */ true, /* overview */ true)
            .expect("fixture selection is encodable and generation space is not exhausted");
        assert_eq!(changed.generation, start.generation);
        assert_eq!(settings.load().selection, stereo);
        let stop = settings
            .configure(stereo, /* sending */ false, /* overview */ true)
            .expect("fixture selection is encodable and generation space is not exhausted");
        assert!(stop.generation > changed.generation);
        assert!(settings.advance_generation(changed).is_none());
        assert_eq!(settings.load(), stop);
        assert!(
            settings
                .advance_generation(stop)
                .expect("fixture selection is encodable and generation space is not exhausted")
                .generation
                > stop.generation
        );
    }
}
