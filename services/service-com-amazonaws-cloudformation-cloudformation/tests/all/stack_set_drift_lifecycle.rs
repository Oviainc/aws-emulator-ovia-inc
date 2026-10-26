// SPDX-License-Identifier: CC0-1.0

//! The StackSet surface, end to end over the awsQuery wire: `CreateStackSet`,
//! `CreateStackInstances`, `DescribeStackSet`, `DescribeStackInstance`,
//! `DescribeStackSetOperation`, `DetectStackSetDrift` and
//! `ListStackInstanceResourceDrifts`.
//!
//! The rule under test: a StackSet is a persisted record of the template and configuration
//! `CreateStackSet` was given; a stack INSTANCE is a stack really deployed from that
//! template in one target account and Region; and a drift detection over the StackSet is
//! the comparison of each instance's resources against that template, recorded under the
//! `OperationId` the call answered — so `DescribeStackSet` reports the last COMPLETED
//! detection's `DriftStatus` and counts, and `ListStackInstanceResourceDrifts` reports that
//! detection's per-resource rows for one instance and refuses an operation id the StackSet
//! holds no record for.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out) through
//! the wire harness, so what it asserts is what a client driving StackSets sees. The table
//! spans the class the rule generalises over: both template spellings (JSON and YAML), a
//! StackSet with and without parameters, tags and a description, the two permission models,
//! and a drift detection over an instance whose resource was changed out of band beside one
//! whose resources were left alone.

use service_com_amazonaws_cloudformation_cloudformation::state::stack_sets::{
    drift_detection_status, operation_action, operation_status, permission_model,
    stack_set_status,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    resource_drift_status, DRIFT_STATUS_DRIFTED, DRIFT_STATUS_IN_SYNC, DRIFT_STATUS_NOT_CHECKED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, member, router, sns, wait_for_the_transition, xml_blocks, xml_text, xml_texts,
    ACCOUNT_ID, REGION,
};

/// One row of the table: the StackSet to create, and what a read of it must report back.
struct StackSetCase {
    case: &'static str,
    stack_set_name: &'static str,
    template: String,
    description: Option<&'static str>,
    permission_model: Option<&'static str>,
    /// `Parameters` the create supplies, as `(ParameterKey, ParameterValue)`.
    parameters: Vec<(&'static str, &'static str)>,
    /// `Tags` the create supplies, as `(Key, Value)`.
    tags: Vec<(&'static str, &'static str)>,
    /// The logical id of the topic each instance of this StackSet deploys.
    topic_logical_id: &'static str,
    /// The `DisplayName` the template declares for that topic — the expected configuration
    /// a drift check compares against.
    declared_display_name: &'static str,
}

/// The class the rule covers: any template, in either spelling, under either permission
/// model, with any parameters, tags and description.
fn stack_set_cases() -> Vec<StackSetCase> {
    vec![
        StackSetCase {
            case: "json template, self-managed, no parameters or tags",
            stack_set_name: "plain-set",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
                 "Properties":{"DisplayName":"plain-display"}}}}"#
                .to_string(),
            description: None,
            permission_model: None,
            parameters: vec![],
            tags: vec![],
            topic_logical_id: "Topic",
            declared_display_name: "plain-display",
        },
        StackSetCase {
            case: "yaml template with a parameter, a description and tags, self-managed",
            stack_set_name: "yaml-set",
            template: concat!(
                "Parameters:\n",
                "  Shown:\n",
                "    Type: String\n",
                "Resources:\n",
                "  Announcer:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      DisplayName: !Ref Shown\n",
            )
            .to_string(),
            description: Some("a described stack set"),
            permission_model: Some(permission_model::SELF_MANAGED),
            parameters: vec![("Shown", "parameterised-display")],
            tags: vec![("owner", "platform"), ("tier", "test")],
            topic_logical_id: "Announcer",
            declared_display_name: "parameterised-display",
        },
        StackSetCase {
            case: "json template under service-managed permissions",
            stack_set_name: "managed-set",
            template: r#"{"Resources":{"Broadcast":{"Type":"AWS::SNS::Topic",
                 "Properties":{"DisplayName":"managed-display"}}}}"#
                .to_string(),
            description: Some("service managed"),
            permission_model: Some(permission_model::SERVICE_MANAGED),
            parameters: vec![],
            tags: vec![("owner", "platform")],
            topic_logical_id: "Broadcast",
            declared_display_name: "managed-display",
        },
    ]
}

/// `CreateStackSet` for one row.
async fn create_stack_set(router: &MultiServiceRouter, case: &StackSetCase) -> (u16, String) {
    let mut members = vec![
        member("StackSetName", case.stack_set_name),
        member("TemplateBody", &case.template),
    ];
    if let Some(description) = case.description {
        members.push(member("Description", description));
    }
    if let Some(model) = case.permission_model {
        members.push(member("PermissionModel", model));
    }
    for (index, (key, value)) in case.parameters.iter().enumerate() {
        let n = index + 1;
        members.push(member(&format!("Parameters.member.{n}.ParameterKey"), key));
        members.push(member(&format!("Parameters.member.{n}.ParameterValue"), value));
    }
    for (index, (key, value)) in case.tags.iter().enumerate() {
        let n = index + 1;
        members.push(member(&format!("Tags.member.{n}.Key"), key));
        members.push(member(&format!("Tags.member.{n}.Value"), value));
    }
    call(router, "CreateStackSet", &members).await
}

/// `CreateStackInstances` in one account and one Region.
async fn create_stack_instances(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    account: &str,
    region: &str,
) -> (u16, String) {
    call(
        router,
        "CreateStackInstances",
        &[
            member("StackSetName", stack_set_name),
            member("Accounts.member.1", account),
            member("Regions.member.1", region),
        ],
    )
    .await
}

/// `DescribeStackSetOperation`, answered with the status it reports.
async fn operation_status_of(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    operation_id: &str,
) -> (u16, String, Option<String>) {
    let (status, body) = call(
        router,
        "DescribeStackSetOperation",
        &[
            member("StackSetName", stack_set_name),
            member("OperationId", operation_id),
        ],
    )
    .await;
    let reported = xml_text(&body, "Status");
    (status, body, reported)
}

/// The `StackId` the instance in `account`/`region` reports, read through
/// `DescribeStackInstance`.
async fn instance_stack_id(
    case: &str,
    router: &MultiServiceRouter,
    stack_set_name: &str,
    account: &str,
    region: &str,
) -> String {
    let (status, body) = call(
        router,
        "DescribeStackInstance",
        &[
            member("StackSetName", stack_set_name),
            member("StackInstanceAccount", account),
            member("StackInstanceRegion", region),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeStackInstance to succeed, got {status} {body}"
    );
    xml_text(&body, "StackId").unwrap_or_else(|| {
        panic!("case {case:?}: DescribeStackInstance answered no StackId: {body}")
    })
}

/// The physical id of one resource of a stack, read through `DescribeStackResources`.
async fn physical_id_of(
    case: &str,
    router: &MultiServiceRouter,
    stack_id: &str,
    logical_id: &str,
) -> String {
    let (status, body) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", stack_id),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeStackResources to succeed, got {status} {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_else(|| {
        panic!("case {case:?}: DescribeStackResources answered no PhysicalResourceId: {body}")
    })
}

/// One `StackInstanceResourceDriftsSummary` as the response carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireResourceDrift {
    logical_resource_id: Option<String>,
    resource_type: Option<String>,
    drift_status: Option<String>,
    property_paths: Vec<String>,
}

/// Every drift row in a `ListStackInstanceResourceDrifts` response, in wire order.
///
/// Read as PARALLEL member lists rather than by splitting the `Summaries` block into its
/// `<member>` elements: a row's `PropertyDifferences` is itself a list of `<member>`s, so a
/// nesting-naive split would cut the row short. Every row reports the three required
/// members, so the lists line up one-to-one; the property paths belong to the single row a
/// response carries here.
fn wire_resource_drifts(xml: &str) -> Vec<WireResourceDrift> {
    let logical_ids = xml_texts(xml, "LogicalResourceId");
    let resource_types = xml_texts(xml, "ResourceType");
    let drift_statuses = xml_texts(xml, "StackResourceDriftStatus");
    let property_paths = xml_texts(xml, "PropertyPath");
    logical_ids
        .into_iter()
        .enumerate()
        .map(|(index, logical_resource_id)| WireResourceDrift {
            logical_resource_id: Some(logical_resource_id),
            resource_type: resource_types.get(index).cloned(),
            drift_status: drift_statuses.get(index).cloned(),
            property_paths: property_paths.clone(),
        })
        .collect()
}

/// `ListStackInstanceResourceDrifts` for one instance under one operation id.
async fn list_resource_drifts(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    operation_id: &str,
    account: &str,
    region: &str,
) -> (u16, String) {
    call(
        router,
        "ListStackInstanceResourceDrifts",
        &[
            member("StackSetName", stack_set_name),
            member("OperationId", operation_id),
            member("StackInstanceAccount", account),
            member("StackInstanceRegion", region),
        ],
    )
    .await
}

/// The `StackSet` block one `DescribeStackSet` call reports.
async fn describe_stack_set(
    case: &str,
    router: &MultiServiceRouter,
    stack_set_name: &str,
) -> String {
    let (status, body) = call(
        router,
        "DescribeStackSet",
        &[member("StackSetName", stack_set_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeStackSet to succeed, got {status} {body}"
    );
    xml_blocks(&body, "StackSet")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: DescribeStackSet answered no StackSet: {body}"))
}

#[tokio::test]
async fn a_stack_set_is_read_back_exactly_as_it_was_created() {
    let router = router();

    for case in stack_set_cases() {
        let (status, body) = create_stack_set(&router, &case).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStackSet to succeed, got {status} {body}",
            case.case
        );
        let stack_set_id = xml_text(&body, "StackSetId").unwrap_or_else(|| {
            panic!(
                "case {:?}: CreateStackSet answered no StackSetId: {body}",
                case.case
            )
        });
        assert!(
            stack_set_id.starts_with(&format!("{}:", case.stack_set_name)),
            "case {:?}: expected the StackSetId to carry the StackSet's name, got \
             {stack_set_id:?}",
            case.case
        );

        // The round trip: every member the create supplied is read back off the record.
        let described = describe_stack_set(case.case, &router, case.stack_set_name).await;
        assert_eq!(
            xml_text(&described, "StackSetId").as_deref(),
            Some(stack_set_id.as_str()),
            "case {:?}: expected the described StackSet to carry the id the create answered, \
             got {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "StackSetName").as_deref(),
            Some(case.stack_set_name),
            "case {:?}: expected the described StackSet to carry its name, got {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "Status").as_deref(),
            Some(stack_set_status::ACTIVE),
            "case {:?}: expected a freshly created StackSet to be {}, got {described}",
            case.case,
            stack_set_status::ACTIVE
        );
        assert_eq!(
            xml_text(&described, "TemplateBody").as_deref(),
            Some(case.template.as_str()),
            "case {:?}: expected the template the create supplied, got {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "Description").as_deref(),
            case.description,
            "case {:?}: expected the description the create supplied, got {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "PermissionModel").as_deref(),
            // "By default, SELF-MANAGED is specified."
            Some(case.permission_model.unwrap_or(permission_model::SELF_MANAGED)),
            "case {:?}: expected the permission model in force, got {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "StackSetARN").as_deref(),
            Some(
                format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stackset/{stack_set_id}")
                    .as_str()
            ),
            "case {:?}: expected the StackSet's own ARN, got {described}",
            case.case
        );
        let parameter_keys = xml_texts(&described, "ParameterKey");
        let expected_keys: Vec<String> = case
            .parameters
            .iter()
            .map(|(key, _)| (*key).to_string())
            .collect();
        assert_eq!(
            parameter_keys, expected_keys,
            "case {:?}: expected the parameters the create supplied, got {parameter_keys:?}",
            case.case
        );
        let tag_keys = xml_texts(&described, "Key");
        let expected_tag_keys: Vec<String> = case
            .tags
            .iter()
            .map(|(key, _)| (*key).to_string())
            .collect();
        assert_eq!(
            tag_keys, expected_tag_keys,
            "case {:?}: expected the tags the create supplied, got {tag_keys:?}",
            case.case
        );
        // "NOT_CHECKED: CloudFormation hasn't checked the StackSet for drift", and no
        // instant at all — "this value will be NULL for any StackSet that drift detection
        // hasn't yet been performed on".
        assert_eq!(
            xml_text(&described, "DriftStatus").as_deref(),
            Some(DRIFT_STATUS_NOT_CHECKED),
            "case {:?}: expected no drift found before a detection has run, got {described}",
            case.case
        );
        assert!(
            xml_text(&described, "LastDriftCheckTimestamp").is_none()
                && xml_text(&described, "DriftDetectionStatus").is_none(),
            "case {:?}: expected no detection instant or status before one has run, got \
             {described}",
            case.case
        );
    }

    // "The name must be unique in the Region where you create your StackSet": the second
    // create of the same name is the modelled refusal, and the first StackSet is untouched.
    let first = &stack_set_cases()[0];
    let (status, body) = create_stack_set(&router, first).await;
    assert_eq!(
        status, 409,
        "case {:?}: expected a duplicate name to be refused with 409, got {status} {body}",
        "a second create of the same name"
    );
    assert!(
        body.contains("NameAlreadyExistsException"),
        "case {:?}: expected NameAlreadyExistsException, got {body}",
        "a second create of the same name"
    );
}

#[tokio::test]
async fn a_stack_set_with_no_template_source_is_refused() {
    let router = router();
    for (case, members) in [
        (
            "neither TemplateBody nor TemplateURL",
            vec![member("StackSetName", "no-template")],
        ),
        (
            "both TemplateBody and TemplateURL",
            vec![
                member("StackSetName", "two-templates"),
                member("TemplateBody", r#"{"Resources":{}}"#),
                member("TemplateURL", "https://example.test/t.json"),
            ],
        ),
    ] {
        let (status, body) = call(&router, "CreateStackSet", &members).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request to be refused with 400, got {status} {body}"
        );
    }
    // A name the documented rule refuses is a bad request, not a StackSet.
    let (status, body) = call(
        &router,
        "CreateStackSet",
        &[
            member("StackSetName", "9-starts-with-a-digit"),
            member("TemplateBody", r#"{"Resources":{}}"#),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {:?}: expected a malformed StackSetName to be refused, got {status} {body}",
        "a StackSetName starting with a digit"
    );
}

#[tokio::test]
async fn drift_detection_over_a_stack_set_reports_what_each_instance_really_is() {
    let router = router();

    // Every row's StackSet gets ONE instance in this Region, deployed from its own
    // template: the instance's stack is a real stack, and its topic a real topic.
    let mut deployed: Vec<(StackSetCase, String)> = Vec::new();
    for case in stack_set_cases() {
        let (status, body) = create_stack_set(&router, &case).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStackSet to succeed, got {status} {body}",
            case.case
        );
        let (status, body) =
            create_stack_instances(&router, case.stack_set_name, ACCOUNT_ID, REGION).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStackInstances to succeed, got {status} {body}",
            case.case
        );
        let operation_id = xml_text(&body, "OperationId").unwrap_or_else(|| {
            panic!(
                "case {:?}: CreateStackInstances answered no OperationId: {body}",
                case.case
            )
        });
        // "Use this operation id with DescribeStackSetOperation": the operation is a record
        // of its own, reported with the action that started it.
        let (status, body, _) =
            operation_status_of(&router, case.stack_set_name, &operation_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeStackSetOperation to succeed, got {status} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "Action").as_deref(),
            Some(operation_action::CREATE),
            "case {:?}: expected the deployment's own action, got {body}",
            case.case
        );
        deployed.push((case, operation_id));
    }

    wait_for_the_transition().await;

    // Each deployment has concluded, and its instance is up to date with its StackSet.
    for (case, operation_id) in &deployed {
        let (status, body, reported) =
            operation_status_of(&router, case.stack_set_name, operation_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeStackSetOperation to succeed, got {status} {body}",
            case.case
        );
        assert_eq!(
            reported.as_deref(),
            Some(operation_status::SUCCEEDED),
            "case {:?}: expected the deployment to have concluded, got {body}",
            case.case
        );
    }

    // ONE instance's topic is changed OUT OF BAND; the others are left alone, so the two
    // halves of the rule are both exercised in the same run.
    let (drifted_case, _) = &deployed[0];
    let drifted_stack_id = instance_stack_id(
        drifted_case.case,
        &router,
        drifted_case.stack_set_name,
        ACCOUNT_ID,
        REGION,
    )
    .await;
    let topic_arn = physical_id_of(
        drifted_case.case,
        &router,
        &drifted_stack_id,
        drifted_case.topic_logical_id,
    )
    .await;
    let (status, body) = sns(
        &router,
        "SetTopicAttributes",
        &[
            ("TopicArn", topic_arn.as_str()),
            ("AttributeName", "DisplayName"),
            ("AttributeValue", "changed-out-of-band"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the out-of-band change to succeed, got {status} {body}",
        drifted_case.case
    );

    // One detection per StackSet, each answering its own operation id.
    let mut detections: Vec<(usize, String)> = Vec::new();
    for (index, (case, _)) in deployed.iter().enumerate() {
        let (status, body) = call(
            &router,
            "DetectStackSetDrift",
            &[member("StackSetName", case.stack_set_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DetectStackSetDrift to succeed, got {status} {body}",
            case.case
        );
        let operation_id = xml_text(&body, "OperationId").unwrap_or_else(|| {
            panic!(
                "case {:?}: DetectStackSetDrift answered no OperationId: {body}",
                case.case
            )
        });
        detections.push((index, operation_id));
    }

    wait_for_the_transition().await;

    for (index, operation_id) in &detections {
        let (case, _) = &deployed[*index];
        let drifted = *index == 0;
        let expected_status = if drifted {
            DRIFT_STATUS_DRIFTED
        } else {
            DRIFT_STATUS_IN_SYNC
        };

        // "Use this operation id with DescribeStackSetOperation to monitor the progress of
        // the drift detection operation."
        let (status, body, reported) =
            operation_status_of(&router, case.stack_set_name, operation_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeStackSetOperation to succeed, got {status} {body}",
            case.case
        );
        assert_eq!(
            reported.as_deref(),
            Some(operation_status::SUCCEEDED),
            "case {:?}: expected the detection to have concluded, got {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "Action").as_deref(),
            Some(operation_action::DETECT_DRIFT),
            "case {:?}: expected the detection's own action, got {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "DriftDetectionStatus").as_deref(),
            Some(drift_detection_status::COMPLETED),
            "case {:?}: expected the detection to report its own completion, got {body}",
            case.case
        );

        // The StackSet reports the last COMPLETED detection's verdict and counts.
        let described = describe_stack_set(case.case, &router, case.stack_set_name).await;
        assert_eq!(
            xml_text(&described, "DriftStatus").as_deref(),
            Some(expected_status),
            "case {:?}: expected the StackSet's drift status to be {expected_status}, got \
             {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "TotalStackInstancesCount").as_deref(),
            Some("1"),
            "case {:?}: expected the one instance to be counted, got {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "DriftedStackInstancesCount").as_deref(),
            Some(if drifted { "1" } else { "0" }),
            "case {:?}: expected the drifted count to follow the instances, got {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "InSyncStackInstancesCount").as_deref(),
            Some(if drifted { "0" } else { "1" }),
            "case {:?}: expected the in-sync count to follow the instances, got {described}",
            case.case
        );

        // The per-resource rows that detection recorded for the instance.
        let (status, body) = list_resource_drifts(
            &router,
            case.stack_set_name,
            operation_id,
            ACCOUNT_ID,
            REGION,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected ListStackInstanceResourceDrifts to succeed, got {status} \
             {body}",
            case.case
        );
        let rows = wire_resource_drifts(&body);
        let expected_resource_status = if drifted {
            resource_drift_status::MODIFIED
        } else {
            resource_drift_status::IN_SYNC
        };
        assert_eq!(
            rows.len(),
            1,
            "case {:?}: expected one row per covered resource, got {rows:?}",
            case.case
        );
        let row = &rows[0];
        assert_eq!(
            row.logical_resource_id.as_deref(),
            Some(case.topic_logical_id),
            "case {:?}: expected the row to name the template's resource, got {row:?}",
            case.case
        );
        assert_eq!(
            row.resource_type.as_deref(),
            Some("AWS::SNS::Topic"),
            "case {:?}: expected the row to carry the resource's type, got {row:?}",
            case.case
        );
        assert_eq!(
            row.drift_status.as_deref(),
            Some(expected_resource_status),
            "case {:?}: expected the resource to be {expected_resource_status}, got {row:?}",
            case.case
        );
        if drifted {
            assert_eq!(
                row.property_paths,
                vec!["/DisplayName".to_string()],
                "case {:?}: expected the difference to name the property that changed, got \
                 {row:?}",
                case.case
            );
            assert!(
                body.contains(case.declared_display_name)
                    && body.contains("changed-out-of-band"),
                "case {:?}: expected the row to carry both sides of the comparison, got {body}",
                case.case
            );
        } else {
            assert!(
                row.property_paths.is_empty(),
                "case {:?}: expected no difference for an unchanged resource, got {row:?}",
                case.case
            );
        }
    }

    // A drift operation id of ANOTHER StackSet, and an id of an operation that is not a
    // drift detection at all, are both ids this read holds no DRIFT operation for: the
    // modelled refusal, never the previous operation's summaries.
    let (other_index, foreign_operation_id) = &detections[1];
    let (own_case, own_deployment_id) = &deployed[0];
    assert_ne!(
        *other_index, 0,
        "the foreign operation id must belong to another StackSet's detection"
    );
    for (case, operation_id) in [
        ("an operation id of another StackSet", foreign_operation_id),
        ("the id of this StackSet's deployment", own_deployment_id),
    ] {
        let (status, body) = list_resource_drifts(
            &router,
            own_case.stack_set_name,
            operation_id,
            ACCOUNT_ID,
            REGION,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the id to be refused with 404, got {status} {body}"
        );
        assert!(
            body.contains("OperationNotFoundException"),
            "case {case:?}: expected OperationNotFoundException, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some("Detect Drift Operation not found"),
            "case {case:?}: expected the refusal CloudFormation words for a drift operation \
             that does not exist, got {body}"
        );
    }

    // An account and Region pair the StackSet holds no instance for is the other modelled
    // refusal.
    let (_, own_operation_id) = &detections[0];
    let (status, body) = list_resource_drifts(
        &router,
        own_case.stack_set_name,
        own_operation_id,
        "999999999999",
        REGION,
    )
    .await;
    assert_eq!(
        status, 404,
        "case {:?}: expected an unknown instance to be refused with 404, got {status} {body}",
        "an account the StackSet has no instance in"
    );
    assert!(
        body.contains("StackInstanceNotFoundException"),
        "case {:?}: expected StackInstanceNotFoundException, got {body}",
        "an account the StackSet has no instance in"
    );
}

#[tokio::test]
async fn the_stack_set_surface_refuses_a_stack_set_this_region_does_not_hold() {
    let router = router();
    for (case, action, members) in [
        (
            "DescribeStackSet",
            "DescribeStackSet",
            vec![member("StackSetName", "no-such-set")],
        ),
        (
            "DetectStackSetDrift",
            "DetectStackSetDrift",
            vec![member("StackSetName", "no-such-set")],
        ),
        (
            "CreateStackInstances",
            "CreateStackInstances",
            vec![
                member("StackSetName", "no-such-set"),
                member("Accounts.member.1", ACCOUNT_ID),
                member("Regions.member.1", REGION),
            ],
        ),
        (
            "DescribeStackSetOperation",
            "DescribeStackSetOperation",
            vec![
                member("StackSetName", "no-such-set"),
                member("OperationId", "no-such-operation"),
            ],
        ),
        (
            "DescribeStackInstance",
            "DescribeStackInstance",
            vec![
                member("StackSetName", "no-such-set"),
                member("StackInstanceAccount", ACCOUNT_ID),
                member("StackInstanceRegion", REGION),
            ],
        ),
        (
            "ListStackInstanceResourceDrifts",
            "ListStackInstanceResourceDrifts",
            vec![
                member("StackSetName", "no-such-set"),
                member("OperationId", "no-such-operation"),
                member("StackInstanceAccount", ACCOUNT_ID),
                member("StackInstanceRegion", REGION),
            ],
        ),
    ] {
        let (status, body) = call(&router, action, &members).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected an unknown StackSet to be refused with 404, got {status} \
             {body}"
        );
        assert!(
            body.contains("StackSetNotFoundException"),
            "case {case:?}: expected StackSetNotFoundException, got {body}"
        );
    }

    // A StackSet with no instance has no stack to detect drift on: the modelled
    // `InvalidOperationException`, not an operation that checked nothing.
    let case = &stack_set_cases()[0];
    let (status, body) = create_stack_set(&router, case).await;
    assert_eq!(
        status, 200,
        "expected CreateStackSet to succeed, got {status} {body}"
    );
    let (status, body) = call(
        &router,
        "DetectStackSetDrift",
        &[member("StackSetName", case.stack_set_name)],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {:?}: expected a StackSet with no instance to be refused with 400, got {status} \
         {body}",
        "detecting drift on a StackSet with no instance"
    );
    assert!(
        body.contains("InvalidOperationException"),
        "case {:?}: expected InvalidOperationException, got {body}",
        "detecting drift on a StackSet with no instance"
    );

    // An account and Region pair carries at most ONE instance of a StackSet, so a second
    // deployment to the same pair is refused rather than leaving the first instance's stack
    // behind with nothing pointing at it.
    let (status, body) =
        create_stack_instances(&router, case.stack_set_name, ACCOUNT_ID, REGION).await;
    assert_eq!(
        status, 200,
        "expected the first CreateStackInstances to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;
    let (status, body) =
        create_stack_instances(&router, case.stack_set_name, ACCOUNT_ID, REGION).await;
    assert_eq!(
        status, 400,
        "case {:?}: expected a second instance in the same account and Region to be refused, \
         got {status} {body}",
        "deploying to an account and Region the StackSet already has an instance in"
    );
    assert!(
        body.contains(ACCOUNT_ID) && body.contains(REGION),
        "case {:?}: expected the refusal to name the account and the Region, got {body}",
        "deploying to an account and Region the StackSet already has an instance in"
    );

    // A request naming neither Accounts nor DeploymentTargets, and one naming no Region,
    // deploy nothing at all.
    for (case, members) in [
        (
            "no accounts and no deployment targets",
            vec![
                member("StackSetName", case.stack_set_name),
                member("Regions.member.1", REGION),
            ],
        ),
        (
            "no Regions",
            vec![
                member("StackSetName", case.stack_set_name),
                member("Accounts.member.1", ACCOUNT_ID),
            ],
        ),
    ] {
        let (status, body) = call(&router, "CreateStackInstances", &members).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request to be refused with 400, got {status} {body}"
        );
    }
}
