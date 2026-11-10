// SPDX-License-Identifier: CC0-1.0

//! AppConfig's own deployment-event notification extensions DELIVER: associating one binds
//! a destination, and the deployment's lifecycle is reported there.
//!
//! The rule under test: an AWS-authored extension publishes no `Uri` of its own, so each of
//! its actions delivers to the destination the ASSOCIATION that bound the extension names —
//! the ARN supplied for the extension's published parameter — and the notification
//! extensions bind the deployment-lifecycle action points, so every stage a deployment
//! reaches is reported to that destination with the documented event: the action point in
//! PascalCase (`Type`), the deployment number, the configuration version, the application,
//! environment and configuration profile the deployment runs on, and the association's own
//! `Parameters`. The binding is the association, so deleting it stops delivery and
//! re-creating it restores it
//! (`smithy_query(query="operation",operation="CreateExtensionAssociation,StartDeployment")`:
//! an association carries `Parameters` as a `ParameterValueMap`, and a deployment reports
//! the extensions applied to it).
//!
//! Delivery is asserted where a client sees it — on the Amazon SQS queue the association
//! named — so the suite mounts SQS beside AppConfig on one engine. The beats below span
//! what the rule generalises over: whether the association exists at the moment the
//! deployment runs (bound, deleted, re-created), the configuration version each deployment
//! rolls out, and the pacing of the strategy (a deployment that completes at once and one
//! that completes a stage later). Where each notification extension's destination comes
//! from — a queue, a topic, or the account's default event bus — is pinned over the whole
//! published catalogue by `crate::state::extension_actions`' own unit table.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call,
};
use crate::extension_action_points::{
    associate, create_version, drain, emulator, get_deployment, given_queue, given_strategy,
    sqs, start_deployment,
};
use crate::list_extensions::{items, list};
use crate::resource_tags::arn;

/// How long a beat waits for the deployment it started to complete.
const SETTLE_BY: Duration = Duration::from_secs(60);

/// How often a beat re-reads the deployment while waiting, which is also what drives the
/// action points the clock has made due.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The `Type` values the two stages of a deployment nobody stopped report, in the order the
/// stages happen.
const DELIVERED_TYPES: &[&str] = &["OnDeploymentStart", "OnDeploymentComplete"];

/// The extension the AWS AppConfig User Guide tells a caller to associate to have
/// deployment events sent to an Amazon SQS queue, and the parameter that association
/// supplies the queue's ARN for. Discovered from `ListExtensions` below rather than passed
/// from here, exactly as a client that does not know the catalogue would.
const QUEUE_PARAMETER: &str = "queueArn";

/// The ARN field an extension AppConfig itself authors leaves empty, because no account
/// owns it: the marker a client picks the authored entries out of a listing by.
const AUTHORED_ARN_MARKER: &str = "::extension/";

/// The authored extension in the account's catalogue whose published name says it sends
/// deployment events to Amazon SQS, as `(identifier, name)` — found the way a client finds
/// it: by walking `ListExtensions` and keeping the entries no account owns.
async fn discover_sqs_extension(router: &MultiServiceRouter) -> (String, String) {
    let listed = items("the account's catalogue", &list(router, "max_results=50").await);
    let authored: Vec<&Value> = listed
        .iter()
        .filter(|item| {
            item.get("Arn")
                .and_then(Value::as_str)
                .is_some_and(|value| value.contains(AUTHORED_ARN_MARKER))
        })
        .collect();
    let matching: Vec<&Value> = authored
        .iter()
        .copied()
        .filter(|item| {
            item.get("Name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.contains("SQS"))
        })
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case \"discovery\": expected exactly one AWS-authored extension whose name names \
         Amazon SQS, got {matching:?} out of {authored:?}"
    );
    let found = matching[0];
    let arn = found
        .get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case \"discovery\": the entry reports no Arn, got {found}"))
        .to_string();
    let name = found
        .get("Name")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case \"discovery\": the entry reports no Name, got {found}"))
        .to_string();
    (arn, name)
}

/// `DELETE /extensionassociations/{ExtensionAssociationId}`.
async fn delete_association(router: &MultiServiceRouter, case: &str, association: &str) {
    let deleted = json_call(
        router,
        "DELETE",
        &format!("/extensionassociations/{association}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        deleted.status,
        204,
        "case {case:?}: DeleteExtensionAssociation expected 204, got {} / {}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );
    let read = json_call(
        router,
        "GET",
        &format!("/extensionassociations/{association}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        read.status,
        404,
        "case {case:?}: a deleted association is gone, got {} / {}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
}

/// Start a deployment of `version` and poll it to `COMPLETE`, answering the deployment
/// number it was issued.
async fn deploy_and_wait_complete(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    environment: &str,
    profile: &str,
    strategy: &str,
    version: &str,
) -> i32 {
    let started = start_deployment(
        router,
        application,
        environment,
        &json!({
            "DeploymentStrategyId": strategy,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": version,
        }),
    )
    .await;
    assert_eq!(
        started.status,
        201,
        "case {case:?}: StartDeployment expected 201, got {} / {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    let number = started
        .json()
        .get("DeploymentNumber")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: StartDeployment answered no DeploymentNumber, got {}",
                started.json()
            )
        }) as i32;

    let deadline = Instant::now() + SETTLE_BY;
    loop {
        let read = get_deployment(router, application, environment, number).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetDeployment expected 200, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let state = read
            .json()
            .get("State")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if state == "COMPLETE" {
            return number;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: deployment {number} of version {version} did not reach COMPLETE \
             within {}s; last state {state:?}",
            SETTLE_BY.as_secs()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// What one beat expects the delivered notifications to be about: the deployment, the
/// version it rolled out, the resources it ran on and the queue the association named.
struct Expected<'a> {
    case: &'a str,
    number: i32,
    version: &'a str,
    application: &'a str,
    environment: &'a str,
    profile: &'a str,
    queue_arn: &'a str,
}

/// Assert `events` are the two lifecycle notifications `expected` describes, each naming
/// the resources the deployment ran on and echoing the association's parameters.
fn assert_lifecycle_notifications(expected: &Expected<'_>, events: &[Value]) {
    let Expected {
        case,
        number,
        version,
        application,
        environment,
        profile,
        queue_arn,
    } = *expected;
    let types: Vec<String> = events
        .iter()
        .map(|event| {
            event
                .get("Type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        types,
        DELIVERED_TYPES
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<String>>(),
        "case {case:?}: expected the start and the completion, in the order the stages \
         happened, got {types:?}"
    );
    // Asserted as a SET as well, so a reordering of the stages cannot pass by accident.
    let distinct: BTreeSet<&String> = types.iter().collect();
    assert_eq!(
        distinct.len(),
        DELIVERED_TYPES.len(),
        "case {case:?}: expected one notification per stage, got {types:?}"
    );

    for event in events {
        let stage = event.get("Type").and_then(Value::as_str).unwrap_or_default();
        assert_eq!(
            event.get("DeploymentNumber").and_then(Value::as_i64),
            Some(i64::from(number)),
            "case {case:?} / {stage}: expected the deployment number, got {event}"
        );
        assert_eq!(
            event.get("ConfigurationVersion").and_then(Value::as_str),
            Some(version),
            "case {case:?} / {stage}: expected the configuration version, got {event}"
        );
        for (member, id) in [
            ("Application", application),
            ("Environment", environment),
            ("ConfigurationProfile", profile),
        ] {
            assert_eq!(
                event
                    .get(member)
                    .and_then(|resource| resource.get("Id"))
                    .and_then(Value::as_str),
                Some(id),
                "case {case:?} / {stage}: expected {member}.Id {id:?}, got {event}"
            );
        }
        // The association's own parameter values travel with the event, which is how the
        // target knows which association it was invoked for.
        assert_eq!(
            event
                .get("Parameters")
                .and_then(|parameters| parameters.get(QUEUE_PARAMETER))
                .and_then(Value::as_str),
            Some(queue_arn),
            "case {case:?} / {stage}: expected the association's {QUEUE_PARAMETER}, got {event}"
        );
    }
}

/// `GetQueueAttributes`' `ApproximateNumberOfMessages`, as the string SQS reports it.
async fn queued_count(router: &MultiServiceRouter, url: &str) -> String {
    let attributes = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["ApproximateNumberOfMessages"] }),
    )
    .await;
    attributes["Attributes"]["ApproximateNumberOfMessages"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn the_sqs_notification_extension_delivers_a_deployments_lifecycle_to_its_queue() {
    let router = emulator();
    let (queue_url, queue_arn) = given_queue(&router, "appconfig-deployment-events").await;
    let (extension, extension_name) = discover_sqs_extension(&router).await;

    let application = create_application(&router, "notified-app").await;
    let environment = create_environment(&router, &application, "notified-env").await;
    let profile = create_profile(&router, &application, "notified-profile").await;
    for version in 1..=3 {
        let written = create_version(
            &router,
            &application,
            &profile,
            "application/json",
            format!(r#"{{"version":{version}}}"#).as_bytes(),
        )
        .await;
        assert_eq!(
            written.status,
            201,
            "seeding version {version} answered {} / {}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
    }
    // A strategy with a rollout window, so the deployment is in flight when the start
    // answers and its completion is reached by the clock rather than by the call.
    let paced = given_strategy(&router, "notified-paced", 5, 0).await;
    // And one with nothing to wait for, so the same delivery is asserted for a deployment
    // that completes at the instant it starts.
    let instant = given_strategy(&router, "notified-instant", 0, 0).await;
    let environment_arn = arn(&format!("application/{application}/environment/{environment}"));

    // Beat 1: the association is bound, so both stages of the deployment are delivered.
    let association = associate(
        &router,
        "bound to the environment",
        &extension,
        &environment_arn,
        &json!({ QUEUE_PARAMETER: queue_arn }),
    )
    .await;
    let number = deploy_and_wait_complete(
        &router,
        "bound",
        &application,
        &environment,
        &profile,
        &paced,
        "1",
    )
    .await;
    let events = drain(&router, &queue_url).await;
    assert_lifecycle_notifications(
        &Expected {
            case: "bound",
            number,
            version: "1",
            application: &application,
            environment: &environment,
            profile: &profile,
            queue_arn: &queue_arn,
        },
        &events,
    );

    // Beat 2: the association is deleted, so the next deployment delivers NOTHING — and it
    // still completes, the notification being an observer of the deployment, not a part of
    // it.
    delete_association(&router, "deleted", &association).await;
    let silent = deploy_and_wait_complete(
        &router,
        "deleted",
        &application,
        &environment,
        &profile,
        &instant,
        "2",
    )
    .await;
    assert!(
        silent > number,
        "case \"deleted\": expected a later deployment than {number}, got {silent}"
    );
    let after_delete = drain(&router, &queue_url).await;
    assert!(
        after_delete.is_empty(),
        "case \"deleted\": an association that no longer exists delivers nothing, got \
         {after_delete:?}"
    );
    let queued = queued_count(&router, &queue_url).await;
    assert_eq!(
        queued, "0",
        "case \"deleted\": expected an empty queue, got {queued:?} messages"
    );

    // Beat 3: re-created by the extension's published NAME rather than by its ARN — either
    // addresses the same extension — and delivery comes back for the next deployment.
    let restored = associate(
        &router,
        "re-created",
        &extension_name,
        &environment_arn,
        &json!({ QUEUE_PARAMETER: queue_arn }),
    )
    .await;
    assert_ne!(
        restored, association,
        "case \"re-created\": a new association is a new resource, got {restored}"
    );
    let third = deploy_and_wait_complete(
        &router,
        "re-created",
        &application,
        &environment,
        &profile,
        &paced,
        "3",
    )
    .await;
    let restored_events = drain(&router, &queue_url).await;
    assert_lifecycle_notifications(
        &Expected {
            case: "re-created",
            number: third,
            version: "3",
            application: &application,
            environment: &environment,
            profile: &profile,
            queue_arn: &queue_arn,
        },
        &restored_events,
    );
}
