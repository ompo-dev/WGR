//! Consumo do ring estéreo f32/48 kHz da engine e saída nativa CPAL.
//! O callback não aloca buffers nem mantém ponteiros crus para a memória WASM.

use anyhow::{Context, Result, bail, ensure};
use cpal::{
    FromSample, Sample, SampleFormat, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};
use wgr_runtime::shared::GuestMemory;

pub struct AudioRing {
    memory: GuestMemory,
    base: u64,
    capacity: u32,
    fraction: u64,
    last_read: Option<u32>,
    last_rate: u32,
}

impl AudioRing {
    pub fn new(memory: GuestMemory, base: u64, capacity: u32) -> Result<Self> {
        ensure!(
            base.is_multiple_of(4)
                && capacity.is_power_of_two()
                && capacity <= 1 << 20
                && base
                    .checked_add(64 + u64::from(capacity) * 8)
                    .is_some_and(|end| end <= memory.len() as u64),
            "Ring de áudio inválido"
        );
        Ok(Self {
            memory,
            base,
            capacity,
            fraction: 0,
            last_read: None,
            last_rate: 0,
        })
    }

    fn sample(&self, frame: u32, channel: u64) -> Result<f32> {
        let address = self.base + 64 + u64::from(frame & (self.capacity - 1)) * 8 + channel * 4;
        let value = f32::from_bits(self.memory.load_u32(address)?);
        Ok(if value.is_finite() {
            value.clamp(-1.0, 1.0)
        } else {
            0.0
        })
    }

    /// Retorna frames produzidos; o restante recebe silêncio inclusive em erro.
    pub fn render<T: Sample + FromSample<f32>>(
        &mut self,
        output: &mut [T],
        channels: u16,
        rate: u32,
        muted: bool,
    ) -> Result<usize> {
        output.fill(T::EQUILIBRIUM);
        ensure!(
            channels > 0 && rate > 0 && output.len().is_multiple_of(channels as usize),
            "Configuração de áudio inválida"
        );
        let read = self.memory.load_u32(self.base + 4)?;
        let head = self.memory.load_u32(self.base)?;
        let available = head.wrapping_sub(read);
        ensure!(
            available <= self.capacity,
            "Contadores do ring de áudio inválidos"
        );
        if self.last_read != Some(read) || self.last_rate != rate {
            self.fraction = 0;
        }
        self.last_rate = rate;
        let mut position = self.fraction;
        let mut produced = 0;
        for frame in output.chunks_exact_mut(channels as usize) {
            let index = position / u64::from(rate);
            let remainder = position % u64::from(rate);
            let next = position
                .checked_add(48_000)
                .context("Posição de áudio excedida")?;
            if index >= u64::from(available)
                || (remainder != 0 && index + 1 >= u64::from(available))
                || next / u64::from(rate) > u64::from(available)
            {
                break;
            }
            if !muted {
                let mut stereo = [0.0; 2];
                for (channel, sample) in stereo.iter_mut().enumerate() {
                    let current = self.sample(read.wrapping_add(index as u32), channel as u64)?;
                    *sample = if remainder == 0 {
                        current
                    } else {
                        let following =
                            self.sample(read.wrapping_add(index as u32 + 1), channel as u64)?;
                        let t = remainder as f32 / rate as f32;
                        current * (1.0 - t) + following * t
                    };
                }
                if channels == 1 {
                    frame[0] = T::from_sample((stereo[0] + stereo[1]) * 0.5);
                } else {
                    frame[0] = T::from_sample(stereo[0]);
                    frame[1] = T::from_sample(stereo[1]);
                }
            }
            position = next;
            produced += 1;
        }
        let consumed = position / u64::from(rate);
        self.fraction = position % u64::from(rate);
        let tail = read.wrapping_add(consumed as u32);
        self.memory.store_u32(self.base + 4, tail)?;
        self.memory.add_u32(self.base + 12, 1)?;
        self.memory.notify(self.base + 4)?;
        self.last_read = Some(tail);
        Ok(produced)
    }
}

pub struct AudioOutput {
    stop: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
    callbacks: Arc<AtomicU64>,
    worker: Option<JoinHandle<()>>,
}

impl AudioOutput {
    pub fn start(memory: GuestMemory, base: u64, capacity: u32, muted: bool) -> Result<Self> {
        let ring = AudioRing::new(memory, base, capacity)?;
        let stop = Arc::new(AtomicBool::new(false));
        let muted = Arc::new(AtomicBool::new(muted));
        let callbacks = Arc::new(AtomicU64::new(0));
        let (ready, receiver) = mpsc::sync_channel(1);
        let stopped = stop.clone();
        let mute = muted.clone();
        let count = callbacks.clone();
        let worker = std::thread::Builder::new()
            .name("wgr-audio".into())
            .spawn(move || match output_stream(ring, mute, count) {
                Ok(stream) => {
                    let _ = ready.send(Ok(()));
                    while !stopped.load(Ordering::Acquire) {
                        std::thread::park_timeout(Duration::from_millis(20));
                    }
                    drop(stream);
                }
                Err(error) => {
                    let _ = ready.send(Err(format!("{error:#}")));
                }
            })?;
        match receiver.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(Self {
                stop,
                muted,
                callbacks,
                worker: Some(worker),
            }),
            result => {
                stop.store(true, Ordering::Release);
                worker.thread().unpark();
                if let Ok(Err(error)) = result {
                    let _ = worker.join();
                    bail!("Saída de áudio: {error}");
                }
                bail!("A saída de áudio não iniciou no prazo");
            }
        }
    }

    pub fn mute(&self, muted: bool) {
        self.muted.store(muted, Ordering::Release);
    }
    pub fn callbacks(&self) -> u64 {
        self.callbacks.load(Ordering::Relaxed)
    }
}

impl Drop for AudioOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

fn output_stream(
    ring: AudioRing,
    muted: Arc<AtomicBool>,
    callbacks: Arc<AtomicU64>,
) -> Result<cpal::Stream> {
    let device = cpal::default_host()
        .default_output_device()
        .context("Dispositivo de áudio ausente")?;
    let config = device.default_output_config()?;
    macro_rules! stream {
        ($sample:ty) => {
            build_stream::<$sample>(&device, config.into(), ring, muted, callbacks)
        };
    }
    match config.sample_format() {
        SampleFormat::F32 => stream!(f32),
        SampleFormat::F64 => stream!(f64),
        SampleFormat::I8 => stream!(i8),
        SampleFormat::I16 => stream!(i16),
        SampleFormat::I24 => stream!(cpal::I24),
        SampleFormat::I32 => stream!(i32),
        SampleFormat::I64 => stream!(i64),
        SampleFormat::U8 => stream!(u8),
        SampleFormat::U16 => stream!(u16),
        SampleFormat::U24 => stream!(cpal::U24),
        SampleFormat::U32 => stream!(u32),
        SampleFormat::U64 => stream!(u64),
        format => bail!("Formato de áudio não PCM: {format}"),
    }
}

fn build_stream<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut ring: AudioRing,
    muted: Arc<AtomicBool>,
    callbacks: Arc<AtomicU64>,
) -> Result<cpal::Stream> {
    let channels = config.channels;
    let rate = config.sample_rate;
    let stream = device.build_output_stream(
        config,
        move |output: &mut [T], _| {
            let _ = ring.render(output, channels, rate, muted.load(Ordering::Acquire));
            callbacks.fetch_add(1, Ordering::Relaxed);
        },
        |error| tracing::error!("Saída de áudio falhou: {error}"),
        None,
    )?;
    stream.play()?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ring(capacity: u32) -> AudioRing {
        let runtime = wgr_runtime::NativeRuntime::new().unwrap();
        let memory = GuestMemory::new64(runtime.engine(), 2, 2, 131072).unwrap();
        AudioRing::new(memory, 64, capacity).unwrap()
    }
    fn put(ring: &AudioRing, index: u32, left: f32, right: f32) {
        let at = ring.base + 64 + u64::from(index & (ring.capacity - 1)) * 8;
        ring.memory.store_u32(at, left.to_bits()).unwrap();
        ring.memory.store_u32(at + 4, right.to_bits()).unwrap();
    }
    #[test]
    fn validates_geometry_before_touching_memory() {
        let ring = ring(8);
        for (base, capacity) in [(1, 8), (64, 0), (64, 3), (u64::MAX - 3, 8), (64, 1 << 21)] {
            assert!(AudioRing::new(ring.memory.clone(), base, capacity).is_err());
        }
    }
    #[test]
    fn wraps_counters_and_silences_underruns_without_reading_past_head() {
        let mut ring = ring(8);
        let start = u32::MAX - 1;
        for i in 0..4 {
            put(
                &ring,
                start.wrapping_add(i),
                i as f32 / 4.0,
                -(i as f32) / 4.0,
            );
        }
        ring.memory.store_u32(68, start).unwrap();
        ring.memory.store_u32(64, start.wrapping_add(4)).unwrap();
        let mut output = [9f32; 12];
        assert_eq!(ring.render(&mut output, 2, 48000, false).unwrap(), 4);
        assert_eq!(
            output,
            [0., 0., 0.25, -0.25, 0.5, -0.5, 0.75, -0.75, 0., 0., 0., 0.]
        );
        assert_eq!(ring.memory.load_u32(68).unwrap(), 2);
        assert_eq!(ring.memory.load_u32(76).unwrap(), 1);
    }
    #[test]
    fn resamples_without_float_counter_drift() {
        let mut ring = ring(8192);
        for i in 0..8000 {
            put(&ring, i, 0.25, 0.25);
        }
        ring.memory.store_u32(64, 8000).unwrap();
        let mut output = vec![0f32; 4410];
        assert_eq!(ring.render(&mut output, 1, 44100, false).unwrap(), 4410);
        assert!(output.iter().all(|&value| value == 0.25));
        assert_eq!(ring.memory.load_u32(68).unwrap(), 4800);
        assert_eq!(ring.fraction, 0);
    }
    #[test]
    fn keeps_fraction_across_callbacks_and_mute_still_consumes() {
        let mut ring = ring(8);
        put(&ring, 0, 0., 0.);
        put(&ring, 1, 1., 1.);
        ring.memory.store_u32(64, 2).unwrap();
        let mut output = [0f32; 1];
        ring.render(&mut output, 1, 96000, false).unwrap();
        assert_eq!(output, [0.]);
        ring.render(&mut output, 1, 96000, false).unwrap();
        assert_eq!(output, [0.5]);
        ring.render(&mut output, 1, 48000, true).unwrap();
        assert_eq!(output, [0.]);
        assert_eq!(ring.memory.load_u32(68).unwrap(), 2);
    }
    #[test]
    fn rejects_corruption_and_sanitizes_non_finite_samples() {
        let mut ring = ring(8);
        put(&ring, 0, f32::NAN, 2.);
        ring.memory.store_u32(64, 1).unwrap();
        let mut output = [9f32; 2];
        ring.render(&mut output, 2, 48000, false).unwrap();
        assert_eq!(output, [0., 1.]);
        ring.memory.store_u32(64, 99).unwrap();
        assert!(ring.render(&mut output, 2, 48000, false).is_err());
        assert_eq!(output, [0., 0.]);
    }

    #[test]
    fn changing_output_rate_resets_the_fraction() {
        let mut ring = ring(8);
        put(&ring, 0, 0., 0.);
        put(&ring, 1, 1., 1.);
        ring.memory.store_u32(64, 2).unwrap();
        let mut output = [0f32; 1];
        ring.render(&mut output, 1, 96000, false).unwrap();
        ring.render(&mut output, 1, 48000, false).unwrap();
        assert_eq!(output, [0.]);
        assert_eq!(ring.memory.load_u32(68).unwrap(), 1);
    }
    #[test]
    #[ignore = "requer dispositivo de saída de áudio; saída permanece muda"]
    fn starts_and_stops_a_real_muted_output() {
        let ring = ring(8192);
        let output = AudioOutput::start(ring.memory, 64, 8192, true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while output.callbacks() == 0 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(output);
    }
}
