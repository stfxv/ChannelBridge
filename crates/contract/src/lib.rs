//! Versioned app/driver identifiers. Never reuse the production UID in a test driver.
#![forbid(unsafe_code)]

pub const PROTOCOL_VERSION: u32 = 1;
pub const DIAGNOSTICS_SELECTOR: u32 = u32::from_be_bytes(*b"cbst");
/// Raw, non-NUL ASCII Cargo package version of the live driver (1..64 bytes).
pub const DRIVER_IDENTITY_SELECTOR: u32 = u32::from_be_bytes(*b"cbid");
pub const DRIVER_IDENTITY_MAX_BYTES: usize = 64;
pub const DIAGNOSTICS_BYTES: usize = 48;
pub const TEST_DEVICE_UID: &str = "ChannelBridge_Rust_Test_UID";
pub const TEST_DEVICE_NAME: &str = "ChannelBridge Mic (Rust Test)";
pub const TEST_DRIVER_BUNDLE_ID: &str = "local.channelbridge.rust.driver.test";
pub const PRODUCTION_DEVICE_UID: &str = "ChannelBridge_UID";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deployment {
    pub device_uid: &'static str,
    pub device_name: &'static str,
    pub driver_bundle_id: &'static str,
    pub app_bundle_id: &'static str,
    pub app_name: &'static str,
    pub settings_directory: &'static str,
}
pub const TEST_DEPLOYMENT: Deployment = Deployment {
    device_uid: TEST_DEVICE_UID,
    device_name: TEST_DEVICE_NAME,
    driver_bundle_id: TEST_DRIVER_BUNDLE_ID,
    app_bundle_id: "local.channelbridge.rust.app.test",
    app_name: "ChannelBridge Rust Test",
    settings_directory: "ChannelBridge Rust Test",
};
pub const PRODUCTION_DEPLOYMENT: Deployment = Deployment {
    device_uid: PRODUCTION_DEVICE_UID,
    device_name: "ChannelBridge Mic",
    driver_bundle_id: "local.channelbridge.driver",
    app_bundle_id: "local.channelbridge.app",
    app_name: "ChannelBridge",
    settings_directory: "ChannelBridge",
};
#[cfg(not(feature = "production"))]
pub const DEPLOYMENT: Deployment = TEST_DEPLOYMENT;
#[cfg(feature = "production")]
pub const DEPLOYMENT: Deployment = PRODUCTION_DEPLOYMENT;
pub const CHANNELS: u32 = 2;
#[cfg(test)]
mod deployment_tests {
    use super::*;
    #[test]
    fn namespaces_are_distinct_and_production_preserves_existing_identity() {
        assert_ne!(TEST_DEPLOYMENT.device_uid, PRODUCTION_DEPLOYMENT.device_uid);
        assert_ne!(
            TEST_DEPLOYMENT.driver_bundle_id,
            PRODUCTION_DEPLOYMENT.driver_bundle_id
        );
        assert_ne!(
            TEST_DEPLOYMENT.app_bundle_id,
            PRODUCTION_DEPLOYMENT.app_bundle_id
        );
        assert_ne!(
            TEST_DEPLOYMENT.settings_directory,
            PRODUCTION_DEPLOYMENT.settings_directory
        );
        assert_eq!(PRODUCTION_DEPLOYMENT.device_uid, "ChannelBridge_UID");
        assert_eq!(
            PRODUCTION_DEPLOYMENT.driver_bundle_id,
            "local.channelbridge.driver"
        );
        assert_eq!(
            PRODUCTION_DEPLOYMENT.app_bundle_id,
            "local.channelbridge.app"
        );
        #[cfg(feature = "production")]
        assert_eq!(DEPLOYMENT, PRODUCTION_DEPLOYMENT);
        #[cfg(not(feature = "production"))]
        assert_eq!(DEPLOYMENT, TEST_DEPLOYMENT);
    }
}
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
