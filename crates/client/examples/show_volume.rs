use cpal::{
    BufferSize, Device, Host, StreamConfig, SupportedStreamConfig, default_host,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use std::io::Write;
fn main() {
    let host = default_host();
    let input_device = host
        .default_input_device()
        .expect("no input device available");
    let name = input_device.name().expect("no name");
    let default_input_config = input_device
        .default_input_config()
        .expect("no default input config");

    let input_config = StreamConfig {
        channels: default_input_config.channels(),
        sample_rate: default_input_config.sample_rate(),
        buffer_size: BufferSize::Default,
    };
    let input_stream = input_device
        .build_input_stream::<f32, _, _>(
            &input_config,
            move |data, info| {
                // 计算 RMS
                let rms = (data.iter().map(|&sample| sample * sample).sum::<f32>()
                    / data.len() as f32)
                    .sqrt();
                // 将 RMS 转换为分贝（dB）
                let db = 20.0 * rms.log10();
                // 将分贝值映射到 0 到 50 的范围，用于显示音量条
                let max_db = 0.0;    // 参考最大分贝值，0 dB
                let min_db = -50.0;  // 参考最小分贝值，-50 dB
                let level = ((db - min_db) / (max_db - min_db) * 50.0).round() as usize;
                let level = level.clamp(0, 50);  // 确保 level 在 0 到 50 之间
                // 生成音量条字符串
                let bar = "█".repeat(level);
                print!("\r音量：[{:<50}] {:.2} dB", bar, db);
            },
            move |err| {
                eprintln!("an error occurred on stream: {}", err);
            },
            None,
        )
        .expect("failed to build input stream");

    input_stream.play().expect("failed to play input stream");

    std::thread::sleep(std::time::Duration::from_secs(10));
}
