#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct RingBufferConfig {
    pub capacity_frames: usize,
}

impl Default for RingBufferConfig {
    fn default() -> Self {
        Self {
            capacity_frames: 48_000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AudioRingBuffer {
    samples: Vec<f32>,
    channels: usize,
    read_index: usize,
    write_index: usize,
    len_samples: usize,
}

impl AudioRingBuffer {
    pub fn new(config: RingBufferConfig, channels: usize) -> Self {
        let channels = channels.max(1);
        let capacity_samples = config.capacity_frames.max(1) * channels;
        Self {
            samples: vec![0.0; capacity_samples],
            channels,
            read_index: 0,
            write_index: 0,
            len_samples: 0,
        }
    }

    pub fn capacity_frames(&self) -> usize {
        self.samples.len() / self.channels
    }

    pub fn retained_bytes(&self) -> usize {
        self.samples.capacity() * std::mem::size_of::<f32>()
    }

    pub fn available_frames(&self) -> usize {
        self.len_samples / self.channels
    }

    pub fn free_frames(&self) -> usize {
        (self.samples.len() - self.len_samples) / self.channels
    }

    pub fn clear(&mut self) {
        self.read_index = 0;
        self.write_index = 0;
        self.len_samples = 0;
    }

    pub fn write(&mut self, input: &[f32]) -> usize {
        let writable_samples = input.len().min(self.samples.len() - self.len_samples);
        if writable_samples == 0 {
            return 0;
        }

        let first = writable_samples.min(self.samples.len() - self.write_index);
        self.samples[self.write_index..self.write_index + first].copy_from_slice(&input[..first]);
        let second = writable_samples - first;
        if second > 0 {
            self.samples[..second].copy_from_slice(&input[first..first + second]);
        }

        self.write_index = (self.write_index + writable_samples) % self.samples.len();
        self.len_samples += writable_samples;
        writable_samples
    }

    pub fn read(&mut self, output: &mut [f32]) -> usize {
        let readable_samples = output.len().min(self.len_samples);
        if readable_samples == 0 {
            return 0;
        }

        let first = readable_samples.min(self.samples.len() - self.read_index);
        output[..first].copy_from_slice(&self.samples[self.read_index..self.read_index + first]);
        let second = readable_samples - first;
        if second > 0 {
            output[first..first + second].copy_from_slice(&self.samples[..second]);
        }

        self.read_index = (self.read_index + readable_samples) % self.samples.len();
        self.len_samples -= readable_samples;
        readable_samples
    }
}
