// SPDX-License-Identifier: CC0-1.0

//! The MEDIA TYPE a response whose body is not XML is described by.
//!
//! The rule under test: a response's `content-type` describes the document it carries. One
//! S3 response is not an XML document at all — `GetBucketPolicy`'s body is "the bucket
//! policy as a JSON document" (`GetBucketPolicyOutput$Policy`, the operation's only output
//! member and its `@httpPayload`), so it is served as `application/json`. A client or a proxy
//! that dispatches on `content-type` will not parse a JSON body it was told is plain text,
//! and the policy document is exactly what such a client reads.
//!
//! The rows differ along the SHAPE of the policy document stored: compact, indented with
//! newlines and tabs, carrying several statements, and carrying non-ASCII text in a `Sid` —
//! so what is pinned is the media type of the response rather than anything about one
//! document. Each row also reads the document back byte for byte, because a media type is
//! only right about a body that was not re-serialized on the way out.
//!
//! The control beside them is a bucket subresource whose body IS an XML document, which
//! keeps its own media type: the rule is about the body, not about every read of a bucket.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns), so what is pinned is the wire response.

use super::bucket_lifecycle::{get_bucket_policy, put_bucket_policy, router, send};
use super::object_and_tagging_roundtrip::{bucket, text};

/// The media type a JSON document is served as.
const JSON: &str = "application/json";

/// The media type this service's XML documents are served as.
const XML: &str = "application/xml";

/// `(case, the policy document the bucket carries)`.
fn rows() -> Vec<(&'static str, String)> {
    vec![
        (
            "a compact document",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"logs.amazonaws.com"},"Action":"s3:PutObject","Resource":"arn:aws:s3:::media-type-bucket/*"}]}"#
                .to_string(),
        ),
        (
            "a document indented with newlines and tabs",
            "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{\n\t\t\t\"Effect\": \
             \"Allow\",\n\t\t\t\"Principal\": {\n\t\t\t\t\"Service\": \
             \"logs.amazonaws.com\"\n\t\t\t},\n\t\t\t\"Action\": \
             \"s3:PutObject\",\n\t\t\t\"Resource\": \
             \"arn:aws:s3:::media-type-bucket/*\"\n\t\t}\n\t]\n}"
                .to_string(),
        ),
        (
            "a document carrying several statements",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"One","Effect":"Allow","Principal":{"Service":"logs.amazonaws.com"},"Action":"s3:PutObject","Resource":"arn:aws:s3:::media-type-bucket/*"},{"Sid":"Two","Effect":"Allow","Principal":{"Service":"delivery.logs.amazonaws.com"},"Action":"s3:GetBucketAcl","Resource":"arn:aws:s3:::media-type-bucket"}]}"#
                .to_string(),
        ),
        (
            "a document carrying non-ASCII text",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"Lieferung","Effect":"Allow","Principal":{"Service":"logs.amazonaws.com"},"Action":"s3:PutObject","Resource":"arn:aws:s3:::media-type-bucket/*"}]}"#
                .to_string(),
        ),
    ]
}

#[tokio::test]
async fn a_bucket_policy_is_served_as_the_json_document_it_is() {
    let router = router();
    let name = "media-type-bucket";
    bucket(&router, name).await;

    for (case, policy) in rows() {
        let attached = put_bucket_policy(&router, name, &policy).await;
        assert_eq!(
            attached.status, 204,
            "case {case:?}: the policy must be attached, got {} with body {:?}",
            attached.status,
            text(&attached)
        );

        let read = get_bucket_policy(&router, name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the policy must read back, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.header("content-type").as_deref(),
            Some(JSON),
            "case {case:?}: the policy document must be described as {JSON:?}, got {:?}",
            read.header("content-type")
        );
        assert_eq!(
            text(&read),
            policy,
            "case {case:?}: the document must read back byte for byte, got {:?}",
            text(&read)
        );
    }

    // The control: a bucket subresource whose body IS an XML document keeps that media
    // type, so what the rule changed is the response that carries JSON and nothing else.
    let versioning = send(&router, "GET", &format!("/{name}?versioning"), &[], "").await;
    assert_eq!(
        versioning.status, 200,
        "the versioning read must answer 200, got {} with body {:?}",
        versioning.status,
        text(&versioning)
    );
    assert_eq!(
        versioning.header("content-type").as_deref(),
        Some(XML),
        "an XML document must still be described as {XML:?}, got {:?}",
        versioning.header("content-type")
    );

    // And the refusal of a bucket carrying NO policy is an XML error document, which is the
    // media type every refusal of this service carries.
    bucket(&router, "media-type-no-policy").await;
    let refused = get_bucket_policy(&router, "media-type-no-policy").await;
    assert_eq!(
        refused.status, 404,
        "a bucket carrying no policy must be refused, got {} with body {:?}",
        refused.status,
        text(&refused)
    );
    assert_eq!(
        refused.header("content-type").as_deref(),
        Some(XML),
        "a refusal must stay an XML document, got {:?}",
        refused.header("content-type")
    );
}
