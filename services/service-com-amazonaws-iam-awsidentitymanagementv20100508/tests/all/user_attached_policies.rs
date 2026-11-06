// SPDX-License-Identifier: CC0-1.0

//! `AttachUserPolicy` — attaching a managed policy to an IAM user — end to end over
//! the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one
//! shared `InMemoryEngine`, exactly as the production router does, so what is under
//! test is the HTTP behaviour (status, awsQuery error code, XML members) of an
//! attachment that was PERSISTED, never a handler called directly. The attachment is
//! read back through the implemented readers of the same state: the entities a
//! policy relates (`ListEntitiesForPolicy`), the policy's own `AttachmentCount`
//! (`GetPolicy`), the context keys the user's policies reference
//! (`GetContextKeysForPrincipalPolicy`), and the dependents a user delete refuses on
//! (`DeleteUser`). The awsQuery harness is the one `tests/all/user_lifecycle.rs`
//! owns and the other suites in this crate share.
//!
//! The suites are TABLES: each row differs along what the rule being asserted
//! generalises over (the policy's owner and path, the user's path and name case, the
//! number of attachments, the entity and usage filter, the form of a refused ARN or
//! name), so no single recorded request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// A managed policy document with a single statement — the form `CreatePolicy`'s own
/// modelled example carries. Every fixture policy needs one to exist at all.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// The customer-managed policies the fixture account holds: `(PolicyName, Path)`.
/// The paths span the account root, a one-segment path and a nested path, so a rule
/// about a policy's ARN is exercised at more than one depth.
const CUSTOMER_POLICIES: &[(&str, &str)] = &[
    ("ReadItems", "/"),
    ("WriteItems", "/app/"),
    ("AuditItems", "/app/audit/"),
];

/// ARNs of AWS-managed policies — owned by AWS rather than by the account, so the
/// account holds no record of them. The rows span the paths the managed-policy ARN
/// grammar admits: the account root, the `/service-role/` path AWS files the policies
/// of a service role under, and the `/job-function/` path. The names and paths are
/// entries of the AWS Managed Policy Reference; what the rule generalises over is the
/// ARN's SHAPE (path depth, name alphabet), which is why the rows differ in path
/// rather than only in name.
const AWS_MANAGED_POLICIES: &[&str] = &[
    "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess",
    "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole",
    "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess",
];

/// How many managed policies one user may carry: AWS's "IAM and STS quotas" page
/// gives "Managed policies attached to an IAM user: 10", which is the quota
/// `crate::state::MAX_MANAGED_POLICIES_PER_USER` records.
const MAX_MANAGED_POLICIES_PER_USER: usize = 10;

/// One fixture user and the fixture policies attached to it, by [`policy_arn`] index.
struct AttachCase {
    user_name: &'static str,
    path: &'static str,
    policies: &'static [usize],
}

/// Rows spanning the user path (root, one segment, nested), the user name's case and
/// alphabet, the NUMBER of attachments (none, one, several, all) and the OWNER of the
/// attached policy (this account, AWS) — the dimensions the attachment rule
/// generalises over.
const ATTACH_CASES: &[AttachCase] = &[
    AttachCase {
        user_name: "alice",
        path: "/",
        policies: &[0],
    },
    AttachCase {
        user_name: "Bob.9",
        path: "/app/",
        policies: &[0, 3, 5],
    },
    AttachCase {
        user_name: "carol_7",
        path: "/app/audit/",
        policies: &[1, 2, 3, 4, 5],
    },
    AttachCase {
        user_name: "dave+x=1",
        path: "/sales/west/",
        policies: &[],
    },
    AttachCase {
        user_name: "EVE@4",
        path: "/",
        policies: &[2],
    },
];

/// How many fixture policies there are: the account's own, then AWS's.
fn policy_count() -> usize {
    CUSTOMER_POLICIES.len() + AWS_MANAGED_POLICIES.len()
}

/// The ARN of the fixture policy at `index`: a customer-managed policy's ARN is
/// `arn:aws:iam::<account>:policy<path><name>`, and AWS's own are listed verbatim.
fn policy_arn(index: usize) -> String {
    match CUSTOMER_POLICIES.get(index) {
        Some((name, path)) => format!("arn:aws:iam::{}:policy{path}{name}", account_id()),
        None => AWS_MANAGED_POLICIES[index - CUSTOMER_POLICIES.len()].to_string(),
    }
}

/// Create a user, answering the ARN the account holds it under and the id its create
/// MINTED, so a later read can be checked against the STORED values.
async fn create_user(router: &MultiServiceRouter, user_name: &str, path: &str) -> (String, String) {
    let (status, body) = call(
        router,
        "CreateUser",
        &[("UserName", user_name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {user_name:?} at {path:?}: CreateUser must succeed, got {status}: {body}"
    );
    (element(&body, "Arn"), element(&body, "UserId"))
}

/// Create a customer-managed policy carrying `document`, answering its ARN.
async fn create_policy(
    router: &MultiServiceRouter,
    policy_name: &str,
    path: &str,
    document: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreatePolicy",
        &[
            ("PolicyName", policy_name),
            ("Path", path),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?} at {path:?}: CreatePolicy must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// One `AttachUserPolicy` call.
async fn attach(router: &MultiServiceRouter, user_name: &str, policy_arn: &str) -> (u16, String) {
    call(
        router,
        "AttachUserPolicy",
        &[("UserName", user_name), ("PolicyArn", policy_arn)],
    )
    .await
}

/// One `ListEntitiesForPolicy` call — the reporting half of an attachment — with any
/// filtering members the row exercises.
async fn entities(
    router: &MultiServiceRouter,
    policy_arn: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut params = vec![("PolicyArn", policy_arn)];
    params.extend_from_slice(extra);
    call(router, "ListEntitiesForPolicy", &params).await
}

/// The users `ListEntitiesForPolicy` reports for `policy_arn`.
async fn attached_users(
    router: &MultiServiceRouter,
    policy_arn: &str,
    extra: &[(&str, &str)],
) -> Vec<String> {
    let (status, body) = entities(router, policy_arn, extra).await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?} with {extra:?}: expected 200, got {status}: {body}"
    );
    elements(&body, "UserName")
}

/// The `AttachmentCount` `GetPolicy` reports for `policy_arn` — "the number of
/// entities (users, groups, and roles) that the policy is attached to".
async fn attachment_count(router: &MultiServiceRouter, policy_arn: &str) -> String {
    let (status, body) = call(router, "GetPolicy", &[("PolicyArn", policy_arn)]).await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?}: GetPolicy must succeed, got {status}: {body}"
    );
    element(&body, "AttachmentCount")
}

/// The fixture users the policy at `index` must be reported as attached to, in the
/// order the service enumerates them: ascending by the case-folded name, which is the
/// key the user store is ordered on.
///
/// This is the rule restated over the fixture table: a user is related to a policy
/// exactly when that policy is among the ones ATTACHED to it.
fn expected_users(index: usize) -> Vec<String> {
    let mut names: Vec<String> = ATTACH_CASES
        .iter()
        .filter(|case| case.policies.contains(&index))
        .map(|case| case.user_name.to_string())
        .collect();
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names
}

/// RULE: `AttachUserPolicy` records the managed policy its `PolicyArn` names on the
/// user its `UserName` names, and every reader of that state reports exactly the
/// recorded set — the entities the policy relates, and the policy's own attachment
/// count — so one user's attachments never show up on another's.
#[tokio::test]
async fn attach_user_policy_persists_each_attachment_and_the_policy_readers_report_it() {
    let router = router();
    for (name, path) in CUSTOMER_POLICIES {
        create_policy(&router, name, path, POLICY_DOCUMENT).await;
    }
    let mut user_ids: Vec<(&str, String)> = Vec::new();
    for case in ATTACH_CASES {
        let (_, user_id) = create_user(&router, case.user_name, case.path).await;
        user_ids.push((case.user_name, user_id));
    }

    for case in ATTACH_CASES {
        for index in case.policies {
            let label = format!("{} <- {}", case.user_name, policy_arn(*index));
            let (status, body) = attach(&router, case.user_name, &policy_arn(*index)).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200, got {status}: {body}"
            );
            assert!(
                body.contains("AttachUserPolicyResponse"),
                "case {label:?}: expected the modelled response envelope, got {body}"
            );
        }
    }

    for index in 0..policy_count() {
        let arn = policy_arn(index);
        let expected = expected_users(index);
        let (status, body) = entities(&router, &arn, &[]).await;
        assert_eq!(status, 200, "case {arn:?}: expected 200, got {status}: {body}");

        let reported = elements(&body, "UserName");
        assert_eq!(
            reported, expected,
            "case {arn:?}: expected the attached users {expected:?}, got {reported:?}"
        );
        // Each user is reported under the id its create MINTED, so what comes back is
        // the stored record rather than a value recomputed while reading.
        for (user, id) in reported.iter().zip(elements(&body, "UserId")) {
            let stored = user_ids
                .iter()
                .find(|(name, _)| *name == user.as_str())
                .map(|(_, id)| id.clone());
            assert_eq!(
                Some(&id),
                stored.as_ref(),
                "case {arn:?}: expected user {user:?} under its stored UserId {stored:?}, \
                 got {id:?}"
            );
        }
        assert!(
            elements(&body, "RoleName").is_empty(),
            "case {arn:?}: no role was attached, so none may be reported: {body}"
        );

        // The same set under the filters that select it: naming the user entity type
        // or the permissions-policy usage keeps every attachment, while the
        // permissions-boundary usage selects none — an attachment is not a boundary.
        for filter in [
            vec![("EntityFilter", "User")],
            vec![("PolicyUsageFilter", "PermissionsPolicy")],
            vec![("EntityFilter", "User"), ("PolicyUsageFilter", "PermissionsPolicy")],
        ] {
            let filtered = attached_users(&router, &arn, &filter).await;
            assert_eq!(
                filtered, expected,
                "case {arn:?} with {filter:?}: expected {expected:?}, got {filtered:?}"
            );
        }
        let bounded = attached_users(
            &router,
            &arn,
            &[("PolicyUsageFilter", "PermissionsBoundary")],
        )
        .await;
        assert!(
            bounded.is_empty(),
            "case {arn:?}: an attachment is not a permissions boundary, got {bounded:?}"
        );

        // A read must not change what a later caller sees.
        let again = attached_users(&router, &arn, &[]).await;
        assert_eq!(
            again, expected,
            "case {arn:?}: a read changed what the next caller sees, got {again:?}"
        );

        // The account's own policies are readable through `GetPolicy`, which counts
        // the attachments that exist at the instant it is asked.
        if index < CUSTOMER_POLICIES.len() {
            let count = attachment_count(&router, &arn).await;
            assert_eq!(
                count,
                expected.len().to_string(),
                "case {arn:?}: expected AttachmentCount {}, got {count:?}",
                expected.len()
            );
        }
    }
}

/// RULE: a managed policy is either attached to a user or it is not, so attaching one
/// that is already attached leaves the user's attachment set exactly as it was — and
/// IAM entity names are not distinguished by case, so the casing of the user name a
/// request uses does not pick out a different user.
#[tokio::test]
async fn attaching_the_same_policy_again_leaves_one_attachment() {
    let router = router();
    let user = "IdempotentUser";
    create_user(&router, user, "/eng/").await;
    let held = create_policy(&router, "Idempotent", "/eng/", POLICY_DOCUMENT).await;

    // The rows differ in the casing the request names the user with, which is what
    // the case-folding rule generalises over, and in the owner of the policy attached.
    let spellings = [user, "idempotentuser", "IDEMPOTENTUSER", "IdEmPoTeNtUsEr"];
    let policies = [held.as_str(), AWS_MANAGED_POLICIES[0]];
    for policy in policies {
        for spelling in spellings {
            let label = format!("{spelling} <- {policy}");
            let (status, body) = attach(&router, spelling, policy).await;
            assert_eq!(
                status, 200,
                "case {label:?}: a repeat attach succeeds, got {status}: {body}"
            );

            let reported = attached_users(&router, policy, &[]).await;
            assert_eq!(
                reported,
                vec![user.to_string()],
                "case {label:?}: expected the user attached exactly once, got {reported:?}"
            );
        }
    }

    let count = attachment_count(&router, &held).await;
    assert_eq!(
        count, "1",
        "expected one attachment however often it was attached, got {count:?}"
    );
}

/// RULE: an operation naming a user the account holds no user under "referenced a
/// resource entity that does not exist" — 404 `NoSuchEntity` — and it persists
/// nothing.
#[tokio::test]
async fn attaching_to_a_user_the_account_does_not_hold_is_refused() {
    let router = router();
    create_user(&router, "HeldUser", "/").await;
    let held = create_policy(&router, "HeldPolicy", "/", POLICY_DOCUMENT).await;

    // Rows spanning a name no user carries, a name that differs from a held one, and
    // the single-character and 64-character bounds of `userNameType` (len 1..64).
    let longest = "U".repeat(64);
    let unheld = ["MissingUser", "HeldUser2", "u", longest.as_str()];
    for user_name in unheld {
        let (status, body) = attach(&router, user_name, &held).await;
        assert_eq!(
            status, 404,
            "case {user_name:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {user_name:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
    }

    let reported = attached_users(&router, &held, &[]).await;
    assert!(
        reported.is_empty(),
        "a refused attach must persist nothing, got {reported:?}"
    );
}

/// RULE: `PolicyArn` must BE a managed policy's ARN —
/// `arn:<partition>:iam::<account>:policy/<path><name>`, with an empty region segment
/// because IAM is global — and a value that is not one is refused 400, persisting
/// nothing. Which code it carries is the layer that took the form: a value that is not
/// an ARN at all is refused with the code IAM answers a wrong FORM with
/// (`ValidationError`), while an ARN naming another service or resource type is this
/// operation's own modelled refusal (`InvalidInput`); both carry IAM's one sentence for
/// an unusable ARN.
#[tokio::test]
async fn attaching_an_arn_that_is_not_a_managed_policy_arn_is_refused() {
    let router = router();
    let user = "ArnValidationUser";
    create_user(&router, user, "/").await;
    let held = create_policy(&router, "ArnValidation", "/", POLICY_DOCUMENT).await;
    assert_eq!(
        attach(&router, user, &held).await.0,
        200,
        "the fixture attachment must succeed"
    );

    // Each row is at least 20 characters, `arnType`'s minimum length, so the handler —
    // not the modelled length constraint — is what refuses it. The rows break one part
    // of the grammar each, and span both layers.
    let malformed = [
        ("this-is-not-an-arn-at-all", "ValidationError"),
        ("arn:aws:s3:::a-bucket-named-like-this", "InvalidInput"),
        ("arn:aws:iam::aws:role/NotAPolicyAtAll", "InvalidInput"),
        ("arn:aws:iam::aws:user/SomeUserNamedHere", "InvalidInput"),
        ("arn:aws:iam:us-east-1:aws:policy/RegionIsSet", "InvalidInput"),
        ("arn::iam::aws:policy/PartitionIsEmpty", "InvalidInput"),
        ("arn:aws:iam:::policy/AccountIsEmptyHere", "InvalidInput"),
        ("arn:aws:iam::aws:policy/service-role/", "InvalidInput"),
        ("arn:aws:iam::aws:policy//EmptySegment", "InvalidInput"),
        ("aws:iam::aws:policy/MissingArnPrefixHere", "ValidationError"),
    ];
    for (policy_arn, expected_code) in malformed {
        assert!(
            policy_arn.len() >= 20,
            "case {policy_arn:?}: a row must satisfy arnType's 20-character minimum"
        );
        let (status, body) = attach(&router, user, policy_arn).await;
        assert_eq!(
            status, 400,
            "case {policy_arn:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {policy_arn:?}: expected {expected_code}, got {:?}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            format!("ARN {policy_arn} is not valid."),
            "case {policy_arn:?}: expected IAM's unusable-ARN sentence, got {:?}",
            element(&body, "Message")
        );
    }

    // A refused attach writes nothing: the user still carries only the fixture.
    let count = attachment_count(&router, &held).await;
    assert_eq!(
        count, "1",
        "a refused attach must leave the fixture attachment alone, got {count:?}"
    );
}

/// RULE: a customer-managed policy exists only where the account holds it, so an
/// account-owned policy ARN naming no held policy refers to no entity — 404
/// `NoSuchEntity`.
#[tokio::test]
async fn attaching_a_customer_managed_policy_the_account_does_not_hold_is_refused() {
    let router = router();
    let user = "CustomerPolicyUser";
    create_user(&router, user, "/").await;
    let held = create_policy(&router, "HeldByTheAccount", "/", POLICY_DOCUMENT).await;

    // Rows spanning the policy's path depth and the owning account (this account and
    // another one), since neither holds a policy here.
    let unheld = [
        format!("arn:aws:iam::{}:policy/NeverCreated", account_id()),
        format!("arn:aws:iam::{}:policy/team/NeverCreated", account_id()),
        format!("arn:aws:iam::{}:policy/a/b/DeeplyPathed", account_id()),
        "arn:aws:iam::210987654321:policy/OtherAccountPolicy".to_string(),
    ];
    for policy_arn in &unheld {
        let (status, body) = attach(&router, user, policy_arn).await;
        assert_eq!(
            status, 404,
            "case {policy_arn:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {policy_arn:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
    }

    let count = attachment_count(&router, &held).await;
    assert_eq!(
        count, "0",
        "a refused attach must persist nothing, got {count:?}"
    );
}

/// RULE: a policy AWS files under the reserved service-linked role path is a service
/// role policy, which "can only be attached to the service-linked role for that
/// service" — 400 `PolicyNotAttachable` for a user, which is no service-linked role.
#[tokio::test]
async fn attaching_a_service_role_policy_to_a_user_is_refused() {
    let router = router();
    let user = "OrdinaryUser";
    create_user(&router, user, "/").await;

    // Rows spanning the service the policy belongs to and the depth below the
    // reserved path.
    let service_role_policies = [
        "arn:aws:iam::aws:policy/aws-service-role/AWSLambdaRole",
        "arn:aws:iam::aws:policy/aws-service-role/AmazonEKSServiceRolePolicy",
        "arn:aws:iam::aws:policy/aws-service-role/elasticbeanstalk/ServicePolicy",
    ];
    for policy_arn in service_role_policies {
        let (status, body) = attach(&router, user, policy_arn).await;
        assert_eq!(
            status, 400,
            "case {policy_arn:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "PolicyNotAttachable",
            "case {policy_arn:?}: expected PolicyNotAttachable, got {:?}",
            error_code(&body)
        );

        let reported = attached_users(&router, policy_arn, &[]).await;
        assert!(
            reported.is_empty(),
            "case {policy_arn:?}: a refused attach persists nothing, got {reported:?}"
        );
    }
}

/// RULE: a user carries a bounded number of managed policies, so an attach that would
/// carry it past the quota is refused with 409 `LimitExceeded` while a repeat attach
/// of a policy already attached stays a no-op — it consumes no slot.
#[tokio::test]
async fn attaching_past_the_user_quota_is_refused() {
    let router = router();
    let user = "QuotaUser";
    create_user(&router, user, "/").await;

    // Distinct well-formed AWS-managed policy ARNs, one per slot the quota allows,
    // plus one more to take the user past it.
    let filler: Vec<String> = (1..=MAX_MANAGED_POLICIES_PER_USER + 1)
        .map(|index| format!("arn:aws:iam::aws:policy/UserQuotaFiller{index:02}"))
        .collect();

    for policy_arn in filler.iter().take(MAX_MANAGED_POLICIES_PER_USER) {
        let (status, body) = attach(&router, user, policy_arn).await;
        assert_eq!(
            status, 200,
            "case {policy_arn:?}: an attach inside the quota succeeds, got {status}: {body}"
        );
    }

    // Already attached: still a success, and still one attachment — so it leaves a
    // slot free for nothing.
    let (status, body) = attach(&router, user, &filler[0]).await;
    assert_eq!(
        status, 200,
        "case {:?}: a repeat attach at the quota succeeds, got {status}: {body}",
        filler[0]
    );

    let past = &filler[MAX_MANAGED_POLICIES_PER_USER];
    let (status, body) = attach(&router, user, past).await;
    assert_eq!(
        status, 409,
        "case {past:?}: expected 409 past the quota, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceeded",
        "case {past:?}: expected LimitExceeded, got {:?}",
        error_code(&body)
    );

    // Exactly the attachments the quota allows were persisted: each of those policies
    // relates the user, and the one past the quota relates nothing.
    for policy_arn in filler.iter().take(MAX_MANAGED_POLICIES_PER_USER) {
        let reported = attached_users(&router, policy_arn, &[]).await;
        assert_eq!(
            reported,
            vec![user.to_string()],
            "case {policy_arn:?}: expected the user attached, got {reported:?}"
        );
    }
    let reported = attached_users(&router, past, &[]).await;
    assert!(
        reported.is_empty(),
        "case {past:?}: a refused attach persists nothing, got {reported:?}"
    );
}

/// RULE: IAM refuses to delete an entity that still carries a dependent, and an
/// attached managed policy is one of them — "Attached managed policies
/// (DetachUserPolicy)" is among the items `DeleteUser` documents removing first — so
/// the delete is 409 `DeleteConflict` and the user survives it.
#[tokio::test]
async fn deleting_a_user_that_still_carries_an_attached_policy_is_refused() {
    let router = router();
    let held = create_policy(&router, "StillAttached", "/", POLICY_DOCUMENT).await;

    // Rows spanning the user's path and the owner of the policy it carries.
    let cases = [
        ("DeleteMeRoot", "/", held.as_str()),
        ("delete_me.nested", "/app/team/", AWS_MANAGED_POLICIES[0]),
        ("DELETE-ME=2", "/app/", AWS_MANAGED_POLICIES[2]),
    ];
    for (user_name, path, policy_arn) in cases {
        create_user(&router, user_name, path).await;
        assert_eq!(
            attach(&router, user_name, policy_arn).await.0,
            200,
            "case {user_name:?}: the fixture attachment must succeed"
        );

        let (status, body) = call(&router, "DeleteUser", &[("UserName", user_name)]).await;
        assert_eq!(
            status, 409,
            "case {user_name:?}: expected 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "DeleteConflict",
            "case {user_name:?}: expected DeleteConflict, got {:?}",
            error_code(&body)
        );

        // The refused delete left the user and its attachment exactly as they were.
        let (status, body) = call(&router, "GetUser", &[("UserName", user_name)]).await;
        assert_eq!(
            status, 200,
            "case {user_name:?}: the user must survive a refused delete, got {status}: {body}"
        );
        let reported = attached_users(&router, policy_arn, &[]).await;
        assert!(
            reported.iter().any(|name| name == user_name),
            "case {user_name:?}: the attachment must survive a refused delete, got {reported:?}"
        );
    }
}

/// RULE: the identity-based policies a user carries include the DEFAULT version of
/// every managed policy attached to it, so a reader of the user's policies — the
/// context keys they reference — answers from the attachments the store holds.
#[tokio::test]
async fn an_attached_policys_condition_keys_are_reported_for_the_user() {
    let router = router();

    // Rows spanning how many attached policies carry a condition (none, one, two) and
    // how many keys each one tests. The key names are AWS global condition context
    // keys (IAM User Guide, "AWS global condition context keys"); what is under test
    // is which keys the attached documents REFERENCE, not which keys exist.
    let documents: &[(&str, &str, &[&str])] = &[
        (
            "SourceIpBound",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"*","Condition":{"IpAddress":{"aws:SourceIp":"10.0.0.0/8"}}}]}"#,
            &["aws:SourceIp"],
        ),
        (
            "MfaAndTimeBound",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"iam:*","Resource":"*","Condition":{"Bool":{"aws:MultiFactorAuthPresent":"false"},"DateGreaterThan":{"aws:CurrentTime":"2026-01-01T00:00:00Z"}}}]}"#,
            &["aws:MultiFactorAuthPresent", "aws:CurrentTime"],
        ),
    ];

    let (user_arn, _) = create_user(&router, "ContextKeyUser", "/eng/").await;

    let mut expected: Vec<String> = Vec::new();
    // Before any attachment: the user's policies reference no key at all.
    let (status, body) = call(
        &router,
        "GetContextKeysForPrincipalPolicy",
        &[("PolicySourceArn", &user_arn)],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert!(
        elements(&body, "member").is_empty(),
        "case \"no attachment\": expected no context key, got {:?}",
        elements(&body, "member")
    );

    for (policy_name, document, keys) in documents {
        let arn = create_policy(&router, policy_name, "/eng/", document).await;
        assert_eq!(
            attach(&router, "ContextKeyUser", &arn).await.0,
            200,
            "case {policy_name:?}: the fixture attachment must succeed"
        );
        expected.extend(keys.iter().map(|key| (*key).to_string()));
        expected.sort();

        let (status, body) = call(
            &router,
            "GetContextKeysForPrincipalPolicy",
            &[("PolicySourceArn", &user_arn)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {policy_name:?}: expected 200, got {status}: {body}"
        );
        let mut reported = elements(&body, "member");
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {policy_name:?}: expected the context keys {expected:?}, got {reported:?}"
        );
    }
}
