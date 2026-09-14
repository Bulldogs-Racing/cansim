//! Bit stuffing / destuffing (ISO 11898-1 frame coding rule).
//!
//! After five consecutive bits of the same polarity in the stuffing region
//! (`SOF` through the CRC sequence), the transmitter inserts one bit of the
//! opposite polarity; receivers remove it. Six consecutive equal bits in the
//! region are a **stuff error**. Fixed-form fields after the CRC sequence
//! (CRC delimiter, ACK field, EOF, IFS) are never stuffed.
//!
//! References: ISO 11898-1; Bosch CAN Specification 2.0.

use super::bit::Bit;
use thiserror::Error;

/// Maximum run of equal bits before a stuff bit is required.
pub const STUFF_RUN: usize = 5;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StuffError {
    #[error("stuff error: {run} consecutive {polarity} bits at stuffed-stream offset {offset}")]
    TooManyConsecutive {
        run: usize,
        polarity: &'static str,
        offset: usize,
    },
    #[error("truncated stream: run of five ends at offset {offset} with no following stuff bit")]
    MissingStuffBit { offset: usize },
}

/// Insert stuff bits into a destuffed protected stream (`SOF`..CRC sequence).
pub fn stuff(protected: &[Bit]) -> Vec<Bit> {
    let mut out = Vec::with_capacity(protected.len() + protected.len() / 4);
    let mut run: usize = 0;
    let mut last: Option<Bit> = None;
    for bit in protected {
        if Some(*bit) == last {
            run += 1;
        } else {
            run = 1;
            last = Some(*bit);
        }
        out.push(*bit);
        if run == STUFF_RUN {
            // Opposite-polarity stuff bit breaks the run; it starts a new
            // run of length 1 for subsequent counting.
            let stuff_bit = bit.flipped();
            out.push(stuff_bit);
            last = Some(stuff_bit);
            run = 1;
        }
    }
    out
}

/// Remove stuff bits, validating the 5-bit rule.
///
/// Returns the destuffed stream, [`StuffError::TooManyConsecutive`] on six
/// consecutive equal bits, or [`StuffError::MissingStuffBit`] when a run of
/// five reaches the end of input with no following stuff bit (a complete
/// transmitted region always carries its stuff bits).
pub fn destuff(stuffed: &[Bit]) -> Result<Vec<Bit>, StuffError> {
    let mut out = Vec::with_capacity(stuffed.len());
    let mut run: usize = 0;
    let mut last: Option<Bit> = None;
    let mut i = 0;
    while i < stuffed.len() {
        let bit = stuffed[i];
        if Some(bit) == last {
            run += 1;
        } else {
            run = 1;
            last = Some(bit);
        }
        if run == STUFF_RUN + 1 {
            return Err(StuffError::TooManyConsecutive {
                run,
                polarity: if bit.is_dominant() {
                    "dominant"
                } else {
                    "recessive"
                },
                offset: i,
            });
        }
        out.push(bit);
        if run == STUFF_RUN {
            // Next bit must be the opposite-polarity stuff bit: consume and
            // skip it (same polarity means 6-in-a-row; end of input means
            // the region is truncated — both are errors here).
            match stuffed.get(i + 1) {
                None => {
                    return Err(StuffError::MissingStuffBit { offset: i + 1 });
                }
                Some(next) if *next == bit => {
                    return Err(StuffError::TooManyConsecutive {
                        run: STUFF_RUN + 1,
                        polarity: if bit.is_dominant() {
                            "dominant"
                        } else {
                            "recessive"
                        },
                        offset: i + 1,
                    });
                }
                Some(next) => {
                    i += 1; // skip stuff bit
                    last = Some(*next);
                    run = 1;
                }
            }
        }
        i += 1;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Bit::*;

    fn bits(s: &str) -> Vec<Bit> {
        s.chars()
            .map(|c| match c {
                '0' => Dominant,
                '1' => Recessive,
                _ => panic!("bad bit char {c:?}"),
            })
            .collect()
    }

    #[test]
    fn no_stuffing_without_runs_of_five() {
        let plain = bits("0101010100110011");
        assert_eq!(stuff(&plain), plain);
        assert_eq!(destuff(&plain).unwrap(), plain);
    }

    #[test]
    fn stuff_bit_inserted_after_five_equal() {
        // Five dominant bits gain one recessive stuff bit.
        assert_eq!(stuff(&bits("00000")), bits("000001"));
        // Five recessive bits gain one dominant stuff bit.
        assert_eq!(stuff(&bits("11111")), bits("111110"));
    }

    #[test]
    fn stuff_bit_resets_run_counting() {
        // 10 dominant bits → two stuff bits, not one.
        assert_eq!(stuff(&bits("0000000000")), bits("000001000001"));
        // Run followed by opposite data bit: five 0s earn a stuff 1, then
        // the three data 1s follow (stuff bit + data bits are distinct).
        assert_eq!(stuff(&bits("00000111")), bits("000001111"));
    }

    #[test]
    fn round_trips_including_worst_case_payloads() {
        for pattern in ["00000000", "11111111", "10101010", "00001111", "11111000"] {
            let raw = bits(&pattern.repeat(4));
            let stuffed = stuff(&raw);
            assert_eq!(destuff(&stuffed).unwrap(), raw, "pattern {pattern}");
        }
        // Alternating runs of exactly five stress back-to-back stuff bits.
        let raw = bits("00000111110000011111");
        assert_eq!(destuff(&stuff(&raw)).unwrap(), raw);
    }

    #[test]
    fn six_consecutive_bits_are_stuff_errors() {
        assert!(matches!(
            destuff(&bits("000000")),
            Err(StuffError::TooManyConsecutive { run: 6, .. })
        ));
        assert!(matches!(
            destuff(&bits("101111111")),
            Err(StuffError::TooManyConsecutive { .. })
        ));
    }

    #[test]
    fn corrupted_stuff_bit_is_detected() {
        // Valid stuffed "000001" with the stuff bit flipped to 0.
        assert!(destuff(&bits("000000")).is_err());
    }

    #[test]
    fn truncated_run_of_five_is_an_error() {
        assert!(matches!(
            destuff(&bits("10100000")),
            Err(StuffError::MissingStuffBit { .. })
        ));
    }
    #[test]
    fn stuff_bit_count_matches_formula_spot_check() {
        // 8 dominant payload bits → one stuff bit after bit 5, then the
        // remaining three continue the post-stuff run (0,0,0 + stuff 1 = run of 1).
        let stuffed = stuff(&bits("00000000"));
        assert_eq!(stuffed.len(), 9);
    }
}
