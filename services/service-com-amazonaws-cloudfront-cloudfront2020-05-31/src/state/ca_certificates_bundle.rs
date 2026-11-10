// SPDX-License-Identifier: CC0-1.0

//! Where a trust store's CA certificates come from: the object Amazon S3 holds at the
//! address `CaCertificatesBundleSource` names.
//!
//! `TrustStore$NumberOfCaCertificates` is "The trust store's number of CA certificates", so
//! a create cannot answer at all without READING the bundle it was given: the count is a
//! property of that object's bytes, not of the request. This emulator holds an Amazon S3 of
//! its own, reached the way any emulated service reaches another — the AWS SDK over the
//! emulator's in-process transport, through the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]), which opens no socket, resolves
//! no name and reaches nothing outside this process. An object a caller staged with
//! `PutObject` is therefore the bundle a trust store is built from.
//!
//! ## What a read establishes
//!
//! Every outcome is one of four, and none of them is a success the read did not establish:
//!
//! * the store served the object and its bytes hold PEM-encoded X.509 certificates, so the
//!   count is known ([`CaCertificatesBundle::Counted`]);
//! * the store ANSWERED that it holds nothing at that address, so the entity the request
//!   named was not found ([`CaCertificatesBundle::Absent`]);
//! * the store ANSWERED that the read is not permitted, so access to it was denied
//!   ([`CaCertificatesBundle::Denied`]);
//! * the bundle could not be read as a CA certificates bundle at all
//!   ([`CaCertificatesBundle::Invalid`]) — the source addresses no object, the object is
//!   empty or larger than this emulator reads, its bytes hold no certificate, or the store
//!   ANSWERED something else about that object. A create that could not read its bundle is
//!   REFUSED: there is no outcome in which a trust store is stored reporting certificates
//!   nothing counted;
//! * nothing ANSWERED the read at all — no object store is mounted to ask, or the call could
//!   not be dispatched — so the request is not refused as the caller's fault either
//!   ([`CaCertificatesBundle::Unanswered`]): that is this emulator's own inability to serve
//!   the operation, and it is reported as such.
//!
//! The PEM layer and the certificate layer are each read by the crate that owns it: `pem`
//! splits the bundle into its blocks, and `x509-cert` decodes a block's DER as an X.509
//! certificate — so what is counted is certificates a parser accepted, never occurrences of
//! a marker in the text.

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;
use ::x509_cert::der::Decode;

use crate::ops::types::CaCertificatesBundleSource;

/// The PEM block label a certificate travels in (RFC 7468 §5), and so the only block of a
/// CA certificates bundle that counts towards its certificate count.
pub const CERTIFICATE_PEM_TAG: &str = "CERTIFICATE";

/// The most bundle bytes this emulator reads into the process.
///
/// CHOSEN by this emulator rather than read off a published quota: it bounds what one
/// control-plane call buffers, and it is far above any plausible CA bundle (a PEM
/// certificate is on the order of a kilobyte). The read is RANGED to it, so a larger object
/// is refused rather than buffered whole.
pub const MAX_CA_CERTIFICATES_BUNDLE_BYTES: usize = 1_048_576;

/// What a read of a trust store's CA certificates bundle established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaCertificatesBundle {
    /// The store served the bundle, and it holds this many CA certificates (at least one).
    Counted(i32),
    /// The store answered that it holds nothing at the address the request named.
    Absent,
    /// The store answered that the read of the bundle is not permitted.
    Denied,
    /// No CA certificates bundle could be read at that address: the source addresses no
    /// object, the object is empty or past the bound, its bytes hold no certificate, or the
    /// store answered something else about that object.
    Invalid,
    /// Nothing answered the read: there is no object store to ask, or the call could not be
    /// dispatched — so nothing is known about the bundle and nothing is the caller's fault.
    Unanswered,
}

/// The Amazon S3 error codes in which the store answers that it holds NOTHING at the
/// address a request named.
const BUNDLE_ABSENT_CODES: &[&str] = &["NoSuchBucket", "NoSuchKey", "NoSuchVersion", "NotFound"];

/// The Amazon S3 error codes in which the store answers that the read is NOT PERMITTED —
/// the bundle may well be there, and what the read established is that it may not be
/// fetched.
const BUNDLE_DENIED_CODES: &[&str] = &["AccessDenied", "AllAccessDisabled"];

/// What the trust store built from `source` can establish about its CA certificates bundle,
/// read through the emulator's own Amazon S3.
///
/// The address is checked for being an address at all BEFORE anything is asked of the store:
/// a blank bucket or key, or a bucket name Amazon S3 holds no bucket under
/// ([`::service_common::s3_bucket_name::is_well_formed`]), names no object anywhere, so no
/// read of it could establish anything and the argument itself is what is invalid.
pub async fn resolve_ca_certificates_bundle(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    source: &CaCertificatesBundleSource,
) -> CaCertificatesBundle {
    let CaCertificatesBundleSource::CaCertificatesBundleS3Location(location) = source;
    let bucket = location.bucket.trim();
    let key = location.key.trim();
    if bucket.is_empty() || key.is_empty() || !::service_common::s3_bucket_name::is_well_formed(bucket)
    {
        ::tracing::debug!(
            bucket = ?location.bucket,
            key = ?location.key,
            "a CA certificates bundle source addresses no Amazon S3 object"
        );
        return CaCertificatesBundle::Invalid;
    }

    // The client is addressed to the Region the LOCATION names — "The location's Region",
    // the Region the bundle is held in — and it is built by the one shared constructor, so
    // this hop is configured exactly as every other emulated service's hop into the object
    // store is. The transport is in-process: the endpoint it carries is addressing for the
    // emulator's own router, not a host anything connects to.
    let client = ::service_common::s3_client::s3_client(ctx.callback(), &location.region);
    let mut read = client
        .get_object()
        .bucket(bucket)
        .key(key)
        // Ranged, so at most the bound plus the one byte that shows the object is past it is
        // ever buffered.
        .range(format!("bytes=0-{MAX_CA_CERTIFICATES_BUNDLE_BYTES}"));
    if let Some(version) = location.version.as_deref().map(str::trim) {
        if !version.is_empty() {
            read = read.version_id(version);
        }
    }
    let answered = match read.send().await {
        Ok(answered) => answered,
        Err(error) => return answered_refusal(&error),
    };
    let body = match answered.body.collect().await {
        Ok(body) => body.into_bytes(),
        Err(error) => {
            ::tracing::warn!(
                error = %error,
                "a CA certificates bundle did not arrive in full from the emulator's Amazon S3"
            );
            return CaCertificatesBundle::Unanswered;
        }
    };
    if body.len() > MAX_CA_CERTIFICATES_BUNDLE_BYTES {
        ::tracing::debug!(
            bytes = body.len(),
            "a CA certificates bundle is larger than this emulator reads"
        );
        return CaCertificatesBundle::Invalid;
    }
    match ca_certificates_in(&body) {
        0 => CaCertificatesBundle::Invalid,
        counted => CaCertificatesBundle::Counted(counted),
    }
}

/// How many CA certificates the bundle `body` holds: every PEM block labelled
/// [`CERTIFICATE_PEM_TAG`] whose contents decode as an X.509 certificate.
///
/// Both layers are read by the crate that owns the format — `pem` for the blocks,
/// `x509-cert` for the DER inside one — so a block that only LOOKS like a certificate (a
/// truncated structure, another kind of DER, a key) is not counted as one. Bytes that are no
/// PEM at all hold no certificate, which is the zero this answers with.
pub fn ca_certificates_in(body: &[u8]) -> i32 {
    let counted = ::pem::parse_many(body)
        .unwrap_or_default()
        .iter()
        .filter(|block| block.tag() == CERTIFICATE_PEM_TAG)
        .filter(|block| ::x509_cert::Certificate::from_der(block.contents()).is_ok())
        .count();
    i32::try_from(counted).unwrap_or(i32::MAX)
}

/// What the object store's refusal `error` establishes about the bundle.
///
/// An answer naming one of [`BUNDLE_ABSENT_CODES`] or [`BUNDLE_DENIED_CODES`] is the store
/// itself saying it holds nothing at that address, or that the read is not permitted — each
/// a fact about the source the request named. Any OTHER answer the store gave about that
/// object is one no bundle can be read through, so the bundle is not usable either: an EMPTY
/// object answers `InvalidRange` to the ranged read above, its first byte not existing.
///
/// A failure carrying no error CODE is not an answer about the object at all — there was no
/// object store to ask, or the call could not be dispatched — so it establishes nothing
/// about the bundle and the caller's request is not what is at fault. It is warned about,
/// because a deployment where no trust store's bundle can be read is worth noticing.
fn answered_refusal<E: ProvideErrorMetadata + ::std::fmt::Debug>(
    error: &E,
) -> CaCertificatesBundle {
    let Some(code) = error.code() else {
        ::tracing::warn!(
            error = ?error,
            "nothing answered a read of a CA certificates bundle: this emulator has no Amazon \
             S3 to read one from"
        );
        return CaCertificatesBundle::Unanswered;
    };
    if BUNDLE_ABSENT_CODES.contains(&code) {
        return CaCertificatesBundle::Absent;
    }
    if BUNDLE_DENIED_CODES.contains(&code) {
        return CaCertificatesBundle::Denied;
    }
    ::tracing::warn!(
        code = ?code,
        error = ?error,
        "the emulator's Amazon S3 answered a CA certificates bundle read with something no \
         bundle can be read through"
    );
    CaCertificatesBundle::Invalid
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failure carrying the error metadata an object store's answer would carry, so the
    /// mapping below is driven by a CODE rather than by a real S3 round trip.
    #[derive(Debug)]
    struct Answered(::aws_smithy_types::error::ErrorMetadata);

    impl Answered {
        /// An answer naming `code`, as the object store names one.
        fn with_code(code: &str) -> Self {
            Self(
                ::aws_smithy_types::error::ErrorMetadata::builder()
                    .code(code)
                    .build(),
            )
        }

        /// A failure that names no code at all — nothing answered.
        fn unanswered() -> Self {
            Self(::aws_smithy_types::error::ErrorMetadata::builder().build())
        }
    }

    impl ProvideErrorMetadata for Answered {
        fn meta(&self) -> &::aws_smithy_types::error::ErrorMetadata {
            &self.0
        }
    }

    /// A PEM block carrying `contents` under `tag`, rendered the way a bundle carries one.
    fn block(tag: &str, contents: &[u8]) -> String {
        ::pem::encode(&::pem::Pem::new(tag.to_string(), contents.to_vec()))
    }

    /// RULE: what is counted is PEM blocks labelled `CERTIFICATE` whose contents decode as
    /// an X.509 certificate — so prose, another block label and a block that is not a
    /// certificate all count for nothing.
    #[test]
    fn only_decodable_certificate_blocks_are_counted() {
        let rows: &[(&str, String, i32)] = &[
            ("bytes that are no PEM at all", "not a bundle".to_string(), 0),
            (
                "a PEM block that is not a certificate",
                block("PUBLIC KEY", b"0123456789"),
                0,
            ),
            (
                "a CERTIFICATE block holding no X.509 structure",
                block(CERTIFICATE_PEM_TAG, b"0123456789"),
                0,
            ),
        ];
        for (case, body, expected) in rows {
            let actual = ca_certificates_in(body.as_bytes());
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected} certificates, got {actual}"
            );
        }
    }

    /// RULE: what the object store ANSWERED decides the outcome — the codes in which it says
    /// it holds nothing are an absent bundle, the codes in which it says the read is not
    /// permitted are a denied one, any other answer about that object is a bundle no trust
    /// store can be built from, and a failure naming no code at all answered nothing.
    #[test]
    fn the_outcome_is_the_answer_the_object_store_gave() {
        let rows: &[(&str, Answered, CaCertificatesBundle)] = &[
            (
                "no such bucket",
                Answered::with_code("NoSuchBucket"),
                CaCertificatesBundle::Absent,
            ),
            (
                "no such key",
                Answered::with_code("NoSuchKey"),
                CaCertificatesBundle::Absent,
            ),
            (
                "no such version",
                Answered::with_code("NoSuchVersion"),
                CaCertificatesBundle::Absent,
            ),
            (
                "the read is not permitted",
                Answered::with_code("AccessDenied"),
                CaCertificatesBundle::Denied,
            ),
            (
                "every access is disabled",
                Answered::with_code("AllAccessDisabled"),
                CaCertificatesBundle::Denied,
            ),
            (
                "an empty object, whose first byte does not exist",
                Answered::with_code("InvalidRange"),
                CaCertificatesBundle::Invalid,
            ),
            (
                "an answer nothing here anticipated",
                Answered::with_code("PermanentRedirect"),
                CaCertificatesBundle::Invalid,
            ),
            (
                "a failure naming no code at all",
                Answered::unanswered(),
                CaCertificatesBundle::Unanswered,
            ),
        ];
        for (case, error, expected) in rows {
            let actual = answered_refusal(error);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
