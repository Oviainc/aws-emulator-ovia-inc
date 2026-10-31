// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OBJECT VERSION an object sub-resource call reports.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns), so what is pinned is the wire behaviour — the
//! response header — rather than any handler's internals. Every request helper comes from
//! the suite that owns the sub-resource it drives, so no call is spelled a second way here.
//!
//! The rule under test: a sub-resource call on an object — its tag set, its ACL, its Object
//! Lock legal hold or retention — acts on ONE object version, and reports the id that
//! version carries in the `x-amz-version-id` response header; a version with no id of its
//! own (the current object of a bucket whose versioning was never enabled, and the null
//! version written while versioning is suspended) is reported as no version at all, so the
//! header is absent. It holds for a read and for a write, for the 204 of a removal as much
//! as for a 200, and for the version a request NAMED as much as for the current one.
//!
//! The sources:
//!
//! * The three tagging operations model the member outright: `PutObjectTaggingOutput`,
//!   `GetObjectTaggingOutput` and `DeleteObjectTaggingOutput` each bind `VersionId` to
//!   `@httpHeader x-amz-version-id` (`smithy_query(query="operation",
//!   operation="PutObjectTagging,GetObjectTagging,DeleteObjectTagging",
//!   model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`),
//!   documented as "the versionId of the object the tag-set was added to" / "removed from" /
//!   "for which you got the tagging information".
//! * The ACL and Object Lock sub-resources model no such member — `PutObjectAclOutput`,
//!   `GetObjectAclOutput`, `PutObjectLegalHoldOutput` and `PutObjectRetentionOutput` carry
//!   `RequestCharged` alone — while the API reports the version it acted on all the same, so
//!   those four bind the header outside their output shape
//!   ([`service_com_amazonaws_s3_amazons3::service::wire::ActedOnVersion`]).
//! * Which version a call acts on is the address it resolves: "to put tags of any other
//!   version, use the versionId query parameter", and "by default, GET returns ACL
//!   information about the current version of an object. To return ACL information about a
//!   different version, use the versionId subresource" (those operations' own
//!   documentation).
//!
//! Two of the seven are absent from the unversioned rows below, and the reason is the
//! service's own: a legal hold and a retention need an S3 Object Lock configuration, which
//! "is only supported on versioned buckets", so there is no such thing as an Object Lock
//! call on a bucket whose versioning was never enabled. They are driven by the
//! version-naming table instead, which is where every version has an id anyway.
//!
//! The tagging operations carry no `versionId` row: naming one is a scaffolded branch of
//! theirs (pinned by [`super::object_tagging`] and [`super::object_tag_set_removal`]), so
//! the version they act on is the key's current one.

use service_com_amazonaws_s3_amazons3::state::{GOVERNANCE_RETENTION_MODE, LEGAL_HOLD_ON_STATUS};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, Answer};
use super::bucket_ownership_controls::put_ownership_controls;
use super::bucket_tagging::tag;
use super::object_acl::{get_object_acl, put_object_acl};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_legal_hold::{legal_hold_payload, lock_enabled_bucket, put_object_legal_hold};
use super::object_retention::{now_seconds, put_object_retention, retention_payload, ONE_HOUR};
use super::object_tag_set_removal::delete_object_tagging;
use super::object_tagging::{get_object_tagging, put_object_tagging};
use super::object_versioning::set_versioning;

/// The header S3 reports the version a call acted on in.
const VERSION_ID_HEADER: &str = "x-amz-version-id";

/// The bytes every object in this suite is written with: which version a sub-resource call
/// acted on has nothing to do with what the version holds, so one payload serves every row.
const BODY: &[u8] = b"subresource-subject";

/// What one sub-resource call answered: the operation's name, the status it must carry, and
/// the response.
struct SubresourceAnswer {
    operation: &'static str,
    expected_status: u16,
    answer: Answer,
}

/// A bucket whose ACLs are ENABLED, so an ACL row is not refused for the bucket's Object
/// Ownership setting before anything about the version it acted on can be observed.
///
/// A bucket is created with the setting that DISABLES ACLs ("by default, `ObjectOwnership`
/// is set to `BucketOwnerEnforced` and ACLs are disabled"), which is what makes this
/// switch necessary; the ACL suite beside this one takes it for the same reason.
async fn acls_enabled(router: &MultiServiceRouter, name: &str) {
    let enabled = put_ownership_controls(router, name, &["ObjectWriter"]).await;
    assert_eq!(
        enabled.status, 200,
        "the bucket {name:?} must take the ObjectWriter ownership setting that enables ACLs, \
         got {} with body {:?}",
        enabled.status,
        text(&enabled)
    );
}

/// An object written at `key`, reporting the version id the write minted — `None` when the
/// bucket's versioning state made it mint none.
async fn written_version(router: &MultiServiceRouter, name: &str, key: &str) -> Option<String> {
    let written = put_object(router, name, key, BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the object {name}/{key} must be written first, got {} with body {:?}",
        written.status,
        text(&written)
    );
    written.header(VERSION_ID_HEADER)
}

/// The five sub-resource calls that need no Object Lock configuration, driven in turn
/// against the key's CURRENT version: the tag set written, read and removed, then the ACL
/// written and read.
///
/// One sequence for every bucket row, so each row exercises a write (200), a read (200) and
/// a removal (204) of a sub-resource through the same addressing.
async fn current_version_answers(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
) -> Vec<SubresourceAnswer> {
    vec![
        SubresourceAnswer {
            operation: "PutObjectTagging",
            expected_status: 200,
            answer: put_object_tagging(router, name, key, &[tag("owner", "subresource-suite")])
                .await,
        },
        SubresourceAnswer {
            operation: "GetObjectTagging",
            expected_status: 200,
            answer: get_object_tagging(router, name, key, "").await,
        },
        SubresourceAnswer {
            operation: "DeleteObjectTagging",
            expected_status: 204,
            answer: delete_object_tagging(router, name, key, "", &[]).await,
        },
        SubresourceAnswer {
            operation: "PutObjectAcl",
            expected_status: 200,
            answer: put_object_acl(router, name, key, "", &[("x-amz-acl", "private")], "").await,
        },
        SubresourceAnswer {
            operation: "GetObjectAcl",
            expected_status: 200,
            answer: get_object_acl(router, name, key, "", &[]).await,
        },
    ]
}

/// One row of the bucket table: the case name and the bucket it stands up, which answers
/// with the version id the key's current object carries — `None` when it carries none.
type BucketRow = (&'static str, &'static str);

/// The buckets the sub-resource calls are driven against, differing along the one thing the
/// rule generalises over: whether the version a call acts on HAS an id.
///
/// `Enabled` is the state that mints one ("if you enable versioning … Amazon S3 generates a
/// unique version ID"), a bucket that was never configured mints none, and `Suspended`
/// leaves the object it then stores as the null version — "if you suspend versioning …
/// Amazon S3 generates a null version ID" — which is no id a request can name back.
const BUCKET_ROWS: &[BucketRow] = &[
    ("versioning enabled", "subresource-version-enabled"),
    ("versioning never enabled", "subresource-version-plain"),
    ("versioning suspended", "subresource-version-suspended"),
];

/// Stand `row` up and answer with the version id its key's current object carries.
async fn stand_up(router: &MultiServiceRouter, case: &str, name: &str, key: &str) -> Option<String> {
    bucket(router, name).await;
    acls_enabled(router, name).await;
    match case {
        "versioning enabled" => {
            set_versioning(router, name, "Enabled").await;
            written_version(router, name, key).await
        }
        "versioning never enabled" => written_version(router, name, key).await,
        "versioning suspended" => {
            // A version WITH an id first, so the suspended write below displaces a real
            // version rather than being the key's only one: what the calls must report is
            // the current object's absence of an id, not the bucket's emptiness.
            set_versioning(router, name, "Enabled").await;
            let minted = written_version(router, name, key).await;
            assert!(
                minted.is_some(),
                "case {case:?}: the setup write under Enabled must mint a version id, got \
                 {minted:?}"
            );
            set_versioning(router, name, "Suspended").await;
            written_version(router, name, key).await
        }
        other => panic!("case {other:?}: the suite stands up no such bucket"),
    }
}

#[tokio::test]
async fn every_object_subresource_reports_the_version_it_acted_on() {
    let router = router();
    let key = "subresource/subject.txt";

    for (case, name) in BUCKET_ROWS {
        let expected = stand_up(&router, case, name, key).await;

        for SubresourceAnswer {
            operation,
            expected_status,
            answer,
        } in current_version_answers(&router, name, key).await
        {
            assert_eq!(
                answer.status, expected_status,
                "case {case:?}/{operation}: expected HTTP {expected_status}, got {} with body \
                 {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.header(VERSION_ID_HEADER),
                expected,
                "case {case:?}/{operation}: the version acted on is {expected:?}, got {:?}",
                answer.header(VERSION_ID_HEADER)
            );
        }

        // The reported id is one the bucket really holds: it addresses the same version
        // back. A row reporting none has nothing to address, and the key itself still
        // reads — which is what makes the absent header "this version has no id" rather
        // than "there is no version".
        let addressed = match &expected {
            Some(version_id) => get_object(&router, name, key, &format!("?versionId={version_id}"), &[]).await,
            None => get_object(&router, name, key, "", &[]).await,
        };
        assert_eq!(
            addressed.status, 200,
            "case {case:?}: the version the calls reported ({expected:?}) must read back, got \
             {} with body {:?}",
            addressed.status,
            text(&addressed)
        );
    }
}

/// One row of the version-naming table: the case name, the `versionId` the request carries
/// (none at all for the current version), and which of the two stored versions the answer
/// must name.
type NamedVersionRow = (&'static str, bool, usize);

/// The addresses a sub-resource call is driven at, differing along what the rule
/// generalises over: whether the request NAMES a version, and whether the version it names
/// is the current one.
///
/// `(case, names the version, the index of the stored version the answer must report)`.
const NAMED_VERSION_ROWS: &[NamedVersionRow] = &[
    ("the older version, named by id", true, 0),
    ("the current version, named by id", true, 1),
    ("no versionId at all, which is the current version", false, 1),
];

#[tokio::test]
async fn a_subresource_call_naming_a_version_reports_the_version_it_named() {
    let router = router();
    let name = "subresource-version-named";
    let key = "locked/subject.txt";
    // An Object Lock configuration is what a legal hold and a retention need, and a bucket
    // created with one has versioning enabled, so every version here has an id.
    lock_enabled_bucket(&router, name).await;
    acls_enabled(&router, name).await;

    let mut stored: Vec<String> = Vec::new();
    for round in 0..2 {
        let minted = written_version(&router, name, key).await;
        let minted = minted.unwrap_or_else(|| {
            panic!("case {round}: a write to an Object Lock bucket must mint a version id")
        });
        assert!(
            !stored.contains(&minted),
            "case {round}: each write must mint its own version id, got the already-stored \
             {minted:?}"
        );
        stored.push(minted);
    }

    for (case, names_the_version, index) in NAMED_VERSION_ROWS {
        let expected = &stored[*index];
        let query = if *names_the_version {
            format!("&versionId={expected}")
        } else {
            String::new()
        };
        // The four sub-resource calls that resolve a `versionId`: both halves of the ACL,
        // and both halves of Object Lock a caller can write. The retention runs further out
        // on every row, because a retention period may be lengthened and never shortened.
        let retention = retention_payload(
            Some(GOVERNANCE_RETENTION_MODE),
            Some(now_seconds() + (*index as i64 + 2) * 24 * ONE_HOUR),
        );
        let answers = vec![
            SubresourceAnswer {
                operation: "PutObjectAcl",
                expected_status: 200,
                answer: put_object_acl(
                    &router,
                    name,
                    key,
                    &query,
                    &[("x-amz-acl", "private")],
                    "",
                )
                .await,
            },
            SubresourceAnswer {
                operation: "GetObjectAcl",
                expected_status: 200,
                answer: get_object_acl(&router, name, key, &query, &[]).await,
            },
            SubresourceAnswer {
                operation: "PutObjectLegalHold",
                expected_status: 200,
                answer: put_object_legal_hold(
                    &router,
                    name,
                    key,
                    &query,
                    &[],
                    &legal_hold_payload(Some(LEGAL_HOLD_ON_STATUS)),
                )
                .await,
            },
            SubresourceAnswer {
                operation: "PutObjectRetention",
                expected_status: 200,
                answer: put_object_retention(&router, name, key, &query, &[], &retention).await,
            },
        ];

        for SubresourceAnswer {
            operation,
            expected_status,
            answer,
        } in answers
        {
            assert_eq!(
                answer.status, expected_status,
                "case {case:?}/{operation}: expected HTTP {expected_status}, got {} with body \
                 {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.header(VERSION_ID_HEADER).as_deref(),
                Some(expected.as_str()),
                "case {case:?}/{operation}: the version acted on is {expected:?}, got {:?}",
                answer.header(VERSION_ID_HEADER)
            );
        }
    }
}
