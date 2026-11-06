// SPDX-License-Identifier: CC0-1.0

//! `AttachGroupPolicy` — attaching a managed policy to an IAM group — end to end
//! over the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one
//! shared `InMemoryEngine`, exactly as the production router does, so what is under
//! test is the HTTP behaviour (status, awsQuery error code, XML members) of an
//! attachment that was PERSISTED, never a handler called directly. The attachment is
//! read back through the implemented readers of the same state: the entities a policy
//! relates (`ListEntitiesForPolicy`), the policy's own `AttachmentCount`
//! (`GetPolicy`), the group's record in the account's configuration
//! (`GetAccountAuthorizationDetails`), what the group's policies decide
//! (`SimulatePrincipalPolicy`) and the dependents a group delete refuses on
//! (`DeleteGroup`). The awsQuery harness is the one `tests/all/user_lifecycle.rs`
//! owns and the other suites in this crate share; the simulation request spelling is
//! the one `tests/all/policy_simulation.rs` owns.
//!
//! The suites are TABLES: each row differs along what the rule being asserted
//! generalises over (the policy's owner and path, the group's path and name case, the
//! number of attachments, the entity and usage filter, the form of a refused ARN or
//! name), so no single recorded request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::policy_simulation::{reported, simulate, ALLOWED, EXPLICIT_DENY, IMPLICIT_DENY};
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

/// How many managed policies one group may carry: AWS's "IAM and STS quotas" page
/// gives "Managed policies attached to an IAM group: 10", which is the quota
/// `crate::state::MAX_MANAGED_POLICIES_PER_GROUP` records.
const MAX_MANAGED_POLICIES_PER_GROUP: usize = 10;

/// One fixture group and the fixture policies attached to it, by [`policy_arn`] index.
struct AttachCase {
    group_name: &'static str,
    path: &'static str,
    policies: &'static [usize],
}

/// Rows spanning the group path (root, one segment, nested), the group name's case and
/// alphabet, the NUMBER of attachments (none, one, several, all) and the OWNER of the
/// attached policy (this account, AWS) — the dimensions the attachment rule
/// generalises over.
const ATTACH_CASES: &[AttachCase] = &[
    AttachCase {
        group_name: "admins",
        path: "/",
        policies: &[0],
    },
    AttachCase {
        group_name: "Auditors.9",
        path: "/app/",
        policies: &[0, 3, 5],
    },
    AttachCase {
        group_name: "readers_7",
        path: "/app/audit/",
        policies: &[1, 2, 3, 4, 5],
    },
    AttachCase {
        group_name: "writers+x=1",
        path: "/sales/west/",
        policies: &[],
    },
    AttachCase {
        group_name: "OPERATORS@4",
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

/// Create a group, answering the ARN the account holds it under and the id its create
/// MINTED, so a later read can be checked against the STORED values.
async fn create_group(
    router: &MultiServiceRouter,
    group_name: &str,
    path: &str,
) -> (String, String) {
    let (status, body) = call(
        router,
        "CreateGroup",
        &[("GroupName", group_name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {group_name:?} at {path:?}: CreateGroup must succeed, got {status}: {body}"
    );
    (element(&body, "Arn"), element(&body, "GroupId"))
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

/// One `AttachGroupPolicy` call.
async fn attach(router: &MultiServiceRouter, group_name: &str, policy_arn: &str) -> (u16, String) {
    call(
        router,
        "AttachGroupPolicy",
        &[("GroupName", group_name), ("PolicyArn", policy_arn)],
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

/// The groups `ListEntitiesForPolicy` reports for `policy_arn`.
async fn attached_groups(
    router: &MultiServiceRouter,
    policy_arn: &str,
    extra: &[(&str, &str)],
) -> Vec<String> {
    let (status, body) = entities(router, policy_arn, extra).await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?} with {extra:?}: expected 200, got {status}: {body}"
    );
    elements(&body, "GroupName")
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

/// The fixture groups the policy at `index` must be reported as attached to, in the
/// order the service enumerates them: ascending by the case-folded name, which is the
/// key the group store is ordered on.
///
/// This is the rule restated over the fixture table: a group is related to a policy
/// exactly when that policy is among the ones ATTACHED to it.
fn expected_groups(index: usize) -> Vec<String> {
    let mut names: Vec<String> = ATTACH_CASES
        .iter()
        .filter(|case| case.policies.contains(&index))
        .map(|case| case.group_name.to_string())
        .collect();
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names
}

/// RULE: `AttachGroupPolicy` records the managed policy its `PolicyArn` names on the
/// group its `GroupName` names, and every reader of that state reports exactly the
/// recorded set — the entities the policy relates, and the policy's own attachment
/// count — so one group's attachments never show up on another's.
#[tokio::test]
async fn attach_group_policy_persists_each_attachment_and_the_policy_readers_report_it() {
    let router = router();
    for (name, path) in CUSTOMER_POLICIES {
        create_policy(&router, name, path, POLICY_DOCUMENT).await;
    }
    let mut group_ids: Vec<(&str, String)> = Vec::new();
    for case in ATTACH_CASES {
        let (_, group_id) = create_group(&router, case.group_name, case.path).await;
        group_ids.push((case.group_name, group_id));
    }

    for case in ATTACH_CASES {
        for index in case.policies {
            let label = format!("{} <- {}", case.group_name, policy_arn(*index));
            let (status, body) = attach(&router, case.group_name, &policy_arn(*index)).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200, got {status}: {body}"
            );
            assert!(
                body.contains("AttachGroupPolicyResponse"),
                "case {label:?}: expected the modelled response envelope, got {body}"
            );
        }
    }

    for index in 0..policy_count() {
        let arn = policy_arn(index);
        let expected = expected_groups(index);
        let (status, body) = entities(&router, &arn, &[]).await;
        assert_eq!(status, 200, "case {arn:?}: expected 200, got {status}: {body}");

        let reported = elements(&body, "GroupName");
        assert_eq!(
            reported, expected,
            "case {arn:?}: expected the attached groups {expected:?}, got {reported:?}"
        );
        // Each group is reported under the id its create MINTED, so what comes back is
        // the stored record rather than a value recomputed while reading.
        for (group, id) in reported.iter().zip(elements(&body, "GroupId")) {
            let stored = group_ids
                .iter()
                .find(|(name, _)| *name == group.as_str())
                .map(|(_, id)| id.clone());
            assert_eq!(
                Some(&id),
                stored.as_ref(),
                "case {arn:?}: expected group {group:?} under its stored GroupId {stored:?}, \
                 got {id:?}"
            );
        }
        for other in ["RoleName", "UserName"] {
            assert!(
                elements(&body, other).is_empty(),
                "case {arn:?}: no {other} was attached, so none may be reported: {body}"
            );
        }

        // The same set under the filters that select it: naming the group entity type
        // or the permissions-policy usage keeps every attachment, while the
        // permissions-boundary usage selects none — an attachment is not a boundary,
        // and a group carries none at all.
        for filter in [
            vec![("EntityFilter", "Group")],
            vec![("PolicyUsageFilter", "PermissionsPolicy")],
            vec![
                ("EntityFilter", "Group"),
                ("PolicyUsageFilter", "PermissionsPolicy"),
            ],
        ] {
            let filtered = attached_groups(&router, &arn, &filter).await;
            assert_eq!(
                filtered, expected,
                "case {arn:?} with {filter:?}: expected {expected:?}, got {filtered:?}"
            );
        }
        for filter in [
            vec![("PolicyUsageFilter", "PermissionsBoundary")],
            vec![("EntityFilter", "User")],
            vec![("EntityFilter", "Role")],
        ] {
            let selected = attached_groups(&router, &arn, &filter).await;
            assert!(
                selected.is_empty(),
                "case {arn:?} with {filter:?}: expected no group, got {selected:?}"
            );
        }

        // A read must not change what a later caller sees.
        let again = attached_groups(&router, &arn, &[]).await;
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

/// RULE: a managed policy is either attached to a group or it is not, so attaching one
/// that is already attached leaves the group's attachment set exactly as it was — and
/// IAM entity names are not distinguished by case, so the casing of the group name a
/// request uses does not pick out a different group.
#[tokio::test]
async fn attaching_the_same_policy_again_leaves_one_attachment() {
    let router = router();
    let group = "IdempotentGroup";
    create_group(&router, group, "/eng/").await;
    let held = create_policy(&router, "Idempotent", "/eng/", POLICY_DOCUMENT).await;

    // The rows differ in the casing the request names the group with, which is what
    // the case-folding rule generalises over, and in the owner of the policy attached.
    let spellings = [group, "idempotentgroup", "IDEMPOTENTGROUP", "IdEmPoTeNtGrOuP"];
    let policies = [held.as_str(), AWS_MANAGED_POLICIES[0]];
    for policy in policies {
        for spelling in spellings {
            let label = format!("{spelling} <- {policy}");
            let (status, body) = attach(&router, spelling, policy).await;
            assert_eq!(
                status, 200,
                "case {label:?}: a repeat attach succeeds, got {status}: {body}"
            );

            let reported = attached_groups(&router, policy, &[]).await;
            assert_eq!(
                reported,
                vec![group.to_string()],
                "case {label:?}: expected the group attached exactly once, got {reported:?}"
            );
        }
    }

    let count = attachment_count(&router, &held).await;
    assert_eq!(
        count, "1",
        "expected one attachment however often it was attached, got {count:?}"
    );
}

/// RULE: a group's record in the account's configuration carries the managed policies
/// attached to it — "a list of the managed policies attached to the group"
/// (`GroupDetail$AttachedManagedPolicies`) — so the attachment is readable through the
/// configuration report as well as through the policy's own entity listing.
#[tokio::test]
async fn the_account_configuration_report_carries_each_groups_attachments() {
    let router = router();
    let held = create_policy(&router, "ReportedPolicy", "/", POLICY_DOCUMENT).await;

    // Rows spanning the number of attachments a group carries (two, one, none) and
    // the owner of each attached policy. The groups are named so that the case-folded
    // order the store enumerates them in is the order of the rows.
    let cases: [(&str, Vec<&str>); 3] = [
        ("a-two", vec![held.as_str(), AWS_MANAGED_POLICIES[0]]),
        ("B_one", vec![AWS_MANAGED_POLICIES[2]]),
        ("c.none", Vec::new()),
    ];
    for (group_name, attachments) in &cases {
        create_group(&router, group_name, "/").await;
        for policy in attachments {
            assert_eq!(
                attach(&router, group_name, policy).await.0,
                200,
                "case {group_name:?}: the fixture attachment {policy:?} must succeed"
            );
        }
    }

    let (status, body) = call(
        &router,
        "GetAccountAuthorizationDetails",
        &[("Filter.member.1", "Group")],
    )
    .await;
    assert_eq!(
        status, 200,
        "GetAccountAuthorizationDetails must succeed, got {status}: {body}"
    );

    // Only the groups are reported, and a group carries an attachment's ARN nowhere
    // but in its attached-policy list, so the ARNs the report carries are the
    // attachments themselves — each group's ascending by ARN, the groups in the
    // case-folded name order the store is keyed on.
    let mut expected: Vec<String> = Vec::new();
    for (_, attachments) in &cases {
        let mut arns: Vec<String> = attachments.iter().map(|arn| (*arn).to_string()).collect();
        arns.sort();
        expected.extend(arns);
    }
    let found = elements(&body, "PolicyArn");
    assert_eq!(
        found, expected,
        "expected the stored attachments {expected:?} in the report, got {found:?}"
    );
}

/// RULE: the identity-based policies in force on a group include the DEFAULT version
/// of every managed policy attached to it, so what a simulation of the group decides
/// comes from the attachment the store holds — and before anything is attached nothing
/// is in force.
#[tokio::test]
async fn an_attached_policy_decides_a_simulation_of_the_group() {
    let router = router();
    // One `Allow` over a service's whole action namespace beside a `Deny` naming one
    // action within it, so a single request reaches all three decisions
    // `PolicyEvaluationDecisionType` declares.
    const PERMISSIONS: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"lakeformation:*","Resource":"*"},{"Effect":"Deny","Action":"lakeformation:PutDataLakeSettings","Resource":"*"}]}"#;
    // Rows spanning the three decisions: the wildcard `Allow` covers the first two,
    // the `Deny` names the third, and no statement is about the fourth.
    let decided: &[(&str, &str)] = &[
        ("lakeformation:GetDataAccess", ALLOWED),
        ("lakeformation:GetDataLakeSettings", ALLOWED),
        ("lakeformation:PutDataLakeSettings", EXPLICIT_DENY),
        ("s3:GetObject", IMPLICIT_DENY),
    ];
    let actions: Vec<&str> = decided.iter().map(|(action, _)| *action).collect();

    let (group_arn, _) = create_group(&router, "simulated+group", "/eng/").await;
    let policy = create_policy(&router, "GroupPermissions", "/eng/", PERMISSIONS).await;

    // Before the attach the group carries nothing, so no action is allowed.
    let (status, body) = simulate(&router, &group_arn, &actions, &[], &[]).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    let before = reported(&body);
    let unattached: Vec<(String, String)> = decided
        .iter()
        .map(|(action, _)| ((*action).to_string(), IMPLICIT_DENY.to_string()))
        .collect();
    assert_eq!(
        before, unattached,
        "case \"no attachment\": expected {unattached:?}, got {before:?}"
    );

    assert_eq!(
        attach(&router, "simulated+group", &policy).await.0,
        200,
        "the fixture attachment must succeed"
    );

    let expected: Vec<(String, String)> = decided
        .iter()
        .map(|(action, decision)| ((*action).to_string(), (*decision).to_string()))
        .collect();
    let (status, body) = simulate(&router, &group_arn, &actions, &[], &[]).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    let after = reported(&body);
    assert_eq!(
        after, expected,
        "case \"attached\": expected {expected:?}, got {after:?}"
    );
}

/// RULE: an operation naming a group the account holds no group under "referenced a
/// resource entity that does not exist" — 404 `NoSuchEntity` — and it persists
/// nothing.
#[tokio::test]
async fn attaching_to_a_group_the_account_does_not_hold_is_refused() {
    let router = router();
    create_group(&router, "HeldGroup", "/").await;
    let held = create_policy(&router, "HeldPolicy", "/", POLICY_DOCUMENT).await;

    // Rows spanning a name no group carries, a name that differs from a held one, and
    // the single-character and 128-character bounds of `groupNameType` (len 1..128).
    let longest = "G".repeat(128);
    let unheld = ["MissingGroup", "HeldGroup2", "g", longest.as_str()];
    for group_name in unheld {
        let (status, body) = attach(&router, group_name, &held).await;
        assert_eq!(
            status, 404,
            "case {group_name:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {group_name:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
    }

    let reported = attached_groups(&router, &held, &[]).await;
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
    let group = "ArnValidationGroup";
    create_group(&router, group, "/").await;
    let held = create_policy(&router, "ArnValidation", "/", POLICY_DOCUMENT).await;
    assert_eq!(
        attach(&router, group, &held).await.0,
        200,
        "the fixture attachment must succeed"
    );

    // Each row is at least 20 characters, `arnType`'s minimum length, so the handler —
    // not the modelled length constraint — is what refuses it. The rows break one part
    // of the grammar each, and span both layers.
    let malformed = [
        ("this-is-not-an-arn-at-all", "ValidationError"),
        ("arn:aws:s3:::a-bucket-named-like-this", "InvalidInput"),
        ("arn:aws:iam::aws:group/NotAPolicyAtAll", "InvalidInput"),
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
        let (status, body) = attach(&router, group, policy_arn).await;
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

    // A refused attach writes nothing: the group still carries only the fixture.
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
    let group = "CustomerPolicyGroup";
    create_group(&router, group, "/").await;
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
        let (status, body) = attach(&router, group, policy_arn).await;
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
/// service" — 400 `PolicyNotAttachable` for a group, which is no service-linked role.
#[tokio::test]
async fn attaching_a_service_role_policy_to_a_group_is_refused() {
    let router = router();
    let group = "OrdinaryGroup";
    create_group(&router, group, "/").await;

    // Rows spanning the service the policy belongs to and the depth below the
    // reserved path.
    let service_role_policies = [
        "arn:aws:iam::aws:policy/aws-service-role/AWSLambdaRole",
        "arn:aws:iam::aws:policy/aws-service-role/AmazonEKSServiceRolePolicy",
        "arn:aws:iam::aws:policy/aws-service-role/elasticbeanstalk/ServicePolicy",
    ];
    for policy_arn in service_role_policies {
        let (status, body) = attach(&router, group, policy_arn).await;
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

        let reported = attached_groups(&router, policy_arn, &[]).await;
        assert!(
            reported.is_empty(),
            "case {policy_arn:?}: a refused attach persists nothing, got {reported:?}"
        );
    }
}

/// RULE: a group carries a bounded number of managed policies, so an attach that would
/// carry it past the quota is refused with 409 `LimitExceeded` while a repeat attach of
/// a policy already attached stays a no-op — it consumes no slot.
#[tokio::test]
async fn attaching_past_the_group_quota_is_refused() {
    let router = router();
    let group = "QuotaGroup";
    create_group(&router, group, "/").await;

    // Distinct well-formed AWS-managed policy ARNs, one per slot the quota allows,
    // plus one more to take the group past it.
    let filler: Vec<String> = (1..=MAX_MANAGED_POLICIES_PER_GROUP + 1)
        .map(|index| format!("arn:aws:iam::aws:policy/GroupQuotaFiller{index:02}"))
        .collect();

    for policy_arn in filler.iter().take(MAX_MANAGED_POLICIES_PER_GROUP) {
        let (status, body) = attach(&router, group, policy_arn).await;
        assert_eq!(
            status, 200,
            "case {policy_arn:?}: an attach inside the quota succeeds, got {status}: {body}"
        );
    }

    // Already attached: still a success, and still one attachment — so it leaves a
    // slot free for nothing.
    let (status, body) = attach(&router, group, &filler[0]).await;
    assert_eq!(
        status, 200,
        "case {:?}: a repeat attach at the quota succeeds, got {status}: {body}",
        filler[0]
    );

    let past = &filler[MAX_MANAGED_POLICIES_PER_GROUP];
    let (status, body) = attach(&router, group, past).await;
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
    // relates the group, and the one past the quota relates nothing.
    for policy_arn in filler.iter().take(MAX_MANAGED_POLICIES_PER_GROUP) {
        let reported = attached_groups(&router, policy_arn, &[]).await;
        assert_eq!(
            reported,
            vec![group.to_string()],
            "case {policy_arn:?}: expected the group attached, got {reported:?}"
        );
    }
    let reported = attached_groups(&router, past, &[]).await;
    assert!(
        reported.is_empty(),
        "case {past:?}: a refused attach persists nothing, got {reported:?}"
    );
}

/// RULE: IAM refuses to delete an entity that still carries a dependent, and an
/// attached managed policy is one of them — "the group must not contain any users or
/// have any attached policies" (`DeleteGroup`) — so the delete is 409 `DeleteConflict`
/// and the group survives it.
#[tokio::test]
async fn deleting_a_group_that_still_carries_an_attached_policy_is_refused() {
    let router = router();
    let held = create_policy(&router, "StillAttached", "/", POLICY_DOCUMENT).await;

    // Rows spanning the group's path and the owner of the policy it carries.
    let cases = [
        ("DeleteMeRoot", "/", held.as_str()),
        ("delete_me.nested", "/app/team/", AWS_MANAGED_POLICIES[0]),
        ("DELETE-ME=2", "/app/", AWS_MANAGED_POLICIES[2]),
    ];
    for (group_name, path, policy_arn) in cases {
        create_group(&router, group_name, path).await;
        assert_eq!(
            attach(&router, group_name, policy_arn).await.0,
            200,
            "case {group_name:?}: the fixture attachment must succeed"
        );

        let (status, body) = call(&router, "DeleteGroup", &[("GroupName", group_name)]).await;
        assert_eq!(
            status, 409,
            "case {group_name:?}: expected 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "DeleteConflict",
            "case {group_name:?}: expected DeleteConflict, got {:?}",
            error_code(&body)
        );

        // The refused delete left the group and its attachment exactly as they were.
        let (status, body) = call(&router, "GetGroup", &[("GroupName", group_name)]).await;
        assert_eq!(
            status, 200,
            "case {group_name:?}: the group must survive a refused delete, got {status}: {body}"
        );
        let reported = attached_groups(&router, policy_arn, &[]).await;
        assert!(
            reported.iter().any(|name| name == group_name),
            "case {group_name:?}: the attachment must survive a refused delete, got {reported:?}"
        );
    }
}
