//! Versioned app/driver identifiers. Never reuse the production UID in a test driver.
#![forbid(unsafe_code)]

pub const PROTOCOL_VERSION: u32 = 1;
pub const TEST_DEVICE_UID: &str = "ChannelBridge_Rust_Test_UID";
pub const TEST_DEVICE_NAME: &str = "ChannelBridge Mic (Rust Test)";
pub const TEST_DRIVER_BUNDLE_ID: &str = "local.channelbridge.rust.driver.test";
pub const PRODUCTION_DEVICE_UID: &str = "ChannelBridge_UID";
pub const CHANNELS: u32 = 2;
pub const SUPPORTED_RATES: &[u32] = &[
    8_000, 16_000, 24_000, 32_000, 44_100, 48_000, 64_000, 88_200, 96_000, 128_000, 176_400,
    192_000, 352_800, 384_000, 705_600, 768_000,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DriverCounters {
    pub generation: u64,
    pub written_frames: u64,
    pub missing_read_frames: u64,
    pub rejected_write_frames: u64,
    pub invalid_timestamps: u64,
}
