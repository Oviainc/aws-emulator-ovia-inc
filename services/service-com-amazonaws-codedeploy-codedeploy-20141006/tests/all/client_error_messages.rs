// SPDX-License-Identifier: CC0-1.0

//! The `$.message` each CodeDeploy client error carries, over a SET of requests.
//!
//! The rule under test: a modelled client error of this service carries the
//! sentence that belongs to the ERROR SHAPE, interpolating only the request value
//! that error is about — the resource's own name or id, delimited from the prose
//! by `: ` where the sentence ends in it — and no other request member; a refusal
//! of a member the request did not give (absent or empty) answers that member's
//! `…RequiredException` with its own fixed sentence rather than the constraint
//! validator's generic clause, while a value that IS present but unacceptable
//! keeps the clause naming the member and the constraint it failed.
//!
//! So each table spans the class the templates generalise over: which operation
//! raises the error (five of them raise `ApplicationDoesNotExistException`),
//! which layer refuses it (the boundary validator or the handler), and the form
//! of the name or id the request carried — every row's expectation is BUILT from
//! that row's own request rather than copied from a recording.
//!
//! The wire plumbing is the lifecycle suite's shared harness
//! (`tests/all/codedeploy_lifecycle.rs`).

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, s3_revision, seeded_group, Emulator};

/// An IAM role ARN, the shape `CreateDeploymentGroup` takes.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// The `$.message` of a refused call.
fn message(body: &Value, case: &str) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a string $.message, got {body}"))
        .to_string()
}

/// Call `operation`, require HTTP 400 with `code`, and answer the message.
async fn refused_message(
    emulator: &Emulator,
    operation: &str,
    request: Value,
    code: &str,
    case: &str,
) -> String {
    let (status, body) = emulator.call(operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from {operation}, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        code,
        "case {case:?}: expected {code} from {operation}, got {body:?}"
    );
    message(&body, case)
}

/// As [`refused_message`], but `None` when the operation is still one of the
/// crate's scaffolded 501s.
///
/// A row for an operation whose model DECLARES the error shape can then be
/// written before the operation is implemented: while it answers 501 the row
/// asserts nothing, and the day a handler is written it must word the shape the
/// way every other operation does or this suite fails. That is what keeps the
/// rule from silently regressing as more operations land.
async fn refused_message_unless_scaffolded(
    emulator: &Emulator,
    operation: &str,
    request: Value,
    code: &str,
    case: &str,
) -> Option<String> {
    let (status, body) = emulator.call(operation, request.clone()).await;
    if status == 501 {
        return None;
    }
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 (or the scaffold's 501) from {operation}, got {status} \
         {body:?}"
    );
    assert_eq!(
        error_code(&body),
        code,
        "case {case:?}: expected {code} from {operation}, got {body:?}"
    );
    Some(message(&body, case))
}

/// The name forms an accepted CodeDeploy name takes: `ApplicationName` and
/// `DeploymentGroupName` are both `@length(1..100)` with pattern
/// `^[A-Za-z0-9+=,.@_-]*$`
/// (`smithy_query(query="operation",operation="GetDeploymentGroup")`), so the
/// rows below differ along that class — plain, every punctuation character the
/// pattern admits, a single character, and the published maximum length.
fn name_forms(prefix: &str) -> Vec<(String, String)> {
    vec![
        ("plain".to_string(), format!("{prefix}-plain")),
        (
            "every punctuation character the pattern admits".to_string(),
            format!("{prefix}+a=b,c.d@e_f-g"),
        ),
        (
            // The modelled minimum length, taken from the prefix so this row is
            // still this table's own name.
            "one character".to_string(),
            prefix.chars().next().unwrap_or('x').to_string(),
        ),
        (
            "the modelled maximum length (100)".to_string(),
            format!(
                "{prefix}{}",
                "n".repeat(100usize.saturating_sub(prefix.len()))
            ),
        ),
    ]
}

/// RULE: `ApplicationDoesNotExistException` says
/// `No application found for name: <applicationName>` — the requested name after
/// the delimiter — whichever operation looked the application up.
#[tokio::test]
async fn an_unknown_application_is_named_after_the_delimiter() {
    let emulator = Emulator::new();

    for (form, ghost) in name_forms("ghost-app") {
        // Rows differ along the operation that raises the error: a read, a
        // listing, and the two creates that resolve an application.
        let requests: Vec<(&str, Value)> = vec![
            ("GetApplication", json!({"applicationName": ghost})),
            (
                // `ListDeployments` takes `applicationName` and
                // `deploymentGroupName` as a pair, so the listing names both.
                "ListDeployments",
                json!({"applicationName": ghost, "deploymentGroupName": "any-group"}),
            ),
            (
                "GetDeploymentGroup",
                json!({"applicationName": ghost, "deploymentGroupName": "any-group"}),
            ),
            (
                "CreateDeploymentGroup",
                json!({
                    "applicationName": ghost,
                    "deploymentGroupName": "any-group",
                    "serviceRoleArn": SERVICE_ROLE,
                }),
            ),
            (
                "CreateDeployment",
                json!({
                    "applicationName": ghost,
                    "deploymentGroupName": "any-group",
                    "revision": s3_revision("bucket", "bundle.zip"),
                }),
            ),
            (
                "GetApplicationRevision",
                json!({
                    "applicationName": ghost,
                    "revision": s3_revision("bucket", "bundle.zip"),
                }),
            ),
            (
                "RegisterApplicationRevision",
                json!({
                    "applicationName": ghost,
                    "revision": s3_revision("bucket", "bundle.zip"),
                }),
            ),
            (
                "ListApplicationRevisions",
                json!({"applicationName": ghost}),
            ),
            (
                // One unknown name fails the whole batch, named by this sentence.
                "BatchGetApplications",
                json!({"applicationNames": [ghost]}),
            ),
            (
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": ghost,
                    "currentDeploymentGroupName": "any-group",
                }),
            ),
            // The remaining operations whose model declares this error
            // (OPERATIONS.md): rows written now so that the day each is
            // implemented it has to word the shape the way the rest do. While an
            // operation is still the scaffold's 501 its row asserts nothing.
            (
                "UpdateApplication",
                json!({"applicationName": ghost, "newApplicationName": "renamed"}),
            ),
            (
                "ListDeploymentGroups",
                json!({"applicationName": ghost}),
            ),
            // `DeleteDeploymentGroup` is NOT in this table: its model declares no
            // `ApplicationDoesNotExistException` (OPERATIONS.md lists its five
            // errors: the two name-required and two invalid-name refusals, and
            // `InvalidRoleException`), because deleting a group nothing holds is a
            // success rather than a refusal. The suite
            // `tests/all/delete_deployment_group.rs` pins that instead.
            (
                "BatchGetDeploymentGroups",
                json!({"applicationName": ghost, "deploymentGroupNames": ["any-group"]}),
            ),
            (
                "BatchGetApplicationRevisions",
                json!({
                    "applicationName": ghost,
                    "revisions": [s3_revision("bucket", "bundle.zip")],
                }),
            ),
        ];

        for (operation, request) in requests {
            let case = format!("{operation} with {form} name");
            let Some(actual) = refused_message_unless_scaffolded(
                &emulator,
                operation,
                request,
                "ApplicationDoesNotExistException",
                &case,
            )
            .await
            else {
                continue;
            };
            let expected = format!("No application found for name: {ghost}");
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

/// RULE: `DeploymentGroupDoesNotExistException` says
/// `No Deployment Group found for name: <deploymentGroupName>` — the group name
/// alone; the application the group was looked up in is NOT part of the sentence.
#[tokio::test]
async fn an_unknown_deployment_group_is_named_alone() {
    let emulator = Emulator::new();

    for (index, (form, ghost)) in name_forms("ghost-dg").into_iter().enumerate() {
        // Each row gets its own application, whose name must not be a substring
        // of the group name, so a leak of it into the sentence is detectable.
        let application = format!("hosting-application-{index}");
        let group = seeded_group(&emulator, &application, "Server").await;
        assert!(
            !ghost.contains(&application) && ghost != group,
            "case {form:?}: the row's ghost group must differ from the seeded one"
        );

        let requests: Vec<(&str, Value)> = vec![
            (
                "GetDeploymentGroup",
                json!({"applicationName": application, "deploymentGroupName": ghost}),
            ),
            (
                "ListDeployments",
                json!({"applicationName": application, "deploymentGroupName": ghost}),
            ),
            (
                "CreateDeployment",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": ghost,
                    "revision": s3_revision("bucket", "bundle.zip"),
                }),
            ),
            (
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "currentDeploymentGroupName": ghost,
                }),
            ),
        ];

        for (operation, request) in requests {
            let case = format!("{operation} with {form} group name");
            let actual = refused_message(
                &emulator,
                operation,
                request,
                "DeploymentGroupDoesNotExistException",
                &case,
            )
            .await;
            let expected = format!("No Deployment Group found for name: {ghost}");
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            assert!(
                !actual.contains(&application),
                "case {case:?}: the sentence is scoped to the group, so it must not name the \
                 application {application:?}, got {actual:?}"
            );
        }
    }
}

/// RULE: `DeploymentDoesNotExistException` says
/// `The deployment <deploymentId> could not be found`, interpolating the id the
/// request carried, for every well-formed id that names no stored deployment.
#[tokio::test]
async fn an_unknown_deployment_is_named_inside_the_sentence() {
    let emulator = Emulator::new();
    // Rows span the ids CodeDeploy's own format admits — `d-` plus nine
    // characters of its uppercase-alphanumeric alphabet
    // (`state::deployment_id_is_well_formed`): the recorded one, all letters,
    // all digits and a mixture. A value OUTSIDE that format is a different
    // refusal (`InvalidDeploymentIdException`, worded by
    // `state::malformed_deployment_id_message`), not this sentence.
    let cases: &[(&str, &str)] = &[
        ("the recorded id", "d-B15C2FD24"),
        ("letters only", "d-ABCDEFGHI"),
        ("digits only", "d-123456789"),
        ("letters and digits", "d-A1B2C3D4E"),
    ];

    for (form, deployment_id) in cases {
        // Rows also differ along the operation that looks the deployment up: a
        // read, the stop, the wait-skip and the lifecycle-hook report, which
        // raise the same error from different layers. An operation still
        // answering the scaffold's 501 asserts nothing until it is implemented.
        for operation in [
            "GetDeployment",
            "StopDeployment",
            "SkipWaitTimeForInstanceTermination",
            "PutLifecycleEventHookExecutionStatus",
            "BatchGetDeploymentInstances",
            "ListDeploymentInstances",
            "ListDeploymentTargets",
        ] {
            let case = format!("{operation} with {form} id");
            let Some(actual) = refused_message_unless_scaffolded(
                &emulator,
                operation,
                json!({"deploymentId": deployment_id, "instanceIds": ["i-0123456789abcdef0"]}),
                "DeploymentDoesNotExistException",
                &case,
            )
            .await
            else {
                continue;
            };
            let expected = format!("The deployment {deployment_id} could not be found");
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

/// RULE: `DeploymentConfigDoesNotExistException` names the configuration after
/// the delimiter and nothing else from the request, whichever operation resolved
/// it.
#[tokio::test]
async fn an_unknown_deployment_configuration_is_named_after_the_delimiter() {
    let emulator = Emulator::new();
    let application = "configuration-resolution";
    let group = seeded_group(&emulator, application, "Server").await;

    for (index, (form, ghost)) in name_forms("ghost-config").into_iter().enumerate() {
        let requests: Vec<(&str, Value)> = vec![
            (
                "GetDeploymentConfig",
                json!({"deploymentConfigName": ghost}),
            ),
            (
                "CreateDeployment",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": group,
                    "deploymentConfigName": ghost,
                    "revision": s3_revision("bucket", "bundle.zip"),
                }),
            ),
            (
                "CreateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": format!("new-group-{index}"),
                    "serviceRoleArn": SERVICE_ROLE,
                    "deploymentConfigName": ghost,
                }),
            ),
            (
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "currentDeploymentGroupName": group,
                    "deploymentConfigName": ghost,
                }),
            ),
        ];

        for (operation, request) in requests {
            let case = format!("{operation} with {form} configuration name");
            let actual = refused_message(
                &emulator,
                operation,
                request,
                "DeploymentConfigDoesNotExistException",
                &case,
            )
            .await;
            let expected = format!("No deployment configuration found for name: {ghost}");
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            assert!(
                !actual.contains(application),
                "case {case:?}: the sentence is scoped to the configuration, so it must not name \
                 the application {application:?}, got {actual:?}"
            );
        }
    }
}

/// RULE: `ApplicationAlreadyExistsException` says `<applicationName> already
/// exists.` — the conflicting name FIRST, then the fixed tail — and
/// `DeploymentGroupAlreadyExistsException` says `An Deployment Group with the
/// name <deploymentGroupName> already exists for this application.`, referring to
/// the application rather than naming it.
#[tokio::test]
async fn a_conflicting_name_is_reported_in_the_shapes_own_sentence() {
    let emulator = Emulator::new();

    for (index, (form, name)) in name_forms("dup").into_iter().enumerate() {
        emulator
            .ok("CreateApplication", json!({"applicationName": name}))
            .await;
        let case = format!("CreateApplication twice with a {form} name");
        let actual = refused_message(
            &emulator,
            "CreateApplication",
            json!({"applicationName": name}),
            "ApplicationAlreadyExistsException",
            &case,
        )
        .await;
        let expected = format!("{name} already exists.");
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );

        // The same name form, now as a deployment group of an application whose
        // name cannot be mistaken for a substring of it.
        let application = format!("owning-application-{index}");
        emulator
            .ok("CreateApplication", json!({"applicationName": application}))
            .await;
        let group_request = json!({
            "applicationName": application,
            "deploymentGroupName": name,
            "serviceRoleArn": SERVICE_ROLE,
        });
        emulator
            .ok("CreateDeploymentGroup", group_request.clone())
            .await;
        let case = format!("CreateDeploymentGroup twice with a {form} name");
        let actual = refused_message(
            &emulator,
            "CreateDeploymentGroup",
            group_request,
            "DeploymentGroupAlreadyExistsException",
            &case,
        )
        .await;
        let expected = format!(
            "An Deployment Group with the name {name} already exists for this application."
        );
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            !actual.contains(&application),
            "case {case:?}: the sentence refers to \"this application\" rather than naming it, \
             got {actual:?}"
        );

        // The same conflict reached by a RENAME: updating another group of the
        // same application onto this name collides with it, and the rename is
        // refused in the same words as the create.
        let renamed = format!("renamed-group-{index}");
        emulator
            .ok(
                "CreateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": renamed,
                    "serviceRoleArn": SERVICE_ROLE,
                }),
            )
            .await;
        let case = format!("UpdateDeploymentGroup renaming onto a {form} name");
        let actual = refused_message(
            &emulator,
            "UpdateDeploymentGroup",
            json!({
                "applicationName": application,
                "currentDeploymentGroupName": renamed,
                "newDeploymentGroupName": name,
            }),
            "DeploymentGroupAlreadyExistsException",
            &case,
        )
        .await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            !actual.contains(&application),
            "case {case:?}: the sentence refers to \"this application\" rather than naming it, \
             got {actual:?}"
        );
    }
}

/// RULE: a member the request did not give — absent or empty, which are the same
/// refusal to CodeDeploy — is answered with that member's `…RequiredException`
/// carrying the FIXED sentence that error owns, with no value interpolated and
/// none of the constraint validator's clause, whichever layer refuses it (the
/// boundary validator for a modelled `@length` minimum, the handler for a member
/// the model leaves unconstrained).
#[tokio::test]
async fn a_member_the_request_did_not_give_is_answered_with_its_own_sentence() {
    let emulator = Emulator::new();
    let group = seeded_group(&emulator, "required-members", "Server").await;
    let revision = s3_revision("bucket", "bundle.zip");

    // Rows span the members, both ways of not giving one (absent / empty) and
    // both refusing layers.
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "CreateApplication",
            json!({}),
            "ApplicationNameRequiredException",
            "Application name is missing",
        ),
        (
            "CreateApplication",
            json!({"applicationName": ""}),
            "ApplicationNameRequiredException",
            "Application name is missing",
        ),
        (
            "GetApplication",
            json!({"applicationName": ""}),
            "ApplicationNameRequiredException",
            "Application name is missing",
        ),
        (
            "DeleteApplication",
            json!({}),
            "ApplicationNameRequiredException",
            "Application name is missing",
        ),
        (
            "CreateDeployment",
            json!({"deploymentGroupName": group, "revision": revision}),
            "ApplicationNameRequiredException",
            "Application name is missing",
        ),
        (
            "CreateDeployment",
            json!({
                "applicationName": "",
                "deploymentGroupName": group,
                "revision": revision,
            }),
            "ApplicationNameRequiredException",
            "Application name is missing",
        ),
        (
            // The boundary refuses the empty name before the handler can look
            // at the rest of the request, so an absent revision does not change
            // which error this answers.
            "CreateDeployment",
            json!({"applicationName": "", "deploymentGroupName": "whatever-group"}),
            "ApplicationNameRequiredException",
            "Application name is missing",
        ),
        (
            "ListDeployments",
            json!({"deploymentGroupName": group}),
            "ApplicationNameRequiredException",
            "Application name is missing",
        ),
        (
            // An empty list is the same refusal as an absent `applicationNames`.
            "BatchGetApplications",
            json!({"applicationNames": []}),
            "ApplicationNameRequiredException",
            "Application name is missing",
        ),
        (
            "BatchGetDeployments",
            json!({"deploymentIds": []}),
            "DeploymentIdRequiredException",
            "Deployment ID is missing",
        ),
        (
            // A blank id inside a non-empty list is the same refusal again.
            "BatchGetDeployments",
            json!({"deploymentIds": ["  "]}),
            "DeploymentIdRequiredException",
            "Deployment ID is missing",
        ),
        (
            "GetDeploymentGroup",
            json!({"applicationName": "required-members", "deploymentGroupName": ""}),
            "DeploymentGroupNameRequiredException",
            "Deployment Group name is missing",
        ),
        (
            // The handler refuses this one: `deploymentGroupName` is modelled
            // OPTIONAL on CreateDeployment, so an absent one never reaches the
            // boundary validator.
            "CreateDeployment",
            json!({"applicationName": "required-members", "revision": revision}),
            "DeploymentGroupNameRequiredException",
            "Deployment Group name is missing",
        ),
        (
            "GetDeployment",
            json!({}),
            "DeploymentIdRequiredException",
            "Deployment ID is missing",
        ),
        (
            // `deploymentId` carries no modelled length, so the handler is what
            // refuses an empty one — and it answers the same sentence.
            "GetDeployment",
            json!({"deploymentId": ""}),
            "DeploymentIdRequiredException",
            "Deployment ID is missing",
        ),
        (
            "GetDeploymentConfig",
            json!({"deploymentConfigName": ""}),
            "DeploymentConfigNameRequiredException",
            "Deployment configuration name is missing",
        ),
        (
            "CreateDeploymentGroup",
            json!({
                "applicationName": "required-members",
                "deploymentGroupName": "new-group",
                "serviceRoleArn": "",
            }),
            "RoleRequiredException",
            "Service role ARN is missing",
        ),
        (
            "CreateDeployment",
            json!({"applicationName": "required-members", "deploymentGroupName": group}),
            "RevisionRequiredException",
            "Revision is missing",
        ),
        (
            "StopDeployment",
            json!({}),
            "DeploymentIdRequiredException",
            "Deployment ID is missing",
        ),
        (
            // `deploymentId` is modelled OPTIONAL on this operation, so its
            // absence is the handler's refusal, worded the same as the others.
            "SkipWaitTimeForInstanceTermination",
            json!({}),
            "DeploymentIdRequiredException",
            "Deployment ID is missing",
        ),
        (
            // `StopDeployment` reads an empty id the way `GetDeployment` does,
            // in its own handler, and answers the same sentence.
            "StopDeployment",
            json!({"deploymentId": ""}),
            "DeploymentIdRequiredException",
            "Deployment ID is missing",
        ),
        // The remaining members of the refusal table. Their operations are still
        // scaffolded 501s, but the boundary refuses a member the request did not
        // give BEFORE any handler runs, so the sentence is the service's either
        // way.
        (
            "GetOnPremisesInstance",
            json!({}),
            "InstanceNameRequiredException",
            "Instance name is missing",
        ),
        (
            // `instanceName` carries no modelled length, so an empty one is the
            // handler's refusal — and it answers the same sentence as the
            // boundary's.
            "GetOnPremisesInstance",
            json!({"instanceName": "   "}),
            "InstanceNameRequiredException",
            "Instance name is missing",
        ),
        (
            "RegisterOnPremisesInstance",
            json!({"instanceName": "", "iamUserArn": "arn:aws:iam::123456789012:user/op"}),
            "InstanceNameRequiredException",
            "Instance name is missing",
        ),
        (
            "DeregisterOnPremisesInstance",
            json!({"instanceName": "  "}),
            "InstanceNameRequiredException",
            "Instance name is missing",
        ),
        (
            // An empty list is the same refusal as an absent `instanceNames`.
            "BatchGetOnPremisesInstances",
            json!({"instanceNames": []}),
            "InstanceNameRequiredException",
            "Instance name is missing",
        ),
        (
            "AddTagsToOnPremisesInstances",
            json!({"instanceNames": [], "tags": [{"Key": "k", "Value": "v"}]}),
            "InstanceNameRequiredException",
            "Instance name is missing",
        ),
        (
            // A blank name inside the list, refused by the same operation.
            "AddTagsToOnPremisesInstances",
            json!({"instanceNames": ["  "], "tags": [{"Key": "k", "Value": "v"}]}),
            "InstanceNameRequiredException",
            "Instance name is missing",
        ),
        (
            "BatchGetOnPremisesInstances",
            json!({}),
            "InstanceNameRequiredException",
            "Instance name is missing",
        ),
        (
            "BatchGetDeploymentInstances",
            json!({"deploymentId": "d-ABCDEFGHI"}),
            "InstanceIdRequiredException",
            "Instance ID is missing",
        ),
        (
            "BatchGetDeploymentTargets",
            json!({"deploymentId": "d-ABCDEFGHI"}),
            "DeploymentTargetIdRequiredException",
            "Deployment target ID is missing",
        ),
        (
            // The model spells this member `ResourceArn`, capitalised, while the
            // refusal names it as `resourceArn`.
            "ListTagsForResource",
            json!({}),
            "ResourceArnRequiredException",
            "Resource ARN is missing",
        ),
        (
            "TagResource",
            json!({"ResourceArn": "", "Tags": [{"Key": "k", "Value": "v"}]}),
            "ResourceArnRequiredException",
            "Resource ARN is missing",
        ),
    ];

    for (operation, request, code, expected) in cases {
        let case = format!("{operation} {request}");
        let actual = refused_message(&emulator, operation, request, code, &case).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            !actual.contains("constraint"),
            "case {case:?}: a required-member error states the missing input, not a shape \
             constraint, got {actual:?}"
        );
    }
}

/// RULE: a value that IS present but is not one the service accepts is a fact
/// about the VALUE, so its `Invalid…Exception` keeps the clause built from the
/// request — naming the member and the constraint it failed — rather than a
/// fixed sentence.
#[tokio::test]
async fn a_value_the_service_will_not_accept_keeps_the_clause_naming_it() {
    let emulator = Emulator::new();
    seeded_group(&emulator, "invalid-values", "Server").await;

    let long_name = "n".repeat(101);
    // Rows span the refusal kinds a present value can fail: the pattern, the
    // length maximum, and an enum's value set.
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "GetApplication",
            json!({"applicationName": "has spaces"}),
            "InvalidApplicationNameException",
            "applicationName",
        ),
        (
            "CreateApplication",
            json!({"applicationName": long_name}),
            "InvalidApplicationNameException",
            "applicationName",
        ),
        (
            "GetDeploymentGroup",
            json!({"applicationName": "invalid-values", "deploymentGroupName": "has spaces"}),
            "InvalidDeploymentGroupNameException",
            "deploymentGroupName",
        ),
        (
            "CreateApplication",
            json!({"applicationName": "ok-name", "computePlatform": "Windows"}),
            "InvalidComputePlatformException",
            "computePlatform",
        ),
    ];

    for (operation, request, code, member) in cases {
        let case = format!("{operation} {request}");
        let actual = refused_message(&emulator, operation, request, code, &case).await;
        assert!(
            actual.contains(member) && actual.contains("failed to satisfy constraint"),
            "case {case:?}: expected a clause naming {member:?} and the constraint it failed, \
             got {actual:?}"
        );
    }
}
