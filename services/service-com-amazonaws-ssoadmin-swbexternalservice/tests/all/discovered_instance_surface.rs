// SPDX-License-Identifier: CC0-1.0

//! The instance a client DISCOVERS, and the surface it serves — end to end over the
//! `awsJson1_1` wire.
//!
//! The rule under test: the instance an account whose IAM Identity Center is already
//! enabled hands out through `ListInstances` is its ORGANIZATION instance, so multi-account
//! permissions are served on it with no enabling call first — `DescribeInstance` reports
//! `PermissionSetsEnabled: true`, and every operation of the permission-set,
//! account-assignment and ABAC surface answers from state against that ARN instead of
//! refusing it as an account instance. The instance a caller creates through
//! `CreateInstance` is the instance of a standalone account and keeps that restriction,
//! which `account_instance_restriction.rs` pins.
//!
//! The tables span what the rule generalises over: the REGION the discovery and the calls
//! that follow it are signed for, the SHAPE of the permission set the canonical flow is
//! driven with, and EVERY implemented operation of the surface — not only the
//! `ListInstances` -> `CreatePermissionSet` pair of the report.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, and nothing here writes to the store: the instance under test is the one
//! the API itself hands out. The harness is the one `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{call, call_in_region, error_code, router};

/// The Regions a discovery is signed for. SOURCE: `RegionName`'s modelled pattern
/// `^([a-z]+-){2,3}\d$` — rows span both the two-group and the three-group form it admits,
/// and none of them is the Region the harness signs for by default.
const REGIONS: [&str; 4] = ["us-east-1", "eu-west-2", "ap-southeast-2", "us-gov-east-1"];

/// One shape of permission set the canonical flow is driven with: the `CreatePermissionSet`
/// members beside `InstanceArn`.
struct FlowCase {
    label: &'static str,
    /// `PermissionSetName`: `length(min: 1, max: 32)`, `^[\w+=,.@-]+$`.
    name: String,
    /// `PermissionSetDescription`: `length(min: 1, max: 700)`.
    description: Option<String>,
    /// `Duration`: an ISO-8601 duration, `length(min: 1, max: 100)`.
    session_duration: Option<&'static str>,
    /// The principal the assignment of the flow is granted to.
    principal_type: &'static str,
    principal_id: &'static str,
    /// `TargetId`: `^\d{12}$`.
    account_id: &'static str,
}

/// The rows: they differ along the members the create carries, the published bounds of the
/// name's length, both `PrincipalType` values, both forms the `PrincipalId` pattern admits
/// and the bounds of a 12-digit account id. The report's own case — a bare
/// `(InstanceArn, Name)` create — is the first row.
fn flow_cases() -> Vec<FlowCase> {
    vec![
        FlowCase {
            label: "a bare create, the reported shape of the request",
            name: "DemoSet".to_string(),
            description: None,
            session_duration: None,
            principal_type: "GROUP",
            principal_id: "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
            account_id: "123456789012",
        },
        FlowCase {
            // `PermissionSetName` is `length(min: 1, max: 32)`: one character is its
            // published minimum.
            label: "a name of the published minimum length, with a description",
            name: "A".to_string(),
            description: Some("read-only access for the audit team".to_string()),
            session_duration: Some("PT1H"),
            principal_type: "USER",
            principal_id: "9067f3a1c2-f81d4fae-7dec-11d0-a765-00a0c91e6bf7",
            account_id: "000000000000",
        },
        FlowCase {
            label: "a name of the published maximum length, with every member set",
            name: "n".repeat(32),
            description: Some("d".repeat(700)),
            session_duration: Some("PT12H"),
            principal_type: "USER",
            principal_id: "1b2d3c4f-5a6b-7c8d-9e0f-1a2b3c4d5e6f",
            account_id: "999999999999",
        },
        FlowCase {
            label: "a name spanning the charset the pattern admits",
            name: "ops+team=eu.west-1@x_1".to_string(),
            description: Some("x".to_string()),
            session_duration: Some("P1D"),
            principal_type: "GROUP",
            principal_id: "11111111-2222-4333-8444-555555555555",
            account_id: "123456789012",
        },
    ]
}

impl FlowCase {
    /// The `CreatePermissionSet` body this row sends under `instance_arn`.
    fn create_body(&self, instance_arn: &str) -> Value {
        let mut body = json!({"InstanceArn": instance_arn, "Name": self.name});
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        if let Some(session_duration) = self.session_duration {
            body["SessionDuration"] = json!(session_duration);
        }
        body
    }

    /// The `CreateAccountAssignment` / `DeleteAccountAssignment` body this row sends for
    /// `permission_set_arn`.
    fn assignment_body(&self, instance_arn: &str, permission_set_arn: &str) -> Value {
        json!({
            "InstanceArn": instance_arn,
            "TargetId": self.account_id,
            "TargetType": "AWS_ACCOUNT",
            "PermissionSetArn": permission_set_arn,
            "PrincipalType": self.principal_type,
            "PrincipalId": self.principal_id,
        })
    }
}

/// The `InstanceArn` a client discovers, read the way a client reads it: `ListInstances`
/// signed for `region`, with no prior call having enabled anything.
async fn discovered_instance_arn(router: &MultiServiceRouter, region: &str) -> String {
    let (status, body) = call_in_region(router, region, "ListInstances", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {region:?}: ListInstances expected 200, got {status} {body}"
    );
    body["Instances"][0]["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {region:?}: ListInstances handed out no instance: {body}"))
        .to_string()
}

/// One call that must be SERVED: 200, and so never the account-instance refusal. Answers
/// the response body for the caller to read on.
async fn served(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must be served on the instance ListInstances hands \
         out, got {status} {answer}"
    );
    answer
}

/// The discovered instance reports permission sets ENABLED and serves a create of one,
/// whatever Region the client discovered it from and with no call enabling anything first.
#[tokio::test]
async fn the_discovered_instance_serves_permission_sets_in_every_region() {
    for region in REGIONS {
        // A fresh store per row: the instance under test is the one the account hands out
        // when nothing has yet decided its Identity Center enablement.
        let router = router();
        let instance_arn = discovered_instance_arn(&router, region).await;

        let (status, described) =
            call_in_region(&router, region, "DescribeInstance", &json!({"InstanceArn": instance_arn}))
                .await;
        assert_eq!(
            status, 200,
            "case {region:?}: DescribeInstance expected 200, got {status} {described}"
        );
        assert_eq!(
            described["PermissionSetsEnabled"],
            Value::Bool(true),
            "case {region:?}: the discovered instance must report permission sets enabled, \
             got {:?}",
            described["PermissionSetsEnabled"]
        );
        assert_eq!(
            described["InstanceArn"].as_str(),
            Some(instance_arn.as_str()),
            "case {region:?}: the describe must answer about the discovered instance, got \
             {described}"
        );

        let (status, created) = call_in_region(
            &router,
            region,
            "CreatePermissionSet",
            &json!({"InstanceArn": instance_arn, "Name": "DiscoveredSet"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: CreatePermissionSet on the discovered instance expected 200, \
             got {status} {created}"
        );
        let minted = created["PermissionSet"]["PermissionSetArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {region:?}: no minted PermissionSetArn: {created}"));
        let instance_id = instance_arn.rsplit('/').next().unwrap_or_default();
        assert!(
            minted.starts_with(&format!("arn:aws:sso:::permissionSet/{instance_id}/ps-")),
            "case {region:?}: the minted ARN must sit under the discovered instance, got \
             {minted:?}"
        );
    }
}

/// The canonical client flow, over every row of the table: discover the instance, create a
/// permission set on it, read it back, grant an assignment from it, poll that grant, give
/// it back and delete the set — each step answered from the state the step before wrote.
#[tokio::test]
async fn the_canonical_flow_runs_on_the_discovered_instance() {
    for flow in flow_cases() {
        let case = flow.label;
        let router = router();
        let instance_arn = discovered_instance_arn(&router, "us-east-1").await;

        let created = served(
            &router,
            case,
            "CreatePermissionSet",
            &flow.create_body(&instance_arn),
        )
        .await;
        let permission_set_arn = created["PermissionSet"]["PermissionSetArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no minted PermissionSetArn: {created}"))
            .to_string();

        // ROUND TRIP: the describe answers from the stored record, not from the request.
        let described = served(
            &router,
            case,
            "DescribePermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
        )
        .await;
        assert_eq!(
            described["PermissionSet"], created["PermissionSet"],
            "case {case:?}: the describe must report what the create stored, got {:?} vs {:?}",
            described["PermissionSet"], created["PermissionSet"]
        );
        assert_eq!(
            described["PermissionSet"]["Name"].as_str(),
            Some(flow.name.as_str()),
            "case {case:?}: expected the created Name, got {:?}",
            described["PermissionSet"]["Name"]
        );
        assert_eq!(
            described["PermissionSet"]["Description"].as_str(),
            flow.description.as_deref(),
            "case {case:?}: expected the created Description, got {:?}",
            described["PermissionSet"]["Description"]
        );
        assert_eq!(
            described["PermissionSet"]["SessionDuration"].as_str(),
            flow.session_duration,
            "case {case:?}: expected the created SessionDuration, got {:?}",
            described["PermissionSet"]["SessionDuration"]
        );

        let listed = served(
            &router,
            case,
            "ListPermissionSets",
            &json!({"InstanceArn": instance_arn}),
        )
        .await;
        let arns: Vec<&str> = listed["PermissionSets"]
            .as_array()
            .map(|rows| rows.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert_eq!(
            arns,
            vec![permission_set_arn.as_str()],
            "case {case:?}: the enumeration must report exactly the stored set, got {listed}"
        );

        let assignment = flow.assignment_body(&instance_arn, &permission_set_arn);
        let accepted = served(&router, case, "CreateAccountAssignment", &assignment).await;
        let request_id = accepted["AccountAssignmentCreationStatus"]["RequestId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no RequestId to poll by: {accepted}"))
            .to_string();

        let polled = served(
            &router,
            case,
            "DescribeAccountAssignmentCreationStatus",
            &json!({
                "InstanceArn": instance_arn,
                "AccountAssignmentCreationRequestId": request_id,
            }),
        )
        .await;
        assert_eq!(
            polled["AccountAssignmentCreationStatus"]["RequestId"].as_str(),
            Some(request_id.as_str()),
            "case {case:?}: the poll must report the operation it names, got {polled}"
        );

        let assignments = served(
            &router,
            case,
            "ListAccountAssignments",
            &json!({
                "InstanceArn": instance_arn,
                "AccountId": flow.account_id,
                "PermissionSetArn": permission_set_arn,
            }),
        )
        .await;
        let principals: Vec<&str> = assignments["AccountAssignments"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row["PrincipalId"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            principals,
            vec![flow.principal_id],
            "case {case:?}: the enumeration must report the granted assignment, got \
             {assignments}"
        );

        served(&router, case, "DeleteAccountAssignment", &assignment).await;
        served(
            &router,
            case,
            "DeletePermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
        )
        .await;

        // The delete removed the record, so the read that answered from it is now a
        // missing resource — never the account-instance refusal.
        let (status, gone) = call(
            &router,
            "DescribePermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a deleted permission set is a missing resource, got {status} {gone}"
        );
        assert_eq!(
            error_code(&gone),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {gone}"
        );
    }
}

/// EVERY implemented operation of the permission-set, account-assignment and ABAC surface,
/// against the discovered instance: each one is served, so none of them refuses that ARN as
/// an account instance. The calls run in dependency order — a read follows the write whose
/// record it reports.
#[tokio::test]
async fn every_operation_of_the_surface_is_served_on_the_discovered_instance() {
    let router = router();
    let instance_arn = discovered_instance_arn(&router, "us-east-1").await;
    let case = "the discovered instance";

    let created = served(
        &router,
        case,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": "SurfaceWalk"}),
    )
    .await;
    let permission_set_arn = created["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no minted PermissionSetArn: {created}"))
        .to_string();
    let on_set = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
    });
    let assignment = json!({
        "InstanceArn": instance_arn,
        "TargetId": "123456789012",
        "TargetType": "AWS_ACCOUNT",
        "PermissionSetArn": permission_set_arn,
        "PrincipalType": "GROUP",
        "PrincipalId": "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
    });
    let customer_reference = json!({"Name": "ReadOnlyPolicy", "Path": "/"});
    let abac_configuration = json!({
        "InstanceArn": instance_arn,
        "InstanceAccessControlAttributeConfiguration": {
            "AccessControlAttributes": [
                {"Key": "Department", "Value": {"Source": ["${path:enterprise.department}"]}},
            ],
        },
    });

    // Each row: the operation and the body it takes. Every member beyond the two ARNs is
    // the one the sibling suite of that operation sends, and the order is the one the
    // records require.
    let mut walk: Vec<(&str, Value)> = vec![
        (
            "UpdatePermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "Description": "the set this walk drives",
                "SessionDuration": "PT2H",
                "RelayState": "https://example.com/console",
            }),
        ),
        (
            "PutInlinePolicyToPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "InlinePolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
            }),
        ),
        ("GetInlinePolicyForPermissionSet", on_set.clone()),
        (
            "AttachManagedPolicyToPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "ManagedPolicyArn": "arn:aws:iam::aws:policy/ReadOnlyAccess",
            }),
        ),
        ("ListManagedPoliciesInPermissionSet", on_set.clone()),
        (
            "DetachManagedPolicyFromPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "ManagedPolicyArn": "arn:aws:iam::aws:policy/ReadOnlyAccess",
            }),
        ),
        (
            "AttachCustomerManagedPolicyReferenceToPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "CustomerManagedPolicyReference": customer_reference,
            }),
        ),
        (
            "ListCustomerManagedPolicyReferencesInPermissionSet",
            on_set.clone(),
        ),
        (
            "DetachCustomerManagedPolicyReferenceFromPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "CustomerManagedPolicyReference": customer_reference,
            }),
        ),
        (
            "PutPermissionsBoundaryToPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "PermissionsBoundary": {
                    "ManagedPolicyArn": "arn:aws:iam::aws:policy/PowerUserAccess",
                },
            }),
        ),
        ("GetPermissionsBoundaryForPermissionSet", on_set.clone()),
        ("DeletePermissionsBoundaryFromPermissionSet", on_set.clone()),
        ("CreateAccountAssignment", assignment.clone()),
        (
            "ListAccountAssignments",
            json!({
                "InstanceArn": instance_arn,
                "AccountId": "123456789012",
                "PermissionSetArn": permission_set_arn,
            }),
        ),
        ("ListAccountsForProvisionedPermissionSet", on_set.clone()),
        ("DeleteAccountAssignment", assignment),
        (
            "DescribeAccountAssignmentDeletionStatus",
            // The `RequestId` the delete above answers with is not known until the walk
            // reaches it, so this row carries the instance alone and the loop fills the
            // request id in from that answer.
            json!({"InstanceArn": instance_arn}),
        ),
        (
            "CreateInstanceAccessControlAttributeConfiguration",
            abac_configuration.clone(),
        ),
        (
            "DescribeInstanceAccessControlAttributeConfiguration",
            json!({"InstanceArn": instance_arn}),
        ),
        (
            "UpdateInstanceAccessControlAttributeConfiguration",
            abac_configuration,
        ),
        (
            "DeleteInstanceAccessControlAttributeConfiguration",
            json!({"InstanceArn": instance_arn}),
        ),
    ];
    walk.push(("DeletePermissionSet", on_set));

    // The one request id this walk cannot write down in advance: the delete's own.
    let mut deletion_request_id: Option<String> = None;
    for (operation, mut body) in walk {
        if operation == "DescribeAccountAssignmentDeletionStatus" {
            let request_id = deletion_request_id
                .as_deref()
                .unwrap_or_else(|| panic!("the delete answered no RequestId to poll by"));
            body["AccountAssignmentDeletionRequestId"] = json!(request_id);
        }
        let answer = served(&router, case, operation, &body).await;
        if operation == "DeleteAccountAssignment" {
            deletion_request_id = answer["AccountAssignmentDeletionStatus"]["RequestId"]
                .as_str()
                .map(str::to_string);
        }
    }
}
