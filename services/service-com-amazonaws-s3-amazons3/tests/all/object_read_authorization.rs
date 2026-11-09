// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what AUTHORIZES an object read made with a credential: the bucket's
//! own policy, and the owner the request says it expects.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle and object suites in this crate own), so what is pinned is the wire
//! answer: the status, the error document's `<Code>` and `<Message>`, and the bytes a served
//! read answers with.
//!
//! The rules under test:
//!
//! * A bucket policy is a RESOURCE-BASED policy, so it is in force on every caller and not
//!   only on the public ones ([`super::anonymous_object_reads`] is the same question asked
//!   of a caller presenting none): an explicit `Deny` about the request refuses it with HTTP
//!   403 `AccessDenied` — "an explicit deny in any policy overrides any allows" (IAM's own
//!   evaluation order) — and it is judged BEFORE anything about the key is reported, so a
//!   refusal never says whether the object is there. A document that says nothing about the
//!   request leaves the read to the bucket's owner, which is the one account this emulator
//!   serves, and so does a `Deny` whose condition this service cannot judge: the emulator
//!   answers the condition keys it knows (the secure transport every one of its endpoints
//!   serves) and refuses only a deny it could decide.
//! * WHICH refusal it is depends on what the address resolved to, and both are worded as the
//!   explicit deny they are, naming the action the policy denied. A version the read did
//!   resolve is refused the read itself (`s3:GetObject` on that object). An address holding
//!   NOTHING is answered by what the caller may learn about the bucket's contents at all:
//!   "if the object that you request doesn't exist, the error that Amazon S3 returns depends
//!   on whether you also have the `s3:ListBucket` permission. If you have the
//!   `s3:ListBucket` permission on the bucket, Amazon S3 returns an HTTP status code 404 Not
//!   Found error. If you don't have the `s3:ListBucket` permission, Amazon S3 returns an HTTP
//!   status code 403 Access Denied error" (`GetObject`'s own documentation) — the 403
//!   carrying the sentence IAM publishes for an explicit deny, which names the caller, the
//!   action and the resource ARN.
//! * The same permission authorizes a HEAD of the object — "a HEAD request has the same
//!   options as a GET operation on an object" (`HeadObject`'s own documentation) — which
//!   carries no body, so the status is its whole answer.
//! * `x-amz-expected-bucket-owner` is the caller's own guard on both: "if the account ID that
//!   you provide does not match the actual owner of the bucket, the request fails with the
//!   HTTP status code 403 Forbidden (access denied)" (`GetObjectRequest$ExpectedBucketOwner`).
//!
//! The policy table's rows differ along what the rule generalises over: which ACTION the deny
//! names (the read itself, a wildcard over the read, every S3 action, the listing), which
//! RESOURCE it names (one key, every key of the bucket, another key, the bucket itself),
//! which PRINCIPAL spelling it reaches the caller by (the wildcard, the account root),
//! whether the deny carries a CONDITION this service can decide (the secure-transport
//! guardrail clause every published bucket policy carries, stated both ways), and whether the
//! key the read addresses is there at all — and each row is driven through both GET and HEAD.

use service_com_amazonaws_s3_amazons3::state::caller_principal_arn;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{put_bucket_policy, router};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_versioning::head_object;

/// The bytes every object in this suite holds. A policy is about who may reach an object,
/// not about what is in it, so one payload serves every row.
const BODY: &[u8] = b"authorized-subject";

/// The key every row addresses, which the setup WRITES.
const HELD_KEY: &str = "reports/held.csv";

/// A key the setup never writes, so a read of it resolves no version at all.
const ABSENT_KEY: &str = "reports/never-written.csv";

/// The code S3 answers an authorization refusal with, and the message that code carries
/// (Amazon S3 "Error responses", list of error codes: "AccessDenied — Access Denied").
const ACCESS_DENIED: &str = "AccessDenied";
/// See [`ACCESS_DENIED`].
const ACCESS_DENIED_MESSAGE: &str = "Access Denied";

/// The code and message a read of a key the bucket holds nothing at answers with.
const NO_SUCH_KEY: &str = "NoSuchKey";
/// See [`NO_SUCH_KEY`].
const NO_SUCH_KEY_MESSAGE: &str = "The specified key does not exist.";

/// A policy document carrying `statements`, in the form a client sends one.
///
/// `pub(crate)`: the suite that pins the identity-OR-resource union
/// ([`super::object_read_policy_union`]) wraps its statements with this one builder rather
/// than spelling the envelope a second way.
pub(crate) fn policy_document(statements: &str) -> String {
    format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{statements}]}}")
}

/// One statement with the effect, principal, action and resource given.
fn statement(effect: &str, principal: &str, action: &str, resource: &str) -> String {
    format!(
        "{{\"Effect\":\"{effect}\",\"Principal\":{principal},\"Action\":\"{action}\",\
         \"Resource\":\"{resource}\"}}"
    )
}

/// One statement reaching every principal, carrying a `Condition`: `resource` is the JSON
/// the element is written as (one ARN or a list of them) and `condition` the whole clause.
fn conditional_statement(effect: &str, action: &str, resource: &str, condition: &str) -> String {
    format!(
        "{{\"Effect\":\"{effect}\",\"Principal\":\"*\",\"Action\":\"{action}\",\
         \"Resource\":{resource},\"Condition\":{condition}}}"
    )
}

/// The sentence S3 refuses a request an explicit `Deny` in the resource's own policy barred
/// with: the principal, the action and the resource ARN, and the reason clause IAM publishes
/// for that decision. Written out here rather than built from the service's own helper, so
/// the wording has something to be wrong about.
fn explicit_deny_message(action: &str, resource: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {action} on resource: \"{resource}\" with an \
         explicit deny in a resource-based policy",
        caller_principal_arn()
    )
}

/// The sentence a read of the HELD object is refused with when the policy explicitly denies
/// it: the explicit deny over `s3:GetObject` on that object, whose ARN is the bucket's own
/// with the key as the rest of the resource part.
fn denied_read_message(bucket: &str) -> String {
    explicit_deny_message("s3:GetObject", &format!("arn:aws:s3:::{bucket}/{HELD_KEY}"))
}

/// What a read must answer: the status, and the error document's code and message (both
/// empty for a read that is served with the object's bytes).
struct Expected {
    status: u16,
    code: &'static str,
    message: String,
}

/// One row: a bucket carrying one policy, the key the read addresses, and what the read of
/// it must then answer.
struct Row {
    case: &'static str,
    /// The bucket this row configures — its own, so one row's policy cannot authorize
    /// another's read.
    bucket: &'static str,
    /// The policy document the bucket carries, or `None` for a bucket carrying none.
    policy: Option<String>,
    key: &'static str,
    expected: Expected,
}

/// `(the object the read addresses, what it must answer)` for every row of the table.
fn rows() -> Vec<Row> {
    let everyone = "\"*\"";
    let account_root = format!("{{\"AWS\":\"{}\"}}", account_root_arn());
    vec![
        Row {
            case: "no policy at all: the bucket's owner reads its own object",
            bucket: "read-authz-no-policy",
            policy: None,
            key: HELD_KEY,
            expected: Expected {
                status: 200,
                code: "",
                message: String::new(),
            },
        },
        Row {
            case: "a policy that only ALLOWS the read, to another principal",
            bucket: "read-authz-allowed",
            policy: Some(policy_document(&statement(
                "Allow",
                "{\"Service\":\"logs.amazonaws.com\"}",
                "s3:GetObject",
                "arn:aws:s3:::read-authz-allowed/*",
            ))),
            key: HELD_KEY,
            expected: Expected {
                status: 200,
                code: "",
                message: String::new(),
            },
        },
        Row {
            case: "a Deny of the read on EVERY key, over a key that is there",
            bucket: "read-authz-deny-all-keys",
            policy: Some(policy_document(&statement(
                "Deny",
                everyone,
                "s3:GetObject",
                "arn:aws:s3:::read-authz-deny-all-keys/*",
            ))),
            key: HELD_KEY,
            expected: Expected {
                status: 403,
                code: ACCESS_DENIED,
                message: denied_read_message("read-authz-deny-all-keys"),
            },
        },
        Row {
            case: "a Deny of the read on THAT ONE key",
            bucket: "read-authz-deny-one-key",
            policy: Some(policy_document(&statement(
                "Deny",
                everyone,
                "s3:GetObject",
                "arn:aws:s3:::read-authz-deny-one-key/reports/held.csv",
            ))),
            key: HELD_KEY,
            expected: Expected {
                status: 403,
                code: ACCESS_DENIED,
                message: denied_read_message("read-authz-deny-one-key"),
            },
        },
        Row {
            case: "a Deny naming the ACCOUNT ROOT rather than the wildcard",
            bucket: "read-authz-deny-account",
            policy: Some(policy_document(&statement(
                "Deny",
                &account_root,
                "s3:Get*",
                "arn:aws:s3:::read-authz-deny-account/*",
            ))),
            key: HELD_KEY,
            expected: Expected {
                status: 403,
                code: ACCESS_DENIED,
                message: denied_read_message("read-authz-deny-account"),
            },
        },
        Row {
            case: "a Deny of every S3 action, over a key that is there",
            bucket: "read-authz-deny-every-action",
            policy: Some(policy_document(&statement(
                "Deny",
                everyone,
                "s3:*",
                "arn:aws:s3:::read-authz-deny-every-action/*",
            ))),
            key: HELD_KEY,
            expected: Expected {
                status: 403,
                code: ACCESS_DENIED,
                message: denied_read_message("read-authz-deny-every-action"),
            },
        },
        Row {
            case: "a Deny of the read on ANOTHER key leaves this one readable",
            bucket: "read-authz-deny-other-key",
            policy: Some(policy_document(&statement(
                "Deny",
                everyone,
                "s3:GetObject",
                "arn:aws:s3:::read-authz-deny-other-key/private/*",
            ))),
            key: HELD_KEY,
            expected: Expected {
                status: 200,
                code: "",
                message: String::new(),
            },
        },
        Row {
            case: "a Deny of the LISTING leaves a key that IS there readable",
            bucket: "read-authz-deny-listing-held",
            policy: Some(policy_document(&statement(
                "Deny",
                everyone,
                "s3:ListBucket",
                "arn:aws:s3:::read-authz-deny-listing-held",
            ))),
            key: HELD_KEY,
            expected: Expected {
                status: 200,
                code: "",
                message: String::new(),
            },
        },
        Row {
            // The guardrail clause AWS publishes for a bucket policy: everything that did
            // NOT arrive over a secure transport is denied. The emulator serves every
            // endpoint over TLS, so the clause does not hold for this request and the Deny
            // carrying it refuses nothing.
            case: "a Deny of everything not sent over a secure transport",
            bucket: "read-authz-insecure-transport",
            policy: Some(policy_document(&conditional_statement(
                "Deny",
                "s3:*",
                "[\"arn:aws:s3:::read-authz-insecure-transport\",\
                 \"arn:aws:s3:::read-authz-insecure-transport/*\"]",
                "{\"Bool\":{\"aws:SecureTransport\":\"false\"}}",
            ))),
            key: HELD_KEY,
            expected: Expected {
                status: 200,
                code: "",
                message: String::new(),
            },
        },
        Row {
            // The same clause over the SAME key, stated so that it DOES hold here: a
            // request over a secure transport is what this one is, so the Deny applies.
            case: "a Deny of everything sent over a secure transport",
            bucket: "read-authz-secure-transport",
            policy: Some(policy_document(&conditional_statement(
                "Deny",
                "s3:*",
                "[\"arn:aws:s3:::read-authz-secure-transport\",\
                 \"arn:aws:s3:::read-authz-secure-transport/*\"]",
                "{\"Bool\":{\"aws:SecureTransport\":\"true\"}}",
            ))),
            key: HELD_KEY,
            expected: Expected {
                status: 403,
                code: ACCESS_DENIED,
                message: denied_read_message("read-authz-secure-transport"),
            },
        },
        Row {
            // A Deny this service cannot JUDGE: the clause is over a condition key nothing
            // here answers, so whether it holds is unknown — and an unknown refusal does
            // not stand in the way of the bucket owner's own read.
            case: "a Deny conditioned on a key this service does not answer",
            bucket: "read-authz-undecidable-deny",
            policy: Some(policy_document(&conditional_statement(
                "Deny",
                "s3:GetObject",
                "\"arn:aws:s3:::read-authz-undecidable-deny/*\"",
                "{\"StringEquals\":{\"s3:ExistingObjectTag/stage\":\"production\"}}",
            ))),
            key: HELD_KEY,
            expected: Expected {
                status: 200,
                code: "",
                message: String::new(),
            },
        },
        Row {
            case: "no policy at all, over a key that is NOT there",
            bucket: "read-authz-absent-no-policy",
            policy: None,
            key: ABSENT_KEY,
            expected: Expected {
                status: 404,
                code: NO_SUCH_KEY,
                message: NO_SUCH_KEY_MESSAGE.to_string(),
            },
        },
        Row {
            case: "a Deny of the READ alone still reports a key that is not there as absent",
            bucket: "read-authz-absent-deny-read",
            policy: Some(policy_document(&statement(
                "Deny",
                everyone,
                "s3:GetObject",
                "arn:aws:s3:::read-authz-absent-deny-read/*",
            ))),
            key: ABSENT_KEY,
            expected: Expected {
                status: 404,
                code: NO_SUCH_KEY,
                message: NO_SUCH_KEY_MESSAGE.to_string(),
            },
        },
        Row {
            case: "a Deny of the LISTING refuses a key that is not there instead of reporting it",
            bucket: "read-authz-absent-deny-listing",
            policy: Some(policy_document(&statement(
                "Deny",
                everyone,
                "s3:ListBucket",
                "arn:aws:s3:::read-authz-absent-deny-listing",
            ))),
            key: ABSENT_KEY,
            expected: Expected {
                status: 403,
                code: ACCESS_DENIED,
                message: explicit_deny_message(
                    "s3:ListBucket",
                    "arn:aws:s3:::read-authz-absent-deny-listing",
                ),
            },
        },
        Row {
            case: "a Deny of every S3 action, over a key that is not there",
            bucket: "read-authz-absent-deny-every-action",
            policy: Some(policy_document(&format!(
                "{},{}",
                statement(
                    "Deny",
                    everyone,
                    "s3:*",
                    "arn:aws:s3:::read-authz-absent-deny-every-action"
                ),
                statement(
                    "Deny",
                    everyone,
                    "s3:*",
                    "arn:aws:s3:::read-authz-absent-deny-every-action/*"
                ),
            ))),
            key: ABSENT_KEY,
            expected: Expected {
                status: 403,
                code: ACCESS_DENIED,
                message: explicit_deny_message(
                    "s3:ListBucket",
                    "arn:aws:s3:::read-authz-absent-deny-every-action",
                ),
            },
        },
    ]
}

/// The ARN of this emulator's own account root — the other spelling a policy statement can
/// reach the caller by.
fn account_root_arn() -> String {
    format!("arn:aws:iam::{}:root", engine::emulator_account_id())
}

/// Create `row`'s bucket, write the one object the rows read, and attach the row's policy.
async fn configured_bucket(router: &MultiServiceRouter, row: &Row) {
    bucket(router, row.bucket).await;
    let written = put_object(router, row.bucket, HELD_KEY, BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "case {:?}: the object must be stored before the policy is attached, got {} with \
         body {:?}",
        row.case,
        written.status,
        text(&written)
    );
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
async fn a_bucket_policy_decides_whether_a_credentialed_read_is_served() {
    let router = router();

    for row in rows() {
        configured_bucket(&router, &row).await;

        let read = get_object(&router, row.bucket, row.key, "", &[]).await;
        assert_eq!(
            read.status, row.expected.status,
            "case {:?}: expected HTTP {}, got {} with body {:?}",
            row.case,
            row.expected.status,
            read.status,
            text(&read)
        );
        if row.expected.code.is_empty() {
            assert_eq!(
                read.body.as_ref(),
                BODY,
                "case {:?}: a read the policy leaves alone must serve the stored bytes, got \
                 {:?}",
                row.case,
                text(&read)
            );
        } else {
            assert_eq!(
                read.error_code(),
                row.expected.code,
                "case {:?}: expected the {:?} code, got {:?} with body {:?}",
                row.case,
                row.expected.code,
                read.error_code(),
                text(&read)
            );
            assert_eq!(
                read.error_message(),
                row.expected.message,
                "case {:?}: expected the message {:?}, got {:?}",
                row.case,
                row.expected.message,
                read.error_message()
            );
        }

        // A HEAD of the same object is authorized by the same permission and carries no
        // body, so the STATUS is its whole answer.
        let headed = head_object(&router, row.bucket, row.key, "").await;
        assert_eq!(
            headed.status, row.expected.status,
            "case {:?}: a HEAD must answer the status its GET does, expected {}, got {}",
            row.case, row.expected.status, headed.status
        );
    }
}

#[tokio::test]
async fn a_denied_read_says_nothing_about_whether_the_key_is_there() {
    let router = router();
    let name = "read-authz-opaque";
    bucket(&router, name).await;
    let written = put_object(&router, name, HELD_KEY, BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the object must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );
    // A policy that denies BOTH the read and the listing: the one state in which a caller
    // can tell nothing about the bucket's contents.
    let policy = policy_document(&format!(
        "{},{}",
        statement("Deny", "\"*\"", "s3:GetObject", &format!("arn:aws:s3:::{name}/*")),
        statement("Deny", "\"*\"", "s3:ListBucket", &format!("arn:aws:s3:::{name}")),
    ));
    let attached = put_bucket_policy(&router, name, &policy).await;
    assert_eq!(
        attached.status, 204,
        "the policy must be attached, got {} with body {:?}",
        attached.status,
        text(&attached)
    );

    // Both reads are refused with 403, and neither carries a code that would say whether
    // the key is there.
    for (case, key) in [
        ("a key the bucket holds", HELD_KEY),
        ("a key it does not", ABSENT_KEY),
    ] {
        let read = get_object(&router, name, key, "", &[]).await;
        assert_eq!(
            read.status, 403,
            "case {case:?}: expected HTTP 403, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.error_code(),
            ACCESS_DENIED,
            "case {case:?}: expected the {ACCESS_DENIED:?} code, got {:?}",
            read.error_code()
        );
        assert!(
            !text(&read).contains(NO_SUCH_KEY),
            "case {case:?}: a refusal must not report whether the key is there, got {:?}",
            text(&read)
        );
    }
}

#[tokio::test]
async fn an_expected_bucket_owner_that_is_not_the_owner_refuses_the_read() {
    let router = router();
    let name = "read-authz-owner-guard";
    bucket(&router, name).await;
    let written = put_object(&router, name, HELD_KEY, BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the object must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );

    // (case, the account id the request expects, the status it must answer). The rows differ
    // along WHOSE account the guard names: the one this emulator serves, another account of
    // the same shape, and a value that is not an account id at all.
    let owner = engine::emulator_account_id();
    let rows: [(&str, String, u16); 3] = [
        ("the account this emulator serves", owner.to_string(), 200),
        ("another account of the same shape", "123456789012".to_string(), 403),
        ("a value that is no account id", "not-an-account".to_string(), 403),
    ];
    for (case, expected_owner, status) in rows {
        for (call, answer) in [
            (
                "GetObject",
                get_object(
                    &router,
                    name,
                    HELD_KEY,
                    "",
                    &[("x-amz-expected-bucket-owner", expected_owner.as_str())],
                )
                .await,
            ),
            (
                "HeadObject",
                super::bucket_lifecycle::send_bytes(
                    &router,
                    "HEAD",
                    &format!("/{name}/{HELD_KEY}"),
                    &[("x-amz-expected-bucket-owner", expected_owner.as_str())],
                    b"",
                )
                .await,
            ),
        ] {
            assert_eq!(
                answer.status, status,
                "case {case:?}/{call}: expected HTTP {status}, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            if status != 200 && call == "GetObject" {
                assert_eq!(
                    answer.error_code(),
                    ACCESS_DENIED,
                    "case {case:?}/{call}: expected the {ACCESS_DENIED:?} code, got {:?}",
                    answer.error_code()
                );
                assert_eq!(
                    answer.error_message(),
                    ACCESS_DENIED_MESSAGE,
                    "case {case:?}/{call}: expected the published message, got {:?}",
                    answer.error_message()
                );
            }
        }
    }
}
