// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of Amplify backend environments.
//!
//! Every case drives the real wire path (the shared harness in
//! `tests/all/app_branch_lifecycle.rs`), so what is asserted is the JSON an AWS SDK
//! client parses, not an internal function's return value.
//!
//! The rule under test: a create of a named resource under an Amplify app PERSISTS the
//! record under the app its URI names and answers exactly what it stored — so the
//! record is readable afterwards through that app and comes back field-for-field, with
//! the ARN the service derived from the app and the name, the optional members the
//! request carried and nothing invented for the ones it omitted, and `createTime`
//! equal to `updateTime` on a record nothing has updated. The name identifies the
//! resource within its app: a second create under the same name is refused and leaves
//! the stored record alone, an unknown app is refused, and the resource is reachable
//! only through its own app, so it goes when that app goes.
//!
//! The table spans the class the rule generalises over — `environmentName` at one
//! character (its modelled `length` minimum), at ordinary names and at the modelled
//! maximum of 255; `stackName` and `deploymentArtifacts` absent, supplied singly,
//! supplied together and at their modelled `length` maxima — rather than one recorded
//! request.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::app_branch_lifecycle::{
    string_member, timestamp_member, Emulator, Reply, ACCOUNT_ID, REGION,
};

/// Every member modelled on `com.amazonaws.amplify#BackendEnvironment`, as the wire
/// names `smithy_query(query="shape",shape="BackendEnvironment")` lists them.
const MODELLED_MEMBERS: [&str; 6] = [
    "backendEnvironmentArn",
    "createTime",
    "deploymentArtifacts",
    "environmentName",
    "stackName",
    "updateTime",
];

/// The `@required` members of `com.amazonaws.amplify#BackendEnvironment`: the ones a
/// reply must carry whatever the request said
/// (`smithy_query(query="shape",shape="BackendEnvironment")`).
const REQUIRED_MEMBERS: [&str; 4] = [
    "backendEnvironmentArn",
    "createTime",
    "environmentName",
    "updateTime",
];

/// `CreateBackendEnvironment` over the wire.
async fn create_environment(emulator: &Emulator, app_id: &str, request: &Value) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/apps/{app_id}/backendenvironments"),
            Some(request),
        )
        .await
}

/// `GetBackendEnvironment` over the wire.
async fn get_environment(emulator: &Emulator, app_id: &str, environment_name: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/apps/{app_id}/backendenvironments/{environment_name}"),
            None,
        )
        .await
}

/// A member that may legitimately be absent, as the value a client sees: JSON `null`
/// for a member the body does not carry.
fn optional_member(record: &Value, member: &str) -> Value {
    record.get(member).cloned().unwrap_or(Value::Null)
}

/// Assert that `record` carries every `@required` member of `BackendEnvironment` and
/// nothing the shape does not model.
fn assert_modelled_record(label: &str, operation: &str, record: &Value) {
    let members = record.as_object().unwrap_or_else(|| {
        panic!("case {label:?}: {operation} must answer a backendEnvironment object, got {record:?}")
    });
    for member in REQUIRED_MEMBERS {
        assert!(
            members.contains_key(member),
            "case {label:?}: {operation} must carry the required member {member:?}, got {record:?}"
        );
    }
    for member in members.keys() {
        assert!(
            MODELLED_MEMBERS.contains(&member.as_str()),
            "case {label:?}: {operation} carries {member:?}, which BackendEnvironment does not \
             model, got {record:?}"
        );
    }
}

/// One `CreateBackendEnvironment` request plus the record the rule says it must
/// produce.
///
/// `stack_name` and `deployment_artifacts` are the WIRE values expected for those
/// members, so a request that omits one expects `Value::Null`: both are OPTIONAL on
/// `BackendEnvironment`, so an omitted member is reported as absent rather than as an
/// invented value.
struct EnvironmentCase {
    case: &'static str,
    request: Value,
    environment_name: String,
    stack_name: Value,
    deployment_artifacts: Value,
}

/// The `CreateBackendEnvironment` table.
///
/// It varies `environmentName` over one character (its modelled `length` minimum), an
/// ordinary name, a name with the punctuation an environment name admits and the
/// modelled maximum of 255, and `stackName` / `deploymentArtifacts` over absent,
/// supplied singly, supplied together and at their modelled `length` maxima of 255 and
/// 1000. The bounds come from
/// `smithy_query(query="operation",operation="CreateBackendEnvironment")`.
fn environment_cases() -> Vec<EnvironmentCase> {
    vec![
        EnvironmentCase {
            case: "only the required environmentName",
            request: json!({ "environmentName": "staging" }),
            environment_name: "staging".to_string(),
            stack_name: Value::Null,
            deployment_artifacts: Value::Null,
        },
        EnvironmentCase {
            case: "environmentName of one character, its modelled length minimum",
            request: json!({ "environmentName": "x" }),
            environment_name: "x".to_string(),
            stack_name: Value::Null,
            deployment_artifacts: Value::Null,
        },
        EnvironmentCase {
            case: "stackName and deploymentArtifacts both supplied",
            request: json!({
                "environmentName": "dev",
                "stackName": "amplify-demo-dev-123456",
                "deploymentArtifacts": "amplify-demo-dev-123456-deployment",
            }),
            environment_name: "dev".to_string(),
            stack_name: json!("amplify-demo-dev-123456"),
            deployment_artifacts: json!("amplify-demo-dev-123456-deployment"),
        },
        EnvironmentCase {
            case: "only stackName supplied",
            request: json!({
                "environmentName": "prod",
                "stackName": "amplify-demo-prod-654321",
            }),
            environment_name: "prod".to_string(),
            stack_name: json!("amplify-demo-prod-654321"),
            deployment_artifacts: Value::Null,
        },
        EnvironmentCase {
            case: "only deploymentArtifacts supplied",
            request: json!({
                "environmentName": "integration",
                "deploymentArtifacts": "amplify-demo-integration-deployment",
            }),
            environment_name: "integration".to_string(),
            stack_name: Value::Null,
            deployment_artifacts: json!("amplify-demo-integration-deployment"),
        },
        EnvironmentCase {
            case: "stackName and deploymentArtifacts at their modelled length maxima",
            request: json!({
                "environmentName": "maxima",
                "stackName": "s".repeat(255),
                "deploymentArtifacts": "a".repeat(1000),
            }),
            environment_name: "maxima".to_string(),
            stack_name: json!("s".repeat(255)),
            deployment_artifacts: json!("a".repeat(1000)),
        },
        EnvironmentCase {
            case: "environmentName at the modelled length maximum of 255",
            request: json!({ "environmentName": "e".repeat(255) }),
            environment_name: "e".repeat(255),
            stack_name: Value::Null,
            deployment_artifacts: Value::Null,
        },
        EnvironmentCase {
            case: "an environmentName carrying case, dots, dashes and underscores",
            request: json!({
                "environmentName": "Feature.Test-1_env",
                "stackName": "amplify-demo-Feature.Test-1_env",
            }),
            environment_name: "Feature.Test-1_env".to_string(),
            stack_name: json!("amplify-demo-Feature.Test-1_env"),
            deployment_artifacts: Value::Null,
        },
    ]
}

/// Assert everything the rule says about one created backend environment.
fn assert_created(label: &str, app_id: &str, case: &EnvironmentCase, created: &Reply) {
    assert_eq!(
        created.status, 200,
        "case {label:?}: expected HTTP 200 from CreateBackendEnvironment, got {created:?}"
    );
    let record = created.member("backendEnvironment");
    assert_modelled_record(label, "CreateBackendEnvironment", &record);

    assert_eq!(
        string_member(&record, "environmentName"),
        case.environment_name,
        "case {label:?}: environmentName must be echoed as the request set it, got {record:?}"
    );
    assert_eq!(
        string_member(&record, "backendEnvironmentArn"),
        format!(
            "arn:aws:amplify:{REGION}:{ACCOUNT_ID}:apps/{app_id}/backendenvironments/{}",
            case.environment_name
        ),
        "case {label:?}: the ARN names the app the URI addressed and the environment, got \
         {record:?}"
    );
    assert_eq!(
        optional_member(&record, "stackName"),
        case.stack_name,
        "case {label:?}: stackName must be what the request carried, got {record:?}"
    );
    assert_eq!(
        optional_member(&record, "deploymentArtifacts"),
        case.deployment_artifacts,
        "case {label:?}: deploymentArtifacts must be what the request carried, got {record:?}"
    );

    let create_time = timestamp_member(&record, "createTime");
    assert!(
        create_time > 0.0,
        "case {label:?}: createTime must be a real instant, got {create_time}"
    );
    assert_eq!(
        timestamp_member(&record, "updateTime"),
        create_time,
        "case {label:?}: an environment nothing has updated has updateTime equal to createTime, \
         got {record:?}"
    );
}

#[tokio::test]
async fn a_created_backend_environment_is_readable_through_its_app() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("backend-env-host").await;
    // A second app exists throughout: an environment created on one app must never
    // change what the other app's environment reads as.
    let other_app_id = emulator.app_id_of_new_app("backend-env-bystander").await;

    let bystander = create_environment(
        &emulator,
        &other_app_id,
        &json!({ "environmentName": "staging", "stackName": "bystander-stack" }),
    )
    .await;
    assert_eq!(
        bystander.status, 200,
        "expected HTTP 200 for the bystander CreateBackendEnvironment, got {bystander:?}"
    );
    let bystander_record = bystander.member("backendEnvironment");

    let mut arns = BTreeSet::new();
    for case in environment_cases() {
        let label = case.case;
        let created = create_environment(&emulator, &app_id, &case.request).await;
        assert_created(label, &app_id, &case, &created);
        let record = created.member("backendEnvironment");

        assert!(
            arns.insert(string_member(&record, "backendEnvironmentArn")),
            "case {label:?}: each environment of an app has its own ARN, got {record:?} twice"
        );

        // The created record is PERSISTED: the read answers it field-for-field.
        let fetched = get_environment(&emulator, &app_id, &case.environment_name).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: expected HTTP 200 from GetBackendEnvironment, got {fetched:?}"
        );
        let fetched_record = fetched.member("backendEnvironment");
        assert_modelled_record(label, "GetBackendEnvironment", &fetched_record);
        assert_eq!(
            fetched_record, record,
            "case {label:?}: GetBackendEnvironment must answer the created record \
             field-for-field, got {fetched_record:?}"
        );

        // A read changes nothing a later caller sees.
        assert_eq!(
            get_environment(&emulator, &app_id, &case.environment_name)
                .await
                .member("backendEnvironment"),
            fetched_record,
            "case {label:?}: reading an environment must not change what a later caller sees"
        );
        assert_eq!(
            get_environment(&emulator, &other_app_id, "staging")
                .await
                .member("backendEnvironment"),
            bystander_record,
            "case {label:?}: creating an environment must not touch another app's environment"
        );
    }

    // An environment is reachable only through its OWN app: the bystander app holds
    // nothing under the names created above, even where it holds a name of its own.
    for case in environment_cases() {
        if case.environment_name == "staging" {
            continue;
        }
        let elsewhere = get_environment(&emulator, &other_app_id, &case.environment_name).await;
        assert_eq!(
            elsewhere.status, 404,
            "case {:?}: an environment of another app is not readable here, got {elsewhere:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn a_second_create_under_the_same_name_is_refused_and_keeps_the_stored_record() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("backend-env-duplicate-host").await;

    // The name identifies the environment within its app, so the second create is
    // refused whatever its other members say — including a request identical to the
    // first one.
    for (label, second) in [
        (
            "the identical request again",
            json!({ "environmentName": "staging", "stackName": "first-stack" }),
        ),
        (
            "the same name with a different stackName",
            json!({ "environmentName": "staging", "stackName": "second-stack" }),
        ),
        (
            "the same name with no optional members at all",
            json!({ "environmentName": "staging" }),
        ),
    ] {
        let first = create_environment(
            &emulator,
            &app_id,
            &json!({ "environmentName": "staging", "stackName": "first-stack" }),
        )
        .await;
        let stored = if first.status == 200 {
            first.member("backendEnvironment")
        } else {
            get_environment(&emulator, &app_id, "staging")
                .await
                .member("backendEnvironment")
        };

        let refused = create_environment(&emulator, &app_id, &second).await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: expected HTTP 400 from the duplicate create, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("BadRequestException"),
            "case {label:?}: expected BadRequestException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            Some(format!(
                "Failed to create backend environment. The backend environment staging already \
                 exists for the app {app_id}"
            )),
            "case {label:?}: the refusal names the resource, the name and the app, got {:?}",
            refused.message()
        );

        // The refusal stored nothing: the first record is still what a reader sees.
        assert_eq!(
            get_environment(&emulator, &app_id, "staging")
                .await
                .member("backendEnvironment"),
            stored,
            "case {label:?}: a refused duplicate must leave the stored environment alone"
        );
    }
}

#[tokio::test]
async fn an_unknown_app_or_environment_is_the_modelled_404() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("backend-env-refusal-host").await;

    // `AppId` is modelled `length 1..20, pattern ^d[a-z0-9]+$`, so each of these is a
    // well-formed id for an app that does not exist — the request passes the boundary
    // validator and the refusal is the handler's.
    for absent_app in ["dnosuchapp1", "d0", &format!("d{}", "9".repeat(18))] {
        for reply in [
            create_environment(&emulator, absent_app, &json!({ "environmentName": "staging" }))
                .await,
            get_environment(&emulator, absent_app, "staging").await,
        ] {
            assert_eq!(
                reply.status, 404,
                "case {absent_app:?}: expected HTTP 404 for an app that does not exist, got \
                 {reply:?}"
            );
            assert_eq!(
                reply.error_code.as_deref(),
                Some("NotFoundException"),
                "case {absent_app:?}: expected NotFoundException, got {:?}",
                reply.error_code
            );
            assert_eq!(
                reply.message(),
                Some(format!("App {absent_app} not found.")),
                "case {absent_app:?}: the 404 names the entity the request addressed, got {:?}",
                reply.message()
            );
        }
    }

    // A name no environment of this app has is the same modelled refusal, naming the
    // environment rather than the app.
    for absent in ["staging", "e", &"z".repeat(255)] {
        let refused = get_environment(&emulator, &app_id, absent).await;
        assert_eq!(
            refused.status, 404,
            "case {absent:?}: expected HTTP 404 from GetBackendEnvironment, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("NotFoundException"),
            "case {absent:?}: expected NotFoundException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            Some(format!("Backend environment {absent} not found.")),
            "case {absent:?}: the 404 names the entity the request addressed, got {:?}",
            refused.message()
        );
    }

    // The refusals stored nothing: the real app still accepts the first create of
    // each name they named.
    for name in ["staging", "e", &"z".repeat(255)] {
        let created =
            create_environment(&emulator, &app_id, &json!({ "environmentName": name })).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected HTTP 200 from CreateBackendEnvironment after the refusals, \
             got {created:?}"
        );
    }
}

#[tokio::test]
async fn environments_are_removed_with_the_app_that_owns_them() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("backend-env-deleted-host").await;
    let survivor_app_id = emulator.app_id_of_new_app("backend-env-surviving-host").await;

    let names = ["staging", "dev", "prod"];
    for name in names {
        let created =
            create_environment(&emulator, &app_id, &json!({ "environmentName": name })).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected HTTP 200 from CreateBackendEnvironment, got {created:?}"
        );
    }
    let survivor = create_environment(
        &emulator,
        &survivor_app_id,
        &json!({ "environmentName": "staging", "stackName": "survivor-stack" }),
    )
    .await;
    assert_eq!(
        survivor.status, 200,
        "expected HTTP 200 for the surviving app's CreateBackendEnvironment, got {survivor:?}"
    );
    let survivor_record = survivor.member("backendEnvironment");

    let deleted = emulator.delete_app(&app_id).await;
    assert_eq!(
        deleted.status, 200,
        "expected HTTP 200 from DeleteApp, got {deleted:?}"
    );

    for name in names {
        let gone = get_environment(&emulator, &app_id, name).await;
        assert_eq!(
            gone.status, 404,
            "case {name:?}: an environment of a deleted app is gone, got {gone:?}"
        );
    }
    assert_eq!(
        get_environment(&emulator, &survivor_app_id, "staging")
            .await
            .member("backendEnvironment"),
        survivor_record,
        "deleting one app must leave another app's environment exactly as it was"
    );
}
