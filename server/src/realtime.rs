use std::{collections::HashMap, env, ffi::OsString, sync::Arc, time::Duration as StdDuration};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use reqwest::{header::CONTENT_LENGTH, redirect::Policy, Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;
use uuid::Uuid;

const AUDIENCE: &str = "taskveil-realtime";
const PUBLISH_BODY_LIMIT: usize = 512;
const PUBLISH_TIMEOUT: StdDuration = StdDuration::from_millis(500);
const TICKET_TTL_SECONDS: i64 = 300;

const WEBSOCKET_URL: &str = "TASKVEIL_REALTIME_WEBSOCKET_URL";
const PUBLISH_URL: &str = "TASKVEIL_REALTIME_PUBLISH_URL";
const CHANNEL_KEY: &str = "TASKVEIL_REALTIME_CHANNEL_KEY";
const TICKET_KEY_CURRENT_ID: &str = "TASKVEIL_REALTIME_TICKET_KEY_CURRENT_ID";
const TICKET_KEY_CURRENT: &str = "TASKVEIL_REALTIME_TICKET_KEY_CURRENT";
const TICKET_KEY_PREVIOUS_ID: &str = "TASKVEIL_REALTIME_TICKET_KEY_PREVIOUS_ID";
const TICKET_KEY_PREVIOUS: &str = "TASKVEIL_REALTIME_TICKET_KEY_PREVIOUS";
const PUBLISH_KEY_CURRENT_ID: &str = "TASKVEIL_REALTIME_PUBLISH_KEY_CURRENT_ID";
const PUBLISH_KEY_CURRENT: &str = "TASKVEIL_REALTIME_PUBLISH_KEY_CURRENT";
const PUBLISH_KEY_PREVIOUS_ID: &str = "TASKVEIL_REALTIME_PUBLISH_KEY_PREVIOUS_ID";
const PUBLISH_KEY_PREVIOUS: &str = "TASKVEIL_REALTIME_PUBLISH_KEY_PREVIOUS";

const CONFIG_VARIABLES: [&str; 11] = [
    WEBSOCKET_URL,
    PUBLISH_URL,
    CHANNEL_KEY,
    TICKET_KEY_CURRENT_ID,
    TICKET_KEY_CURRENT,
    TICKET_KEY_PREVIOUS_ID,
    TICKET_KEY_PREVIOUS,
    PUBLISH_KEY_CURRENT_ID,
    PUBLISH_KEY_CURRENT,
    PUBLISH_KEY_PREVIOUS_ID,
    PUBLISH_KEY_PREVIOUS,
];

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Default)]
pub struct RealtimeGateway {
    enabled: Option<Arc<EnabledGateway>>,
}

struct EnabledGateway {
    websocket_url: String,
    publish_url: Url,
    channel_key: [u8; 32],
    ticket_current: SigningKey,
    publish_current: SigningKey,
    client: Client,
}

struct SigningKey {
    id: String,
    value: [u8; 32],
}

pub struct RealtimeSettings {
    pub websocket_url: String,
    pub publish_url: String,
    pub channel_key: String,
    pub ticket_key_current_id: String,
    pub ticket_key_current: String,
    pub ticket_key_previous_id: String,
    pub ticket_key_previous: String,
    pub publish_key_current_id: String,
    pub publish_key_current: String,
    pub publish_key_previous_id: String,
    pub publish_key_previous: String,
}

#[derive(Error, Debug, PartialEq, Eq)]
pub enum RealtimeConfigError {
    #[error("realtime configuration is partial; missing {0}")]
    Missing(&'static str),
    #[error("realtime configuration variable {0} is not valid Unicode")]
    NotUnicode(&'static str),
    #[error("realtime configuration variable {0} is invalid")]
    Invalid(&'static str),
    #[error("realtime current and previous key IDs must differ")]
    DuplicateKeyId,
    #[error("realtime channel, ticket, and publish keys must be distinct")]
    DuplicateKeyMaterial,
    #[error("failed to build realtime HTTP client")]
    Client,
}

#[derive(Serialize, Deserialize)]
pub struct RealtimeTicketResponse {
    pub websocket_url: String,
    pub ticket: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublishOutcome {
    Disabled,
    Success,
    Timeout,
    Transport,
    Provider(StatusCode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RealtimeEvent {
    TicketIssued,
    TicketUnavailable,
    PublishSucceeded,
    PublishTimeout,
    PublishTransportError,
    PublishProviderError(u16),
}

#[derive(Serialize)]
struct RealtimeObservation {
    event: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    status_class: Option<u16>,
}

impl RealtimeEvent {
    fn observation(self) -> RealtimeObservation {
        match self {
            Self::TicketIssued => RealtimeObservation {
                event: "realtime_ticket_issued",
                status_class: None,
            },
            Self::TicketUnavailable => RealtimeObservation {
                event: "realtime_ticket_unavailable",
                status_class: None,
            },
            Self::PublishSucceeded => RealtimeObservation {
                event: "realtime_publish_succeeded",
                status_class: None,
            },
            Self::PublishTimeout => RealtimeObservation {
                event: "realtime_publish_timeout",
                status_class: None,
            },
            Self::PublishTransportError => RealtimeObservation {
                event: "realtime_publish_transport_error",
                status_class: None,
            },
            Self::PublishProviderError(status_class) => RealtimeObservation {
                event: "realtime_publish_provider_error",
                status_class: Some(status_class),
            },
        }
    }

    fn is_failure(self) -> bool {
        !matches!(self, Self::TicketIssued | Self::PublishSucceeded)
    }
}

pub(crate) fn observe_realtime(event: RealtimeEvent) {
    let observation = event.observation();
    if event.is_failure() {
        tracing::warn!(
            event = observation.event,
            status_class = observation.status_class,
            "realtime operation failed"
        );
    } else {
        tracing::info!(
            event = observation.event,
            status_class = observation.status_class,
            "realtime operation completed"
        );
    }
}

impl RealtimeGateway {
    pub fn from_env() -> Result<Self, RealtimeConfigError> {
        Self::from_values(env::var_os)
    }

    pub fn from_string_values(
        lookup: impl Fn(&'static str) -> Option<String>,
    ) -> Result<Self, RealtimeConfigError> {
        Self::from_values(|name| lookup(name).map(OsString::from))
    }

    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn from_settings(settings: RealtimeSettings) -> Result<Self, RealtimeConfigError> {
        let websocket_url =
            parse_endpoint(&settings.websocket_url, "wss", "/v1/connect", WEBSOCKET_URL)?;
        let publish_url =
            parse_endpoint(&settings.publish_url, "https", "/v1/publish", PUBLISH_URL)?;
        let channel_key = parse_key(&settings.channel_key, CHANNEL_KEY)?;
        let ticket_current = SigningKey {
            id: parse_key_id(settings.ticket_key_current_id, TICKET_KEY_CURRENT_ID)?,
            value: parse_key(&settings.ticket_key_current, TICKET_KEY_CURRENT)?,
        };
        let ticket_previous = SigningKey {
            id: parse_key_id(settings.ticket_key_previous_id, TICKET_KEY_PREVIOUS_ID)?,
            value: parse_key(&settings.ticket_key_previous, TICKET_KEY_PREVIOUS)?,
        };
        let publish_current = SigningKey {
            id: parse_key_id(settings.publish_key_current_id, PUBLISH_KEY_CURRENT_ID)?,
            value: parse_key(&settings.publish_key_current, PUBLISH_KEY_CURRENT)?,
        };
        let publish_previous = SigningKey {
            id: parse_key_id(settings.publish_key_previous_id, PUBLISH_KEY_PREVIOUS_ID)?,
            value: parse_key(&settings.publish_key_previous, PUBLISH_KEY_PREVIOUS)?,
        };
        if ticket_current.id == ticket_previous.id || publish_current.id == publish_previous.id {
            return Err(RealtimeConfigError::DuplicateKeyId);
        }
        let key_material = [
            channel_key,
            ticket_current.value,
            ticket_previous.value,
            publish_current.value,
            publish_previous.value,
        ];
        if key_material
            .iter()
            .enumerate()
            .any(|(index, key)| key_material[index + 1..].contains(key))
        {
            return Err(RealtimeConfigError::DuplicateKeyMaterial);
        }
        let client = Client::builder()
            .timeout(PUBLISH_TIMEOUT)
            .redirect(Policy::none())
            .build()
            .map_err(|_| RealtimeConfigError::Client)?;
        Ok(Self {
            enabled: Some(Arc::new(EnabledGateway {
                websocket_url: websocket_url.to_string(),
                publish_url,
                channel_key,
                ticket_current,
                publish_current,
                client,
            })),
        })
    }

    pub fn issue_ticket(&self, tenant_id: Uuid, device_id: Uuid) -> Option<RealtimeTicketResponse> {
        self.issue_ticket_at(tenant_id, device_id, Utc::now())
    }

    pub async fn publish_change(&self, tenant_id: Uuid, device_id: Uuid) {
        match self.publish_outcome(tenant_id, device_id, Utc::now()).await {
            PublishOutcome::Disabled => {}
            PublishOutcome::Success => observe_realtime(RealtimeEvent::PublishSucceeded),
            PublishOutcome::Timeout => observe_realtime(RealtimeEvent::PublishTimeout),
            PublishOutcome::Transport => {
                observe_realtime(RealtimeEvent::PublishTransportError);
            }
            PublishOutcome::Provider(status) => {
                observe_realtime(RealtimeEvent::PublishProviderError(status.as_u16() / 100))
            }
        }
    }

    fn from_values(
        lookup: impl Fn(&'static str) -> Option<OsString>,
    ) -> Result<Self, RealtimeConfigError> {
        let values: HashMap<_, _> = CONFIG_VARIABLES
            .into_iter()
            .map(|name| (name, lookup(name)))
            .collect();
        if values.values().all(Option::is_none) {
            return Ok(Self::disabled());
        }
        for name in CONFIG_VARIABLES {
            if values[&name].is_none() {
                return Err(RealtimeConfigError::Missing(name));
            }
        }

        let value = |name| {
            values[&name]
                .clone()
                .expect("presence checked")
                .into_string()
                .map_err(|_| RealtimeConfigError::NotUnicode(name))
        };
        Self::from_settings(RealtimeSettings {
            websocket_url: value(WEBSOCKET_URL)?,
            publish_url: value(PUBLISH_URL)?,
            channel_key: value(CHANNEL_KEY)?,
            ticket_key_current_id: value(TICKET_KEY_CURRENT_ID)?,
            ticket_key_current: value(TICKET_KEY_CURRENT)?,
            ticket_key_previous_id: value(TICKET_KEY_PREVIOUS_ID)?,
            ticket_key_previous: value(TICKET_KEY_PREVIOUS)?,
            publish_key_current_id: value(PUBLISH_KEY_CURRENT_ID)?,
            publish_key_current: value(PUBLISH_KEY_CURRENT)?,
            publish_key_previous_id: value(PUBLISH_KEY_PREVIOUS_ID)?,
            publish_key_previous: value(PUBLISH_KEY_PREVIOUS)?,
        })
    }

    fn issue_ticket_at(
        &self,
        tenant_id: Uuid,
        device_id: Uuid,
        now: DateTime<Utc>,
    ) -> Option<RealtimeTicketResponse> {
        let enabled = self.enabled.as_ref()?;
        let issued_at = now.timestamp();
        // Keep the response timestamp identical to the whole-second `exp` signed
        // into the ticket so clients never observe a later apparent expiry.
        let expires_at = DateTime::from_timestamp(issued_at + TICKET_TTL_SECONDS, 0)
            .expect("ticket expiry remains in the DateTime range");
        let channel = opaque_channel(&enabled.channel_key, tenant_id);
        let device = opaque_device(&enabled.channel_key, tenant_id, device_id);
        let payload = format!(
            "{{\"kid\":\"{}\",\"aud\":\"{}\",\"channel\":\"{}\",\"device\":\"{}\",\"iat\":{},\"exp\":{}}}",
            enabled.ticket_current.id,
            AUDIENCE,
            channel,
            device,
            issued_at,
            expires_at.timestamp()
        );
        let payload_segment = URL_SAFE_NO_PAD.encode(payload.as_bytes());
        let signature_input = format!("taskveil-realtime-ticket-v1\n{payload_segment}");
        let signature = sign(&enabled.ticket_current.value, signature_input.as_bytes());
        Some(RealtimeTicketResponse {
            websocket_url: enabled.websocket_url.clone(),
            ticket: format!("{payload_segment}.{}", URL_SAFE_NO_PAD.encode(signature)),
            expires_at,
        })
    }

    async fn publish_outcome(
        &self,
        tenant_id: Uuid,
        device_id: Uuid,
        now: DateTime<Utc>,
    ) -> PublishOutcome {
        let Some(enabled) = self.enabled.as_ref() else {
            return PublishOutcome::Disabled;
        };
        let channel = opaque_channel(&enabled.channel_key, tenant_id);
        let source_device = opaque_device(&enabled.channel_key, tenant_id, device_id);
        let body =
            format!("{{\"v\":1,\"channel\":\"{channel}\",\"source_device\":\"{source_device}\"}}");
        if body.len() > PUBLISH_BODY_LIMIT {
            return PublishOutcome::Transport;
        }
        let timestamp = now.timestamp().to_string();
        let mut signature_input =
            format!("taskveil-realtime-publish-v1\n{timestamp}\n").into_bytes();
        signature_input.extend_from_slice(body.as_bytes());
        let signature =
            URL_SAFE_NO_PAD.encode(sign(&enabled.publish_current.value, &signature_input));
        let response = enabled
            .client
            .post(enabled.publish_url.clone())
            .header(CONTENT_LENGTH, body.len())
            .header("X-Taskveil-Realtime-Key-Id", &enabled.publish_current.id)
            .header("X-Taskveil-Realtime-Timestamp", timestamp)
            .header("X-Taskveil-Realtime-Signature", signature)
            .body(body)
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => PublishOutcome::Success,
            Ok(response) => PublishOutcome::Provider(response.status()),
            Err(error) if error.is_timeout() => PublishOutcome::Timeout,
            Err(_) => PublishOutcome::Transport,
        }
    }
}

fn parse_endpoint(
    value: &str,
    scheme: &str,
    path: &str,
    variable: &'static str,
) -> Result<Url, RealtimeConfigError> {
    let url = Url::parse(value).map_err(|_| RealtimeConfigError::Invalid(variable))?;
    if url.scheme() != scheme
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || url.path() != path
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(RealtimeConfigError::Invalid(variable));
    }
    Ok(url)
}

fn parse_key(value: &str, variable: &'static str) -> Result<[u8; 32], RealtimeConfigError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| RealtimeConfigError::Invalid(variable))?;
    if decoded.len() != 32 || URL_SAFE_NO_PAD.encode(&decoded) != value {
        return Err(RealtimeConfigError::Invalid(variable));
    }
    decoded
        .try_into()
        .map_err(|_| RealtimeConfigError::Invalid(variable))
}

fn parse_key_id(value: String, variable: &'static str) -> Result<String, RealtimeConfigError> {
    if value.is_empty()
        || value.len() > 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(RealtimeConfigError::Invalid(variable));
    }
    Ok(value)
}

fn opaque_channel(key: &[u8; 32], tenant_id: Uuid) -> String {
    let input = format!("taskveil-realtime-channel-v1\n{}", tenant_id.hyphenated());
    URL_SAFE_NO_PAD.encode(sign(key, input.as_bytes()))
}

fn opaque_device(key: &[u8; 32], tenant_id: Uuid, device_id: Uuid) -> String {
    let input = format!(
        "taskveil-realtime-device-v1\n{}\n{}",
        tenant_id.hyphenated(),
        device_id.hyphenated()
    );
    URL_SAFE_NO_PAD.encode(sign(key, input.as_bytes()))
}

fn sign(key: &[u8; 32], input: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts 32-byte keys");
    mac.update(input);
    mac.finalize().into_bytes().into()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc as StdArc, Mutex},
        time::Duration,
    };

    use axum::{
        http::{HeaderMap, StatusCode as AxumStatus},
        routing::post,
        Router,
    };
    use serde::Deserialize;
    use tokio::net::TcpListener;

    use super::*;

    fn encoded_key(byte: u8) -> String {
        URL_SAFE_NO_PAD.encode([byte; 32])
    }

    fn valid_values() -> HashMap<&'static str, OsString> {
        HashMap::from([
            (WEBSOCKET_URL, "wss://realtime.example/v1/connect".into()),
            (PUBLISH_URL, "https://realtime.example/v1/publish".into()),
            (CHANNEL_KEY, encoded_key(1).into()),
            (TICKET_KEY_CURRENT_ID, "ticket-current".into()),
            (TICKET_KEY_CURRENT, encoded_key(2).into()),
            (TICKET_KEY_PREVIOUS_ID, "ticket-previous".into()),
            (TICKET_KEY_PREVIOUS, encoded_key(3).into()),
            (PUBLISH_KEY_CURRENT_ID, "publish-current".into()),
            (PUBLISH_KEY_CURRENT, encoded_key(4).into()),
            (PUBLISH_KEY_PREVIOUS_ID, "publish-previous".into()),
            (PUBLISH_KEY_PREVIOUS, encoded_key(5).into()),
        ])
    }

    #[test]
    fn configuration_is_disabled_only_when_every_variable_is_absent() {
        assert!(RealtimeGateway::from_values(|_| None)
            .unwrap()
            .enabled
            .is_none());
        let mut values = valid_values();
        values.remove(PUBLISH_KEY_PREVIOUS);
        assert_eq!(
            RealtimeGateway::from_values(|name| values.get(name).cloned()).err(),
            Some(RealtimeConfigError::Missing(PUBLISH_KEY_PREVIOUS))
        );
    }

    #[test]
    fn configuration_rejects_insecure_urls_invalid_keys_and_key_reuse() {
        let mut values = valid_values();
        values.insert(PUBLISH_URL, "http://realtime.example/v1/publish".into());
        assert_eq!(
            RealtimeGateway::from_values(|name| values.get(name).cloned()).err(),
            Some(RealtimeConfigError::Invalid(PUBLISH_URL))
        );

        let mut values = valid_values();
        values.insert(CHANNEL_KEY, encoded_key(2).into());
        assert_eq!(
            RealtimeGateway::from_values(|name| values.get(name).cloned()).err(),
            Some(RealtimeConfigError::DuplicateKeyMaterial)
        );

        let mut values = valid_values();
        values.insert(TICKET_KEY_PREVIOUS_ID, "ticket-current".into());
        assert_eq!(
            RealtimeGateway::from_values(|name| values.get(name).cloned()).err(),
            Some(RealtimeConfigError::DuplicateKeyId)
        );

        let mut values = valid_values();
        values.insert(TICKET_KEY_CURRENT, format!("{}=", encoded_key(2)).into());
        assert_eq!(
            RealtimeGateway::from_values(|name| values.get(name).cloned()).err(),
            Some(RealtimeConfigError::Invalid(TICKET_KEY_CURRENT))
        );
    }

    #[derive(Deserialize)]
    struct Fixture {
        rust_generated: SignatureVectorFixture,
        typescript_generated: SignatureVectorFixture,
        opaque_identifiers: OpaqueIdentifierFixture,
    }

    #[derive(Deserialize)]
    struct SignatureVectorFixture {
        ticket: TicketFixture,
        publish: PublishFixture,
    }

    #[derive(Deserialize)]
    struct TicketFixture {
        key_base64url: String,
        payload: String,
        payload_segment: String,
        signature: String,
        token: String,
    }

    #[derive(Deserialize)]
    struct PublishFixture {
        key_base64url: String,
        key_id: String,
        timestamp: i64,
        body: String,
        signature: String,
    }

    #[derive(Deserialize)]
    struct OpaqueIdentifierFixture {
        channel_key_base64url: String,
        tenant_id: Uuid,
        device_id: Uuid,
        channel: String,
        device: String,
    }

    fn fixture() -> Fixture {
        serde_json::from_str(include_str!(
            "../../realtime-worker/test/fixtures/realtime-hmac-v1.json"
        ))
        .unwrap()
    }

    #[test]
    fn rust_generated_signatures_match_the_shared_fixture() {
        assert_signature_vector(&fixture().rust_generated);
    }

    #[test]
    fn typescript_generated_signatures_verify_in_rust() {
        assert_signature_vector(&fixture().typescript_generated);
    }

    fn assert_signature_vector(vector: &SignatureVectorFixture) {
        let ticket_key = parse_key(&vector.ticket.key_base64url, TICKET_KEY_CURRENT).unwrap();
        let payload_segment = URL_SAFE_NO_PAD.encode(vector.ticket.payload.as_bytes());
        assert_eq!(payload_segment, vector.ticket.payload_segment);
        let ticket_input = format!("taskveil-realtime-ticket-v1\n{payload_segment}");
        let signature = URL_SAFE_NO_PAD.encode(sign(&ticket_key, ticket_input.as_bytes()));
        assert_eq!(signature, vector.ticket.signature);
        assert_eq!(
            format!("{payload_segment}.{signature}"),
            vector.ticket.token
        );

        assert!(vector.publish.key_id.ends_with("-publish"));
        let publish_key = parse_key(&vector.publish.key_base64url, PUBLISH_KEY_CURRENT).unwrap();
        let input = format!(
            "taskveil-realtime-publish-v1\n{}\n{}",
            vector.publish.timestamp, vector.publish.body
        );
        assert_eq!(
            URL_SAFE_NO_PAD.encode(sign(&publish_key, input.as_bytes())),
            vector.publish.signature
        );
    }

    #[test]
    fn opaque_identifiers_match_the_shared_domain_separator_fixture() {
        let fixture = fixture().opaque_identifiers;
        let key = parse_key(&fixture.channel_key_base64url, CHANNEL_KEY).unwrap();
        assert_eq!(opaque_channel(&key, fixture.tenant_id), fixture.channel);
        assert_eq!(
            opaque_device(&key, fixture.tenant_id, fixture.device_id),
            fixture.device
        );
    }

    #[test]
    fn structured_observations_have_a_secret_safe_field_allowlist() {
        for event in [
            RealtimeEvent::TicketIssued,
            RealtimeEvent::TicketUnavailable,
            RealtimeEvent::PublishSucceeded,
            RealtimeEvent::PublishTimeout,
            RealtimeEvent::PublishTransportError,
            RealtimeEvent::PublishProviderError(5),
        ] {
            let value = serde_json::to_value(event.observation()).unwrap();
            let fields = value.as_object().unwrap();
            assert!(fields
                .keys()
                .all(|key| key == "event" || key == "status_class"));
            assert!(fields.len() <= 2);
            let serialized = value.to_string();
            for forbidden in ["ticket", "tenant", "device", "channel", "record", "secret"] {
                assert!(!serialized.contains(&format!("{forbidden}_id")));
            }
        }
    }

    #[tokio::test]
    async fn provider_error_timeout_and_network_failure_are_typed_outcomes() {
        let provider_url = spawn_provider(AxumStatus::SERVICE_UNAVAILABLE, Duration::ZERO).await;
        let gateway = test_gateway(provider_url);
        assert_eq!(
            gateway
                .publish_outcome(Uuid::nil(), Uuid::max(), Utc::now())
                .await,
            PublishOutcome::Provider(StatusCode::SERVICE_UNAVAILABLE)
        );

        let timeout_url = spawn_provider(AxumStatus::NO_CONTENT, Duration::from_secs(1)).await;
        let gateway = test_gateway(timeout_url);
        assert_eq!(
            gateway
                .publish_outcome(Uuid::nil(), Uuid::max(), Utc::now())
                .await,
            PublishOutcome::Timeout
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_url = Url::parse(&format!(
            "http://{}/v1/publish",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        drop(listener);
        let gateway = test_gateway(closed_url);
        assert_eq!(
            gateway
                .publish_outcome(Uuid::nil(), Uuid::max(), Utc::now())
                .await,
            PublishOutcome::Transport
        );
    }

    #[tokio::test]
    async fn actual_publish_request_uses_the_fixed_body_and_raw_body_signature() {
        let captured = StdArc::new(Mutex::new(None));
        let destination = captured.clone();
        let app = Router::new().route(
            "/v1/publish",
            post(move |headers: HeaderMap, body: String| {
                let destination = destination.clone();
                async move {
                    *destination.lock().unwrap() = Some((headers, body));
                    AxumStatus::NO_CONTENT
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let gateway = test_gateway(Url::parse(&format!("http://{address}/v1/publish")).unwrap());
        let now = DateTime::from_timestamp(1_784_059_200, 0).unwrap();
        assert_eq!(
            gateway.publish_outcome(Uuid::nil(), Uuid::max(), now).await,
            PublishOutcome::Success
        );

        let (headers, body) = captured.lock().unwrap().take().unwrap();
        let channel = opaque_channel(&[1; 32], Uuid::nil());
        let device = opaque_device(&[1; 32], Uuid::nil(), Uuid::max());
        assert_eq!(
            body,
            format!("{{\"v\":1,\"channel\":\"{channel}\",\"source_device\":\"{device}\"}}")
        );
        assert_eq!(headers["X-Taskveil-Realtime-Key-Id"], "publish-current");
        assert_eq!(headers["X-Taskveil-Realtime-Timestamp"], "1784059200");
        assert_eq!(headers[CONTENT_LENGTH], body.len().to_string());
        let signature = URL_SAFE_NO_PAD
            .decode(headers["X-Taskveil-Realtime-Signature"].to_str().unwrap())
            .unwrap();
        let input = format!("taskveil-realtime-publish-v1\n1784059200\n{body}");
        let mut mac = HmacSha256::new_from_slice(&[4; 32]).unwrap();
        mac.update(input.as_bytes());
        mac.verify_slice(&signature).unwrap();
    }

    async fn spawn_provider(status: AxumStatus, delay: Duration) -> Url {
        let app = Router::new().route(
            "/v1/publish",
            post(move || async move {
                tokio::time::sleep(delay).await;
                status
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Url::parse(&format!("http://{address}/v1/publish")).unwrap()
    }

    fn test_gateway(publish_url: Url) -> RealtimeGateway {
        RealtimeGateway {
            enabled: Some(Arc::new(EnabledGateway {
                websocket_url: "wss://realtime.example/v1/connect".to_owned(),
                publish_url,
                channel_key: [1; 32],
                ticket_current: SigningKey {
                    id: "ticket-current".to_owned(),
                    value: [2; 32],
                },
                publish_current: SigningKey {
                    id: "publish-current".to_owned(),
                    value: [4; 32],
                },
                client: Client::builder()
                    .timeout(PUBLISH_TIMEOUT)
                    .redirect(Policy::none())
                    .build()
                    .unwrap(),
            })),
        }
    }
}
