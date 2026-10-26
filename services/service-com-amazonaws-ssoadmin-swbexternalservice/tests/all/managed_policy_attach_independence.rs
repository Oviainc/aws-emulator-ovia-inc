// SPDX-License-Identifier: CC0-1.0

//! WHERE `AttachManagedPolicyToPermissionSet` writes, end to end over the `awsJson1_1` wire.
//!
//! THE RULE under test: the AWS managed policies of a permission set are a collection of its
//! own, held against the (instance, permission set) pair the request names and keyed by the
//! `ManagedPolicyArn` that identifies an AWS managed policy — so one attach adds exactly one
//! entry to the set of THAT pair and changes nothing else: no other permission set of the
//! same instance, none of the other state of the same permission set (its mutable members,
//! its tags, its inline policy, its customer managed policy references, its permissions
//! boundary, its provisioning), and nothing a permission set no attach named reports. The
//! same holds in reverse: no mutation of that other state alters what the managed-policy set
//! reports, and the set is held against the permission set's minted identity rather than its
//! name.
//!
//! `managed_policies.rs` pins the round trip itself (what one attach stores, what a detach
//! removes, the refusals and the per-permission-set ceiling) and `policy_attach_conflict.rs`
//! pins the duplicate-attach refusal. This suite pins the remaining half of the same rule:
//! what the attach must LEAVE ALONE. Each interleaving is asserted NON-vacuously — after the
//! neighbouring mutations the readers of that other state are called too and must report the
//! changes the table made.
//!
//! The members each interleaved request carries are the model's own
//! (`smithy_query(query="operation",operation="UpdatePermissionSet,TagResource,PutInlinePolicyToPermissionSet,AttachCustomerManagedPolicyReferenceToPermissionSet,PutPermissionsBoundaryToPermissionSet,ProvisionPermissionSet",model_path="services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json")`).
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises over:
//! the NUMBER of policies attached, the partitions, nested paths and name charset the model's
//! `ManagedPolicyArn` pattern admits, its published upper length bound (`length(min: 20, max:
//! 2048)`), one policy NAME reached under several paths — which the ARN identity makes several
//! policies — and the order the attaches are made in.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `awsJson1_1` envelope are exercised
//! rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns, and the
//! calls run against an ORGANIZATION instance because the permission-set surface is withdrawn
//! on an account instance.
//!
//! Every modelled 4xx refusal of this service is answered with HTTP 400 whatever `@httpError`
//! its shape declares, so the assertions below pin the status AND the wire error code, which
//! is what tells one refusal from another.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{call, error_code, organization_router};

/// The fixed part of an AWS managed policy ARN of the `aws` partition, per the model's
/// `ManagedPolicyArn` pattern
/// `^arn:aws(-[a-z]{1,5}){0,3}:iam::aws:policy((/[A-Za-z0-9\.,\+@=_-]+)*)/([A-Za-z0-9\.,\+=@_-]+)$`.
const POLICY_PREFIX: &str = "arn:aws:iam::aws:policy/";

/// `ManagedPolicyArn`'s published upper length bound (`length(min: 20, max: 2048)`).
const ARN_MAX: usize = 2048;

/// The one policy a NEIGHBOURING permission set carries, so the bystander assertions are
/// about a set that is not empty.
const NEIGHBOUR_POLICY_ARN: &str = "arn:aws:iam::aws:policy/NeighbourOnlyAccess";

/// The AWS managed policy an interleaved `PutPermissionsBoundaryToPermissionSet` sends: the
/// boundary is a slot of its own, so this ARN must never join the attached SET.
const BOUNDARY_POLICY_ARN: &str = "arn:aws:iam::aws:policy/PowerUserAccess";

/// A `Description` within the member's own constraints (`length 1..700`).
const NEW_DESCRIPTION: &str = "the description an interleaved update wrote";
/// A `SessionDuration` in the ISO-8601 form the member's pattern requires.
const NEW_SESSION_DURATION: &str = "PT4H";
/// A `RelayState` the member's own pattern admits.
const NEW_RELAY_STATE: &str = "https://example.com/start";
/// An `InlinePolicy` document, within `PermissionSetPolicyDocument`'s `length 1..32768`.
const INLINE_DOCUMENT: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"ec2:Describe*","Resource":"*"}]}"#;
/// The tag an interleaved `TagResource` writes (`TagKey` 1..128, `TagValue` 0..256).
const TAG_KEY: &str = "CostCentre";
const TAG_VALUE: &str = "Platform";
/// The (`Name`, `Path`) of the customer managed policy reference the interleaving attaches —
/// a `Name` that is also an AWS policy name, so the two collections are told apart by the
/// collection they live in rather than by their text.
const REFERENCE_NAME: &str = "ReadOnlyAccess";
const REFERENCE_PATH: &str = "/";
/// A `TargetId` of `^\d{12}$`, for the interleaved provisioning.
const TARGET_ACCOUNT: &str = "345678901234";

/// An ARN of exactly [`ARN_MAX`] characters: the bound is reached with a long PATH, so the
/// final segment stays inside `AttachedManagedPolicy$Name`'s own `length(min: 1, max: 100)`.
fn max_length_policy_arn() -> String {
    const NAME: &str = "/MaxLengthAccess";
    let path = "p".repeat(ARN_MAX - POLICY_PREFIX.len() - NAME.len());
    format!("{POLICY_PREFIX}{path}{NAME}")
}

/// One row: a SET of AWS managed policy ARNs to attach, and the ARNs a later list must
/// report for them.
struct SetCase {
    label: &'static str,
    /// What each attach sends as `ManagedPolicyArn`, in the order the attaches are made.
    sent: Vec<String>,
    /// The same ARNs, sorted — every comparison in this suite is about the SET.
    expected: Vec<String>,
}

/// One row, with its expectation sorted so both sides of every comparison are in one order.
fn case(label: &'static str, sent: Vec<String>) -> SetCase {
    let mut expected = sent.clone();
    expected.sort();
    SetCase {
        label,
        sent,
        expected,
    }
}

/// The table. Every row's ARNs are distinct, so each attach of a row is a FIRST attach, and
/// no row reaches `state::MAX_MANAGED_POLICIES_PER_PERMISSION_SET`.
fn set_cases() -> Vec<SetCase> {
    vec![
        case(
            "a single policy with no path",
            vec![format!("{POLICY_PREFIX}ReadOnlyAccess")],
        ),
        case(
            "one policy name under three different paths",
            vec![
                format!("{POLICY_PREFIX}Shared"),
                format!("{POLICY_PREFIX}job-function/Shared"),
                format!("{POLICY_PREFIX}a/b/c/Shared"),
            ],
        ),
        case(
            "policies of three partitions the pattern admits",
            vec![
                format!("{POLICY_PREFIX}AdministratorAccess"),
                "arn:aws-us-gov:iam::aws:policy/AdministratorAccess".to_string(),
                "arn:aws-cn:iam::aws:policy/AdministratorAccess".to_string(),
            ],
        ),
        case(
            "the full name charset beside the upper length bound",
            vec![
                format!("{POLICY_PREFIX}a.b,c+d=e@f_g-H9"),
                max_length_policy_arn(),
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

/// `AttachManagedPolicyToPermissionSet`, as `(status, body)`.
async fn attach(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    managed_policy_arn: &str,
) -> (u16, Value) {
    call(
        router,
        "AttachManagedPolicyToPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
            "ManagedPolicyArn": managed_policy_arn,
        }),
    )
    .await
}

/// The ARNs the permission set's managed-policy set holds, read over the wire and sorted, so
/// a comparison is about the SET rather than about a page order.
async fn attached_set(
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
        .unwrap_or_else(|| panic!("case {case:?}: the list must report an array, got {body}"))
        .iter()
        .map(|item| item["Arn"].as_str().unwrap_or_default().to_string())
        .collect();
    found.sort();
    found
}

/// The (`Name`, `Path`) identities the permission set's OTHER policy collection holds.
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
        "case {case:?}: ListCustomerManagedPolicyReferencesInPermissionSet expected 200, got \
         {status} {body}"
    );
    let mut found: Vec<String> = body["CustomerManagedPolicyReferences"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the reference list must be an array, got {body}"))
        .iter()
        .map(|item| {
            format!(
                "{}|{}",
                item["Name"].as_str().unwrap_or_default(),
                item["Path"].as_str().unwrap_or_default()
            )
        })
        .collect();
    found.sort();
    found
}

/// Attach every ARN of a row and assert the set then reports exactly them.
async fn attach_all(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    case: &SetCase,
) {
    for arn in &case.sent {
        let (status, body) = attach(router, instance_arn, permission_set_arn, arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the attach of {arn} expected 200, got {status} {body}",
            case.label
        );
    }
    let found = attached_set(router, instance_arn, permission_set_arn, case.label).await;
    assert_eq!(
        found, case.expected,
        "case {:?}: the attaches must store exactly {:?}, got {found:?}",
        case.label, case.expected
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
/// its customer managed policy references, its permissions boundary and its provisioning —
/// and one row REMOVES what an earlier row added, so a mutation in both directions is
/// covered.
fn interleaved_mutations(instance_arn: &str, permission_set_arn: &str) -> Vec<Interleaved> {
    let reference = json!({"Name": REFERENCE_NAME, "Path": REFERENCE_PATH});
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
            label: "a customer managed policy reference attached to the same permission set",
            operation: "AttachCustomerManagedPolicyReferenceToPermissionSet",
            body: json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "CustomerManagedPolicyReference": reference,
            }),
        },
        Interleaved {
            label: "a permissions boundary put on the same permission set",
            operation: "PutPermissionsBoundaryToPermissionSet",
            body: json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "PermissionsBoundary": {"ManagedPolicyArn": BOUNDARY_POLICY_ARN},
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
            label: "the customer managed policy reference detached again",
            operation: "DetachCustomerManagedPolicyReferenceFromPermissionSet",
            body: json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "CustomerManagedPolicyReference": reference,
            }),
        },
    ]
}

/// THE RULE: each attach adds exactly ONE entry to the set of the (instance, permission set)
/// pair the request names, and the permission sets it does not name do not move — neither a
/// neighbour carrying a policy of its own nor one no attach ever named.
#[tokio::test]
async fn an_attach_adds_one_entry_to_the_set_of_the_pair_it_names() {
    let (router, instance_arn) = organization_router();

    for (index, case) in set_cases().into_iter().enumerate() {
        let subject = create_permission_set(&router, &instance_arn, &format!("Subject{index}")).await;
        let neighbour =
            create_permission_set(&router, &instance_arn, &format!("Neighbour{index}")).await;
        // A third permission set that never receives an attach: its set must stay empty
        // through everything the other two do.
        let untouched =
            create_permission_set(&router, &instance_arn, &format!("Untouched{index}")).await;

        let (status, body) = attach(&router, &instance_arn, &neighbour, NEIGHBOUR_POLICY_ARN).await;
        assert_eq!(
            status, 200,
            "case {:?}: the neighbour's own attach expected 200, got {status} {body}",
            case.label
        );

        for (already, arn) in case.sent.iter().enumerate() {
            let (status, body) = attach(&router, &instance_arn, &subject, arn).await;
            assert_eq!(
                status, 200,
                "case {:?}: the attach of {arn} expected 200, got {status} {body}",
                case.label
            );

            let grown = attached_set(&router, &instance_arn, &subject, case.label).await;
            assert_eq!(
                grown.len(),
                already + 1,
                "case {:?}: the subject's set must hold one entry per attach, got {grown:?}",
                case.label
            );
            assert!(
                grown.contains(arn),
                "case {:?}: the subject's set must hold the just-attached {arn}, got {grown:?}",
                case.label
            );

            let on_neighbour = attached_set(&router, &instance_arn, &neighbour, case.label).await;
            assert_eq!(
                on_neighbour,
                vec![NEIGHBOUR_POLICY_ARN.to_string()],
                "case {:?}: the neighbour must keep reporting only its own policy, got \
                 {on_neighbour:?}",
                case.label
            );
            let none = attached_set(&router, &instance_arn, &untouched, case.label).await;
            assert!(
                none.is_empty(),
                "case {:?}: a permission set no attach named must report none, got {none:?}",
                case.label
            );
        }

        let found = attached_set(&router, &instance_arn, &subject, case.label).await;
        assert_eq!(
            found, case.expected,
            "case {:?}: the subject must hold exactly {:?}, got {found:?}",
            case.label, case.expected
        );

        // The same ARNs attached to the NEIGHBOUR in the reverse order: the collection is a
        // set keyed by ARN, so what it reports does not depend on the order the attaches were
        // made in.
        for arn in case.sent.iter().rev() {
            let (status, body) = attach(&router, &instance_arn, &neighbour, arn).await;
            assert_eq!(
                status, 200,
                "case {:?}: the reverse-order attach of {arn} expected 200, got {status} {body}",
                case.label
            );
        }
        let mut reversed_expected = case.expected.clone();
        reversed_expected.push(NEIGHBOUR_POLICY_ARN.to_string());
        reversed_expected.sort();
        let on_neighbour = attached_set(&router, &instance_arn, &neighbour, case.label).await;
        assert_eq!(
            on_neighbour, reversed_expected,
            "case {:?}: the attach order must not change the reported set {reversed_expected:?}, \
             got {on_neighbour:?}",
            case.label
        );
    }
}

/// THE RULE in reverse: every other part of the permission set's record can move without
/// changing the managed-policy set the attaches wrote — asserted for each row, with every
/// interleaving applied in turn and the set read back after each one, and the permissions
/// boundary's own ARN shown never to join the attached set.
#[tokio::test]
async fn other_state_of_the_permission_set_never_changes_the_attached_policies() {
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

            let found = attached_set(&router, &instance_arn, &permission_set_arn, case.label).await;
            assert_eq!(
                found, case.expected,
                "case {:?} / {:?}: the attached set must still be {:?}, got {found:?}",
                case.label, row.label, case.expected
            );

            // The customer managed policy references are a collection of their OWN, keyed by
            // (`Name`, `Path`): the interleaved attach and detach are reported there and
            // nowhere else, which is also what makes those two rows non-vacuous.
            if row.operation.ends_with("CustomerManagedPolicyReferenceToPermissionSet") {
                let references =
                    reference_set(&router, &instance_arn, &permission_set_arn, case.label).await;
                assert_eq!(
                    references,
                    vec![format!("{REFERENCE_NAME}|{REFERENCE_PATH}")],
                    "case {:?}: the reference must be reported in its own collection, got \
                     {references:?}",
                    case.label
                );
            }
            if row.operation.ends_with("CustomerManagedPolicyReferenceFromPermissionSet") {
                let references =
                    reference_set(&router, &instance_arn, &permission_set_arn, case.label).await;
                assert!(
                    references.is_empty(),
                    "case {:?}: the detached reference must leave its collection empty, got \
                     {references:?}",
                    case.label
                );
            }
        }

        // NOT vacuous: the interleaved rows really did change the rest of the record, and each
        // of those changes is reported by its OWN reader.
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
        for (member, written) in [
            ("Description", NEW_DESCRIPTION),
            ("SessionDuration", NEW_SESSION_DURATION),
            ("RelayState", NEW_RELAY_STATE),
        ] {
            assert_eq!(
                described["PermissionSet"][member].as_str(),
                Some(written),
                "case {:?}: the interleaved update must be reported in {member}, got {described}",
                case.label
            );
        }

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
            Some(BOUNDARY_POLICY_ARN),
            "case {:?}: the interleaved boundary put must be reported, got {boundary}",
            case.label
        );
        // And the boundary is a SLOT, not an attachment: its ARN is not in the attached set.
        let found = attached_set(&router, &instance_arn, &permission_set_arn, case.label).await;
        assert!(
            !found.contains(&BOUNDARY_POLICY_ARN.to_string()),
            "case {:?}: the boundary's ARN must not join the attached set, got {found:?}",
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
    }
}

/// The set is held against the permission set's minted identity, not against its NAME: a
/// permission set created again under the name a deleted one used is a different resource and
/// carries none of the deleted one's attachments.
#[tokio::test]
async fn a_permission_set_recreated_under_the_same_name_carries_no_earlier_policies() {
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
        let found = attached_set(&router, &instance_arn, &second, case.label).await;
        assert!(
            found.is_empty(),
            "case {:?}: the recreated permission set must report no attachments, got {found:?}",
            case.label
        );

        // And the deleted ARN resolves to nothing at all now, so the earlier set is not
        // reachable through it either.
        let (status, body) = call(
            &router,
            "ListManagedPoliciesInPermissionSet",
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
