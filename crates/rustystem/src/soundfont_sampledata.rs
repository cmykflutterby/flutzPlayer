#![allow(dead_code)]

use std::io::Read;

use crate::binary_reader::BinaryReader;
use crate::error::SoundFontError;
use crate::four_cc::FourCC;
use crate::read_counter::ReadCounter;

#[non_exhaustive]
pub(crate) struct SoundFontSampleData {
    pub bits_per_sample: i32,
    pub wave_data: Vec<i16>,
}

#[non_exhaustive]
pub(crate) struct SoundFontSampleDataMetadata {
    pub bits_per_sample: i32,
    pub sample_count: usize,
}

impl SoundFontSampleData {
    pub(crate) fn new<R: Read>(reader: &mut R) -> Result<Self, SoundFontError> {
        let chunk_id = BinaryReader::read_four_cc(reader)?;
        if chunk_id != b"LIST" {
            return Err(SoundFontError::ListChunkNotFound);
        }

        let end = BinaryReader::read_u32(reader)? as usize;
        let reader = &mut ReadCounter::new(reader);

        let list_type = BinaryReader::read_four_cc(reader)?;
        if list_type != b"sdta" {
            return Err(SoundFontError::InvalidListChunkType {
                expected: FourCC::from_bytes(*b"sdta"),
                actual: list_type,
            });
        }

        let mut wave_data: Option<Vec<i16>> = None;

        while reader.bytes_read() < end {
            let id = BinaryReader::read_four_cc(reader)?;
            let size = BinaryReader::read_u32(reader)? as usize;

            match id.as_bytes() {
                b"smpl" => wave_data = Some(BinaryReader::read_wave_data(reader, size)?),
                b"sm24" => BinaryReader::discard_data(reader, size)?,
                _ => return Err(SoundFontError::ListContainsUnknownId(id)),
            }
        }

        let Some(wave_data) = wave_data else {
            return Err(SoundFontError::SampleDataNotFound);
        };

        if wave_data.len() < 2 {
            return Err(SoundFontError::SampleDataNotFound);
        }

        // SoundFont3 compressed sample format
        let mut four_cc = [0u8; 4];
        four_cc[..2].copy_from_slice(&(wave_data[0] as u16).to_le_bytes());
        four_cc[2..].copy_from_slice(&(wave_data[1] as u16).to_le_bytes());
        if &four_cc == b"OggS" {
            return Err(SoundFontError::UnsupportedSampleFormat);
        }

        Ok(Self {
            bits_per_sample: 16,
            wave_data,
        })
    }

    pub(crate) fn metadata_only<R: Read>(
        reader: &mut R,
    ) -> Result<SoundFontSampleDataMetadata, SoundFontError> {
        let chunk_id = BinaryReader::read_four_cc(reader)?;
        if chunk_id != b"LIST" {
            return Err(SoundFontError::ListChunkNotFound);
        }

        let end = BinaryReader::read_u32(reader)? as usize;
        let reader = &mut ReadCounter::new(reader);

        let list_type = BinaryReader::read_four_cc(reader)?;
        if list_type != b"sdta" {
            return Err(SoundFontError::InvalidListChunkType {
                expected: FourCC::from_bytes(*b"sdta"),
                actual: list_type,
            });
        }

        let mut sample_count = None::<usize>;

        while reader.bytes_read() < end {
            let id = BinaryReader::read_four_cc(reader)?;
            let size = BinaryReader::read_u32(reader)? as usize;

            match id.as_bytes() {
                b"smpl" => {
                    if size < 4 || size % 2 != 0 {
                        return Err(SoundFontError::SampleDataNotFound);
                    }
                    let mut four_cc = [0u8; 4];
                    reader.read_exact(&mut four_cc)?;
                    if &four_cc == b"OggS" {
                        return Err(SoundFontError::UnsupportedSampleFormat);
                    }
                    BinaryReader::discard_data(reader, size - four_cc.len())?;
                    sample_count = Some(size / 2);
                }
                b"sm24" => BinaryReader::discard_data(reader, size)?,
                _ => return Err(SoundFontError::ListContainsUnknownId(id)),
            }
        }

        let Some(sample_count) = sample_count else {
            return Err(SoundFontError::SampleDataNotFound);
        };

        if sample_count < 2 {
            return Err(SoundFontError::SampleDataNotFound);
        }

        Ok(SoundFontSampleDataMetadata {
            bits_per_sample: 16,
            sample_count,
        })
    }
}
