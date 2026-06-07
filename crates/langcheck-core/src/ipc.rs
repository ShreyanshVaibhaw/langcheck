//! Wire protocol for broker IPC — the channel between the post-MVP TSF adapter
//! (running inside host apps) and the broker.
//!
//! Platform-independent message types + a tiny length-framed binary encoding. The
//! *transport* (a same-user, local-only named pipe) lives in `langcheck-windows`;
//! this module is pure so the format stays deterministic and unit-testable on every
//! platform.
//!
//! The broker is the only process that holds language logic and persistence
//! (`blueprint.md` Sections 7.1, 11.4), so the adapter never decides anything: it
//! sends a just-typed token and the broker replies with a [`Response`].
//!
//! Encoding is deliberately hand-rolled (no serde dependency in `core`) and
//! defensive: every `decode_*` validates tags, lengths, and UTF-8, returns an
//! [`IpcError`] rather than panicking, and rejects oversized fields.

use std::fmt;

use crate::session::Boundary;

/// Protocol version, bumped on any incompatible wire change so the broker can
/// reject a mismatched client.
pub const PROTOCOL_VERSION: u8 = 1;

/// Upper bound on a single string field (token / replacement). A defensive cap so
/// a corrupt or hostile length prefix cannot trigger a huge allocation.
pub const MAX_FIELD_LEN: usize = 256;

/// A request from the adapter to the broker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Liveness/handshake check.
    Ping,
    /// Ask whether a just-typed `token`, followed by `boundary`, should be
    /// corrected. The broker replies with [`Response::Leave`] or
    /// [`Response::Replace`].
    Evaluate { token: String, boundary: Boundary },
    /// Focus beacon: the adapter is now the active input method in the foreground
    /// (sent on focus, before any word is typed). Lets the broker tell the MVP
    /// keystroke path to stand down for that window before it can fire — avoiding a
    /// race where both paths correct the same word. The broker replies [`Pong`].
    Active,
}

/// A reply from the broker to the adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// Liveness reply.
    Pong,
    /// Leave the token unchanged.
    Leave,
    /// Replace the token with `replacement`.
    Replace { replacement: String },
}

/// A decoding failure. Encoding never fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    /// No bytes to decode.
    Empty,
    /// The leading message tag is not recognised.
    UnknownTag(u8),
    /// The buffer ended mid-field.
    Truncated,
    /// A string field was not valid UTF-8.
    BadUtf8,
    /// A length prefix exceeded [`MAX_FIELD_LEN`].
    TooLong,
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IpcError::Empty => write!(f, "empty IPC message"),
            IpcError::UnknownTag(tag) => write!(f, "unknown IPC tag {tag}"),
            IpcError::Truncated => write!(f, "truncated IPC message"),
            IpcError::BadUtf8 => write!(f, "invalid UTF-8 in IPC message"),
            IpcError::TooLong => write!(f, "IPC field exceeds maximum length"),
        }
    }
}

impl std::error::Error for IpcError {}

// Message tags.
const TAG_PING: u8 = 0x01;
const TAG_EVALUATE: u8 = 0x02;
const TAG_ACTIVE: u8 = 0x03;
const TAG_PONG: u8 = 0x01;
const TAG_LEAVE: u8 = 0x02;
const TAG_REPLACE: u8 = 0x03;

/// Encode a [`Request`] to bytes (without the transport length prefix).
pub fn encode_request(request: &Request) -> Vec<u8> {
    let mut out = Vec::new();
    match request {
        Request::Ping => out.push(TAG_PING),
        Request::Evaluate { token, boundary } => {
            out.push(TAG_EVALUATE);
            out.push(boundary_to_u8(*boundary));
            put_string(&mut out, token);
        }
        Request::Active => out.push(TAG_ACTIVE),
    }
    out
}

/// Decode a [`Request`] from bytes.
pub fn decode_request(bytes: &[u8]) -> Result<Request, IpcError> {
    let (&tag, mut rest) = bytes.split_first().ok_or(IpcError::Empty)?;
    match tag {
        TAG_PING => Ok(Request::Ping),
        TAG_EVALUATE => {
            let (&code, after) = rest.split_first().ok_or(IpcError::Truncated)?;
            rest = after;
            let boundary = boundary_from_u8(code)?;
            let token = take_string(&mut rest)?;
            Ok(Request::Evaluate { token, boundary })
        }
        TAG_ACTIVE => Ok(Request::Active),
        other => Err(IpcError::UnknownTag(other)),
    }
}

/// Encode a [`Response`] to bytes (without the transport length prefix).
pub fn encode_response(response: &Response) -> Vec<u8> {
    let mut out = Vec::new();
    match response {
        Response::Pong => out.push(TAG_PONG),
        Response::Leave => out.push(TAG_LEAVE),
        Response::Replace { replacement } => {
            out.push(TAG_REPLACE);
            put_string(&mut out, replacement);
        }
    }
    out
}

/// Decode a [`Response`] from bytes.
pub fn decode_response(bytes: &[u8]) -> Result<Response, IpcError> {
    let (&tag, mut rest) = bytes.split_first().ok_or(IpcError::Empty)?;
    match tag {
        TAG_PONG => Ok(Response::Pong),
        TAG_LEAVE => Ok(Response::Leave),
        TAG_REPLACE => {
            let replacement = take_string(&mut rest)?;
            Ok(Response::Replace { replacement })
        }
        other => Err(IpcError::UnknownTag(other)),
    }
}

/// Append a `u32` big-endian length-prefixed UTF-8 string.
fn put_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u32).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

/// Read a `u32`-length-prefixed UTF-8 string from the front of `rest`.
fn take_string(rest: &mut &[u8]) -> Result<String, IpcError> {
    if rest.len() < 4 {
        return Err(IpcError::Truncated);
    }
    let (len_bytes, after_len) = rest.split_at(4);
    let len = u32::from_be_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]) as usize;
    if len > MAX_FIELD_LEN {
        return Err(IpcError::TooLong);
    }
    if after_len.len() < len {
        return Err(IpcError::Truncated);
    }
    let (text, after) = after_len.split_at(len);
    let value = std::str::from_utf8(text).map_err(|_| IpcError::BadUtf8)?;
    *rest = after;
    Ok(value.to_owned())
}

fn boundary_to_u8(boundary: Boundary) -> u8 {
    match boundary {
        Boundary::Space => 0,
        Boundary::Period => 1,
        Boundary::Comma => 2,
        Boundary::Question => 3,
        Boundary::Exclamation => 4,
        Boundary::Colon => 5,
        Boundary::Semicolon => 6,
    }
}

fn boundary_from_u8(code: u8) -> Result<Boundary, IpcError> {
    match code {
        0 => Ok(Boundary::Space),
        1 => Ok(Boundary::Period),
        2 => Ok(Boundary::Comma),
        3 => Ok(Boundary::Question),
        4 => Ok(Boundary::Exclamation),
        5 => Ok(Boundary::Colon),
        6 => Ok(Boundary::Semicolon),
        other => Err(IpcError::UnknownTag(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_boundaries() -> [Boundary; 7] {
        [
            Boundary::Space,
            Boundary::Period,
            Boundary::Comma,
            Boundary::Question,
            Boundary::Exclamation,
            Boundary::Colon,
            Boundary::Semicolon,
        ]
    }

    #[test]
    fn request_round_trips() {
        for simple in [Request::Ping, Request::Active] {
            assert_eq!(decode_request(&encode_request(&simple)).unwrap(), simple);
        }
        for boundary in all_boundaries() {
            let req = Request::Evaluate {
                token: "wierd".to_owned(),
                boundary,
            };
            assert_eq!(decode_request(&encode_request(&req)).unwrap(), req);
        }
    }

    #[test]
    fn response_round_trips() {
        for resp in [
            Response::Pong,
            Response::Leave,
            Response::Replace {
                replacement: "weird".to_owned(),
            },
        ] {
            assert_eq!(decode_response(&encode_response(&resp)).unwrap(), resp);
        }
    }

    #[test]
    fn empty_and_unknown_tags_error() {
        assert_eq!(decode_request(&[]), Err(IpcError::Empty));
        assert_eq!(decode_response(&[]), Err(IpcError::Empty));
        assert_eq!(decode_request(&[0xFF]), Err(IpcError::UnknownTag(0xFF)));
        assert_eq!(decode_response(&[0xFF]), Err(IpcError::UnknownTag(0xFF)));
    }

    #[test]
    fn truncated_messages_error_not_panic() {
        // Evaluate tag + boundary but no string length.
        assert_eq!(decode_request(&[TAG_EVALUATE, 0]), Err(IpcError::Truncated));
        // Evaluate with a length prefix that overruns the buffer.
        let mut bytes = vec![TAG_EVALUATE, 0];
        bytes.extend_from_slice(&5u32.to_be_bytes());
        bytes.extend_from_slice(b"ab"); // claims 5, provides 2
        assert_eq!(decode_request(&bytes), Err(IpcError::Truncated));
    }

    #[test]
    fn unknown_boundary_code_errors() {
        assert_eq!(
            decode_request(&[TAG_EVALUATE, 99]),
            Err(IpcError::UnknownTag(99))
        );
    }

    #[test]
    fn oversized_field_is_rejected() {
        let mut bytes = vec![TAG_REPLACE];
        bytes.extend_from_slice(&((MAX_FIELD_LEN as u32) + 1).to_be_bytes());
        assert_eq!(decode_response(&bytes), Err(IpcError::TooLong));
    }

    #[test]
    fn bad_utf8_is_rejected() {
        let mut bytes = vec![TAG_REPLACE];
        bytes.extend_from_slice(&2u32.to_be_bytes());
        bytes.extend_from_slice(&[0xFF, 0xFE]);
        assert_eq!(decode_response(&bytes), Err(IpcError::BadUtf8));
    }
}
