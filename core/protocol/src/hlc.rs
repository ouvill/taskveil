//! Canonical validation for the fixed-width HLC wire value.

use thiserror::Error;

const ENCODED_PREFIX: &str = "01";
const BIASED_WALL_WIDTH: usize = 20;
const COUNTER_WIDTH: usize = 10;
const DEVICE_ID_MAX_BYTES: usize = 64;
const DEVICE_HEX_WIDTH: usize = DEVICE_ID_MAX_BYTES * 2;
const ENCODED_LEN: usize =
    ENCODED_PREFIX.len() + BIASED_WALL_WIDTH + COUNTER_WIDTH + DEVICE_HEX_WIDTH;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum HlcWireError {
    #[error("HLC device id must be 1..=64 printable ASCII bytes")]
    InvalidDeviceId,
    #[error("encoded HLC has invalid length")]
    InvalidLength,
    #[error("encoded HLC has unsupported version prefix")]
    UnsupportedVersion,
    #[error("encoded HLC contains invalid digits")]
    InvalidDigits,
}

/// Validated fields from the canonical fixed-width HLC wire representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireHlc {
    pub wall_ms: i64,
    pub counter: u32,
    pub device_id: String,
}

impl WireHlc {
    pub fn decode(encoded: &str) -> Result<Self, HlcWireError> {
        if encoded.len() != ENCODED_LEN {
            return Err(HlcWireError::InvalidLength);
        }
        if !encoded.starts_with(ENCODED_PREFIX) {
            return Err(HlcWireError::UnsupportedVersion);
        }

        let wall_start = ENCODED_PREFIX.len();
        let counter_start = wall_start + BIASED_WALL_WIDTH;
        let device_start = counter_start + COUNTER_WIDTH;
        let biased_wall = encoded[wall_start..counter_start]
            .parse::<u64>()
            .map_err(|_| HlcWireError::InvalidDigits)?;
        let counter = encoded[counter_start..device_start]
            .parse::<u32>()
            .map_err(|_| HlcWireError::InvalidDigits)?;
        let device_bytes = decode_hex(&encoded[device_start..])?;
        let unpadded_len = device_bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(device_bytes.len());
        if device_bytes[unpadded_len..].iter().any(|byte| *byte != 0) {
            return Err(HlcWireError::InvalidDeviceId);
        }
        let device_id = String::from_utf8(device_bytes[..unpadded_len].to_vec())
            .map_err(|_| HlcWireError::InvalidDeviceId)?;
        if device_id.is_empty()
            || device_id.len() > DEVICE_ID_MAX_BYTES
            || !device_id.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(HlcWireError::InvalidDeviceId);
        }

        Ok(Self {
            wall_ms: (biased_wall as i128 + i64::MIN as i128) as i64,
            counter,
            device_id,
        })
    }
}

fn decode_hex(encoded: &str) -> Result<Vec<u8>, HlcWireError> {
    if !encoded.len().is_multiple_of(2) {
        return Err(HlcWireError::InvalidDigits);
    }
    encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_digit(pair[0])?;
            let low = hex_digit(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_digit(byte: u8) -> Result<u8, HlcWireError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(HlcWireError::InvalidDigits),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_hlc_wire_value_decodes() {
        let device_hex = format!("{:0<128}", "6465766963652d61");
        let encoded = format!(
            "01{:020}{:010}{}",
            (42_i128 - i64::MIN as i128) as u64,
            7,
            device_hex,
        );
        let decoded = WireHlc::decode(&encoded).unwrap();
        assert_eq!(decoded.wall_ms, 42);
        assert_eq!(decoded.counter, 7);
        assert_eq!(decoded.device_id, "device-a");
    }

    #[test]
    fn non_zero_bytes_after_device_padding_are_rejected() {
        let mut device_hex = format!("{:0<128}", "6465766963652d61");
        device_hex.replace_range(126..128, "62");
        let encoded = format!(
            "01{:020}{:010}{}",
            (42_i128 - i64::MIN as i128) as u64,
            7,
            device_hex,
        );

        assert_eq!(
            WireHlc::decode(&encoded),
            Err(HlcWireError::InvalidDeviceId)
        );
    }
}
