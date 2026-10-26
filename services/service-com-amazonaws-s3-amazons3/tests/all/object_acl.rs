// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an OBJECT's access control list: `PutObjectAcl` stores it and
//! `GetObjectAcl` reports it.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns), so what is pinned is the wire behaviour —
//! status, body, error code — rather than any handler's internals. The document readers
//! and the `AccessControlPolicy` writer come from the bucket ACL suite beside this one:
//! both operations carry the same `AccessControlPolicy` shape, so there is one parser for
//! it.
//!
//! The rules under test, with their sources:
//!
//! * `GetObjectAcl` "returns the access control list (ACL) of an object" — the grants last
//!   set on the addressed version, in the order the ACL names them, beside the owner — and
//!   an object nobody has set an ACL on reports the ACL every object starts with, the
//!   owner's `FULL_CONTROL` alone, without one being written. "By default, GET returns ACL
//!   information about the current version of an object. To return ACL information about a
//!   different version, use the versionId subresource" (that operation's own
//!   documentation).
//! * `PutObjectAcl` "uses the acl subresource to set the access control list (ACL)
//!   permissions for a new or existing object" from the ONE ACL the request names — a
//!   canned ACL in `x-amz-acl`, explicit grants in the `x-amz-grant-*` headers, or an
//!   `AccessControlPolicy` body — and that ACL becomes the object's ACL IN FULL: a
//!   replacement, not a merge. "You cannot specify access permission using both the body
//!   and the request headers", so a request naming its ACL in two ways is refused, and so
//!   is one naming it in none. Every refusal leaves the stored ACL untouched.
//! * "If your bucket uses the bucket owner enforced setting for S3 Object Ownership, ACLs
//!   are disabled … Requests to set ACLs or update ACLs fail and return the
//!   AccessControlListNotSupported error code. Requests to read ACLs are still supported"
//!   and "return the bucket-owner-full-control ACL" — and because disabling ACLs takes
//!   nothing away, the grants the object stores are reported again once the setting is
//!   changed back.
//! * With `BlockPublicAcls` on, "PUT Bucket ACL and PUT Object ACL calls fail if the
//!   specified ACL is public" (`PublicAccessBlockConfiguration$BlockPublicAcls`).
//! * An ACL call naming an address this emulator does not hold answers the service's own
//!   404 for the missing link — `NoSuchBucket`, `NoSuchKey` or `NoSuchVersion` — and one
//!   whose `x-amz-expected-bucket-owner` is not the bucket's owner answers `AccessDenied`
//!   (HTTP 403).

use service_com_amazonaws_s3_amazons3::state::{
    canonical_user_id, ALL_USERS_GROUP_URI, AUTHENTICATED_USERS_GROUP_URI, GRANTEE_CANONICAL_USER,
    GRANTEE_GROUP, LOG_DELIVERY_GROUP_URI, MAX_ACL_GRANTS, OBJECT_CANNED_A_C_L_VALUES,
    PERMISSION_FULL_CONTROL, PERMISSION_READ, PERMISSION_READ_ACP, PERMISSION_WRITE,
    PERMISSION_WRITE_ACP,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_acl::{
    access_control_policy, default_acl, group_grant, reported_acl, reported_grants, user_grant,
    GrantXml, ReportedGrant,
};
use super::bucket_lifecycle::{create_bucket, router, send, send_bytes, Answer};
use super::bucket_ownership_controls::put_ownership_controls;
use super::bucket_public_access_block::{allow_public_access, put_public_access_block};
use super::object_and_tagging_roundtrip::put_object;
use super::object_versioning::set_versioning;

/// The bytes every object in this suite is written with: an ACL is about who may reach an
/// object, not about what is in it, so one payload serves every row.
const BODY: &[u8] = b"acl-subject";

/// The header S3 reports a stored version's id in.
const VERSION_ID_HEADER: &str = "x-amz-version-id";

/// `PUT /<bucket>/<key>?acl<query>` — `PutObjectAcl`, with whatever headers and body the
/// case names.
async fn put_object_acl(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    query: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}/{key}?acl{query}"),
        headers,
        body,
    )
    .await
}

/// `GET /<bucket>/<key>?acl<query>` — `GetObjectAcl`.
async fn get_object_acl(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    query: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send_bytes(
        router,
        "GET",
        &format!("/{bucket}/{key}?acl{query}"),
        headers,
        b"",
    )
    .await
}

/// `GET /<bucket>?acl` — `GetBucketAcl`, the bucket half of the same subresource: the
/// Block Public Access rule under test covers "this bucket and any objects that it
/// contains", so the bucket's own ACL is read beside the object's.
async fn get_bucket_acl(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "GET", &format!("/{bucket}?acl"), &[], "").await
}

/// `PUT /<bucket>?acl` — `PutBucketAcl`, with whatever headers the case names.
async fn put_bucket_acl(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(router, "PUT", &format!("/{bucket}?acl"), headers, "").await
}

/// A bucket holding `key`, with its ACLs ENABLED.
///
/// A bucket is created with the Object Ownership setting that DISABLES ACLs — "by default,
/// `ObjectOwnership` is set to `BucketOwnerEnforced` and ACLs are disabled"
/// (`smithy_query(query="shape",shape="ObjectOwnership")`) — so every ACL write on a fresh
/// bucket is refused `AccessControlListNotSupported`. The suites below are about what an
/// ACL write DOES, so each of their buckets is switched to `ObjectWriter`, the setting
/// under which ACLs affect permissions; the tests that are about the disabled posture
/// switch it back themselves.
///
/// Block Public Access is lifted for the same reason: a bucket is created with all four
/// switches on, so a PUBLIC ACL would be denied by that guardrail before anything about the
/// ACL write itself could be observed. The tests that are about THAT guardrail turn the
/// switches back on themselves.
async fn bucket_holding(router: &MultiServiceRouter, bucket: &str, key: &str) {
    let created = create_bucket(router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket {bucket:?} must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let enabled = put_ownership_controls(router, bucket, &["ObjectWriter"]).await;
    assert_eq!(
        enabled.status, 200,
        "the bucket {bucket:?} must take the ObjectWriter ownership setting that enables \
         ACLs, got {} with body {:?}",
        enabled.status,
        String::from_utf8_lossy(&enabled.body)
    );
    allow_public_access(router, bucket).await;
    let written = put_object(router, bucket, key, BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the object {key:?} must be written first, got {} with body {:?}",
        written.status,
        String::from_utf8_lossy(&written.body)
    );
}

/// One row of the round-trip table: the case, the request headers, the request body, and
/// the grants the object must report afterwards.
type AclRow = (
    &'static str,
    Vec<(&'static str, String)>,
    String,
    Vec<ReportedGrant>,
);

/// The table of ACLs every row of which must round-trip unchanged.
///
/// The rows differ along what the store-and-report rule generalises over: WHICH of the
/// three ways the request names its ACL, WHICH canned ACL (every value `ObjectCannedACL`
/// publishes, including the three a bucket's ACL has no row for), WHICH permission and
/// grantee type a grant names, HOW MANY grantees one header carries, and whether a header
/// value is quoted. The ACL the recorded calls send (a public-read grant, and an explicit
/// grant header replacing the owner's grant) are two rows of it.
fn round_trip_rows() -> Vec<AclRow> {
    let owner = canonical_user_id();
    let mut rows: Vec<AclRow> = vec![
        (
            "the canned ACL private: owner FULL_CONTROL and nobody else",
            vec![("x-amz-acl", "private".to_string())],
            String::new(),
            default_acl(),
        ),
        (
            "the canned ACL public-read: AllUsers READ beside the owner",
            vec![("x-amz-acl", "public-read".to_string())],
            String::new(),
            vec![
                user_grant(&owner, PERMISSION_FULL_CONTROL),
                group_grant(ALL_USERS_GROUP_URI, PERMISSION_READ),
            ],
        ),
        (
            "the canned ACL public-read-write: AllUsers READ and WRITE",
            vec![("x-amz-acl", "public-read-write".to_string())],
            String::new(),
            vec![
                user_grant(&owner, PERMISSION_FULL_CONTROL),
                group_grant(ALL_USERS_GROUP_URI, PERMISSION_READ),
                group_grant(ALL_USERS_GROUP_URI, PERMISSION_WRITE),
            ],
        ),
        (
            "the canned ACL authenticated-read: AuthenticatedUsers READ",
            vec![("x-amz-acl", "authenticated-read".to_string())],
            String::new(),
            vec![
                user_grant(&owner, PERMISSION_FULL_CONTROL),
                group_grant(AUTHENTICATED_USERS_GROUP_URI, PERMISSION_READ),
            ],
        ),
        (
            // "bucket-owner-read — Object owner gets FULL_CONTROL. Bucket owner gets READ"
            // (Amazon S3 User Guide, Canned ACL): this emulator serves one account, so both
            // grants name the same canonical user.
            "the canned ACL bucket-owner-read: the owner's FULL_CONTROL and READ",
            vec![("x-amz-acl", "bucket-owner-read".to_string())],
            String::new(),
            vec![
                user_grant(&owner, PERMISSION_FULL_CONTROL),
                user_grant(&owner, PERMISSION_READ),
            ],
        ),
        (
            // "bucket-owner-full-control — Both the object owner and the bucket owner get
            // FULL_CONTROL" (same table), which is one grant when the two are one account.
            "the canned ACL bucket-owner-full-control: the owner's FULL_CONTROL once",
            vec![("x-amz-acl", "bucket-owner-full-control".to_string())],
            String::new(),
            default_acl(),
        ),
        (
            // "aws-exec-read — Owner gets FULL_CONTROL" (same table); the READ that row
            // also gives goes to an Amazon EC2 grantee whose canonical user id AWS
            // publishes nowhere, so none is invented for it.
            "the canned ACL aws-exec-read: the owner's FULL_CONTROL",
            vec![("x-amz-acl", "aws-exec-read".to_string())],
            String::new(),
            default_acl(),
        ),
        (
            "one grant header naming the log-delivery group by its URI",
            vec![(
                "x-amz-grant-read",
                format!("uri={LOG_DELIVERY_GROUP_URI}"),
            )],
            String::new(),
            vec![group_grant(LOG_DELIVERY_GROUP_URI, PERMISSION_READ)],
        ),
        (
            "a grant header REPLACING the owner's grant with one of its own",
            vec![("x-amz-grant-read-acp", format!("uri={ALL_USERS_GROUP_URI}"))],
            String::new(),
            vec![group_grant(ALL_USERS_GROUP_URI, PERMISSION_READ_ACP)],
        ),
        (
            "two grant headers, applied in the model's member order",
            vec![
                ("x-amz-grant-full-control", format!("id=\"{owner}\"")),
                ("x-amz-grant-read", format!("uri={ALL_USERS_GROUP_URI}")),
            ],
            String::new(),
            vec![
                user_grant(&owner, PERMISSION_FULL_CONTROL),
                group_grant(ALL_USERS_GROUP_URI, PERMISSION_READ),
            ],
        ),
        (
            "one grant header naming TWO grantees, one quoted and one not",
            vec![(
                "x-amz-grant-read",
                format!("uri=\"{ALL_USERS_GROUP_URI}\", uri={AUTHENTICATED_USERS_GROUP_URI}"),
            )],
            String::new(),
            vec![
                group_grant(ALL_USERS_GROUP_URI, PERMISSION_READ),
                group_grant(AUTHENTICATED_USERS_GROUP_URI, PERMISSION_READ),
            ],
        ),
        (
            "a grant header whose type is spelled in another case",
            vec![("x-amz-grant-write-acp", format!("URI={ALL_USERS_GROUP_URI}"))],
            String::new(),
            vec![group_grant(ALL_USERS_GROUP_URI, PERMISSION_WRITE_ACP)],
        ),
        (
            "an AccessControlPolicy body naming the owner and two grants",
            vec![],
            access_control_policy(
                Some(&owner),
                &[
                    GrantXml::user(&owner, PERMISSION_FULL_CONTROL),
                    GrantXml::group(ALL_USERS_GROUP_URI, PERMISSION_READ_ACP),
                ],
            ),
            vec![
                user_grant(&owner, PERMISSION_FULL_CONTROL),
                group_grant(ALL_USERS_GROUP_URI, PERMISSION_READ_ACP),
            ],
        ),
        (
            "an AccessControlPolicy body naming no owner and one grant",
            vec![],
            access_control_policy(None, &[GrantXml::group(LOG_DELIVERY_GROUP_URI, PERMISSION_WRITE)]),
            vec![group_grant(LOG_DELIVERY_GROUP_URI, PERMISSION_WRITE)],
        ),
    ];
    // One row per published `Permission` value, each in a body grant to the same account,
    // so the whole catalog is exercised rather than the four the rows above happen to name.
    for permission in [
        PERMISSION_FULL_CONTROL,
        PERMISSION_READ,
        PERMISSION_READ_ACP,
        PERMISSION_WRITE,
        PERMISSION_WRITE_ACP,
    ] {
        rows.push((
            "a body grant naming one published Permission",
            vec![],
            access_control_policy(None, &[GrantXml::user(&owner, permission)]),
            vec![user_grant(&owner, permission)],
        ));
    }
    rows
}

#[tokio::test]
async fn putting_an_object_acl_stores_it_and_reads_back_through_get() {
    let router = router();
    let owner = canonical_user_id();

    for (index, (case, headers, body, expected)) in round_trip_rows().into_iter().enumerate() {
        let bucket = format!("object-acl-round-trip-{index}");
        // Keys differing in shape, so the address resolution is exercised over more than
        // one spelling: a plain key, a nested one, and one carrying a dot segment.
        let key = ["acl-subject.txt", "nested/acl-subject.txt", "v1.2/object"][index % 3];
        bucket_holding(&router, &bucket, key).await;

        let borrowed: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let put = put_object_acl(&router, &bucket, key, "", &borrowed, &body).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected the modelled 200 from PutObjectAcl, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );

        let read = get_object_acl(&router, &bucket, key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetObjectAcl on the object just \
             configured, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let (reported_owner, grants) = reported_acl(&read);
        assert_eq!(
            reported_owner.as_deref(),
            Some(owner.as_str()),
            "case {case:?}: the ACL's owner is the account that holds the object, got \
             {reported_owner:?}"
        );
        assert_eq!(
            grants, expected,
            "case {case:?}: expected the stored grants back unchanged, got {grants:?}"
        );

        // The second put REPLACES: what comes back is the new ACL alone, with nothing
        // merged in from the ACL above.
        let replacement = put_object_acl(
            &router,
            &bucket,
            key,
            "",
            &[("x-amz-grant-read", &format!("uri={ALL_USERS_GROUP_URI}"))],
            "",
        )
        .await;
        assert_eq!(
            replacement.status, 200,
            "case {case:?}: the replacing put must succeed, got {} with body {:?}",
            replacement.status,
            String::from_utf8_lossy(&replacement.body)
        );
        let reread = get_object_acl(&router, &bucket, key, "", &[]).await;
        assert_eq!(
            reported_grants(&reread),
            vec![group_grant(ALL_USERS_GROUP_URI, PERMISSION_READ)],
            "case {case:?}: the ACL must be replaced, not merged, got {:?}",
            reported_grants(&reread)
        );
    }
}

#[tokio::test]
async fn every_published_object_canned_acl_is_accepted() {
    let router = router();

    // The catalog comes from the model's own value list for `ObjectCannedACL`
    // (`OBJECT_CANNED_A_C_L_VALUES`), so a regenerated model cannot leave a value here
    // untested — including `aws-exec-read`, which no row of the table above names by hand.
    for (index, canned) in OBJECT_CANNED_A_C_L_VALUES.iter().enumerate() {
        let bucket = format!("object-acl-canned-{index}");
        let key = "canned.txt";
        bucket_holding(&router, &bucket, key).await;

        let put = put_object_acl(&router, &bucket, key, "", &[("x-amz-acl", canned)], "").await;
        assert_eq!(
            put.status, 200,
            "case {canned:?}: a published canned ACL must be accepted, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        let grants = reported_grants(&get_object_acl(&router, &bucket, key, "", &[]).await);
        assert!(
            grants.contains(&user_grant(&canonical_user_id(), PERMISSION_FULL_CONTROL)),
            "case {canned:?}: every canned ACL for an object gives its owner FULL_CONTROL, \
             got {grants:?}"
        );
    }

    // `log-delivery-write` is a BUCKET canned ACL: the object enum does not publish it, so
    // the boundary refuses it before the handler runs.
    let bucket = "object-acl-canned-outside";
    let key = "canned.txt";
    bucket_holding(&router, bucket, key).await;
    for canned in ["log-delivery-write", "PRIVATE", "public"] {
        let refused = put_object_acl(&router, bucket, key, "", &[("x-amz-acl", canned)], "").await;
        assert_eq!(
            refused.status, 400,
            "case {canned:?}: a canned ACL outside {OBJECT_CANNED_A_C_L_VALUES:?} must be \
             refused, got {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            reported_grants(&get_object_acl(&router, bucket, key, "", &[]).await),
            default_acl(),
            "case {canned:?}: a refused put must leave the stored ACL untouched"
        );
    }
}

#[tokio::test]
async fn an_object_with_no_acl_reports_the_owner_full_control_alone() {
    let router = router();

    // Objects nobody has set an ACL on, differing in the bucket's ACL posture and in how
    // the object was stored, so the answer is about the default ACL rather than about one
    // object.
    for (case, bucket, acls_enabled) in [
        ("an object of a bucket whose ACLs are enabled", "object-acl-default-enabled", true),
        (
            "an object of a bucket whose ACLs are disabled",
            "object-acl-default-disabled",
            false,
        ),
    ] {
        let key = "untouched.txt";
        bucket_holding(&router, bucket, key).await;
        if !acls_enabled {
            let disabled = put_ownership_controls(&router, bucket, &["BucketOwnerEnforced"]).await;
            assert_eq!(
                disabled.status, 200,
                "case {case:?}: the bucket must take the setting that disables ACLs, got {}",
                disabled.status
            );
        }

        let read = get_object_acl(&router, bucket, key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: reading the ACL of an object nobody configured answers 200, got \
             {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            reported_grants(&read),
            default_acl(),
            "case {case:?}: the object must report the owner's FULL_CONTROL alone, got {:?}",
            reported_grants(&read)
        );

        // Reading twice changes nothing: the read reports, it does not configure.
        let again = get_object_acl(&router, bucket, key, "", &[]).await;
        assert_eq!(
            reported_grants(&again),
            default_acl(),
            "case {case:?}: a second read must answer the same way, got {:?}",
            reported_grants(&again)
        );
    }
}

#[tokio::test]
async fn the_acl_of_one_version_is_not_the_acl_of_another() {
    let router = router();
    let bucket = "object-acl-versions";
    let key = "versioned.txt";
    bucket_holding(&router, bucket, key).await;
    set_versioning(&router, bucket, "Enabled").await;

    // Two versions of the same key, each addressable by the id the store minted for it.
    let mut version_ids: Vec<String> = Vec::new();
    for round in 0..2 {
        let written = put_object(&router, bucket, key, BODY, &[]).await;
        let version_id = written.header(VERSION_ID_HEADER).unwrap_or_default();
        assert!(
            !version_id.is_empty(),
            "case {round}: a write to a versioning-enabled bucket must report a version id, \
             got {:?}",
            written.header(VERSION_ID_HEADER)
        );
        version_ids.push(version_id);
    }
    let (older, current) = (&version_ids[0], &version_ids[1]);

    // The ACL is set on the OLDER version alone.
    let put = put_object_acl(
        &router,
        bucket,
        key,
        &format!("&versionId={older}"),
        &[("x-amz-grant-read", &format!("uri={ALL_USERS_GROUP_URI}"))],
        "",
    )
    .await;
    assert_eq!(
        put.status, 200,
        "an ACL named for a stored version must be accepted, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );

    for (case, query, expected) in [
        (
            "the version the ACL was set on",
            format!("&versionId={older}"),
            vec![group_grant(ALL_USERS_GROUP_URI, PERMISSION_READ)],
        ),
        (
            "the current version, which it was not set on",
            format!("&versionId={current}"),
            default_acl(),
        ),
        (
            "no version at all, which addresses the current one",
            String::new(),
            default_acl(),
        ),
    ] {
        let read = get_object_acl(&router, bucket, key, &query, &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            reported_grants(&read),
            expected,
            "case {case:?}: expected the ACL of that version alone, got {:?}",
            reported_grants(&read)
        );
    }
}

#[tokio::test]
async fn disabling_acls_hides_the_stored_grants_and_enabling_them_again_restores_them() {
    let router = router();
    let bucket = "object-acl-ownership";
    let key = "owned.txt";
    bucket_holding(&router, bucket, key).await;

    // The ACL the object carries throughout this test.
    let stored = vec![
        user_grant(&canonical_user_id(), PERMISSION_FULL_CONTROL),
        group_grant(ALL_USERS_GROUP_URI, PERMISSION_READ),
    ];
    let put = put_object_acl(&router, bucket, key, "", &[("x-amz-acl", "public-read")], "").await;
    assert_eq!(
        put.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );
    assert_eq!(
        reported_grants(&get_object_acl(&router, bucket, key, "", &[]).await),
        stored,
        "the object must report the ACL just set on it"
    );

    // Every value of `ObjectOwnership` that disables ACLs, then every one that does not:
    // the read answers the bucket-owner-full-control ACL while they are disabled and the
    // stored grants again once they affect permissions.
    for (case, ownership, hides) in [
        ("BucketOwnerEnforced, which disables ACLs", "BucketOwnerEnforced", true),
        ("ObjectWriter, under which ACLs apply", "ObjectWriter", false),
        (
            "BucketOwnerEnforced again, so the switch is not one-way",
            "BucketOwnerEnforced",
            true,
        ),
        (
            "BucketOwnerPreferred, under which ACLs apply",
            "BucketOwnerPreferred",
            false,
        ),
    ] {
        let switched = put_ownership_controls(&router, bucket, &[ownership]).await;
        assert_eq!(
            switched.status, 200,
            "case {case:?}: the bucket must take the ownership setting, got {} with body {:?}",
            switched.status,
            String::from_utf8_lossy(&switched.body)
        );

        // "Requests to read ACLs are still supported" either way.
        let read = get_object_acl(&router, bucket, key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: reading an object's ACL answers 200 whatever the ownership \
             setting is, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let expected = if hides { default_acl() } else { stored.clone() };
        assert_eq!(
            reported_grants(&read),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reported_grants(&read)
        );

        // And a WRITE is refused exactly while they are disabled, in every form that names
        // an ACL.
        for (form, headers, body) in [
            (
                "a canned ACL",
                vec![("x-amz-acl", "private")],
                String::new(),
            ),
            (
                "a grant header",
                vec![("x-amz-grant-read", "uri=http://acs.amazonaws.com/groups/s3/LogDelivery")],
                String::new(),
            ),
            (
                "an AccessControlPolicy body",
                vec![],
                access_control_policy(
                    None,
                    &[GrantXml::user(&canonical_user_id(), PERMISSION_FULL_CONTROL)],
                ),
            ),
        ] {
            let answer = put_object_acl(&router, bucket, key, "", &headers, &body).await;
            if hides {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}/{form}: a bucket with ACLs disabled refuses the put with \
                     HTTP 400, got {} with body {:?}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                assert_eq!(
                    answer.error_code(),
                    "AccessControlListNotSupported",
                    "case {case:?}/{form}: expected the documented error code, got {:?}",
                    answer.error_code()
                );
                assert_eq!(
                    answer.error_message(),
                    "The bucket does not allow ACLs",
                    "case {case:?}/{form}: expected the documented message, got {:?}",
                    answer.error_message()
                );
            } else {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}/{form}: a bucket whose ACLs apply must take the put, got \
                     {} with body {:?}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
            }
        }

        // Whatever the form above did, the ACL under test is put back, so each row judges
        // the ownership setting rather than the row before it.
        if !hides {
            let restored =
                put_object_acl(&router, bucket, key, "", &[("x-amz-acl", "public-read")], "").await;
            assert_eq!(
                restored.status, 200,
                "case {case:?}: the ACL must be restorable, got {}",
                restored.status
            );
        }
    }
}

#[tokio::test]
async fn block_public_acls_refuses_a_public_object_acl_and_keeps_the_stored_acl() {
    let router = router();
    let bucket = "object-acl-guardrail";
    let key = "guarded.txt";
    bucket_holding(&router, bucket, key).await;

    let on = put_public_access_block(&router, bucket, &[("BlockPublicAcls", true)], &[]).await;
    assert_eq!(
        on.status, 200,
        "BlockPublicAcls must be stored, got {} with body {:?}",
        on.status,
        String::from_utf8_lossy(&on.body)
    );

    // Every row names a PUBLIC ACL, differing in the form it arrives in and in which
    // predefined group it reaches: "an ACL is public when it grants access to AllUsers or
    // AuthenticatedUsers" (Amazon S3 User Guide, The Meaning of Public).
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
            "a body granting AuthenticatedUsers WRITE",
            vec![],
            access_control_policy(
                None,
                &[GrantXml::group(AUTHENTICATED_USERS_GROUP_URI, PERMISSION_WRITE)],
            ),
        ),
    ] {
        let refused = put_object_acl(&router, bucket, key, "", &headers, &body).await;
        assert_eq!(
            refused.status, 403,
            "case {case:?}: a public ACL must be refused with HTTP 403 while BlockPublicAcls \
             is on, got {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "AccessDenied",
            "case {case:?}: expected AccessDenied, got {:?}",
            refused.error_code()
        );
        // The refusal names who was refused, which action, which resource and the setting
        // that prevented it — every part of it taken from the request and from this
        // emulator's own identity.
        let expected_message = format!(
            "User: arn:aws:iam::{}:user/emulator is not authorized to perform: s3:PutObjectAcl \
             on resource: \"arn:aws:s3:::{bucket}/{key}\" because public ACLs are prevented by \
             the BlockPublicAcls setting in S3 Block Public Access.",
            engine::emulator_account_id()
        );
        assert_eq!(
            refused.error_message(),
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {:?}",
            refused.error_message()
        );
        assert_eq!(
            reported_grants(&get_object_acl(&router, bucket, key, "", &[]).await),
            default_acl(),
            "case {case:?}: a refused put must leave the stored ACL untouched"
        );
    }

    // An ACL that reaches nobody outside the account is still accepted: the switch blocks
    // PUBLIC grants, not every grant.
    for (case, headers, body) in [
        (
            "the private canned ACL",
            vec![("x-amz-acl", "private")],
            String::new(),
        ),
        (
            "the bucket-owner-full-control canned ACL",
            vec![("x-amz-acl", "bucket-owner-full-control")],
            String::new(),
        ),
        (
            "a body granting LogDelivery WRITE",
            vec![],
            access_control_policy(
                None,
                &[GrantXml::group(LOG_DELIVERY_GROUP_URI, PERMISSION_WRITE)],
            ),
        ),
    ] {
        let accepted = put_object_acl(&router, bucket, key, "", &headers, &body).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: an ACL that reaches nobody public must be accepted, got {} with \
             body {:?}",
            accepted.status,
            String::from_utf8_lossy(&accepted.body)
        );
    }

    // With the switch off the public ACL is accepted, so the refusals above are about the
    // stored configuration.
    let off = put_public_access_block(&router, bucket, &[("BlockPublicAcls", false)], &[]).await;
    assert_eq!(off.status, 200, "the switch must be turned off, got {}", off.status);
    let accepted = put_object_acl(&router, bucket, key, "", &[("x-amz-acl", "public-read")], "").await;
    assert_eq!(
        accepted.status, 200,
        "with BlockPublicAcls off the public canned ACL must be accepted, got {} with body {:?}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
}

/// One row of the refusal table: the case, the request headers, the request body, and the
/// wire error code the refusal must carry.
type RefusalRow = (
    &'static str,
    Vec<(&'static str, String)>,
    String,
    &'static str,
);

#[tokio::test]
async fn a_refused_object_acl_changes_nothing() {
    let router = router();
    let bucket = "object-acl-refusals";
    let key = "refused.txt";
    bucket_holding(&router, bucket, key).await;
    let owner = canonical_user_id();

    // The ACL the object holds throughout: every refusal below must leave it exactly as it
    // is.
    let stored = vec![group_grant(LOG_DELIVERY_GROUP_URI, PERMISSION_WRITE)];
    let setup = put_object_acl(
        &router,
        bucket,
        key,
        "",
        &[("x-amz-grant-write", &format!("uri={LOG_DELIVERY_GROUP_URI}"))],
        "",
    )
    .await;
    assert_eq!(
        setup.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        setup.status,
        String::from_utf8_lossy(&setup.body)
    );

    let one_grant_body =
        access_control_policy(None, &[GrantXml::group(ALL_USERS_GROUP_URI, PERMISSION_READ)]);
    let too_many: Vec<GrantXml<'_>> = std::iter::repeat_n(
        GrantXml::group(ALL_USERS_GROUP_URI, PERMISSION_READ),
        MAX_ACL_GRANTS + 1,
    )
    .collect();
    let foreign_owner = "0".repeat(64);

    // One row per way a request can fall outside what the API accepts.
    let rows: Vec<RefusalRow> = vec![
        (
            "a canned ACL and a grant header together",
            vec![
                ("x-amz-acl", "private".to_string()),
                ("x-amz-grant-read", format!("uri={ALL_USERS_GROUP_URI}")),
            ],
            String::new(),
            "InvalidRequest",
        ),
        (
            "a body and a canned ACL together",
            vec![("x-amz-acl", "private".to_string())],
            one_grant_body.clone(),
            "InvalidRequest",
        ),
        (
            "a body and a grant header together",
            vec![("x-amz-grant-read", format!("uri={ALL_USERS_GROUP_URI}"))],
            one_grant_body.clone(),
            "InvalidRequest",
        ),
        (
            "no canned ACL, no grant header and no body",
            vec![],
            String::new(),
            "MissingRequestBodyError",
        ),
        (
            "a group URI that is not one of the predefined groups",
            vec![(
                "x-amz-grant-read",
                "uri=http://acs.amazonaws.com/groups/global/AllUser".to_string(),
            )],
            String::new(),
            "InvalidArgument",
        ),
        (
            "a grant header pair whose type is not id, uri or emailAddress",
            vec![("x-amz-grant-read", "account=123456789012".to_string())],
            String::new(),
            "InvalidArgument",
        ),
        (
            "a grant header pair that is not a pair at all",
            vec![("x-amz-grant-read", ALL_USERS_GROUP_URI.to_string())],
            String::new(),
            "InvalidArgument",
        ),
        (
            "a body Grant carrying no Permission",
            vec![],
            access_control_policy(
                None,
                &[GrantXml {
                    grantee_type: GRANTEE_GROUP,
                    named_by: Some(("URI", ALL_USERS_GROUP_URI)),
                    permission: None,
                }],
            ),
            "MalformedACLError",
        ),
        (
            "a body Grantee of type CanonicalUser that names no ID",
            vec![],
            access_control_policy(
                None,
                &[GrantXml {
                    grantee_type: GRANTEE_CANONICAL_USER,
                    named_by: None,
                    permission: Some(PERMISSION_READ),
                }],
            ),
            "MalformedACLError",
        ),
        (
            // `Grant$Permission` targets the `Permission` enum, so a value outside the
            // published set is refused by the BOUNDARY — before this operation's handler
            // runs — with the code S3 answers a payload it cannot parse against the schema
            // it publishes.
            "a body grant naming a Permission outside the published set",
            vec![],
            access_control_policy(None, &[GrantXml::user(&owner, "READ_WRITE")]),
            "MalformedXML",
        ),
        (
            "more grants than one ACL may hold",
            vec![],
            access_control_policy(None, &too_many),
            "MalformedACLError",
        ),
        (
            "a body naming an owner that is not the account that holds the object",
            vec![],
            access_control_policy(
                Some(&foreign_owner),
                &[GrantXml::user(&owner, PERMISSION_FULL_CONTROL)],
            ),
            "InvalidArgument",
        ),
    ];

    for (case, headers, body, code) in rows {
        let borrowed: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let refused = put_object_acl(&router, bucket, key, "", &borrowed, &body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the request to be refused with HTTP 400, got {} with \
             body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: expected the documented error code, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            reported_grants(&get_object_acl(&router, bucket, key, "", &[]).await),
            stored,
            "case {case:?}: a refused put must leave the stored ACL untouched, got {:?}",
            reported_grants(&get_object_acl(&router, bucket, key, "", &[]).await)
        );
    }
}

#[tokio::test]
async fn an_acl_call_naming_an_address_this_emulator_does_not_hold_answers_its_own_404() {
    let router = router();
    let bucket = "object-acl-addresses";
    let key = "held.txt";
    bucket_holding(&router, bucket, key).await;

    // (case, the bucket, the key, the query, the status and code the answer must carry).
    // One row per link of the chain bucket -> key -> version that can be missing, with the
    // near-misses a lookup has to tell apart: a key that is a prefix of the held one, one
    // that extends it, and one differing only in case.
    let rows: Vec<(&str, &str, &str, String, u16, &str)> = vec![
        (
            "a bucket this emulator does not hold",
            "object-acl-absent",
            key,
            String::new(),
            404,
            "NoSuchBucket",
        ),
        (
            "a key the bucket does not hold",
            bucket,
            "absent.txt",
            String::new(),
            404,
            "NoSuchKey",
        ),
        (
            "a key that is a prefix of the held one",
            bucket,
            "held",
            String::new(),
            404,
            "NoSuchKey",
        ),
        (
            "a key that extends the held one",
            bucket,
            "held.txt.bak",
            String::new(),
            404,
            "NoSuchKey",
        ),
        (
            "a key differing from the held one only in case",
            bucket,
            "Held.txt",
            String::new(),
            404,
            "NoSuchKey",
        ),
        (
            "a versionId this store never minted",
            bucket,
            key,
            "&versionId=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            404,
            "NoSuchVersion",
        ),
        (
            "an expected bucket owner that is not the bucket's owner",
            bucket,
            key,
            String::new(),
            403,
            "AccessDenied",
        ),
    ];

    for (case, named_bucket, named_key, query, status, code) in rows {
        let headers: Vec<(&str, &str)> = if code == "AccessDenied" {
            vec![("x-amz-expected-bucket-owner", "123456789012")]
        } else {
            Vec::new()
        };
        let read = get_object_acl(&router, named_bucket, named_key, &query, &headers).await;
        assert_eq!(
            read.status, status,
            "case {case:?}/GetObjectAcl: expected HTTP {status}, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.error_code(),
            code,
            "case {case:?}/GetObjectAcl: expected {code}, got {:?}",
            read.error_code()
        );

        let mut write_headers = vec![("x-amz-acl", "private")];
        write_headers.extend_from_slice(&headers);
        let written =
            put_object_acl(&router, named_bucket, named_key, &query, &write_headers, "").await;
        assert_eq!(
            written.status, status,
            "case {case:?}/PutObjectAcl: expected HTTP {status}, got {} with body {:?}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
        assert_eq!(
            written.error_code(),
            code,
            "case {case:?}/PutObjectAcl: expected {code}, got {:?}",
            written.error_code()
        );
    }

    // The address that IS held answers normally, so the rows above failed on the lookup
    // rather than on the operations being unreachable.
    let read = get_object_acl(&router, bucket, key, "", &[]).await;
    assert_eq!(
        read.status, 200,
        "the held object must report its ACL, got {} with body {:?}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
}

#[tokio::test]
async fn ignoring_public_acls_keeps_the_public_grants_out_of_every_acl_read() {
    let router = router();

    // One row per PUBLIC predefined group, so the rule is pinned over the whole set of
    // grantees that make an ACL public rather than over the one group the recorded call
    // names: "an ACL is public when it grants access to AllUsers or AuthenticatedUsers"
    // (Amazon S3 User Guide, The Meaning of Public).
    for (index, group) in [ALL_USERS_GROUP_URI, AUTHENTICATED_USERS_GROUP_URI]
        .into_iter()
        .enumerate()
    {
        let bucket = format!("object-acl-ignore-public-{index}");
        let key = "shared.txt";
        bucket_holding(&router, &bucket, key).await;

        // Both ACLs carry a public grant AND a grant that is not public, so what the reads
        // drop is the public grant alone rather than the whole ACL.
        let seeded = access_control_policy(
            None,
            &[
                GrantXml::user(&canonical_user_id(), PERMISSION_FULL_CONTROL),
                GrantXml::group(group, PERMISSION_READ),
            ],
        );
        let on_object = put_object_acl(&router, &bucket, key, "", &[], &seeded).await;
        assert_eq!(
            on_object.status, 200,
            "case {group:?}: the object's ACL must be set first, got {} with body {:?}",
            on_object.status,
            String::from_utf8_lossy(&on_object.body)
        );
        let on_bucket = put_bucket_acl(
            &router,
            &bucket,
            &[
                ("x-amz-grant-full-control", &format!("id={}", canonical_user_id())),
                ("x-amz-grant-read", &format!("uri={group}")),
            ],
        )
        .await;
        assert_eq!(
            on_bucket.status, 200,
            "case {group:?}: the bucket's ACL must be set first, got {} with body {:?}",
            on_bucket.status,
            String::from_utf8_lossy(&on_bucket.body)
        );

        let with_public = vec![
            user_grant(&canonical_user_id(), PERMISSION_FULL_CONTROL),
            group_grant(group, PERMISSION_READ),
        ];
        let without_public = vec![user_grant(&canonical_user_id(), PERMISSION_FULL_CONTROL)];

        // (case, what IgnorePublicAcls is set to, what both reads must then report). The
        // switch is turned on, off and on again: the stored grants are untouched, so the
        // public one comes back every time it is off.
        for (case, ignore, expected) in [
            ("the switch has never been set", None, &with_public),
            ("the switch is off", Some(false), &with_public),
            ("the switch is on", Some(true), &without_public),
            ("the switch is off again", Some(false), &with_public),
            ("the switch is on again", Some(true), &without_public),
        ] {
            if let Some(ignore) = ignore {
                let stored = put_public_access_block(
                    &router,
                    &bucket,
                    &[("IgnorePublicAcls", ignore)],
                    &[],
                )
                .await;
                assert_eq!(
                    stored.status, 200,
                    "case {case:?}: IgnorePublicAcls={ignore} must be stored, got {} with \
                     body {:?}",
                    stored.status,
                    String::from_utf8_lossy(&stored.body)
                );
            }

            for (reader, answer) in [
                (
                    "GetObjectAcl",
                    get_object_acl(&router, &bucket, key, "", &[]).await,
                ),
                ("GetBucketAcl", get_bucket_acl(&router, &bucket).await),
            ] {
                assert_eq!(
                    answer.status, 200,
                    "case {group:?}/{case:?}/{reader}: the read must answer 200, got {} with \
                     body {:?}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                assert_eq!(
                    &reported_grants(&answer),
                    expected,
                    "case {group:?}/{case:?}/{reader}: expected {expected:?}, got {:?}",
                    reported_grants(&answer)
                );
            }
        }
    }
}

/// One row of the write-time ACL table: the case, the `PutObject` headers, and the grants
/// the stored object must report afterwards.
type WriteAclRow = (&'static str, Vec<(&'static str, String)>, Vec<ReportedGrant>);

#[tokio::test]
async fn a_write_that_names_an_acl_stores_it_on_the_object_it_writes() {
    let router = router();
    let owner = canonical_user_id();

    // The rows differ along what the rule generalises over: WHICH way the write names its
    // ACL (a canned ACL or the explicit grant headers), WHICH canned ACL, WHICH permission
    // each of the four grant headers a write carries stands for, and how many grantees one
    // header names.
    let rows: Vec<WriteAclRow> = vec![
        (
            "no ACL at all: the object carries the owner's FULL_CONTROL",
            vec![],
            default_acl(),
        ),
        (
            "the canned ACL public-read",
            vec![("x-amz-acl", "public-read".to_string())],
            vec![
                user_grant(&owner, PERMISSION_FULL_CONTROL),
                group_grant(ALL_USERS_GROUP_URI, PERMISSION_READ),
            ],
        ),
        (
            "the canned ACL private",
            vec![("x-amz-acl", "private".to_string())],
            default_acl(),
        ),
        (
            "the canned ACL bucket-owner-read",
            vec![("x-amz-acl", "bucket-owner-read".to_string())],
            vec![
                user_grant(&owner, PERMISSION_FULL_CONTROL),
                user_grant(&owner, PERMISSION_READ),
            ],
        ),
        (
            "a grant header REPLACING the owner's grant with one of its own",
            vec![(
                "x-amz-grant-read",
                format!("uri={LOG_DELIVERY_GROUP_URI}"),
            )],
            vec![group_grant(LOG_DELIVERY_GROUP_URI, PERMISSION_READ)],
        ),
        (
            "the full-control grant header naming the owner",
            vec![("x-amz-grant-full-control", format!("id={owner}"))],
            vec![user_grant(&owner, PERMISSION_FULL_CONTROL)],
        ),
        (
            "the read-acp grant header naming a group",
            vec![("x-amz-grant-read-acp", format!("uri={ALL_USERS_GROUP_URI}"))],
            vec![group_grant(ALL_USERS_GROUP_URI, PERMISSION_READ_ACP)],
        ),
        (
            "the write-acp grant header naming two grantees",
            vec![(
                "x-amz-grant-write-acp",
                format!("id={owner}, uri={LOG_DELIVERY_GROUP_URI}"),
            )],
            vec![
                user_grant(&owner, PERMISSION_WRITE_ACP),
                group_grant(LOG_DELIVERY_GROUP_URI, PERMISSION_WRITE_ACP),
            ],
        ),
    ];

    for (index, (case, headers, expected)) in rows.into_iter().enumerate() {
        let bucket = format!("object-acl-on-write-{index}");
        let key = "written.txt";
        bucket_holding(&router, &bucket, key).await;

        let borrowed: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let written = put_object(&router, &bucket, key, BODY, &borrowed).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the write must be accepted, got {} with body {:?}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
        assert_eq!(
            reported_grants(&get_object_acl(&router, &bucket, key, "", &[]).await),
            expected,
            "case {case:?}: expected the ACL the write named, got {:?}",
            reported_grants(&get_object_acl(&router, &bucket, key, "", &[]).await)
        );

        // A write REPLACES the whole object, so the ACL of the object it replaces goes with
        // it: the second write names none, and the key carries the default afterwards.
        let replaced = put_object(&router, &bucket, key, BODY, &[]).await;
        assert_eq!(
            replaced.status, 200,
            "case {case:?}: the replacing write must be accepted, got {}",
            replaced.status
        );
        assert_eq!(
            reported_grants(&get_object_acl(&router, &bucket, key, "", &[]).await),
            default_acl(),
            "case {case:?}: a write naming no ACL must leave the object with the default, \
             got {:?}",
            reported_grants(&get_object_acl(&router, &bucket, key, "", &[]).await)
        );
    }
}

#[tokio::test]
async fn a_write_naming_an_acl_is_refused_the_way_an_acl_write_is() {
    let router = router();
    let owner = canonical_user_id();

    // A canned ACL and a grant header together: "If you use this header, you cannot use
    // other access control-specific headers in your request" (`PutObjectRequest$ACL`).
    let bucket = "object-acl-write-refusals";
    let key = "refused.txt";
    bucket_holding(&router, bucket, key).await;
    let both = put_object(
        &router,
        bucket,
        key,
        BODY,
        &[
            ("x-amz-acl", "private"),
            ("x-amz-grant-read", &format!("uri={ALL_USERS_GROUP_URI}")),
        ],
    )
    .await;
    assert_eq!(
        both.error_code(),
        "InvalidRequest",
        "a write naming its ACL twice must be refused, got {:?} with status {}",
        both.error_code(),
        both.status
    );
    let bad_pair = put_object(
        &router,
        bucket,
        key,
        BODY,
        &[("x-amz-grant-read", "account=123456789012")],
    )
    .await;
    assert_eq!(
        bad_pair.error_code(),
        "InvalidArgument",
        "a grant header pair whose type is not id, uri or emailAddress must be refused, got \
         {:?} with status {}",
        bad_pair.error_code(),
        bad_pair.status
    );

    // "Buckets that use this setting only accept PUT requests that don't specify an ACL or
    // PUT requests that specify bucket owner full control ACLs … PUT requests that contain
    // other ACLs … fail and return a 400 error with the error code
    // AccessControlListNotSupported" (`PutObjectRequest$ACL`).
    let enforced = "object-acl-write-enforced";
    bucket_holding(&router, enforced, key).await;
    let disabled = put_ownership_controls(&router, enforced, &["BucketOwnerEnforced"]).await;
    assert_eq!(
        disabled.status, 200,
        "the bucket must take the setting that disables ACLs, got {}",
        disabled.status
    );

    // (case, the headers, whether the write must be accepted).
    for (case, headers, accepted) in [
        ("a write naming no ACL at all", vec![], true),
        (
            "a write naming the bucket-owner-full-control canned ACL",
            vec![("x-amz-acl", "bucket-owner-full-control".to_string())],
            true,
        ),
        (
            "a write naming the equivalent grant: the owner's FULL_CONTROL",
            vec![("x-amz-grant-full-control", format!("id={owner}"))],
            true,
        ),
        (
            "a write naming the private canned ACL",
            vec![("x-amz-acl", "private".to_string())],
            true,
        ),
        (
            "a write naming the public-read canned ACL",
            vec![("x-amz-acl", "public-read".to_string())],
            false,
        ),
        (
            "a write naming a custom grant",
            vec![(
                "x-amz-grant-read",
                format!("uri={LOG_DELIVERY_GROUP_URI}"),
            )],
            false,
        ),
    ] {
        let borrowed: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let written = put_object(&router, enforced, key, BODY, &borrowed).await;
        if accepted {
            assert_eq!(
                written.status, 200,
                "case {case:?}: a write the bucket owner enforced setting admits must be \
                 accepted, got {} with body {:?}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
        } else {
            assert_eq!(
                written.status, 400,
                "case {case:?}: a write naming another ACL must be refused with HTTP 400, \
                 got {} with body {:?}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
            assert_eq!(
                written.error_code(),
                "AccessControlListNotSupported",
                "case {case:?}: expected the documented error code, got {:?}",
                written.error_code()
            );
            assert_eq!(
                written.error_message(),
                "The bucket does not allow ACLs",
                "case {case:?}: expected the documented message, got {:?}",
                written.error_message()
            );
        }
    }
}
