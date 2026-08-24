//! Web Audio backend implementation.
//!
//! Default backend on WebAssembly.

extern crate js_sys;
extern crate wasm_bindgen;
extern crate web_sys;

use self::wasm_bindgen::prelude::*;
use self::wasm_bindgen::JsCast;
use self::web_sys::{AudioContext, AudioContextOptions};
use crate::traits::{DeviceTrait, HostTrait, StreamTrait};
use crate::{
    BackendSpecificError, BufferSize, BuildStreamError, Data, DefaultStreamConfigError,
    DeviceDescription, DeviceDescriptionBuilder, DeviceId, DeviceIdError, DeviceNameError,
    DevicesError, InputCallbackInfo, OutputCallbackInfo, PauseStreamError, PlayStreamError,
    SampleFormat, SampleRate, StreamConfig, StreamError, SupportedBufferSize,
    SupportedStreamConfig, SupportedStreamConfigRange, SupportedStreamConfigsError,
};
use std::cell::RefCell;
use std::ops::DerefMut;
use std::sync::{Arc, Mutex};
use std::time::Duration;

thread_local! {
    static SHARED_AUDIO_CONTEXT: RefCell<Option<Arc<AudioContext>>> = RefCell::new(None);
}

/// Close the shared AudioContext and clear the thread-local, stopping all callbacks.
pub fn close_audio_context() {
    SHARED_AUDIO_CONTEXT.with(|cell| {
        if let Some(ctx) = cell.borrow_mut().take() {
            let _ = ctx.close();
        }
    });
}

fn get_or_create_audio_context(sample_rate: SampleRate) -> Result<Arc<AudioContext>, BuildStreamError> {
    SHARED_AUDIO_CONTEXT.with(|cell| {
        let mut ctx_opt = cell.borrow_mut();
        if let Some(ctx) = ctx_opt.as_ref() {
            return Ok(ctx.clone());
        }
        let stream_opts = AudioContextOptions::new();
        stream_opts.set_sample_rate(sample_rate as f32);
        let ctx = AudioContext::new_with_context_options(&stream_opts).map_err(
            |err| -> BuildStreamError {
                let description = format!("{:?}", err);
                BackendSpecificError { description }.into()
            },
        )?;
        #[allow(clippy::arc_with_non_send_sync)]
        let ctx = Arc::new(ctx);
        *ctx_opt = Some(ctx.clone());
        Ok(ctx)
    })
}

/// Type alias for shared closure handles used in audio callbacks

/// Content is false if the iterator is empty.
pub struct Devices(bool);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Device;

pub struct Host;

pub struct Stream {
    ctx: Arc<AudioContext>,
    buffer_size_frames: usize,
    _closures: Vec<Box<dyn std::any::Any>>,
}

// WASM runs in a single-threaded environment, so Send and Sync are safe by design.
unsafe impl Send for Stream {}
unsafe impl Sync for Stream {}

// Compile-time assertion that Stream is Send and Sync
crate::assert_stream_send!(Stream);
crate::assert_stream_sync!(Stream);

pub use crate::iter::{SupportedInputConfigs, SupportedOutputConfigs};

const MIN_CHANNELS: u16 = 1;
const MAX_CHANNELS: u16 = 32;
const MIN_SAMPLE_RATE: SampleRate = 8_000;
const MAX_SAMPLE_RATE: SampleRate = 96_000;
const DEFAULT_SAMPLE_RATE: SampleRate = 44_100;
const MIN_BUFFER_SIZE: u32 = 1;
const MAX_BUFFER_SIZE: u32 = u32::MAX;
const DEFAULT_BUFFER_SIZE: usize = 2048;
const SUPPORTED_SAMPLE_FORMAT: SampleFormat = SampleFormat::F32;

impl Host {
    pub fn new() -> Result<Self, crate::HostUnavailable> {
        Ok(Host)
    }
}

impl HostTrait for Host {
    type Devices = Devices;
    type Device = Device;

    fn is_available() -> bool {
        // Assume this host is always available on webaudio.
        true
    }

    fn devices(&self) -> Result<Self::Devices, DevicesError> {
        Devices::new()
    }

    fn default_input_device(&self) -> Option<Self::Device> {
        default_input_device()
    }

    fn default_output_device(&self) -> Option<Self::Device> {
        default_output_device()
    }
}

impl Devices {
    fn new() -> Result<Self, DevicesError> {
        Ok(Self::default())
    }
}

impl Device {
    fn description(&self) -> Result<DeviceDescription, DeviceNameError> {
        Ok(DeviceDescriptionBuilder::new("Default Device".to_string())
            .direction(crate::DeviceDirection::Duplex)
            .build())
    }

    fn id(&self) -> Result<DeviceId, DeviceIdError> {
        Ok(DeviceId(
            crate::platform::HostId::WebAudio,
            "default".to_string(),
        ))
    }

    fn supported_input_configs(
        &self,
    ) -> Result<SupportedInputConfigs, SupportedStreamConfigsError> {
        let buffer_size = SupportedBufferSize::Range {
            min: MIN_BUFFER_SIZE,
            max: MAX_BUFFER_SIZE,
        };
        // Input only supports mono and stereo
        let configs: Vec<_> = (MIN_CHANNELS..=2)
            .map(|channels| SupportedStreamConfigRange {
                channels,
                min_sample_rate: MIN_SAMPLE_RATE,
                max_sample_rate: MAX_SAMPLE_RATE,
                buffer_size,
                sample_format: SUPPORTED_SAMPLE_FORMAT,
            })
            .collect();
        Ok(configs.into_iter())
    }

    fn supported_output_configs(
        &self,
    ) -> Result<SupportedOutputConfigs, SupportedStreamConfigsError> {
        let buffer_size = SupportedBufferSize::Range {
            min: MIN_BUFFER_SIZE,
            max: MAX_BUFFER_SIZE,
        };
        let configs: Vec<_> = (MIN_CHANNELS..=MAX_CHANNELS)
            .map(|channels| SupportedStreamConfigRange {
                channels,
                min_sample_rate: MIN_SAMPLE_RATE,
                max_sample_rate: MAX_SAMPLE_RATE,
                buffer_size,
                sample_format: SUPPORTED_SAMPLE_FORMAT,
            })
            .collect();
        Ok(configs.into_iter())
    }

    fn default_input_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        const EXPECT: &str = "expected at least one valid webaudio input stream config";
        let config = self
            .supported_input_configs()
            .expect(EXPECT)
            .max_by(|a, b| a.cmp_default_heuristics(b))
            .unwrap()
            .with_sample_rate(DEFAULT_SAMPLE_RATE);
        Ok(config)
    }

    fn default_output_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        const EXPECT: &str = "expected at least one valid webaudio stream config";
        let config = self
            .supported_output_configs()
            .expect(EXPECT)
            .max_by(|a, b| a.cmp_default_heuristics(b))
            .unwrap()
            .with_sample_rate(DEFAULT_SAMPLE_RATE);

        Ok(config)
    }
}

impl DeviceTrait for Device {
    type SupportedInputConfigs = SupportedInputConfigs;
    type SupportedOutputConfigs = SupportedOutputConfigs;
    type Stream = Stream;

    fn description(&self) -> Result<DeviceDescription, DeviceNameError> {
        Device::description(self)
    }

    fn id(&self) -> Result<DeviceId, DeviceIdError> {
        Device::id(self)
    }

    fn supported_input_configs(
        &self,
    ) -> Result<Self::SupportedInputConfigs, SupportedStreamConfigsError> {
        Device::supported_input_configs(self)
    }

    fn supported_output_configs(
        &self,
    ) -> Result<Self::SupportedOutputConfigs, SupportedStreamConfigsError> {
        Device::supported_output_configs(self)
    }

    fn default_input_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        Device::default_input_config(self)
    }

    fn default_output_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        Device::default_output_config(self)
    }

    fn build_input_stream_raw<D, E>(
        &self,
        config: StreamConfig,
        sample_format: SampleFormat,
        data_callback: D,
        error_callback: E,
        _timeout: Option<Duration>,
    ) -> Result<Self::Stream, BuildStreamError>
    where
        D: FnMut(&Data, &InputCallbackInfo) + Send + 'static,
        E: FnMut(StreamError) + Send + 'static,
    {
        if !valid_config(config, sample_format) {
            return Err(BuildStreamError::StreamConfigNotSupported);
        }

        let n_channels = config.channels as usize;

        let buffer_size_frames = match config.buffer_size {
            BufferSize::Fixed(v) => {
                if !(MIN_BUFFER_SIZE..=MAX_BUFFER_SIZE).contains(&v) {
                    return Err(BuildStreamError::StreamConfigNotSupported);
                }
                v as usize
            }
            BufferSize::Default => DEFAULT_BUFFER_SIZE,
        };

        let data_callback = Arc::new(Mutex::new(Box::new(data_callback)));
        let error_callback = Arc::new(Mutex::new(Box::new(error_callback)));

        // Reuse shared AudioContext for input.
        let ctx = get_or_create_audio_context(config.sample_rate)?;

        // Create a ScriptProcessorNode for capturing input samples.
        let script_node = ctx
            .create_script_processor_with_buffer_size_and_number_of_input_channels_and_number_of_output_channels(
                buffer_size_frames as u32,
                n_channels as u32,
                n_channels as u32,
            )
            .map_err(|err| -> BuildStreamError {
                let description = format!("{:?}", err);
                BackendSpecificError { description }.into()
            })?;

        // Set up the onaudioprocess callback.
        let ctx_handle = ctx.clone();
        let mut interleaved = vec![0f32; buffer_size_frames * n_channels];
        let callback_closure = Closure::wrap(Box::new(move |event: web_sys::AudioProcessingEvent| {
            let input_buffer = event.input_buffer().unwrap();

            for channel in 0..n_channels {
                if let Ok(channel_data) = input_buffer.get_channel_data(channel as u32) {
                    for (i, &sample) in channel_data.iter().enumerate() {
                        interleaved[i * n_channels + channel] = sample;
                    }
                }
            }

            let now = ctx_handle.current_time();
            let len = interleaved.len();
            let data = interleaved.as_ptr() as *mut ();
            let data = unsafe { Data::from_parts(data, len, sample_format) };
            let callback = crate::StreamInstant::from_secs_f64(now);
            let capture = callback;
            let timestamp = crate::InputStreamTimestamp { callback, capture };
            let info = InputCallbackInfo { timestamp };
            let mut data_callback = data_callback.lock().unwrap();
            (data_callback.deref_mut())(&data, &info);
        }) as Box<dyn FnMut(web_sys::AudioProcessingEvent)>);

        script_node.set_onaudioprocess(Some(callback_closure.as_ref().unchecked_ref()));

        // Connect script node to destination (required for it to process).
        script_node
            .connect_with_audio_node(&ctx.destination())
            .map_err(|err| -> BuildStreamError {
                let description = format!("{:?}", err);
                BackendSpecificError { description }.into()
            })?;

        // Request microphone access via getUserMedia and connect to script node.
        let script_node_clone = script_node.clone();
        let ctx_clone = ctx.clone();
        let error_callback_clone = error_callback.clone();
        let constraints = web_sys::MediaStreamConstraints::new();
        // Disable Chrome's automatic audio processing that can cause gaps
        let audio_constraints = js_sys::Object::new();
        js_sys::Reflect::set(&audio_constraints, &"echoCancellation".into(), &JsValue::FALSE).unwrap();
        js_sys::Reflect::set(&audio_constraints, &"noiseSuppression".into(), &JsValue::FALSE).unwrap();
        js_sys::Reflect::set(&audio_constraints, &"autoGainControl".into(), &JsValue::FALSE).unwrap();
        // Honor the app's microphone selection (see `crate::web`); `exact`
        // so a stale id errors instead of capturing another device.
        if let Some(id) = crate::web::preferred_input_device_id() {
            let device_id = js_sys::Object::new();
            js_sys::Reflect::set(&device_id, &"exact".into(), &id.into()).unwrap();
            js_sys::Reflect::set(&audio_constraints, &"deviceId".into(), &device_id).unwrap();
        }
        constraints.set_audio(&audio_constraints.into());

        let window = web_sys::window().unwrap();
        let navigator = window.navigator();
        let media_devices = navigator.media_devices().map_err(|err| -> BuildStreamError {
            let description = format!("{:?}", err);
            BackendSpecificError { description }.into()
        })?;

        let promise = media_devices
            .get_user_media_with_constraints(&constraints)
            .map_err(|err| -> BuildStreamError {
                let description = format!("{:?}", err);
                BackendSpecificError { description }.into()
            })?;

        // Handle the getUserMedia promise asynchronously.
        wasm_bindgen_futures::spawn_local(async move {
            match wasm_bindgen_futures::JsFuture::from(promise).await {
                Ok(stream) => {
                    let media_stream: web_sys::MediaStream = stream.unchecked_into();
                    match ctx_clone.create_media_stream_source(&media_stream) {
                        Ok(source) => {
                            if let Err(err) =
                                source.connect_with_audio_node(&script_node_clone)
                            {
                                let description = format!("{:?}", err);
                                let err = BackendSpecificError { description };
                                let mut error_callback = error_callback_clone.lock().unwrap();
                                (error_callback.deref_mut())(err.into());
                            }
                        }
                        Err(err) => {
                            let description = format!("{:?}", err);
                            let err = BackendSpecificError { description };
                            let mut error_callback = error_callback_clone.lock().unwrap();
                            (error_callback.deref_mut())(err.into());
                        }
                    }
                }
                Err(err) => {
                    let description = format!("{:?}", err);
                    let err = BackendSpecificError { description };
                    let mut error_callback = error_callback_clone.lock().unwrap();
                    (error_callback.deref_mut())(err.into());
                }
            }
        });

        Ok(Stream {
            ctx,
            buffer_size_frames,
            _closures: vec![Box::new(callback_closure)],
        })
    }

    /// Create an output stream.
    fn build_output_stream_raw<D, E>(
        &self,
        config: StreamConfig,
        sample_format: SampleFormat,
        data_callback: D,
        _error_callback: E,
        _timeout: Option<Duration>,
    ) -> Result<Self::Stream, BuildStreamError>
    where
        D: FnMut(&mut Data, &OutputCallbackInfo) + Send + 'static,
        E: FnMut(StreamError) + Send + 'static,
    {
        if !valid_config(config, sample_format) {
            return Err(BuildStreamError::StreamConfigNotSupported);
        }

        let n_channels = config.channels as usize;

        let buffer_size_frames = match config.buffer_size {
            BufferSize::Fixed(v) => {
                if !(MIN_BUFFER_SIZE..=MAX_BUFFER_SIZE).contains(&v) {
                    return Err(BuildStreamError::StreamConfigNotSupported);
                }
                v as usize
            }
            BufferSize::Default => DEFAULT_BUFFER_SIZE,
        };

        let data_callback = Arc::new(Mutex::new(Box::new(data_callback)));

        // Reuse shared AudioContext for output.
        let ctx = get_or_create_audio_context(config.sample_rate)?;

        // Create a ScriptProcessorNode for gapless output.
        let script_node = ctx
            .create_script_processor_with_buffer_size_and_number_of_input_channels_and_number_of_output_channels(
                buffer_size_frames as u32,
                1, // Chrome requires at least 1 input channel to fire onaudioprocess
                n_channels as u32,
            )
            .map_err(|err| -> BuildStreamError {
                let description = format!("{:?}", err);
                BackendSpecificError { description }.into()
            })?;

        let ctx_handle = ctx.clone();
        let mut temporary_buffer = vec![0f32; buffer_size_frames * n_channels];
        let mut temporary_channel_buffer = vec![0f32; buffer_size_frames];
        let callback = Closure::wrap(Box::new(move |event: web_sys::AudioProcessingEvent| {
            let output_buffer = event.output_buffer().unwrap();
            let n_frames = output_buffer.length() as usize;

            // Fill the interleaved buffer via data callback.
            {
                let len = temporary_buffer.len();
                let data = temporary_buffer.as_mut_ptr() as *mut ();
                let mut data = unsafe { Data::from_parts(data, len, sample_format) };
                let mut data_callback = data_callback.lock().unwrap();
                let now = ctx_handle.current_time();
                let callback = crate::StreamInstant::from_secs_f64(now);
                let playback = callback;
                let timestamp = crate::OutputStreamTimestamp { callback, playback };
                let info = OutputCallbackInfo { timestamp };
                (data_callback.deref_mut())(&mut data, &info);
            }

            // Deinterleave and copy to output channels.
            for channel in 0..n_channels {
                for i in 0..n_frames {
                    temporary_channel_buffer[i] = temporary_buffer[n_channels * i + channel];
                }
                output_buffer
                    .copy_to_channel(&temporary_channel_buffer, channel as i32)
                    .unwrap();
            }
        }) as Box<dyn FnMut(web_sys::AudioProcessingEvent)>);

        script_node.set_onaudioprocess(Some(callback.as_ref().unchecked_ref()));

        // Connect to destination for output.
        script_node
            .connect_with_audio_node(&ctx.destination())
            .map_err(|err| -> BuildStreamError {
                let description = format!("{:?}", err);
                BackendSpecificError { description }.into()
            })?;

        Ok(Stream {
            ctx,
            buffer_size_frames,
            _closures: vec![Box::new(callback)],
        })
    }
}


impl Stream {
    /// Return the [`AudioContext`](https://developer.mozilla.org/docs/Web/API/AudioContext) used
    /// by this stream.
    pub fn audio_context(&self) -> &AudioContext {
        &self.ctx
    }
}

impl StreamTrait for Stream {
    fn play(&self) -> Result<(), PlayStreamError> {
        match self.ctx.resume() {
            Ok(_) => Ok(()),
            Err(err) => {
                let description = format!("{:?}", err);
                let err = BackendSpecificError { description };
                Err(err.into())
            }
        }
    }

    fn pause(&self) -> Result<(), PauseStreamError> {
        match self.ctx.suspend() {
            Ok(_) => Ok(()),
            Err(err) => {
                let description = format!("{:?}", err);
                let err = BackendSpecificError { description };
                Err(err.into())
            }
        }
    }

    fn buffer_size(&self) -> Option<crate::FrameCount> {
        Some(self.buffer_size_frames as crate::FrameCount)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // Don't close the shared AudioContext — other streams may still use it.
    }
}

impl Default for Devices {
    fn default() -> Devices {
        // We produce an empty iterator if the WebAudio API isn't available.
        Devices(is_webaudio_available())
    }
}

impl Iterator for Devices {
    type Item = Device;

    #[inline]
    fn next(&mut self) -> Option<Device> {
        if self.0 {
            self.0 = false;
            Some(Device)
        } else {
            None
        }
    }
}

fn default_input_device() -> Option<Device> {
    if is_webaudio_available() {
        Some(Device)
    } else {
        None
    }
}

fn default_output_device() -> Option<Device> {
    if is_webaudio_available() {
        Some(Device)
    } else {
        None
    }
}

// Detects whether the `AudioContext` global variable is available.
fn is_webaudio_available() -> bool {
    js_sys::Reflect::get(&js_sys::global(), &JsValue::from("AudioContext"))
        .unwrap()
        .is_truthy()
}

// Whether or not the given stream configuration is valid for building a stream.
fn valid_config(conf: StreamConfig, sample_format: SampleFormat) -> bool {
    conf.channels <= MAX_CHANNELS
        && conf.channels >= MIN_CHANNELS
        && conf.sample_rate <= MAX_SAMPLE_RATE
        && conf.sample_rate >= MIN_SAMPLE_RATE
        && sample_format == SUPPORTED_SAMPLE_FORMAT
}


#[cfg(target_feature = "atomics")]
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = AudioBuffer)]
    type ExternalArrayAudioBuffer;

    # [wasm_bindgen(catch, method, structural, js_class = "AudioBuffer", js_name = copyToChannel)]
    pub fn copy_to_channel(
        this: &ExternalArrayAudioBuffer,
        source: &js_sys::Float32Array,
        channel_number: i32,
    ) -> Result<(), JsValue>;
}
