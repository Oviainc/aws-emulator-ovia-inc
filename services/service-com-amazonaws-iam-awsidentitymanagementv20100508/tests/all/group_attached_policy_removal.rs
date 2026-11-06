// SPDX-License-Identifier: CC0-1.0

//! `DetachGroupPolicy` — removing the attachment of a managed policy from an IAM group
//! — end to end over the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, `Message`, XML members) of a detachment
//! that was PERSISTED, never a handler called directly. Each removal is read back
//! through the implemented readers of the same state: the group's own attachments
//! (`ListAttachedGroupPolicies`), the entities the policy relates
//! (`ListEntitiesForPolicy`), the policy's own `AttachmentCount` (`GetPolicy`) and the
//! dependents a group delete refuses on (`DeleteGroup`). The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and the other suites in this crate share; what
//! `AttachGroupPolicy` itself refuses is asserted by
//! `tests/all/group_attached_policies.rs`, and how the attachments are paged by
//! `tests/all/group_attached_policy_listing.rs`.
//!
//! The suites are TABLES: each row differs along what the rule being asserted
//! generalises over (the group's path, name case and name alphabet, the owner and path
//! of the policy whose attachment is removed, how many of a group's attachments are
//! removed, the entity kind the family's refusal names, the form of a refused ARN or
//! group name), so no single recorded request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// A managed policy document with a single statement — the form `CreatePolicy`'s own
/// modelled example carries. Every customer-managed fixture policy needs one to exist
/// at all.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// A trust policy with a single statement, which the fixture role needs.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"config.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The customer-managed policies the fixture account holds: `(PolicyName, Path)`. The
/// paths span the account root, a one-segment path and a nested path, so a rule about a
/// policy's ARN is exercised at more than one depth.
const CUSTOMER_POLICIES: &[(&str, &str)] = &[
    ("ReadItems", "/"),
    ("WriteItems", "/app/"),
    ("AuditItems", "/app/audit/"),
];

/// ARNs of AWS-managed policies — owned by AWS rather than by the account, so the
/// account holds no record of them. The rows span the paths the managed-policy ARN
/// grammar admits: the account root and the `/job-function/` path. The names and paths
/// are entries of the AWS Managed Policy Reference; what the rule generalises over is
/// the ARN's SHAPE (path depth, name alphabet), which is why the rows differ in path
/// rather than only in name.
const AWS_MANAGED_POLICIES: &[&str] = &[
    "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess",
    "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess",
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

/// The sentence this service words the absence of an ATTACHMENT with: the policy ARN
/// exactly as the request supplied it, and nothing of the entity the attachment was asked
/// for on.
///
/// What a detach addresses is the POLICY, so IAM's `NoSuchEntity` message is the one it
/// words a `PolicyArn` it cannot serve from with — `state::policy_not_found` — and the
/// entity name appears nowhere in it. The sentence belongs to the family rather than to
/// one handler, which is why
/// [`detaching_a_policy_the_entity_does_not_carry_is_refused`] drives all three detach
/// operations through it: a wording one of them composes for itself is how the family's
/// text drifts for a single entity kind.
fn not_attached_message(policy_arn: &str) -> String {
    format!("Policy {policy_arn} was not found.")
}

/// Create a group at `path`.
async fn create_group(router: &MultiServiceRouter, group_name: &str, path: &str) {
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
}

/// Create a customer-managed policy carrying [`POLICY_DOCUMENT`], answering its ARN.
async fn create_policy(router: &MultiServiceRouter, policy_name: &str, path: &str) -> String {
    let (status, body) = call(
        router,
        "CreatePolicy",
        &[
            ("PolicyName", policy_name),
            ("Path", path),
            ("PolicyDocument", POLICY_DOCUMENT),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?} at {path:?}: CreatePolicy must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Create every fixture policy the account owns, so each one is attachable.
async fn create_customer_policies(router: &MultiServiceRouter) {
    for (name, path) in CUSTOMER_POLICIES {
        create_policy(router, name, path).await;
    }
}

/// One `AttachGroupPolicy` call that must succeed, which is how every row of this suite
/// gets the attachment it then removes.
async fn attach_ok(router: &MultiServiceRouter, group_name: &str, policy_arn: &str) {
    let (status, body) = call(
        router,
        "AttachGroupPolicy",
        &[("GroupName", group_name), ("PolicyArn", policy_arn)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {group_name:?} <- {policy_arn:?}: the fixture attach must succeed, got \
         {status}: {body}"
    );
}

/// One `DetachGroupPolicy` call.
async fn detach(router: &MultiServiceRouter, group_name: &str, policy_arn: &str) -> (u16, String) {
    call(
        router,
        "DetachGroupPolicy",
        &[("GroupName", group_name), ("PolicyArn", policy_arn)],
    )
    .await
}

/// One `DetachGroupPolicy` call that must succeed, answering nothing but the modelled
/// envelope.
async fn detach_ok(router: &MultiServiceRouter, group_name: &str, policy_arn: &str) {
    let (status, body) = detach(router, group_name, policy_arn).await;
    assert_eq!(
        status, 200,
        "case {group_name:?} -> {policy_arn:?}: expected 200, got {status}: {body}"
    );
    assert!(
        body.contains("DetachGroupPolicyResponse"),
        "case {group_name:?} -> {policy_arn:?}: expected the modelled response envelope, \
         got {body}"
    );
}

/// The ARNs `ListAttachedGroupPolicies` reports for `group_name`, in the order the
/// service enumerates them, each checked to be reported under the friendly name its own
/// ARN ends in.
async fn attached_arns(router: &MultiServiceRouter, group_name: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListAttachedGroupPolicies",
        &[("GroupName", group_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {group_name:?}: ListAttachedGroupPolicies must succeed, got {status}: {body}"
    );
    let arns = elements(&body, "PolicyArn");
    let names = elements(&body, "PolicyName");
    assert_eq!(
        names.len(),
        arns.len(),
        "case {group_name:?}: every AttachedPolicy must carry both members: {body}"
    );
    for (name, arn) in names.iter().zip(&arns) {
        assert!(
            arn.ends_with(&format!("/{name}")),
            "case {group_name:?}: expected {arn:?} reported under the name its ARN ends \
             in, got {name:?}"
        );
    }
    arns
}

/// The groups `ListEntitiesForPolicy` reports for `policy_arn` — the reporting half of
/// an attachment, read from the policy's side.
async fn attached_groups(router: &MultiServiceRouter, policy_arn: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListEntitiesForPolicy",
        &[("PolicyArn", policy_arn)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?}: ListEntitiesForPolicy must succeed, got {status}: {body}"
    );
    elements(&body, "GroupName")
}

/// The `AttachmentCount` `GetPolicy` reports for `policy_arn` — "the number of entities
/// (users, groups, and roles) that the policy is attached to".
async fn attachment_count(router: &MultiServiceRouter, policy_arn: &str) -> String {
    let (status, body) = call(router, "GetPolicy", &[("PolicyArn", policy_arn)]).await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?}: GetPolicy must succeed, got {status}: {body}"
    );
    element(&body, "AttachmentCount")
}

/// One fixture group, the fixture policies attached to it and the ones then DETACHED
/// from it, by [`policy_arn`] index.
struct DetachCase {
    group_name: &'static str,
    path: &'static str,
    attached: &'static [usize],
    detached: &'static [usize],
}

/// Rows spanning the group path (root, one segment, nested), the group name's case and
/// alphabet, how much of the group's attachment set is removed (all of it, one of
/// several, several of several, none) and the OWNER and PATH of the policy removed (this
/// account at three path depths, AWS at two) — the dimensions the removal rule
/// generalises over.
const DETACH_CASES: &[DetachCase] = &[
    DetachCase {
        group_name: "admins",
        path: "/",
        attached: &[0, 1],
        detached: &[0, 1],
    },
    DetachCase {
        group_name: "Auditors.9",
        path: "/app/",
        attached: &[0, 2, 3],
        detached: &[2],
    },
    DetachCase {
        group_name: "readers_7",
        path: "/app/audit/",
        attached: &[1, 3, 4],
        detached: &[4, 1],
    },
    DetachCase {
        group_name: "writers+x=1",
        path: "/sales/west/",
        attached: &[2, 4],
        detached: &[],
    },
    DetachCase {
        group_name: "OPERATORS@4",
        path: "/",
        attached: &[3],
        detached: &[3],
    },
];

/// The attachments `case` was given, in the order the service enumerates them:
/// ascending by ARN, which is the key the attachment set is ordered on.
fn fixture_attachments(case: &DetachCase) -> Vec<String> {
    let mut arns: Vec<String> = case.attached.iter().map(|index| policy_arn(*index)).collect();
    arns.sort_unstable();
    arns
}

/// The attachments `case` must still carry once its detaches have run: the rule
/// restated over the fixture table — what a detach removes is the one attachment it
/// names, and nothing else.
fn remaining_attachments(case: &DetachCase) -> Vec<String> {
    let mut arns: Vec<String> = case
        .attached
        .iter()
        .filter(|index| !case.detached.contains(*index))
        .map(|index| policy_arn(*index))
        .collect();
    arns.sort_unstable();
    arns
}

/// The fixture groups the policy at `index` must still be reported as attached to, in
/// the order the service enumerates them: ascending by the case-folded name, which is
/// the key the group store is ordered on.
fn groups_still_carrying(index: usize) -> Vec<String> {
    let mut names: Vec<String> = DETACH_CASES
        .iter()
        .filter(|case| case.attached.contains(&index) && !case.detached.contains(&index))
        .map(|case| case.group_name.to_string())
        .collect();
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names
}

/// RULE: `DetachGroupPolicy` removes from the group its `GroupName` names exactly the
/// one managed-policy attachment its `PolicyArn` names, leaving every other attachment
/// of that group and every other group's attachments as they were, and every reader of
/// that state — the group's attachments, the entities the policy relates and the
/// policy's own attachment count — reports the set that is left.
#[tokio::test]
async fn detach_group_policy_removes_only_the_named_attachment_and_every_reader_agrees() {
    let router = router();
    create_customer_policies(&router).await;
    for case in DETACH_CASES {
        create_group(&router, case.group_name, case.path).await;
        for index in case.attached {
            attach_ok(&router, case.group_name, &policy_arn(*index)).await;
        }
    }

    // The attachments the detaches below remove are there to begin with: without this
    // pass, a row asserting that a group carries nothing afterwards would also pass
    // against a service that never attached anything.
    for case in DETACH_CASES {
        let expected = fixture_attachments(case);
        let reported = attached_arns(&router, case.group_name).await;
        assert_eq!(
            reported, expected,
            "case {:?}: expected the fixture attachments {expected:?}, got {reported:?}",
            case.group_name
        );
    }

    for case in DETACH_CASES {
        for index in case.detached {
            detach_ok(&router, case.group_name, &policy_arn(*index)).await;
        }
    }

    for case in DETACH_CASES {
        let expected = remaining_attachments(case);
        let reported = attached_arns(&router, case.group_name).await;
        assert_eq!(
            reported, expected,
            "case {:?}: expected {expected:?} still attached, got {reported:?}",
            case.group_name
        );
        // A read must not change what a later caller sees.
        let again = attached_arns(&router, case.group_name).await;
        assert_eq!(
            again, expected,
            "case {:?}: a read changed what the next caller sees, got {again:?}",
            case.group_name
        );
    }

    for index in 0..policy_count() {
        let arn = policy_arn(index);
        let expected = groups_still_carrying(index);
        let reported = attached_groups(&router, &arn).await;
        assert_eq!(
            reported, expected,
            "case {arn:?}: expected the groups still carrying it {expected:?}, got \
             {reported:?}"
        );
        // The account's own policies are readable through `GetPolicy`, which counts the
        // attachments that exist at the instant it is asked.
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

/// RULE: IAM entity names "are not distinguished by case", so the casing a request
/// spells a group name with does not pick out a different group — and because a detach
/// removes the attachment rather than decrementing a count, asking a second time names
/// an attachment the group no longer carries, which is the modelled not-found.
#[tokio::test]
async fn the_group_name_is_matched_case_insensitively_and_a_repeat_detach_is_refused() {
    let router = router();
    let group = "CaseFoldedGroup";
    create_group(&router, group, "/eng/").await;
    let held = create_policy(&router, "CaseFolded", "/eng/").await;

    // The rows differ in the casing the request names the group with, which is what the
    // case-folding rule generalises over, and in the owner of the policy detached.
    let spellings = [group, "casefoldedgroup", "CASEFOLDEDGROUP", "CaSeFoLdEdGrOuP"];
    let policies = [held.as_str(), AWS_MANAGED_POLICIES[0]];
    for policy in policies {
        for spelling in spellings {
            let case = format!("{spelling} -> {policy}");
            attach_ok(&router, group, policy).await;
            detach_ok(&router, spelling, policy).await;

            let reported = attached_arns(&router, group).await;
            assert!(
                reported.is_empty(),
                "case {case:?}: expected the attachment gone, got {reported:?}"
            );

            // Asking again names an attachment that no longer exists. The sentence names
            // the POLICY the request asked about, echoing the ARN exactly as supplied, and
            // nothing of the group — so it does not vary with the case the request spelled
            // the group in.
            let (status, body) = detach(&router, spelling, policy).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404 for an attachment the group no longer \
                 carries, got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "NoSuchEntity",
                "case {case:?}: expected NoSuchEntity, got {code:?}: {body}"
            );
            let message = element(&body, "Message");
            assert_eq!(
                message,
                not_attached_message(policy),
                "case {case:?}: expected the family's sentence, got {message:?}"
            );
        }
    }
}

/// One detach operation of the family: the member it takes its entity under and the
/// fixture entity's stored name.
struct DetachCall {
    action: &'static str,
    entity_member: &'static str,
    entity_name: &'static str,
}

/// Every operation of this service that removes a managed policy's attachment from an
/// entity (`smithy_query(query="digest")`, the `Detach*Policy` operations). One refusal
/// sentence serves all three, because what the request addresses is the policy.
const DETACH_CALLS: &[DetachCall] = &[
    DetachCall {
        action: "DetachGroupPolicy",
        entity_member: "GroupName",
        entity_name: "UncarriedGroup",
    },
    DetachCall {
        action: "DetachUserPolicy",
        entity_member: "UserName",
        entity_name: "uncarried-user",
    },
    DetachCall {
        action: "DetachRolePolicy",
        entity_member: "RoleName",
        entity_name: "UncarriedRole",
    },
];

/// RULE: a detach's subject is the ATTACHMENT, so a policy the named entity does not
/// carry refers to no entity that exists — 404 `NoSuchEntity`, worded as the detach
/// family words that absence for every entity kind — and the attachment another entity
/// holds of the same policy is left alone.
#[tokio::test]
async fn detaching_a_policy_the_entity_does_not_carry_is_refused() {
    let router = router();
    let carrying = "CarryingGroup";
    create_group(&router, carrying, "/").await;
    create_group(&router, "UncarriedGroup", "/eng/").await;

    let (status, body) = call(&router, "CreateUser", &[("UserName", "uncarried-user")]).await;
    assert_eq!(
        status, 200,
        "CreateUser must succeed for the fixture, got {status}: {body}"
    );
    let (status, body) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", "UncarriedRole"),
            ("Path", "/"),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateRole must succeed for the fixture, got {status}: {body}"
    );

    // The policy exists and is even attached — to ANOTHER group — so what every row
    // below is refused on is the attachment it names, not the policy.
    let held = create_policy(&router, "CarriedElsewhere", "/app/").await;
    attach_ok(&router, carrying, &held).await;

    // Rows spanning the owner of the policy whose attachment is asked for: one this
    // account holds and carries elsewhere, and one AWS owns that nothing carries.
    let uncarried = [held.as_str(), AWS_MANAGED_POLICIES[1]];
    for call_under_test in DETACH_CALLS {
        for policy in uncarried {
            let case = format!("{} on {policy}", call_under_test.action);
            let (status, body) = call(
                &router,
                call_under_test.action,
                &[
                    (call_under_test.entity_member, call_under_test.entity_name),
                    ("PolicyArn", policy),
                ],
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "NoSuchEntity",
                "case {case:?}: expected NoSuchEntity, got {code:?}: {body}"
            );
            let message = element(&body, "Message");
            assert_eq!(
                message,
                not_attached_message(policy),
                "case {case:?}: expected the family's sentence, got {message:?}"
            );
        }
    }

    // None of the refusals wrote anything: the group that does carry the policy still
    // does, and no user or role was attached by a call that failed.
    let reported = attached_arns(&router, carrying).await;
    assert_eq!(
        reported,
        vec![held.clone()],
        "a refused detach must leave another entity's attachment alone, got {reported:?}"
    );
    let groups = attached_groups(&router, &held).await;
    assert_eq!(
        groups,
        vec![carrying.to_string()],
        "expected only the carrying group reported, got {groups:?}"
    );
}

/// RULE: a `GroupName` the account holds no group under refers to a resource entity
/// that does not exist — 404 `NoSuchEntity` naming the group as the request spelled it
/// — and the refused call persists nothing.
#[tokio::test]
async fn detaching_from_a_group_the_account_does_not_hold_is_refused() {
    let router = router();
    let group = "HeldGroup";
    create_group(&router, group, "/").await;
    let held = create_policy(&router, "HeldPolicy", "/").await;
    attach_ok(&router, group, &held).await;

    // Rows spanning a name no group carries, a name that differs from a held one by one
    // character, and the single-character and 128-character bounds of `groupNameType`
    // (len 1..128, `smithy_query(query="shape", shape="groupNameType")`).
    let longest = "G".repeat(128);
    let unheld = ["MissingGroup", "HeldGroup2", "g", longest.as_str()];
    for group_name in unheld {
        let (status, body) = detach(&router, group_name, &held).await;
        assert_eq!(
            status, 404,
            "case {group_name:?}: expected HTTP 404, got {status}: {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "NoSuchEntity",
            "case {group_name:?}: expected NoSuchEntity, got {code:?}: {body}"
        );
        let message = element(&body, "Message");
        assert_eq!(
            message,
            format!("The group with name {group_name} cannot be found."),
            "case {group_name:?}: expected the absent-group sentence, got {message:?}"
        );
    }

    let reported = attached_arns(&router, group).await;
    assert_eq!(
        reported,
        vec![held.clone()],
        "a refused detach must leave the held attachment alone, got {reported:?}"
    );
}

/// RULE: `PolicyArn` must BE a managed policy's ARN —
/// `arn:<partition>:iam::<account>:policy/<path><name>`, with an empty region segment
/// because IAM is global — and a value that is not one is refused 400, removing nothing.
/// Which code it carries is the layer that took the form: a value that is not an ARN at
/// all is refused with the code IAM answers a wrong FORM with (`ValidationError`), while
/// an ARN naming another service or resource type is this operation's own modelled
/// refusal (`InvalidInput`).
#[tokio::test]
async fn detaching_an_arn_that_is_not_a_managed_policy_arn_is_refused() {
    let router = router();
    let group = "ArnValidationGroup";
    create_group(&router, group, "/").await;
    let held = create_policy(&router, "ArnValidation", "/").await;
    attach_ok(&router, group, &held).await;

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
        let (status, body) = detach(&router, group, policy_arn).await;
        assert_eq!(
            status, 400,
            "case {policy_arn:?}: expected HTTP 400, got {status}: {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {policy_arn:?}: expected {expected_code}, got {code:?}: {body}"
        );
    }

    let reported = attached_arns(&router, group).await;
    assert_eq!(
        reported,
        vec![held.clone()],
        "a refused detach must leave the held attachment alone, got {reported:?}"
    );
}

/// RULE: an attached managed policy is a dependent a group delete refuses on, so a
/// group whose attachments have all been detached no longer depends on any policy and
/// its delete succeeds.
#[tokio::test]
async fn a_group_whose_attachments_are_detached_no_longer_blocks_its_delete() {
    let router = router();
    create_customer_policies(&router).await;

    // Rows spanning how many managed policies the group carries, and the owner of each
    // (this account, AWS).
    let rows: &[(&str, &[usize])] = &[
        ("DeletableOne", &[0]),
        ("DeletableTwo", &[1, 3]),
        ("DeletableThree", &[0, 2, 4]),
    ];
    for (group, attached) in rows {
        create_group(&router, group, "/").await;
        for index in *attached {
            attach_ok(&router, group, &policy_arn(*index)).await;
        }

        let (status, body) = call(&router, "DeleteGroup", &[("GroupName", group)]).await;
        assert_eq!(
            status, 409,
            "case {group:?}: a group carrying an attachment must not be deletable, got \
             {status}: {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "DeleteConflict",
            "case {group:?}: expected DeleteConflict, got {code:?}: {body}"
        );

        for index in *attached {
            detach_ok(&router, group, &policy_arn(*index)).await;
        }

        let (status, body) = call(&router, "DeleteGroup", &[("GroupName", group)]).await;
        assert_eq!(
            status, 200,
            "case {group:?}: expected the delete to succeed once nothing is attached, got \
             {status}: {body}"
        );
        let (status, body) = call(&router, "GetGroup", &[("GroupName", group)]).await;
        assert_eq!(
            status, 404,
            "case {group:?}: expected the deleted group to be gone, got {status}: {body}"
        );
    }
}
