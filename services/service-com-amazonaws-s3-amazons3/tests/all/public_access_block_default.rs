// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Block Public Access configuration a bucket is CREATED with,
//! and for the sentence S3 refuses a call that configuration denies with.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite owns), so what is pinned is the wire behaviour.
//!
//! The rules under test, with their sources:
//!
//! * "Amazon S3 automatically enables S3 Block Public Access … for new buckets" (Amazon S3
//!   User Guide, Blocking public access to your Amazon S3 storage): a bucket comes into
//!   being with all four members of `PublicAccessBlockConfiguration` true, so
//!   `GetPublicAccessBlock` answers 200 with four switches on rather than the
//!   subresource-not-there refusal — which is reserved for the state
//!   `DeletePublicAccessBlock` leaves the bucket in. The four members are
//!   `BlockPublicAcls`, `IgnorePublicAcls`, `BlockPublicPolicy`, `RestrictPublicBuckets`
//!   (`smithy_query(query="shape",shape="PublicAccessBlockConfiguration")`).
//! * The switches are IN FORCE from that moment: with `BlockPublicPolicy` on S3 "reject[s]
//!   calls to PUT Bucket policy if the specified bucket policy allows public access"
//!   (`…$BlockPublicPolicy`) and with `BlockPublicAcls` on "PUT Bucket ACL and PUT Object
//!   ACL calls fail if the specified ACL is public" (`…$BlockPublicAcls`).
//! * Each refusal is `AccessDenied` with HTTP 403 and a message naming the caller, the IAM
//!   action and the resource the request addressed, and the setting that prevented the
//!   call. Its fixed text is `User: ` / ` is not authorized to perform: ` / ` on resource: `
//!   / ` because public policies are prevented by the BlockPublicPolicy setting in S3 Block
//!   Public Access.` (and the `public ACLs` / `BlockPublicAcls` variant); the caller ARN,
//!   the action and the quoted resource ARN are the request's own values.

use super::bucket_lifecycle::{
    constraint_region, create_bucket, get_bucket_policy, put_bucket_policy, router, send,
    send_in_region,
};
use super::bucket_ownership_controls::put_ownership_controls;
use super::bucket_public_access_block::{
    allow_public_access, get_public_access_block, reported_flags,
};
use super::object_and_tagging_roundtrip::{put_object, text};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The predefined group URI that names everyone, so a grant to it is a PUBLIC grant ("The
/// Meaning of Public", Amazon S3 User Guide).
const ALL_USERS: &str = "http://acs.amazonaws.com/groups/global/AllUsers";

/// The principal a request of this emulator speaks for, as a refusal names it — the one
/// account this emulator serves, spelled the way `sts:GetCallerIdentity` reports it.
fn caller() -> String {
    engine::emulator_arn("iam", "", "user/emulator")
}

/// The sentence S3 refuses `action` on `resource` with when the `BlockPublicPolicy` switch
/// prevented it.
fn policy_denial(action: &str, resource: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {action} on resource: \"{resource}\" because \
         public policies are prevented by the BlockPublicPolicy setting in S3 Block Public \
         Access.",
        caller()
    )
}

/// The same sentence for the `BlockPublicAcls` switch.
fn acl_denial(action: &str, resource: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {action} on resource: \"{resource}\" because \
         public ACLs are prevented by the BlockPublicAcls setting in S3 Block Public Access.",
        caller()
    )
}

/// A bucket policy whose single statement allows `action` on the bucket to `principal`.
fn policy_for(bucket: &str, principal: &str, action: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"s\",\"Effect\":\"Allow\",\
         \"Principal\":{principal},\"Action\":\"{action}\",\
         \"Resource\":\"arn:aws:s3:::{bucket}/*\"}}]}}"
    )
}

/// An `AccessControlPolicy` body granting `permission` to the predefined group `uri`.
fn group_policy_body(uri: &str, permission: &str) -> String {
    format!(
        "<AccessControlPolicy xmlns=\"{XMLNS}\"><AccessControlList><Grant>\
         <Grantee xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" \
         xsi:type=\"Group\"><URI>{uri}</URI></Grantee>\
         <Permission>{permission}</Permission></Grant></AccessControlList>\
         </AccessControlPolicy>"
    )
}

#[tokio::test]
async fn every_newly_created_bucket_reports_all_four_switches_on() {
    let router = router();

    // The rows differ along what the creation default generalises over: the REGION the
    // bucket is created in (the default one, the legacy `EU` constraint and a regional
    // one), the Object Ownership setting the create names (none, and each of the two under
    // which ACLs affect permissions), and the NAME's form — none of which the default
    // depends on.
    let rows: &[(&str, &str, Option<&str>, Option<&str>)] = &[
        ("no constraint and no ownership header", "pab-default-plain", None, None),
        ("the legacy EU constraint", "pab-default-eu", Some("EU"), None),
        (
            "a regional constraint",
            "pab.default.regional-bucket",
            Some("eu-west-1"),
            None,
        ),
        (
            "the object-writer ownership setting",
            "pab-default-object-writer",
            None,
            Some("ObjectWriter"),
        ),
        (
            "the bucket-owner-preferred ownership setting",
            "pab-default-owner-preferred",
            None,
            Some("BucketOwnerPreferred"),
        ),
    ];

    for (case, bucket, constraint, ownership) in rows {
        let headers: Vec<(&str, &str)> = ownership
            .map(|setting| vec![("x-amz-object-ownership", setting)])
            .unwrap_or_default();
        let body = constraint
            .map(|constraint| {
                format!(
                    "<CreateBucketConfiguration xmlns=\"{XMLNS}\">\
                     <LocationConstraint>{constraint}</LocationConstraint>\
                     </CreateBucketConfiguration>"
                )
            })
            .unwrap_or_default();
        // Addressed to the endpoint of the Region the constraint names, since an endpoint
        // for one Region cannot place a bucket in another.
        let created = send_in_region(
            &router,
            &constraint_region(*constraint),
            "PUT",
            &format!("/{bucket}"),
            &headers,
            &body,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must succeed, got {} with body {:?}",
            created.status,
            text(&created)
        );

        // Twice: the read reports, it does not configure, so a second read answers the
        // same thing.
        for attempt in ["the first read", "a second read"] {
            let read = get_public_access_block(&router, bucket, &[]).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}/{attempt}: GetPublicAccessBlock on a bucket nothing has \
                 configured answers 200, got {} with body {:?}",
                read.status,
                text(&read)
            );
            let switches = reported_flags(&read);
            assert_eq!(
                switches,
                [Some(true); 4],
                "case {case:?}/{attempt}: every switch must be reported true, got \
                 {switches:?} in body {:?}",
                text(&read)
            );
        }
    }
}

#[tokio::test]
async fn the_creation_default_denies_a_public_policy_and_a_public_acl() {
    let router = router();

    // One row per call the two switches govern: the bucket policy write
    // (`BlockPublicPolicy`), and the three ACL writes (`BlockPublicAcls`) — a bucket ACL,
    // an object ACL and the ACL a write names on `PutObject`. Each names the IAM action its
    // refusal must carry and the resource ARN that refusal must quote.
    let bucket = "pab-default-enforced";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        text(&created)
    );
    // ACLs have to affect permissions at all before the Block Public Access switch is what
    // decides the ACL answers.
    put_ownership_controls(&router, bucket, &["ObjectWriter"]).await;
    let stored = put_object(&router, bucket, "subject.txt", b"held", &[]).await;
    assert_eq!(
        stored.status, 200,
        "the object must be stored first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );
    let bucket_arn = format!("arn:aws:s3:::{bucket}");
    let object_arn = format!("{bucket_arn}/subject.txt");

    struct Row {
        case: &'static str,
        method: &'static str,
        uri: String,
        headers: Vec<(&'static str, &'static str)>,
        body: String,
        message: String,
    }

    let rows: Vec<Row> = vec![
        Row {
            case: "a bucket policy granting a wildcard principal",
            method: "PUT",
            uri: format!("/{bucket}?policy"),
            headers: Vec::new(),
            body: policy_for(bucket, "\"*\"", "s3:GetObject"),
            message: policy_denial("s3:PutBucketPolicy", &bucket_arn),
        },
        Row {
            case: "a bucket policy whose wildcard is inside an AWS principal map",
            method: "PUT",
            uri: format!("/{bucket}?policy"),
            headers: Vec::new(),
            body: policy_for(bucket, "{\"AWS\":\"*\"}", "s3:PutObject"),
            message: policy_denial("s3:PutBucketPolicy", &bucket_arn),
        },
        Row {
            case: "a public canned bucket ACL",
            method: "PUT",
            uri: format!("/{bucket}?acl"),
            headers: vec![("x-amz-acl", "public-read")],
            body: String::new(),
            message: acl_denial("s3:PutBucketAcl", &bucket_arn),
        },
        Row {
            case: "a bucket ACL body granting AllUsers READ",
            method: "PUT",
            uri: format!("/{bucket}?acl"),
            headers: Vec::new(),
            body: group_policy_body(ALL_USERS, "READ"),
            message: acl_denial("s3:PutBucketAcl", &bucket_arn),
        },
        Row {
            case: "a public canned object ACL",
            method: "PUT",
            uri: format!("/{bucket}/subject.txt?acl"),
            headers: vec![("x-amz-acl", "public-read-write")],
            body: String::new(),
            message: acl_denial("s3:PutObjectAcl", &object_arn),
        },
        Row {
            case: "a write naming a public canned ACL",
            method: "PUT",
            uri: format!("/{bucket}/subject.txt"),
            headers: vec![("x-amz-acl", "authenticated-read")],
            body: String::new(),
            message: acl_denial("s3:PutObject", &object_arn),
        },
    ];

    for row in &rows {
        let answer = send(&router, row.method, &row.uri, &row.headers, &row.body).await;
        assert_eq!(
            answer.status, 403,
            "case {:?}: a call the creation default denies is HTTP 403, got {} with body {:?}",
            row.case,
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            "AccessDenied",
            "case {:?}: expected AccessDenied, got {:?}",
            row.case,
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            row.message,
            "case {:?}: the refusal must name the caller, the action, the resource and the \
             setting; expected {:?}, got {:?}",
            row.case,
            row.message,
            answer.error_message()
        );
    }

    // Nothing above was stored: the bucket still carries no policy at all.
    let read = get_bucket_policy(&router, bucket).await;
    assert_eq!(
        read.error_code(),
        "NoSuchBucketPolicy",
        "a denied put must leave the bucket without a policy, got {} with body {:?}",
        read.status,
        text(&read)
    );
}

#[tokio::test]
async fn lifting_the_default_lets_the_same_calls_through() {
    let router = router();
    let bucket = "pab-default-lifted";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {}",
        created.status
    );
    put_ownership_controls(&router, bucket, &["ObjectWriter"]).await;

    // The refusals above are the stored CONFIGURATION's, not a property of the request: a
    // put that replaces it with every switch off lets the same two calls through.
    allow_public_access(&router, bucket).await;

    let policy = policy_for(bucket, "\"*\"", "s3:GetObject");
    let accepted = put_bucket_policy(&router, bucket, &policy).await;
    assert_eq!(
        accepted.status, 204,
        "with BlockPublicPolicy off the public policy must be accepted, got {} with body {:?}",
        accepted.status,
        text(&accepted)
    );
    let acl = send(
        &router,
        "PUT",
        &format!("/{bucket}?acl"),
        &[("x-amz-acl", "public-read")],
        "",
    )
    .await;
    assert_eq!(
        acl.status, 200,
        "with BlockPublicAcls off the public canned ACL must be accepted, got {} with body \
         {:?}",
        acl.status,
        text(&acl)
    );
}
