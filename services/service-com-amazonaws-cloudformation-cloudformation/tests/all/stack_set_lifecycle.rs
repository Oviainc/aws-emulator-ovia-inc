// SPDX-License-Identifier: CC0-1.0

//! `CreateStackSet` and `DescribeStackSet` over the awsQuery wire.
//!
//! The rule every case here holds the service to: **`CreateStackSet` PERSISTS a StackSet in
//! the administrator account and Region the call was signed for, carrying the configuration
//! the request stated, and every later read answers from that record** — so
//! `DescribeStackSet` reports the stored template, parameters, capabilities, tags, roles,
//! permission model and execution settings rather than echoing a request, a name the Region
//! already holds is refused unless the request repeats the `ClientRequestToken` of the call
//! that created it, and `GetTemplateSummary` summarises the very template the StackSet was
//! created with.
//!
//! The rows span the class the rule generalises over rather than one recorded request: the
//! minimal create and one that states every optional member, both permission models, a
//! template with and without declared parameters, supplied parameters and template defaults,
//! each nested configuration structure (`AutoDeployment`, `ManagedExecution`), tags and
//! capabilities, and a StackSet created from an existing stack. The refusals are a table of
//! their own: each name the documented name rule rejects, each way a request names no
//! template or two, and a template whose capabilities are not acknowledged.
//!
//! Every call goes through the real request path (form-encoded body in, XML envelope out),
//! so what is asserted is what a client sees.

use serde_json::json;

use service_com_amazonaws_cloudformation_cloudformation::state::stack_sets::{
    detailed_status, instance_status, permission_model, StoredStackSet, MAX_STACK_SETS,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    DRIFT_STATUS_NOT_CHECKED, STACK_NAME_MAX_LEN,
};

use crate::harness::{
    call, call_in, member, router, router_with_engine, store, xml_text, xml_texts, REGION,
};
use crate::stack_lifecycle::{list_members, ACCOUNT_ID};

/// A template declaring one resource and no parameters.
const PLAIN_TEMPLATE: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#;

/// A template declaring two parameters, one of them with a `Default`: what the StackSet
/// reports as `Parameters` is resolved against THIS template.
const PARAMETERISED_TEMPLATE: &str = concat!(
    r#"{"Parameters":{"Stage":{"Type":"String","Default":"dev"},"#,
    r#""Size":{"Type":"String"}},"#,
    r#""Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#
);

/// A custom administration role ARN, as a self-managed StackSet names one.
const ADMIN_ROLE: &str = "arn:aws:iam::000000000000:role/StackSetAdministrator";

/// One expected member of the described StackSet: the XML tag and the text it must carry,
/// or `None` for a member the StackSet does not carry at all.
type Expected = (&'static str, Option<&'static str>);

/// One row of the create-and-read-back table.
struct Case {
    /// What the row is, named in every assertion it makes.
    case: &'static str,
    /// The StackSet the row creates, unique per row so the rows do not collide.
    stack_set_name: &'static str,
    /// The `CreateStackSet` members beyond `StackSetName`.
    request: Vec<(String, String)>,
    /// What `DescribeStackSet` must then report for each scalar member.
    expected: Vec<Expected>,
    /// What it must report for each LIST member, whose entries the query envelope renders
    /// as `<member>` inside the member's own element: the entries in wire order, and an
    /// empty list for a StackSet that carries none.
    expected_lists: Vec<(&'static str, Vec<&'static str>)>,
}

/// `CreateStackSet` with whatever the row states.
async fn create_stack_set(
    router: &smithy_server_runtime::MultiServiceRouter,
    stack_set_name: &str,
    request: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![member("StackSetName", stack_set_name)];
    members.extend_from_slice(request);
    call(router, "CreateStackSet", &members).await
}

/// `DescribeStackSet` for one StackSet, by name or by id.
async fn describe_stack_set(
    router: &smithy_server_runtime::MultiServiceRouter,
    name_or_id: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeStackSet",
        &[member("StackSetName", name_or_id)],
    )
    .await
}

/// The table: each row states a different configuration, and names what the stored StackSet
/// must then report for it.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "the minimal create: a name and a template",
            stack_set_name: "set-minimal",
            request: vec![member("TemplateBody", PLAIN_TEMPLATE)],
            expected: vec![
                ("StackSetName", Some("set-minimal")),
                // The documented defaults, spelled as the documentation spells them rather
                // than read back out of the implementation's own constants: "The status of
                // the StackSet" is ACTIVE for one that exists, "By default, SELF-MANAGED is
                // specified", and "if you do not specify an execution role, CloudFormation
                // uses the AWSCloudFormationStackSetExecutionRole role".
                ("Status", Some("ACTIVE")),
                ("TemplateBody", Some(PLAIN_TEMPLATE)),
                ("PermissionModel", Some("SELF_MANAGED")),
                (
                    "ExecutionRoleName",
                    Some("AWSCloudFormationStackSetExecutionRole"),
                ),
                // "When false (default), StackSets performs one operation at a time."
                ("Active", Some("false")),
                // Members this StackSet carries none of.
                ("Description", None),
                ("AdministrationRoleARN", None),
                ("Enabled", None),
                ("ParameterKey", None),
            ],
            expected_lists: vec![
                ("Capabilities", vec![]),
                ("Parameters", vec![]),
                ("Tags", vec![]),
                // No stack instance, so no Region it is deployed in and no OU.
                ("Regions", vec![]),
                ("OrganizationalUnitIds", vec![]),
            ],
        },
        Case {
            case: "a described, tagged and acknowledged create",
            stack_set_name: "set-described",
            request: vec![
                member("TemplateBody", PLAIN_TEMPLATE),
                member("Description", "the fleet baseline"),
                member("Tags.member.1.Key", "env"),
                member("Tags.member.1.Value", "prod"),
                member("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
            expected: vec![
                ("Description", Some("the fleet baseline")),
                ("Key", Some("env")),
                ("Value", Some("prod")),
            ],
            expected_lists: vec![("Capabilities", vec!["CAPABILITY_IAM"])],
        },
        Case {
            case: "self-managed permissions with both custom roles",
            stack_set_name: "set-self-managed",
            request: vec![
                member("TemplateBody", PLAIN_TEMPLATE),
                member("PermissionModel", permission_model::SELF_MANAGED),
                member("AdministrationRoleARN", ADMIN_ROLE),
                member("ExecutionRoleName", "FleetExecution"),
            ],
            expected: vec![
                ("PermissionModel", Some(permission_model::SELF_MANAGED)),
                ("AdministrationRoleARN", Some(ADMIN_ROLE)),
                ("ExecutionRoleName", Some("FleetExecution")),
            ],
            expected_lists: vec![],
        },
        Case {
            case: "service-managed permissions with automatic deployment",
            stack_set_name: "set-service-managed",
            request: vec![
                member("TemplateBody", PLAIN_TEMPLATE),
                member("PermissionModel", permission_model::SERVICE_MANAGED),
                member("AutoDeployment.Enabled", "true"),
                member("AutoDeployment.RetainStacksOnAccountRemoval", "false"),
            ],
            expected: vec![
                ("PermissionModel", Some(permission_model::SERVICE_MANAGED)),
                ("Enabled", Some("true")),
                ("RetainStacksOnAccountRemoval", Some("false")),
            ],
            expected_lists: vec![],
        },
        Case {
            case: "service-managed permissions keep the roles the request stated",
            stack_set_name: "set-service-managed-roles",
            request: vec![
                member("TemplateBody", PLAIN_TEMPLATE),
                member("PermissionModel", permission_model::SERVICE_MANAGED),
                member("AdministrationRoleARN", ADMIN_ROLE),
                member("ExecutionRoleName", "FleetExecution"),
            ],
            expected: vec![
                ("PermissionModel", Some(permission_model::SERVICE_MANAGED)),
                // Nothing a request supplied is discarded: the roles it named are the ones
                // the record carries.
                ("AdministrationRoleARN", Some(ADMIN_ROLE)),
                ("ExecutionRoleName", Some("FleetExecution")),
            ],
            expected_lists: vec![],
        },
        Case {
            case: "service-managed permissions name no execution role of their own",
            stack_set_name: "set-service-managed-default-role",
            request: vec![
                member("TemplateBody", PLAIN_TEMPLATE),
                member("PermissionModel", permission_model::SERVICE_MANAGED),
            ],
            expected: vec![
                ("PermissionModel", Some(permission_model::SERVICE_MANAGED)),
                // The documented default role belongs to the self-managed model, where the
                // account creates those roles itself; with service-managed permissions
                // "StackSets automatically creates the IAM roles required to deploy".
                ("ExecutionRoleName", None),
                ("AdministrationRoleARN", None),
            ],
            expected_lists: vec![],
        },
        Case {
            case: "managed execution turned on",
            stack_set_name: "set-managed-execution",
            request: vec![
                member("TemplateBody", PLAIN_TEMPLATE),
                member("ManagedExecution.Active", "true"),
            ],
            expected: vec![("Active", Some("true"))],
            expected_lists: vec![],
        },
        Case {
            case: "a template with declared parameters, one value supplied",
            stack_set_name: "set-parameterised",
            request: vec![
                member("TemplateBody", PARAMETERISED_TEMPLATE),
                member("Parameters.member.1.ParameterKey", "Size"),
                member("Parameters.member.1.ParameterValue", "large"),
            ],
            expected: vec![
                ("TemplateBody", Some(PARAMETERISED_TEMPLATE)),
                // The template's own `Default` for the parameter no value was supplied
                // for, so the first declared parameter is reported too.
                ("ParameterKey", Some("Stage")),
                ("ParameterValue", Some("dev")),
            ],
            expected_lists: vec![],
        },
    ]
}

#[tokio::test]
async fn a_created_stackset_is_read_back_through_describe() {
    let router = router();

    for case in cases() {
        let Case {
            case,
            stack_set_name,
            request,
            expected,
            expected_lists,
        } = case;
        let (status, created) = create_stack_set(&router, stack_set_name, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateStackSet to answer 200, got {status} {created}"
        );
        let stack_set_id = xml_text(&created, "StackSetId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a StackSetId, got {created}"));
        // "The ID of the StackSet that you're creating": `<name>:<uuid>`.
        assert!(
            stack_set_id.starts_with(&format!("{stack_set_name}:")),
            "case {case:?}: expected the id to carry the StackSet's name, got {stack_set_id:?}"
        );

        // The read is taken by the id the create handed back, so what is asserted is the
        // record that call wrote rather than a lookup by the name the request carried.
        let (status, body) = describe_stack_set(&router, &stack_set_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeStackSet to answer 200, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "StackSetId").as_deref(),
            Some(stack_set_id.as_str()),
            "case {case:?}: expected the id the create minted, got {body}"
        );
        // Every StackSet carries its ARN, built over the account and Region the call was
        // signed for.
        assert_eq!(
            xml_text(&body, "StackSetARN").as_deref(),
            Some(
                format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stackset/{stack_set_id}")
                    .as_str()
            ),
            "case {case:?}: expected the StackSet's own ARN, got {body}"
        );
        // Nothing has drift checked a StackSet this call just created.
        assert_eq!(
            xml_text(&body, "DriftStatus").as_deref(),
            Some(DRIFT_STATUS_NOT_CHECKED),
            "case {case:?}: expected an unchecked drift status, got {body}"
        );
        assert_eq!(
            xml_text(&body, "LastDriftCheckTimestamp"),
            None,
            "case {case:?}: expected no drift-check instant, got {body}"
        );
        for (tag, value) in expected {
            assert_eq!(
                xml_text(&body, tag).as_deref(),
                value,
                "case {case:?}: expected {tag} to be {value:?}, got {body}"
            );
        }
        for (tag, entries) in expected_lists {
            assert_eq!(
                list_members(&body, tag),
                entries
                    .iter()
                    .map(|entry| entry.to_string())
                    .collect::<Vec<String>>(),
                "case {case:?}: expected {tag} to carry {entries:?}, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_parameters_in_force_are_the_supplied_values_over_the_template_defaults() {
    let router = router();

    let (status, created) = create_stack_set(
        &router,
        "set-parameter-order",
        &[
            member("TemplateBody", PARAMETERISED_TEMPLATE),
            member("Parameters.member.1.ParameterKey", "Size"),
            member("Parameters.member.1.ParameterValue", "large"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"parameters in force\": expected 200, got {status} {created}"
    );

    let (status, body) = describe_stack_set(&router, "set-parameter-order").await;
    assert_eq!(
        status, 200,
        "case \"parameters in force\": expected 200, got {status} {body}"
    );
    assert_eq!(
        xml_texts(&body, "ParameterKey"),
        vec!["Stage".to_string(), "Size".to_string()],
        "case \"parameters in force\": expected both declared parameters, got {body}"
    );
    assert_eq!(
        xml_texts(&body, "ParameterValue"),
        vec!["dev".to_string(), "large".to_string()],
        "case \"parameters in force\": expected the default and the supplied value, got {body}"
    );
}

#[tokio::test]
async fn a_name_the_region_already_holds_is_refused() {
    let router = router();
    let template = vec![member("TemplateBody", PLAIN_TEMPLATE)];

    let (status, first) = create_stack_set(&router, "set-twice", &template).await;
    assert_eq!(
        status, 200,
        "case \"the first create\": expected 200, got {status} {first}"
    );

    // Each row is a second create of the SAME name, differing in what it states beyond it:
    // the name is what collides, so none of them may create a second StackSet.
    let rows: Vec<(&str, Vec<(String, String)>)> = vec![
        ("the same request again", template.clone()),
        (
            "another template under the same name",
            vec![member("TemplateBody", PARAMETERISED_TEMPLATE)],
        ),
        (
            "a fresh client request token under the same name",
            vec![
                member("TemplateBody", PLAIN_TEMPLATE),
                member("ClientRequestToken", "a-different-token"),
            ],
        ),
    ];
    for (case, request) in &rows {
        let (status, body) = create_stack_set(&router, "set-twice", request).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("NameAlreadyExistsException"),
            "case {case:?}: expected the modelled refusal, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some("StackSet already exists."),
            "case {case:?}: expected the fixed refusal sentence, got {body}"
        );
    }

    // The refused creates left the first StackSet exactly as it was.
    let (status, body) = describe_stack_set(&router, "set-twice").await;
    assert_eq!(
        status, 200,
        "case \"the StackSet after the refusals\": expected 200, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "TemplateBody").as_deref(),
        Some(PLAIN_TEMPLATE),
        "case \"the StackSet after the refusals\": expected the first template, got {body}"
    );
}

#[tokio::test]
async fn a_retry_carrying_the_same_client_request_token_answers_the_same_stackset() {
    let router = router();
    let request = vec![
        member("TemplateBody", PLAIN_TEMPLATE),
        member("ClientRequestToken", "create-set-token"),
    ];

    let (status, first) = create_stack_set(&router, "set-retried", &request).await;
    assert_eq!(
        status, 200,
        "case \"the first call\": expected 200, got {status} {first}"
    );
    let first_id = xml_text(&first, "StackSetId").expect("a StackSetId");

    // "Specify this token if you plan to retry requests so that CloudFormation knows that
    // you're not attempting to create another StackSet with the same name."
    for attempt in ["the first retry", "the second retry"] {
        let (status, retried) = create_stack_set(&router, "set-retried", &request).await;
        assert_eq!(
            status, 200,
            "case {attempt:?}: expected 200, got {status} {retried}"
        );
        assert_eq!(
            xml_text(&retried, "StackSetId").as_deref(),
            Some(first_id.as_str()),
            "case {attempt:?}: expected the id of the StackSet already created, got {retried}"
        );
    }
}

#[tokio::test]
async fn a_stackset_name_is_unique_per_region() {
    let router = router();
    let other_region = "us-west-2";
    let request = &[
        member("StackSetName", "set-regional"),
        member("TemplateBody", PLAIN_TEMPLATE),
    ];

    let (status, here) = call_in(&router, REGION, "CreateStackSet", request).await;
    assert_eq!(
        status, 200,
        "case \"the administrator Region\": expected 200, got {status} {here}"
    );
    let (status, there) = call_in(&router, other_region, "CreateStackSet", request).await;
    assert_eq!(
        status, 200,
        "case \"a second Region\": expected 200, got {status} {there}"
    );
    assert_ne!(
        xml_text(&here, "StackSetId"),
        xml_text(&there, "StackSetId"),
        "case \"one name in two Regions\": expected two StackSets, got {here} and {there}"
    );

    // Each Region reads its own StackSet, and neither reads the other's.
    for (case, region, other) in [
        ("the administrator Region's read", REGION, other_region),
        ("the second Region's read", other_region, REGION),
    ] {
        let (status, body) = call_in(
            &router,
            region,
            "DescribeStackSet",
            &[member("StackSetName", "set-regional")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let arn = xml_text(&body, "StackSetARN").unwrap_or_default();
        assert!(
            arn.contains(&format!(":{region}:")) && !arn.contains(&format!(":{other}:")),
            "case {case:?}: expected the Region's own StackSet, got {arn:?}"
        );
    }
}

#[tokio::test]
async fn a_stackset_no_region_holds_is_refused_rather_than_described() {
    let router = router();
    let (status, created) =
        create_stack_set(&router, "set-present", &[member("TemplateBody", PLAIN_TEMPLATE)]).await;
    assert_eq!(
        status, 200,
        "case \"the StackSet that exists\": expected 200, got {status} {created}"
    );
    let stack_set_id = xml_text(&created, "StackSetId").expect("a StackSetId");

    // Each row names a `StackSetName` that resolves to no StackSet of this administrator
    // account and Region.
    let rows: Vec<(&str, String)> = vec![
        ("a name no StackSet carries", "set-absent".to_string()),
        (
            "an id no StackSet carries",
            "set-present:00000000-0000-4000-8000-000000000000".to_string(),
        ),
        (
            "the name of a StackSet of this Region with its case changed",
            "SET-PRESENT".to_string(),
        ),
        ("a StackSet of another Region", stack_set_id.clone()),
    ];
    for (case, name_or_id) in &rows {
        let region = if *case == "a StackSet of another Region" {
            "eu-west-1"
        } else {
            REGION
        };
        let (status, body) = call_in(
            &router,
            region,
            "DescribeStackSet",
            &[member("StackSetName", name_or_id)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("StackSetNotFoundException"),
            "case {case:?}: expected the modelled refusal, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(format!("StackSet {name_or_id} not found").as_str()),
            "case {case:?}: expected the refusal to name what was asked for, got {body}"
        );
    }
}

#[tokio::test]
async fn the_documented_name_rule_decides_which_names_are_accepted() {
    let router = router();
    let too_long = "a".repeat(STACK_NAME_MAX_LEN + 1);
    let at_the_bound = "b".repeat(STACK_NAME_MAX_LEN);

    // The rule: "A stack name can contain only alphanumeric characters (case-sensitive) and
    // hyphens. It must start with an alphabetic character and can't be longer than 128
    // characters." Each row is a name on one side of one clause of it, including the
    // published length bound and one character past it.
    let rows: Vec<(&str, String, bool)> = vec![
        ("a letter, digits and hyphens", "Set-9-a".to_string(), true),
        ("one letter", "s".to_string(), true),
        ("exactly the length bound", at_the_bound, true),
        ("one past the length bound", too_long, false),
        ("starting with a digit", "9set".to_string(), false),
        ("starting with a hyphen", "-set".to_string(), false),
        ("an underscore", "set_one".to_string(), false),
        ("a colon, as an id carries", "set:one".to_string(), false),
        ("a space", "set one".to_string(), false),
        ("empty", String::new(), false),
    ];
    for (case, stack_set_name, accepted) in &rows {
        let (status, body) =
            create_stack_set(&router, stack_set_name, &[member("TemplateBody", PLAIN_TEMPLATE)])
                .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the name to be accepted, got {status} {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the name to be refused, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected the service's own refusal code, got {body}"
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        assert!(
            message.contains("stackSetName"),
            "case {case:?}: expected the refusal to name the member, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_request_naming_no_template_or_two_is_refused() {
    let router = router();

    // "Conditional: You must specify either the TemplateBody or the TemplateURL parameter,
    // but not both."
    struct Row {
        /// What the row is, named in every assertion it makes.
        case: &'static str,
        /// The template members the row states, which is what makes it that row.
        request: Vec<(String, String)>,
        /// The status it must be answered with.
        expected_status: u16,
        /// Text the refusal must carry.
        expected_message: &'static str,
    }
    let rows: Vec<Row> = vec![
        Row {
            case: "neither template member",
            request: vec![],
            expected_status: 400,
            expected_message: "Either TemplateBody or TemplateURL must be specified",
        },
        Row {
            case: "both template members",
            request: vec![
                member("TemplateBody", PLAIN_TEMPLATE),
                member("TemplateURL", "https://example.s3.amazonaws.com/t.json"),
            ],
            expected_status: 400,
            expected_message: "You cannot specify both Template URL and Template Body.",
        },
        Row {
            // A `TemplateURL` is resolved by reading the object it addresses out of this
            // emulator's own Amazon S3: a URL naming a bucket nothing created is the
            // REQUEST's own bad reference, not a StackSet created from a template nobody
            // read.
            case: "a template URL naming no object this account can read",
            request: vec![member("TemplateURL", "https://example.s3.amazonaws.com/t.json")],
            expected_status: 400,
            expected_message: "S3 error: The specified bucket does not exist\nFor more \
                               information check \
                               http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html",
        },
    ];
    for Row {
        case,
        request,
        expected_status,
        expected_message,
    } in &rows
    {
        let (status, body) = create_stack_set(&router, "set-template-source", request).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        assert!(
            message.contains(expected_message),
            "case {case:?}: expected a refusal mentioning {expected_message:?}, got {message:?}"
        );
    }

    // Nothing above created a StackSet, so the name is still free.
    let (status, body) = describe_stack_set(&router, "set-template-source").await;
    assert_eq!(
        status, 404,
        "case \"the refused name\": expected no StackSet to exist, got {status} {body}"
    );
}

#[tokio::test]
async fn a_template_whose_capabilities_are_not_acknowledged_creates_no_stackset() {
    let router = router();
    // "In some cases, you must explicitly acknowledge that your StackSet template contains
    // certain capabilities in order for CloudFormation to create the StackSet": an IAM
    // resource is one of them.
    let template = json!({"Resources": {"Role": {
        "Type": "AWS::IAM::Role",
        "Properties": {"AssumeRolePolicyDocument": {
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"Service": "cloudformation.amazonaws.com"},
                "Action": "sts:AssumeRole",
            }],
        }},
    }}})
    .to_string();

    let (status, body) = create_stack_set(
        &router,
        "set-iam-unacknowledged",
        &[member("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"an IAM template with nothing acknowledged\": expected 400, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Message").as_deref(),
        Some("Requires capabilities : [CAPABILITY_IAM]"),
        "case \"an IAM template with nothing acknowledged\": expected the capability named, \
         got {body}"
    );
    let (status, absent) = describe_stack_set(&router, "set-iam-unacknowledged").await;
    assert_eq!(
        status, 404,
        "case \"after the refusal\": expected no StackSet, got {status} {absent}"
    );

    // The same template, acknowledged, is created and read back.
    let (status, created) = create_stack_set(
        &router,
        "set-iam-acknowledged",
        &[
            member("TemplateBody", &template),
            member("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"an IAM template acknowledged\": expected 200, got {status} {created}"
    );
    let (status, body) = describe_stack_set(&router, "set-iam-acknowledged").await;
    assert_eq!(
        status, 200,
        "case \"an IAM template acknowledged\": expected 200, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Capability").as_deref(),
        None,
        "case \"an IAM template acknowledged\": a capability is a list entry, got {body}"
    );
    assert_eq!(
        list_members(&body, "Capabilities"),
        vec!["CAPABILITY_IAM".to_string()],
        "case \"an IAM template acknowledged\": expected the acknowledgement stored, got {body}"
    );
}

#[tokio::test]
async fn the_template_a_stackset_holds_is_what_get_template_summary_reports() {
    let router = router();
    let (status, created) = create_stack_set(
        &router,
        "set-summarised",
        &[member("TemplateBody", PARAMETERISED_TEMPLATE)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the StackSet to summarise\": expected 200, got {status} {created}"
    );

    // A second reader over the same record: the summary is of the template that StackSet was
    // created with, so it reports the parameters that template declares.
    for (case, name_or_id) in [
        ("by name", "set-summarised".to_string()),
        (
            "by id",
            xml_text(&created, "StackSetId").expect("a StackSetId"),
        ),
    ] {
        let (status, body) = call(
            &router,
            "GetTemplateSummary",
            &[member("StackSetName", &name_or_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        // The declarations are a SET: which order the response lists them in follows how
        // the template reader walked the body's `Parameters` section, so the assertion is
        // over the keys themselves rather than their order.
        let mut declared = xml_texts(&body, "ParameterKey");
        declared.sort();
        assert_eq!(
            declared,
            vec!["Size".to_string(), "Stage".to_string()],
            "case {case:?}: expected the template's declared parameters, got {body}"
        );
        assert_eq!(
            list_members(&body, "ResourceTypes"),
            vec!["AWS::SNS::Topic".to_string()],
            "case {case:?}: expected the type the template declares, got {body}"
        );
    }
}

#[tokio::test]
async fn a_store_already_holding_its_ceiling_of_stacksets_refuses_another() {
    let (router, engine) = router_with_engine();

    // Every growth site of this store is bounded, and the bound belongs to the WRITE path:
    // the ceiling is seeded here through the one device that records a StackSet, because a
    // ceiling's worth of wire calls is not what the rule is about.
    {
        let held = store(&engine);
        let mut held = held.lock();
        for index in 0..MAX_STACK_SETS {
            held.record_stack_set(StoredStackSet::new(format!("seeded-{index}"), REGION));
        }
    }

    let (status, body) = create_stack_set(
        &router,
        "set-past-the-ceiling",
        &[member("TemplateBody", PLAIN_TEMPLATE)],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"past the StackSet ceiling\": expected 400, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("LimitExceededException"),
        "case \"past the StackSet ceiling\": expected the modelled refusal, got {body}"
    );
    assert_eq!(
        xml_text(&body, "Message").as_deref(),
        Some(
            format!(
                "Limit for stack sets exceeded: no more than {MAX_STACK_SETS} stack sets can \
                 exist in this account"
            )
            .as_str()
        ),
        "case \"past the StackSet ceiling\": expected the ceiling named, got {body}"
    );
    // The refusal wrote nothing: the store still holds exactly the seeded StackSets.
    assert_eq!(
        store(&engine).lock().stack_sets.len(),
        MAX_STACK_SETS,
        "case \"past the StackSet ceiling\": expected the refused create to add none"
    );
}

#[tokio::test]
async fn a_stack_imported_into_a_new_stackset_becomes_its_one_instance() {
    let router = router();

    // The stack to import, created through this service's own wire.
    let (status, stack) = call(
        &router,
        "CreateStack",
        &[
            member("StackName", "imported-stack"),
            member("TemplateBody", PLAIN_TEMPLATE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the stack to import\": expected 200, got {status} {stack}"
    );
    let stack_id = xml_text(&stack, "StackId").expect("a StackId");

    // "The stack ID you are importing into a new StackSet": the StackSet takes that stack's
    // template, and the stack becomes its one stack instance.
    let (status, created) = create_stack_set(
        &router,
        "set-imported",
        &[member("StackId", &stack_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the import\": expected 200, got {status} {created}"
    );

    let (status, body) = describe_stack_set(&router, "set-imported").await;
    assert_eq!(
        status, 200,
        "case \"the imported StackSet\": expected 200, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "TemplateBody").as_deref(),
        Some(PLAIN_TEMPLATE),
        "case \"the imported StackSet\": expected the stack's own template, got {body}"
    );
    assert_eq!(
        list_members(&body, "Regions"),
        vec![REGION.to_string()],
        "case \"the imported StackSet\": expected the Region the stack is deployed in, got {body}"
    );
    assert_eq!(
        xml_text(&body, "TotalStackInstancesCount").as_deref(),
        Some("1"),
        "case \"the imported StackSet\": expected one stack instance counted, got {body}"
    );

    // The instance the import recorded, read through the operation that reports instances.
    let (status, instances) = call(
        &router,
        "ListStackInstances",
        &[member("StackSetName", "set-imported")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the imported instance\": expected 200, got {status} {instances}"
    );
    for (tag, expected) in [
        ("StackId", stack_id.as_str()),
        ("Account", ACCOUNT_ID),
        ("Region", REGION),
        ("Status", instance_status::CURRENT),
        ("DetailedStatus", detailed_status::SUCCEEDED),
    ] {
        assert_eq!(
            xml_text(&instances, tag).as_deref(),
            Some(expected),
            "case {tag:?}: expected {expected:?}, got {instances}"
        );
    }

    // A stack no Region holds is refused, and no StackSet is created for it.
    let (status, body) = create_stack_set(
        &router,
        "set-import-absent",
        &[member(
            "StackId",
            &format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/ghost/0"),
        )],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"importing a stack that does not exist\": expected 400, got {status} {body}"
    );
    let (status, absent) = describe_stack_set(&router, "set-import-absent").await;
    assert_eq!(
        status, 404,
        "case \"after the refused import\": expected no StackSet, got {status} {absent}"
    );
}
