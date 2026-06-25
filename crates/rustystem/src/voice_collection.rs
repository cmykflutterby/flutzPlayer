#![allow(dead_code)]

use crate::channel::Channel;
use crate::envelope_stage::EnvelopeStage;
use crate::instrument_region::InstrumentRegion;
use crate::synthesizer_settings::SynthesizerSettings;
use crate::voice::Voice;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct VolumeEnvelopeAggregateDebug {
    pub(crate) delay_voices: usize,
    pub(crate) attack_voices: usize,
    pub(crate) hold_voices: usize,
    pub(crate) decay_voices: usize,
    pub(crate) release_voices: usize,
    pub(crate) value_sum: f32,
}

impl VolumeEnvelopeAggregateDebug {
    pub(crate) fn average_value(&self, active_voices: usize) -> f32 {
        if active_voices > 0 {
            self.value_sum / active_voices as f32
        } else {
            0.0
        }
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct VoiceCollection {
    voices: Vec<Voice>,
    pub(crate) active_voice_count: usize,
    pub(crate) max_active_voice_count: usize,
    pub(crate) total_voice_requests: u64,
    pub(crate) exclusive_class_reuses: u64,
    pub(crate) free_voice_allocations: u64,
    pub(crate) contention_steals: u64,
}

impl VoiceCollection {
    pub(crate) fn new(settings: &SynthesizerSettings) -> Self {
        let mut voices: Vec<Voice> = Vec::new();
        for _i in 0..settings.maximum_polyphony {
            voices.push(Voice::new(settings));
        }

        Self {
            voices,
            active_voice_count: 0,
            max_active_voice_count: 0,
            total_voice_requests: 0,
            exclusive_class_reuses: 0,
            free_voice_allocations: 0,
            contention_steals: 0,
        }
    }

    pub(crate) fn request_new(
        &mut self,
        region: &InstrumentRegion,
        channel: i32,
    ) -> Option<&mut Voice> {
        self.total_voice_requests = self.total_voice_requests.saturating_add(1);

        // If an exclusive class is assigned to the region, find a voice with the same class.
        // If found, reuse it to avoid playing multiple voices with the same class at a time.
        let exclusive_class = region.get_exclusive_class();
        if exclusive_class != 0 {
            for i in 0..self.active_voice_count {
                let voice = &self.voices[i];
                if voice.exclusive_class() == exclusive_class && voice.channel() == channel {
                    self.exclusive_class_reuses = self.exclusive_class_reuses.saturating_add(1);
                    return Some(&mut self.voices[i]);
                }
            }
        }

        // If the number of active voices is less than the limit, use a free one.
        if (self.active_voice_count) < self.voices.len() {
            let i = self.active_voice_count;
            self.active_voice_count += 1;
            self.max_active_voice_count = self.max_active_voice_count.max(self.active_voice_count);
            self.free_voice_allocations = self.free_voice_allocations.saturating_add(1);
            return Some(&mut self.voices[i]);
        }

        // Too many active voices...
        // Find one which has the lowest priority.
        let mut candidate: usize = 0;
        let mut lowest_priority = f32::MAX;
        for i in 0..self.active_voice_count {
            let voice = &self.voices[i];
            let priority = voice.priority();
            if priority < lowest_priority {
                lowest_priority = priority;
                candidate = i;
            } else if priority == lowest_priority {
                // Same priority...
                // The older one should be more suitable for reuse.
                if voice.voice_length() > self.voices[candidate].voice_length() {
                    candidate = i;
                }
            }
        }
        self.contention_steals = self.contention_steals.saturating_add(1);
        Some(&mut self.voices[candidate])
    }

    pub(crate) fn process(&mut self, data: &[i16], channels: &[Channel]) {
        let mut i: usize = 0;

        loop {
            if i == self.active_voice_count {
                return;
            }

            if self.voices[i].process(data, channels) {
                i += 1;
            } else {
                self.active_voice_count -= 1;
                self.voices.swap(i, self.active_voice_count);
            }
        }
    }

    pub(crate) fn get_active_voices(&mut self) -> &mut [Voice] {
        &mut self.voices[0..self.active_voice_count]
    }

    pub(crate) fn clear(&mut self) {
        self.active_voice_count = 0;
        self.max_active_voice_count = 0;
        self.total_voice_requests = 0;
        self.exclusive_class_reuses = 0;
        self.free_voice_allocations = 0;
        self.contention_steals = 0;
    }

    pub(crate) fn voice_count(&self) -> usize {
        self.voices.len()
    }

    pub(crate) fn voice_buffer_bytes(&self) -> usize {
        self.voices.iter().map(Voice::memory_debug_bytes).sum()
    }

    pub(crate) fn volume_envelope_debug(&self) -> VolumeEnvelopeAggregateDebug {
        let mut debug = VolumeEnvelopeAggregateDebug::default();
        for voice in &self.voices[0..self.active_voice_count] {
            debug.value_sum += voice.volume_envelope_value();
            match voice.volume_envelope_stage() {
                EnvelopeStage::Delay => debug.delay_voices += 1,
                EnvelopeStage::Attack => debug.attack_voices += 1,
                EnvelopeStage::Hold => debug.hold_voices += 1,
                EnvelopeStage::Decay => debug.decay_voices += 1,
                EnvelopeStage::Release => debug.release_voices += 1,
            }
        }
        debug
    }
}
