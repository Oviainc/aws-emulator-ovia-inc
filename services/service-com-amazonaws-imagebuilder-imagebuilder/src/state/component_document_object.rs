// SPDX-License-Identifier: CC0-1.0

//! Where a component document named by `uri` comes from: the object Amazon S3 holds at the
//! address the request gave.
//!
//! `CreateComponent` takes the document "using exactly one of the following methods:
//! Inline, using the data property in the request body. A URL that points to a YAML
//! document file stored in Amazon S3, using the uri property in the request body", and
//! `uri` adds "This must be an S3 URL (s3://bucket/key), and you must have permission to
//! access the S3 bucket it points to". So half of the way a component arrives is an object
//! of the object store, and this emulator holds an Amazon S3 of its own — reached the way
//! any emulated service reaches another, through the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]), which routes IN PROCESS: no
//! socket, no outbound request, no host resolution. An object a caller staged with
//! `PutObject` is therefore the object Image Builder reads, and from there the document is
//! read exactly like an inline one ([`super::parse_component_document`]) — one document
//! answers the same whichever way it was named.
//!
//! The fetch is a ONE-TIME SNAPSHOT: what the object held when the create ran is stored on
//! the component record, so a later overwrite of the object cannot change what a read of
//! that build version reports. A component build version is immutable, and its document is
//! part of it.
//!
//! ## What grounds a refusal
//!
//! The two ways a fetch fails to produce a document are kept apart, because they are
//! different claims:
//!
//! * Amazon S3 ANSWERED that there is no such bucket or key, or that the read is not
//!   permitted ([`UNREADABLE_OBJECT_CODES`]) — a fact about the `uri` the request carried,
//!   so the create is refused as that parameter's value and nothing is minted;
//! * nothing answered the read at all — this emulator was assembled with no object store to
//!   ask — so nothing about the object is established, and the create reports a server-side
//!   failure rather than claiming the `uri` names nothing.
//!
//! HOW MUCH IS READ IS BOUNDED, and bounded twice: the documented ceiling for a component
//! document ([`MAX_DOCUMENT_BYTES`]) is checked against the length Amazon S3's own answer
//! DECLARES before a byte of the body is taken into this process, and against the bytes that
//! arrive as they arrive, so an answer that declared no length is bounded by the same ceiling
//! rather than buffered whole.

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

/// The scheme a component document URI carries, from the `uri` member's own documentation:
/// "This must be an S3 URL (s3://bucket/key)".
pub const COMPONENT_URI_SCHEME: &str = "s3://";

/// The most a component document this service reads from an object may hold.
///
/// The bound is the `uri` member's own: "If you use Amazon S3, you can specify component
/// content up to your service quota for component size, which is 64 KB by default."
pub const MAX_DOCUMENT_BYTES: usize = 64 * 1024;

/// The Amazon S3 error codes in which the store ANSWERS that it does not serve the object a
/// request named: the ones in which it reports that an address holds nothing, and the two in
/// which it reports that the read is not permitted. Every other answer is this emulator
/// failing to ask its own object store, which establishes nothing about the object.
const UNREADABLE_OBJECT_CODES: &[&str] = &[
    "NoSuchBucket",
    "NoSuchKey",
    "NoSuchVersion",
    "NotFound",
    "AccessDenied",
    "AllAccessDisabled",
];

/// What a fetch of the object a `uri` names established.
#[derive(Debug)]
pub enum DocumentObject {
    /// The document the object holds.
    Read(String),
    /// The `uri` is not an S3 URL of the form the member documents, so it addresses no
    /// object at all and nothing was asked.
    NotAnS3Url,
    /// The object store ANSWERED that it serves no such object, or that the read is not
    /// permitted: a fact about the `uri` the request carried.
    NotServed,
    /// The object was served but what it holds is not a component document; the clause says
    /// why.
    Rejected(String),
    /// Nothing established what the object holds: no object store answered the read.
    Unestablished,
}

/// The bucket and the key a component document `uri` names, when it is an S3 URL of the
/// form the member documents (`s3://bucket/key`): the scheme, a bucket, and a non-empty key.
#[must_use]
pub fn parse_s3_uri(uri: &str) -> Option<(&str, &str)> {
    let path = uri.strip_prefix(COMPONENT_URI_SCHEME)?;
    path.split_once('/')
        .filter(|(bucket, key)| !bucket.is_empty() && !key.is_empty())
}

/// The document the object at `uri` holds, read through the emulator's own Amazon S3.
pub async fn document_at(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    uri: &str,
) -> DocumentObject {
    let Some((bucket, key)) = parse_s3_uri(uri) else {
        return DocumentObject::NotAnS3Url;
    };
    let region = super::request_region(ctx.parts());
    object_document(&s3_client(ctx, &region), bucket, key).await
}

/// Read one object's document through `client`.
async fn object_document(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
) -> DocumentObject {
    let mut read = match client.get_object().bucket(bucket).key(key).send().await {
        Ok(read) => read,
        Err(error) => {
            let code = error.code().unwrap_or_default();
            if UNREADABLE_OBJECT_CODES.contains(&code) {
                // The store itself says the object is not there, or not readable by this
                // caller: the refusal is grounded in its answer.
                return DocumentObject::NotServed;
            }
            // Any other answer is this emulator failing to ask its own object store rather
            // than the store reporting an unreadable object — and it is worth a warning,
            // because a deployment with no Amazon S3 mounted to ask is one where no
            // uri-named component is ever created. The caller's own values are rendered in
            // their escaped `Debug` form, so a bucket or key carrying a newline cannot
            // forge a log line.
            ::tracing::warn!(
                bucket = ?bucket,
                key = ?key,
                code = ?error.code(),
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a component document could not be read from the emulator's Amazon S3"
            );
            return DocumentObject::Unestablished;
        }
    };
    // The documented ceiling, against what the store SAYS the object holds, before any of it
    // is read into this process.
    if let Some(declared) = read
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
    {
        if declared > MAX_DOCUMENT_BYTES {
            return DocumentObject::Rejected(too_large_requirement());
        }
    }
    // And against the bytes that actually arrive, which is the bound that holds for an answer
    // declaring no length at all: the stream is read a chunk at a time and dropped as soon as
    // what has arrived is past the ceiling, so no answer can put more than one chunk past it
    // into this process.
    let mut bytes: Vec<u8> = Vec::new();
    loop {
        match read.body.next().await {
            None => break,
            Some(Ok(chunk)) => {
                bytes.extend_from_slice(&chunk);
                if bytes.len() > MAX_DOCUMENT_BYTES {
                    return DocumentObject::Rejected(too_large_requirement());
                }
            }
            // The store answered, but the body did not arrive: nothing about the document
            // was established.
            Some(Err(error)) => {
                ::tracing::warn!(
                    bucket = ?bucket,
                    key = ?key,
                    error = %error,
                    "a component document's bytes did not arrive from the emulator's Amazon S3"
                );
                return DocumentObject::Unestablished;
            }
        }
    }
    match String::from_utf8(bytes) {
        Ok(document) => DocumentObject::Read(document),
        // A component document is YAML text; bytes that are not text are not one.
        Err(_) => DocumentObject::Rejected(
            "The component document must be UTF-8 text.".to_string(),
        ),
    }
}

/// The requirement clause an object holding more than the documented component size is
/// refused with.
fn too_large_requirement() -> String {
    format!("The component document must be at most {MAX_DOCUMENT_BYTES} bytes.")
}

/// The Amazon S3 client this service reads a component document object with, addressed to
/// `region`.
///
/// The transport is the emulator's own in-process callback, so the `endpoint_url` below is an
/// ADDRESS the in-process router reads rather than a host any resolver looks up: this client
/// reaches the emulator's own Amazon S3 and nothing else, whatever bucket or key a caller
/// names. The one caller-supplied value that reaches its authority is the signing region, and
/// it is narrowed before it gets here — [`super::request_region`] admits only a value shaped
/// like a Region identifier and otherwise answers the default Region.
///
/// There is nothing to retry, time out or watch for a stalled stream, each of which would
/// need a sleep implementation this client is not given. The same goes for caching the
/// identity: the credentials the callback hands over are used directly, and they only sign
/// this operation's in-process calls.
fn s3_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves, so the request addresses its
        // bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// One row: what a failing assertion calls it, the uri, and the address it names.
#[cfg(test)]
type UriRow = (&'static str, &'static str, Option<(&'static str, &'static str)>);

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a component document uri names a bucket and a non-empty key under the `s3`
    /// scheme, and nothing else is an S3 URL.
    #[test]
    fn only_an_s3_url_names_a_bucket_and_a_key() {
        let rows: &[UriRow] = &[
            (
                "a bare key",
                "s3://documents/component.yaml",
                Some(("documents", "component.yaml")),
            ),
            (
                "a nested prefix",
                "s3://documents/team/build/component.yaml",
                Some(("documents", "team/build/component.yaml")),
            ),
            ("no key at all", "s3://documents", None),
            ("an empty key", "s3://documents/", None),
            ("an empty bucket", "s3:///component.yaml", None),
            (
                "another scheme",
                "https://example.com/component.yaml",
                None,
            ),
            ("no scheme", "documents/component.yaml", None),
            ("the empty text", "", None),
        ];
        for (case, uri, expected) in rows {
            let actual = parse_s3_uri(uri);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
