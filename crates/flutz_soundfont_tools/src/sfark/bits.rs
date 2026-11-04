use super::*;

pub(super) struct BitReader<'a> {
    input: &'a [u8],
    word_offset: usize,
    bits: u32,
    remaining_bits: usize,
    previous_fix_bits: i16,
}

impl<'a> BitReader<'a> {
    pub(super) fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            word_offset: 0,
            bits: 0,
            remaining_bits: 0,
            previous_fix_bits: 8,
        }
    }

    pub(super) fn read_flag(&mut self) -> Result<bool> {
        Ok(self.read_bits(1)? != 0)
    }

    pub(super) fn read_u32_le(&mut self) -> Result<u32> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(super) fn read_bytes(&mut self, len: usize) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(len);
        for _ in 0..len {
            bytes.push(self.read_bits(8)? as u8);
        }
        Ok(bytes)
    }

    pub(super) fn read_bits(&mut self, count: usize) -> Result<u16> {
        if count == 0 {
            return Ok(0);
        }
        if count > 16 {
            return Err(SfArkError::CorruptData(format!(
                "cannot read {count} bits at once"
            )));
        }
        if self.remaining_bits < 16 {
            let next = self.next_word()?;
            self.bits = (self.bits << 16) | u32::from(next);
            self.remaining_bits += 16;
        }

        if self.remaining_bits < count {
            return Err(SfArkError::CorruptData("bitstream underrun".to_owned()));
        }

        self.remaining_bits -= count;
        let value = (self.bits >> self.remaining_bits) & low_mask(count);
        self.bits &= low_mask(self.remaining_bits);
        Ok(value as u16)
    }

    pub(super) fn input_diff(&mut self, previous: i16) -> Result<i16> {
        let magnitude = self.read_group_count()? as i32;
        let signed = if magnitude == 0 {
            0
        } else if self.read_flag()? {
            -magnitude
        } else {
            magnitude
        };
        Ok(previous.wrapping_add(signed as i16))
    }

    pub(super) fn uncrunch_window(
        &mut self,
        word_len: usize,
        window_words: usize,
    ) -> Result<Vec<i16>> {
        let mut output = vec![0; word_len];
        let mut start = 0;
        while start < word_len {
            let end = (start + window_words).min(word_len);
            self.uncrunch(&mut output[start..end])?;
            start = end;
        }
        Ok(output)
    }

    fn uncrunch(&mut self, output: &mut [i16]) -> Result<()> {
        let fix_bits = self.input_diff(self.previous_fix_bits)?;
        self.previous_fix_bits = fix_bits;

        match fix_bits {
            0..=13 => {
                let low_bit_count = fix_bits as usize + 1;
                for word in output {
                    let low_bits = self.read_bits(low_bit_count)? as i32;
                    let group = self.read_group_count()? as i32;
                    let sign_mask = -((low_bits & 1) as i32);
                    *word = (((group << fix_bits) | (low_bits >> 1)) ^ sign_mask) as i16;
                }
            }
            14 => {
                for word in output {
                    *word = self.read_bits(16)? as i16;
                }
            }
            -1 => {
                for word in output {
                    *word = if self.read_flag()? { -1 } else { 0 };
                }
            }
            -2 => output.fill(0),
            other => {
                return Err(SfArkError::CorruptData(format!(
                    "invalid fix-bits value {other}"
                )));
            }
        }

        Ok(())
    }

    fn read_group_count(&mut self) -> Result<u16> {
        let mut count = 0u16;
        while !self.read_flag()? {
            count = count
                .checked_add(1)
                .ok_or_else(|| SfArkError::CorruptData("group count overflow".to_owned()))?;
        }
        Ok(count)
    }

    fn next_word(&mut self) -> Result<u16> {
        let byte_offset = self.word_offset * 2;
        let bytes = self
            .input
            .get(byte_offset..byte_offset + 2)
            .ok_or_else(|| SfArkError::CorruptData("bitstream ended early".to_owned()))?;
        self.word_offset += 1;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }
}

pub(super) fn quick_abs2(value: i16) -> u32 {
    (value ^ (value >> 15)) as u16 as u32
}

pub(super) fn nsdiv(value: i32, shift: u32) -> i32 {
    value >> shift
}

pub(super) fn sdiv(value: i32, shift: u32) -> i32 {
    if value >= 0 {
        value >> shift
    } else {
        -((-value) >> shift)
    }
}

pub(super) fn nbits(value: usize) -> usize {
    if value == 0 {
        0
    } else {
        usize::BITS as usize - value.leading_zeros() as usize
    }
}

pub(super) fn low_mask(bits: usize) -> u32 {
    if bits == 0 {
        0
    } else if bits >= 32 {
        u32::MAX
    } else {
        (1u32 << bits) - 1
    }
}

pub(super) fn looks_like_sf2(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"sfbk"
}
