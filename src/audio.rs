use crate::protocol::{FRAME_BYTES, SAMPLE_RATE};
use std::{collections::VecDeque, mem::size_of, ptr};
use windows_sys::Win32::Media::Audio::*;

// The allocation and PCM storage must not move while a Windows driver owns them.
#[repr(align(8))]
struct Header(WAVEHDR);

struct Buffer {
    header: Box<Header>,
    _bytes: Box<[u8]>,
}

impl Buffer {
    fn new(bytes: Vec<u8>) -> Self {
        let mut bytes = bytes.into_boxed_slice();
        let mut header: Box<Header> = Box::new(Header(unsafe { std::mem::zeroed() }));
        header.0.lpData = bytes.as_mut_ptr();
        header.0.dwBufferLength = bytes.len() as u32;
        Self {
            header,
            _bytes: bytes,
        }
    }

    fn done(&self) -> bool {
        // Header is explicitly aligned; dwFlags is at a 4-byte aligned offset.
        unsafe { ptr::read_volatile(ptr::addr_of!(self.header.0.dwFlags)) & WHDR_DONE != 0 }
    }
}

fn format() -> WAVEFORMATEX {
    WAVEFORMATEX {
        wFormatTag: 1,
        nChannels: 1,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * 2,
        nBlockAlign: 2,
        wBitsPerSample: 16,
        cbSize: 0,
    }
}

fn audio_error(code: u32, microphone: bool) -> String {
    let mut text = [0_u16; 256];
    unsafe {
        if microphone {
            waveInGetErrorTextW(code, text.as_mut_ptr(), text.len() as u32);
        } else {
            waveOutGetErrorTextW(code, text.as_mut_ptr(), text.len() as u32);
        }
    }
    let length = text.iter().position(|c| *c == 0).unwrap_or(text.len());
    format!(
        "{}: {} ({code})",
        if microphone {
            "Микрофон"
        } else {
            "Динамики"
        },
        String::from_utf16_lossy(&text[..length])
    )
}

pub struct Capture {
    handle: HWAVEIN,
    buffers: Vec<Buffer>,
    next: usize,
}

impl Capture {
    pub fn open() -> Result<Self, String> {
        let mut handle = ptr::null_mut();
        let result =
            unsafe { waveInOpen(&mut handle, WAVE_MAPPER, &format(), 0, 0, CALLBACK_NULL) };
        if result != 0 {
            return Err(audio_error(result, true));
        }
        let mut capture = Self {
            handle,
            buffers: Vec::new(),
            next: 0,
        };
        for _ in 0..8 {
            let mut buffer = Buffer::new(vec![0; FRAME_BYTES]);
            let header = &mut buffer.header.0;
            let result =
                unsafe { waveInPrepareHeader(handle, header, size_of::<WAVEHDR>() as u32) };
            if result != 0 {
                return Err(audio_error(result, true));
            }
            capture.buffers.push(buffer);
            let header = &mut capture.buffers.last_mut().unwrap().header.0;
            let result = unsafe { waveInAddBuffer(handle, header, size_of::<WAVEHDR>() as u32) };
            if result != 0 {
                return Err(audio_error(result, true));
            }
        }
        Ok(capture)
    }

    pub fn start(&mut self) -> Result<(), String> {
        let result = unsafe { waveInStart(self.handle) };
        if result == 0 {
            Ok(())
        } else {
            Err(audio_error(result, true))
        }
    }

    pub fn poll(&mut self) -> Result<Option<Vec<u8>>, String> {
        let buffer = &mut self.buffers[self.next];
        if !buffer.done() {
            return Ok(None);
        }
        let length = buffer.header.0.dwBytesRecorded as usize;
        let pcm = buffer._bytes[..length.min(FRAME_BYTES)].to_vec();
        let result = unsafe {
            waveInAddBuffer(
                self.handle,
                &mut buffer.header.0,
                size_of::<WAVEHDR>() as u32,
            )
        };
        if result != 0 {
            return Err(audio_error(result, true));
        }
        self.next = (self.next + 1) % self.buffers.len();
        Ok((pcm.len() == FRAME_BYTES).then_some(pcm))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            waveInReset(self.handle);
            for buffer in &mut self.buffers {
                waveInUnprepareHeader(
                    self.handle,
                    &mut buffer.header.0,
                    size_of::<WAVEHDR>() as u32,
                );
            }
            waveInClose(self.handle);
        }
    }
}

pub struct Speaker {
    handle: HWAVEOUT,
    pending: VecDeque<Buffer>,
}

impl Speaker {
    pub fn open() -> Result<Self, String> {
        let mut handle = ptr::null_mut();
        let result =
            unsafe { waveOutOpen(&mut handle, WAVE_MAPPER, &format(), 0, 0, CALLBACK_NULL) };
        if result != 0 {
            return Err(audio_error(result, false));
        }
        Ok(Self {
            handle,
            pending: VecDeque::new(),
        })
    }

    pub fn collect(&mut self) {
        while self.pending.front().is_some_and(Buffer::done) {
            let mut buffer = self.pending.pop_front().unwrap();
            unsafe {
                waveOutUnprepareHeader(
                    self.handle,
                    &mut buffer.header.0,
                    size_of::<WAVEHDR>() as u32,
                );
            }
        }
    }

    pub fn idle(&mut self) -> bool {
        self.collect();
        self.pending.is_empty()
    }

    pub fn play(&mut self, pcm: Vec<u8>) -> Result<(), String> {
        self.collect();
        // Bound latency after a burst; never build an ever-growing sound queue.
        if self.pending.len() >= 8 {
            return Ok(());
        }
        let mut buffer = Buffer::new(pcm);
        let size = size_of::<WAVEHDR>() as u32;
        let result = unsafe { waveOutPrepareHeader(self.handle, &mut buffer.header.0, size) };
        if result != 0 {
            return Err(audio_error(result, false));
        }
        let result = unsafe { waveOutWrite(self.handle, &mut buffer.header.0, size) };
        if result != 0 {
            unsafe {
                waveOutUnprepareHeader(self.handle, &mut buffer.header.0, size);
            }
            return Err(audio_error(result, false));
        }
        self.pending.push_back(buffer);
        Ok(())
    }

    pub fn reset(&mut self) {
        unsafe {
            waveOutReset(self.handle);
        }
        self.collect();
    }
}

impl Drop for Speaker {
    fn drop(&mut self) {
        self.reset();
        unsafe {
            waveOutClose(self.handle);
        }
    }
}

pub fn chime() -> Vec<u8> {
    let mut pcm = Vec::with_capacity(SAMPLE_RATE as usize * 480 / 1000 * 2);
    for frequency in [523.25_f32, 659.25, 783.99] {
        // C5, E5, G5: one major triad.
        let tone = SAMPLE_RATE as usize * 110 / 1000;
        let silence = SAMPLE_RATE as usize * 50 / 1000;
        for n in 0..tone {
            let attack = (n as f32 / 160.0).min(1.0);
            let release = ((tone - n) as f32 / 240.0).min(1.0);
            let wave = (n as f32 * frequency * std::f32::consts::TAU / SAMPLE_RATE as f32).sin();
            let sample = (wave * attack * release * 0.12 * i16::MAX as f32) as i16;
            pcm.extend(sample.to_le_bytes());
        }
        pcm.resize(pcm.len() + silence * 2, 0);
    }
    pcm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warning_has_three_soft_tones_and_lasts_exactly_480_ms() {
        let pcm = chime();
        assert_eq!(pcm.len(), SAMPLE_RATE as usize * 480 / 1000 * 2);
        let samples: Vec<_> = pcm
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert!(samples.iter().all(|s| s.unsigned_abs() <= 3933));
        for part in samples.as_chunks::<2560>().0 {
            assert!(part[..1760].iter().any(|s| *s != 0));
            assert!(part[1760..].iter().all(|s| *s == 0));
            assert_eq!(part[0], 0);
        }
    }
}
