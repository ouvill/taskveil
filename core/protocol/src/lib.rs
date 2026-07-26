//! Taskveil network wire contracts.
//!
//! This crate is deliberately limited to serializable DTOs, wire enums,
//! canonical wire value types, protocol versions, and framing limits. It must
//! not depend on storage, HTTP transports, domain logic, crypto operations, or
//! client runtime code.

pub mod account;
pub mod envelope;
pub mod hlc;
pub mod organization;
pub mod sync;

pub use envelope::{
    parse_envelope_header, EnvelopeHeader, EnvelopeHeaderError, ENVELOPE_HEADER_LEN,
    ENVELOPE_MAGIC, ENVELOPE_MIN_LEN, ENVELOPE_VERSION, MAX_ENCRYPTED_BLOB_LEN,
};
pub use hlc::{HlcWireError, WireHlc};
pub use sync::RotationStatus;
