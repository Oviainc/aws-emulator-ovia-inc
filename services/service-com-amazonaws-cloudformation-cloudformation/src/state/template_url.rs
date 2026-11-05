// SPDX-License-Identifier: CC0-1.0

//! Resolving a template submitted BY REFERENCE: the Amazon S3 object a `TemplateURL`
//! addresses, read so the operation can go on exactly as it would with an inline
//! `TemplateBody`.
//!
//! ## The rule this module implements
//!
//! A template reaches CloudFormation one of two ways: inline, as the request's own
//! `TemplateBody`, or by reference, as a `TemplateURL` — "the URL of the file that contains
//! the template. The URL must point to a template located in an Amazon S3 bucket …
//! The location for an Amazon S3 bucket must start with `https://`". The two are the same
//! template: the reference says WHERE the body is, not what it means, so an operation that
//! takes either resolves the reference to the body it addresses and is then held to every
//! rule a submitted body is held to — its form, its quotas, its capabilities, its
//! transforms.
//!
//! The object is read through the in-process callback into this same emulator, exactly as
//! [`super::include`] reads an `AWS::Include` snippet and [`super::provisioning`] creates a
//! stack's resources: the template a client staged with its own `PutObject` is the one the
//! stack deploys. NOTHING here opens a socket or leaves the host — the URL's host is
//! parsed for the bucket it names and is never connected to, so no caller-supplied text
//! reaches a destination.
//!
//! ## What the URL may spell
//!
//! The URL is read by the `url` crate — its scheme, host, path and query — rather than by a
//! parser of this service's own. Both documented Amazon S3 addressing styles name the same
//! object, so both resolve to the same bucket and key: virtual-hosted
//! (`https://<bucket>.s3.<region>.amazonaws.com/<key>`) and path-style
//! (`https://s3.<region>.amazonaws.com/<bucket>/<key>`), with the `?versionId=` a versioned
//! object is addressed by. A host that names no S3 service label at all is read path-style
//! too: a client addressing this emulator through an endpoint of its own still names its
//! bucket in the first path segment, and the host is not what the object is read from.
//!
//! A member whose own model admits the direct `s3://<bucket>/<key>` form beside those two
//! ([`addressed_object`]) reads its object through the SAME addressing, so no operation
//! spells an Amazon S3 address of its own; [`read_bytes`] is likewise the one read of a
//! caller-addressed object ([`read`] is its text form), bounded by what that object is for.
//!
//! ## What the read refuses
//!
//! An object Amazon S3 will not hand over is the `ValidationError` CloudFormation answers
//! for the request's own reference, and WHICH refusal it is decides how that is worded — two
//! sentences, each CloudFormation's own:
//!
//! * a key or a version the bucket does not hold is an object CloudFormation could see was
//!   ABSENT, reported as that, naming the bucket and key the URL addressed and the code
//!   Amazon S3 gave ([`unreadable_object`]);
//! * every OTHER refusal — a bucket that is not there, a read Amazon S3 will not serve, a
//!   body that stopped mid-stream — is reported as what Amazon S3 ANSWERED, under the prefix
//!   CloudFormation reports another service's refusal under and followed by the line that
//!   error is documented on ([`s3_error`]); where Amazon S3 worded nothing, what it
//!   classified the refusal as stands in its place, so no refusal this module cannot place
//!   becomes a claim that the object is not there.
//!
//! Text that addresses no object at all is refused before anything is read.
//!
//! ## Bounds
//!
//! The object is caller-supplied, so at most [`super::TEMPLATE_BODY_MAX_BYTES`] of it are
//! held — the one ceiling this service holds every template body to, and the size past
//! which its template reader ([`super::template::document`]) will not read a document at
//! all. A larger object is refused with the wording that ceiling owns
//! ([`super::template_body_too_long`]) rather than read into a body nothing can process.

/// The scheme a `TemplateURL` must carry: "The location for an Amazon S3 bucket must start
/// with `https://`. URLs from S3 static websites are not supported."
const HTTPS_SCHEME: &str = "https";

/// The host label that marks the Amazon S3 service in either addressing style: `s3` on its
/// own (`bucket.s3.us-east-1.amazonaws.com`, `s3.us-east-1.amazonaws.com`) or the legacy
/// region-suffixed spelling (`bucket.s3-us-east-1.amazonaws.com`).
const S3_LABEL: &str = "s3";
const S3_LABEL_PREFIX: &str = "s3-";

/// The query parameter that addresses one version of a versioned object.
const VERSION_ID: &str = "versionId";

/// The refusal for a `TemplateURL` whose scheme is not the documented one.
const UNSUPPORTED_URL: &str = "TemplateURL must be a supported URL.";

/// The refusal for text that addresses no Amazon S3 object at all, so no object was even
/// asked for.
const NOT_A_READABLE_OBJECT: &str =
    "TemplateURL must reference a valid S3 object to which you have access.";

/// The error code that stands in for one a refused read did not name: the object was not
/// read, and nothing on the other end identified the problem.
const UNIDENTIFIED_FAILURE: &str = "InternalFailure";

/// The refusal for an object AMAZON S3 SAID IS NOT THERE, worded as CloudFormation words
/// it: the bucket and key the request's own URL addressed, and the code Amazon S3 named.
///
/// This is the sentence for an absence CloudFormation could SEE — a key or a version the
/// bucket does not hold ([`ABSENT_OBJECT_CODES`]). A refusal that reports no absence is
/// worded by [`s3_error`] instead, because it establishes nothing about the object.
fn unreadable_object(object: &Object, code: &str) -> String {
    format!(
        "S3 object does not exist. Bucket: {}, Key: {}. Error: {code}",
        object.bucket, object.key
    )
}

/// The line CloudFormation puts after an Amazon S3 refusal it reports, pointing at the
/// Amazon S3 error-response documentation the code it carries is listed in.
const S3_ERROR_REFERENCE: &str =
    "For more information check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html";

/// The `ValidationError` message CloudFormation answers for a read Amazon S3 refused without
/// establishing that the object is absent: what AMAZON S3 ANSWERED, under the prefix
/// CloudFormation reports another service's refusal under, and then the line that error is
/// documented on.
///
/// The message is Amazon S3's, not this module's: CloudFormation reports what it was told by
/// the service it asked, so nothing here names the bucket, the key or the error code — a
/// read that was refused says only that it was refused.
fn s3_error(message: &str) -> String {
    format!("S3 error: {message}\n{S3_ERROR_REFERENCE}")
}

/// The Amazon S3 error codes that report an object the bucket does NOT HOLD — a key, or a
/// version of one — which is the one refusal CloudFormation words as the object's absence
/// ([`unreadable_object`]) rather than as what Amazon S3 answered.
const ABSENT_OBJECT_CODES: &[&str] = &["NoSuchKey", "NoSuchVersion"];

/// What Amazon S3 answered a refused read with, as the message CloudFormation reports for
/// it: an object the bucket does not hold as that object's absence, and every other refusal
/// — a bucket that is not there, a read Amazon S3 will not serve — as the sentence Amazon S3
/// itself worded for it.
///
/// A refusal that carried NO message is reported as what it DID identify: the code, which is
/// Amazon S3's own classification of it, and [`UNIDENTIFIED_FAILURE`] where it carried
/// neither — the read was not served, and nothing on the other end identified the problem.
/// Only a code that reports an absence is reported AS an absence, so no refusal is turned
/// into a claim that the object is not there.
fn refused_read_message(object: &Object, code: Option<&str>, message: Option<&str>) -> String {
    match (code, message) {
        (Some(code), _) if ABSENT_OBJECT_CODES.contains(&code) => unreadable_object(object, code),
        (_, Some(message)) => s3_error(message),
        (Some(code), None) => s3_error(code),
        (None, None) => s3_error(UNIDENTIFIED_FAILURE),
    }
}

/// The Amazon S3 object a `TemplateURL` addresses.
#[derive(Debug, PartialEq, Eq)]
pub struct Object {
    pub bucket: String,
    pub key: String,
    /// The object VERSION the URL addresses, where it addresses one.
    pub version: Option<String>,
}

/// The body the `TemplateURL` addresses, read from this emulator's Amazon S3.
///
/// `Err(message)` is the `ValidationError` message the operation answers: the URL is the
/// caller's own, so a reference that names no readable object is the REQUEST's problem, not
/// a template-format one.
///
/// `region` is the Region the request is signed for, which is the scope the in-process call
/// is signed for — the same one [`super::include`] reads a snippet in.
pub async fn fetch(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    url: &str,
) -> Result<String, String> {
    let object = object(url)?;
    read(
        ctx,
        region,
        &object,
        super::TEMPLATE_BODY_MAX_BYTES,
        &super::template_body_too_long(),
    )
    .await
}

/// The TEXT of one Amazon S3 object, read from this emulator's own Amazon S3 through the
/// in-process callback — the one read of a caller-addressed object this service makes.
///
/// `max_bytes` is the ceiling the caller holds the object to and `too_long` the refusal past
/// it: an object a request addresses is caller-sized, so every read of one is bounded by what
/// the object is FOR — a template body by the documented `TemplateBody` maximum, a list of
/// identifiers by how many of them the operation may be given.
///
/// `Err(message)` is the `ValidationError` message the operation answers: the URL is the
/// caller's own, so an object Amazon S3 will not hand over is the REQUEST's problem.
pub async fn read(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    object: &Object,
    max_bytes: usize,
    too_long: &str,
) -> Result<String, String> {
    let body = read_bytes(ctx, region, object, max_bytes, too_long).await?;
    // What the reference addressed is TEXT; an object that is not valid UTF-8 carries none, so
    // it is not what this reference addressed. The object itself WAS handed over, so this
    // reports the read that could not be served rather than the object's absence.
    String::from_utf8(body).map_err(|_| s3_error(UNIDENTIFIED_FAILURE))
}

/// The BYTES of one Amazon S3 object, read the same way [`read`] reads its text, for a member
/// whose object is not text at all: the ZIP archive an extension's schema handler package is.
///
/// Bounded by the same ceiling, and refused with the same wording, as every other read of a
/// caller-addressed object: the object is the caller's, so the caller's own purpose for it
/// says what it may weigh.
pub async fn read_bytes(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    object: &Object,
    max_bytes: usize,
    too_long: &str,
) -> Result<Vec<u8>, String> {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let read = super::provisioner::s3_client(ctx, region)
        .get_object()
        .bucket(&object.bucket)
        .key(&object.key)
        .set_version_id(object.version.clone())
        .send()
        .await
        .map_err(|error| {
            // The object the caller's own URL addressed was not handed over: which refusal
            // Amazon S3 gave is what decides how CloudFormation words it
            // ([`refused_read_message`]).
            refused_read_message(object, error.code(), error.message())
        })?;
    // The length the object REPORTS, before a single byte of it is held.
    if read
        .content_length()
        .is_some_and(|length| length > max_bytes as i64)
    {
        return Err(too_long.to_string());
    }
    let body = read
        .body
        .collect()
        .await
        // The object WAS handed over and the body then stopped: nothing established that
        // it is absent, so the refusal is reported as the refusal it was.
        .map_err(|_| s3_error(UNIDENTIFIED_FAILURE))?
        .into_bytes();
    if body.len() > max_bytes {
        return Err(too_long.to_string());
    }
    Ok(body.to_vec())
}

/// The bucket, key and object version a `TemplateURL` addresses.
///
/// The URL itself is read by the `url` crate — scheme, host, path and query — so nothing
/// here parses caller-supplied URL text by hand; what is read HERE is which Amazon S3
/// addressing style the host spells.
///
/// `Err(message)` is the refusal for text that addresses no object: a scheme the
/// documentation does not allow, or an https URL carrying no bucket and key.
pub fn object(template_url: &str) -> Result<Object, String> {
    let parsed = ::url::Url::parse(template_url).map_err(|_| UNSUPPORTED_URL.to_string())?;
    if parsed.scheme() != HTTPS_SCHEME {
        return Err(UNSUPPORTED_URL.to_string());
    }
    hosted_object(&parsed).ok_or_else(|| NOT_A_READABLE_OBJECT.to_string())
}

/// The Amazon S3 object `url` addresses, for a member whose OWN model constrains which
/// schemes it may carry: the `s3://<bucket>/<key>` form beside the two http(s) addressing
/// styles [`object`] reads.
///
/// `None` for text that addresses no object at all, so the caller words that refusal for the
/// member it is about — the scheme itself is the boundary's to refuse where the model carries
/// the pattern that states it.
pub fn addressed_object(url: &str) -> Option<Object> {
    // `s3://<bucket>/<key>` is read by the ONE reading of that spelling this crate holds
    // ([`super::template::s3_uri_parts`]), so an object addressed that way is the same object
    // here as everywhere else a URI may carry it.
    if let Some((bucket, key, version)) = super::template::s3_uri_parts(url) {
        return Some(Object {
            bucket: bucket.to_string(),
            key: key.to_string(),
            version: version.map(str::to_string),
        });
    }
    hosted_object(&::url::Url::parse(url).ok()?)
}

/// The bucket and key an http(s) URL names in either documented Amazon S3 addressing style,
/// or `None` for one that carries no bucket and key at all.
fn hosted_object(parsed: &::url::Url) -> Option<Object> {
    // The host is read for the BUCKET it names and for nothing else: nothing here connects
    // to it.
    let host = parsed.host_str()?;
    // Virtual-hosted addressing puts the bucket in the labels BEFORE the S3 service label
    // (a bucket name may itself carry dots, so every label before it belongs to it);
    // path-style addressing — the S3 label first, or a host naming no S3 label at all —
    // puts it in the first path segment.
    let labels: Vec<&str> = host.split('.').collect();
    let service_label = labels
        .iter()
        .position(|label| *label == S3_LABEL || label.starts_with(S3_LABEL_PREFIX));
    let path = parsed.path().trim_start_matches('/');
    let (bucket, key) = match service_label {
        Some(0) | None => match path.split_once('/') {
            Some((bucket, key)) => (bucket.to_string(), key.to_string()),
            None => return None,
        },
        // Taken rather than indexed: the bucket is every label BEFORE the service one, and
        // nothing here slices caller-supplied text by position.
        Some(at) => (
            labels
                .iter()
                .take(at)
                .copied()
                .collect::<Vec<&str>>()
                .join("."),
            path.to_string(),
        ),
    };
    // The key as it was STORED: a path carries it percent-encoded, which is the spelling a
    // client's own `PutObject` URL carried too.
    let key = decoded(&key);
    if bucket.is_empty() || key.is_empty() {
        return None;
    }
    Some(Object {
        bucket,
        key,
        version: version_of(parsed),
    })
}

/// The object VERSION a URL addresses, where it addresses one.
///
/// "To retrieve a specific version, you must use the versionId subresource": the query the URL
/// carries, already decoded by the URL reader. A query naming anything else addresses the
/// object's current version.
fn version_of(parsed: &::url::Url) -> Option<String> {
    parsed
        .query_pairs()
        .find(|(name, _)| name == VERSION_ID)
        .map(|(_, value)| value.into_owned())
}

/// One percent-decoding of a URL path, through the crate's existing encoder.
///
/// Text that is not valid UTF-8 once decoded is left exactly as it was written: it is then
/// a key Amazon S3 holds nothing under, which the read refuses.
fn decoded(text: &str) -> String {
    ::percent_encoding::percent_decode_str(text)
        .decode_utf8()
        .map_or_else(|_| text.to_string(), |decoded| decoded.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both documented addressing styles, the legacy region-suffixed host, a bucket name
    /// carrying dots, an encoded key, a versioned object and an endpoint host that names no
    /// S3 label — every one of them addressing the same object in the same way.
    #[test]
    fn a_template_url_names_the_bucket_and_key_of_one_object() {
        let rows: &[(&str, &str, &str, &str, Option<&str>)] = &[
            (
                "virtual-hosted, regional",
                "https://assets.s3.us-west-2.amazonaws.com/templates/main.json",
                "assets",
                "templates/main.json",
                None,
            ),
            (
                "virtual-hosted, global",
                "https://assets.s3.amazonaws.com/main.yaml",
                "assets",
                "main.yaml",
                None,
            ),
            (
                "virtual-hosted, region-suffixed label",
                "https://assets.s3-eu-west-1.amazonaws.com/main.yaml",
                "assets",
                "main.yaml",
                None,
            ),
            (
                "virtual-hosted, a bucket name carrying dots",
                "https://my.cdk.assets.s3.us-east-1.amazonaws.com/a/b/c.json",
                "my.cdk.assets",
                "a/b/c.json",
                None,
            ),
            (
                "path-style, regional",
                "https://s3.us-east-1.amazonaws.com/assets/templates/main.json",
                "assets",
                "templates/main.json",
                None,
            ),
            (
                "path-style, global",
                "https://s3.amazonaws.com/assets/main.json",
                "assets",
                "main.json",
                None,
            ),
            (
                "a host naming no S3 label is read path-style",
                "https://localhost:9297/assets/main.json",
                "assets",
                "main.json",
                None,
            ),
            (
                "a percent-encoded key is the key as it was stored",
                "https://assets.s3.us-east-1.amazonaws.com/a%20b/main.json",
                "assets",
                "a b/main.json",
                None,
            ),
            (
                "a versioned object",
                "https://assets.s3.us-east-1.amazonaws.com/main.json?versionId=abc123",
                "assets",
                "main.json",
                Some("abc123"),
            ),
            (
                "a query that is not a version is not part of the key",
                "https://s3.us-east-1.amazonaws.com/assets/main.json?x-id=GetObject",
                "assets",
                "main.json",
                None,
            ),
        ];
        for (case, url, bucket, key, version) in rows {
            let parsed = object(url);
            assert_eq!(
                parsed.as_ref().map(|object| object.bucket.as_str()),
                Ok(*bucket),
                "case {case:?}: expected bucket {bucket:?}, got {parsed:?}"
            );
            assert_eq!(
                parsed.as_ref().map(|object| object.key.as_str()),
                Ok(*key),
                "case {case:?}: expected key {key:?}, got {parsed:?}"
            );
            assert_eq!(
                parsed.as_ref().map(|object| object.version.as_deref()),
                Ok(*version),
                "case {case:?}: expected version {version:?}, got {parsed:?}"
            );
        }
    }

    /// Text that addresses no object at all, each refused for what is wrong with it.
    #[test]
    fn a_url_that_addresses_no_object_is_refused() {
        let rows: &[(&str, &str, &str)] = &[
            (
                "a scheme the documentation does not allow",
                "s3://assets/main.json",
                UNSUPPORTED_URL,
            ),
            (
                "no scheme at all",
                "assets.s3.amazonaws.com/main.json",
                UNSUPPORTED_URL,
            ),
            (
                "an http URL",
                "http://assets.s3.amazonaws.com/main.json",
                UNSUPPORTED_URL,
            ),
            (
                "no path, so no key",
                "https://assets.s3.us-east-1.amazonaws.com",
                NOT_A_READABLE_OBJECT,
            ),
            (
                "an empty key",
                "https://assets.s3.us-east-1.amazonaws.com/",
                NOT_A_READABLE_OBJECT,
            ),
            (
                "path-style with a bucket but no key",
                "https://s3.us-east-1.amazonaws.com/assets",
                NOT_A_READABLE_OBJECT,
            ),
        ];
        for (case, url, refusal) in rows {
            let parsed = object(url);
            assert_eq!(
                parsed.as_ref().err().map(String::as_str),
                Some(*refusal),
                "case {case:?}: expected the refusal {refusal:?}, got {parsed:?}"
            );
        }
    }

    /// What Amazon S3 answered a refused read with, as the message CloudFormation reports
    /// for it — one row per refusal class: the object's absence as absence, a bucket that is
    /// not there and a read Amazon S3 will not serve as what it said, and a refusal carrying
    /// no message of its own (which no wire call can produce, so only this reaches it) as the
    /// absence sentence under the code it did carry.
    #[test]
    fn a_refused_read_is_reported_as_the_class_of_refusal_it_was() {
        let object = Object {
            bucket: "assets".to_string(),
            key: "templates/main.json".to_string(),
            version: None,
        };
        let rows: &[(&str, Option<&str>, Option<&str>, String)] = &[
            (
                "a key the bucket does not hold",
                Some("NoSuchKey"),
                Some("The specified key does not exist."),
                "S3 object does not exist. Bucket: assets, Key: templates/main.json. Error: \
                 NoSuchKey"
                    .to_string(),
            ),
            (
                "a version the bucket does not hold",
                Some("NoSuchVersion"),
                None,
                "S3 object does not exist. Bucket: assets, Key: templates/main.json. Error: \
                 NoSuchVersion"
                    .to_string(),
            ),
            (
                "a bucket that is not there",
                Some("NoSuchBucket"),
                Some("The specified bucket does not exist"),
                "S3 error: The specified bucket does not exist\nFor more information check \
                 http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html"
                    .to_string(),
            ),
            (
                "a read Amazon S3 will not serve",
                Some("AccessDenied"),
                Some("Access Denied"),
                "S3 error: Access Denied\nFor more information check \
                 http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html"
                    .to_string(),
            ),
            (
                "a refusal that classified itself but worded nothing",
                Some("AccessDenied"),
                None,
                "S3 error: AccessDenied\nFor more information check \
                 http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html"
                    .to_string(),
            ),
            (
                "a refusal that identified nothing at all",
                None,
                None,
                "S3 error: InternalFailure\nFor more information check \
                 http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html"
                    .to_string(),
            ),
        ];
        for (case, code, message, expected) in rows {
            let reported = refused_read_message(&object, *code, *message);
            assert_eq!(
                reported, *expected,
                "case {case:?}: expected {expected:?}, got {reported:?}"
            );
        }
    }
}
