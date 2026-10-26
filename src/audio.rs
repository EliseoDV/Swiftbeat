//! Salida de audio propia, en lugar del backend rodio de librespot.
//!
//! El de librespot abre el dispositivo una sola vez: si desaparece (se
//! desconectan los audífonos), espera para siempre a que el audio avance y el
//! reproductor se congela; y si no hay ninguno, hace `unwrap()` y, con
//! `panic = "abort"`, cierra la app. Esta sigue al dispositivo predeterminado de
//! Windows: si el actual falla o cambia, abre el nuevo, y si no hay ninguno
//! devuelve un error para que librespot pause.

use librespot_playback::{
    NUM_CHANNELS, SAMPLE_RATE,
    audio_backend::{Sink, SinkError, SinkResult},
    convert::Converter,
    decoder::AudioPacket,
};
use rodio::cpal::{
    self,
    traits::{DeviceTrait, HostTrait},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Fragmentos en cola antes de esperar (~0,5 s de audio), como el backend de librespot.
const MAX_QUEUED: usize = 26;
/// Cada cuánto se comprueba si Windows cambió el dispositivo predeterminado.
const DEVICE_CHECK: Duration = Duration::from_secs(1);
/// Tiempo sin que la cola avance para dar el dispositivo por perdido.
const STUCK: Duration = Duration::from_secs(1);

/// El reproductor quiso sonar y no había ninguna salida de audio.
static NO_AUDIO: AtomicBool = AtomicBool::new(false);

/// Devuelve si faltó la salida de audio desde la última consulta.
pub fn take_no_audio() -> bool {
    NO_AUDIO.swap(false, Ordering::Relaxed)
}

struct Stream {
    sink: rodio::Sink,
    _stream: rodio::OutputStream,
    device: String,
    /// cpal avisó de un error: el dispositivo ya no sirve.
    failed: Arc<AtomicBool>,
}

pub struct Output {
    stream: Option<Stream>,
    checked_at: Instant,
    playing: bool,
}

fn default_device() -> Option<(cpal::Device, String)> {
    let device = cpal::default_host().default_output_device()?;
    let name = device.name().unwrap_or_default();
    Some((device, name))
}

fn open_stream(device: cpal::Device, failed: &Arc<AtomicBool>) -> Result<rodio::OutputStream, rodio::StreamError> {
    let builder = |device: cpal::Device| {
        let failed = failed.clone();
        rodio::OutputStreamBuilder::from_device(device).map(|b| {
            b.with_error_callback(move |e: cpal::StreamError| {
                log::warn!("salida de audio: {e}");
                failed.store(true, Ordering::Relaxed);
            })
        })
    };
    // 44,1 kHz si el dispositivo lo admite; si no, la configuración que acepte.
    builder(device.clone())?
        .with_sample_rate(SAMPLE_RATE)
        .open_stream()
        .or_else(|_| builder(device)?.open_stream_or_fallback())
}

impl Output {
    pub fn new() -> Self {
        Self {
            stream: None,
            checked_at: Instant::now(),
            playing: false,
        }
    }

    /// Abre el dispositivo predeterminado (soltando antes el anterior).
    fn open(&mut self) {
        self.stream = None;
        self.checked_at = Instant::now();
        let Some((device, name)) = default_device() else {
            log::warn!("no hay ninguna salida de audio");
            return;
        };
        let failed = Arc::new(AtomicBool::new(false));
        let mut stream = match open_stream(device, &failed) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("no se pudo abrir {name}: {e}");
                return;
            }
        };
        stream.log_on_drop(false);
        let sink = rodio::Sink::connect_new(stream.mixer());
        if !self.playing {
            sink.pause();
        }
        log::info!("salida de audio: {name}");
        self.stream = Some(Stream {
            sink,
            _stream: stream,
            device: name,
            failed,
        });
    }

    /// Reabre la salida si falló o si Windows cambió el dispositivo predeterminado.
    /// Sin `force`, como mucho una comprobación por segundo.
    fn check_device(&mut self, force: bool) {
        let failed = self.stream.as_ref().is_some_and(|s| s.failed.load(Ordering::Relaxed));
        if !failed && !force && self.checked_at.elapsed() < DEVICE_CHECK {
            return;
        }
        self.checked_at = Instant::now();
        if let Some(s) = &self.stream
            && !failed
            && default_device().is_some_and(|(_, name)| name == s.device)
        {
            return;
        }
        if self.stream.is_some() {
            log::info!("la salida de audio cambió o se perdió: se abre la predeterminada");
        }
        self.open();
    }

    fn sink(&self) -> SinkResult<&rodio::Sink> {
        match &self.stream {
            Some(s) => Ok(&s.sink),
            None => {
                NO_AUDIO.store(true, Ordering::Relaxed);
                Err(SinkError::NotConnected("no hay salida de audio".into()))
            }
        }
    }
}

impl Sink for Output {
    fn start(&mut self) -> SinkResult<()> {
        self.playing = true;
        self.check_device(true);
        self.sink()?.play();
        Ok(())
    }

    fn stop(&mut self) -> SinkResult<()> {
        self.playing = false;
        let Some(s) = &self.stream else { return Ok(()) };
        // Deja sonar lo que queda en cola, sin esperar a un dispositivo que ya no avanza.
        let deadline = Instant::now() + STUCK;
        while !s.sink.empty() && !s.failed.load(Ordering::Relaxed) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        s.sink.pause();
        Ok(())
    }

    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        self.check_device(false);
        let samples = packet.samples().map_err(|e| SinkError::OnWrite(e.to_string()))?;
        let data = converter.f64_to_f32(samples);
        let sink = self.sink()?;
        sink.append(rodio::buffer::SamplesBuffer::new(NUM_CHANNELS as u16, SAMPLE_RATE, data));

        let Some(s) = &self.stream else { return Ok(()) };
        let (mut len, mut moved_at) = (s.sink.len(), Instant::now());
        while len > MAX_QUEUED {
            if s.failed.load(Ordering::Relaxed) {
                break;
            }
            if moved_at.elapsed() > STUCK {
                // El dispositivo dejó de consumir sin avisar: en la siguiente escritura se reabre.
                s.failed.store(true, Ordering::Relaxed);
                break;
            }
            thread::sleep(Duration::from_millis(10));
            let now = s.sink.len();
            if now != len {
                (len, moved_at) = (now, Instant::now());
            }
        }
        Ok(())
    }
}
