//! Hybrid Logical Clock (HLC)。
//!
//! 各デバイスが保持し、書込のたびにインクリメントする（`docs/03_技術仕様書.md` §6.3）。
//! 比較順序は `wall_ms` -> `counter` -> `device_id` の辞書順。

use serde::{Deserialize, Serialize};
use thiserror::Error;

const ENCODED_PREFIX: &str = "01";
const BIASED_WALL_WIDTH: usize = 20;
const COUNTER_WIDTH: usize = 10;
const DEVICE_ID_MAX_BYTES: usize = 64;
const OBSERVATION_COUNTER_HEADROOM: u32 = 2;
const NETWORK_COUNTER_MAX: u32 = u32::MAX - OBSERVATION_COUNTER_HEADROOM;

/// HLC encode/decode error.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum HlcError {
    #[error("HLC device id must be 1..=64 printable ASCII bytes")]
    InvalidDeviceId,
    #[error("encoded HLC has invalid length")]
    InvalidLength,
    #[error("encoded HLC has unsupported version prefix")]
    UnsupportedVersion,
    #[error("encoded HLC contains invalid digits")]
    InvalidDigits,
    #[error("HLC logical counter exhausted")]
    CounterExhausted,
    #[error("HLC does not reserve observation counter headroom")]
    InsufficientCounterHeadroom,
}

/// Hybrid Logical Clockの値。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct Hlc {
    /// 物理時刻由来のミリ秒 (UTC epoch millis)。
    pub wall_ms: i64,
    /// 同一 `wall_ms` 内での論理カウンタ。
    pub counter: u32,
    /// 発行元デバイスID（タイブレーク用）。
    pub device_id: String,
}

impl Hlc {
    /// 新しいHLCを初期値 (`wall_ms: 0, counter: 0`) で生成する。
    pub fn new(device_id: impl Into<String>) -> Self {
        Self {
            wall_ms: 0,
            counter: 0,
            device_id: device_id.into(),
        }
    }

    /// 物理時刻 `physical_ms` に基づき時計を進め、新しいHLC値を返す。
    ///
    /// - 物理時刻が現在保持している `wall_ms` より進んでいれば `wall_ms` を更新し `counter` を0にリセットする。
    /// - 物理時刻が同一（または後退）していれば `wall_ms` は維持し `counter` を1つ進める。
    pub fn now(&mut self, physical_ms: i64) -> Result<Hlc, HlcError> {
        let (wall_ms, counter) = if physical_ms > self.wall_ms {
            (physical_ms, 0)
        } else {
            increment_or_rollover(self.wall_ms, self.counter)?
        };
        self.wall_ms = wall_ms;
        self.counter = counter;
        Ok(self.clone())
    }

    /// 受信したHLCと物理時刻に基づきローカル時計を進め、新しいローカルHLCを返す。
    pub fn merge(&mut self, remote: &Hlc, physical_ms: i64) -> Result<Hlc, HlcError> {
        let max_wall_ms = physical_ms.max(self.wall_ms).max(remote.wall_ms);
        let base_counter = if max_wall_ms == self.wall_ms && max_wall_ms == remote.wall_ms {
            self.counter.max(remote.counter)
        } else if max_wall_ms == self.wall_ms {
            self.counter
        } else if max_wall_ms == remote.wall_ms {
            remote.counter
        } else {
            0
        };
        let (wall_ms, counter) = if max_wall_ms == self.wall_ms || max_wall_ms == remote.wall_ms {
            increment_or_rollover(max_wall_ms, base_counter)?
        } else {
            (max_wall_ms, base_counter)
        };
        self.wall_ms = wall_ms;
        self.counter = counter;
        Ok(self.clone())
    }

    /// 固定幅・文字列順ソート可能なHLC表現へエンコードする。
    ///
    /// 形式は `01 || biased_wall_ms(20 decimal) || counter(10 decimal)
    /// || device_id_utf8_bytes(64 bytes, NUL padded, hex)`。
    pub fn encode(&self) -> Result<String, HlcError> {
        validate_device_id(&self.device_id)?;
        let biased_wall_ms = biased_wall_ms(self.wall_ms);
        let mut device_bytes = [0u8; DEVICE_ID_MAX_BYTES];
        let raw_device_id = self.device_id.as_bytes();
        device_bytes[..raw_device_id.len()].copy_from_slice(raw_device_id);

        Ok(format!(
            "{ENCODED_PREFIX}{biased_wall_ms:0BIASED_WALL_WIDTH$}{counter:0COUNTER_WIDTH$}{device_hex}",
            counter = self.counter,
            device_hex = encode_hex(&device_bytes),
        ))
    }

    /// [`Hlc::encode`] で生成された固定幅文字列からHLCを復元する。
    pub fn decode(encoded: &str) -> Result<Self, HlcError> {
        let wire = taskveil_protocol::WireHlc::decode(encoded).map_err(|error| match error {
            taskveil_protocol::HlcWireError::InvalidDeviceId => HlcError::InvalidDeviceId,
            taskveil_protocol::HlcWireError::InvalidLength => HlcError::InvalidLength,
            taskveil_protocol::HlcWireError::UnsupportedVersion => HlcError::UnsupportedVersion,
            taskveil_protocol::HlcWireError::InvalidDigits => HlcError::InvalidDigits,
            taskveil_protocol::HlcWireError::InsufficientCounterHeadroom => {
                HlcError::InsufficientCounterHeadroom
            }
        })?;
        Ok(Self {
            wall_ms: wire.wall_ms,
            counter: wire.counter,
            device_id: wire.device_id,
        })
    }

    /// network境界でobserve可能なHLCを復元する。
    ///
    /// observe自身のincrementと、その直後のhonest local tickのために2 counterを予約する。
    pub fn decode_observable(encoded: &str) -> Result<Self, HlcError> {
        let hlc = Self::decode(encoded)?;
        if !hlc.has_observation_headroom() {
            return Err(HlcError::InsufficientCounterHeadroom);
        }
        Ok(hlc)
    }

    /// observe 1回と、その直後のlocal tick 1回を安全に表現できるかを返す。
    pub fn has_observation_headroom(&self) -> bool {
        self.counter <= NETWORK_COUNTER_MAX
    }

    /// `server_now_ms + allowed_future_skew_ms` より未来のHLCかを判定する。
    pub fn exceeds_future_skew(&self, server_now_ms: i64, allowed_future_skew_ms: i64) -> bool {
        self.wall_ms > server_now_ms.saturating_add(allowed_future_skew_ms)
    }

    /// future skewのstrict boundaryへ到達したか、超えたかを判定する。
    ///
    /// boundaryちょうどもcounterに関係なく一時拒否する。boundary直前のhigh
    /// counterをobserveして1ms carryしたhonest mutationは、server clockが1ms
    /// 進んだretryでstrict boundaryの内側へ戻る。
    pub fn reaches_future_skew_boundary(
        &self,
        server_now_ms: i64,
        allowed_future_skew_ms: i64,
    ) -> bool {
        self.wall_ms >= server_now_ms.saturating_add(allowed_future_skew_ms)
    }
}

fn increment_or_rollover(wall_ms: i64, counter: u32) -> Result<(i64, u32), HlcError> {
    if counter < NETWORK_COUNTER_MAX {
        return Ok((wall_ms, counter + 1));
    }
    Ok((wall_ms.checked_add(1).ok_or(HlcError::CounterExhausted)?, 0))
}

fn validate_device_id(device_id: &str) -> Result<(), HlcError> {
    if device_id.is_empty()
        || device_id.len() > DEVICE_ID_MAX_BYTES
        || !device_id.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(HlcError::InvalidDeviceId);
    }
    Ok(())
}

fn biased_wall_ms(wall_ms: i64) -> u64 {
    (wall_ms as i128 - i64::MIN as i128) as u64
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

impl PartialOrd for Hlc {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Hlc {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.wall_ms
            .cmp(&other.wall_ms)
            .then_with(|| self.counter.cmp(&other.counter))
            .then_with(|| self.device_id.cmp(&other.device_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_advances_wall_ms_and_resets_counter_when_physical_time_moves_forward() {
        let mut clock = Hlc::new("device-a");
        let first = clock.now(1_000).unwrap();
        assert_eq!(first.wall_ms, 1_000);
        assert_eq!(first.counter, 0);

        let second = clock.now(2_000).unwrap();
        assert_eq!(second.wall_ms, 2_000);
        assert_eq!(second.counter, 0);
    }

    #[test]
    fn now_increments_counter_when_physical_time_does_not_advance() {
        let mut clock = Hlc::new("device-a");
        let first = clock.now(1_000).unwrap();
        let second = clock.now(1_000).unwrap();
        let third = clock.now(500).unwrap(); // 物理時刻の後退（クロックスキュー）も許容

        assert_eq!(first.counter, 0);
        assert_eq!(second.counter, 1);
        assert_eq!(third.counter, 2);
        assert_eq!(second.wall_ms, 1_000);
        assert_eq!(third.wall_ms, 1_000);
    }

    #[test]
    fn successive_values_are_monotonically_increasing() {
        let mut clock = Hlc::new("device-a");
        let mut previous = clock.now(1_000).unwrap();
        for physical_ms in [1_000, 1_000, 1_500, 1_500, 1_500, 2_000] {
            let next = clock.now(physical_ms).unwrap();
            assert!(
                next > previous,
                "{next:?} should be greater than {previous:?}"
            );
            previous = next;
        }
    }

    #[test]
    fn ordering_breaks_ties_by_device_id() {
        let a = Hlc {
            wall_ms: 100,
            counter: 1,
            device_id: "device-a".to_string(),
        };
        let b = Hlc {
            wall_ms: 100,
            counter: 1,
            device_id: "device-b".to_string(),
        };
        assert!(a < b);
    }

    #[test]
    fn merge_advances_past_remote_clock() {
        let mut clock = Hlc {
            wall_ms: 1_000,
            counter: 2,
            device_id: "device-a".to_string(),
        };
        let remote = Hlc {
            wall_ms: 1_500,
            counter: 4,
            device_id: "device-b".to_string(),
        };

        let merged = clock.merge(&remote, 1_200).unwrap();

        assert_eq!(merged.wall_ms, 1_500);
        assert_eq!(merged.counter, 5);
        assert_eq!(merged.device_id, "device-a");
    }

    #[test]
    fn merge_handles_equal_wall_times_deterministically() {
        let mut clock = Hlc {
            wall_ms: 1_000,
            counter: 2,
            device_id: "device-a".to_string(),
        };
        let remote = Hlc {
            wall_ms: 1_000,
            counter: 4,
            device_id: "device-b".to_string(),
        };

        let merged = clock.merge(&remote, 1_000).unwrap();

        assert_eq!(merged.wall_ms, 1_000);
        assert_eq!(merged.counter, 5);
    }

    #[test]
    fn encode_decode_roundtrips() {
        let hlc = Hlc {
            wall_ms: 1_799_000_000_000,
            counter: 42,
            device_id: "device-a".to_string(),
        };

        assert_eq!(Hlc::decode(&hlc.encode().unwrap()).unwrap(), hlc);
    }

    #[test]
    fn encoded_string_order_matches_hlc_order() {
        let mut values = vec![
            Hlc {
                wall_ms: 1,
                counter: 0,
                device_id: "device-b".to_string(),
            },
            Hlc {
                wall_ms: 0,
                counter: 9,
                device_id: "device-z".to_string(),
            },
            Hlc {
                wall_ms: 1,
                counter: 0,
                device_id: "device-a".to_string(),
            },
            Hlc {
                wall_ms: -1,
                counter: u32::MAX,
                device_id: "device-z".to_string(),
            },
        ];
        let mut encoded = values
            .iter()
            .map(|hlc| hlc.encode().unwrap())
            .collect::<Vec<_>>();

        values.sort();
        encoded.sort();

        let decoded = encoded
            .iter()
            .map(|value| Hlc::decode(value).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(decoded, values);
    }

    #[test]
    fn encoded_values_are_fixed_width() {
        let short = Hlc {
            wall_ms: 1,
            counter: 1,
            device_id: "a".to_string(),
        };
        let long = Hlc {
            wall_ms: i64::MAX,
            counter: u32::MAX,
            device_id: "device-abcdefghijklmnopqrstuvwxyz-0123456789".to_string(),
        };

        assert_eq!(short.encode().unwrap().len(), long.encode().unwrap().len());
    }

    #[test]
    fn future_skew_detection_uses_physical_component_only() {
        let hlc = Hlc {
            wall_ms: 1_000,
            counter: 0,
            device_id: "device-a".to_string(),
        };

        assert!(!hlc.exceeds_future_skew(500, 500));
        assert!(hlc.exceeds_future_skew(499, 500));
    }

    #[test]
    fn strict_future_boundary_retries_after_one_ms_then_closes_under_multiple_relays() {
        let boundary_minus_one = Hlc {
            wall_ms: 999,
            counter: NETWORK_COUNTER_MAX,
            device_id: "remote-device".to_string(),
        };

        assert!(!boundary_minus_one.reaches_future_skew_boundary(500, 500));

        let mut first_client = Hlc::new("first-client");
        let observed = first_client.merge(&boundary_minus_one, 500).unwrap();
        assert_eq!(observed.wall_ms, 1_000);
        assert_eq!(observed.counter, 0);
        let mutation = first_client.now(500).unwrap();
        assert_eq!(mutation.wall_ms, 1_000);
        assert_eq!(mutation.counter, 1);
        assert!(mutation.reaches_future_skew_boundary(500, 500));
        assert!(!mutation.reaches_future_skew_boundary(501, 500));

        let mut second_client = Hlc::new("second-client");
        let second_observation = second_client.merge(&mutation, 501).unwrap();
        let second_mutation = second_client.now(501).unwrap();
        assert_eq!(
            (second_observation.wall_ms, second_observation.counter),
            (1_000, 2)
        );
        assert_eq!(
            (second_mutation.wall_ms, second_mutation.counter),
            (1_000, 3)
        );
        assert!(!second_mutation.reaches_future_skew_boundary(501, 500));

        let mut third_client = Hlc::new("third-client");
        let third_observation = third_client.merge(&second_mutation, 501).unwrap();
        let third_mutation = third_client.now(501).unwrap();
        assert_eq!(
            (third_observation.wall_ms, third_observation.counter),
            (1_000, 4)
        );
        assert_eq!((third_mutation.wall_ms, third_mutation.counter), (1_000, 5));
        assert!(!third_mutation.reaches_future_skew_boundary(501, 500));
    }

    #[test]
    fn strict_future_boundary_rejects_every_counter_at_boundary_and_future_attackers() {
        for (wall_ms, counter) in [
            (1_000, 0),
            (1_000, NETWORK_COUNTER_MAX),
            (1_001, 0),
            (1_002, 0),
        ] {
            assert!(Hlc {
                wall_ms,
                counter,
                device_id: "attacker".to_string(),
            }
            .reaches_future_skew_boundary(500, 500));
        }
    }

    #[test]
    fn now_returns_typed_error_without_mutating_exhausted_clock() {
        let mut clock = Hlc {
            wall_ms: i64::MAX,
            counter: u32::MAX,
            device_id: "device-a".to_string(),
        };
        let original = clock.clone();

        assert_eq!(clock.now(i64::MAX), Err(HlcError::CounterExhausted));
        assert_eq!(clock, original);
    }

    #[test]
    fn merge_returns_typed_error_without_mutating_when_wall_cannot_roll_over() {
        let mut clock = Hlc {
            wall_ms: 1_000,
            counter: 0,
            device_id: "device-a".to_string(),
        };
        let original = clock.clone();
        let remote = Hlc {
            wall_ms: i64::MAX,
            counter: u32::MAX,
            device_id: "device-b".to_string(),
        };

        assert_eq!(clock.merge(&remote, 1_500), Err(HlcError::CounterExhausted));
        assert_eq!(clock, original);
    }

    #[test]
    fn non_observable_counter_rolls_over_without_panicking() {
        let mut clock = Hlc::new("device-a");
        let remote = Hlc {
            wall_ms: 2_000,
            counter: u32::MAX - 1,
            device_id: "device-b".to_string(),
        };

        let merged = clock.merge(&remote, 1_500).unwrap();

        assert_eq!(merged.wall_ms, 2_001);
        assert_eq!(merged.counter, 0);
        assert_eq!(clock.now(2_000).unwrap().counter, 1);
    }

    #[test]
    fn observable_decode_reserves_merge_and_honest_tick_headroom() {
        for counter in [u32::MAX, u32::MAX - 1] {
            let encoded = Hlc {
                wall_ms: 1_000,
                counter,
                device_id: "device-a".to_string(),
            }
            .encode()
            .unwrap();
            assert_eq!(
                Hlc::decode_observable(&encoded),
                Err(HlcError::InsufficientCounterHeadroom)
            );
        }

        let boundary = Hlc {
            wall_ms: 1_000,
            counter: u32::MAX - 2,
            device_id: "device-a".to_string(),
        };
        assert_eq!(
            Hlc::decode_observable(&boundary.encode().unwrap()).unwrap(),
            boundary
        );
    }

    #[test]
    fn boundary_counter_survives_pull_mutation_push_and_second_observation() {
        let remote = Hlc {
            wall_ms: 2_000,
            counter: NETWORK_COUNTER_MAX,
            device_id: "device-a".to_string(),
        };
        let mut first_client = Hlc::new("device-b");

        let observed = first_client.merge(&remote, 1_500).unwrap();
        assert_eq!((observed.wall_ms, observed.counter), (2_001, 0));
        let outbound = first_client.now(1_500).unwrap();
        assert!(outbound > remote);
        let outbound = Hlc::decode_observable(&outbound.encode().unwrap()).unwrap();

        let mut second_client = Hlc::new("device-c");
        let second_observation = second_client.merge(&outbound, 1_500).unwrap();
        let second_outbound = second_client.now(1_500).unwrap();

        assert!(second_observation > outbound);
        assert!(second_outbound > second_observation);
        assert!(Hlc::decode_observable(&second_outbound.encode().unwrap()).is_ok());
    }
}
