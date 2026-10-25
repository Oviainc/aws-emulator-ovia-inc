// SPDX-License-Identifier: CC0-1.0

//! What `GetInlinePolicyForPermissionSet` reports is a function of ONE thing only.
//!
//! THE RULE under test: `GetInlinePolicyForPermissionSet` reports the inline policy
//! document held against the (instance, permission set) pair the request names, as the last
//! `PutInlinePolicyToPermissionSet` on that pair stored it — so no other change to the
//! permission set (its mutable members, its tags, its managed-policy attachments, its
//! customer-managed policy references, its permissions boundary) and no change to any OTHER
//! permission set of the same instance can alter what it reports.
//!
//! `inline_policy.rs` pins the round trip itself (what a put stores and a get reports, over
//! a table of documents). This suite pins the other half of the same rule: the inline policy
//! is a field of its own, held per permission set, so the rest of the record moving under it
//! is reported alongside it and never through it. Every row of the interleaving table is a
//! DIFFERENT neighbouring mutation of the same permission set, and the second table's rows
//! are mutations of a neighbouring permission set, up to and including its deletion.
//!
//! The members each interleaved request carries are the model's own
//! (`smithy_query(query="operation",operation="UpdatePermissionSet,TagResource,AttachManagedPolicyToPermissionSet,AttachCustomerManagedPolicyReferenceToPermissionSet,PutPermissionsBoundaryToPermissionSet",model_path="services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json")`),
//! and the invariance is asserted NON-vacuously: after the interleaving the readers of that
//! other state are called too, and must report the changes the table made.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns, and the calls
//! run against an ORGANIZATION instance, because the permission-set surface is withdrawn on
//! an account instance.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{call, error_code, organization_router};

/// The documents this suite interleaves around. Rows differ along the SIZE of the document
/// (the published minimum of `PermissionSetPolicyDocument`'s `length(min: 1, max: 32768)`
/// up to a multi-statement document), the characters it is built from (ASCII, the tab /
/// newline / carriage return the member's pattern singles out, and the top of its
/// `\u{0020}-\u{00FF}` range) and the number of statements it carries.
fn policy_documents() -> Vec<(&'static str, String)> {
    vec![
        (
            "a document of the minimum length 1",
            "x".to_string(),
        ),
        (
            "a single-statement policy in canonical JSON",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"sts:AssumeRole","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "a three-statement policy laid out with tabs, newlines and high-range characters",
            "{\r\n\t\"Version\": \"2012-10-17\",\r\n\t\"Statement\": [\r\n\t\t\
             {\"Sid\": \"\u{00c9}t\u{00e9}\", \"Effect\": \"Allow\", \"Action\": \"s3:List*\", \
             \"Resource\": \"*\"},\r\n\t\t\
             {\"Effect\": \"Allow\", \"Action\": [\"ec2:Describe*\"], \"Resource\": \"*\"},\r\n\t\t\
             {\"Effect\": \"Deny\", \"Action\": \"iam:*\", \"Resource\": \"*\"}\r\n\t]\r\n}"
                .to_string(),
        ),
    ]
}

/// A `Description` the member's own constraints admit (`length 1..700`, pattern
/// `^[\u{0009}\u{000A}\u{000D}\u{0020}-\u{007E}\u{00A1}-\u{00FF}]*$`).
const NEW_DESCRIPTION: &str = "the description an interleaved update wrote";
/// A `SessionDuration` in the ISO-8601 form the member's pattern requires.
const NEW_SESSION_DURATION: &str = "PT2H";
/// A `RelayState` the member's own pattern admits.
const NEW_RELAY_STATE: &str = "https://example.com/start";
/// An AWS managed policy ARN, in the form `ManagedPolicyArn`'s pattern requires.
const READ_ONLY_POLICY_ARN: &str = "arn:aws:iam::aws:policy/ReadOnlyAccess";
/// A second one, used as the permissions boundary so the boundary is distinguishable from
/// the attachment.
const POWER_USER_POLICY_ARN: &str = "arn:aws:iam::aws:policy/PowerUserAccess";
/// The tag an interleaved `TagResource` writes.
const TAG_KEY: &str = "Team";
const TAG_VALUE: &str = "Platform";

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

/// `GetInlinePolicyForPermissionSet`, as `(status, InlinePolicy or Null)`.
async fn get_policy(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
) -> (u16, Value) {
    let (status, body) = call(
        router,
        "GetInlinePolicyForPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
        }),
    )
    .await;
    if status != 200 {
        return (status, body);
    }
    (status, body["InlinePolicy"].clone())
}

/// A put of `document`, as `(status, body)`.
async fn put_policy(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    document: &str,
) -> (u16, Value) {
    call(
        router,
        "PutInlinePolicyToPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
            "InlinePolicy": document,
        }),
    )
    .await
}

/// One row of the interleaving table: a neighbouring mutation of the SAME permission set,
/// as the operation to call and the body to send.
struct Interleaved {
    label: &'static str,
    operation: &'static str,
    body: Value,
}

/// The neighbouring mutations, in the order they are applied. Rows differ along WHICH part
/// of the permission set's record they move: its mutable members, its tags, its
/// managed-policy attachments, its customer-managed policy references and its permissions
/// boundary — and one row REMOVES what an earlier row added, so a mutation in both
/// directions is covered.
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
            label: "a managed policy attached to the same permission set",
            operation: "AttachManagedPolicyToPermissionSet",
            body: json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "ManagedPolicyArn": READ_ONLY_POLICY_ARN,
            }),
        },
        Interleaved {
            label: "a customer managed policy reference attached to the same permission set",
            operation: "AttachCustomerManagedPolicyReferenceToPermissionSet",
            body: json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "CustomerManagedPolicyReference": {"Name": "Neighbour", "Path": "/apps/"},
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
            label: "the managed policy detached again",
            operation: "DetachManagedPolicyFromPermissionSet",
            body: json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "ManagedPolicyArn": READ_ONLY_POLICY_ARN,
            }),
        },
    ]
}

/// THE RULE: every other part of the permission set's record can move without changing what
/// the inline-policy report answers — asserted for each document of the table, with every
/// interleaving row applied in turn and the report read back after each one.
#[tokio::test]
async fn other_state_of_the_permission_set_never_changes_the_reported_policy() {
    let (router, instance_arn) = organization_router();

    for (index, (label, document)) in policy_documents().into_iter().enumerate() {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Interleaved{index}")).await;
        let (status, body) = put_policy(&router, &instance_arn, &permission_set_arn, &document).await;
        assert_eq!(
            status, 200,
            "case {label:?}: PutInlinePolicyToPermissionSet expected 200, got {status} {body}"
        );

        for row in interleaved_mutations(&instance_arn, &permission_set_arn) {
            let (status, body) = call(&router, row.operation, &row.body).await;
            assert_eq!(
                status, 200,
                "case {label:?} / {:?}: {} expected 200, got {status} {body}",
                row.label, row.operation
            );

            let (status, stored) = get_policy(&router, &instance_arn, &permission_set_arn).await;
            assert_eq!(
                status, 200,
                "case {label:?} / {:?}: the get expected 200, got {status} {stored}",
                row.label
            );
            assert_eq!(
                stored.as_str(),
                Some(document.as_str()),
                "case {label:?} / {:?}: the stored document must be reported unchanged, got \
                 {stored:?}",
                row.label
            );
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
            "case {label:?}: DescribePermissionSet expected 200, got {status} {described}"
        );
        assert_eq!(
            described["PermissionSet"]["Description"].as_str(),
            Some(NEW_DESCRIPTION),
            "case {label:?}: the interleaved update must be reported, got {described}"
        );
        assert_eq!(
            described["PermissionSet"]["SessionDuration"].as_str(),
            Some(NEW_SESSION_DURATION),
            "case {label:?}: the interleaved update must be reported, got {described}"
        );
        assert_eq!(
            described["PermissionSet"]["RelayState"].as_str(),
            Some(NEW_RELAY_STATE),
            "case {label:?}: the interleaved update must be reported, got {described}"
        );

        let (status, boundary) = call(
            &router,
            "GetPermissionsBoundaryForPermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetPermissionsBoundaryForPermissionSet expected 200, got {status} \
             {boundary}"
        );
        assert_eq!(
            boundary["PermissionsBoundary"]["ManagedPolicyArn"].as_str(),
            Some(POWER_USER_POLICY_ARN),
            "case {label:?}: the interleaved boundary put must be reported, got {boundary}"
        );

        let (status, tags) = call(
            &router,
            "ListTagsForResource",
            &json!({"InstanceArn": instance_arn, "ResourceArn": permission_set_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListTagsForResource expected 200, got {status} {tags}"
        );
        assert!(
            tags["Tags"]
                .as_array()
                .is_some_and(|written| written.iter().any(|tag| {
                    tag["Key"].as_str() == Some(TAG_KEY) && tag["Value"].as_str() == Some(TAG_VALUE)
                })),
            "case {label:?}: the interleaved tag must be reported, got {tags}"
        );

        let (status, attached) = call(
            &router,
            "ListManagedPoliciesInPermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListManagedPoliciesInPermissionSet expected 200, got {status} \
             {attached}"
        );
        assert!(
            attached["AttachedManagedPolicies"]
                .as_array()
                .is_some_and(|policies| policies
                    .iter()
                    .all(|policy| policy["Arn"].as_str() != Some(READ_ONLY_POLICY_ARN))),
            "case {label:?}: the detached managed policy must be gone, got {attached}"
        );

        // And the report is STILL the document, after every other reader has been called.
        let (_, stored) = get_policy(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            stored.as_str(),
            Some(document.as_str()),
            "case {label:?}: the document must survive the whole interleaving, got {stored:?}"
        );
    }
}

/// The report belongs to ONE permission set: whatever happens to a NEIGHBOUR of the same
/// instance — a put of a different document, an update, an attachment, even its deletion —
/// the subject reports its own document, and a permission set that never got a put still
/// reports none.
#[tokio::test]
async fn a_neighbouring_permission_set_never_changes_the_reported_policy() {
    let (router, instance_arn) = organization_router();
    let documents = policy_documents();
    let (subject_label, subject_document) = documents[2].clone();

    let subject = create_permission_set(&router, &instance_arn, "Subject").await;
    let neighbour = create_permission_set(&router, &instance_arn, "Neighbour").await;
    // A third permission set that never receives a put: its report must stay absent through
    // everything the other two do.
    let untouched = create_permission_set(&router, &instance_arn, "Untouched").await;

    let (status, body) = put_policy(&router, &instance_arn, &subject, &subject_document).await;
    assert_eq!(
        status, 200,
        "case {subject_label:?}: the subject's put expected 200, got {status} {body}"
    );

    // Rows differ along WHAT is done to the neighbour: each document of the table put on it
    // in turn, then its own interleaved mutations, then its deletion.
    let mut rows: Vec<(String, &'static str, Value)> = Vec::new();
    for (label, document) in &documents {
        rows.push((
            format!("a put of {label} on the neighbour"),
            "PutInlinePolicyToPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": neighbour,
                "InlinePolicy": document,
            }),
        ));
    }
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

        let (status, stored) = get_policy(&router, &instance_arn, &subject).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the subject's get expected 200, got {status} {stored}"
        );
        assert_eq!(
            stored.as_str(),
            Some(subject_document.as_str()),
            "case {label:?}: the subject must keep reporting its own document, got {stored:?}"
        );

        let (status, none) = get_policy(&router, &instance_arn, &untouched).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the untouched get expected 200, got {status} {none}"
        );
        assert_eq!(
            none,
            Value::Null,
            "case {label:?}: a permission set no put named must report no policy, got {none}"
        );
    }

    // The deleted neighbour's record is gone with it, policy and all.
    let (status, body) = get_policy(&router, &instance_arn, &neighbour).await;
    assert_eq!(
        status, 400,
        "the deleted neighbour's get expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "the deleted neighbour's get expected ResourceNotFoundException, got {body}"
    );
}
