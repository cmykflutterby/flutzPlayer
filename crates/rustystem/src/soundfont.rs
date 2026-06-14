#![allow(dead_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

use crate::binary_reader::BinaryReader;
use crate::error::SoundFontError;
use crate::four_cc::FourCC;
use crate::generator_type::GeneratorType;
use crate::instrument::Instrument;
use crate::preset::Preset;
use crate::sample_header::SampleHeader;
use crate::soundfont_info::SoundFontInfo;
use crate::soundfont_parameters::SoundFontParameters;
use crate::soundfont_sampledata::{SoundFontSampleData, SoundFontSampleDataMetadata};
use crate::LoopMode;

/// Reperesents a SoundFont.
#[derive(Debug)]
#[non_exhaustive]
pub struct SoundFont {
    pub(crate) info: SoundFontInfo,
    pub(crate) bits_per_sample: i32,
    pub(crate) wave_data: Vec<i16>,
    pub(crate) sample_headers: Vec<SampleHeader>,
    pub(crate) presets: Vec<Preset>,
    pub(crate) instruments: Vec<Instrument>,
}

/// SoundFont metadata parsed without materializing the full sample payload.
#[derive(Debug)]
#[non_exhaustive]
pub struct SoundFontMetadata {
    pub(crate) info: SoundFontInfo,
    pub(crate) bits_per_sample: i32,
    pub(crate) sample_data_sample_count: usize,
    pub(crate) sample_headers: Vec<SampleHeader>,
    pub(crate) presets: Vec<Preset>,
    pub(crate) instruments: Vec<Instrument>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SoundFontMetadataClosure {
    pub preset_ids: Vec<usize>,
    pub instrument_ids: Vec<usize>,
    pub sample_ids: Vec<usize>,
}

impl SoundFontMetadataClosure {
    pub fn new(preset_ids: Vec<usize>, instrument_ids: Vec<usize>, sample_ids: Vec<usize>) -> Self {
        Self {
            preset_ids,
            instrument_ids,
            sample_ids,
        }
    }
}

impl SoundFont {
    /// Loads a SoundFont from the stream.
    ///
    /// # Arguments
    ///
    /// * `reader` - The data stream used to load the SoundFont.
    pub fn new<R: Read>(reader: &mut R) -> Result<Self, SoundFontError> {
        let chunk_id = BinaryReader::read_four_cc(reader)?;
        if chunk_id != b"RIFF" {
            return Err(SoundFontError::RiffChunkNotFound);
        }

        let _size = BinaryReader::read_i32(reader);

        let form_type = BinaryReader::read_four_cc(reader)?;
        if form_type != b"sfbk" {
            return Err(SoundFontError::InvalidRiffChunkType {
                expected: FourCC::from_bytes(*b"sfbk"),
                actual: form_type,
            });
        }

        let info = SoundFontInfo::new(reader)?;
        let sample_data = SoundFontSampleData::new(reader)?;
        let parameters = SoundFontParameters::new(reader)?;

        let sound_font = Self {
            info,
            bits_per_sample: sample_data.bits_per_sample,
            wave_data: sample_data.wave_data,
            sample_headers: parameters.sample_headers,
            presets: parameters.presets,
            instruments: parameters.instruments,
        };

        sound_font.sanity_check()?;

        Ok(sound_font)
    }

    /// Loads only the metadata portions of a SoundFont from the stream.
    pub fn metadata_only<R: Read>(reader: &mut R) -> Result<SoundFontMetadata, SoundFontError> {
        let chunk_id = BinaryReader::read_four_cc(reader)?;
        if chunk_id != b"RIFF" {
            return Err(SoundFontError::RiffChunkNotFound);
        }

        let _size = BinaryReader::read_i32(reader);

        let form_type = BinaryReader::read_four_cc(reader)?;
        if form_type != b"sfbk" {
            return Err(SoundFontError::InvalidRiffChunkType {
                expected: FourCC::from_bytes(*b"sfbk"),
                actual: form_type,
            });
        }

        let info = SoundFontInfo::new(reader)?;
        let sample_data = SoundFontSampleData::metadata_only(reader)?;
        let parameters = SoundFontParameters::new(reader)?;

        Ok(SoundFontMetadata::from_parts(info, sample_data, parameters))
    }

    pub fn compact_for_preset(&self, bank: i32, program: i32) -> Result<Self, SoundFontError> {
        let closure = self.closure_for_preset(bank, program);
        self.compact_from_closure(&closure)
    }

    pub fn compact_from_closure(
        &self,
        closure: &SoundFontMetadataClosure,
    ) -> Result<Self, SoundFontError> {
        self.compact_from_closure_with_wave_data(closure, None)
    }

    pub fn compact_from_closure_and_wave_data(
        &self,
        closure: &SoundFontMetadataClosure,
        wave_data: Vec<i16>,
    ) -> Result<Self, SoundFontError> {
        self.compact_from_closure_with_wave_data(closure, Some(wave_data))
    }

    fn compact_from_closure_with_wave_data(
        &self,
        closure: &SoundFontMetadataClosure,
        provided_wave_data: Option<Vec<i16>>,
    ) -> Result<Self, SoundFontError> {
        compact_from_parts(
            &self.info,
            self.bits_per_sample,
            &self.sample_headers,
            &self.presets,
            &self.instruments,
            closure,
            provided_wave_data,
            Some(&self.wave_data),
        )
    }

    fn sanity_check(&self) -> Result<(), SoundFontError> {
        // https://github.com/sinshu/rustysynth/issues/22
        // https://github.com/sinshu/rustysynth/issues/33
        // https://github.com/sinshu/rustysynth/pull/51
        for instrument in &self.instruments {
            for region in &instrument.regions {
                let start = region.get_sample_start();
                let end = region.get_sample_end();
                let start_loop = region.get_sample_start_loop();
                let end_loop = region.get_sample_end_loop();
                let loop_mode = region.get_sample_modes();

                if start < 0 || end as usize > self.wave_data.len() || end <= start {
                    return Err(SoundFontError::SanityCheckFailed);
                }

                if loop_mode != LoopMode::NoLoop
                    && (start_loop < 0
                        || end_loop as usize > self.wave_data.len()
                        || end_loop <= start_loop)
                {
                    return Err(SoundFontError::SanityCheckFailed);
                }
            }
        }

        Ok(())
    }

    /// Gets the information of the SoundFont.
    pub fn get_info(&self) -> &SoundFontInfo {
        &self.info
    }

    /// Gets the bits per sample of the sample data.
    pub fn get_bits_per_sample(&self) -> i32 {
        self.bits_per_sample
    }

    /// Gets the sample data.
    pub fn get_wave_data(&self) -> &[i16] {
        &self.wave_data[..]
    }

    /// Gets the samples of the SoundFont.
    pub fn get_sample_headers(&self) -> &[SampleHeader] {
        &self.sample_headers[..]
    }

    /// Gets the presets of the SoundFont.
    pub fn get_presets(&self) -> &[Preset] {
        &self.presets[..]
    }

    /// Gets the instruments of the SoundFont.
    pub fn get_instruments(&self) -> &[Instrument] {
        &self.instruments[..]
    }

    pub fn closure_for_preset(&self, bank: i32, program: i32) -> SoundFontMetadataClosure {
        let mut preset_ids = Vec::new();
        let mut instrument_ids = std::collections::BTreeSet::<usize>::new();
        let mut sample_ids = std::collections::BTreeSet::<usize>::new();

        for (preset_id, preset) in self.presets.iter().enumerate() {
            if preset.get_bank_number() != bank || preset.get_patch_number() != program {
                continue;
            }
            preset_ids.push(preset_id);
            for region in preset.get_regions() {
                let instrument_id = region.get_instrument_id();
                if instrument_ids.insert(instrument_id) {
                    if let Some(instrument) = self.instruments.get(instrument_id) {
                        for region in instrument.get_regions() {
                            sample_ids.insert(region.get_sample_id());
                        }
                    }
                }
            }
        }

        SoundFontMetadataClosure {
            preset_ids,
            instrument_ids: instrument_ids.into_iter().collect(),
            sample_ids: sample_ids.into_iter().collect(),
        }
    }
}

fn compact_from_parts(
    info: &SoundFontInfo,
    bits_per_sample: i32,
    sample_headers: &[SampleHeader],
    presets: &[Preset],
    instruments: &[Instrument],
    closure: &SoundFontMetadataClosure,
    provided_wave_data: Option<Vec<i16>>,
    source_wave_data: Option<&[i16]>,
) -> Result<SoundFont, SoundFontError> {
    let selected_samples = closure.sample_ids.iter().copied().collect::<BTreeSet<_>>();
    if closure.preset_ids.is_empty()
        || closure.instrument_ids.is_empty()
        || selected_samples.is_empty()
    {
        return Err(SoundFontError::SanityCheckFailed);
    }

    let mut compact_wave_data = provided_wave_data.unwrap_or_default();
    let copy_wave_data = compact_wave_data.is_empty();
    let mut expected_wave_data_len = 0usize;
    let mut sample_id_map = BTreeMap::<usize, usize>::new();
    let mut compact_sample_headers = Vec::<SampleHeader>::new();

    for old_sample_id in selected_samples {
        let Some(old_header) = sample_headers.get(old_sample_id) else {
            return Err(SoundFontError::SanityCheckFailed);
        };
        if old_header.start < 0 || old_header.end <= old_header.start {
            return Err(SoundFontError::SanityCheckFailed);
        }
        let new_start = expected_wave_data_len as i32;
        let old_start = old_header.start as usize;
        let old_end = old_header.end as usize;
        if let Some(source_wave_data) = source_wave_data {
            if old_end > source_wave_data.len() {
                return Err(SoundFontError::SanityCheckFailed);
            }
        }
        let sample_len = old_end - old_start;
        if copy_wave_data {
            let Some(source_wave_data) = source_wave_data else {
                return Err(SoundFontError::SanityCheckFailed);
            };
            compact_wave_data.extend_from_slice(&source_wave_data[old_start..old_end]);
        }
        expected_wave_data_len = expected_wave_data_len.saturating_add(sample_len);

        let mut new_header = old_header.clone();
        let shift = new_start - old_header.start;
        new_header.start = old_header.start + shift;
        new_header.end = old_header.end + shift;
        new_header.start_loop = old_header.start_loop + shift;
        new_header.end_loop = old_header.end_loop + shift;

        let new_sample_id = compact_sample_headers.len();
        sample_id_map.insert(old_sample_id, new_sample_id);
        compact_sample_headers.push(new_header);
    }

    if compact_wave_data.len() != expected_wave_data_len {
        return Err(SoundFontError::SanityCheckFailed);
    }

    let mut instrument_id_map = BTreeMap::<usize, usize>::new();
    let mut compact_instruments = Vec::<Instrument>::new();
    for old_instrument_id in &closure.instrument_ids {
        let Some(old_instrument) = instruments.get(*old_instrument_id) else {
            return Err(SoundFontError::SanityCheckFailed);
        };
        let mut instrument = old_instrument.clone();
        instrument.regions = instrument
            .regions
            .into_iter()
            .filter_map(|mut region| {
                let old_sample_id = region.get_sample_id();
                let new_sample_id = *sample_id_map.get(&old_sample_id)?;
                let new_sample = compact_sample_headers.get(new_sample_id)?;
                region.gs[GeneratorType::SAMPLE_ID as usize] = new_sample_id as i16;
                region.sample_start = new_sample.start;
                region.sample_end = new_sample.end;
                region.sample_start_loop = new_sample.start_loop;
                region.sample_end_loop = new_sample.end_loop;
                Some(region)
            })
            .collect();
        if instrument.regions.is_empty() {
            continue;
        }
        let new_instrument_id = compact_instruments.len();
        instrument_id_map.insert(*old_instrument_id, new_instrument_id);
        compact_instruments.push(instrument);
    }

    let mut compact_presets = Vec::<Preset>::new();
    for old_preset_id in &closure.preset_ids {
        let Some(old_preset) = presets.get(*old_preset_id) else {
            return Err(SoundFontError::SanityCheckFailed);
        };
        let mut preset = old_preset.clone();
        preset.regions = preset
            .regions
            .into_iter()
            .filter_map(|mut region| {
                let new_instrument_id = *instrument_id_map.get(&region.get_instrument_id())?;
                region.instrument = new_instrument_id;
                region.gs[GeneratorType::INSTRUMENT as usize] = new_instrument_id as i16;
                Some(region)
            })
            .collect();
        if preset.regions.is_empty() {
            continue;
        }
        compact_presets.push(preset);
    }

    let sound_font = SoundFont {
        info: info.clone(),
        bits_per_sample,
        wave_data: compact_wave_data,
        sample_headers: compact_sample_headers,
        presets: compact_presets,
        instruments: compact_instruments,
    };
    sound_font.sanity_check()?;
    Ok(sound_font)
}

impl SoundFontMetadata {
    fn from_parts(
        info: SoundFontInfo,
        sample_data: SoundFontSampleDataMetadata,
        parameters: SoundFontParameters,
    ) -> Self {
        Self {
            info,
            bits_per_sample: sample_data.bits_per_sample,
            sample_data_sample_count: sample_data.sample_count,
            sample_headers: parameters.sample_headers,
            presets: parameters.presets,
            instruments: parameters.instruments,
        }
    }

    pub fn get_info(&self) -> &SoundFontInfo {
        &self.info
    }

    pub fn get_bits_per_sample(&self) -> i32 {
        self.bits_per_sample
    }

    pub fn get_sample_data_sample_count(&self) -> usize {
        self.sample_data_sample_count
    }

    pub fn get_sample_headers(&self) -> &[SampleHeader] {
        &self.sample_headers[..]
    }

    pub fn get_presets(&self) -> &[Preset] {
        &self.presets[..]
    }

    pub fn get_instruments(&self) -> &[Instrument] {
        &self.instruments[..]
    }

    pub fn closure_for_preset(&self, bank: i32, program: i32) -> SoundFontMetadataClosure {
        closure_for_preset(&self.presets, &self.instruments, bank, program)
    }

    pub fn compact_from_closure_and_wave_data(
        &self,
        closure: &SoundFontMetadataClosure,
        wave_data: Vec<i16>,
    ) -> Result<SoundFont, SoundFontError> {
        compact_from_parts(
            &self.info,
            self.bits_per_sample,
            &self.sample_headers,
            &self.presets,
            &self.instruments,
            closure,
            Some(wave_data),
            None,
        )
    }

    pub fn compact_for_preset_and_wave_data(
        &self,
        bank: i32,
        program: i32,
        wave_data: Vec<i16>,
    ) -> Result<SoundFont, SoundFontError> {
        let closure = self.closure_for_preset(bank, program);
        self.compact_from_closure_and_wave_data(&closure, wave_data)
    }
}

fn closure_for_preset(
    presets: &[Preset],
    instruments: &[Instrument],
    bank: i32,
    program: i32,
) -> SoundFontMetadataClosure {
    let mut preset_ids = Vec::new();
    let mut instrument_ids = std::collections::BTreeSet::<usize>::new();
    let mut sample_ids = std::collections::BTreeSet::<usize>::new();

    for (preset_id, preset) in presets.iter().enumerate() {
        if preset.get_bank_number() != bank || preset.get_patch_number() != program {
            continue;
        }
        preset_ids.push(preset_id);
        for region in preset.get_regions() {
            let instrument_id = region.get_instrument_id();
            if instrument_ids.insert(instrument_id) {
                if let Some(instrument) = instruments.get(instrument_id) {
                    for region in instrument.get_regions() {
                        sample_ids.insert(region.get_sample_id());
                    }
                }
            }
        }
    }

    SoundFontMetadataClosure {
        preset_ids,
        instrument_ids: instrument_ids.into_iter().collect(),
        sample_ids: sample_ids.into_iter().collect(),
    }
}
