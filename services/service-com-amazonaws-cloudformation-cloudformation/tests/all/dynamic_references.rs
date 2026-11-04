// SPDX-License-Identifier: CC0-1.0

//! The `{{resolve:...}}` DYNAMIC REFERENCES a template's resource properties carry, end to
//! end over the wire.
//!
//! RULE: CloudFormation resolves a template's dynamic references ITSELF, against the store
//! each one names, BEFORE the deployment asks any owning service for anything, and the FORM
//! of the reference decides both which values that store may answer with — `ssm` resolves a
//! Parameter Store parameter held as plain text (`String`, `StringList`) and `ssm-secure`
//! resolves only one held encrypted (`SecureString`) — and WHERE the reference may stand: a
//! secure reference is supported only in the resource properties AWS publishes for it. A
//! reference the deployment cannot honour (no such parameter, no such version, a type that
//! form does not resolve, or a secure one standing in an unsupported property) is
//! CloudFormation's OWN refusal of the deployment: no resource is deployed at all, the
//! diagnostic saying which reference could not be honoured and why is what the STACK
//! reports as its status reason, and the create rolls back to `ROLLBACK_COMPLETE`. A
//! reference it can honour reaches the service that owns the resource as the VALUE, and a
//! secure one's value is kept out of everything the stack itself reports.
//!
//! The rows differ along what the rule generalises over: the form, the type the parameter
//! is held under, whether a version selector is given and whether it exists, whether the
//! parameter exists at all, the resource type and the property path carrying the reference
//! (including a published nested path and an unpublished one), and whether the reference is
//! the whole property value or sits inside a longer one.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudformation_cloudformation::state::{
    resource_status, status, RESOURCE_CREATION_INITIATED,
};

use crate::stack_lifecycle::{
    acknowledging_iam, call, create_stack, describe_stack, member, rds, router,
    wait_for_the_transition, xml_blocks, xml_text, REGION,
};
use crate::stack_resources::element;

/// AWS Systems Manager's `X-Amz-Target` prefix, from its model's service shape
/// (`com.amazonaws.ssm#AmazonSSM`), which speaks awsJson1_1.
const SSM_TARGET_PREFIX: &str = "AmazonSSM";

/// One Systems Manager call over the awsJson1_1 wire its model declares, signed for `ssm`
/// in the Region the stacks here are created in — so a parameter this suite writes is the
/// one a stack's reference resolves against.
///
/// The ONE Systems Manager caller of this crate's suites: `ssm_typed_parameters.rs` writes
/// and reads the parameters an SSM-TYPED template parameter addresses through it, so the
/// calling convention is spelled once.
pub(crate) async fn ssm(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ssm/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://ssm.{REGION}.amazonaws.com/"))
        .header("host", format!("ssm.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{SSM_TARGET_PREFIX}.{operation}"))
        .header("authorization", authorization)
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed Systems Manager request");
    let response = router.dispatch(request).await;
    let status_code = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status_code, parsed)
}

/// Write one parameter a reference or an SSM-typed parameter can name, and assert the store
/// took it.
pub(crate) async fn put_parameter(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    parameter_type: &str,
    value: &str,
) {
    let (status_code, body) = ssm(
        router,
        "PutParameter",
        &json!({"Name": name, "Type": parameter_type, "Value": value}),
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: PutParameter({name}, {parameter_type}) must answer 200, got \
         {status_code} {body}"
    );
}

/// The value Parameter Store holds for `name`, or `None` when it holds no such parameter.
async fn parameter_value(router: &MultiServiceRouter, name: &str) -> Option<String> {
    let (status_code, body) = ssm(
        router,
        "GetParameter",
        &json!({"Name": name, "WithDecryption": true}),
    )
    .await;
    if status_code != 200 {
        return None;
    }
    body.pointer("/Parameter/Value")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `MasterUsername` Amazon RDS holds for the DB instance `identifier`, or `None` when
/// it holds no such instance.
async fn master_username(router: &MultiServiceRouter, identifier: &str) -> Option<String> {
    let (status_code, body) = rds(
        router,
        "DescribeDBInstances",
        &[("DBInstanceIdentifier", identifier)],
    )
    .await;
    if status_code != 200 {
        return None;
    }
    xml_text(&body, "MasterUsername")
}

/// What the deployment of a template carrying one reference must come to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deployed {
    /// The reference resolved: the stack completes.
    Completed,
    /// The reference is one the deployment cannot honour: the resource fails and the stack
    /// rolls back.
    RolledBack,
}

/// Where in which resource type's properties a row puts the reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carrier {
    /// `AWS::RDS::DBInstance`'s `MasterUserPassword` — a property AWS publishes as
    /// supporting a secure reference, and the recorded shape.
    DatabasePassword,
    /// `AWS::RDS::DBInstance`'s `MasterUsername` — a property of the SAME type that AWS
    /// does not publish for a secure reference, and one Amazon RDS reports back.
    DatabaseUsername,
    /// `AWS::SSM::Parameter`'s `Value` — no type publishes it for a secure reference, and
    /// it is what lets a row read the deployed value back out of the store the stack really
    /// wrote it to.
    ParameterValue,
    /// `AWS::IAM::User`'s `LoginProfile.Password` — a published NESTED path, on a type the
    /// issue did not cite.
    UserPassword,
}

/// The logical id of the resource carrying the reference, whichever carrier it is.
const CARRIER: &str = "Carrier";

/// The parameter an `AWS::SSM::Parameter` carrier is deployed as.
fn copy_parameter(stack: &str) -> String {
    format!("/{stack}/copy")
}

/// The identifier an `AWS::RDS::DBInstance` carrier is deployed under. A stack name is
/// `[a-zA-Z][-a-zA-Z0-9]*`, which is already within what Amazon RDS accepts for one.
fn db_identifier(stack: &str) -> String {
    format!("{stack}-db")
}

/// The template for one row: the carrier's property is exactly `property`, so a row can put
/// the reference inside a longer value as well as alone.
fn template(carrier: Carrier, property: &str, stack: &str) -> String {
    match carrier {
        Carrier::DatabasePassword => json!({
            "Resources": {CARRIER: {
                "Type": "AWS::RDS::DBInstance",
                "Properties": {
                    "DBInstanceIdentifier": db_identifier(stack),
                    "Engine": "mysql",
                    "DBInstanceClass": "db.t3.micro",
                    "MasterUsername": "probeadmin",
                    "MasterUserPassword": property
                }
            }}
        })
        .to_string(),
        Carrier::DatabaseUsername => json!({
            "Resources": {CARRIER: {
                "Type": "AWS::RDS::DBInstance",
                "Properties": {
                    "DBInstanceIdentifier": db_identifier(stack),
                    "Engine": "mysql",
                    "DBInstanceClass": "db.t3.micro",
                    "MasterUsername": property,
                    "MasterUserPassword": "an-example-master-password"
                }
            }}
        })
        .to_string(),
        Carrier::ParameterValue => json!({
            "Resources": {CARRIER: {
                "Type": "AWS::SSM::Parameter",
                "Properties": {
                    "Name": copy_parameter(stack),
                    "Type": "String",
                    "Value": property
                }
            }}
        })
        .to_string(),
        Carrier::UserPassword => json!({
            "Resources": {CARRIER: {
                "Type": "AWS::IAM::User",
                "Properties": {"LoginProfile": {"Password": property}}
            }}
        })
        .to_string(),
    }
}

/// One row of the resolution table.
struct Row<'a> {
    case: &'a str,
    /// The stack name, which also seeds the parameter and database names so rows cannot
    /// collide.
    stack: &'a str,
    /// The parameter the row writes before deploying: its `Type` and its value. `None` for a
    /// row whose reference names a parameter nobody created.
    held: Option<(&'a str, &'a str)>,
    /// The property value the carrier declares, with `{name}` standing for the parameter
    /// name the row wrote.
    property: &'a str,
    carrier: Carrier,
    expected: Deployed,
    /// For a row that completes and whose carrier reports the property back: the value the
    /// deployed resource must carry, which is the property with its reference resolved.
    deployed_value: Option<&'a str>,
    /// For a row that rolls back: the fixed texts the STACK's own `ResourceStatusReason`
    /// must carry, so the reason says WHY the reference could not be honoured and not merely
    /// that something could not be created. `{name}` stands for the parameter name.
    reason: &'a [&'a str],
}

/// The `(ResourceStatus, ResourceStatusReason)` of every event a stack recorded for one
/// logical id, newest first.
///
/// A `StackEvent` holds no list of its own, so each `<member>` of the response is one event
/// and the shared leaf reader ([`element`]) reads its members.
async fn resource_events(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> Vec<(String, String)> {
    let (status_code, body) =
        call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
    assert_eq!(
        status_code, 200,
        "DescribeStackEvents({stack}) must answer 200, got {status_code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .filter(|entry| element(entry, "LogicalResourceId").as_deref() == Some(logical_id))
        .map(|entry| {
            (
                element(entry, "ResourceStatus").unwrap_or_default(),
                element(entry, "ResourceStatusReason").unwrap_or_default(),
            )
        })
        .collect()
}

/// What the carrier reports for the property the reference stood in, where its service
/// reports it at all.
async fn deployed_property(
    router: &MultiServiceRouter,
    carrier: Carrier,
    stack: &str,
) -> Option<String> {
    match carrier {
        Carrier::ParameterValue => parameter_value(router, &copy_parameter(stack)).await,
        Carrier::DatabaseUsername => master_username(router, &db_identifier(stack)).await,
        // A password is a value no read of either service reports back, which is the whole
        // reason those are the properties a secure reference is supported in.
        Carrier::DatabasePassword | Carrier::UserPassword => None,
    }
}

#[tokio::test]
async fn a_dynamic_reference_is_honoured_only_as_its_form_allows() {
    let rows: &[Row<'_>] = &[
        Row {
            case: "the recorded case: ssm-secure against a plain String parameter",
            stack: "dynref-secure-on-string",
            held: Some(("String", "not-a-secret")),
            property: "{{resolve:ssm-secure:{name}:1}}",
            carrier: Carrier::DatabasePassword,
            expected: Deployed::RolledBack,
            deployed_value: None,
            reason: &["ssm-secure prefix was used for non-secure parameter", "{name}"],
        },
        Row {
            case: "ssm-secure against a StringList parameter is the same misuse",
            stack: "dynref-secure-on-list",
            held: Some(("StringList", "one,two")),
            property: "{{resolve:ssm-secure:{name}}}",
            carrier: Carrier::DatabasePassword,
            expected: Deployed::RolledBack,
            deployed_value: None,
            reason: &["ssm-secure prefix was used for non-secure parameter", "{name}"],
        },
        Row {
            case: "ssm-secure against a SecureString parameter resolves",
            stack: "dynref-secure-on-secure",
            held: Some(("SecureString", "a-real-secret")),
            property: "{{resolve:ssm-secure:{name}:1}}",
            carrier: Carrier::DatabasePassword,
            expected: Deployed::Completed,
            deployed_value: None,
            reason: &[],
        },
        Row {
            case: "ssm-secure with no version selector resolves the latest",
            stack: "dynref-secure-no-version",
            held: Some(("SecureString", "a-real-secret")),
            property: "{{resolve:ssm-secure:{name}}}",
            carrier: Carrier::DatabasePassword,
            expected: Deployed::Completed,
            deployed_value: None,
            reason: &[],
        },
        Row {
            case: "ssm-secure in a published NESTED property path resolves",
            stack: "dynref-secure-nested",
            held: Some(("SecureString", "a-real-secret")),
            property: "{{resolve:ssm-secure:{name}}}",
            carrier: Carrier::UserPassword,
            expected: Deployed::Completed,
            deployed_value: None,
            reason: &[],
        },
        Row {
            case: "ssm-secure naming a version the parameter does not have",
            stack: "dynref-secure-bad-version",
            held: Some(("SecureString", "a-real-secret")),
            property: "{{resolve:ssm-secure:{name}:9}}",
            carrier: Carrier::DatabasePassword,
            expected: Deployed::RolledBack,
            deployed_value: None,
            reason: &["ParameterVersionNotFound", "{name}"],
        },
        Row {
            case: "ssm-secure naming a parameter nobody created",
            stack: "dynref-secure-missing",
            held: None,
            property: "{{resolve:ssm-secure:{name}:1}}",
            carrier: Carrier::DatabasePassword,
            expected: Deployed::RolledBack,
            deployed_value: None,
            reason: &["ParameterNotFound", "{name}"],
        },
        Row {
            case: "ssm-secure in a property of the same type AWS does not publish for it",
            stack: "dynref-secure-unsupported",
            held: Some(("SecureString", "a-real-secret")),
            property: "{{resolve:ssm-secure:{name}:1}}",
            carrier: Carrier::DatabaseUsername,
            expected: Deployed::RolledBack,
            deployed_value: None,
            reason: &[
                "SSM Secure reference is not supported in: \
                 [AWS::RDS::DBInstance/Properties/MasterUsername]",
            ],
        },
        Row {
            case: "ssm-secure in a property no resource type publishes for it",
            stack: "dynref-secure-in-parameter",
            held: Some(("SecureString", "a-real-secret")),
            property: "{{resolve:ssm-secure:{name}}}",
            carrier: Carrier::ParameterValue,
            expected: Deployed::RolledBack,
            deployed_value: None,
            reason: &[
                "SSM Secure reference is not supported in: \
                 [AWS::SSM::Parameter/Properties/Value]",
            ],
        },
        Row {
            case: "ssm against a String parameter resolves, version selector and all",
            stack: "dynref-plain-on-string",
            held: Some(("String", "plainly-stored")),
            property: "{{resolve:ssm:{name}:1}}",
            carrier: Carrier::ParameterValue,
            expected: Deployed::Completed,
            deployed_value: Some("plainly-stored"),
            reason: &[],
        },
        Row {
            case: "ssm against a StringList parameter resolves",
            stack: "dynref-plain-on-list",
            held: Some(("StringList", "one,two")),
            property: "{{resolve:ssm:{name}}}",
            carrier: Carrier::ParameterValue,
            expected: Deployed::Completed,
            deployed_value: Some("one,two"),
            reason: &[],
        },
        Row {
            case: "the plain form is bound by no property list: it resolves anywhere",
            stack: "dynref-plain-username",
            held: Some(("String", "resolvedadmin")),
            property: "{{resolve:ssm:{name}}}",
            carrier: Carrier::DatabaseUsername,
            expected: Deployed::Completed,
            deployed_value: Some("resolvedadmin"),
            reason: &[],
        },
        Row {
            case: "ssm against a SecureString parameter is a misuse of the plain form",
            stack: "dynref-plain-on-secure",
            held: Some(("SecureString", "a-real-secret")),
            property: "{{resolve:ssm:{name}}}",
            carrier: Carrier::ParameterValue,
            expected: Deployed::RolledBack,
            deployed_value: None,
            reason: &["ssm prefix was used for secure parameter", "{name}"],
        },
        Row {
            case: "ssm naming a parameter nobody created",
            stack: "dynref-plain-missing",
            held: None,
            property: "{{resolve:ssm:{name}}}",
            carrier: Carrier::ParameterValue,
            expected: Deployed::RolledBack,
            deployed_value: None,
            reason: &["ParameterNotFound", "{name}"],
        },
        Row {
            case: "a reference INSIDE a longer property value resolves in place",
            stack: "dynref-embedded",
            held: Some(("String", "middle")),
            property: "head-{{resolve:ssm:{name}}}-tail",
            carrier: Carrier::ParameterValue,
            expected: Deployed::Completed,
            deployed_value: Some("head-middle-tail"),
            reason: &[],
        },
        Row {
            case: "a property carrying no reference at all is deployed as it is written",
            stack: "dynref-none",
            held: Some(("String", "unused")),
            property: "a plain literal",
            carrier: Carrier::ParameterValue,
            expected: Deployed::Completed,
            deployed_value: Some("a plain literal"),
            reason: &[],
        },
    ];

    for row in rows {
        let router = router();
        let name = format!("/{}/source", row.stack);
        if let Some((parameter_type, value)) = row.held {
            put_parameter(&router, row.case, &name, parameter_type, value).await;
        }
        let property = row.property.replace("{name}", &name);
        let (status_code, created) = create_stack(
            &router,
            row.stack,
            &template(row.carrier, &property, row.stack),
            // Acknowledged for every row, so the one declaring an IAM resource is accepted
            // and the others are unaffected.
            &acknowledging_iam(&[]),
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must answer 200 — the deployment is what reports the \
             outcome — got {status_code} {created}",
            row.case
        );
        // Twice: a failed create owes its rollback a transition of its own past the one the
        // create itself took.
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        let (status_code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeStacks must answer 200, got {status_code} {described}",
            row.case
        );
        let expected_status = match row.expected {
            Deployed::Completed => status::CREATE_COMPLETE,
            Deployed::RolledBack => status::ROLLBACK_COMPLETE,
        };
        let actual = xml_text(&described, "StackStatus").unwrap_or_default();
        assert_eq!(
            actual, expected_status,
            "case {:?}: expected {expected_status}, got {actual:?} in {described}",
            row.case
        );

        // A reference the deployment cannot honour is CLOUDFORMATION's own refusal: the
        // diagnostic is on the stack's own events and nothing was asked about the resource
        // carrying it. One it can honour is resolved and the resource really deployed.
        let events = resource_events(&router, row.stack, CARRIER).await;
        let stack_events = resource_events(&router, row.stack, row.stack).await;
        let failed = events
            .iter()
            .find(|(event_status, _)| event_status == resource_status::CREATE_FAILED);
        match row.expected {
            Deployed::RolledBack => {
                // The newest event the stack recorded about ITSELF that carries a reason:
                // what a client reads to learn why the deployment failed.
                let Some((_, reason)) = stack_events
                    .iter()
                    .find(|(_, reason)| !reason.is_empty())
                else {
                    panic!(
                        "case {:?}: expected the stack's own events to report why the \
                         deployment failed, got {stack_events:?}",
                        row.case
                    );
                };
                let stack_reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
                for fragment in row.reason {
                    let fragment = fragment.replace("{name}", &name);
                    assert!(
                        reason.contains(&fragment),
                        "case {:?}: expected the stack's reason to carry {fragment:?}, got \
                         {reason:?}",
                        row.case
                    );
                    assert!(
                        stack_reason.contains(&fragment),
                        "case {:?}: expected StackStatusReason to carry {fragment:?}, got \
                         {stack_reason:?}",
                        row.case
                    );
                }
                // Nothing was asked of the service that owns the resource carrying the
                // reference: CloudFormation refused the deployment before it deployed
                // anything, so that resource has no event of its own at all.
                assert!(
                    events.is_empty(),
                    "case {:?}: expected no event for {CARRIER} — nothing was deployed — got \
                     {events:?}",
                    row.case
                );
            }
            Deployed::Completed => {
                assert!(
                    failed.is_none(),
                    "case {:?}: a reference the deployment can honour fails nothing, got \
                     {events:?}",
                    row.case
                );
                // The resource's create was really INITIATED — the deployment got past
                // CloudFormation's own resolution of the reference and asked for the
                // resource, rather than completing a resource nothing was asked for.
                assert!(
                    events.iter().any(|(event_status, reason)| {
                        event_status == resource_status::CREATE_IN_PROGRESS
                            && reason == RESOURCE_CREATION_INITIATED
                    }),
                    "case {:?}: expected a {} event reporting {RESOURCE_CREATION_INITIATED:?}, \
                     got {events:?}",
                    row.case,
                    resource_status::CREATE_IN_PROGRESS
                );
            }
        }

        // Where the carrier's own service reports the property back, it reports what the
        // reference RESOLVED TO: the value reached the service that owns the resource,
        // rather than the reference's own text.
        if let Some(expected_value) = row.deployed_value {
            let held = deployed_property(&router, row.carrier, row.stack).await;
            assert_eq!(
                held.as_deref(),
                Some(expected_value),
                "case {:?}: expected the deployed resource to carry {expected_value:?}, got \
                 {held:?}",
                row.case
            );
        }
    }
}

/// CloudFormation's resolution of a template's dynamic references is its OWN gate on the
/// deployment, run BEFORE any owning service is asked for anything: a reference it cannot
/// honour fails the stack with the resolver's diagnostic and NOTHING is deployed — not even
/// a resource the template declares ahead of the one carrying the reference.
#[tokio::test]
async fn a_reference_that_cannot_be_honoured_is_refused_before_anything_is_deployed() {
    let router = router();
    let stack = "dynref-gate";
    // A parameter held as plain text, which is not what the secure form resolves.
    let source = format!("/{stack}/source");
    put_parameter(&router, "the gate", &source, "String", "not-a-secret").await;

    // Two resources, with the misuse on the one declared SECOND: a deployment that resolved
    // each reference only as it reached that resource would already have created `Alpha` in
    // Systems Manager by the time it reached `Zulu`.
    let deployed_by_the_stack = format!("/{stack}/alpha");
    let misuse = format!("{{{{resolve:ssm-secure:{source}}}}}");
    let template = json!({
        "Resources": {
            "Alpha": {
                "Type": "AWS::SSM::Parameter",
                "Properties": {
                    "Name": deployed_by_the_stack,
                    "Type": "String",
                    "Value": "a plain literal"
                }
            },
            "Zulu": {
                "Type": "AWS::RDS::DBInstance",
                "Properties": {
                    "DBInstanceIdentifier": db_identifier(stack),
                    "Engine": "mysql",
                    "DBInstanceClass": "db.t3.micro",
                    "MasterUsername": "probeadmin",
                    "MasterUserPassword": misuse
                }
            }
        }
    })
    .to_string();
    let (status_code, created) = create_stack(&router, stack, &template, &[]).await;
    assert_eq!(
        status_code, 200,
        "CreateStack must answer 200 — the deployment is what reports the outcome — got \
         {status_code} {created}"
    );
    // Twice: a failed create owes its rollback a transition of its own past the one the
    // create itself took.
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let (_, described) = describe_stack(&router, stack).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::ROLLBACK_COMPLETE),
        "a reference the deployment cannot honour rolls the create back, got {described}"
    );
    let stack_reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
    for fragment in [
        "ssm-secure prefix was used for non-secure parameter",
        source.as_str(),
    ] {
        assert!(
            stack_reason.contains(fragment),
            "case {stack:?}: expected the stack's reason to carry {fragment:?}, got \
             {stack_reason:?}"
        );
    }

    // Neither resource was asked of its owning service: the parameter the stack would have
    // written is not in Systems Manager, the database is not in Amazon RDS, and neither
    // logical resource has an event of its own.
    let alpha = parameter_value(&router, &deployed_by_the_stack).await;
    assert_eq!(
        alpha, None,
        "expected the resource declared ahead of the misuse never to be deployed, got \
         {alpha:?}"
    );
    let zulu = master_username(&router, &db_identifier(stack)).await;
    assert_eq!(
        zulu, None,
        "expected the resource carrying the misuse never to be deployed, got {zulu:?}"
    );
    for logical_id in ["Alpha", "Zulu"] {
        let events = resource_events(&router, stack, logical_id).await;
        assert!(
            events.is_empty(),
            "case {logical_id:?}: expected no resource event — nothing was deployed — got \
             {events:?}"
        );
    }
}

/// A SECURE reference's value reaches the service that owns the resource and nothing else:
/// the stack's own records and events keep the reference exactly as the template wrote it,
/// because the point of the reference is that the secret stays in the store it is held in.
#[tokio::test]
async fn a_resolved_secure_reference_is_kept_out_of_what_the_stack_reports() {
    let router = router();
    let stack = "dynref-secrecy";
    let secret = "a-real-secret-value";
    let source = "/dynref-secrecy/source";
    put_parameter(&router, "secrecy", source, "SecureString", secret).await;

    let reference = format!("{{{{resolve:ssm-secure:{source}:1}}}}");
    let (status_code, created) = create_stack(
        &router,
        stack,
        &template(Carrier::DatabasePassword, &reference, stack),
        &[],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "CreateStack must answer 200, got {status_code} {created}"
    );
    wait_for_the_transition().await;

    let (_, described) = describe_stack(&router, stack).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "the secure reference resolves in a property that supports it, so the stack \
         completes, got {described}"
    );

    let (_, events) = call(&router, "DescribeStackEvents", &[member("StackName", stack)]).await;
    let (_, resources) = call(
        &router,
        "DescribeStackResources",
        &[member("StackName", stack)],
    )
    .await;
    let (_, template_body) = crate::stack_lifecycle::get_template(&router, stack, None).await;
    for (case, body) in [
        ("DescribeStacks", &described),
        ("DescribeStackEvents", &events),
        ("DescribeStackResources", &resources),
        ("GetTemplate", &template_body),
    ] {
        assert!(
            !body.contains(secret),
            "case {case:?}: the resolved secure value must not appear in what the stack \
             reports, got {body}"
        );
    }
    // The event that reports what the resource was created with reports the REFERENCE, which
    // is what the stack holds for it.
    assert!(
        events.contains("resolve:ssm-secure"),
        "expected the properties the events report to carry the reference as the template \
         wrote it, got {events}"
    );
}
