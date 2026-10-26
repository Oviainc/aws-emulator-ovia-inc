// SPDX-License-Identifier: CC0-1.0

//! What `ListCustomerManagedPolicyReferencesInPermissionSet` reports is a function of ONE
//! thing only: the attaches and detaches made on the permission set the request names.
//!
//! THE RULE under test: the customer managed policy references of a permission set are a
//! collection of its own, held against the (instance, permission set) pair and keyed by the
//! (`Name`, `Path`) identity of each reference — so
//! `AttachCustomerManagedPolicyReferenceToPermissionSet` adds exactly one entry to the set
//! of the pair the request names and changes nothing else, no other mutation of the same
//! permission set (its mutable members, its tags, its inline policy, its AWS managed-policy
//! attachments, its permissions boundary, a provisioning of it) and no mutation of any
//! OTHER permission set of the same instance can alter what that set reports, a permission
//! set no attach named reports none, and an AWS managed policy whose ARN carries the same
//! policy name as a reference is a different attachment in a different set.
//!
//! `customer_managed_policy_references.rs` pins the round trip itself (what an attach stores
//! and a list reports, over a table of reference forms) and `policy_attach_conflict.rs` pins
//! the duplicate-attach refusal. This suite pins the remaining half of the same rule: WHERE
//! the attach writes, and therefore what it must leave alone. Each row of the interleaving
//! table is a DIFFERENT neighbouring mutation, and the invariance is asserted NON-vacuously:
//! after the interleaving the readers of that other state are called too and must report the
//! changes the table made.
//!
//! The members each interleaved request carries are the model's own
//! (`smithy_query(query="operation",operation="UpdatePermissionSet,TagResource,PutInlinePolicyToPermissionSet,AttachManagedPolicyToPermissionSet,PutPermissionsBoundaryToPermissionSet,ProvisionPermissionSet",model_path="services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json")`).
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `awsJson1_1` envelope are exercised
//! rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns, and the
//! calls run against an ORGANIZATION instance because the permission-set surface is
//! withdrawn on an account instance.
//!
//! Every modelled 4xx refusal of this service is answered with HTTP 400 whatever
//! `@httpError` its shape declares, so the assertions below pin the status AND the wire error
//! code, which is what tells one refusal from another.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{call, error_code, organization_router};

/// `CustomerManagedPolicyReference$Path`'s documented default: "The default is `/`."
const DEFAULT_PATH: &str = "/";

/// An AWS managed policy ARN, in the form `ManagedPolicyArn`'s pattern requires
/// (`^arn:aws(-[a-z]{1,5}){0,3}:iam::aws:policy((/[A-Za-z0-9\.,\+@=_-]+)*)/([A-Za-z0-9\.,\+=@_-]+)$`).
const READ_ONLY_POLICY_ARN: &str = "arn:aws:iam::aws:policy/ReadOnlyAccess";

/// A second AWS managed policy, used as the permissions boundary so the boundary is
/// distinguishable from the attachment.
const POWER_USER_POLICY_ARN: &str = "arn:aws:iam::aws:policy/PowerUserAccess";

/// A `Description` the member's own constraints admit (`length 1..700`).
const NEW_DESCRIPTION: &str = "the description an interleaved update wrote";
/// A `SessionDuration` in the ISO-8601 form the member's pattern requires.
const NEW_SESSION_DURATION: &str = "PT3H";
/// A `RelayState` the member's own pattern admits.
const NEW_RELAY_STATE: &str = "https://example.com/portal";
/// An `InlinePolicy` document, within `PermissionSetPolicyDocument`'s `length 1..32768`.
const INLINE_DOCUMENT: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:List*","Resource":"*"}]}"#;
/// The tag an interleaved `TagResource` writes (`TagKey` 1..128, `TagValue` 0..256).
const TAG_KEY: &str = "Owner";
const TAG_VALUE: &str = "IdentityTeam";
/// A `TargetId` of `^\d{12}$`, for the interleaved provisioning.
const TARGET_ACCOUNT: &str = "210987654321";

/// One row of the table: a SET of references to attach, as the request bodies send them,
/// and the (`Name`, `Path`) identities a later list must report for them.
struct SetCase {
    label: &'static str,
    /// What each attach sends as `CustomerManagedPolicyReference`.
    sent: Vec<Value>,
    /// The identities, `Name|Path`, the list must report — sorted.
    identities: Vec<String>,
}

/// `Name|Path`, the identity a reference is held under once `Path` is materialised.
fn identity(name: &str, path: &str) -> String {
    format!("{name}|{path}")
}

/// One row, with its expected identities sorted — every comparison in this suite is about
/// the SET, so both sides are compared in one order.
fn case(label: &'static str, sent: Vec<Value>, mut identities: Vec<String>) -> SetCase {
    identities.sort();
    SetCase {
        label,
        sent,
        identities,
    }
}

/// The table. Rows differ along what the rule generalises over: the NUMBER of references in
/// the set (one, a handful), the presence and nesting of `Path`, the character classes
/// `Name` (`^[\w+=,.@-]+$`) and `Path` (`^((/[A-Za-z0-9\.,\+@=_-]+)*)/$`) admit, whether a
/// reference's `Name` collides with the AWS managed policy the interleaving attaches, and
/// whether one `Name` appears under several paths — which the identity rule makes several
/// references.
fn set_cases() -> Vec<SetCase> {
    vec![
        case(
            "a single reference with no Path at all",
            vec![json!({"Name": "SoleReference"})],
            vec![identity("SoleReference", DEFAULT_PATH)],
        ),
        case(
            "one Name under three different paths",
            vec![
                json!({"Name": "Shared"}),
                json!({"Name": "Shared", "Path": "/engineering/"}),
                json!({"Name": "Shared", "Path": "/engineering/platform/"}),
            ],
            vec![
                identity("Shared", "/engineering/"),
                identity("Shared", "/engineering/platform/"),
                identity("Shared", DEFAULT_PATH),
            ],
        ),
        case(
            "a reference whose Name is the AWS managed policy's policy name",
            vec![
                json!({"Name": "ReadOnlyAccess", "Path": "/"}),
                json!({"Name": "PowerUserAccess", "Path": "/boundaries/"}),
            ],
            vec![
                identity("PowerUserAccess", "/boundaries/"),
                identity("ReadOnlyAccess", "/"),
            ],
        ),
        case(
            "references using the full character classes of both members",
            vec![
                json!({"Name": "a.b,c+d=e@f_g-H9", "Path": "/a.b,c+d@e=f_g-H9/"}),
                json!({"Name": "plain", "Path": "/a/b/c/d/e/"}),
                json!({"Name": "W_9", "Path": "/"}),
                json!({"Name": "trailing-dash-"}),
            ],
            vec![
                identity("W_9", "/"),
                identity("a.b,c+d=e@f_g-H9", "/a.b,c+d@e=f_g-H9/"),
                identity("plain", "/a/b/c/d/e/"),
                identity("trailing-dash-", DEFAULT_PATH),
            ],
        ),
    ]
}

/// Create a permission set named `name` on `instance_arn` and return its minted ARN.
async fn create_permission_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreatePermissionSet expected 200, got {status} {body}"
    );
    body["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no minted PermissionSetArn in {body}"))
        .to_string()
}

/// Attach one reference, as `(status, body)`.
async fn attach(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    reference: &Value,
) -> (u16, Value) {
    call(
        router,
        "AttachCustomerManagedPolicyReferenceToPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
            "CustomerManagedPolicyReference": reference,
        }),
    )
    .await
}

/// The identities the permission set's reference set holds, read over the wire and sorted,
/// so a comparison is about the SET rather than about a page order.
async fn reference_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    case: &str,
) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListCustomerManagedPolicyReferencesInPermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the list expected 200, got {status} {body}"
    );
    let mut found: Vec<String> = body["CustomerManagedPolicyReferences"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the list must report an array, got {body}"))
        .iter()
        .map(|item| {
            identity(
                item["Name"].as_str().unwrap_or_default(),
                item["Path"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    found.sort();
    found
}

/// The AWS managed policy ARNs the permission set's OTHER policy set holds, sorted.
async fn managed_policy_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    case: &str,
) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListManagedPoliciesInPermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListManagedPoliciesInPermissionSet expected 200, got {status} {body}"
    );
    let mut found: Vec<String> = body["AttachedManagedPolicies"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the managed list must be an array, got {body}"))
        .iter()
        .map(|item| item["Arn"].as_str().unwrap_or_default().to_string())
        .collect();
    found.sort();
    found
}

/// Attach every reference of a row and assert the set reports exactly its identities.
async fn attach_all(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    case: &SetCase,
) {
    for reference in &case.sent {
        let (status, body) = attach(router, instance_arn, permission_set_arn, reference).await;
        assert_eq!(
            status, 200,
            "case {:?}: the attach of {reference} expected 200, got {status} {body}",
            case.label
        );
    }
    let found = reference_set(router, instance_arn, permission_set_arn, case.label).await;
    assert_eq!(
        found, case.identities,
        "case {:?}: the attaches must store exactly {:?}, got {found:?}",
        case.label, case.identities
    );
}

/// One neighbouring mutation: the operation to call and the body to send.
struct Interleaved {
    label: &'static str,
    operation: &'static str,
    body: Value,
}

/// The neighbouring mutations, in the order they are applied. Rows differ along WHICH part of
/// the permission set's record they move: its mutable members, its tags, its inline policy,
/// its AWS managed-policy attachments, its permissions boundary and its provisioning — and
/// one row REMOVES what an earlier row added, so a mutation in both directions is covered.
fn interleaved_mutations(instance_arn: &str, permission_set_arn: &str) -> Vec<Interleaved> {
    vec![
        Interleaved {
            label: "an update that rewrites every mutable member of the permission set",
            operation: "UpdatePermissionSet",
            body: json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "Description": NEW_DESCRIPTION,
                "SessionDuration": NEW_SESSION_DURATION,
                "RelayState": NEW_RELAY_STATE,
            }),
        },
        Interleaved {
            label: "a tag written onto the same permission set",
            operation: "TagResource",
            body: json!({
                "InstanceArn": instance_arn,
                "ResourceArn": permission_set_arn,
                "Tags": [{"Key": TAG_KEY, "Value": TAG_VALUE}],
            }),
        },
        Interleaved {
            label: "an inline policy put on the same permission set",
            operation: "PutInlinePolicyToPermissionSet",
            body: json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "InlinePolicy": INLINE_DOCUMENT,
            }),
        },
        Interleaved {
            label: "an AWS managed policy attached to the same permission set",
            operation: "AttachManagedPolicyToPermissionSet",
            body: json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "ManagedPolicyArn": READ_ONLY_POLICY_ARN,
            }),
        },
        Interleaved {
            label: "a permissions boundary put on the same permission set",
            operation: "PutPermissionsBoundaryToPermissionSet",
            body: json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "PermissionsBoundary": {"ManagedPolicyArn": POWER_USER_POLICY_ARN},
            }),
        },
        Interleaved {
            label: "a provisioning of the same permission set to one account",
            operation: "ProvisionPermissionSet",
            body: json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "TargetId": TARGET_ACCOUNT,
                "TargetType": "AWS_ACCOUNT",
            }),
        },
        Interleaved {
            label: "the AWS managed policy detached again",
            operation: "DetachManagedPolicyFromPermissionSet",
            body: json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "ManagedPolicyArn": READ_ONLY_POLICY_ARN,
            }),
        },
    ]
}

/// THE RULE: every other part of the permission set's record can move without changing the
/// reference set the attaches wrote — asserted for each row of the table, with every
/// interleaving applied in turn and the set read back after each one.
#[tokio::test]
async fn other_state_of_the_permission_set_never_changes_the_reported_references() {
    let (router, instance_arn) = organization_router();

    for (index, case) in set_cases().into_iter().enumerate() {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Interleaved{index}")).await;
        attach_all(&router, &instance_arn, &permission_set_arn, &case).await;

        for row in interleaved_mutations(&instance_arn, &permission_set_arn) {
            let (status, body) = call(&router, row.operation, &row.body).await;
            assert_eq!(
                status, 200,
                "case {:?} / {:?}: {} expected 200, got {status} {body}",
                case.label, row.label, row.operation
            );

            let found =
                reference_set(&router, &instance_arn, &permission_set_arn, case.label).await;
            assert_eq!(
                found, case.identities,
                "case {:?} / {:?}: the reference set must still be {:?}, got {found:?}",
                case.label, row.label, case.identities
            );

            // The AWS managed policies are a set of their OWN, keyed by ARN: while that
            // attachment stands, it is reported there and only there — not even the
            // reference whose `Name` is that policy's own policy name joins it, and no
            // reference of this row appears in it.
            if row.operation == "AttachManagedPolicyToPermissionSet" {
                let managed =
                    managed_policy_set(&router, &instance_arn, &permission_set_arn, case.label)
                        .await;
                assert_eq!(
                    managed,
                    vec![READ_ONLY_POLICY_ARN.to_string()],
                    "case {:?}: the managed-policy set must hold exactly the attached ARN, \
                     got {managed:?}",
                    case.label
                );
            }
        }

        // NOT vacuous: the interleaved rows really did change the rest of the record, and
        // each of those changes is reported by its OWN reader.
        let (status, described) = call(
            &router,
            "DescribePermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribePermissionSet expected 200, got {status} {described}",
            case.label
        );
        assert_eq!(
            described["PermissionSet"]["Description"].as_str(),
            Some(NEW_DESCRIPTION),
            "case {:?}: the interleaved update must be reported, got {described}",
            case.label
        );
        assert_eq!(
            described["PermissionSet"]["SessionDuration"].as_str(),
            Some(NEW_SESSION_DURATION),
            "case {:?}: the interleaved update must be reported, got {described}",
            case.label
        );
        assert_eq!(
            described["PermissionSet"]["RelayState"].as_str(),
            Some(NEW_RELAY_STATE),
            "case {:?}: the interleaved update must be reported, got {described}",
            case.label
        );

        let (status, policy) = call(
            &router,
            "GetInlinePolicyForPermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: GetInlinePolicyForPermissionSet expected 200, got {status} {policy}",
            case.label
        );
        assert_eq!(
            policy["InlinePolicy"].as_str(),
            Some(INLINE_DOCUMENT),
            "case {:?}: the interleaved inline put must be reported, got {policy}",
            case.label
        );

        let (status, boundary) = call(
            &router,
            "GetPermissionsBoundaryForPermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: GetPermissionsBoundaryForPermissionSet expected 200, got {status} \
             {boundary}",
            case.label
        );
        assert_eq!(
            boundary["PermissionsBoundary"]["ManagedPolicyArn"].as_str(),
            Some(POWER_USER_POLICY_ARN),
            "case {:?}: the interleaved boundary put must be reported, got {boundary}",
            case.label
        );

        let (status, tags) = call(
            &router,
            "ListTagsForResource",
            &json!({"InstanceArn": instance_arn, "ResourceArn": permission_set_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource expected 200, got {status} {tags}",
            case.label
        );
        assert!(
            tags["Tags"].as_array().is_some_and(|written| written
                .iter()
                .any(|tag| tag["Key"].as_str() == Some(TAG_KEY)
                    && tag["Value"].as_str() == Some(TAG_VALUE))),
            "case {:?}: the interleaved tag must be reported, got {tags}",
            case.label
        );

        // The AWS managed policies are a set of their OWN: the attach and the detach the
        // interleaving made are reported there, and none of this row's references ever
        // appear in it — not even the one whose `Name` is that policy's own policy name.
        let managed = managed_policy_set(&router, &instance_arn, &permission_set_arn, case.label)
            .await;
        assert!(
            managed.is_empty(),
            "case {:?}: the detached managed policy must leave that set empty, got {managed:?}",
            case.label
        );
    }
}

/// The reported set belongs to ONE permission set: whatever happens to a NEIGHBOUR of the
/// same instance — the very same references attached to it, its own mutations, even its
/// deletion — the subject reports its own set, and a permission set no attach named reports
/// none.
#[tokio::test]
async fn a_neighbouring_permission_set_never_changes_the_reported_references() {
    let (router, instance_arn) = organization_router();
    let cases = set_cases();
    // Selected by label, so adding or reordering a row of the table cannot silently change
    // which set the subject carries.
    let subject_case = cases
        .iter()
        .find(|case| case.label.starts_with("references using the full character classes"))
        .expect("the table carries the full-character-class row");

    let subject = create_permission_set(&router, &instance_arn, "Subject").await;
    let neighbour = create_permission_set(&router, &instance_arn, "Neighbour").await;
    // A third permission set that never receives an attach: its set must stay empty through
    // everything the other two do.
    let untouched = create_permission_set(&router, &instance_arn, "Untouched").await;

    attach_all(&router, &instance_arn, &subject, subject_case).await;

    /// What the two bystanders must report after a row has been applied to the neighbour.
    async fn bystanders_unmoved(
        router: &MultiServiceRouter,
        instance_arn: &str,
        subject: &str,
        subject_identities: &[String],
        untouched: &str,
        label: &str,
    ) {
        let found = reference_set(router, instance_arn, subject, label).await;
        assert_eq!(
            found, subject_identities,
            "case {label:?}: the subject must keep reporting its own set {subject_identities:?}, \
             got {found:?}"
        );
        let none = reference_set(router, instance_arn, untouched, label).await;
        assert!(
            none.is_empty(),
            "case {label:?}: a permission set no attach named must report none, got {none:?}"
        );
    }

    // Every reference of every row of the table attached to the NEIGHBOUR, one at a time:
    // its own set grows by exactly one entry per attach (so the attaches are not vacuous)
    // while the two bystanders do not move. Every identity of the table is distinct, so each
    // of these is a first attach.
    let mut attached_so_far = 0usize;
    for case in &cases {
        for reference in &case.sent {
            let label = format!("an attach of {reference} on the neighbour");
            let (status, body) = attach(&router, &instance_arn, &neighbour, reference).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the attach expected 200, got {status} {body}"
            );
            attached_so_far += 1;

            let grown = reference_set(&router, &instance_arn, &neighbour, &label).await;
            assert_eq!(
                grown.len(),
                attached_so_far,
                "case {label:?}: the neighbour's set must hold one entry per attach, got \
                 {grown:?}"
            );

            bystanders_unmoved(
                &router,
                &instance_arn,
                &subject,
                &subject_case.identities,
                &untouched,
                &label,
            )
            .await;
        }
    }

    // The neighbour now holds the union of the whole table, once each.
    let mut union: Vec<String> = cases
        .iter()
        .flat_map(|case| case.identities.iter().cloned())
        .collect();
    union.sort();
    let on_neighbour = reference_set(&router, &instance_arn, &neighbour, "the neighbour").await;
    assert_eq!(
        on_neighbour, union,
        "the neighbour must hold the whole table {union:?}, got {on_neighbour:?}"
    );

    // Then the neighbour's OTHER state moved, and finally the neighbour deleted outright.
    let mut rows: Vec<(String, &'static str, Value)> = Vec::new();
    for row in interleaved_mutations(&instance_arn, &neighbour) {
        rows.push((
            format!("{} on the neighbour", row.label),
            row.operation,
            row.body,
        ));
    }
    rows.push((
        "the neighbour deleted outright".to_string(),
        "DeletePermissionSet",
        json!({"InstanceArn": instance_arn, "PermissionSetArn": neighbour}),
    ));

    for (label, operation, body) in rows {
        let (status, answer) = call(&router, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {label:?}: {operation} expected 200, got {status} {answer}"
        );
        bystanders_unmoved(
            &router,
            &instance_arn,
            &subject,
            &subject_case.identities,
            &untouched,
            &label,
        )
        .await;
    }

    // The deleted neighbour's record is gone with it, reference set and all.
    let (status, body) = call(
        &router,
        "ListCustomerManagedPolicyReferencesInPermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": neighbour}),
    )
    .await;
    assert_eq!(
        status, 400,
        "the deleted neighbour's list expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "the deleted neighbour's list expected ResourceNotFoundException, got {body}"
    );
}

/// The set is held against the permission set's own identity, not against its NAME: a
/// permission set created again under the name a deleted one used is a different resource
/// and carries none of the deleted one's references.
#[tokio::test]
async fn a_permission_set_recreated_under_the_same_name_carries_no_earlier_references() {
    let (router, instance_arn) = organization_router();

    for case in set_cases() {
        let name = "Recreated";
        let first = create_permission_set(&router, &instance_arn, name).await;
        attach_all(&router, &instance_arn, &first, &case).await;

        let (status, body) = call(
            &router,
            "DeletePermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": first}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DeletePermissionSet expected 200, got {status} {body}",
            case.label
        );

        let second = create_permission_set(&router, &instance_arn, name).await;
        assert_ne!(
            second, first,
            "case {:?}: the recreated permission set must carry a freshly minted ARN, got \
             {second}",
            case.label
        );
        let found = reference_set(&router, &instance_arn, &second, case.label).await;
        assert!(
            found.is_empty(),
            "case {:?}: the recreated permission set must report no references, got {found:?}",
            case.label
        );

        // And the first ARN resolves to nothing at all now, so the earlier set is not
        // reachable through it either.
        let (status, body) = call(
            &router,
            "ListCustomerManagedPolicyReferencesInPermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": first}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: the deleted ARN's list expected HTTP 400, got {status} {body}",
            case.label
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {:?}: the deleted ARN's list expected ResourceNotFoundException, got {body}",
            case.label
        );

        // Leave the instance as it was found, so the next row starts from the same place.
        let (status, body) = call(
            &router,
            "DeletePermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": second}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the cleanup delete expected 200, got {status} {body}",
            case.label
        );
    }
}
