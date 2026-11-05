// SPDX-License-Identifier: CC0-1.0

//! A general purpose bucket has ONE tag set, whichever API writes it.
//!
//! The rule under test: S3 Control's generic tagging surface (`TagResource`,
//! `ListTagsForResource`, `UntagResource`) addresses the resource its `ResourceArn` names
//! and reads and writes THAT resource's one tag set — so for a general purpose bucket ARN
//! (`arn:<partition>:s3:::<bucket>`, whose Region and account fields are empty because a
//! bucket name is globally unique) it reads and writes the same tag set Amazon S3's own
//! `GetBucketTagging` / `PutBucketTagging` read and write. A bucket is "the following
//! Amazon S3 resource" `TagResource`'s own documentation names first, so that ARN form is
//! served and not refused, whatever `S3ResourceArn`'s modelled `@pattern` admits.
//!
//! Both directions are driven, because a target holding two independent tag stores would
//! pass either one alone: a set written through S3 Control is reported by S3, and a set
//! written through S3 is reported by S3 Control.
//!
//! The rows span what the rule generalises over: how many pairs a call carries (one, two,
//! three), the length and form of the bucket name (the shortest S3 allows, the longest,
//! one carrying dots and digits), the ARN's partition, and tag values carrying spaces and
//! an XML-escaped character. Each row also pins the states around the write: before any
//! tag the bucket has no tag set at all (`NoSuchTagSet`), and once the last key is removed
//! it has none again.
//!
//! This suite needs TWO services on one engine, so it lives here rather than in either
//! service's own crate: the s3control crate's own `resource_tagging` suite pins the rest
//! of the surface (the resources that service itself holds, the tag-count bound and the
//! ARN forms it refuses), and the S3 crate's `bucket_tagging` suite pins S3's own half.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_provisions_stack_resources::xml_texts;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The account id the S3 Control requests name, in `x-amz-account-id`: the
/// caller's own, which is the only account S3 Control authorizes a request for
/// (`engine::emulator_account_id`).
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The XML namespace S3's own model declares, which a client's payload carries.
const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The XML namespace S3 Control's model declares.
const S3CONTROL_XMLNS: &str = "http://awss3control.amazonaws.com/doc/2018-08-20/";

/// One response: the status and the body as text.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    /// The S3-style error code (`<Code>`) of a failure response, or the empty string.
    fn code(&self) -> String {
        xml_texts(&self.body, "Code").into_iter().next().unwrap_or_default()
    }

    /// The `<Message>` of a failure response, or the empty string.
    fn message(&self) -> String {
        xml_texts(&self.body, "Message")
            .into_iter()
            .next()
            .unwrap_or_default()
    }

    /// The `(Key, Value)` pairs the body reports, sorted: a tag set is a set, so no
    /// member order is part of the contract.
    fn tags(&self) -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = xml_texts(&self.body, "Key")
            .into_iter()
            .zip(xml_texts(&self.body, "Value"))
            .collect();
        pairs.sort();
        pairs
    }
}

/// Dispatch one request signed for the `s3` signing name, which is what both services
/// sign as; the URI path is what tells them apart.
fn send(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    method: &str,
    path: &str,
    body: &str,
) -> Answer {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        // The credential scope is how a request names the service it is for and the
        // region a handler builds its ARNs in. The signature is never checked: this
        // router runs with authentication off.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/s3/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        )
        .header("x-amz-account-id", ACCOUNT)
        .header("content-type", "application/xml")
        .body(bytes::Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    Answer {
        status: response.status().as_u16(),
        body: String::from_utf8_lossy(response.body()).into_owned(),
    }
}

/// Escape a value for an XML text node, the way a client's XML writer does.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The `<Tag>` elements of a tag set, the element name both models give `TagList`'s and
/// `TagSet`'s member.
fn tag_elements(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| {
            format!(
                "<Tag><Key>{}</Key><Value>{}</Value></Tag>",
                xml_escape(key),
                xml_escape(value)
            )
        })
        .collect()
}

/// `PUT /<bucket>` — S3's `CreateBucket`. A create sent to a regional endpoint names its
/// location constraint: the unspecified constraint means us-east-1.
fn create_bucket(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime, bucket: &str) {
    let answer = send(
        router,
        runtime,
        "PUT",
        &format!("/{bucket}"),
        &format!(
            "<CreateBucketConfiguration xmlns=\"{S3_XMLNS}\">\
             <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
        ),
    );
    assert_eq!(
        answer.status, 200,
        "setup: CreateBucket({bucket:?}) must succeed, got {} / {}",
        answer.status, answer.body
    );
}

/// `GET /<bucket>?tagging` — S3's `GetBucketTagging`.
fn s3_get_bucket_tagging(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    bucket: &str,
) -> Answer {
    send(router, runtime, "GET", &format!("/{bucket}?tagging"), "")
}

/// `PUT /<bucket>?tagging` — S3's `PutBucketTagging`, which REPLACES the whole set.
fn s3_put_bucket_tagging(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    bucket: &str,
    pairs: &[(String, String)],
) -> Answer {
    send(
        router,
        runtime,
        "PUT",
        &format!("/{bucket}?tagging"),
        &format!(
            "<Tagging xmlns=\"{S3_XMLNS}\"><TagSet>{}</TagSet></Tagging>",
            tag_elements(pairs)
        ),
    )
}

/// `POST /v20180820/tags/<arn>` — S3 Control's `TagResource`, which MERGES.
fn s3control_tag_resource(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    resource_arn: &str,
    pairs: &[(String, String)],
) -> Answer {
    send(
        router,
        runtime,
        "POST",
        &format!("/v20180820/tags/{resource_arn}"),
        &format!(
            "<TagResourceRequest xmlns=\"{S3CONTROL_XMLNS}\"><Tags>{}</Tags>\
             </TagResourceRequest>",
            tag_elements(pairs)
        ),
    )
}

/// `GET /v20180820/tags/<arn>` — S3 Control's `ListTagsForResource`.
fn s3control_list_tags(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    resource_arn: &str,
) -> Answer {
    send(
        router,
        runtime,
        "GET",
        &format!("/v20180820/tags/{resource_arn}"),
        "",
    )
}

/// `DELETE /v20180820/tags/<arn>?tagKeys=…` — S3 Control's `UntagResource`, one query
/// parameter per key.
fn s3control_untag_resource(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    resource_arn: &str,
    keys: &[&str],
) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={key}"))
        .collect::<Vec<_>>()
        .join("&");
    send(
        router,
        runtime,
        "DELETE",
        &format!("/v20180820/tags/{resource_arn}?{query}"),
        "",
    )
}

/// `pairs` as owned strings, sorted by key.
fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut owned: Vec<(String, String)> = entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    owned.sort();
    owned
}

/// One row: the bucket, the ARN S3 Control is handed for it, and the tag set written
/// through each service.
struct Row {
    case: &'static str,
    bucket: String,
    /// The ARN form the S3 Control call carries for that bucket.
    resource_arn: String,
    /// What `TagResource` writes.
    through_s3control: Vec<(String, String)>,
    /// What `PutBucketTagging` later replaces it with.
    through_s3: Vec<(String, String)>,
}

/// A 63-character bucket name — the longest S3 documents ("Bucket names must be between
/// 3 (min) and 63 (max) characters long", S3's bucket naming rules).
fn longest_bucket_name() -> String {
    let name = format!("xsvc-tagging-{}", "b".repeat(50));
    assert_eq!(name.len(), 63, "the longest-name row must be 63 characters");
    name
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "two pairs, the recorded shape",
            bucket: "xsvc-tagging-plain".to_string(),
            resource_arn: "arn:aws:s3:::xsvc-tagging-plain".to_string(),
            through_s3control: pairs(&[("Team", "platform"), ("CostCenter", "engineering")]),
            through_s3: pairs(&[("Team", "storage")]),
        },
        Row {
            // The shortest name S3's naming rules allow.
            case: "one pair, the shortest bucket name",
            bucket: "x3t".to_string(),
            resource_arn: "arn:aws:s3:::x3t".to_string(),
            through_s3control: pairs(&[("env", "dev")]),
            through_s3: pairs(&[("env", "prod"), ("owner", "data platform")]),
        },
        Row {
            case: "three pairs, the longest bucket name",
            bucket: longest_bucket_name(),
            resource_arn: format!("arn:aws:s3:::{}", longest_bucket_name()),
            through_s3control: pairs(&[
                ("Team", "platform"),
                ("CostCenter", "engineering"),
                ("Tier", "gold"),
            ]),
            through_s3: pairs(&[("Tier", "silver")]),
        },
        Row {
            case: "a dotted, digit-bearing bucket name",
            bucket: "xsvc.tagging.7.dots".to_string(),
            resource_arn: "arn:aws:s3:::xsvc.tagging.7.dots".to_string(),
            through_s3control: pairs(&[("retention", "long term")]),
            through_s3: pairs(&[("retention", "short term"), ("note", "a_plus/b@7")]),
        },
        Row {
            // The bucket's name is the ARN's resource whichever partition the ARN names,
            // because the bucket is the same globally named resource.
            case: "another partition's ARN for the bucket",
            bucket: "xsvc-tagging-partition".to_string(),
            resource_arn: "arn:aws-cn:s3:::xsvc-tagging-partition".to_string(),
            through_s3control: pairs(&[("Team", "platform"), ("note", "value with spaces")]),
            through_s3: pairs(&[("Team", "platform")]),
        },
    ]
}

#[test]
fn a_bucket_tagged_through_s3control_reports_those_tags_through_s3() {
    std::thread::Builder::new()
        .name("s3control-bucket-tags".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            for row in rows() {
                let case = row.case;
                create_bucket(&router, &runtime, &row.bucket);

                // Before anything is written the bucket has NO tag set — the state S3
                // publishes `NoSuchTagSet` for — so a reported tag below can only have
                // come from the write between these two reads.
                let untagged = s3_get_bucket_tagging(&router, &runtime, &row.bucket);
                assert_eq!(
                    (untagged.status, untagged.code().as_str()),
                    (404, "NoSuchTagSet"),
                    "case {case:?}: an untagged bucket must answer NoSuchTagSet, got {} / {}",
                    untagged.status,
                    untagged.body
                );
                assert_eq!(
                    untagged.message(),
                    "The TagSet does not exist",
                    "case {case:?}: expected the published NoSuchTagSet message, got {}",
                    untagged.body
                );

                // S3 Control writes the tags, through the general purpose bucket ARN.
                let tagged =
                    s3control_tag_resource(&router, &runtime, &row.resource_arn, &row.through_s3control);
                assert_eq!(
                    tagged.status, 204,
                    "case {case:?}: TagResource on a general purpose bucket ARN must be \
                     accepted with 204, got {} / {}",
                    tagged.status, tagged.body
                );

                // ... and S3 reports exactly them: one tag set per bucket, not one per API.
                let read = s3_get_bucket_tagging(&router, &runtime, &row.bucket);
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: expected 200 from GetBucketTagging, got {} / {}",
                    read.status, read.body
                );
                assert_eq!(
                    read.tags(),
                    row.through_s3control,
                    "case {case:?}: S3 must report the tags S3 Control wrote, got {}",
                    read.body
                );

                // The same set read back through S3 Control's own reader.
                let listed = s3control_list_tags(&router, &runtime, &row.resource_arn);
                assert_eq!(
                    (listed.status, listed.tags()),
                    (200, row.through_s3control.clone()),
                    "case {case:?}: ListTagsForResource must report the same set, got {} / {}",
                    listed.status,
                    listed.body
                );

                // The other direction: S3 REPLACES the whole set, and S3 Control reports
                // what S3 left behind.
                let replaced =
                    s3_put_bucket_tagging(&router, &runtime, &row.bucket, &row.through_s3);
                assert_eq!(
                    replaced.status, 204,
                    "case {case:?}: expected 204 from PutBucketTagging, got {} / {}",
                    replaced.status, replaced.body
                );
                let listed = s3control_list_tags(&router, &runtime, &row.resource_arn);
                assert_eq!(
                    (listed.status, listed.tags()),
                    (200, row.through_s3.clone()),
                    "case {case:?}: S3 Control must report the tags S3 wrote, got {} / {}",
                    listed.status,
                    listed.body
                );

                // S3 Control removes the keys S3 left, and the bucket has no tag set
                // again — the state it started in.
                let keys: Vec<&str> = row.through_s3.iter().map(|(key, _)| key.as_str()).collect();
                let untagged =
                    s3control_untag_resource(&router, &runtime, &row.resource_arn, &keys);
                assert_eq!(
                    untagged.status, 204,
                    "case {case:?}: UntagResource must be accepted with 204, got {} / {}",
                    untagged.status, untagged.body
                );
                let read = s3_get_bucket_tagging(&router, &runtime, &row.bucket);
                assert_eq!(
                    (read.status, read.code().as_str()),
                    (404, "NoSuchTagSet"),
                    "case {case:?}: a bucket whose last tag was removed must answer \
                     NoSuchTagSet again, got {} / {}",
                    read.status,
                    read.body
                );
            }

            // The tags of one bucket are that bucket's: a write through either API reaches
            // only the bucket its own request named.
            let (first, second) = ("xsvc-tagging-one", "xsvc-tagging-two");
            for bucket in [first, second] {
                create_bucket(&router, &runtime, bucket);
            }
            let tagged = s3control_tag_resource(
                &router,
                &runtime,
                &format!("arn:aws:s3:::{first}"),
                &pairs(&[("Team", "platform")]),
            );
            assert_eq!(
                tagged.status, 204,
                "tagging the first bucket must be accepted, got {} / {}",
                tagged.status, tagged.body
            );
            let other = s3_get_bucket_tagging(&router, &runtime, second);
            assert_eq!(
                (other.status, other.code().as_str()),
                (404, "NoSuchTagSet"),
                "the other bucket must still have no tag set, got {} / {}",
                other.status,
                other.body
            );
            let listed = s3control_list_tags(&router, &runtime, &format!("arn:aws:s3:::{second}"));
            assert_eq!(
                (listed.status, listed.tags()),
                (200, Vec::new()),
                "the other bucket must report no tags, got {} / {}",
                listed.status,
                listed.body
            );
        })
        .expect("spawn the bucket-tagging thread")
        .join()
        .expect("a bucket's tag set must be one store across both services");
}
