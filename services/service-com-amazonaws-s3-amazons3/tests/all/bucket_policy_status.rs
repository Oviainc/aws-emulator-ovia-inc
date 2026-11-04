// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetBucketPolicyStatus`: what a bucket's POLICY STATUS reports.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns), so what is pinned is the wire behaviour —
//! status, body, error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `PolicyStatus$IsPublic` is "the policy status for this bucket. TRUE indicates that
//!   this bucket is public" (`smithy_query(query="shape",shape="PolicyStatus")`), and the
//!   status is computed from the document the bucket CARRIES: a bucket is public exactly
//!   when one of its policy's statements both allows and reaches everyone — an `Allow`
//!   whose principal is the wildcard in any of the forms a policy writes it, or an `Allow`
//!   naming a `NotPrincipal` — and is not held back by a `Condition` restricting the grant
//!   to fixed values of one of the keys Amazon S3 lists under "The Meaning of 'Public'"
//!   (Amazon S3 User Guide, Blocking public access). A condition whose operand names every
//!   value the key could take — the bare wildcard, or the whole address space for
//!   `aws:SourceIp` — restricts nothing, so the grant stays public.
//! * A bucket carrying NO policy has no policy status to report: this operation states
//!   something ABOUT a policy, so the bucket without one is refused `NoSuchBucketPolicy`
//!   (404) exactly as the document readers are — which is what keeps "no policy" from being
//!   answered as "a policy that is not public".
//! * Reading the status changes nothing: the document reads back unchanged afterwards and
//!   a second read reports the same status.
//! * Block Public Access does not enter the answer. Its switches govern what S3 lets a
//!   public policy DO ("restricts access to this bucket …", "reject calls to PUT Bucket
//!   policy …", `smithy_query(query="shape",shape="PublicAccessBlockConfiguration")`),
//!   while this operation reports the policy's own status.
//! * A request naming a bucket the emulator does not hold is `NoSuchBucket` (404), and one
//!   whose `x-amz-expected-bucket-owner` is not the bucket's owner is `AccessDenied` (403),
//!   as `GetBucketPolicyStatusRequest$ExpectedBucketOwner` documents.

use quick_xml::events::Event;
use quick_xml::Reader;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{
    create_bucket, get_bucket_policy, put_bucket_policy, router, send, Answer,
};
use super::bucket_public_access_block::allow_public_access;

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// Every condition key Amazon S3 lists under "The Meaning of 'Public'" (Amazon S3 User
/// Guide, Blocking public access) as one whose fixed values keep a wildcard-principal
/// statement from being public, paired with a value of the right FORM for that key.
///
/// Written out here rather than imported so the handler and this table cannot agree on a
/// wrong catalog; `aws:SourceIp` carries a documentation range (RFC 5737 `203.0.113.0/24`)
/// because that key restricts only while the addresses it names are not the whole internet.
const RESTRICTING_CONDITIONS: [(&str, &str, &str); 13] = [
    ("aws:PrincipalArn", "ArnEquals", "arn:aws:iam::111122223333:role/Reader"),
    ("aws:PrincipalOrgID", "StringEquals", "o-exampleorgid"),
    ("aws:PrincipalOrgPaths", "ForAnyValue:StringLike", "o-exampleorgid/r-examplerootid/*"),
    ("aws:SourceAccount", "StringEquals", "111122223333"),
    ("aws:SourceArn", "ArnEquals", "arn:aws:sns:us-east-1:111122223333:topic"),
    ("aws:SourceIp", "IpAddress", "203.0.113.0/24"),
    ("aws:SourceOwner", "StringEquals", "111122223333"),
    ("aws:SourceVpc", "StringEquals", "vpc-0123456789abcdef0"),
    ("aws:SourceVpce", "StringEquals", "vpce-0123456789abcdef0"),
    ("aws:userid", "StringEquals", "AIDAEXAMPLEUSERID"),
    ("s3:DataAccessPointAccount", "StringEquals", "111122223333"),
    (
        "s3:DataAccessPointArn",
        "ArnEquals",
        "arn:aws:s3:us-east-1:111122223333:accesspoint/reader",
    ),
    (
        "s3:x-amz-server-side-encryption-aws-kms-key-id",
        "StringEquals",
        "arn:aws:kms:us-east-1:111122223333:key/0123abcd-ef45-6789-abcd-0123456789ab",
    ),
];

/// `GET /<bucket>?policyStatus` — `GetBucketPolicyStatus`.
async fn get_bucket_policy_status(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(router, "GET", &format!("/{bucket}?policyStatus"), headers, "").await
}

/// `PUT /<bucket>?publicAccessBlock` — `PutPublicAccessBlock`, with all four switches on.
async fn block_all_public_access(router: &MultiServiceRouter, bucket: &str) -> Answer {
    let body = format!(
        "<PublicAccessBlockConfiguration xmlns=\"{XMLNS}\">\
         <BlockPublicAcls>true</BlockPublicAcls>\
         <IgnorePublicAcls>true</IgnorePublicAcls>\
         <BlockPublicPolicy>true</BlockPublicPolicy>\
         <RestrictPublicBuckets>true</RestrictPublicBuckets>\
         </PublicAccessBlockConfiguration>"
    );
    send(
        router,
        "PUT",
        &format!("/{bucket}?publicAccessBlock"),
        &[],
        &body,
    )
    .await
}

/// The `IsPublic` a `GetBucketPolicyStatus` body reports, or `None` when the body carries
/// no such element.
///
/// Parsed with `quick-xml` (already a dependency of this workspace), the way the
/// public-access-block suite beside this one reads its payload, so a response that
/// under-reports the member fails loudly instead of defaulting to `false` here.
fn reported_is_public(answer: &Answer) -> Option<bool> {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut element = String::new();
    let mut reported = None;
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the response body must be well-formed XML: {error}"))
        {
            Event::Start(start) => {
                element = String::from_utf8_lossy(start.name().as_ref()).into_owned();
            }
            Event::Text(text) => {
                if element == "IsPublic" {
                    let decoded = text
                        .xml10_content()
                        .unwrap_or_else(|error| panic!("the response body must decode: {error}"));
                    reported = match decoded.as_ref() {
                        "true" => Some(true),
                        "false" => Some(false),
                        other => panic!("IsPublic must be a boolean, got {other:?}"),
                    };
                }
            }
            Event::End(_) => element.clear(),
            Event::Eof => break,
            _ => {}
        }
    }
    reported
}

/// One row of the status table: the case name, the policy document, and the `IsPublic` the
/// bucket must then report.
type StatusRow = (String, String, bool);

/// The documents the rule is asserted over, spanning the forms a wildcard principal is
/// written in, the effect, the number of statements, the presence and kind of condition,
/// and the catalog of restricting condition keys.
fn status_rows() -> Vec<StatusRow> {
    let mut rows: Vec<StatusRow> = vec![
        (
            "a bare wildcard principal".to_string(),
            serde_json::json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "s3:GetObject",
                    "Resource": "arn:aws:s3:::any-bucket/*",
                }],
            })
            .to_string(),
            true,
        ),
        (
            "a principal map whose AWS value is the wildcard".to_string(),
            serde_json::json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": "*"},
                    "Action": "s3:*",
                }],
            })
            .to_string(),
            true,
        ),
        (
            "a principal map listing the wildcard beside a named role".to_string(),
            serde_json::json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": ["arn:aws:iam::111122223333:root", "*"]},
                    "Action": "s3:ListBucket",
                }],
            })
            .to_string(),
            true,
        ),
        (
            "a principal list carrying the wildcard".to_string(),
            serde_json::json!({
                "Statement": {
                    "Effect": "Allow",
                    "Principal": ["*"],
                    "Action": "s3:GetObject",
                },
            })
            .to_string(),
            true,
        ),
        (
            "an Allow naming a NotPrincipal, so every other principal is granted".to_string(),
            serde_json::json!({
                "Statement": [{
                    "Effect": "Allow",
                    "NotPrincipal": {"AWS": "arn:aws:iam::111122223333:root"},
                    "Action": "s3:GetObject",
                }],
            })
            .to_string(),
            true,
        ),
        (
            "a Deny to the wildcard principal takes nothing public".to_string(),
            serde_json::json!({
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "s3:*",
                }],
            })
            .to_string(),
            false,
        ),
        (
            "an Allow to one named account".to_string(),
            serde_json::json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": "arn:aws:iam::111122223333:root"},
                    "Action": "s3:GetObject",
                }],
            })
            .to_string(),
            false,
        ),
        (
            "an Allow to a service principal".to_string(),
            serde_json::json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": "cloudtrail.amazonaws.com"},
                    "Action": "s3:PutObject",
                }],
            })
            .to_string(),
            false,
        ),
        (
            "a statement naming no principal at all".to_string(),
            "{\"Statement\":[{\"Effect\":\"Allow\",\"Action\":\"s3:*\"}]}".to_string(),
            false,
        ),
        (
            "a condition on a key outside the catalog leaves the grant public".to_string(),
            serde_json::json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "s3:PutObject",
                    "Condition": {"StringEquals": {"s3:x-amz-acl": "bucket-owner-full-control"}},
                }],
            })
            .to_string(),
            true,
        ),
        (
            "an aws:SourceIp of the whole IPv4 space restricts nothing".to_string(),
            serde_json::json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "s3:GetObject",
                    "Condition": {"IpAddress": {"aws:SourceIp": "0.0.0.0/0"}},
                }],
            })
            .to_string(),
            true,
        ),
        (
            "an aws:SourceIp list carrying the whole IPv6 space restricts nothing".to_string(),
            serde_json::json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "s3:GetObject",
                    "Condition": {"IpAddress": {"aws:SourceIp": ["203.0.113.0/24", "::/0"]}},
                }],
            })
            .to_string(),
            true,
        ),
        (
            "a restricting key spelled in another case, since condition keys are \
             case-insensitive"
                .to_string(),
            serde_json::json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "s3:GetObject",
                    "Condition": {"StringEquals": {"AWS:SourceVpc": "vpc-0123456789abcdef0"}},
                }],
            })
            .to_string(),
            false,
        ),
        (
            "a restricting key whose operand is the bare wildcard names no fixed value"
                .to_string(),
            serde_json::json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "s3:GetObject",
                    "Condition": {"ArnLike": {"aws:PrincipalArn": "*"}},
                }],
            })
            .to_string(),
            true,
        ),
        (
            "a restricting key whose list carries the bare wildcard beside a fixed value"
                .to_string(),
            serde_json::json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "s3:GetObject",
                    "Condition": {
                        "ArnLike": {
                            "aws:PrincipalArn": ["arn:aws:iam::111122223333:role/Reader", "*"],
                        },
                    },
                }],
            })
            .to_string(),
            true,
        ),
        (
            "a restricting key whose operand is an empty list names no fixed value".to_string(),
            serde_json::json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "s3:GetObject",
                    "Condition": {"StringEquals": {"aws:SourceAccount": []}},
                }],
            })
            .to_string(),
            true,
        ),
        (
            "a restricting key whose list carries two fixed values".to_string(),
            serde_json::json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "s3:GetObject",
                    "Condition": {
                        "StringEquals": {"aws:SourceAccount": ["111122223333", "444455556666"]},
                    },
                }],
            })
            .to_string(),
            false,
        ),
        (
            "three statements, the last of them public".to_string(),
            serde_json::json!({
                "Statement": [
                    {"Effect": "Deny", "Principal": "*", "Action": "s3:DeleteObject"},
                    {
                        "Effect": "Allow",
                        "Principal": {"AWS": "arn:aws:iam::111122223333:root"},
                        "Action": "s3:PutObject",
                    },
                    {"Effect": "Allow", "Principal": "*", "Action": "s3:GetObject"},
                ],
            })
            .to_string(),
            true,
        ),
        (
            "three statements, none of them public".to_string(),
            serde_json::json!({
                "Statement": [
                    {"Effect": "Deny", "Principal": "*", "Action": "s3:DeleteObject"},
                    {
                        "Effect": "Allow",
                        "Principal": {"AWS": "arn:aws:iam::111122223333:root"},
                        "Action": "s3:PutObject",
                    },
                    {
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "s3:GetObject",
                        "Condition": {
                            "StringEquals": {"aws:SourceVpce": "vpce-0123456789abcdef0"},
                        },
                    },
                ],
            })
            .to_string(),
            false,
        ),
    ];

    // One row per entry of the published catalog: a wildcard-principal Allow restricted to
    // a fixed value of that key is not public.
    for (key, operator, value) in RESTRICTING_CONDITIONS {
        rows.push((
            format!("a wildcard principal restricted by {key}"),
            serde_json::json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "s3:GetObject",
                    "Condition": {operator: {key: value}},
                }],
            })
            .to_string(),
            false,
        ));
    }
    rows
}

#[tokio::test]
async fn the_policy_status_reports_whether_the_stored_policy_grants_public_access() {
    let router = router();
    let bucket = "harvest-destination";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    // Many of the rows below ARE public documents, which the Block Public Access
    // configuration every bucket is created with would deny; this test is about the status
    // the stored document reports.
    allow_public_access(&router, bucket).await;

    for (label, document, expected) in status_rows() {
        let put = put_bucket_policy(&router, bucket, &document).await;
        assert_eq!(
            put.status, 204,
            "case {label:?}: the row's document must be accepted, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );

        let answer = get_bucket_policy_status(&router, bucket, &[]).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: expected HTTP 200, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            reported_is_public(&answer),
            Some(expected),
            "case {label:?}: expected IsPublic {expected}, got body {:?}",
            String::from_utf8_lossy(&answer.body)
        );

        // The read changed nothing: the document is still there byte for byte, and a
        // second read reports the same status.
        let read = get_bucket_policy(&router, bucket).await;
        assert_eq!(
            String::from_utf8_lossy(&read.body),
            document.as_str(),
            "case {label:?}: reading the status must leave the document untouched"
        );
        let again = get_bucket_policy_status(&router, bucket, &[]).await;
        assert_eq!(
            reported_is_public(&again),
            Some(expected),
            "case {label:?}: a second read must report the same status, got body {:?}",
            String::from_utf8_lossy(&again.body)
        );
    }
}

#[tokio::test]
async fn a_bucket_with_no_policy_is_refused_the_way_the_document_readers_refuse_it() {
    let router = router();
    // Several buckets, differing in name length and form, one of them outside `us-east-1`:
    // the status of a bucket without a policy is not a property of the bucket.
    let buckets: &[(&str, Option<&str>)] = &[
        ("abc", None),
        ("harvest-destination", None),
        ("logs.2026.archive-bucket", Some("eu-west-1")),
    ];
    for (bucket, region) in buckets {
        let created = create_bucket(&router, bucket, *region, None).await;
        assert_eq!(
            created.status, 200,
            "case {bucket:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        // Both readers answer the same absent subresource the same way: there is no policy,
        // so there is neither a document nor a status for one.
        for (reader, answer) in [
            ("GetBucketPolicy", get_bucket_policy(&router, bucket).await),
            (
                "GetBucketPolicyStatus",
                get_bucket_policy_status(&router, bucket, &[]).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {bucket:?}/{reader}: expected HTTP 404, got {} with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchBucketPolicy",
                "case {bucket:?}/{reader}: a fresh bucket carries no policy, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                reported_is_public(&answer),
                None,
                "case {bucket:?}/{reader}: a refusal reports no status, got body {:?}",
                String::from_utf8_lossy(&answer.body)
            );
        }

        // Once a policy IS there, the same read answers the status of THAT document.
        let attached = put_bucket_policy(
            &router,
            bucket,
            "{\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\
             \"arn:aws:iam::111122223333:root\"},\"Action\":\"s3:GetObject\"}]}",
        )
        .await;
        assert_eq!(
            attached.status, 204,
            "case {bucket:?}: the document must be accepted, got {} with body {:?}",
            attached.status,
            String::from_utf8_lossy(&attached.body)
        );
        let answer = get_bucket_policy_status(&router, bucket, &[]).await;
        assert_eq!(
            answer.status, 200,
            "case {bucket:?}: expected HTTP 200 once a policy is stored, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            reported_is_public(&answer),
            Some(false),
            "case {bucket:?}: a policy granting one account is not public, got body {:?}",
            String::from_utf8_lossy(&answer.body)
        );
    }
}

#[tokio::test]
async fn block_public_access_does_not_change_the_policy_status() {
    let router = router();
    // (case, the policy the bucket carries, the status it reports) — the pair of states the
    // switches could wrongly be folded into, asserted on both sides.
    let rows: &[(&str, &str, bool)] = &[
        (
            "a public policy",
            "{\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":\"*\",\"Action\":\"s3:GetObject\"}]}",
            true,
        ),
        (
            "a policy granting one account",
            "{\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\
             \"arn:aws:iam::111122223333:root\"},\"Action\":\"s3:GetObject\"}]}",
            false,
        ),
    ];
    for (index, (label, document, expected)) in rows.iter().enumerate() {
        let bucket = format!("status-under-block-{index}");
        let created = create_bucket(&router, &bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        // One row's document is public, which the configuration the bucket was created with
        // would deny; the switches are turned back on below, which is what this test is
        // about.
        allow_public_access(&router, &bucket).await;
        let put = put_bucket_policy(&router, &bucket, document).await;
        assert_eq!(
            put.status, 204,
            "case {label:?}: the document must be accepted, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );

        let before = get_bucket_policy_status(&router, &bucket, &[]).await;
        assert_eq!(
            reported_is_public(&before),
            Some(*expected),
            "case {label:?}: expected IsPublic {expected} before the switches are set, got body \
             {:?}",
            String::from_utf8_lossy(&before.body)
        );

        let blocked = block_all_public_access(&router, &bucket).await;
        assert_eq!(
            blocked.status, 200,
            "case {label:?}: the switches must be stored, got {} with body {:?}",
            blocked.status,
            String::from_utf8_lossy(&blocked.body)
        );

        // The switches govern what a public policy may DO; the policy's own status is
        // whatever its statements say.
        let after = get_bucket_policy_status(&router, &bucket, &[]).await;
        assert_eq!(
            reported_is_public(&after),
            Some(*expected),
            "case {label:?}: expected IsPublic {expected} with every switch on, got body {:?}",
            String::from_utf8_lossy(&after.body)
        );
    }
}

#[tokio::test]
async fn a_policy_status_read_is_refused_for_a_missing_bucket_or_another_owner() {
    let router = router();
    let bucket = "harvest-destination";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );

    let owner = engine::emulator_account_id();
    let not_the_owner = "111122223333";
    assert_ne!(
        owner, not_the_owner,
        "the mismatching row must name an account that is NOT the bucket's owner"
    );

    // (case, bucket, expected-bucket-owner header, status, error code) — a refusal keeps
    // its own status and code rather than being answered with a status document.
    let refused: &[(&str, &str, Option<&str>, u16, &str)] = &[
        (
            "a bucket the emulator does not hold",
            "no-such-bucket-at-all",
            None,
            404,
            "NoSuchBucket",
        ),
        (
            "an expected bucket owner that is not the owner",
            bucket,
            Some(not_the_owner),
            403,
            "AccessDenied",
        ),
        (
            "both: another owner named for a bucket that is not there",
            "no-such-bucket-at-all",
            Some(not_the_owner),
            403,
            "AccessDenied",
        ),
    ];
    for (label, target, expected_owner, status, code) in refused {
        let headers: Vec<(&str, &str)> = expected_owner
            .map(|account| vec![("x-amz-expected-bucket-owner", account)])
            .unwrap_or_default();
        let answer = get_bucket_policy_status(&router, target, &headers).await;
        assert_eq!(
            answer.status, *status,
            "case {label:?}: expected HTTP {status}, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {label:?}: expected error code {code}, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            reported_is_public(&answer),
            None,
            "case {label:?}: a refusal reports no status, got body {:?}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    // The owner's own account id in that header is accepted, so the 403 above is about the
    // MISMATCH and not about the header being present. The bucket carries a policy by then,
    // since the status is a statement about one.
    let attached = put_bucket_policy(
        &router,
        bucket,
        "{\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\
         \"arn:aws:iam::111122223333:root\"},\"Action\":\"s3:GetObject\"}]}",
    )
    .await;
    assert_eq!(
        attached.status, 204,
        "the document must be accepted, got {} with body {:?}",
        attached.status,
        String::from_utf8_lossy(&attached.body)
    );
    let as_owner =
        get_bucket_policy_status(&router, bucket, &[("x-amz-expected-bucket-owner", owner)]).await;
    assert_eq!(
        as_owner.status, 200,
        "case \"the owner's own account id\": expected HTTP 200, got {} with body {:?}",
        as_owner.status,
        String::from_utf8_lossy(&as_owner.body)
    );
    assert_eq!(
        reported_is_public(&as_owner),
        Some(false),
        "case \"the owner's own account id\": a policy granting one account is not public, got \
         body {:?}",
        String::from_utf8_lossy(&as_owner.body)
    );
}
