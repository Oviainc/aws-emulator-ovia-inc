// SPDX-License-Identifier: CC0-1.0

//! The OTHER half of the reserved tag-key rule: the tags AWS sets from inside itself.
//!
//! RULE: a tag key in the namespace AWS keeps for its own tags (`aws:`) is AWS's own — so a
//! request a CLIENT makes may not write one (`reserved_tag_key_prefix.rs` pins that half),
//! while the AWS service that owns such a tag sets it through no public API at all. In this
//! emulator one service reaches another only over the in-process callback, so a tag set that
//! arrives on that path may carry a reserved key, and every later read of the resource
//! reports it beside the client's own tags.
//!
//! The rows differ along what the rule generalises over: the reserved NAMESPACE the key sits
//! in (the stack provenance a CloudFormation deployment stamps on each resource it creates,
//! and another service's own namespace the issue did not cite), the bare prefix as the whole
//! key, a key that merely CONTAINS the prefix without being in the namespace, an ordinary
//! client key, and the PATH the tag set arrives on — the modelled `Tagging` payload of
//! `PutBucketTagging` and of `PutObjectTagging`, the `x-amz-tagging` header a write carries,
//! and `CreateBucketConfiguration$Tags` on the create itself — because the reservation is a
//! rule of the tag set, not of one operation.
//!
//! Every case drives real HTTP through the router, so what is asserted is what a client
//! sees; the in-process path is the one the framework's callback transport stamps
//! (`smithy_server_runtime::CallbackRecursionDepth`), which is a request EXTENSION and
//! therefore cannot arrive from a socket.

use smithy_server_runtime::{CallbackRecursionDepth, MultiServiceRouter};

use super::bucket_lifecycle::{router, Answer};
use super::bucket_tagging::{tag, tag_set_of, tagging_payload};

/// The credential scope that routes a request to this service. Nothing here is verified:
/// the harness runs with request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The Regional endpoint host every request below is addressed to, so each request is
/// path-style.
const HOST: &str = "s3.us-east-1.amazonaws.com";

/// The XML namespace a client's writer puts on a `CreateBucketConfiguration`.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The code S3 refuses a tag that does not pass input validation with.
const INVALID_TAG: &str = "InvalidTag";

/// The fixed text S3's BUCKET tagging path states for a key in the reserved namespace.
const SYSTEM_TAGS_MESSAGE: &str = "System tags cannot be added/updated by requester";

/// The fixed text its OBJECT tagging path states for that same key.
const RESERVED_PREFIX_MESSAGE: &str = "Your TagKey cannot be prefixed with aws:";

/// Which path a request arrives on, which is what decides whether a reserved tag key may be
/// stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Path {
    /// A client's own request, arriving from a socket: it carries no in-process stamp.
    Caller,
    /// A request another emulated service made in-process, which the framework's callback
    /// transport stamps.
    InProcess,
}

/// Dispatch one request on `path`.
///
/// The same request either way but for the stamp: an in-process call carries the
/// [`CallbackRecursionDepth`] extension the callback transport puts on it, which is the one
/// thing about a request a client cannot choose for itself.
async fn send_on(
    router: &MultiServiceRouter,
    path: Path,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{HOST}{uri}"))
        .header("host", HOST)
        .header("authorization", AUTHORIZATION);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    if path == Path::InProcess {
        builder = builder.extension(CallbackRecursionDepth(1));
    }
    let request = builder
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// One row: a tag key, and whether it is in the namespace AWS keeps for itself.
struct Row {
    case: &'static str,
    key: &'static str,
    /// Whether the key is in the reserved namespace, so a client's request may not name it
    /// and an in-process one may.
    reserved: bool,
}

/// The rows the rule generalises over.
///
/// The three stack-provenance keys are the ones CloudFormation records on every resource it
/// provisions (this emulator's `service-com-amazonaws-cloudformation-cloudformation`
/// `state::provisioning::{STACK_NAME_TAG, STACK_ID_TAG, LOGICAL_ID_TAG}`, which are AWS's own
/// `aws:cloudformation:*` keys); the Service Catalog key is another service's own namespace,
/// which the issue did not cite. The last three rows are the boundary: the bare prefix is
/// still the namespace, a key that merely contains `aws:` is not in it, and an ordinary key
/// is nobody's but the client's. The prefix itself is the published one
/// (`service_com_amazonaws_s3_amazons3::state::RESERVED_TAG_KEY_PREFIX`).
const ROWS: &[Row] = &[
    Row {
        case: "the stack a resource belongs to",
        key: "aws:cloudformation:stack-name",
        reserved: true,
    },
    Row {
        case: "that stack's id",
        key: "aws:cloudformation:stack-id",
        reserved: true,
    },
    Row {
        case: "the resource's logical id in the template",
        key: "aws:cloudformation:logical-id",
        reserved: true,
    },
    Row {
        case: "another service's own namespace",
        key: "aws:servicecatalog:provisioningPrincipalArn",
        reserved: true,
    },
    Row {
        case: "the bare reserved prefix as the whole key",
        key: "aws:",
        reserved: true,
    },
    Row {
        case: "a key that merely contains the prefix",
        key: "not-aws:prefixed",
        reserved: false,
    },
    Row {
        case: "a client's own key",
        key: "env",
        reserved: false,
    },
];

/// The value every row writes. A tag VALUE is a value either way: the rule is about the KEY,
/// so one value serves every row.
const VALUE: &str = "probe-value";

/// A key a client owns, written beside the row's key so each case also shows the other half
/// of the set travelled.
const CLIENT_KEY: &str = "team";

/// The tag set each row sends: the row's key and a client's own, in that order.
fn written(row: &Row) -> Vec<(String, String)> {
    vec![tag(row.key, VALUE), tag(CLIENT_KEY, "platform")]
}

/// Create the bucket a case works in, as a client, so nothing about the fixture depends on
/// the path under test.
async fn given_bucket(router: &MultiServiceRouter, case: &str, bucket: &str) {
    let answer = send_on(router, Path::Caller, "PUT", &format!("/{bucket}"), &[], b"").await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the fixture bucket must be created, got {} with body {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// Write an object a case can tag, as a client, for the same reason.
async fn given_object(router: &MultiServiceRouter, case: &str, bucket: &str, key: &str) {
    let answer = send_on(
        router,
        Path::Caller,
        "PUT",
        &format!("/{bucket}/{key}"),
        &[("content-type", "text/plain")],
        b"payload",
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the fixture object must be written, got {} with body {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// `PutBucketTagging` and `PutObjectTagging`: the reserved key is refused to a client and
/// stored for a service, and either way the read-back reports exactly what the write left.
#[tokio::test]
async fn the_tagging_payload_takes_a_reserved_key_only_on_the_in_process_path() {
    for row in ROWS {
        let case = row.case;
        for path in [Path::Caller, Path::InProcess] {
            let admitted = !row.reserved || path == Path::InProcess;
            let router = router();
            let bucket = "reserved-key-origin";
            given_bucket(&router, case, bucket).await;
            given_object(&router, case, bucket, "tagged.csv").await;

            // The set already in force, written as a client, which a refused write must
            // leave exactly as it is.
            let established = vec![tag("keep", "me")];
            for (call, uri) in [
                ("PutBucketTagging", format!("/{bucket}?tagging")),
                ("PutObjectTagging", format!("/{bucket}/tagged.csv?tagging")),
            ] {
                let answer = send_on(
                    &router,
                    Path::Caller,
                    "PUT",
                    &uri,
                    &[("content-type", "application/xml")],
                    tagging_payload(&established).as_bytes(),
                )
                .await;
                assert!(
                    answer.status == 200 || answer.status == 204,
                    "case {case:?} on {path:?}: the setup {call} must succeed, got {} with \
                     body {:?}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
            }

            let set = written(row);
            for (call, uri, read, message) in [
                (
                    "PutBucketTagging",
                    format!("/{bucket}?tagging"),
                    format!("/{bucket}?tagging"),
                    SYSTEM_TAGS_MESSAGE,
                ),
                (
                    "PutObjectTagging",
                    format!("/{bucket}/tagged.csv?tagging"),
                    format!("/{bucket}/tagged.csv?tagging"),
                    RESERVED_PREFIX_MESSAGE,
                ),
            ] {
                let answer = send_on(
                    &router,
                    path,
                    "PUT",
                    &uri,
                    &[("content-type", "application/xml")],
                    tagging_payload(&set).as_bytes(),
                )
                .await;
                if admitted {
                    assert!(
                        answer.status == 200 || answer.status == 204,
                        "case {case:?} on {path:?}: {call} must store the key, got {} with \
                         body {:?}",
                        answer.status,
                        String::from_utf8_lossy(&answer.body)
                    );
                } else {
                    assert_eq!(
                        (answer.status, answer.error_code(), answer.error_message()),
                        (400, INVALID_TAG.to_string(), message.to_string()),
                        "case {case:?} on {path:?}: {call} must refuse the key, got {} with \
                         body {:?}",
                        answer.status,
                        String::from_utf8_lossy(&answer.body)
                    );
                    assert_eq!(
                        answer.element("TagKey").as_deref(),
                        Some(row.key),
                        "case {case:?} on {path:?}: {call} must name the offending key, got \
                         {:?}",
                        String::from_utf8_lossy(&answer.body)
                    );
                }

                // What the resource holds afterwards: the whole written set where the write
                // was taken, and the established set where it was refused.
                let expected = if admitted {
                    set.clone()
                } else {
                    established.clone()
                };
                let held = send_on(&router, Path::Caller, "GET", &read, &[], b"").await;
                assert_eq!(
                    held.status, 200,
                    "case {case:?} on {path:?}: the read after {call} must be served, got {} \
                     with body {:?}",
                    held.status,
                    String::from_utf8_lossy(&held.body)
                );
                assert_eq!(
                    tag_set_of(&held),
                    expected,
                    "case {case:?} on {path:?}: after {call} the tag set must be {expected:?}, \
                     got {:?}",
                    tag_set_of(&held)
                );
            }
        }
    }
}

/// The `x-amz-tagging` header of a write and `CreateBucketConfiguration$Tags` of a create:
/// the same line, on the two paths that carry a tag set alongside something else.
#[tokio::test]
async fn a_tag_set_carried_beside_a_write_follows_the_same_rule() {
    for row in ROWS {
        let case = row.case;
        for path in [Path::Caller, Path::InProcess] {
            let admitted = !row.reserved || path == Path::InProcess;
            let router = router();
            let bucket = "reserved-key-on-write";
            given_bucket(&router, case, bucket).await;
            let set = written(row);

            // "The tag-set must be encoded as URL Query parameters" —
            // `PutObjectRequest$Tagging`.
            let header = set
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<String>>()
                .join("&");
            let answer = send_on(
                &router,
                path,
                "PUT",
                &format!("/{bucket}/written.csv"),
                &[("content-type", "text/plain"), ("x-amz-tagging", &header)],
                b"payload",
            )
            .await;
            if admitted {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} on {path:?}: the write must store the key, got {} with body \
                     {:?}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                let held = send_on(
                    &router,
                    Path::Caller,
                    "GET",
                    &format!("/{bucket}/written.csv?tagging"),
                    &[],
                    b"",
                )
                .await;
                assert_eq!(
                    tag_set_of(&held),
                    set,
                    "case {case:?} on {path:?}: the written object must carry {set:?}, got {:?}",
                    tag_set_of(&held)
                );
            } else {
                assert_eq!(
                    (answer.status, answer.error_code(), answer.error_message()),
                    (
                        400,
                        INVALID_TAG.to_string(),
                        RESERVED_PREFIX_MESSAGE.to_string()
                    ),
                    "case {case:?} on {path:?}: the write must refuse the key, got {} with \
                     body {:?}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
            }

            // "An array of tags that you can apply to the bucket that you're creating"
            // (`CreateBucketConfiguration$Tags`): the wrapped list layout the model
            // declares, a `TagSet` whose member is spelled `Tag`.
            let mut configuration = format!("<CreateBucketConfiguration xmlns=\"{XMLNS}\"><Tags>");
            for (key, value) in &set {
                configuration.push_str(&format!(
                    "<Tag><Key>{key}</Key><Value>{value}</Value></Tag>"
                ));
            }
            configuration.push_str("</Tags></CreateBucketConfiguration>");
            let created = "reserved-key-at-create";
            let answer = send_on(
                &router,
                path,
                "PUT",
                &format!("/{created}"),
                &[("content-type", "application/xml")],
                configuration.as_bytes(),
            )
            .await;
            let read = send_on(
                &router,
                Path::Caller,
                "GET",
                &format!("/{created}?tagging"),
                &[],
                b"",
            )
            .await;
            if admitted {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} on {path:?}: the create must store the key, got {} with \
                     body {:?}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                assert_eq!(
                    tag_set_of(&read),
                    set,
                    "case {case:?} on {path:?}: the created bucket must carry {set:?}, got {:?}",
                    tag_set_of(&read)
                );
            } else {
                assert_eq!(
                    (answer.status, answer.error_code(), answer.error_message()),
                    (
                        400,
                        INVALID_TAG.to_string(),
                        SYSTEM_TAGS_MESSAGE.to_string()
                    ),
                    "case {case:?} on {path:?}: the create must refuse the key, got {} with \
                     body {:?}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                // A refused create leaves no bucket behind, so the tag read of it is the
                // service's own `NoSuchBucket`.
                assert_eq!(
                    (read.status, read.error_code()),
                    (404, "NoSuchBucket".to_string()),
                    "case {case:?} on {path:?}: a refused create must create nothing, got {} \
                     with body {:?}",
                    read.status,
                    String::from_utf8_lossy(&read.body)
                );
            }
        }
    }
}
