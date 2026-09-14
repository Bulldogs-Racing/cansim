//! Real CAN arbitration semantics (PROMPT.md §8).
//!
//! Dominant (0) beats recessive (1); the numerically lowest identifier wins
//! the bus. Mixed standard/extended arbitration follows ISO 11898-1: the
//! 11-bit base IDs are compared first, then the IDE bit (standard `0` beats
//! extended `1`), then the 18-bit extension, then RTR (data `0` beats
//! remote `1`).
//!
//! [`arbitration_key`] exposes this ordering as a tuple so the bus, tests,
//! and (later) the UI visualizer share one implementation.

use super::frame::CanFrame;
use std::cmp::Ordering;

/// Total order key for arbitration: smaller wins.
///
/// `(base_id, ide_bit, extension, rtr_bit)` where `ide_bit` is 0 for
/// standard / 1 for extended and `rtr_bit` is 0 for data / 1 for remote.
pub fn arbitration_key(frame: &CanFrame) -> (u16, u8, u32, u8) {
    let ide = if frame.id.is_standard() { 0 } else { 1 };
    let rtr = if frame.is_remote { 1 } else { 0 };
    (frame.id.base_id(), ide, frame.id.extension_id(), rtr)
}

/// Compare two frames under arbitration rules (lower wins).
pub fn compare_frames(a: &CanFrame, b: &CanFrame) -> Ordering {
    arbitration_key(a).cmp(&arbitration_key(b))
}

/// Rank simultaneous transmission requests: index of the winner plus the
/// losing indices in arbitration order (best loser first).
///
/// Returns `None` when `requests` is empty.
pub fn arbitrate_ranking(requests: &[CanFrame]) -> Option<(usize, Vec<usize>)> {
    if requests.is_empty() {
        return None;
    }
    let mut order: Vec<usize> = (0..requests.len()).collect();
    order.sort_by(|&i, &j| compare_frames(&requests[i], &requests[j]));
    let winner = order[0];
    let losers = order[1..].to_vec();
    Some((winner, losers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::can::id::CanId;

    fn std(id: u16) -> CanFrame {
        CanFrame::new(CanId::new_standard(id).unwrap(), &[0]).unwrap()
    }

    #[test]
    fn lower_standard_id_wins() {
        assert_eq!(compare_frames(&std(0x100), &std(0x300)), Ordering::Less);
        assert_eq!(compare_frames(&std(0x300), &std(0x100)), Ordering::Greater);
        assert_eq!(compare_frames(&std(0x200), &std(0x200)), Ordering::Equal);
    }

    #[test]
    fn standard_beats_extended_on_equal_base() {
        let s = CanFrame::new(CanId::new_standard(0x100).unwrap(), &[0]).unwrap();
        let e = CanFrame::new(CanId::new_extended(0x100 << 18).unwrap(), &[0]).unwrap();
        assert_eq!(s.base_id_check(), e.base_id_check());
        assert_eq!(compare_frames(&s, &e), Ordering::Less);
    }

    #[test]
    fn data_beats_remote_on_equal_id() {
        let id = CanId::new_standard(0x100).unwrap();
        let data = CanFrame::new(id, &[1]).unwrap();
        let rtr = CanFrame::new_remote(id, 1).unwrap();
        assert_eq!(compare_frames(&data, &rtr), Ordering::Less);
    }

    #[test]
    fn ranking_reports_winner_and_losers() {
        let frames = vec![std(0x300), std(0x100), std(0x200)];
        let (winner, losers) = arbitrate_ranking(&frames).unwrap();
        assert_eq!(winner, 1);
        assert_eq!(losers, vec![2, 0]);
        assert!(arbitrate_ranking(&[]).is_none());
    }

    trait BaseCheck {
        fn base_id_check(&self) -> u16;
    }
    impl BaseCheck for CanFrame {
        fn base_id_check(&self) -> u16 {
            self.id.base_id()
        }
    }
}
