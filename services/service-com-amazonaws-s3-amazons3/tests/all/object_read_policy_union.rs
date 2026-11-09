// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the UNION that authorizes an object read made by an IAM identity of
//! the bucket's own account: the identity-based documents in force on the principal that
//! signed the request, and the bucket's own resource policy.
//!
//! The rule under test: "Amazon S3 evaluates all of the relevant access policies, user
//! policies, and resource-based policies in deciding whether to authorize the request"
//! (Amazon S3 User Guide, *Identity and Access Management for Amazon S3*), and within one
//! account "the resource-based policy and the identity-based policy are evaluated together,
//! and access is granted if either allows it" (IAM User Guide, *Policy evaluation logic*) —
//! so a grant in EITHER document alone admits the read, and with NEITHER the request falls to
//! IAM's implicit deny and is refused HTTP 403 `AccessDenied`, naming the principal, the
//! action and the object ARN, with no object bytes served. A statement that names the ACCOUNT
//! rather than the caller delegates to the account instead of granting ("you must also attach
//! an identity-based policy" to the principal), so it admits nothing on its own.
//!
//! Every assertion drives real HTTP requests through a router holding S3 beside IAM over ONE
//! shared engine, so the user, its access key and its policies are made with IAM's own
//! operations and each read is presented with that user's access key id — which is where the
//! emulator reads the signing identity from. Nothing reaches into a store.
//!
//! The table's rows differ along what the rule generalises over, rather than along one
//! recorded call:
//!
//! * WHERE the grant sits — nowhere, the identity's own document, the bucket policy, or both;
//! * how its ACTION element is spelled — the action itself, a prefix wildcard over it, the
//!   whole service, and another action, which reaches the read not at all;
//! * how its RESOURCE element is spelled — every key of the bucket, or the exact key;
//! * which PRINCIPAL spelling a bucket-policy statement reaches the caller by — the caller's
//!   own user ARN, another user's, and the account root;
//! * WHICH action the read asks for — `s3:GetObject` for a read of the current version and
//!   `s3:GetObjectVersion` for one naming a `versionId`, each needing its own grant;
//! * and the OPERATION, since a HEAD of the object is authorized by the same permission ("a
//!   `HEAD` request has the same options as a `GET` operation on an object", `HeadObject`).
//!
//! [`a_resource_policy_attached_later_admits_only_the_principal_it_names`] then drives the
//! union as a STATE TRANSITION over one bucket and three principals at once: attaching a
//! bucket policy that names one of them must flip that one to a served read and leave the
//! others exactly as they were, and withdrawing it must take the grant away again.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, put_bucket_policy, send, Answer};
use super::identity_authorization::{
    no_grant_message, router, s3, statement, user_arn, user_with_grant,
};
use super::object_read_authorization::policy_document;
use super::object_versioning::set_versioning;

/// The access-key id standing for the account's own published credential: neither STS nor IAM
/// holds a record of it, so it resolves to the account's ambient identity — the position AWS
/// account root credentials are in, which no identity-based policy bounds. It writes this
/// suite's fixtures, so no fixture depends on the grants under test.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The key every read in this suite addresses, which each fixture WRITES twice.
const HELD_KEY: &str = "reports/held.csv";

/// The bytes of the FIRST write, which stays readable as the older version — so a read that
/// names a `versionId` has something of its own to serve and a read of the current version
/// cannot pass by serving it.
const OLDER_BODY: &[u8] = b"union-subject-first-write";

/// The bytes of the SECOND write, which is the key's current version.
const CURRENT_BODY: &[u8] = b"union-subject-second-write";

/// The code S3 answers an authorization refusal with (Amazon S3 "Error responses", list of
/// error codes: "AccessDenied"), and the status that code carries.
const ACCESS_DENIED: &str = "AccessDenied";
/// See [`ACCESS_DENIED`].
const ACCESS_DENIED_STATUS: u16 = 403;

/// The IAM action a read of the CURRENT version of an object is authorized under: "to use
/// `GetObject`, you must have the `READ` access to the object" — `s3:GetObject`
/// (`GetObject`'s own documentation of its general purpose bucket permissions).
const GET_OBJECT: &str = "s3:GetObject";

/// The IAM action a read that NAMES a version is authorized under: "if you include a
/// `versionId` in your request header, you must have the `s3:GetObjectVersion` permission to
/// access a specific version of an object. The `s3:GetObject` permission is not required in
/// this scenario" (`GetObject`'s own documentation).
const GET_OBJECT_VERSION: &str = "s3:GetObjectVersion";

/// The ARN of the object every read in this suite addresses — the resource element a grant
/// over the exact key names, and the one a refusal blames.
fn object_arn(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}/{HELD_KEY}")
}

/// The ARN of this emulator's own account root, the spelling a statement names the ACCOUNT by.
fn account_root_arn() -> String {
    format!("arn:aws:iam::{}:root", engine::emulator_account_id())
}

/// Which resources a grant's `Resource` element names.
#[derive(Debug, Clone, Copy)]
enum Scope {
    /// Every key of the bucket, `arn:aws:s3:::<bucket>/*`.
    EveryKey,
    /// The one key the read addresses.
    ExactKey,
}

impl Scope {
    /// The `Resource` element this scope is written as, for `bucket`.
    fn resource(self, bucket: &str) -> String {
        match self {
            Scope::EveryKey => format!("arn:aws:s3:::{bucket}/*"),
            Scope::ExactKey => object_arn(bucket),
        }
    }
}

/// Whom a bucket-policy statement's `Principal` element names.
#[derive(Debug, Clone, Copy)]
enum Who {
    /// The caller's own IAM user.
    Caller,
    /// Another IAM user of the same account, which the caller is not.
    SomebodyElse,
    /// The account's own root, which delegates to the account rather than granting.
    AccountRoot,
}

/// The read the row drives, and so the action it asks for.
#[derive(Debug, Clone, Copy)]
enum Call {
    /// The key's current version, which asks for [`GET_OBJECT`].
    Current,
    /// The older version by its id, which asks for [`GET_OBJECT_VERSION`].
    OlderVersion,
}

impl Call {
    /// The IAM action a read of this shape is judged under.
    fn action(self) -> &'static str {
        match self {
            Call::Current => GET_OBJECT,
            Call::OlderVersion => GET_OBJECT_VERSION,
        }
    }

    /// The bytes a SERVED read of this shape must answer with.
    fn served_body(self) -> &'static [u8] {
        match self {
            Call::Current => CURRENT_BODY,
            Call::OlderVersion => OLDER_BODY,
        }
    }
}

/// One bucket-policy statement: whom it names, which action it states, and over what.
#[derive(Debug, Clone, Copy)]
struct PolicyGrant {
    who: Who,
    effect: &'static str,
    action: &'static str,
    scope: Scope,
}

/// One row: what the caller's own document grants, what the bucket policy states, the read
/// driven, and whether the union admits it.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// The actions the caller's own inline identity policy allows — empty for a principal
    /// carrying no identity policy at all.
    identity: &'static [&'static str],
    /// What that identity policy's `Resource` element names.
    identity_scope: Scope,
    /// The statement the bucket carries, or `None` for a bucket carrying no policy.
    policy: Option<PolicyGrant>,
    call: Call,
    /// Whether the read must be SERVED. A row that is not served must be refused 403
    /// `AccessDenied` naming the row's own action and the object ARN.
    served: bool,
}

/// A statement allowing `action` on `scope` to `who`.
const fn grant(who: Who, action: &'static str, scope: Scope) -> Option<PolicyGrant> {
    Some(PolicyGrant {
        who,
        effect: "Allow",
        action,
        scope,
    })
}

/// The table: one row per (where the grant sits, how it is spelled, what the read asks for)
/// combination the rule distinguishes.
const ROWS: &[Row] = &[
    Row {
        case: "neither an identity-policy grant nor a bucket policy at all",
        identity: &[],
        identity_scope: Scope::EveryKey,
        policy: None,
        call: Call::Current,
        served: false,
    },
    Row {
        case: "an identity-policy grant alone, over every key of the bucket",
        identity: &[GET_OBJECT],
        identity_scope: Scope::EveryKey,
        policy: None,
        call: Call::Current,
        served: true,
    },
    Row {
        case: "an identity-policy grant alone, over the exact key",
        identity: &[GET_OBJECT],
        identity_scope: Scope::ExactKey,
        policy: None,
        call: Call::Current,
        served: true,
    },
    Row {
        case: "an identity-policy grant alone, spelled as a prefix wildcard",
        identity: &["s3:Get*"],
        identity_scope: Scope::EveryKey,
        policy: None,
        call: Call::Current,
        served: true,
    },
    Row {
        case: "an identity-policy grant alone, spelled as the whole service",
        identity: &["s3:*"],
        identity_scope: Scope::EveryKey,
        policy: None,
        call: Call::Current,
        served: true,
    },
    Row {
        case: "an identity-policy grant for another action only",
        identity: &["s3:PutObject"],
        identity_scope: Scope::EveryKey,
        policy: None,
        call: Call::Current,
        served: false,
    },
    Row {
        case: "a bucket-policy grant naming the caller, with no identity policy",
        identity: &[],
        identity_scope: Scope::EveryKey,
        policy: grant(Who::Caller, GET_OBJECT, Scope::EveryKey),
        call: Call::Current,
        served: true,
    },
    Row {
        case: "a bucket-policy grant naming the caller over the exact key",
        identity: &[],
        identity_scope: Scope::EveryKey,
        policy: grant(Who::Caller, GET_OBJECT, Scope::ExactKey),
        call: Call::Current,
        served: true,
    },
    Row {
        case: "a bucket-policy grant naming the caller, spelled as the whole service",
        identity: &[],
        identity_scope: Scope::EveryKey,
        policy: grant(Who::Caller, "s3:*", Scope::EveryKey),
        call: Call::Current,
        served: true,
    },
    Row {
        case: "a bucket-policy grant naming another user of the account",
        identity: &[],
        identity_scope: Scope::EveryKey,
        policy: grant(Who::SomebodyElse, GET_OBJECT, Scope::EveryKey),
        call: Call::Current,
        served: false,
    },
    Row {
        case: "a bucket-policy grant naming the account root rather than the caller",
        identity: &[],
        identity_scope: Scope::EveryKey,
        policy: grant(Who::AccountRoot, GET_OBJECT, Scope::EveryKey),
        call: Call::Current,
        served: false,
    },
    Row {
        case: "a bucket-policy grant naming the caller for another action",
        identity: &[],
        identity_scope: Scope::EveryKey,
        policy: grant(Who::Caller, "s3:PutObject", Scope::EveryKey),
        call: Call::Current,
        served: false,
    },
    Row {
        case: "a grant on BOTH sides of the union",
        identity: &[GET_OBJECT],
        identity_scope: Scope::EveryKey,
        policy: grant(Who::Caller, GET_OBJECT, Scope::EveryKey),
        call: Call::Current,
        served: true,
    },
    Row {
        case: "an identity-policy grant with a bucket-policy deny of the same read",
        identity: &[GET_OBJECT],
        identity_scope: Scope::EveryKey,
        policy: Some(PolicyGrant {
            who: Who::Caller,
            effect: "Deny",
            action: GET_OBJECT,
            scope: Scope::EveryKey,
        }),
        call: Call::Current,
        served: false,
    },
    Row {
        case: "a versioned read with neither grant",
        identity: &[],
        identity_scope: Scope::EveryKey,
        policy: None,
        call: Call::OlderVersion,
        served: false,
    },
    Row {
        case: "a versioned read granted only by the identity policy",
        identity: &[GET_OBJECT_VERSION],
        identity_scope: Scope::EveryKey,
        policy: None,
        call: Call::OlderVersion,
        served: true,
    },
    Row {
        case: "a versioned read granted only by the bucket policy",
        identity: &[],
        identity_scope: Scope::EveryKey,
        policy: grant(Who::Caller, GET_OBJECT_VERSION, Scope::EveryKey),
        call: Call::OlderVersion,
        served: true,
    },
    Row {
        case: "a versioned read whose only grants, on both sides, are the unversioned action",
        identity: &[GET_OBJECT],
        identity_scope: Scope::EveryKey,
        policy: grant(Who::Caller, GET_OBJECT, Scope::EveryKey),
        call: Call::OlderVersion,
        served: false,
    },
];

/// A versioning-enabled bucket holding [`HELD_KEY`] twice over — [`OLDER_BODY`] then
/// [`CURRENT_BODY`] — and the version ids of the two writes, older first. Written with the
/// account's own credential, so no grant under test decides whether the fixture is there.
///
/// Its own fixture rather than the sibling suite's ([`super::identity_authorization`]), for
/// one reason this suite's rows turn on: the two writes carry DIFFERENT bytes, so a read that
/// names a `versionId` cannot pass by serving the current version and the other way round.
async fn seeded_bucket(router: &MultiServiceRouter, bucket: &str) -> (String, String) {
    let created = create_bucket(router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "bucket {bucket:?}: the create must be served, got {} with body {:?}",
        created.status,
        body_text(&created)
    );
    set_versioning(router, bucket, "Enabled").await;
    let first = write(router, bucket, OLDER_BODY).await;
    assert_eq!(
        first.status, 200,
        "bucket {bucket:?}: the first write must be stored, got {} with body {:?}",
        first.status,
        body_text(&first)
    );
    let older = first
        .header("x-amz-version-id")
        .expect("a versioning-enabled bucket mints a version id for every write");
    let second = write(router, bucket, CURRENT_BODY).await;
    assert_eq!(
        second.status, 200,
        "bucket {bucket:?}: the second write must be stored, got {} with body {:?}",
        second.status,
        body_text(&second)
    );
    let current = second
        .header("x-amz-version-id")
        .expect("a versioning-enabled bucket mints a version id for every write");
    (older, current)
}

/// One fixture write of [`HELD_KEY`], made with the account's own credential.
async fn write(router: &MultiServiceRouter, bucket: &str, payload: &[u8]) -> Answer {
    s3(
        router,
        Some(ACCOUNT_KEY),
        "PUT",
        &format!("/{bucket}/{HELD_KEY}"),
        payload,
    )
    .await
}

/// A read of [`HELD_KEY`] as `akid` speaks for its principal: `method` is `GET` or `HEAD`, and
/// `version_id` names the stored version the read addresses, when it names one.
async fn read(
    router: &MultiServiceRouter,
    akid: &str,
    bucket: &str,
    method: &str,
    version_id: Option<&str>,
) -> Answer {
    let query = match version_id {
        Some(version_id) => format!("?versionId={version_id}"),
        None => String::new(),
    };
    s3(
        router,
        Some(akid),
        method,
        &format!("/{bucket}/{HELD_KEY}{query}"),
        b"",
    )
    .await
}

/// The answer's body as text, for an assertion message.
fn body_text(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).to_string()
}

/// RULE: within one account an object read is admitted by a grant in EITHER the caller's
/// identity-based documents or the bucket's own resource policy, and with neither it is
/// refused 403 `AccessDenied` naming the principal, the action that read asked for and the
/// object ARN — serving no bytes.
#[tokio::test]
async fn either_policy_alone_admits_an_object_read_and_neither_refuses_it() {
    let router = router();
    // The principal one row's statement names instead of the caller: a user the account
    // really holds, since `PutBucketPolicy` refuses a document naming one it does not.
    let other = "read-union-somebody-else";
    user_with_grant(&router, other, &[], &[]).await;

    for (index, row) in ROWS.iter().enumerate() {
        // Its own bucket and its own caller per row, so no row's grant can decide another's
        // answer.
        let bucket = format!("read-union-{index}");
        let (older, current) = seeded_bucket(&router, &bucket).await;
        let user = format!("read-union-user-{index}");
        let akid = user_with_grant(
            &router,
            &user,
            row.identity,
            &[row.identity_scope.resource(&bucket)],
        )
        .await;

        if let Some(policy) = row.policy {
            let principal = match policy.who {
                Who::Caller => user_arn(&user),
                Who::SomebodyElse => user_arn(other),
                Who::AccountRoot => account_root_arn(),
            };
            let document = policy_document(&statement(
                policy.effect,
                &principal,
                policy.action,
                &policy.scope.resource(&bucket),
            ));
            let attached = put_bucket_policy(&router, &bucket, &document).await;
            assert_eq!(
                attached.status,
                204,
                "case {:?}: the bucket policy must be stored, got {} with body {:?}",
                row.case,
                attached.status,
                body_text(&attached)
            );
        }

        let version_id = match row.call {
            Call::Current => None,
            Call::OlderVersion => Some(older.as_str()),
        };
        let answer = read(&router, &akid, &bucket, "GET", version_id).await;
        if row.served {
            assert_eq!(
                answer.status,
                200,
                "case {:?}: expected the read to be served, got {} with body {:?}",
                row.case,
                answer.status,
                body_text(&answer)
            );
            assert_eq!(
                answer.body.as_ref(),
                row.call.served_body(),
                "case {:?}: expected the bytes of the version addressed, got {:?}",
                row.case,
                body_text(&answer)
            );
        } else {
            let expected = no_grant_message(
                &user_arn(&user),
                row.call.action(),
                &object_arn(&bucket),
            );
            assert_eq!(
                answer.status, ACCESS_DENIED_STATUS,
                "case {:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {} with body {:?}",
                row.case,
                answer.status,
                body_text(&answer)
            );
            assert_eq!(
                answer.error_code(),
                ACCESS_DENIED,
                "case {:?}: expected the {ACCESS_DENIED:?} code, got {:?}",
                row.case,
                body_text(&answer)
            );
            // The refusal names the principal, the action the read asked for and the
            // resource; an explicit deny is worded for the document that decided it, so
            // only the implicit-deny rows are pinned to the no-grant sentence.
            if row.policy.is_none_or(|policy| policy.effect == "Allow") {
                assert_eq!(
                    answer.error_message(),
                    expected,
                    "case {:?}: expected the message {expected:?}, got {:?}",
                    row.case,
                    answer.error_message()
                );
            }
            assert!(
                !answer.body.as_ref().windows(OLDER_BODY.len()).any(|window| window == OLDER_BODY)
                    && !answer
                        .body
                        .as_ref()
                        .windows(CURRENT_BODY.len())
                        .any(|window| window == CURRENT_BODY),
                "case {:?}: a refused read must serve no object bytes, got {:?}",
                row.case,
                body_text(&answer)
            );
        }

        // A HEAD of the same object is authorized by the same permission ("a `HEAD` request
        // has the same options as a `GET` operation on an object", `HeadObject`) and carries
        // no body, so what it answers is its status and its headers: a served HEAD reports
        // the length of the version addressed, and a refused one is the same 403 — asserted
        // against the row rather than against the GET, so a HEAD cannot pass by agreeing
        // with a wrong GET.
        let headed = read(&router, &akid, &bucket, "HEAD", version_id).await;
        let expected_status = match row.served {
            true => 200,
            false => ACCESS_DENIED_STATUS,
        };
        assert_eq!(
            headed.status, expected_status,
            "case {:?}: expected the HEAD to answer HTTP {expected_status}, got {}",
            row.case, headed.status
        );
        if row.served {
            assert_eq!(
                headed.header("content-length"),
                Some(row.call.served_body().len().to_string()),
                "case {:?}: the HEAD must report the length of the version addressed, got {:?}",
                row.case,
                headed.header("content-length")
            );
            assert_eq!(
                headed.header("x-amz-version-id").as_deref(),
                version_id.or(Some(current.as_str())),
                "case {:?}: the HEAD must name the version it answered for, got {:?}",
                row.case,
                headed.header("x-amz-version-id")
            );
        }
    }
}

/// RULE: the union is taken from the documents in force AT THE TIME OF THE REQUEST, so
/// attaching a resource policy that names one principal admits that principal's read and
/// leaves every other principal's outcome exactly as it was, and withdrawing it takes the
/// grant away again.
#[tokio::test]
async fn a_resource_policy_attached_later_admits_only_the_principal_it_names() {
    let router = router();
    let bucket = "read-union-transition";
    let (_older, _current) = seeded_bucket(&router, bucket).await;

    // Three principals of the one account: one granted the read by its OWN document, one
    // the bucket policy will name later, and one granted by neither at any point.
    let identity_user = "read-union-identity-only";
    let identity_akid = user_with_grant(
        &router,
        identity_user,
        &[GET_OBJECT],
        &[format!("arn:aws:s3:::{bucket}/*")],
    )
    .await;
    let resource_user = "read-union-resource-only";
    let resource_akid = user_with_grant(&router, resource_user, &[], &[]).await;
    let ungranted_user = "read-union-no-grant";
    let ungranted_akid = user_with_grant(&router, ungranted_user, &[], &[]).await;

    // (the stage's name, whether the bucket-policy grant is in force)
    let stages: [(&str, bool); 3] = [
        ("before any bucket policy is attached", false),
        ("with a bucket policy naming the second principal", true),
        ("after that bucket policy is withdrawn", false),
    ];
    for (stage, policy_in_force) in stages {
        match policy_in_force {
            true => {
                let document = policy_document(&statement(
                    "Allow",
                    &user_arn(resource_user),
                    GET_OBJECT,
                    &format!("arn:aws:s3:::{bucket}/*"),
                ));
                let attached = put_bucket_policy(&router, bucket, &document).await;
                assert_eq!(
                    attached.status,
                    204,
                    "stage {stage:?}: the bucket policy must be stored, got {} with body {:?}",
                    attached.status,
                    body_text(&attached)
                );
            }
            false => {
                let removed = send(&router, "DELETE", &format!("/{bucket}?policy"), &[], "").await;
                assert_eq!(
                    removed.status,
                    204,
                    "stage {stage:?}: the bucket must be left without a policy, got {} with \
                     body {:?}",
                    removed.status,
                    body_text(&removed)
                );
            }
        }

        // (the principal, its key, whether the union admits its read at this stage)
        let callers: [(&str, &str, bool); 3] = [
            (identity_user, identity_akid.as_str(), true),
            (resource_user, resource_akid.as_str(), policy_in_force),
            (ungranted_user, ungranted_akid.as_str(), false),
        ];
        for (user, akid, served) in callers {
            let answer = read(&router, akid, bucket, "GET", None).await;
            let case = format!("stage {stage:?}, caller {user:?}");
            if served {
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?}: expected the read to be served, got {} with body {:?}",
                    answer.status,
                    body_text(&answer)
                );
                assert_eq!(
                    answer.body.as_ref(),
                    CURRENT_BODY,
                    "case {case:?}: expected the stored bytes, got {:?}",
                    body_text(&answer)
                );
            } else {
                let expected =
                    no_grant_message(&user_arn(user), GET_OBJECT, &object_arn(bucket));
                assert_eq!(
                    answer.status, ACCESS_DENIED_STATUS,
                    "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {} with body {:?}",
                    answer.status,
                    body_text(&answer)
                );
                assert_eq!(
                    answer.error_code(),
                    ACCESS_DENIED,
                    "case {case:?}: expected the {ACCESS_DENIED:?} code, got {:?}",
                    body_text(&answer)
                );
                assert_eq!(
                    answer.error_message(),
                    expected,
                    "case {case:?}: expected the message {expected:?}, got {:?}",
                    answer.error_message()
                );
            }
        }
    }
}
