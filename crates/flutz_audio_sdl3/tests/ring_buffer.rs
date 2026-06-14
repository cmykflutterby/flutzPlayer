use flutz_audio_sdl3::{AudioRingBuffer, RingBufferConfig};

#[test]
fn ring_buffer_preserves_wrapped_samples() {
    let mut ring = AudioRingBuffer::new(RingBufferConfig { capacity_frames: 3 }, 2);
    assert_eq!(ring.capacity_frames(), 3);
    assert_eq!(ring.free_frames(), 3);

    assert_eq!(ring.write(&[1.0, 2.0, 3.0, 4.0]), 4);
    assert_eq!(ring.available_frames(), 2);

    let mut first = [0.0; 2];
    assert_eq!(ring.read(&mut first), 2);
    assert_eq!(first, [1.0, 2.0]);

    assert_eq!(ring.write(&[5.0, 6.0, 7.0, 8.0]), 4);
    assert_eq!(ring.available_frames(), 3);

    let mut rest = [0.0; 6];
    assert_eq!(ring.read(&mut rest), 6);
    assert_eq!(rest, [3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
    assert_eq!(ring.available_frames(), 0);
}

#[test]
fn ring_buffer_limits_writes_to_capacity() {
    let mut ring = AudioRingBuffer::new(RingBufferConfig { capacity_frames: 2 }, 2);
    assert_eq!(ring.write(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]), 4);
    assert_eq!(ring.free_frames(), 0);

    let mut output = [0.0; 6];
    assert_eq!(ring.read(&mut output), 4);
    assert_eq!(&output[..4], &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(&output[4..], &[0.0, 0.0]);
}
