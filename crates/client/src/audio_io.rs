use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        atomic::{AtomicPtr, AtomicU32},
    },
};

use bytes::Bytes;
use cpal::{
    self, Device, Host, SampleRate, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use crabmic_model::{RoomAudioPacket, RoomUserId, cancellable};
use opus;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::instrument;

#[derive(Debug)]
pub struct AudioIoChannelDeviceSide {
    pub audio_output: tokio::sync::mpsc::Sender<Bytes>,
    pub audio_input: tokio::sync::mpsc::Receiver<RoomAudioPacket>,
}

#[derive(Debug)]
pub struct AudioIoChannelNetworkSide {
    pub audio_input: tokio::sync::mpsc::Receiver<Bytes>,
    pub audio_output: tokio::sync::mpsc::Sender<RoomAudioPacket>,
}

pub fn channel() -> (AudioIoChannelDeviceSide, AudioIoChannelNetworkSide) {
    const DEFAULT_CHANNEL_SIZE: usize = 1024;
    let (audio_output_tx, audio_output_rx) = tokio::sync::mpsc::channel(DEFAULT_CHANNEL_SIZE);
    let (audio_input_tx, audio_input_rx) = tokio::sync::mpsc::channel(DEFAULT_CHANNEL_SIZE);
    (
        AudioIoChannelDeviceSide {
            audio_output: audio_output_tx,
            audio_input: audio_input_rx,
        },
        AudioIoChannelNetworkSide {
            audio_input: audio_output_rx,
            audio_output: audio_input_tx,
        },
    )
}
pub enum AudioIoServiceRequest {
    SetOutputVolume {
        user_id: RoomUserId,
        option: VolumeOption,
    },
    SetInputVolume {
        option: VolumeOption,
    },
    CreateChannel {
        user_id: RoomUserId,
    },
    RemoveChannel {
        user_id: RoomUserId,
    },
    SetInputDevice {
        device: Option<Device>,
    },
    SetOutputDevice {
        device: Option<Device>,
    },
}

#[derive(Debug, Clone)]
pub struct VolumeOption {
    pub volume: f32,
    pub muted: bool,
}

impl std::fmt::Display for VolumeOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const MUTED: char = '🔕';
        const UNMUTED: char = '🔔';
        let mute = if self.muted { MUTED } else { UNMUTED };
        let volume = self.volume;
        write!(f, "{volume:.02}/{mute}")
    }
}

impl VolumeOption {
    pub fn new(volume: f32) -> Self {
        Self {
            volume,
            muted: false,
        }
    }
    pub fn mute(&mut self) {
        self.muted = true
    }
    pub fn unmute(&mut self) {
        self.muted = false
    }
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume
    }
    pub fn get_volume(&self) -> f32 {
        if self.muted { 0.0 } else { self.volume }
    }
}

impl Default for VolumeOption {
    fn default() -> Self {
        Self {
            volume: 1.0,
            muted: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Loudness(f32);

pub struct AudioIoMetric {
    volumes: HashMap<RoomUserId, VolumeOption>,
    loudness: HashMap<RoomUserId, Loudness>,
}

pub struct AudioIoService {
    channel: AudioIoChannelDeviceSide,
    request_api: tokio::sync::mpsc::Receiver<AudioIoServiceRequest>,
    stream_config: cpal::StreamConfig,
    ct: CancellationToken,
}

impl AudioIoService {
    const BUFFER_SIZE: u32 = 4096;
    const SAMPLE_RATE: u32 = 48000;
    pub fn new(channel: AudioIoChannelDeviceSide, ct: CancellationToken) -> (Self, AudioIoClient) {
        let stream_config = StreamConfig {
            channels: 2,
            sample_rate: cpal::SampleRate(Self::SAMPLE_RATE),
            buffer_size: cpal::BufferSize::Fixed(Self::BUFFER_SIZE),
        };
        let (request_client, request_api) = tokio::sync::mpsc::channel(64);
        let service = Self {
            channel,
            request_api,
            stream_config,
            ct,
        };
        let client = AudioIoClient {
            tx: request_client,
            host: Arc::new(cpal::default_host()),
        };
        (service, client)
    }
    pub fn spawn(self) {
        tokio::spawn(self.serve());
    }
    #[instrument(name = "audio_io_service", skip(self))]
    async fn serve(self) -> Result<(), crate::ClientError> {
        let Self {
            channel:
                AudioIoChannelDeviceSide {
                    audio_output,
                    mut audio_input,
                },
            mut request_api,
            stream_config,
            ct,
        } = self;
        let sample_rate = stream_config.sample_rate.0;
        let mut decoders = HashMap::<RoomUserId, opus::Decoder>::new();
        let mixup_buffers = <Arc<Mutex<HashMap<RoomUserId, [f32; 4096]>>>>::default();
        let weights = <Arc<RwLock<HashMap<RoomUserId, f32>>>>::default();
        let input_volume = <Arc<RwLock<VolumeOption>>>::default();
        let mut input_handle = None;
        let mut output_handle = None;
        tracing::info!("output stream started");
        enum Event {
            Cancel,
            Request(AudioIoServiceRequest),
            Packet(RoomAudioPacket),
        }
        loop {
            let event = tokio::select! {
                _ = ct.cancelled() => {
                    Event::Cancel
                }
                event = request_api.recv() => {
                    if let Some(event) = event {
                        Event::Request(event)
                    } else {
                        break;
                    }
                }
                packet = audio_input.recv() => {
                    if let Some(packet) = packet {
                        Event::Packet(packet)
                    } else {
                        break;
                    }
                }
            };
            match event {
                Event::Cancel => {
                    break;
                }
                Event::Request(audio_io_service_request) => match audio_io_service_request {
                    AudioIoServiceRequest::SetOutputVolume { user_id, option } => {
                        let mut volumes = weights.write().await;
                        volumes.insert(user_id, option.get_volume());
                    }
                    AudioIoServiceRequest::SetInputVolume { option } => {
                        tracing::debug!(%option, "set input option");
                        let mut input_volume = input_volume.write().await;
                        *input_volume = option;
                    }
                    AudioIoServiceRequest::CreateChannel { user_id } => {
                        let decoder = opus::Decoder::new(sample_rate, opus::Channels::Stereo)
                            .map_err(crate::ClientError::contextual(
                                "fail to create opus decoder",
                            ))?;
                        decoders.insert(user_id, decoder);
                        let mut mixup_buffers = mixup_buffers.lock().await;
                        mixup_buffers.insert(user_id, [0.0; 4096]);
                        let mut weights = weights.write().await;
                        weights.insert(user_id, 1.0);
                    }
                    AudioIoServiceRequest::RemoveChannel { user_id } => {
                        decoders.remove(&user_id);
                        let mut mixup_buffers = mixup_buffers.lock().await;
                        mixup_buffers.remove(&user_id);
                        let mut weights = weights.write().await;
                        weights.remove(&user_id);
                    }
                    AudioIoServiceRequest::SetInputDevice { device } => {
                        if let Some(prev_input) = input_handle.take() {
                            drop(prev_input);
                        }
                        let Some(device) = device else {
                            continue;
                        };
                        let config = device
                            .default_input_config()
                            .expect("fail to get default input config");
                        let stream_config = stream_config.clone();
                        tracing::info!(?config, "set input device");
                        let config = StreamConfig {
                            channels: config.channels(),
                            sample_rate: config.sample_rate(),
                            buffer_size: cpal::BufferSize::Fixed(AudioIoService::BUFFER_SIZE),
                        };
                        let audio_output = audio_output.clone();
                        let input_volume = input_volume.clone();
                        let new_input_handle = std::thread::spawn(move || {
                            let mut encoder = opus::Encoder::new(
                                sample_rate,
                                opus::Channels::Stereo,
                                opus::Application::Audio,
                            )
                            .expect("fail to create opus encoder");
                            let mut buffer = [0u8; 4096];
                            let stream_result = device.build_input_stream(
                                &config,
                                move |data: &[f32], _| {
                                    let volume_option = input_volume.blocking_read();
                                    let volume = volume_option.get_volume();
                                    let data =
                                        data.iter().map(|&x| (x * volume)).collect::<Vec<_>>();
                                    let volume = data.iter().sum::<f32>() / data.len() as f32;
                                    tracing::debug!(?volume);
                                    let encode_result = encoder.encode_float(&data, &mut buffer);
                                    match encode_result {
                                        Ok(size) => {
                                            let bytes = Bytes::copy_from_slice(&buffer[..size]);
                                            let _ = audio_output.blocking_send(bytes);
                                        }
                                        Err(_) => {
                                            tracing::error!("fail to encode audio packet");
                                        }
                                    }
                                },
                                |e| {
                                    tracing::error!("audio output error: {:?}", e);
                                },
                                None,
                            );
                            match stream_result {
                                Ok(stream) => {
                                    stream.play().unwrap();
                                }
                                Err(e) => {
                                    tracing::error!(
                                        ?stream_config,
                                        "fail to create input stream: {:?}",
                                        e
                                    );
                                }
                            }
                        });
                        input_handle.replace(new_input_handle);
                    }
                    AudioIoServiceRequest::SetOutputDevice { device } => {
                        let Some(device) = device else {
                            output_handle.take();
                            continue;
                        };
                        let mixup_buffers = mixup_buffers.clone();
                        let weights = weights.clone();
                        let stream_config = stream_config.clone();
                        let new_output_handle = std::thread::spawn(move || {
                            let stream_result = device.build_output_stream(
                                &stream_config,
                                move |data: &mut [f32], _| {
                                    let weights = weights.blocking_read();
                                    for (user_id, buffer) in mixup_buffers.blocking_lock().iter() {
                                        let weight = weights.get(user_id).copied().unwrap_or(1.0);
                                        for (a, b) in data.iter_mut().zip(buffer.iter()) {
                                            *a += { *b } * weight;
                                        }
                                    }
                                },
                                |e| {
                                    tracing::error!("audio output error: {:?}", e);
                                },
                                None,
                            );
                            match stream_result {
                                Ok(stream) => {
                                    stream.play().unwrap();
                                }
                                Err(_) => todo!(),
                            }
                        });
                        output_handle.replace(new_output_handle);
                    }
                },
                Event::Packet(room_audio_packet) => {
                    let user_id = room_audio_packet.user_id;
                    let decoder = decoders
                        .entry(room_audio_packet.user_id)
                        .or_insert_with(|| {
                            opus::Decoder::new(sample_rate, opus::Channels::Stereo)
                                .expect("fail to create opus decoder")
                        });
                    let mut mixup_buffers = mixup_buffers.lock().await;
                    let buffer = mixup_buffers
                        .entry(room_audio_packet.user_id)
                        .or_insert([0.0; 4096]);
                    buffer.fill(0.0);
                    let decoded_size = decoder
                        .decode_float(&room_audio_packet.data, buffer, false)
                        .map_err(crate::ClientError::contextual(
                            "fail to decode audio packet",
                        ))?;
                    let loadness = buffer.iter().sum::<f32>() / (decoded_size as f32);
                    tracing::debug!(%loadness, %user_id, "audio packet decoded");
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct AudioIoClient {
    host: Arc<Host>,
    tx: tokio::sync::mpsc::Sender<AudioIoServiceRequest>,
}

impl std::fmt::Debug for AudioIoClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioIoClient")
            .field("host_id", &self.host.id())
            .field("tx", &self.tx)
            .finish()
    }
}
impl AudioIoClient {
    pub fn get_device_by_name(&self, name: &str) -> crate::Result<Device> {
        let host = cpal::default_host();
        let devices = host
            .devices()
            .map_err(crate::ClientError::contextual("fail to get devices"))?;
        for device in devices {
            let device_name = device
                .name()
                .map_err(crate::ClientError::contextual("fail to get device name"))?;
            if device_name == name {
                return Ok(device);
            }
        }
        Err(crate::ClientError::local(format!(
            "device not found: {}",
            name
        )))
    }
    pub async fn set_input_device_as_default(&self) -> crate::Result<()> {
        let default_device = self
            .host
            .default_input_device()
            .ok_or_else(|| crate::ClientError::local("no default input device"))?;
        let default_device_name = default_device
            .name()
            .map_err(crate::ClientError::contextual("fail to get device name"))?;
        tracing::info!(default_name=?default_device_name, "set input device as default");
        self.tx
            .send(AudioIoServiceRequest::SetInputDevice {
                device: Some(default_device),
            })
            .await
            .map_err(crate::ClientError::contextual("fail to send request"))?;
        Ok(())
    }
    pub async fn set_output_device_as_default(&self) -> crate::Result<()> {
        let default_device = self
            .host
            .default_output_device()
            .ok_or_else(|| crate::ClientError::local("no default input device"))?;
        let default_device_name = default_device
            .name()
            .map_err(crate::ClientError::contextual("fail to get device name"))?;
        tracing::info!(default_name=?default_device_name, "set output device as default");
        self.tx
            .send(AudioIoServiceRequest::SetOutputDevice {
                device: Some(default_device),
            })
            .await
            .map_err(crate::ClientError::contextual("fail to send request"))?;
        Ok(())
    }
    pub async fn set_input_device(&self, device_name: Option<String>) -> crate::Result<()> {
        tracing::info!(?device_name, "set input device");
        let device = if let Some(device) = device_name {
            Some(self.get_device_by_name(&device)?)
        } else {
            None
        };
        self.tx
            .send(AudioIoServiceRequest::SetInputDevice { device })
            .await
            .map_err(crate::ClientError::contextual("fail to send request"))?;
        Ok(())
    }
    pub async fn set_output_device(&self, device_name: Option<String>) -> crate::Result<()> {
        tracing::info!(?device_name, "set output device");
        let device = if let Some(device) = device_name {
            Some(self.get_device_by_name(&device)?)
        } else {
            None
        };
        self.tx
            .send(AudioIoServiceRequest::SetOutputDevice { device })
            .await
            .map_err(crate::ClientError::contextual("fail to send request"))?;
        Ok(())
    }
    pub async fn set_output_volume(
        &self,
        user_id: RoomUserId,
        option: VolumeOption,
    ) -> crate::Result<()> {
        self.tx
            .send(AudioIoServiceRequest::SetOutputVolume { user_id, option })
            .await
            .map_err(crate::ClientError::contextual("fail to send request"))?;
        Ok(())
    }
    pub async fn set_input_volume(&self, option: VolumeOption) -> crate::Result<()> {
        self.tx
            .send(AudioIoServiceRequest::SetInputVolume { option })
            .await
            .map_err(crate::ClientError::contextual("fail to send request"))?;
        Ok(())
    }
}
