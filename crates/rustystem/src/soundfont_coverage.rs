use std::collections::BTreeSet;

use crate::SoundFont;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BankProgram {
    pub bank: u16,
    pub program: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MelodicCoverage {
    pub bank_programs: BTreeSet<BankProgram>,
}

impl MelodicCoverage {
    pub fn contains(&self, bank: u16, program: u8) -> bool {
        self.bank_programs.contains(&BankProgram { bank, program })
    }

    pub fn len(&self) -> usize {
        self.bank_programs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bank_programs.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PercussionKeyRange {
    pub low_key: u8,
    pub high_key: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PercussionCoverage {
    pub has_bank_128: bool,
    pub key_ranges: BTreeSet<PercussionKeyRange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SoundFontCoverageMetadata {
    pub preset_names: Vec<String>,
    pub sample_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SoundFontCoverage {
    pub melodic: MelodicCoverage,
    pub percussion: PercussionCoverage,
    pub metadata: SoundFontCoverageMetadata,
}

impl SoundFontCoverage {
    pub fn provides_melodic(&self, bank: u16, program: u8) -> bool {
        self.melodic.contains(bank, program)
    }

    pub fn provides_percussion(&self) -> bool {
        self.percussion.has_bank_128
    }
}

pub fn extract_coverage_from_sf2(soundfont: &SoundFont) -> SoundFontCoverage {
    let mut coverage = SoundFontCoverage::default();

    for preset in soundfont.get_presets() {
        let bank = preset.get_bank_number();
        let program = preset.get_patch_number();
        coverage
            .metadata
            .preset_names
            .push(preset.get_name().to_owned());

        if !(0..=u16::MAX as i32).contains(&bank) || !(0..=u8::MAX as i32).contains(&program) {
            continue;
        }

        if bank == 128 {
            coverage.percussion.has_bank_128 = true;
        } else {
            coverage.melodic.bank_programs.insert(BankProgram {
                bank: bank as u16,
                program: program as u8,
            });
        }
    }

    coverage.metadata.sample_count = soundfont.get_sample_headers().len();
    coverage
}
