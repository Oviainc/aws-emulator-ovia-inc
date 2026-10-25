// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the S3 bucket control plane's GUARDRAILS: Block Public Access,
//! Object Ownership, and the endpoint a `CreateBucket` may be served by.
//!
//! The rules under test, with their sources:
//!
//! * Block Public Access. With `BlockPublicPolicy` on, S3 "reject[s] calls to PUT Bucket
//!   policy if the specified bucket policy allows public access"
//!   (`PublicAccessBlockConfiguration$BlockPublicPolicy`); with `BlockPublicAcls` on, "PUT
//!   Bucket ACL and PUT Object ACL calls fail if the specified ACL is public"
//!   (`…$BlockPublicAcls`). Both refusals are `AccessDenied` with HTTP 403, and both leave
//!   what the bucket already carries untouched — so a RE-PUT of a document that was
//!   accepted before the switch went on is refused too.
//! * Object Ownership. "By default, `ObjectOwnership` is set to `BucketOwnerEnforced` and
//!   ACLs are disabled" (`smithy_query(query="shape",shape="ObjectOwnership")`), so a
//!   bucket nothing has configured refuses every ACL write with
//!   `AccessControlListNotSupported`; `CreateBucket`'s `x-amz-object-ownership` chooses
//!   another setting, and a create that names BOTH an ACL and the enforced setting is
//!   refused `InvalidBucketAclWithObjectOwnership`.
//! * The create endpoint. A `CreateBucket` naming no `LocationConstraint` can only mean the
//!   one Region the unspecified constraint stands for — "If you don't specify a Region, the
//!   bucket is created in the US East (N. Virginia) Region (us-east-1) by default"
//!   (`CreateBucketConfiguration$LocationConstraint`) — so an endpoint for any other Region
//!   refuses it with `IllegalLocationConstraintException`.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, get_bucket_policy, put_bucket_policy, router, send, Answer};
use super::bucket_ownership_controls::put_ownership_controls;
use super::bucket_public_access_block::put_public_access_block;

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The predefined group URI that names everyone, so a grant to it is a PUBLIC grant
/// ("The Meaning of Public", Amazon S3 User Guide).
const ALL_USERS: &str = "http://acs.amazonaws.com/groups/global/AllUsers";

/// The predefined group URI that names every AWS account — also public.
const AUTHENTICATED_USERS: &str = "http://acs.amazonaws.com/groups/global/AuthenticatedUsers";

/// The predefined group an access-logging target bucket grants to: an S3 service
/// principal, which is not the public.
const LOG_DELIVERY: &str = "http://acs.amazonaws.com/groups/s3/LogDelivery";

/// `PUT /<bucket>?acl` — `PutBucketAcl`, with whatever headers and body the case names.
async fn put_acl(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Answer {
    send(router, "PUT", &format!("/{bucket}?acl"), headers, body).await
}

/// A bucket policy whose single statement allows `action` on the bucket to `principal`,
/// spelled the way a client sends one.
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
async fn block_public_policy_refuses_a_public_policy_and_keeps_the_stored_one() {
    let router = router();
    let bucket = "guardrails-policy";
    create_bucket(&router, bucket, None, None).await;

    // A policy that is NOT public, stored before the switch goes on: every refusal below
    // must leave exactly this document in place.
    let scoped = policy_for(
        bucket,
        "{\"Service\":\"cloudfront.amazonaws.com\"}",
        "s3:GetObject",
    );
    let stored = put_bucket_policy(&router, bucket, &scoped).await;
    assert_eq!(
        stored.status, 204,
        "the setup put must succeed, got {} with body {:?}",
        stored.status,
        String::from_utf8_lossy(&stored.body)
    );

    let on = put_public_access_block(&router, bucket, &[("BlockPublicPolicy", true)], &[]).await;
    assert_eq!(
        on.status, 200,
        "BlockPublicPolicy must be stored, got {} with body {:?}",
        on.status,
        String::from_utf8_lossy(&on.body)
    );

    // Every row is a policy that allows and reaches everyone, differing in HOW the
    // wildcard principal is spelled and in which action it allows.
    for (case, policy) in [
        (
            "a bare wildcard principal",
            policy_for(bucket, "\"*\"", "s3:GetObject"),
        ),
        (
            "a wildcard inside an AWS principal map",
            policy_for(bucket, "{\"AWS\":\"*\"}", "s3:GetObject"),
        ),
        (
            "a wildcard principal on every action",
            policy_for(bucket, "\"*\"", "s3:*"),
        ),
        (
            "a wildcard principal on a write action",
            policy_for(bucket, "{\"AWS\":[\"*\"]}", "s3:PutObject"),
        ),
    ] {
        let refused = put_bucket_policy(&router, bucket, &policy).await;
        assert_eq!(
            refused.status, 403,
            "case {case:?}: a public policy must be refused with HTTP 403 while \
             BlockPublicPolicy is on, got {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "AccessDenied",
            "case {case:?}: expected AccessDenied, got {:?}",
            refused.error_code()
        );
        let read = get_bucket_policy(&router, bucket).await;
        assert_eq!(
            String::from_utf8_lossy(&read.body),
            scoped,
            "case {case:?}: a refused put must leave the stored policy untouched"
        );
    }

    // A policy that is not public is still accepted, so the refusals above are about what
    // the policy grants and not about the switch blocking every put — including a RE-PUT
    // of the very document that was stored before the switch went on.
    for (case, policy) in [
        ("the stored policy, re-put unchanged", scoped.clone()),
        (
            "a policy scoped to one account",
            policy_for(
                bucket,
                "{\"AWS\":\"arn:aws:iam::123456789012:root\"}",
                "s3:GetObject",
            ),
        ),
    ] {
        let accepted = put_bucket_policy(&router, bucket, &policy).await;
        assert_eq!(
            accepted.status, 204,
            "case {case:?}: a policy that is not public must still be accepted, got {} \
             with body {:?}",
            accepted.status,
            String::from_utf8_lossy(&accepted.body)
        );
    }

    // And with the switch off, the same public policy the rows above refused is accepted:
    // the guardrail is the stored configuration, not the policy's shape.
    let off = put_public_access_block(&router, bucket, &[("BlockPublicPolicy", false)], &[]).await;
    assert_eq!(off.status, 200, "the switch must be turned off, got {}", off.status);
    let public = policy_for(bucket, "\"*\"", "s3:GetObject");
    let accepted = put_bucket_policy(&router, bucket, &public).await;
    assert_eq!(
        accepted.status, 204,
        "with BlockPublicPolicy off the public policy must be accepted, got {} with body \
         {:?}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
}

#[tokio::test]
async fn block_public_acls_refuses_a_public_grant_and_keeps_the_stored_acl() {
    let router = router();
    let bucket = "guardrails-acl";
    create_bucket(&router, bucket, None, None).await;
    // ACLs must affect permissions at all before the Block Public Access switch is what
    // decides the answer.
    put_ownership_controls(&router, bucket, &["ObjectWriter"]).await;

    let on = put_public_access_block(&router, bucket, &[("BlockPublicAcls", true)], &[]).await;
    assert_eq!(
        on.status, 200,
        "BlockPublicAcls must be stored, got {} with body {:?}",
        on.status,
        String::from_utf8_lossy(&on.body)
    );

    // Every row names a PUBLIC grant, differing in the form it arrives in (a canned ACL, a
    // grant header, an `AccessControlPolicy` body) and in which predefined group it
    // reaches.
    for (case, headers, body) in [
        (
            "the public-read canned ACL",
            vec![("x-amz-acl", "public-read")],
            String::new(),
        ),
        (
            "the public-read-write canned ACL",
            vec![("x-amz-acl", "public-read-write")],
            String::new(),
        ),
        (
            "the authenticated-read canned ACL",
            vec![("x-amz-acl", "authenticated-read")],
            String::new(),
        ),
        (
            "a grant header naming AllUsers",
            vec![("x-amz-grant-read", "uri=http://acs.amazonaws.com/groups/global/AllUsers")],
            String::new(),
        ),
        (
            "a body granting AllUsers READ",
            vec![],
            group_policy_body(ALL_USERS, "READ"),
        ),
        (
            "a body granting AuthenticatedUsers WRITE",
            vec![],
            group_policy_body(AUTHENTICATED_USERS, "WRITE"),
        ),
    ] {
        let refused = put_acl(&router, bucket, &headers, &body).await;
        assert_eq!(
            refused.status, 403,
            "case {case:?}: a public ACL must be refused with HTTP 403 while \
             BlockPublicAcls is on, got {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "AccessDenied",
            "case {case:?}: expected AccessDenied, got {:?}",
            refused.error_code()
        );
    }

    // An ACL that reaches nobody outside the account is still accepted: the switch blocks
    // PUBLIC grants, not every grant. A grant to the LogDelivery group is to an S3 service
    // principal, which is not the public.
    for (case, headers, body) in [
        (
            "the private canned ACL",
            vec![("x-amz-acl", "private")],
            String::new(),
        ),
        (
            "the log-delivery-write canned ACL",
            vec![("x-amz-acl", "log-delivery-write")],
            String::new(),
        ),
        (
            "a body granting LogDelivery WRITE",
            vec![],
            group_policy_body(LOG_DELIVERY, "WRITE"),
        ),
    ] {
        let accepted = put_acl(&router, bucket, &headers, &body).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: an ACL that reaches nobody public must be accepted, got {} \
             with body {:?}",
            accepted.status,
            String::from_utf8_lossy(&accepted.body)
        );
    }

    // With the switch off the public grant is accepted, so the refusals above are about
    // the stored configuration.
    let off = put_public_access_block(&router, bucket, &[("BlockPublicAcls", false)], &[]).await;
    assert_eq!(off.status, 200, "the switch must be turned off, got {}", off.status);
    let accepted = put_acl(&router, bucket, &[("x-amz-acl", "public-read")], "").await;
    assert_eq!(
        accepted.status, 200,
        "with BlockPublicAcls off the public canned ACL must be accepted, got {} with \
         body {:?}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
}

/// `PUT /<bucket>` — `CreateBucket`, with the ownership header and the canned ACL the case
/// names.
async fn create_bucket_with(
    router: &MultiServiceRouter,
    bucket: &str,
    ownership: Option<&str>,
    acl: Option<&str>,
) -> Answer {
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(ownership) = ownership {
        headers.push(("x-amz-object-ownership", ownership));
    }
    if let Some(acl) = acl {
        headers.push(("x-amz-acl", acl));
    }
    send(router, "PUT", &format!("/{bucket}"), &headers, "").await
}

#[tokio::test]
async fn a_bucket_is_created_with_the_object_ownership_the_request_names() {
    let router = router();

    // (case, the ownership setting the create names, whether ACLs then affect permissions)
    for (index, (case, ownership, acls_enabled)) in [
        ("no header at all, which is the enforced default", None, false),
        ("the enforced setting, named explicitly", Some("BucketOwnerEnforced"), false),
        ("the object-writer setting", Some("ObjectWriter"), true),
        (
            "the bucket-owner-preferred setting",
            Some("BucketOwnerPreferred"),
            true,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let bucket = format!("ownership-on-create-{index}");
        let created = create_bucket_with(&router, &bucket, ownership, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must succeed, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        let put = put_acl(&router, &bucket, &[("x-amz-acl", "private")], "").await;
        if acls_enabled {
            assert_eq!(
                put.status, 200,
                "case {case:?}: ACLs affect permissions under {ownership:?}, so the write \
                 must be accepted, got {} with body {:?}",
                put.status,
                String::from_utf8_lossy(&put.body)
            );
        } else {
            assert_eq!(
                put.error_code(),
                "AccessControlListNotSupported",
                "case {case:?}: ACLs are disabled under {ownership:?}, so even the private \
                 canned ACL is refused, got {:?} with status {}",
                put.error_code(),
                put.status
            );
            assert_eq!(
                put.status, 400,
                "case {case:?}: expected HTTP 400, got {}",
                put.status
            );
        }
    }
}

#[tokio::test]
async fn a_create_naming_both_an_acl_and_the_enforced_setting_is_refused() {
    let router = router();

    // A canned ACL that grants access beyond the bucket owner cannot be applied to a
    // bucket whose Object Ownership setting disables ACLs, so naming both is refused —
    // including the case where the enforced setting is the DEFAULT the create did not have
    // to name. The `private` canned ACL is NOT such an ACL: it expands to exactly the
    // owner's FULL_CONTROL, which is what an ACL-disabled bucket reports anyway, so the
    // row below this table pins that it is served.
    for (index, (case, ownership, acl)) in [
        ("the default enforced setting and a public canned ACL", None, "public-read"),
        (
            "the enforced setting named explicitly, with a public canned ACL",
            Some("BucketOwnerEnforced"),
            "public-read-write",
        ),
        (
            "the enforced setting with the log-delivery canned ACL",
            Some("BucketOwnerEnforced"),
            "log-delivery-write",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let bucket = format!("ownership-acl-conflict-{index}");
        let refused = create_bucket_with(&router, &bucket, ownership, Some(acl)).await;
        assert_eq!(
            refused.error_code(),
            "InvalidBucketAclWithObjectOwnership",
            "case {case:?}: expected the documented refusal, got {:?} with status {} and \
             body {:?}",
            refused.error_code(),
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {}",
            refused.status
        );

        // The refused create left no bucket behind.
        let read = send(&router, "HEAD", &format!("/{bucket}"), &[], "").await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create must not have recorded the bucket, got {}",
            read.status
        );
    }

    // And a create that names an ACL beside a setting under which ACLs affect permissions
    // is served, with the ACL applied: the refusal above is about the combination.
    let created =
        create_bucket_with(&router, "ownership-acl-ok", Some("ObjectWriter"), Some("public-read"))
            .await;
    assert_eq!(
        created.status, 200,
        "a create naming an ACL beside ObjectWriter must succeed, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let read = send(&router, "GET", "/ownership-acl-ok?acl", &[], "").await;
    let body = String::from_utf8_lossy(&read.body);
    assert!(
        body.contains(ALL_USERS),
        "the canned ACL the create named must be the bucket's ACL, got {body:?}"
    );

    // The `private` canned ACL grants nothing beyond the bucket owner, so it is served
    // beside the enforced setting too: the refusals above are about an ACL that reaches
    // further than the owner, which is what the published wording of that error names.
    for (case, bucket, ownership) in [
        ("the default enforced setting", "ownership-acl-private-default", None),
        (
            "the enforced setting named explicitly",
            "ownership-acl-private-named",
            Some("BucketOwnerEnforced"),
        ),
    ] {
        let created = create_bucket_with(&router, bucket, ownership, Some("private")).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the private canned ACL grants nothing beyond the owner, so the \
             create must succeed, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        // And the bucket is still one whose ACLs are disabled.
        let put = put_acl(&router, bucket, &[("x-amz-acl", "private")], "").await;
        assert_eq!(
            put.error_code(),
            "AccessControlListNotSupported",
            "case {case:?}: the created bucket must still have ACLs disabled, got {:?}",
            put.error_code()
        );
    }
}

/// Dispatch one request signed for `region`, so the emulator sees the endpoint a client of
/// that Region addressed it as.
///
/// The suite's shared harness signs every request for `us-east-1`; the location-constraint
/// rule is about the Region in the credential scope, so this is the one place a request
/// carries another one.
async fn send_in_region(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: &str,
) -> Answer {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{region}/s3/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    );
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{region}.amazonaws.com{uri}"))
        .header("host", format!("s3.{region}.amazonaws.com"))
        .header("authorization", authorization)
        .header("content-type", "application/xml")
        .body(bytes::Bytes::copy_from_slice(body.as_bytes()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The `CreateBucketConfiguration` payload naming `constraint`, or no body at all for a
/// create that names none.
fn location_payload(constraint: Option<&str>) -> String {
    match constraint {
        None => String::new(),
        Some(constraint) => format!(
            "<CreateBucketConfiguration xmlns=\"{XMLNS}\">\
             <LocationConstraint>{constraint}</LocationConstraint>\
             </CreateBucketConfiguration>"
        ),
    }
}

#[tokio::test]
async fn a_create_naming_no_location_constraint_is_served_only_by_the_us_east_1_endpoint() {
    let router = router();

    // Regional endpoints, differing in their prefix and in how far they are from the one
    // Region the unspecified constraint means: a create naming no constraint is refused by
    // every one of them.
    for (index, region) in ["us-west-2", "eu-west-1", "ap-northeast-3", "us-gov-west-1"]
        .into_iter()
        .enumerate()
    {
        let bucket = format!("location-refused-{index}");
        let refused = send_in_region(
            &router,
            region,
            "PUT",
            &format!("/{bucket}"),
            &location_payload(None),
        )
        .await;
        assert_eq!(
            refused.error_code(),
            "IllegalLocationConstraintException",
            "case {region:?}: a create naming no LocationConstraint cannot be served by a \
             regional endpoint, got {:?} with status {} and body {:?}",
            refused.error_code(),
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.status, 400,
            "case {region:?}: expected HTTP 400, got {}",
            refused.status
        );

        // Nothing was recorded, and the SAME endpoint serves the create once it names the
        // constraint — so the refusal is about the missing constraint, not about the
        // endpoint being unreachable.
        let named = send_in_region(
            &router,
            region,
            "PUT",
            &format!("/{bucket}"),
            &location_payload(Some(region)),
        )
        .await;
        assert_eq!(
            named.status, 200,
            "case {region:?}: the create naming the constraint must succeed, got {} with \
             body {:?}",
            named.status,
            String::from_utf8_lossy(&named.body)
        );
    }

    // The one endpoint the unspecified constraint names serves it: this is the asymmetry
    // the rule is about.
    let created = send_in_region(
        &router,
        "us-east-1",
        "PUT",
        "/location-default-endpoint",
        &location_payload(None),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the us-east-1 endpoint serves a create naming no constraint, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
}
