// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the resolution of a caller-supplied `roleArn` against this
//! emulator's own IAM records.
//!
//! The rule under test: Entity Resolution does the work a resource describes ITSELF,
//! acting as the role the request hands it ("Entity Resolution assumes this role to
//! create resources on your behalf as part of workflow execution"), so every write that
//! would STORE a role resolves it BEFORE it stores anything and refuses the request with
//! the modelled `AccessDeniedException` (HTTP 403) — persisting nothing — when IAM
//! ANSWERED that the service cannot assume it: this account holds no such role, or the
//! trust document the role carries does not admit the Entity Resolution service
//! principal. A role nothing can be shown about is NOT refused, which is why every other
//! suite in this crate — each of which mounts Entity Resolution alone, with no IAM to ask
//! — still creates workflows whose `roleArn` no IAM record backs.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: what the
//! role's trust document says (which principal, which action, which effect, which casing,
//! how many statements, whether it is a policy at all), whether the account holds the role
//! at all, which OPERATION carries the role, and — where the member is optional — whether
//! the request names a role at all. Every call goes over the wire through the harness the
//! `schema_mapping_workflow_lifecycle` suite owns.

use serde_json::{json, Value};
use service_com_amazonaws_entityresolution_awsveniceservice::state::SERVICE_PRINCIPAL;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{
    name_key, role_arn, ComAmazonawsIamAwsidentitymanagementv20100508State as IamState,
    DEFAULT_PATH,
};

use crate::id_mapping_workflow_lifecycle::{rule_based, ID_NAMESPACE_TYPES};
use crate::schema_mapping_workflow_lifecycle::{
    account_id, assert_error_shape, glue_table_arn, string_member, Emulator, Reply,
};

/// The `message` template a refused role is reported with: AWS names the role the caller
/// passed, which is what tells a caller WHICH value to fix. Written out here rather than
/// read from the crate's own helper, so this suite pins the wording a client sees.
fn role_not_assumable(role: &str) -> String {
    format!("Exception in assuming the passed role: {role}")
}

/// Seed `role_name` into the shared engine's IAM store carrying `trust` as its
/// `AssumeRolePolicyDocument` — the record `CreateRole` writes — and answer the ARN IAM
/// holds it under.
fn seed_role(emulator: &Emulator, role_name: &str, trust: Option<String>) -> String {
    let state = emulator.engine().state::<IamState>();
    let mut store = state.lock();
    let arn = role_arn(DEFAULT_PATH, role_name);
    store.roles.insert(
        name_key(role_name),
        Role {
            path: DEFAULT_PATH.to_string(),
            role_name: role_name.to_string(),
            role_id: format!("AROA{}", role_name.to_uppercase()),
            arn: arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(1_700_000_000),
            assume_role_policy_document: trust,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    arn
}

/// A trust document with one statement: `effect` for `principal` performing `action`.
fn trust(effect: &str, principal: &str, action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "Service": principal },
            "Action": action,
        }],
    })
    .to_string()
}

/// The trust document a role this service can assume carries.
fn service_trust() -> String {
    trust("Allow", SERVICE_PRINCIPAL, "sts:AssumeRole")
}

/// A `CreateSchemaMapping` body: the schema mapping a matching workflow's input source
/// names. It carries no role of its own.
fn schema_request(name: &str) -> Value {
    json!({
        "schemaName": name,
        "mappedInputFields": [
            { "fieldName": "id", "type": "UNIQUE_ID" },
            { "fieldName": "email", "type": "EMAIL_ADDRESS" },
        ],
    })
}

/// A `CreateMatchingWorkflow` body naming `schema` and `role`.
fn matching_workflow_request(name: &str, schema: &str, role: &str) -> Value {
    json!({
        "workflowName": name,
        "inputSourceConfig": [
            { "inputSourceARN": glue_table_arn("customers"), "schemaName": schema },
        ],
        "outputSourceConfig": [{
            "outputS3Path": "s3://er-output/roles",
            "output": [{ "name": "id" }],
        }],
        "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
        "roleArn": role,
    })
}

/// The same body without the `workflowName`, which `UpdateMatchingWorkflow` takes as its
/// `@httpLabel` instead.
fn matching_workflow_update(schema: &str, role: &str) -> Value {
    let mut request = matching_workflow_request("unused", schema, role);
    request
        .as_object_mut()
        .expect("the request body is a JSON object")
        .remove("workflowName");
    request
}

/// A `CreateIdMappingWorkflow` body naming `role`, whose `roleArn` member is OPTIONAL
/// (modelled `default: ""`), so `None` sends no role at all.
fn id_mapping_workflow_request(name: Option<&str>, role: Option<&str>) -> Value {
    let mut request = json!({
        "inputSourceConfig": [{
            "inputSourceARN": glue_table_arn("customers"),
            "schemaName": "customers",
            "type": ID_NAMESPACE_TYPES[0],
        }],
        "idMappingTechniques": rule_based(
            "SOURCE",
            "MANY_TO_MANY",
            "ONE_SOURCE_TO_ONE_TARGET",
            Some(json!([{ "ruleName": "by email", "matchingKeys": ["email"] }])),
        ),
    });
    let object = request
        .as_object_mut()
        .expect("the request body is a JSON object");
    if let Some(name) = name {
        object.insert("workflowName".to_string(), json!(name));
    }
    if let Some(role) = role {
        object.insert("roleArn".to_string(), json!(role));
    }
    request
}

/// A `CreateIdNamespace` body naming `role`, whose `roleArn` member is also optional.
fn id_namespace_request(name: Option<&str>, role: Option<&str>) -> Value {
    let mut request = json!({ "type": ID_NAMESPACE_TYPES[0] });
    let object = request
        .as_object_mut()
        .expect("the request body is a JSON object");
    if let Some(name) = name {
        object.insert("idNamespaceName".to_string(), json!(name));
    }
    if let Some(role) = role {
        object.insert("roleArn".to_string(), json!(role));
    }
    request
}

/// What one row asks of the role resolution.
struct Row {
    /// The case name every assertion of the row reports.
    case: &'static str,
    /// The role the account holds under the row's role name, as its trust document — or
    /// `None` for a row whose role the account does not hold at all.
    held: Option<Option<String>>,
    /// Whether the write must be admitted.
    admitted: bool,
}

/// The trust-document table, which every suite below spans.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a trust document admitting the Entity Resolution principal",
            held: Some(Some(service_trust())),
            admitted: true,
        },
        Row {
            case: "a service wildcard in the action",
            held: Some(Some(trust("Allow", SERVICE_PRINCIPAL, "sts:*"))),
            admitted: true,
        },
        Row {
            case: "the principal and the action named in another casing",
            held: Some(Some(trust(
                "Allow",
                "ENTITYRESOLUTION.amazonaws.com",
                "STS:AssumeRole",
            ))),
            admitted: true,
        },
        Row {
            case: "a document whose second statement admits the principal",
            held: Some(Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {
                            "Effect": "Allow",
                            "Principal": { "Service": "glue.amazonaws.com" },
                            "Action": "sts:AssumeRole",
                        },
                        {
                            "Effect": "Allow",
                            "Principal": { "Service": SERVICE_PRINCIPAL },
                            "Action": "sts:AssumeRole",
                        },
                    ],
                })
                .to_string(),
            )),
            admitted: true,
        },
        Row {
            case: "a trust document admitting another service only",
            held: Some(Some(trust(
                "Allow",
                "lambda.amazonaws.com",
                "sts:AssumeRole",
            ))),
            admitted: false,
        },
        Row {
            case: "a trust document naming another action",
            held: Some(Some(trust("Allow", SERVICE_PRINCIPAL, "s3:GetObject"))),
            admitted: false,
        },
        Row {
            case: "a trust document explicitly denying the principal",
            held: Some(Some(trust("Deny", SERVICE_PRINCIPAL, "sts:AssumeRole"))),
            admitted: false,
        },
        Row {
            case: "a trust document carrying no statement",
            held: Some(Some(json!({ "Version": "2012-10-17" }).to_string())),
            admitted: false,
        },
        Row {
            case: "a trust document that is not a policy at all",
            held: Some(Some("this is not a policy".to_string())),
            admitted: false,
        },
        Row {
            case: "a role carrying no trust document",
            held: Some(None),
            admitted: false,
        },
        Row {
            case: "a role this account does not hold",
            held: None,
            admitted: false,
        },
    ]
}

/// The role the row's write names: seeded with its trust document, or — for the row whose
/// role the account does not hold — the ARN IAM would hold it under, seeded nowhere.
fn role_of(emulator: &Emulator, row: &Row, role_name: &str) -> String {
    match &row.held {
        Some(trust) => seed_role(emulator, role_name, trust.clone()),
        None => role_arn(DEFAULT_PATH, role_name),
    }
}

/// Assert the row's verdict on `reply`, and that a refusal names the role it refused.
fn assert_verdict(row: &Row, reply: &Reply, role: &str) {
    if row.admitted {
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected the write to be admitted, got {reply:?}",
            row.case
        );
        return;
    }
    assert_error_shape(row.case, reply, 403, "AccessDeniedException");
    assert_eq!(
        string_member(&reply.body, "message"),
        role_not_assumable(role),
        "case {:?}: expected the refusal to name the role it refused, got {:?}",
        row.case,
        reply.body
    );
}

/// Every form of trust document, and every absence of one, decides whether a matching
/// workflow can be created with that role — and a refusal persists no workflow.
#[tokio::test]
async fn a_matching_workflow_is_created_only_for_a_role_iam_shows_the_service_can_assume() {
    let emulator = Emulator::with_iam();
    let seeded = emulator.create_schema_mapping(&schema_request("customers")).await;
    assert_eq!(
        seeded.status, 200,
        "seeding the referenced schema mapping failed: {seeded:?}"
    );

    for (index, row) in rows().iter().enumerate() {
        let role = role_of(&emulator, row, &format!("matching-role-{index}"));
        let workflow = format!("matching-{index}");

        let reply = emulator
            .create_matching_workflow(&matching_workflow_request(&workflow, "customers", &role))
            .await;
        assert_verdict(row, &reply, &role);

        let read = emulator.get_matching_workflow(&workflow).await;
        if row.admitted {
            // Round trip: the workflow the create admitted is readable, carrying the
            // role it was created with.
            assert_eq!(
                string_member(&read.body, "roleArn"),
                role,
                "case {:?}: expected the stored workflow to carry the role it was created \
                 with, got {read:?}",
                row.case
            );
        } else {
            assert_error_shape(row.case, &read, 404, "ResourceNotFoundException");
        }
    }
}

/// The SAME decision on every other write that stores a role: the two updates that
/// REPLACE one, and the two other creates that take one. A refused update leaves the
/// stored resource with the role it had.
#[tokio::test]
async fn every_write_that_stores_a_role_is_held_to_the_same_resolution() {
    let emulator = Emulator::with_iam();
    let seeded = emulator.create_schema_mapping(&schema_request("customers")).await;
    assert_eq!(
        seeded.status, 200,
        "seeding the referenced schema mapping failed: {seeded:?}"
    );
    let assumable = seed_role(&emulator, "baseline-role", Some(service_trust()));

    for (index, row) in rows().iter().enumerate() {
        let role = role_of(&emulator, row, &format!("every-write-role-{index}"));

        // `UpdateMatchingWorkflow`, over a workflow created with an assumable role.
        let workflow = format!("updated-matching-{index}");
        let created = emulator
            .create_matching_workflow(&matching_workflow_request(
                &workflow,
                "customers",
                &assumable,
            ))
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: seeding {workflow:?} failed: {created:?}",
            row.case
        );
        let updated = emulator
            .update_matching_workflow(&workflow, &matching_workflow_update("customers", &role))
            .await;
        assert_verdict(row, &updated, &role);
        let read = emulator.get_matching_workflow(&workflow).await;
        let expected_role = if row.admitted { &role } else { &assumable };
        assert_eq!(
            string_member(&read.body, "roleArn"),
            *expected_role,
            "case {:?}: expected the stored workflow to carry {expected_role:?}, got {read:?}",
            row.case
        );

        // `CreateIdMappingWorkflow`, whose `roleArn` is optional but present here.
        let id_mapping = format!("id-mapping-{index}");
        let reply = emulator
            .call(
                "POST",
                "/idmappingworkflows",
                Some(&id_mapping_workflow_request(Some(&id_mapping), Some(&role))),
            )
            .await;
        assert_verdict(row, &reply, &role);

        // `UpdateIdMappingWorkflow`, over a workflow created with the assumable role.
        let id_mapping_update = format!("id-mapping-update-{index}");
        let created = emulator
            .call(
                "POST",
                "/idmappingworkflows",
                Some(&id_mapping_workflow_request(
                    Some(&id_mapping_update),
                    Some(&assumable),
                )),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: seeding {id_mapping_update:?} failed: {created:?}",
            row.case
        );
        let updated = emulator
            .call(
                "PUT",
                &format!("/idmappingworkflows/{id_mapping_update}"),
                Some(&id_mapping_workflow_request(None, Some(&role))),
            )
            .await;
        assert_verdict(row, &updated, &role);
        let read = emulator
            .call(
                "GET",
                &format!("/idmappingworkflows/{id_mapping_update}"),
                None,
            )
            .await;
        assert_eq!(
            string_member(&read.body, "roleArn"),
            *expected_role,
            "case {:?}: expected the stored ID mapping workflow to carry {expected_role:?}, \
             got {read:?}",
            row.case
        );

        // `CreateIdNamespace` and `UpdateIdNamespace`, the other pair that stores a role.
        let namespace = format!("id-namespace-{index}");
        let reply = emulator
            .create_id_namespace(&id_namespace_request(Some(&namespace), Some(&role)))
            .await;
        assert_verdict(row, &reply, &role);

        let namespace_update = format!("id-namespace-update-{index}");
        let created = emulator
            .create_id_namespace(&id_namespace_request(
                Some(&namespace_update),
                Some(&assumable),
            ))
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: seeding {namespace_update:?} failed: {created:?}",
            row.case
        );
        let updated = emulator
            .update_id_namespace(&namespace_update, &id_namespace_request(None, Some(&role)))
            .await;
        assert_verdict(row, &updated, &role);
        let read = emulator.get_id_namespace(&namespace_update).await;
        assert_eq!(
            read.body.get("roleArn").and_then(Value::as_str),
            Some(expected_role.as_str()),
            "case {:?}: expected the stored namespace to carry {expected_role:?}, got {read:?}",
            row.case
        );
    }
}

/// What the role member NAMES decides whether IAM can be asked about it at all, so the
/// rows here differ along the FORM of the value rather than along a trust document: this
/// account's role ARN resolves, a role of another account does not (another account's IAM
/// is not this emulator's to ask), and an optional member the request omits hands the
/// service no role to assume.
#[tokio::test]
async fn a_role_is_resolved_only_where_this_accounts_iam_can_answer_for_it() {
    let emulator = Emulator::with_iam();
    let seeded = emulator.create_schema_mapping(&schema_request("customers")).await;
    assert_eq!(
        seeded.status, 200,
        "seeding the referenced schema mapping failed: {seeded:?}"
    );
    let assumable = seed_role(&emulator, "form-service-role", Some(service_trust()));
    let unassumable = seed_role(
        &emulator,
        "form-lambda-role",
        Some(trust("Allow", "lambda.amazonaws.com", "sts:AssumeRole")),
    );

    // (case, the `roleArn` the request carries, whether the write must be admitted)
    let forms: Vec<(&str, String, bool)> = vec![
        (
            "this account's ARN of a role that trusts the service",
            assumable.clone(),
            true,
        ),
        (
            "the same role under a service-role path, which IAM holds by name",
            format!("arn:aws:iam::{}:role/service-role/form-service-role", account_id()),
            true,
        ),
        (
            "this account's ARN of a role that trusts another service",
            unassumable.clone(),
            false,
        ),
        (
            "this account's ARN of a role it does not hold",
            role_arn(DEFAULT_PATH, "form-absent-role"),
            false,
        ),
        (
            // Another account's IAM is not this emulator's to ask, so nothing is shown
            // about the role and the write goes ahead.
            "another account's role ARN, which this IAM cannot be asked about",
            "arn:aws:iam::123456789012:role/their-role".to_string(),
            true,
        ),
        (
            // `CreateMatchingWorkflow$roleArn` is modelled as a plain required `String`
            // with no pattern, so a value that is no ARN reaches the handler and names no
            // IAM role this emulator can be asked about.
            "a value that is not an ARN at all",
            "form-service-role".to_string(),
            true,
        ),
    ];

    for (index, (case, role, admitted)) in forms.iter().enumerate() {
        let workflow = format!("form-{index}");
        let reply = emulator
            .create_matching_workflow(&matching_workflow_request(&workflow, "customers", role))
            .await;
        if *admitted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the write to be admitted, got {reply:?}"
            );
            continue;
        }
        assert_eq!(
            reply.status, 403,
            "case {case:?}: expected the write to be refused, got {reply:?}"
        );
        assert_eq!(
            string_member(&reply.body, "message"),
            role_not_assumable(role),
            "case {case:?}: expected the refusal to name the role it refused, got {:?}",
            reply.body
        );
    }

    // The two writes whose `roleArn` is OPTIONAL: a request that names NO role hands the
    // service nothing to assume, so there is nothing to resolve and nothing to refuse.
    let no_role_mapping = emulator
        .call(
            "POST",
            "/idmappingworkflows",
            Some(&id_mapping_workflow_request(Some("no-role-mapping"), None)),
        )
        .await;
    assert_eq!(
        no_role_mapping.status, 200,
        "a request naming no role must be admitted, got {no_role_mapping:?}"
    );
    let no_role_namespace = emulator
        .create_id_namespace(&id_namespace_request(Some("no-role-namespace"), None))
        .await;
    assert_eq!(
        no_role_namespace.status, 200,
        "a request naming no role must be admitted, got {no_role_namespace:?}"
    );
}

/// With NO IAM mounted there is nothing to ask, so nothing is shown about any role and
/// nothing is refused — the property every other suite in this crate depends on.
#[tokio::test]
async fn with_no_iam_to_ask_no_role_is_refused() {
    let emulator = Emulator::new();
    let seeded = emulator.create_schema_mapping(&schema_request("customers")).await;
    assert_eq!(
        seeded.status, 200,
        "seeding the referenced schema mapping failed: {seeded:?}"
    );

    // The same role values the suite above refuses once IAM can answer for them.
    for (case, role) in [
        ("this account's ARN of a role no IAM holds", role_arn(DEFAULT_PATH, "unmounted-role")),
        ("another account's role ARN", "arn:aws:iam::123456789012:role/their-role".to_string()),
    ] {
        let reply = emulator
            .create_matching_workflow(&matching_workflow_request(
                &format!("unmounted-{}", role.len()),
                "customers",
                &role,
            ))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: with no IAM to ask the write must be admitted, got {reply:?}"
        );
    }
}
