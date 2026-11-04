use super::*;

#[derive(Debug)]
pub(super) struct Effects {
    pub(super) reverb: Reverb,
    pub(super) reverb_input: Vec<f32>,
    pub(super) reverb_output_left: Vec<f32>,
    pub(super) reverb_output_right: Vec<f32>,

    pub(super) chorus: Chorus,
    pub(super) chorus_input_left: Vec<f32>,
    pub(super) chorus_input_right: Vec<f32>,
    pub(super) chorus_output_left: Vec<f32>,
    pub(super) chorus_output_right: Vec<f32>,
}

impl Effects {
    pub(super) fn new(settings: &SynthesizerSettings) -> Effects {
        Self {
            reverb: Reverb::new(settings.sample_rate),
            reverb_input: vec![0_f32; settings.block_size],
            reverb_output_left: vec![0_f32; settings.block_size],
            reverb_output_right: vec![0_f32; settings.block_size],
            chorus: Chorus::new(settings.sample_rate, 0.002, 0.0019, 0.4),
            chorus_input_left: vec![0_f32; settings.block_size],
            chorus_input_right: vec![0_f32; settings.block_size],
            chorus_output_left: vec![0_f32; settings.block_size],
            chorus_output_right: vec![0_f32; settings.block_size],
        }
    }

    pub(super) fn memory_debug_bytes(&self) -> usize {
        f32_vec_bytes(&self.reverb_input)
            + f32_vec_bytes(&self.reverb_output_left)
            + f32_vec_bytes(&self.reverb_output_right)
            + f32_vec_bytes(&self.chorus_input_left)
            + f32_vec_bytes(&self.chorus_input_right)
            + f32_vec_bytes(&self.chorus_output_left)
            + f32_vec_bytes(&self.chorus_output_right)
    }
}
