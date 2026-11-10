// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what AUTHORIZES an object WRITE: the bucket's own policy — including
//! the conditions its statements are written over — and the account the request says owns the
//! bucket.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle and object suites in this crate own), so what is pinned is the wire
//! answer: the status, the error document's `<Code>` and `<Message>`, and whether the write
//! left anything behind.
//!
//! The rules under test:
//!
//! * A bucket policy is a RESOURCE-BASED policy, so it is in force on the object WRITE path
//!   too, not only on reads: "an explicit deny in any policy overrides any allow" (IAM's own
//!   evaluation order), so a `Deny` about the write refuses it with HTTP 403 `AccessDenied`
//!   and NOTHING is stored. The refusal carries the sentence IAM publishes for an explicit
//!   deny in a resource-based policy, naming the caller, `s3:PutObject` and the object's ARN.
//! * A statement's `Condition` is judged against the condition keys THIS request carries —
//!   the write's own `x-amz-acl`, `x-amz-storage-class`, `x-amz-server-side-encryption` and
//!   `x-amz-server-side-encryption-aws-kms-key-id` headers (Amazon S3 User Guide, *Actions,
//!   resources, and condition keys for Amazon S3*) — which is what makes a policy that
//!   requires encryption mandatory rather than advisory: a write naming no algorithm is
//!   refused by it and a write naming the required one is served. A key the request does not
//!   carry is DECIDED (IAM's reading of a negated operator, an `IfExists` spelling and a
//!   `Null` clause), while a clause over a key this service does not answer leaves the
//!   statement undecided and refuses nothing.
//! * `CopyObject` authorizes BOTH halves of the copy separately: the write of the destination
//!   (`s3:PutObject` under the destination bucket's policy) and the READ of its source —
//!   "you must have READ access to the source object" (`CopyObject`'s own documentation),
//!   which is `s3:GetObject` under the SOURCE bucket's policy. A caller the source policy
//!   denies is refused `AccessDenied` and no destination object is created.
//! * `x-amz-expected-bucket-owner` is the caller's own confused-deputy guard, modelled on
//!   effectively every operation of this service: "if the account ID that you provide does
//!   not match the actual owner of the bucket, the request fails with the HTTP status code
//!   403 Forbidden (access denied)". The rows span bucket and object operations, reads and
//!   writes alike.

use service_com_amazonaws_s3_amazons3::state::caller_principal_arn;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{put_bucket_policy, router, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};

/// The bytes every write in this suite carries. A policy is about who may write an object,
/// not about what is in it, so one payload serves every row.
const BODY: &[u8] = b"policy-governed-payload";

/// The key every write in this suite addresses.
const KEY: &str = "reports/written.csv";

/// The code S3 answers an authorization refusal with.
const ACCESS_DENIED: &str = "AccessDenied";

/// The published sentence that code carries when it is not an explicit deny being reported
/// (Amazon S3 "Error responses", list of error codes: "AccessDenied — Access Denied").
const ACCESS_DENIED_MESSAGE: &str = "Access Denied";

/// The code and message a read of a key nothing was written at answers with.
const NO_SUCH_KEY: &str = "NoSuchKey";

/// An account id that is NOT the one account this emulator serves.
const NOT_THE_OWNER: &str = "111122223333";

/// A policy document carrying `statements`, in the form a client sends one.
///
/// `pub(crate)`, like the two below it: the suite that pins what a policy written over the
/// CUSTOMER-KEY condition decides ([`super::object_customer_key_encryption`]) builds its
/// documents through these rather than spelling a second JSON shape.
pub(crate) fn policy_document(statements: &str) -> String {
    format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{statements}]}}")
}

/// One statement reaching every principal: the effect, the action, the resource ARN, and the
/// whole `Condition` clause when the row carries one.
pub(crate) fn statement(
    effect: &str,
    action: &str,
    resource: &str,
    condition: Option<&str>,
) -> String {
    let condition = condition
        .map(|clause| format!(",\"Condition\":{clause}"))
        .unwrap_or_default();
    format!(
        "{{\"Effect\":\"{effect}\",\"Principal\":\"*\",\"Action\":\"{action}\",\
         \"Resource\":\"{resource}\"{condition}}}"
    )
}

/// The sentence S3 refuses a request an explicit `Deny` in the resource's own policy barred
/// with: the principal, the action and the resource ARN, and the reason clause IAM publishes
/// for that decision. Written out here rather than built from the service's own helper, so
/// the wording has something to be wrong about.
pub(crate) fn explicit_deny_message(action: &str, resource: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {action} on resource: \"{resource}\" with an \
         explicit deny in a resource-based policy",
        caller_principal_arn()
    )
}

/// One row of the write-authorization table: the bucket it configures (its own, so one row's
/// policy cannot authorize another's write), the policy that bucket carries, the headers the
/// write sends, and whether the write must be refused.
struct WriteRow {
    case: &'static str,
    bucket: &'static str,
    policy: Option<String>,
    headers: Vec<(&'static str, &'static str)>,
    refused: bool,
}

/// The clause a policy that makes SSE-KMS mandatory is written as: everything but a write
/// naming that algorithm is denied.
fn unless_algorithm(algorithm: &str) -> String {
    format!("{{\"StringNotEquals\":{{\"s3:x-amz-server-side-encryption\":\"{algorithm}\"}}}}")
}

/// The rows: they differ along WHICH header the condition is written over (the encryption
/// algorithm, the KMS key, the canned ACL, the storage class), which FORM the clause takes (a
/// negated comparison, an existence test, a positive comparison, none at all), whether the
/// write CARRIES the header the clause names, and whether the key the condition names is one
/// this service answers at all.
fn write_rows() -> Vec<WriteRow> {
    vec![
        WriteRow {
            case: "no policy at all: the bucket's owner writes its own object",
            bucket: "write-authz-no-policy",
            policy: None,
            headers: Vec::new(),
            refused: false,
        },
        WriteRow {
            case: "a Deny of the write on every key, with no condition at all",
            bucket: "write-authz-deny-outright",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                "arn:aws:s3:::write-authz-deny-outright/*",
                None,
            ))),
            headers: Vec::new(),
            refused: true,
        },
        WriteRow {
            case: "a Deny of every S3 action on every key",
            bucket: "write-authz-deny-every-action",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:*",
                "arn:aws:s3:::write-authz-deny-every-action/*",
                None,
            ))),
            headers: Vec::new(),
            refused: true,
        },
        WriteRow {
            case: "a Deny of the write on ANOTHER prefix leaves this key writable",
            bucket: "write-authz-deny-other-prefix",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                "arn:aws:s3:::write-authz-deny-other-prefix/private/*",
                None,
            ))),
            headers: Vec::new(),
            refused: false,
        },
        WriteRow {
            case: "encryption required, and the write names none",
            bucket: "write-authz-kms-required-bare",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                "arn:aws:s3:::write-authz-kms-required-bare/*",
                Some(&unless_algorithm("aws:kms")),
            ))),
            headers: Vec::new(),
            refused: true,
        },
        WriteRow {
            case: "encryption required, and the write names the required algorithm",
            bucket: "write-authz-kms-required-met",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                "arn:aws:s3:::write-authz-kms-required-met/*",
                Some(&unless_algorithm("aws:kms")),
            ))),
            headers: vec![("x-amz-server-side-encryption", "aws:kms")],
            refused: false,
        },
        WriteRow {
            case: "encryption required, and the write names ANOTHER algorithm",
            bucket: "write-authz-kms-required-other",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                "arn:aws:s3:::write-authz-kms-required-other/*",
                Some(&unless_algorithm("aws:kms")),
            ))),
            headers: vec![("x-amz-server-side-encryption", "AES256")],
            refused: true,
        },
        WriteRow {
            case: "SSE-S3 required, and the write names it",
            bucket: "write-authz-aes-required-met",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                "arn:aws:s3:::write-authz-aes-required-met/*",
                Some(&unless_algorithm("AES256")),
            ))),
            headers: vec![("x-amz-server-side-encryption", "AES256")],
            refused: false,
        },
        WriteRow {
            case: "the encryption header's PRESENCE required, and the write omits it",
            bucket: "write-authz-presence-required",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                "arn:aws:s3:::write-authz-presence-required/*",
                Some("{\"Null\":{\"s3:x-amz-server-side-encryption\":\"true\"}}"),
            ))),
            headers: Vec::new(),
            refused: true,
        },
        WriteRow {
            case: "the encryption header's PRESENCE required, and the write carries it",
            bucket: "write-authz-presence-met",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                "arn:aws:s3:::write-authz-presence-met/*",
                Some("{\"Null\":{\"s3:x-amz-server-side-encryption\":\"true\"}}"),
            ))),
            headers: vec![("x-amz-server-side-encryption", "AES256")],
            refused: false,
        },
        WriteRow {
            case: "a KMS key the policy denies",
            bucket: "write-authz-deny-kms-key",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                "arn:aws:s3:::write-authz-deny-kms-key/*",
                Some(
                    "{\"StringEquals\":{\"s3:x-amz-server-side-encryption-aws-kms-key-id\":\
                     \"alias/forbidden\"}}",
                ),
            ))),
            headers: vec![
                ("x-amz-server-side-encryption", "aws:kms"),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id",
                    "alias/forbidden",
                ),
            ],
            refused: true,
        },
        WriteRow {
            case: "a KMS key the policy denies, with another key named",
            bucket: "write-authz-allow-kms-key",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                "arn:aws:s3:::write-authz-allow-kms-key/*",
                Some(
                    "{\"StringEquals\":{\"s3:x-amz-server-side-encryption-aws-kms-key-id\":\
                     \"alias/forbidden\"}}",
                ),
            ))),
            headers: vec![
                ("x-amz-server-side-encryption", "aws:kms"),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id",
                    "alias/permitted",
                ),
            ],
            refused: false,
        },
        WriteRow {
            case: "a canned ACL the policy denies",
            bucket: "write-authz-deny-acl",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                "arn:aws:s3:::write-authz-deny-acl/*",
                Some("{\"StringEquals\":{\"s3:x-amz-acl\":\"public-read\"}}"),
            ))),
            headers: vec![("x-amz-acl", "public-read")],
            refused: true,
        },
        WriteRow {
            case: "a canned ACL the policy denies, with a private write",
            bucket: "write-authz-deny-acl-private",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                "arn:aws:s3:::write-authz-deny-acl-private/*",
                Some("{\"StringEquals\":{\"s3:x-amz-acl\":\"public-read\"}}"),
            ))),
            headers: vec![("x-amz-acl", "private")],
            refused: false,
        },
        WriteRow {
            case: "a storage class the policy denies",
            bucket: "write-authz-deny-class",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                "arn:aws:s3:::write-authz-deny-class/*",
                Some("{\"StringEquals\":{\"s3:x-amz-storage-class\":\"GLACIER\"}}"),
            ))),
            headers: vec![("x-amz-storage-class", "GLACIER")],
            refused: true,
        },
        WriteRow {
            case: "a storage class the policy denies, with another class named",
            bucket: "write-authz-deny-class-other",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                "arn:aws:s3:::write-authz-deny-class-other/*",
                Some("{\"StringEquals\":{\"s3:x-amz-storage-class\":\"GLACIER\"}}"),
            ))),
            headers: vec![("x-amz-storage-class", "STANDARD_IA")],
            refused: false,
        },
        WriteRow {
            case: "a Deny of everything not sent over a secure transport",
            bucket: "write-authz-insecure-transport",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                "arn:aws:s3:::write-authz-insecure-transport/*",
                Some("{\"Bool\":{\"aws:SecureTransport\":\"false\"}}"),
            ))),
            headers: Vec::new(),
            refused: false,
        },
        WriteRow {
            case: "a Deny of everything sent over a secure transport",
            bucket: "write-authz-secure-transport",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                "arn:aws:s3:::write-authz-secure-transport/*",
                Some("{\"Bool\":{\"aws:SecureTransport\":\"true\"}}"),
            ))),
            headers: Vec::new(),
            refused: true,
        },
        WriteRow {
            case: "a Deny conditioned on a key this service does not answer",
            bucket: "write-authz-undecidable",
            policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                "arn:aws:s3:::write-authz-undecidable/*",
                Some("{\"StringEquals\":{\"s3:x-amz-grant-read\":\"id=someone\"}}"),
            ))),
            headers: Vec::new(),
            refused: false,
        },
    ]
}

/// Create `row`'s bucket and attach its policy, with the public-access guardrail left as a
/// fresh bucket carries it (no row's policy grants the public anything).
async fn configured_bucket(router: &MultiServiceRouter, row: &WriteRow) {
    bucket(router, row.bucket).await;
    if let Some(policy) = row.policy.as_deref() {
        let attached = put_bucket_policy(router, row.bucket, policy).await;
        assert_eq!(
            attached.status, 204,
            "case {:?}: the policy must be attached, got {} with body {:?}",
            row.case,
            attached.status,
            text(&attached)
        );
    }
}

#[tokio::test]
async fn a_bucket_policy_decides_whether_a_write_is_stored() {
    // The two rows whose policy is written over a KMS key id name keys KMS really holds: a
    // write naming a key resolves that reference against KMS before storing anything
    // ([`super::kms_key_references`]), so what these rows turn on is the policy alone.
    let router = super::kms_key_references::router_with_kms();
    let (key_id, _) =
        super::kms_key_references::created_key(&router, "the key these policies name").await;
    for alias in ["alias/forbidden", "alias/permitted"] {
        super::kms_key_references::created_alias(&router, alias, &key_id).await;
    }

    for row in write_rows() {
        configured_bucket(&router, &row).await;

        let written = put_object(&router, row.bucket, KEY, BODY, &row.headers).await;
        let expected_status = if row.refused { 403 } else { 200 };
        assert_eq!(
            written.status, expected_status,
            "case {:?}: expected HTTP {expected_status}, got {} with body {:?}",
            row.case,
            written.status,
            text(&written)
        );

        if row.refused {
            assert_eq!(
                written.error_code(),
                ACCESS_DENIED,
                "case {:?}: expected the {ACCESS_DENIED:?} code, got {:?} with body {:?}",
                row.case,
                written.error_code(),
                text(&written)
            );
            assert_eq!(
                written.error_message(),
                explicit_deny_message(
                    "s3:PutObject",
                    &format!("arn:aws:s3:::{}/{KEY}", row.bucket)
                ),
                "case {:?}: expected the explicit-deny sentence, got {:?}",
                row.case,
                written.error_message()
            );
            // A refused write stored NOTHING: the key is still empty, which is what makes
            // the refusal a refusal rather than a wording.
            let read = get_object(&router, row.bucket, KEY, "", &[]).await;
            assert_eq!(
                read.error_code(),
                NO_SUCH_KEY,
                "case {:?}: a refused write must leave the key empty, got {} with body {:?}",
                row.case,
                read.status,
                text(&read)
            );
        } else {
            // A write the policy leaves alone is stored, and reads back with its bytes.
            let read = get_object(&router, row.bucket, KEY, "", &[]).await;
            assert_eq!(
                read.body.as_ref(),
                BODY,
                "case {:?}: a write the policy leaves alone must be readable, got {} with \
                 body {:?}",
                row.case,
                read.status,
                text(&read)
            );
        }
    }
}

/// `PUT /<bucket>/<key>` with `x-amz-copy-source` — `CopyObject`.
async fn copy_object(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    copy_source: &str,
) -> Answer {
    send_bytes(
        router,
        "PUT",
        &format!("/{bucket_name}/{key}"),
        &[("x-amz-copy-source", copy_source)],
        b"",
    )
    .await
}

/// One row of the copy-authorization table: the policy each bucket carries, the source key
/// the copy names, and — when the copy is refused — the action and resource its refusal
/// states.
struct CopyRow {
    case: &'static str,
    /// The SOURCE bucket's policy, `None` for a bucket carrying none.
    source_policy: Option<String>,
    /// The DESTINATION bucket's policy, `None` for a bucket carrying none.
    destination_policy: Option<String>,
    source_key: &'static str,
    /// The action the refusal names, empty for a copy that is served.
    denied_action: &'static str,
    /// The resource the refusal names, empty for a copy that is served.
    denied_resource: String,
}

impl CopyRow {
    /// Whether this row's copy must be refused: a row that names a denied action is one.
    fn refused(&self) -> bool {
        !self.denied_action.is_empty()
    }
}

#[tokio::test]
async fn a_copy_authorizes_its_source_read_and_its_destination_write_separately() {
    let router = router();
    let source_name = "copy-authz-source";
    let destination = "copy-authz-destination";
    bucket(&router, source_name).await;
    bucket(&router, destination).await;
    for key in [KEY, "public/also-written.csv"] {
        let written = put_object(&router, source_name, key, BODY, &[]).await;
        assert_eq!(
            written.status, 200,
            "the source object {key:?} must be written first, got {} with body {:?}",
            written.status,
            text(&written)
        );
    }

    // The rows differ along WHICH half of the copy the policy denies, and whether the denied
    // resource is the one the copy actually names.
    let rows: Vec<CopyRow> = vec![
        CopyRow {
            case: "neither policy says anything",
            source_policy: None,
            destination_policy: None,
            source_key: KEY,
            denied_action: "",
            denied_resource: String::new(),
        },
        CopyRow {
            case: "the SOURCE bucket denies the read of that object",
            source_policy: Some(policy_document(&statement(
                "Deny",
                "s3:GetObject",
                &format!("arn:aws:s3:::{source_name}/*"),
                None,
            ))),
            destination_policy: None,
            source_key: KEY,
            denied_action: "s3:GetObject",
            denied_resource: format!("arn:aws:s3:::{source_name}/{KEY}"),
        },
        CopyRow {
            case: "the SOURCE bucket denies the read of ANOTHER prefix",
            source_policy: Some(policy_document(&statement(
                "Deny",
                "s3:GetObject",
                &format!("arn:aws:s3:::{source_name}/private/*"),
                None,
            ))),
            destination_policy: None,
            source_key: KEY,
            denied_action: "",
            denied_resource: String::new(),
        },
        CopyRow {
            case: "the SOURCE bucket denies every S3 action on the key copied",
            source_policy: Some(policy_document(&statement(
                "Deny",
                "s3:*",
                &format!("arn:aws:s3:::{source_name}/public/*"),
                None,
            ))),
            destination_policy: None,
            source_key: "public/also-written.csv",
            denied_action: "s3:GetObject",
            denied_resource: format!("arn:aws:s3:::{source_name}/public/also-written.csv"),
        },
        CopyRow {
            case: "the DESTINATION bucket denies the write",
            source_policy: None,
            destination_policy: Some(policy_document(&statement(
                "Deny",
                "s3:PutObject",
                &format!("arn:aws:s3:::{destination}/*"),
                None,
            ))),
            source_key: KEY,
            denied_action: "s3:PutObject",
            denied_resource: format!("arn:aws:s3:::{destination}/copied.csv"),
        },
    ];

    for row in rows {
        let case = row.case;
        let refused = row.refused();
        // Each row states BOTH policies, so a row that names none clears what the row before
        // it attached.
        for (name, document) in [
            (source_name, row.source_policy),
            (destination, row.destination_policy),
        ] {
            let answer = match document {
                Some(policy) => put_bucket_policy(&router, name, &policy).await,
                None => send(&router, "DELETE", &format!("/{name}?policy"), &[], "").await,
            };
            assert!(
                answer.status == 204 || answer.status == 200,
                "case {case:?}: the policy of {name:?} must be set, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
        }

        let copied = copy_object(
            &router,
            destination,
            "copied.csv",
            &format!("{source_name}/{}", row.source_key),
        )
        .await;
        let expected_status = if refused { 403 } else { 200 };
        assert_eq!(
            copied.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with body {:?}",
            copied.status,
            text(&copied)
        );

        if refused {
            assert_eq!(
                copied.error_code(),
                ACCESS_DENIED,
                "case {case:?}: expected the {ACCESS_DENIED:?} code, got {:?}",
                copied.error_code()
            );
            assert_eq!(
                copied.error_message(),
                explicit_deny_message(row.denied_action, &row.denied_resource),
                "case {case:?}: expected the explicit-deny sentence, got {:?}",
                copied.error_message()
            );
            // "No destination object is created": the refusal left the destination key as
            // it was.
            let read = get_object(&router, destination, "copied.csv", "", &[]).await;
            assert_eq!(
                read.error_code(),
                NO_SUCH_KEY,
                "case {case:?}: a refused copy must create no destination object, got {} \
                 with body {:?}",
                read.status,
                text(&read)
            );
        } else {
            let read = get_object(&router, destination, "copied.csv", "", &[]).await;
            assert_eq!(
                read.body.as_ref(),
                BODY,
                "case {case:?}: a copy neither policy denies must be served, got {} with \
                 body {:?}",
                read.status,
                text(&read)
            );
            // Cleared again, so the next row's refusal is about the next row's policy.
            let removed = send(
                &router,
                "DELETE",
                &format!("/{destination}/copied.csv"),
                &[],
                "",
            )
            .await;
            assert_eq!(
                removed.status, 204,
                "case {case:?}: the copy must be removed before the next row, got {}",
                removed.status
            );
        }
    }
}

/// One operation the owner guard is driven through: its name, the method, the URI under the
/// bucket, and the body it carries.
struct GuardedCall {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: &'static str,
    /// The status the call answers when the guard names the bucket's real owner.
    served: u16,
}

#[tokio::test]
async fn an_expected_bucket_owner_that_is_not_the_owner_refuses_the_call() {
    let router = router();
    let name = "owner-guard-bucket";
    bucket(&router, name).await;
    let written = put_object(&router, name, KEY, BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the object must be written first, got {} with body {:?}",
        written.status,
        text(&written)
    );
    let owner = engine::emulator_account_id();
    assert_ne!(
        owner, NOT_THE_OWNER,
        "the mismatching row must name an account that is NOT the bucket's owner"
    );

    // The rows span the operation KINDS the member is modelled on: a listing, an object
    // write, a bucket HEAD, a bucket subresource read and write, and an object delete.
    let calls = vec![
        GuardedCall {
            case: "ListObjectsV2",
            method: "GET",
            uri: format!("/{name}?list-type=2"),
            body: "",
            served: 200,
        },
        GuardedCall {
            case: "HeadBucket",
            method: "HEAD",
            uri: format!("/{name}"),
            body: "",
            served: 200,
        },
        GuardedCall {
            case: "GetBucketPolicy",
            method: "GET",
            uri: format!("/{name}?policy"),
            body: "",
            // The bucket carries no policy, so the owner's own read is the absent-subresource
            // 404 rather than a document — the guard is what this row is about.
            served: 404,
        },
        GuardedCall {
            case: "PutBucketTagging",
            method: "PUT",
            uri: format!("/{name}?tagging"),
            body: "<Tagging xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><TagSet>\
                   <Tag><Key>owner</Key><Value>guard</Value></Tag></TagSet></Tagging>",
            served: 204,
        },
        GuardedCall {
            case: "GetBucketTagging",
            method: "GET",
            uri: format!("/{name}?tagging"),
            body: "",
            served: 200,
        },
        GuardedCall {
            case: "PutObject",
            method: "PUT",
            uri: format!("/{name}/{KEY}"),
            body: "",
            served: 200,
        },
        GuardedCall {
            case: "DeleteObject",
            method: "DELETE",
            uri: format!("/{name}/{KEY}"),
            body: "",
            served: 204,
        },
    ];

    for call in calls {
        let refused = send(
            &router,
            call.method,
            &call.uri,
            &[("x-amz-expected-bucket-owner", NOT_THE_OWNER)],
            call.body,
        )
        .await;
        assert_eq!(
            refused.status, 403,
            "case {:?}: an owner that is not the owner must be refused 403, got {} with \
             body {:?}",
            call.case,
            refused.status,
            text(&refused)
        );
        // A HEAD carries no body, so only the status says anything about it.
        if call.method != "HEAD" {
            assert_eq!(
                refused.error_code(),
                ACCESS_DENIED,
                "case {:?}: expected the {ACCESS_DENIED:?} code, got {:?}",
                call.case,
                refused.error_code()
            );
            assert_eq!(
                refused.error_message(),
                ACCESS_DENIED_MESSAGE,
                "case {:?}: expected the published sentence, got {:?}",
                call.case,
                refused.error_message()
            );
        }

        // The owner's own account id in that header is accepted, so the 403 is about the
        // MISMATCH and not about the header being present.
        let served = send(
            &router,
            call.method,
            &call.uri,
            &[("x-amz-expected-bucket-owner", owner)],
            call.body,
        )
        .await;
        assert_eq!(
            served.status, call.served,
            "case {:?}: the owner's own account id must be accepted, expected HTTP {}, got {} \
             with body {:?}",
            call.case,
            call.served,
            served.status,
            text(&served)
        );
    }
}
