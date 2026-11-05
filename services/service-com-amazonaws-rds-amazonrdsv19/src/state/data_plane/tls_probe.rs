// SPDX-License-Identifier: CC0-1.0

//! What the TLS handshake of one session negotiated, read from the handshake itself.
//!
//! The core owns the TLS: a wire handler hands its plaintext connection to
//! [`::emulator_server::TlsUpgrader`] and gets back the decrypted stream — and nothing
//! else, so the protocol version and cipher suite the handshake settled on are not
//! reported back to the handler. A PostgreSQL client reads them from `pg_stat_ssl` and a
//! MySQL client from `Ssl_version` / `Ssl_cipher`, so this module OBSERVES them: the
//! connection is wrapped in a tap that keeps a copy of the first bytes the server writes,
//! which is the server's own `ServerHello`, and [`negotiated`] reads the version and the
//! suite out of it.
//!
//! Nothing here is inferred from configuration: a handshake whose `ServerHello` this
//! module cannot read reports no version and no suite, and the session then says it is
//! encrypted without naming what it negotiated, rather than naming something nobody
//! measured.

use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::wire_session::TlsFacts;

/// How many bytes of the server's first writes are kept. A `ServerHello` record is a few
/// hundred bytes; this bounds what the tap can hold whatever the server writes.
const MAX_RECORDED_BYTES: usize = 2048;

/// The copy of the server's first bytes, shared between the tap and its reader.
#[derive(Debug, Clone, Default)]
pub struct HandshakeRecord(Arc<Mutex<Vec<u8>>>);

impl HandshakeRecord {
    /// What the handshake negotiated, as far as the recorded bytes show.
    #[must_use]
    pub fn negotiated(&self) -> TlsFacts {
        let recorded = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        negotiated(&recorded)
    }
}

/// A connection that keeps a copy of the first [`MAX_RECORDED_BYTES`] the SERVER writes on
/// it, and is otherwise the connection it wraps.
pub struct HandshakeTap<S> {
    inner: S,
    record: HandshakeRecord,
}

impl<S> HandshakeTap<S> {
    /// Wrap `inner`, recording into `record`.
    pub fn new(inner: S, record: HandshakeRecord) -> Self {
        Self { inner, record }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for HandshakeTap<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for HandshakeTap<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let written = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(count)) = &written {
            let mut recorded = self
                .record
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if recorded.len() < MAX_RECORDED_BYTES {
                let room = MAX_RECORDED_BYTES - recorded.len();
                let taken = (*count).min(room);
                if let Some(bytes) = buf.get(..taken) {
                    recorded.extend_from_slice(bytes);
                }
            }
        }
        written
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// The protocol version and cipher suite a recorded `ServerHello` names.
///
/// Reads the TLS record (content type 22, handshake), the `ServerHello` inside it, its
/// legacy version, the suite it selected and — for a TLS 1.3 handshake, where the version
/// travels in an extension rather than in the legacy field — the `supported_versions`
/// extension. Every read is bounds-checked: a truncated or unexpected handshake reports
/// nothing rather than panicking on a request path.
#[must_use]
pub fn negotiated(recorded: &[u8]) -> TlsFacts {
    read_server_hello(recorded).unwrap_or_default()
}

/// The reading itself, written against `Option` so every index is checked.
fn read_server_hello(recorded: &[u8]) -> Option<TlsFacts> {
    let body = server_hello_body(recorded)?;
    // ServerHello: legacy_version(2) random(32) session_id_len(1) session_id
    //              cipher_suite(2) compression(1) extensions_len(2) extensions
    let legacy_version = u16::from_be_bytes([*body.first()?, *body.get(1)?]);
    let session_id_length = usize::from(*body.get(34)?);
    let after_session = 35 + session_id_length;
    let cipher_suite = u16::from_be_bytes([
        *body.get(after_session)?,
        *body.get(after_session + 1)?,
    ]);
    let extensions_length = usize::from(u16::from_be_bytes([
        *body.get(after_session + 3)?,
        *body.get(after_session + 4)?,
    ]));
    let extensions_start = after_session + 5;
    let extensions = body
        .get(extensions_start..extensions_start + extensions_length)
        .or_else(|| body.get(extensions_start..))
        .unwrap_or_default();
    let version = selected_version(extensions).unwrap_or(legacy_version);
    Some(TlsFacts {
        protocol: protocol_name(version).map(str::to_string),
        cipher: cipher_suite_name(cipher_suite).map(str::to_string),
    })
}

/// The version a TLS 1.3 `ServerHello` carries in its `supported_versions` extension,
/// which is where the negotiated version lives when the legacy field says TLS 1.2.
fn selected_version(extensions: &[u8]) -> Option<u16> {
    const SUPPORTED_VERSIONS: u16 = 0x002b;
    let mut at = 0usize;
    while at + 4 <= extensions.len() {
        let kind = u16::from_be_bytes([*extensions.get(at)?, *extensions.get(at + 1)?]);
        let length = usize::from(u16::from_be_bytes([
            *extensions.get(at + 2)?,
            *extensions.get(at + 3)?,
        ]));
        let value = extensions.get(at + 4..at + 4 + length)?;
        if kind == SUPPORTED_VERSIONS {
            return Some(u16::from_be_bytes([*value.first()?, *value.get(1)?]));
        }
        at += 4 + length;
    }
    None
}

/// The name a database server reports a TLS version under (`SHOW ssl`'s neighbours:
/// `pg_stat_ssl.version`, MySQL's `Ssl_version`), from the two-byte version the handshake
/// settled on. Source: the version values the TLS 1.0-1.3 RFCs assign.
fn protocol_name(version: u16) -> Option<&'static str> {
    match version {
        0x0304 => Some("TLSv1.3"),
        0x0303 => Some("TLSv1.2"),
        0x0302 => Some("TLSv1.1"),
        0x0301 => Some("TLSv1"),
        _ => None,
    }
}

/// The name a database server reports a cipher suite under, from the two-byte suite the
/// `ServerHello` selected.
///
/// Source: the suite values in IANA's TLS Cipher Suite registry. A TLS 1.3 suite is
/// reported under its registry name (`TLS_AES_256_GCM_SHA384`), which is what OpenSSL —
/// and so PostgreSQL and MySQL — reports for it; a TLS 1.2 suite is reported under the
/// OpenSSL name those servers use for it. The suite value itself says which of the two it
/// is, so the version is not part of this reading. The list covers the suites the core's
/// TLS can negotiate; a suite that is not in it is reported as no name rather than a
/// guess.
fn cipher_suite_name(suite: u16) -> Option<&'static str> {
    match suite {
        0x1301 => Some("TLS_AES_128_GCM_SHA256"),
        0x1302 => Some("TLS_AES_256_GCM_SHA384"),
        0x1303 => Some("TLS_CHACHA20_POLY1305_SHA256"),
        0xc02b => Some("ECDHE-ECDSA-AES128-GCM-SHA256"),
        0xc02c => Some("ECDHE-ECDSA-AES256-GCM-SHA384"),
        0xc02f => Some("ECDHE-RSA-AES128-GCM-SHA256"),
        0xc030 => Some("ECDHE-RSA-AES256-GCM-SHA384"),
        0xcca8 => Some("ECDHE-RSA-CHACHA20-POLY1305"),
        0xcca9 => Some("ECDHE-ECDSA-CHACHA20-POLY1305"),
        _ => None,
    }
}

/// The `ServerHello` handshake body inside a recorded TLS record, if that is what the
/// recorded bytes begin with.
fn server_hello_body(recorded: &[u8]) -> Option<&[u8]> {
    const CONTENT_TYPE_HANDSHAKE: u8 = 22;
    const HANDSHAKE_SERVER_HELLO: u8 = 2;
    if *recorded.first()? != CONTENT_TYPE_HANDSHAKE {
        return None;
    }
    let record_length = usize::from(u16::from_be_bytes([
        *recorded.get(3)?,
        *recorded.get(4)?,
    ]));
    let available = recorded.len().checked_sub(5)?;
    let record = recorded.get(5..5 + record_length.min(available))?;
    if *record.first()? != HANDSHAKE_SERVER_HELLO {
        return None;
    }
    let body_length = (usize::from(*record.get(1)?) << 16)
        | (usize::from(*record.get(2)?) << 8)
        | usize::from(*record.get(3)?);
    let available = record.len().checked_sub(4)?;
    record.get(4..4 + body_length.min(available))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A TLS 1.3 `ServerHello` record, assembled byte for byte as the RFC lays it out, so
    /// the reading is checked against the wire format rather than against itself.
    fn server_hello(legacy: u16, suite: u16, supported: Option<u16>) -> Vec<u8> {
        let mut extensions = Vec::new();
        if let Some(version) = supported {
            extensions.extend_from_slice(&0x002bu16.to_be_bytes());
            extensions.extend_from_slice(&2u16.to_be_bytes());
            extensions.extend_from_slice(&version.to_be_bytes());
        }
        let mut body = Vec::new();
        body.extend_from_slice(&legacy.to_be_bytes());
        body.extend_from_slice(&[7u8; 32]);
        body.push(0);
        body.extend_from_slice(&suite.to_be_bytes());
        body.push(0);
        body.extend_from_slice(
            &u16::try_from(extensions.len()).unwrap_or_default().to_be_bytes(),
        );
        body.extend_from_slice(&extensions);
        let mut handshake = vec![2u8];
        let length = body.len();
        handshake.push(u8::try_from((length >> 16) & 0xff).unwrap_or_default());
        handshake.push(u8::try_from((length >> 8) & 0xff).unwrap_or_default());
        handshake.push(u8::try_from(length & 0xff).unwrap_or_default());
        handshake.extend_from_slice(&body);
        let mut record = vec![22u8, 3, 3];
        record.extend_from_slice(
            &u16::try_from(handshake.len()).unwrap_or_default().to_be_bytes(),
        );
        record.extend_from_slice(&handshake);
        record
    }

    /// One row of the table below: the case, the recorded bytes, and the protocol and
    /// cipher they name.
    type Case = (&'static str, Vec<u8>, Option<&'static str>, Option<&'static str>);

    #[test]
    fn a_recorded_server_hello_names_the_version_and_the_suite() {
        // The rows span what the reading generalises over: a TLS 1.3 handshake (version
        // in the extension), a TLS 1.2 one (version in the legacy field), a suite the
        // list does not name, and bytes that are no handshake at all.
        let cases: &[Case] = &[
            (
                "tls 1.3, aes-256",
                server_hello(0x0303, 0x1302, Some(0x0304)),
                Some("TLSv1.3"),
                Some("TLS_AES_256_GCM_SHA384"),
            ),
            (
                "tls 1.2, ecdhe-rsa",
                server_hello(0x0303, 0xc030, None),
                Some("TLSv1.2"),
                Some("ECDHE-RSA-AES256-GCM-SHA384"),
            ),
            (
                "a suite this list does not name",
                server_hello(0x0303, 0x00ff, Some(0x0304)),
                Some("TLSv1.3"),
                None,
            ),
            ("no handshake at all", vec![1, 2, 3], None, None),
            ("nothing recorded", Vec::new(), None, None),
        ];
        for (case, recorded, protocol, cipher) in cases {
            let facts = negotiated(recorded);
            assert_eq!(
                facts.protocol.as_deref(),
                *protocol,
                "case {case:?}: expected the protocol {protocol:?}, got {:?}",
                facts.protocol
            );
            assert_eq!(
                facts.cipher.as_deref(),
                *cipher,
                "case {case:?}: expected the cipher {cipher:?}, got {:?}",
                facts.cipher
            );
        }
    }
}
