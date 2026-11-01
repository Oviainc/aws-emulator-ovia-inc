// SPDX-License-Identifier: CC0-1.0

//! What a registration does about the Amazon EC2 instances it names: it STORES them, and
//! resolves nothing against the service that owns them.
//!
//! RULE under test: `RegisterApplication` is asynchronous and accepts the Amazon EC2
//! instance ids it is given on their modelled form alone
//! (`smithy_query(query="shape", shape="InstanceId")`: `^i-[\w\d]{8}$|^i-[\w\d]{17}$`) —
//! it does not ask the service that owns instances whether one exists, so a registration
//! naming an id that owner holds no record of is SERVED, the application it creates is
//! readable, and the ids it named are the ids the stored record reports. This service
//! invents no instance state of its own either way.
//!
//! WHY that is the rule, rather than the refusal it would be easy to read into the
//! modelled `ResourceNotFoundException`: the recorded behaviour of this service registers
//! applications against instance ids nothing ever launched, and the call succeeds — the
//! behavioural units `ssm-sap.deregister-application-removes-resource`,
//! `ssm-sap.get-application-missing-identifier-rejected`,
//! `ssm-sap.get-application-nonexistent-id-validation-error` and
//! `ssm-sap.get-operation-invalid-uuid-format-rejected` each register on a freshly
//! generated id of exactly that kind. This suite is the guardrail for it: a later change
//! that starts resolving instances at registration time fails here, beside the rule,
//! rather than only against the corpus.
//!
//! The rows span what the rule generalises over: both lengths the modelled id form
//! admits, ids the owning service would answer as unknown and ids it would not read as an
//! instance id at all, an id that owner really does hold, which member names it
//! (`Instances`, a declared component's `Ec2InstanceId`) — and whether that owner is
//! mounted beside this service at all, since the answer must not depend on it.

use serde_json::{json, Value};

use crate::application_lifecycle::{settle, valid_registration};
use crate::ec2_instance_shutdown::{registration, settle_wait, Emulator};

/// Instance ids the owner of an instance holds nothing for, spanning the answers it has
/// for one.
///
/// The first two are well formed — the two lengths EC2 has issued resource ids in — and
/// name no instance, since nothing in this emulator launches an all-`f` counter value. The
/// rest are outside the form EC2 reads an instance id in at all (a long form whose leading
/// digit is above zero, the all-zero placeholder, a body that is not hexadecimal) while
/// still inside the `InstanceId` pattern this service's own model declares, so each one
/// reaches the handler.
const INSTANCES_NOBODY_HOLDS: &[(&str, &str)] = &[
    (
        "a long-form id nothing was launched for",
        "i-0ffffffffffffffff",
    ),
    ("a short-form id nothing was launched for", "i-ffffffff"),
    (
        "a long-form id whose leading digit is above zero",
        "i-1234567890abcdef0",
    ),
    ("the all-zero placeholder id", "i-00000000000000000"),
    ("a body that is not hexadecimal", "i-0zzzzzzzzzzzzzzzz"),
];

/// A string member of the `Operation` the registration filed, read back through
/// `GetOperation` — so what an accepted registration stored is read out of persisted state
/// rather than out of the registering call's own answer.
async fn recorded_property(
    emulator: &Emulator,
    case: &str,
    operation_id: &str,
    property: &str,
) -> String {
    let reply = emulator
        .ssm_sap("/get-operation", json!({"OperationId": operation_id}))
        .await;
    reply
        .get("Operation")
        .and_then(|operation| operation.get("Properties"))
        .and_then(|properties| properties.get(property))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the operation to report {property}, got {reply}")
        })
        .to_string()
}

/// The `OperationId` a registration answered with.
fn operation_id(case: &str, reply: &Value) -> String {
    reply
        .get("OperationId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an OperationId, got {reply}"))
        .to_string()
}

/// RULE: with the owner of the instances mounted beside this service, a registration
/// naming an instance that owner holds nothing for is still served — and the instance it
/// named is what the stored record reports.
#[tokio::test]
async fn a_registration_is_served_whatever_the_owner_of_the_instance_holds() {
    let emulator = Emulator::new();
    // The control: an id that owner really does hold, so the rows differ in what EC2
    // would answer and in nothing else.
    let launched = emulator.launch_running(1).await[0].clone();
    let mut rows: Vec<(String, String)> = INSTANCES_NOBODY_HOLDS
        .iter()
        .map(|(label, instance_id)| ((*label).to_string(), (*instance_id).to_string()))
        .collect();
    rows.push(("an id the owner does hold".to_string(), launched));

    for (index, (label, instance_id)) in rows.iter().enumerate() {
        let application_id = format!("stored-instance-{index}");
        let reply = emulator
            .ssm_sap(
                "/register-application",
                registration(&application_id, instance_id, &[]),
            )
            .await;

        // The registration is under way, as an asynchronous call's answer reports it.
        assert_eq!(
            reply.get("Application").and_then(|app| app.get("Status")),
            Some(&json!("REGISTERING")),
            "case {label:?}: expected a REGISTERING application, got {reply}"
        );

        let recorded = recorded_property(
            &emulator,
            label,
            &operation_id(label, &reply),
            "Instances",
        )
        .await;
        assert_eq!(
            &recorded, instance_id,
            "case {label:?}: expected the record to report the instance the registration \
             named, got {recorded:?}"
        );

        // Nothing about the instance changes the registration's own outcome: the change
        // the clock owes it is the one every registration gets.
        tokio::time::sleep(settle_wait()).await;
        let fetched = emulator
            .ssm_sap(
                "/get-application",
                json!({"ApplicationId": &application_id}),
            )
            .await;
        assert_eq!(
            fetched.get("Application").and_then(|app| app.get("Status")),
            Some(&json!("ACTIVATED")),
            "case {label:?}: expected an ACTIVATED application, got {fetched}"
        );
    }
}

/// RULE: the same holds for the instance a DECLARED COMPONENT names, which is the other
/// place a registration associates an instance with the application.
#[tokio::test]
async fn a_component_on_an_instance_the_owner_holds_nothing_for_is_stored_too() {
    let emulator = Emulator::new();
    let system_instance = emulator.launch_running(1).await[0].clone();

    for (index, (label, instance_id)) in INSTANCES_NOBODY_HOLDS.iter().enumerate() {
        let case = format!("{label} as a component's Ec2InstanceId");
        let application_id = format!("stored-component-{index}");
        let reply = emulator
            .ssm_sap(
                "/register-application",
                registration(
                    &application_id,
                    &system_instance,
                    &[("WD", instance_id)],
                ),
            )
            .await;

        // The operation record names each declared component by its id and the instance
        // the declaration put it on, so the instance the component named is in there.
        let recorded = recorded_property(
            &emulator,
            &case,
            &operation_id(&case, &reply),
            "ComponentsInfo",
        )
        .await;
        assert!(
            recorded.contains(instance_id),
            "case {case:?}: expected the record to report the component's instance \
             {instance_id:?}, got {recorded:?}"
        );

        // The component is one of the application's own, reported by the listing that
        // walks them.
        let components = emulator
            .ssm_sap(
                "/list-components",
                json!({"ApplicationId": &application_id}),
            )
            .await;
        let listed = components
            .get("Components")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            listed
                .iter()
                .any(|component| component.get("ComponentId") == Some(&json!("HDB-WD"))),
            "case {case:?}: expected the declared component to be listed, got {components}"
        );
    }
}

/// RULE: the answer does not depend on the owner of the instances being reachable at all
/// — with no EC2 mounted beside this service the same registrations are served, so a
/// caller that never launched an instance can register against an id of the modelled
/// form.
#[tokio::test]
async fn a_registration_is_served_where_the_owner_of_the_instance_is_not_mounted() {
    let emulator = crate::application_lifecycle::Emulator::new();

    for (index, (label, instance_id)) in INSTANCES_NOBODY_HOLDS.iter().enumerate() {
        let application_id = format!("no-owner-{index}");
        let mut body = valid_registration(&application_id, "HANA");
        body["Instances"] = json!([instance_id]);

        let reply = emulator.send("/register-application", body).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected 200 from RegisterApplication, got {reply:?}"
        );
        settle().await;

        let fetched = emulator
            .send(
                "/get-application",
                json!({"ApplicationId": &application_id}),
            )
            .await;
        assert_eq!(
            fetched.app("Id").and_then(Value::as_str),
            Some(application_id.as_str()),
            "case {label:?}: expected the registration to be readable, got {fetched:?}"
        );
        assert_eq!(
            fetched.app_str("Status"),
            "ACTIVATED",
            "case {label:?}: expected an ACTIVATED application, got {fetched:?}"
        );
    }
}
