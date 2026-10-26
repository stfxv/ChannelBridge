//! HAL buffer validation. Construct borrows only for the current callback.
use crate::devices::Error;
use channelbridge_hal_abi::{AudioBuffer, AudioBufferList, StreamFormat, fourcc};

pub struct CaptureLayout {
    channels: Box<[usize]>,
    total: usize,
    rate: f64,
}
impl CaptureLayout {
    /// Control-thread preparation. Native float32 client formats only; physical
    /// integer bit depth remains separate metadata and is converted by HAL.
    pub fn try_new(formats: &[StreamFormat]) -> Result<Self, Error> {
        let mut channels = Vec::new();
        let mut total = 0;
        let mut rate = None;
        for format in formats {
            let count = format.channels_per_frame as usize;
            if count == 0
                || count > 512
                || format.format_id != fourcc(b"lpcm")
                || format.bits_per_channel != 32
                || format.format_flags & 1 == 0
                || format.format_flags & 2 != 0
                || format.frames_per_packet != 1
                || !format.sample_rate.is_finite()
                || format.sample_rate <= 0.0
                || rate.is_some_and(|value| value != format.sample_rate)
            {
                return Err(Error::MalformedProperty);
            }
            rate = Some(format.sample_rate);
            if format.format_flags & 32 != 0 {
                if format.bytes_per_frame != 4 || format.bytes_per_packet != 4 {
                    return Err(Error::MalformedProperty);
                }
                channels.extend((0..count).map(|_| 1));
            } else {
                if format.bytes_per_frame != count as u32 * 4
                    || format.bytes_per_packet != count as u32 * 4
                {
                    return Err(Error::MalformedProperty);
                }
                channels.push(count);
            }
            total += count;
            if total > 512 {
                return Err(Error::MalformedProperty);
            }
        }
        let rate = rate.ok_or(Error::MalformedProperty)?;
        Ok(Self {
            channels: channels.into_boxed_slice(),
            total,
            rate,
        })
    }
    pub fn channels(&self) -> usize {
        self.total
    }
    pub fn sample_rate(&self) -> f64 {
        self.rate
    }
}

#[derive(Clone, Copy)]
struct FloatBuffer<'a> {
    channels: usize,
    samples: &'a [f32],
    first_channel: usize,
}
pub struct InputBuffers<'a> {
    buffers: [Option<FloatBuffer<'a>>; 512],
    count: usize,
    frames: usize,
    channels: usize,
}

/// Read the count first; a zero-buffer C list may contain only its header and
/// must never be dereferenced as a Rust reference to the full 24-byte struct.
unsafe fn buffer_count(pointer: *const AudioBufferList) -> Result<usize, Error> {
    if pointer.is_null() {
        return Err(Error::MalformedProperty);
    }
    // SAFETY: caller guarantees at least a readable C list header.
    let count = unsafe { pointer.cast::<u32>().read_unaligned() } as usize;
    if count > 512 {
        return Err(Error::MalformedProperty);
    }
    Ok(count)
}
unsafe fn descriptor(pointer: *const AudioBufferList, index: usize) -> AudioBuffer {
    // SAFETY: index is below the list count; 8-byte first-buffer offset and
    // 16-byte entries are compared with Apple's SDK by test_rust_abi.c.
    unsafe {
        pointer
            .cast::<u8>()
            .add(8)
            .cast::<AudioBuffer>()
            .add(index)
            .read_unaligned()
    }
}

impl<'a> InputBuffers<'a> {
    /// # Safety
    /// HAL supplies the complete descriptor list and readable native float32
    /// storage matching layout. Storage stays immutable and alive for 'a.
    pub unsafe fn from_raw(
        pointer: *const AudioBufferList,
        layout: &CaptureLayout,
    ) -> Result<Self, Error> {
        let count = unsafe { buffer_count(pointer)? };
        if count != layout.channels.len() {
            return Err(Error::MalformedProperty);
        }
        let mut result = Self {
            buffers: [None; 512],
            count,
            frames: 0,
            channels: layout.total,
        };
        let mut first_channel = 0;
        for index in 0..count {
            let buffer = unsafe { descriptor(pointer, index) };
            let channels = layout.channels[index];
            if buffer.channels as usize != channels
                || buffer.byte_size as usize % (channels * 4) != 0
            {
                return Err(Error::MalformedProperty);
            }
            let frames = buffer.byte_size as usize / (channels * 4);
            if frames > 4096 || (index != 0 && frames != result.frames) {
                return Err(Error::MalformedProperty);
            }
            if index == 0 {
                result.frames = frames;
            }
            let samples = if frames == 0 {
                &[][..]
            } else {
                if buffer.data.is_null() || (buffer.data as usize) % align_of::<f32>() != 0 {
                    return Err(Error::MalformedProperty);
                }
                // SAFETY: validated float layout/count; caller guarantees live
                // immutable payload storage throughout the callback lifetime.
                unsafe { std::slice::from_raw_parts(buffer.data.cast::<f32>(), frames * channels) }
            };
            result.buffers[index] = Some(FloatBuffer {
                channels,
                samples,
                first_channel,
            });
            first_channel += channels;
        }
        Ok(result)
    }
    pub fn frames(&self) -> usize {
        self.frames
    }
    pub fn channels(&self) -> usize {
        self.channels
    }
    pub fn sample(&self, frame: usize, channel: usize) -> Option<f32> {
        if frame >= self.frames || channel >= self.channels {
            return None;
        }
        for buffer in self.buffers.iter().take(self.count).flatten() {
            if channel >= buffer.first_channel && channel < buffer.first_channel + buffer.channels {
                return buffer
                    .samples
                    .get(frame * buffer.channels + channel - buffer.first_channel)
                    .copied();
            }
        }
        None
    }
}

/// # Safety
/// HAL supplies the complete list and unique writable payloads for this call.
/// Call before constructing input borrows, and never retain any payload pointer.
pub unsafe fn silence_outputs(pointer: *mut AudioBufferList) -> Result<(), Error> {
    if pointer.is_null() {
        return Ok(());
    }
    let count = unsafe { buffer_count(pointer)? };
    for index in 0..count {
        let buffer = unsafe { descriptor(pointer, index) };
        if buffer.byte_size as usize > 512 * 4096 * 8 {
            return Err(Error::MalformedProperty);
        }
        if !buffer.data.is_null() {
            // SAFETY: caller grants writable byte_size bytes, bounded above.
            unsafe {
                buffer
                    .data
                    .cast::<u8>()
                    .write_bytes(0, buffer.byte_size as usize)
            };
        }
    }
    Ok(())
}

/// # Safety
/// HAL supplies the complete list and one unique writable interleaved stereo
/// float32 payload, alive for 'a. This device format was checked before Start.
pub unsafe fn stereo_output<'a>(
    pointer: *mut AudioBufferList,
) -> Result<&'a mut [[f32; 2]], Error> {
    if unsafe { buffer_count(pointer)? } != 1 {
        return Err(Error::MalformedProperty);
    }
    let buffer = unsafe { descriptor(pointer, 0) };
    if buffer.channels != 2 || buffer.byte_size % 8 != 0 || buffer.byte_size > 4096 * 8 {
        return Err(Error::MalformedProperty);
    }
    if buffer.byte_size == 0 {
        return Ok(&mut []);
    }
    if buffer.data.is_null() || (buffer.data as usize) % align_of::<f32>() != 0 {
        return Err(Error::MalformedProperty);
    }
    // SAFETY: caller supplies unique storage and the validated native format.
    Ok(unsafe {
        std::slice::from_raw_parts_mut(
            buffer.data.cast::<[f32; 2]>(),
            buffer.byte_size as usize / 8,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[repr(C)]
    struct TwoBuffers {
        count: u32,
        buffers: [AudioBuffer; 2],
    }
    fn format(channels: u32) -> StreamFormat {
        StreamFormat {
            sample_rate: 48_000.0,
            format_id: fourcc(b"lpcm"),
            format_flags: 9,
            bytes_per_packet: channels * 4,
            frames_per_packet: 1,
            bytes_per_frame: channels * 4,
            channels_per_frame: channels,
            bits_per_channel: 32,
            reserved: 0,
        }
    }
    #[test]
    fn multiple_streams_map_global_channels_and_reject_short_buffers() {
        let layout = CaptureLayout::try_new(&[format(2), format(1)]).unwrap();
        let mut first = [1_f32, 2.0, 3.0, 4.0];
        let mut second = [5_f32, 6.0];
        let mut list = TwoBuffers {
            count: 2,
            buffers: [
                AudioBuffer {
                    channels: 2,
                    byte_size: 16,
                    data: first.as_mut_ptr().cast(),
                },
                AudioBuffer {
                    channels: 1,
                    byte_size: 8,
                    data: second.as_mut_ptr().cast(),
                },
            ],
        };
        // SAFETY: native initialized descriptors and disjoint live float arrays.
        let input = unsafe { InputBuffers::from_raw((&list as *const TwoBuffers).cast(), &layout) }
            .unwrap();
        assert_eq!(input.frames(), 2);
        assert_eq!(input.channels(), 3);
        assert_eq!(input.sample(1, 0), Some(3.0));
        assert_eq!(input.sample(1, 2), Some(6.0));
        assert_eq!(input.sample(1, 3), None);
        list.buffers[1].byte_size = 4;
        // SAFETY: readable descriptors; inconsistent frame counts reject early.
        assert!(
            unsafe { InputBuffers::from_raw((&list as *const TwoBuffers).cast(), &layout) }
                .is_err()
        );
    }
    #[test]
    fn zero_buffer_header_is_not_read_as_a_full_buffer_list() {
        let mut header = 0_u32;
        // SAFETY: count is zero; no bytes beyond the initialized count are read.
        assert!(unsafe { silence_outputs((&mut header as *mut u32).cast()) }.is_ok());
        // SAFETY: zero header is valid but cannot describe stereo output.
        assert!(unsafe { stereo_output((&mut header as *mut u32).cast()) }.is_err());
    }
}
