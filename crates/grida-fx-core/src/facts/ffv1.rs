//! The transparency flag of an FFV1 stream (RFC 9043), for `has_alpha` (spec/facts.md §5.4).
//!
//! A version 2 or 3 stream says it in its configuration record (the codec private data); a
//! version 0 or 1 stream in the header of its key frames. Both are read with FFV1's range coder,
//! its default state table and one shared array of 32 states, field by field up to the flag:
//! `version`, (`micro_version` from version 3), `coder_type` (and, for coder type 2, a custom
//! state table of 255 signed values), `colorspace_type`, (`bits_per_raw_sample` from version 1),
//! `chroma_planes`, `log2_h_chroma_subsample`, `log2_v_chroma_subsample`, then the flag
//! (`extra_plane`, which ffmpeg calls transparency). A version 3 record must also pass its CRC
//! (the CRC-32 of RFC 9043 §4.2.2 over the whole record, its last four bytes included, is 0).
//! Anything that does not decode gives `None`.

/// More bytes than this read past the end make a header invalid (as in ffmpeg's decoder).
const MAX_OVERREAD: u32 = 2;

/// The transparency flag from a configuration record (version 2 or 3), or, without one, from
/// the first frame's key-frame header (version 0 or 1).
pub(super) fn has_alpha(record: &[u8], first_frame: Option<&[u8]>) -> Option<bool> {
    if !record.is_empty() {
        let mut coder = RangeDecoder::new(record);
        let mut states = [128u8; 32];
        let version = coder.symbol(&mut states, false)?;
        if !matches!(version, 2 | 3) || (version == 3 && (record.len() < 4 || crc32(record) != 0)) {
            return None;
        }
        return header(&mut coder, &mut states, version);
    }
    let frame = first_frame.filter(|frame| !frame.is_empty())?;
    let mut coder = RangeDecoder::new(frame);
    let mut key_state = [128u8; 1];
    if !coder.bit(&mut key_state, 0) {
        return None;
    }
    let mut states = [128u8; 32];
    let version = coder.symbol(&mut states, false)?;
    if version > 1 {
        return None;
    }
    header(&mut coder, &mut states, version)
}

/// Reads from the field after `version` up to the transparency flag.
fn header(coder: &mut RangeDecoder, states: &mut [u8; 32], version: i64) -> Option<bool> {
    if version > 2 {
        // A version 3 record ends with its CRC, which is not range coded.
        coder.end = coder.end.saturating_sub(4).max(coder.at);
        coder.symbol(states, false)?; // micro_version
    }
    let coder_type = coder.symbol(states, false)?;
    if coder_type == 2 {
        for _ in 1..256 {
            coder.symbol(states, true)?;
        }
    }
    coder.symbol(states, false)?; // colorspace_type
    if version > 0 {
        coder.symbol(states, false)?; // bits_per_raw_sample
    }
    coder.bit(states, 0); // chroma_planes
    coder.symbol(states, false)?; // log2_h_chroma_subsample
    coder.symbol(states, false)?; // log2_v_chroma_subsample
    let transparency = coder.bit(states, 0);
    (coder.overread <= MAX_OVERREAD).then_some(transparency)
}

/// The CRC-32 FFV1 protects its records with: polynomial 0x04C11DB7, most significant bit first,
/// starting from 0, with no final complement.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0;
    for &byte in bytes {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// FFV1's binary range decoder (RFC 9043 §3.8.1).
struct RangeDecoder<'a> {
    bytes: &'a [u8],
    at: usize,
    end: usize,
    low: u32,
    range: u32,
    overread: u32,
    one_state: [u8; 256],
    zero_state: [u8; 256],
}

impl<'a> RangeDecoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        let (one_state, zero_state) = default_states();
        let mut coder = RangeDecoder {
            bytes,
            at: 2,
            end: bytes.len(),
            low: 0,
            range: 0xFF00,
            overread: 0,
            one_state,
            zero_state,
        };
        match bytes {
            [high, low, ..] => coder.low = u32::from(*high) << 8 | u32::from(*low),
            _ => coder.overread = 2,
        }
        if coder.low >= 0xFF00 {
            coder.low = 0xFF00;
            coder.end = coder.at;
        }
        coder
    }

    fn bit(&mut self, states: &mut [u8], i: usize) -> bool {
        let state = states[i];
        let split = (self.range * u32::from(state)) >> 8;
        self.range -= split;
        let bit = if self.low < self.range {
            states[i] = self.zero_state[usize::from(state)];
            false
        } else {
            self.low -= self.range;
            states[i] = self.one_state[usize::from(state)];
            self.range = split;
            true
        };
        if self.range < 0x100 {
            self.range <<= 8;
            self.low <<= 8;
            if self.at < self.end {
                // A broken stream can push `low` past `range`; it then only has to stay bounded.
                self.low = self.low.wrapping_add(u32::from(self.bytes[self.at]));
                self.at += 1;
            } else {
                self.overread += 1;
            }
        }
        bit
    }

    /// A symbol: zero, or an exponent in unary and a mantissa, and a sign when `signed`.
    fn symbol(&mut self, states: &mut [u8; 32], signed: bool) -> Option<i64> {
        if self.bit(states, 0) {
            return Some(0);
        }
        let mut exponent = 0usize;
        while self.bit(states, 1 + exponent.min(9)) {
            exponent += 1;
            if exponent > 31 {
                return None;
            }
        }
        let mut value: i64 = 1;
        for i in (0..exponent).rev() {
            value = 2 * value + i64::from(self.bit(states, 22 + i.min(9)));
        }
        if signed && self.bit(states, 11 + exponent.min(10)) {
            value = -value;
        }
        Some(value)
    }
}

/// The default state transition tables: `one_state` as ffmpeg's `ff_build_rac_states` builds it
/// for a factor of 0.05 × 2^32 and a largest state of 248 (RFC 9043's default table), and
/// `zero_state[i] = 256 - one_state[256 - i]`.
fn default_states() -> ([u8; 256], [u8; 256]) {
    const ONE: i64 = 1 << 32;
    // 0.05 × 2^32, truncated to an integer.
    const FACTOR: i64 = 214_748_364;
    const MAX_P: i64 = 256 - 8;
    let mut one_state = [0u8; 256];
    let mut zero_state = [0u8; 256];
    let mut last_p8: i64 = 0;
    let mut p: i64 = ONE / 2;
    for _ in 0..128 {
        let mut p8 = (256 * p + ONE / 2) >> 32;
        if p8 <= last_p8 {
            p8 = last_p8 + 1;
        }
        if last_p8 != 0 && last_p8 < 256 && p8 <= MAX_P {
            one_state[last_p8 as usize] = p8 as u8;
        }
        p += ((ONE - p) * FACTOR + ONE / 2) >> 32;
        last_p8 = p8;
    }
    for i in (256 - MAX_P)..=MAX_P {
        if one_state[i as usize] != 0 {
            continue;
        }
        let mut p = (i * ONE + 128) >> 8;
        p += ((ONE - p) * FACTOR + ONE / 2) >> 32;
        let mut p8 = (256 * p + ONE / 2) >> 32;
        if p8 <= i {
            p8 = i + 1;
        }
        if p8 > MAX_P {
            p8 = MAX_P;
        }
        one_state[i as usize] = p8 as u8;
    }
    for i in 1..255 {
        zero_state[i] = (256 - u16::from(one_state[256 - i])) as u8;
    }
    (one_state, zero_state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn the_default_table_starts_as_rfc_9043_prints_it() {
        let (one, zero) = default_states();
        assert_eq!(
            one[..24],
            [
                0, 0, 0, 0, 0, 0, 0, 0, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34,
                35
            ]
        );
        assert_eq!(one[248], 248);
        assert_eq!(zero[1], (256 - u16::from(one[255])) as u8);
    }

    // Configuration records ffmpeg wrote for 64x48 streams (version 3, micro version 4).
    const YUVA420P: &str =
        "562b8503aa420f345b7b2bc43c417c7fecc729b9731cd404e78fbee30bcbe8d4734d73dbfccce422c633";
    const YUV420P: &str =
        "562b84d19c052f413c6026e95c376f5d1b76979d3ac9c420431e8b9f5520512f4ef8a1683b9b17137c03";
    const RGBA: &str =
        "5619e8908dfcfcbc1b4933aeeebaaabe852a4efbf847ccb07bbb3a69dcb4741f5fbf8a0c7adf1f90dc43";
    // Coder type 2: the record carries a custom state table before the fields that follow.
    const CUSTOM_TABLE: &str = "56061e2ffdb846d7709730018601eee11ce6bd454db5382774d82e107d75c5aae33d602cd88683fe5b47b10fc55ef17bff8178b57dc1212221be19c651d9310f08b67f7ab2c9dcca599ec7ddfde33307c787ddee7487e265c4fbfcd4c01ccd25e4684ad70edf1a5235be5667fe306fbe96c56002384c2e17c2334d3d63a079d1300a50489dbab46ddf204ce365a7a2219de799321bddc814436d2bc43c417c7fecca9ded4c02a3e9d3ac9d15fe1669ae68b6b5bbb1cdbe1699a9cdcb39a878";

    #[test]
    fn configuration_records() {
        assert_eq!(has_alpha(&hex(YUVA420P), None), Some(true));
        assert_eq!(has_alpha(&hex(YUV420P), None), Some(false));
        assert_eq!(has_alpha(&hex(RGBA), None), Some(true));
        assert_eq!(has_alpha(&hex(CUSTOM_TABLE), None), Some(true));
    }

    #[test]
    fn key_frame_headers() {
        // The first bytes of key frames ffmpeg wrote: version 1 yuva420p and yuv420p, and
        // version 0 yuva420p.
        assert_eq!(
            has_alpha(&[], Some(&hex("9aeca1a4067dfbd89ac4df02026053c9"))),
            Some(true)
        );
        assert_eq!(
            has_alpha(&[], Some(&hex("9aec87309a496df656689957fd954a81"))),
            Some(false)
        );
        assert_eq!(
            has_alpha(&[], Some(&hex("f50803553d9a57361bf6bcf4914448e6"))),
            Some(true)
        );
        // A version 3 record is not a frame header, and an empty frame has none.
        assert_eq!(has_alpha(&[], Some(&hex(YUV420P))), None);
        assert_eq!(has_alpha(&[], Some(&[])), None);
        assert_eq!(has_alpha(&[], None), None);
    }

    #[test]
    fn what_does_not_decode_gives_none() {
        // Records whose version is not 2 or 3.
        assert_eq!(has_alpha(&[0xFF, 0xFF], None), None);
        assert_eq!(has_alpha(&[0x00], None), None);
        assert_eq!(has_alpha(&[0x00; 64], None), None);
        // A version 3 record cut short, or with a byte changed, fails its CRC.
        let record = hex(YUVA420P);
        assert_eq!(crc32(&record), 0);
        for cut in [3, 6, 20, record.len() - 1] {
            assert_eq!(has_alpha(&record[..cut], None), None, "{cut} bytes");
        }
        let mut changed = record.clone();
        changed[10] ^= 1;
        assert_eq!(has_alpha(&changed, None), None);
    }
}
