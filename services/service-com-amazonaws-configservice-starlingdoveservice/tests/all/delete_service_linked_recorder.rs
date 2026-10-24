// SPDX-License-Identifier: CC0-1.0

//! `DeleteServiceLinkedConfigurationRecorder`, end to end over the wire.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the emulator binary mounts — and asserts what
//! the service PERSISTED, by reading the recorders back through the operations that
//! report them.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded case:
//!
//! 1. a request names ONE service-linked configuration recorder, by its `Arn` or by
//!    the `ServicePrincipal` it is linked to, and the delete drops that recorder and
//!    its recording status from state, answering the ARN and name it was stored with,
//!    while every other recorder the account holds is left alone;
//! 2. the service can be linked again afterwards, getting a recorder of its own with a
//!    new identity, so the delete really released the name;
//! 3. a selector that names no service-linked recorder — a service nothing is linked
//!    to, an ARN no recorder carries, the ARN of the customer managed recorder, or a
//!    recorder this operation already deleted — is the modelled
//!    `NoSuchConfigurationRecorderException` and removes nothing;
//! 4. a request carrying both selectors, or neither, is the modelled
//!    `ValidationException` ("You must specify exactly one of Arn or
//!    ServicePrincipal") and removes nothing;
//! 5. a `ServicePrincipal` that names no Amazon Web Services service is the modelled
//!    `ValidationException` rather than a missing resource;
//! 6. deleting a third-party service-linked recorder leaves the connector it recorded
//!    through in place, and that connector can carry the service's recorder again.

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    SERVICE_LINKED_PRINCIPALS, RECORDING_SCOPE_INTERNAL,
};

use crate::config_setup_and_rule_loop::{recorder_request, Emulator, ROLE_ARN};

/// The operation under test.
const DELETE: &str = "DeleteServiceLinkedConfigurationRecorder";
/// The operation that creates the recorder under test.
const PUT: &str = "PutServiceLinkedConfigurationRecorder";
/// The operation that creates a THIRD-PARTY service-linked recorder.
const PUT_THIRD_PARTY: &str = "PutThirdPartyServiceLinkedConfigurationRecorder";

/// A service `PutServiceLinkedConfigurationRecorder` creates a recorder for
/// (`state::SERVICE_LINKED_PRINCIPALS`): only a service Config links a configuration
/// recorder to has one to delete.
const LINKED: &str = SERVICE_LINKED_PRINCIPALS[0];

/// Another such service, whose recorder every case leaves alone, so each delete is
/// shown to remove the ONE recorder it names and no other.
const BYSTANDER: &str = SERVICE_LINKED_PRINCIPALS[1];

/// The supported service principal that is NOT `service_principal`, so a case can hold
/// a second recorder the delete must leave alone whichever row it is running.
fn bystander_of(service_principal: &str) -> &'static str {
    if service_principal == BYSTANDER {
        LINKED
    } else {
        BYSTANDER
    }
}

/// Which of the two selectors a request names the recorder with. A request carries
/// exactly one of them (`DeleteServiceLinkedConfigurationRecorderRequest$Arn`: "You
/// must specify exactly one of Arn or ServicePrincipal").
#[derive(Debug, Clone, Copy)]
enum By {
    Arn,
    ServicePrincipal,
}

/// The `DeleteServiceLinkedConfigurationRecorder` body naming the recorder with `arn`
/// and linked to `service_principal` through `selector`.
fn delete_request(selector: By, arn: &str, service_principal: &str) -> Value {
    match selector {
        By::Arn => json!({ "Arn": arn }),
        By::ServicePrincipal => json!({ "ServicePrincipal": service_principal }),
    }
}

/// Create the service-linked configuration recorder of `service_principal` over the
/// wire, returning `(arn, name)` as Config assigned them.
async fn put_service_linked(
    emulator: &Emulator,
    service_principal: &str,
    tags: &[(&str, &str)],
) -> (String, String) {
    let mut request = json!({ "ServicePrincipal": service_principal });
    if !tags.is_empty() {
        request["Tags"] = tags
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect();
    }
    let created = emulator.ok(PUT, &request).await;
    let arn = created["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("{PUT} must answer an Arn, got {created}"))
        .to_string();
    let name = created["Name"]
        .as_str()
        .unwrap_or_else(|| panic!("{PUT} must answer a Name, got {created}"))
        .to_string();
    (arn, name)
}

/// Every configuration recorder this account holds, read back over the wire.
async fn described_recorders(emulator: &Emulator) -> Vec<Value> {
    let described = emulator
        .ok("DescribeConfigurationRecorders", &json!({}))
        .await;
    described["ConfigurationRecorders"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("DescribeConfigurationRecorders must answer a list, got {described}")
        })
}

/// The recorders `DescribeConfigurationRecorders` reports for `service_principal`.
async fn described_for(emulator: &Emulator, service_principal: &str) -> Vec<Value> {
    let described = emulator
        .ok(
            "DescribeConfigurationRecorders",
            &json!({ "ServicePrincipal": service_principal }),
        )
        .await;
    described["ConfigurationRecorders"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("DescribeConfigurationRecorders must answer a list, got {described}")
        })
}

/// The statuses `DescribeConfigurationRecorderStatus` reports for `service_principal`.
async fn status_for(emulator: &Emulator, service_principal: &str) -> Vec<Value> {
    let described = emulator
        .ok(
            "DescribeConfigurationRecorderStatus",
            &json!({ "ServicePrincipal": service_principal }),
        )
        .await;
    described["ConfigurationRecordersStatus"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("DescribeConfigurationRecorderStatus must answer a list, got {described}")
        })
}

/// Every recorder name `ListConfigurationRecorders` summarises.
async fn listed_names(emulator: &Emulator) -> Vec<String> {
    let listed = emulator
        .ok("ListConfigurationRecorders", &json!({}))
        .await;
    listed["ConfigurationRecorderSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("ListConfigurationRecorders must answer a list, got {listed}"))
        .iter()
        .map(|summary| {
            summary["name"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary must carry a name, got {summary}"))
                .to_string()
        })
        .collect()
}

/// The `message` an error body carries, under either spelling the wire may use.
fn error_message(body: &Value) -> String {
    for key in ["message", "Message"] {
        if let Some(message) = body.get(key).and_then(Value::as_str) {
            return message.to_string();
        }
    }
    format!("<no message in {body}>")
}

/// Call `operation` and require the modelled refusal `expected_code` with HTTP 400,
/// returning the message it carried.
async fn refused_with(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    body: &Value,
    expected_code: &str,
) -> String {
    let (status, parsed) = emulator.call(operation, body).await;
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
        .unwrap_or_else(|| format!("<no __type in {parsed}>"));
    assert_eq!(
        (status, code.as_str()),
        (400, expected_code),
        "case {case:?}: expected 400 {expected_code} from {operation}, got {status} {code} \
         with body {parsed}"
    );
    error_message(&parsed)
}

/// The ARN of the customer managed configuration recorder this account holds, created
/// over the wire like every other step.
async fn put_customer_managed(emulator: &Emulator) -> String {
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({ "roleARN": ROLE_ARN })),
        )
        .await;
    let own: Vec<Value> = described_recorders(emulator)
        .await
        .into_iter()
        .filter(|recorder| recorder.get("servicePrincipal").is_none())
        .collect();
    assert_eq!(
        own.len(),
        1,
        "exactly one recorder is customer managed, got {own:?}"
    );
    own[0]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("the customer managed recorder carries an arn, got {:?}", own[0]))
        .to_string()
}

#[tokio::test]
async fn a_delete_removes_the_one_service_linked_recorder_it_names() {
    // Rows differ along which selector names the recorder and along the service it is
    // linked to — every service Config links a recorder to, each named both ways —
    // because the rule is about naming one recorder, not about one service.
    let mut cases: Vec<(String, &str, By)> = Vec::new();
    for service_principal in SERVICE_LINKED_PRINCIPALS {
        for (label, selector) in [("by ARN", By::Arn), ("by service principal", By::ServicePrincipal)]
        {
            cases.push((format!("{service_principal}, {label}"), service_principal, selector));
        }
    }

    for (case, service_principal, selector) in cases {
        let case = case.as_str();
        let bystander = bystander_of(service_principal);
        let emulator = Emulator::new();
        let (arn, name) = put_service_linked(&emulator, service_principal, &[("owner", "team")])
            .await;
        let (bystander_arn, bystander_name) = put_service_linked(&emulator, bystander, &[]).await;
        let customer_managed = put_customer_managed(&emulator).await;
        let bystander_before = described_for(&emulator, bystander).await;

        // The recorder is in state before the delete, recording for its service.
        assert_eq!(
            described_for(&emulator, service_principal).await.len(),
            1,
            "case {case:?}: the recorder must exist before it is deleted"
        );

        let deleted = emulator
            .ok(DELETE, &delete_request(selector, &arn, service_principal))
            .await;
        assert_eq!(
            deleted,
            json!({ "Arn": arn, "Name": name }),
            "case {case:?}: the delete must answer the identity the recorder was stored \
             with, got {deleted}"
        );

        // It is gone from every read path that reports recorders.
        assert_eq!(
            described_for(&emulator, service_principal).await,
            Vec::<Value>::new(),
            "case {case:?}: no recorder is linked to {service_principal:?} any more"
        );
        assert_eq!(
            status_for(&emulator, service_principal).await,
            Vec::<Value>::new(),
            "case {case:?}: the deleted recorder has no recording status to report"
        );
        let names = listed_names(&emulator).await;
        assert!(
            !names.contains(&name),
            "case {case:?}: {name:?} must not be summarised any more, got {names:?}"
        );
        refused_with(
            &emulator,
            case,
            "DescribeConfigurationRecorderStatus",
            &json!({ "Arn": arn }),
            "NoSuchConfigurationRecorderException",
        )
        .await;
        // "When a resource is deleted, the tags associated with that resource are
        // deleted as well" (`TagResource`).
        refused_with(
            &emulator,
            case,
            "ListTagsForResource",
            &json!({ "ResourceArn": arn }),
            "ResourceNotFoundException",
        )
        .await;

        // Every other recorder the account holds is untouched: the one linked to
        // another service, ARN and all, and the customer managed one.
        assert_eq!(
            described_for(&emulator, bystander).await,
            bystander_before,
            "case {case:?}: the recorder of another service must be left alone"
        );
        let names = listed_names(&emulator).await;
        assert!(
            names.contains(&bystander_name),
            "case {case:?}: {bystander_name:?} must still be summarised, got {names:?}"
        );
        let own: Vec<Value> = described_recorders(&emulator)
            .await
            .into_iter()
            .filter(|recorder| recorder["arn"] == json!(customer_managed))
            .collect();
        assert_eq!(
            own.len(),
            1,
            "case {case:?}: the customer managed recorder must be left alone, got {own:?}"
        );
        assert_ne!(
            bystander_arn, arn,
            "case {case:?}: the two service-linked recorders are distinct resources"
        );

        // A second delete of the same recorder finds nothing: the first one removed it
        // rather than hiding it.
        refused_with(
            &emulator,
            case,
            DELETE,
            &delete_request(selector, &arn, service_principal),
            "NoSuchConfigurationRecorderException",
        )
        .await;
    }
}

#[tokio::test]
async fn a_service_can_be_linked_again_after_its_recorder_is_deleted() {
    // Rows differ along the selector the delete used and the service, so the release of
    // the assigned name is shown for both ways of naming the recorder.
    let cases: [(&str, &str, By); 2] = [
        ("by ARN", LINKED, By::Arn),
        ("by service principal", BYSTANDER, By::ServicePrincipal),
    ];

    for (case, service_principal, selector) in cases {
        let emulator = Emulator::new();
        let (first_arn, first_name) = put_service_linked(&emulator, service_principal, &[]).await;
        emulator
            .ok(
                DELETE,
                &delete_request(selector, &first_arn, service_principal),
            )
            .await;

        // The create is no conflict any more, and Config assigns the same name to the
        // same service while the recorder itself is a new resource with its own ARN.
        let (second_arn, second_name) =
            put_service_linked(&emulator, service_principal, &[("owner", "team")]).await;
        assert_eq!(
            second_name, first_name,
            "case {case:?}: the name Config assigns {service_principal:?} does not change, \
             got {second_name:?}"
        );
        assert_ne!(
            second_arn, first_arn,
            "case {case:?}: the recreated recorder is a new resource, got {second_arn:?}"
        );

        let described = described_for(&emulator, service_principal).await;
        assert_eq!(
            described.len(),
            1,
            "case {case:?}: the service holds its recorder again, got {described:?}"
        );
        assert_eq!(
            described[0]["arn"], json!(second_arn),
            "case {case:?}: the recorder read back is the recreated one, got {described:?}"
        );
        assert_eq!(
            described[0]["recordingScope"], json!(RECORDING_SCOPE_INTERNAL),
            "case {case:?}: the recreated recorder is service-linked, got {described:?}"
        );
        let statuses = status_for(&emulator, service_principal).await;
        assert_eq!(
            statuses.len(),
            1,
            "case {case:?}: the recreated recorder reports a status again, got {statuses:?}"
        );
        // The delete left the old ARN naming nothing, so it is the new one that reads
        // back.
        refused_with(
            &emulator,
            case,
            DELETE,
            &json!({ "Arn": first_arn }),
            "NoSuchConfigurationRecorderException",
        )
        .await;
    }
}

#[tokio::test]
async fn a_selector_that_names_no_service_linked_recorder_is_refused() {
    let emulator = Emulator::new();
    let (arn, name) = put_service_linked(&emulator, LINKED, &[]).await;
    let customer_managed = put_customer_managed(&emulator).await;
    let before = described_recorders(&emulator).await;

    // Rows differ along HOW the selector misses: a service nothing is linked to, an ARN
    // of this Region's ARN form that no recorder carries, the ARN of the recorder this
    // account manages itself (which `DeleteConfigurationRecorder` owns), a recorder
    // name where an ARN belongs, and a value that is no ARN at all.
    let ghost_arn = emulator_server::emulator_arn(
        "config",
        &emulator.region,
        "configuration-recorder/AWSConfigurationRecorderForGhost/cr-ghost",
    );
    let cases: [(&str, Value); 6] = [
        (
            "a service nothing is linked to",
            json!({ "ServicePrincipal": BYSTANDER }),
        ),
        (
            "a service Config links no recorder to",
            json!({ "ServicePrincipal": "backup.amazonaws.com" }),
        ),
        ("an ARN no recorder carries", json!({ "Arn": ghost_arn })),
        (
            "the ARN of the customer managed recorder",
            json!({ "Arn": customer_managed }),
        ),
        ("the recorder's name where an ARN belongs", json!({ "Arn": name })),
        ("a value that is no ARN at all", json!({ "Arn": "not-an-arn" })),
    ];

    for (case, request) in cases {
        refused_with(
            &emulator,
            case,
            DELETE,
            &request,
            "NoSuchConfigurationRecorderException",
        )
        .await;

        // The refusal removed nothing: both recorders are still exactly as they were.
        assert_eq!(
            described_recorders(&emulator).await,
            before,
            "case {case:?}: a refused delete must leave the store alone"
        );
    }

    // And the recorder the account does hold is still deletable by either selector.
    let deleted = emulator.ok(DELETE, &json!({ "Arn": arn })).await;
    assert_eq!(
        deleted,
        json!({ "Arn": arn, "Name": name }),
        "the recorder in state is still deletable, got {deleted}"
    );
}

#[tokio::test]
async fn a_request_that_does_not_name_exactly_one_selector_is_refused() {
    let emulator = Emulator::new();
    let linked = LINKED;
    let (arn, name) = put_service_linked(&emulator, linked, &[]).await;
    let before = described_recorders(&emulator).await;

    // Rows differ along which members the request carries: both of them (naming the
    // same recorder, and naming two different ones) and neither.
    let cases: [(&str, Value); 3] = [
        (
            "both selectors naming the same recorder",
            json!({ "Arn": arn, "ServicePrincipal": linked }),
        ),
        (
            "both selectors naming different recorders",
            json!({ "Arn": arn, "ServicePrincipal": bystander_of(linked) }),
        ),
        ("neither selector", json!({})),
    ];

    for (case, request) in cases {
        let message = refused_with(&emulator, case, DELETE, &request, "ValidationException").await;
        assert!(
            message.contains("Arn") && message.contains("ServicePrincipal"),
            "case {case:?}: the refusal must name both members of the rule, got {message:?}"
        );
        assert_eq!(
            described_recorders(&emulator).await,
            before,
            "case {case:?}: a refused delete must leave the store alone"
        );
    }

    // Exactly one of them still deletes the recorder.
    let deleted = emulator
        .ok(DELETE, &json!({ "ServicePrincipal": linked }))
        .await;
    assert_eq!(
        deleted,
        json!({ "Arn": arn, "Name": name }),
        "one selector names the recorder, got {deleted}"
    );
}

#[tokio::test]
async fn a_service_principal_that_names_no_service_is_refused() {
    // Rows differ along the form of the value: the modelled pattern `^[\w+=,.@-]+$`
    // (smithy_query(query="operation",
    // operation="DeleteServiceLinkedConfigurationRecorder")) admits all of them, and
    // none names an Amazon Web Services service, so none names a recorder either.
    let cases: [(&str, &str); 6] = [
        ("no DNS suffix at all", "observabilityadmin"),
        ("another suffix", "backup.amazon.com"),
        ("the suffix on its own", "amazonaws.com"),
        ("an empty service label", ".amazonaws.com"),
        ("an empty inner label", "backup..amazonaws.com"),
        ("a hyphen-bounded label", "-backup.amazonaws.com"),
    ];

    let emulator = Emulator::new();
    let (_, name) = put_service_linked(&emulator, LINKED, &[]).await;
    let before = described_recorders(&emulator).await;

    for (case, service_principal) in cases {
        let message = refused_with(
            &emulator,
            case,
            DELETE,
            &json!({ "ServicePrincipal": service_principal }),
            "ValidationException",
        )
        .await;
        assert!(
            message.contains("ServicePrincipal") && message.contains(service_principal),
            "case {case:?}: the refusal must name the member and quote the value, \
             got {message:?}"
        );
        assert_eq!(
            described_recorders(&emulator).await,
            before,
            "case {case:?}: a refused delete must leave the store alone"
        );
    }

    let names = listed_names(&emulator).await;
    assert!(
        names.contains(&name),
        "the recorder in state survived every refusal, got {names:?}"
    );
}

#[tokio::test]
async fn deleting_a_third_party_recorder_leaves_its_connector() {
    // Rows differ along the selector that names the third-party recorder; the service
    // principal is the one Config records a third-party cloud service provider for
    // (`state::THIRD_PARTY_SERVICE_LINKED_PRINCIPALS`).
    for (case, selector) in [("by ARN", By::Arn), ("by service principal", By::ServicePrincipal)] {
        let emulator = Emulator::new();
        let service_principal = "securityhub.amazonaws.com";
        let connector = emulator
            .ok(
                "PutConnector",
                &json!({
                    "ConnectorConfiguration": { "azure": {
                        "tenantIdentifier": "tenant-delete",
                        "clientIdentifier": "client-delete",
                    } },
                }),
            )
            .await;
        let connector_arn = connector["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: PutConnector must answer an Arn"))
            .to_string();
        let scope = json!({ "scopeType": "tenant", "allRegions": true });

        let created = emulator
            .ok(
                PUT_THIRD_PARTY,
                &json!({
                    "ServicePrincipal": service_principal,
                    "ConnectorArn": connector_arn,
                    "ScopeConfiguration": scope,
                }),
            )
            .await;
        let arn = created["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: {PUT_THIRD_PARTY} must answer an Arn"))
            .to_string();
        let name = created["Name"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: {PUT_THIRD_PARTY} must answer a Name"))
            .to_string();

        let deleted = emulator
            .ok(DELETE, &delete_request(selector, &arn, service_principal))
            .await;
        assert_eq!(
            deleted,
            json!({ "Arn": arn, "Name": name }),
            "case {case:?}: the delete must answer the recorder's stored identity, \
             got {deleted}"
        );
        assert_eq!(
            described_for(&emulator, service_principal).await,
            Vec::<Value>::new(),
            "case {case:?}: the third-party recorder is gone"
        );

        // The connector is a resource of its own (`DeleteConnector`'s), so it outlives
        // the recorder that recorded through it.
        let listed = emulator.ok("ListConnectors", &json!({})).await;
        let arns: Vec<String> = listed["ConnectorSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| summary["arn"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            arns.contains(&connector_arn),
            "case {case:?}: the connector must survive the recorder, got {arns:?}"
        );

        // And it can carry the service's recorder again.
        let recreated = emulator
            .ok(
                PUT_THIRD_PARTY,
                &json!({
                    "ServicePrincipal": service_principal,
                    "ConnectorArn": connector_arn,
                    "ScopeConfiguration": { "scopeType": "subscription", "allRegions": true },
                }),
            )
            .await;
        assert_eq!(
            recreated["Name"], json!(name),
            "case {case:?}: the recreated recorder carries the name Config assigns the \
             service, got {recreated}"
        );
        assert_ne!(
            recreated["Arn"], json!(arn),
            "case {case:?}: the recreated recorder is a new resource, got {recreated}"
        );
    }
}
