//! Versioned app/driver identifiers. Never reuse the production UID in a test driver.
#![forbid(unsafe_code)]

pub const PROTOCOL_VERSION: u32 = 1;
pub const DIAGNOSTICS_SELECTOR: u32 = u32::from_be_bytes(*b"cbst");
pub const DIAGNOSTICS_BYTES: usize = 48;
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

#[derive(Debug, PartialEq, Eq)]
pub enum DiagnosticError {
    Size(usize),
    Version(u32),
    Reserved,
}
impl std::fmt::Display for DiagnosticError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Size(size) => write!(
                formatter,
                "expected {DIAGNOSTICS_BYTES} diagnostic bytes, got {size}"
            ),
            Self::Version(version) => write!(
                formatter,
                "unsupported driver diagnostic protocol {version}"
            ),
            Self::Reserved => write!(formatter, "nonzero reserved diagnostic field"),
        }
    }
}
impl std::error::Error for DiagnosticError {}
impl DriverCounters {
    /// Decode the HAL's native-endian protocol v1 bytes, never Rust struct layout.
    /// Counters are independently sampled cumulative totals, not an atomic epoch.
    pub fn decode(bytes: &[u8]) -> Result<Self, DiagnosticError> {
        if bytes.len() != DIAGNOSTICS_BYTES {
            return Err(DiagnosticError::Size(bytes.len()));
        }
        let version = u32::from_ne_bytes(bytes[..4].try_into().expect("checked length"));
        if version != PROTOCOL_VERSION {
            return Err(DiagnosticError::Version(version));
        }
        if bytes[4..8] != [0; 4] {
            return Err(DiagnosticError::Reserved);
        }
        let value = |offset| {
            u64::from_ne_bytes(
                bytes[offset..offset + 8]
                    .try_into()
                    .expect("checked field size"),
            )
        };
        Ok(Self {
            generation: value(8),
            written_frames: value(16),
            missing_read_frames: value(24),
            rejected_write_frames: value(32),
            invalid_timestamps: value(40),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_decode_preserves_large_counts_and_rejects_incompatible_messages() {
        let mut bytes = vec![0_u8; 1]; // Intentionally unaligned message origin.
        bytes.extend(1_u32.to_ne_bytes());
        bytes.extend(0_u32.to_ne_bytes());
        for value in [7_u64, (1 << 54) + 3, 23, 29, 31] {
            bytes.extend(value.to_ne_bytes());
        }
        let fixture = &mut bytes[1..];
        assert_eq!(
            DriverCounters::decode(fixture).unwrap(),
            DriverCounters {
                generation: 7,
                written_frames: (1 << 54) + 3,
                missing_read_frames: 23,
                rejected_write_frames: 29,
                invalid_timestamps: 31
            }
        );
        assert_eq!(
            DriverCounters::decode(&fixture[..47]),
            Err(DiagnosticError::Size(47))
        );
        fixture[..4].copy_from_slice(&2_u32.to_ne_bytes());
        assert_eq!(
            DriverCounters::decode(fixture),
            Err(DiagnosticError::Version(2))
        );
        fixture[..4].copy_from_slice(&1_u32.to_ne_bytes());
        fixture[4] = 1;
        assert_eq!(
            DriverCounters::decode(fixture),
            Err(DiagnosticError::Reserved)
        );
        assert_eq!(
            DriverCounters::decode(&[0; 49]),
            Err(DiagnosticError::Size(49))
        );
    }
}
