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
//! * The create endpoint. An endpoint for ONE Region can only create a bucket IN that
//!   Region, so the `LocationConstraint` a `CreateBucket` carries has to name that Region —
//!   where an absent constraint names "the US East (N. Virginia) Region (us-east-1) by
//!   default" (`CreateBucketConfiguration$LocationConstraint`) and the legacy `EU` names
//!   `eu-west-1`. A constraint naming any other Region is refused with
//!   `IllegalLocationConstraintException` (HTTP 400), whose message names the constraint the
//!   request carried, and nothing is created. The GLOBAL endpoint is not an endpoint for one
//!   Region — "if you send your CreateBucket request to the s3.amazonaws.com global
//!   endpoint, the request goes to the us-east-1 Region … even if the location constraint in
//!   the request specifies another Region" (`CreateBucket`'s own documentation) — so it
//!   serves a create for any of them.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{
    create_bucket, get_bucket_policy, put_bucket_policy, router, send, send_bytes_to_host,
    send_in_region, Answer,
};
use super::bucket_ownership_controls::put_ownership_controls;
use super::bucket_public_access_block::{allow_public_access, put_public_access_block};

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
    // The bucket is created with `IgnorePublicAcls` on, under which S3 keeps a public grant
    // out of every ACL read, so the guardrail is lifted before the stored ACL is read back.
    allow_public_access(&router, "ownership-acl-ok").await;
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

/// A create addressed to the endpoint of `region`, carrying `body`.
///
/// The suite's shared harness signs every request for `us-east-1`; the location-constraint
/// rule is about WHICH endpoint the request reached, so these rows go through the harness's
/// own per-Region sender ([`send_in_region`]).
async fn create_at_endpoint(
    router: &MultiServiceRouter,
    region: &str,
    bucket: &str,
    body: &str,
) -> Answer {
    send_in_region(router, region, "PUT", &format!("/{bucket}"), &[], body).await
}

/// The host of S3's GLOBAL endpoint, the one endpoint that is not an endpoint for a single
/// Region.
const GLOBAL_ENDPOINT: &str = "s3.amazonaws.com";

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
        let refused = create_at_endpoint(&router, region, &bucket, &location_payload(None)).await;
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
        let named =
            create_at_endpoint(&router, region, &bucket, &location_payload(Some(region))).await;
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
    let created = create_at_endpoint(
        &router,
        "us-east-1",
        "location-default-endpoint",
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

/// One row of the named-constraint table: the endpoint's Region, the constraint the create
/// carries, and whether that endpoint can place the bucket.
#[derive(Debug, Clone, Copy)]
struct EndpointRow {
    case: &'static str,
    endpoint_region: &'static str,
    constraint: &'static str,
    served: bool,
}

#[tokio::test]
async fn a_named_location_constraint_is_served_only_by_the_endpoint_of_its_own_region() {
    let router = router();

    // The rows differ along what the rule generalises over: which Region the endpoint is
    // for, how far the constraint is from it (another Region of the same partition, a
    // Region of another partition, the default Region named explicitly), and the FORM the
    // constraint takes — the legacy `EU` alias and the Region code it stands for are one
    // Region, so each is served by that Region's endpoint and refused elsewhere.
    let rows: &[EndpointRow] = &[
        EndpointRow {
            case: "another Region's constraint on a regional endpoint",
            endpoint_region: "us-east-1",
            constraint: "eu-west-1",
            served: false,
        },
        EndpointRow {
            case: "a neighbouring Region's constraint",
            endpoint_region: "us-west-2",
            constraint: "us-west-1",
            served: false,
        },
        EndpointRow {
            case: "a constraint of another partition",
            endpoint_region: "us-west-2",
            constraint: "cn-north-1",
            served: false,
        },
        EndpointRow {
            case: "the legacy EU alias on an endpoint that is not eu-west-1",
            endpoint_region: "us-east-1",
            constraint: "EU",
            served: false,
        },
        EndpointRow {
            case: "the endpoint's own Region",
            endpoint_region: "ap-northeast-3",
            constraint: "ap-northeast-3",
            served: true,
        },
        EndpointRow {
            case: "the legacy EU alias on the endpoint of the Region it stands for",
            endpoint_region: "eu-west-1",
            constraint: "EU",
            served: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let EndpointRow {
            case,
            endpoint_region,
            constraint,
            served,
        } = *row;
        let bucket = format!("constraint-endpoint-{index}");
        let answer = create_at_endpoint(
            &router,
            endpoint_region,
            &bucket,
            &location_payload(Some(constraint)),
        )
        .await;
        if served {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            continue;
        }
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "IllegalLocationConstraintException",
            "case {case:?}: expected the error code \"IllegalLocationConstraintException\", \
             got {:?}",
            answer.error_code()
        );
        // The message names the constraint the request carried, as it carried it.
        let expected = format!(
            "The {constraint} location constraint is incompatible for the region specific \
             endpoint this request was sent to."
        );
        assert_eq!(
            answer.error_message(),
            expected,
            "case {case:?}: expected the message {expected:?}, got {:?}",
            answer.error_message()
        );
        // Refused before anything was written: the bucket is not there afterwards. Read at
        // the same endpoint, since a read of a bucket this emulator does not hold answers
        // about the address rather than about the Region.
        let read = send(&router, "HEAD", &format!("/{bucket}"), &[], "").await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create must leave no bucket behind, got {}",
            read.status
        );
    }
}

#[tokio::test]
async fn the_global_endpoint_serves_a_create_for_any_region() {
    let router = router();

    // The global endpoint is not an endpoint for one Region, so every constraint — and no
    // constraint at all — is served there, including the ones a regional endpoint refuses
    // above. `us-east-1` is not one of them: it is not a `BucketLocationConstraint` value
    // at all (that Region is what naming NO constraint means).
    for (index, constraint) in [None, Some("eu-west-1"), Some("EU"), Some("ap-northeast-3")]
        .into_iter()
        .enumerate()
    {
        let bucket = format!("global-endpoint-{index}");
        let created = send_bytes_to_host(
            &router,
            "PUT",
            GLOBAL_ENDPOINT,
            &format!("/{bucket}"),
            &[("content-type", "application/xml")],
            location_payload(constraint).as_bytes(),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {constraint:?}: the global endpoint serves the create, got {} with body \
             {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
    }
}
