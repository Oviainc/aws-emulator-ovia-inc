// SPDX-License-Identifier: CC0-1.0

//! Which appliance an `applianceFqdn` NAMES, end to end over awsJson1_0.
//!
//! The rule under test: an `applianceFqdn` is "a fully qualified domain name"
//! (`smithy_query(query="doc",shape="CreateEnvironmentConnectorInput",member="applianceFqdn")`)
//! and a domain name is case-insensitive, so two spellings that differ only in case name
//! the SAME VCF appliance. Every site that asks whether a request names the appliance a
//! connector already targets therefore answers yes for either spelling:
//!
//! * `UpdateEnvironmentConnector` is `@idempotent`
//!   (`smithy_query(query="operation",operation="UpdateEnvironmentConnector")`): a
//!   request naming the appliance the connector already reaches reconfigures nothing, so
//!   the stored record — its `state`, its `status`, its `checks`, its `applianceFqdn` and
//!   its `modifiedAt` — and the environment's own `modifiedAt` all hold.
//! * `CreateEnvironmentConnector` is `@idempotent` too: a repeat naming the appliance and
//!   the secret the stored connector of that type carries reports that stored record
//!   rather than being refused as the second connector of its type.
//! * A request naming a DIFFERENT appliance is a reconfiguration (the update) or a
//!   conflict (the create), so the rows that differ by more than case sit in the same
//!   tables — what separates them is the appliance named, not the shape of the request.
//!
//! What the rule generalises over, and so what the tables span: the connector type (the
//! whole modelled catalog), the form and length of the FQDN (a multi-label name, a
//! single-label one, and both modelled bounds of `ApplianceFqdn`, len 1..253), and the
//! case transformation between the stored spelling and the requested one (upper from
//! lower, lower from upper, and label-by-label mixed case).
//!
//! Every record is read back through `ListEnvironmentConnectors` and `GetEnvironment`,
//! because what must hold is what was STORED.

use serde_json::{json, Value};

use crate::environment_lifecycle::{
    call, create_settled, emulator, error_type, settle, Emulator, PAST_THE_TRANSITION,
};

/// Every `ConnectorType` the model declares, from the catalog this crate generated from
/// the model (`smithy_query(query="shape",shape="ConnectorType")`), so the rows span the
/// whole catalog rather than one type.
const CONNECTOR_TYPES: &[&str] =
    service_com_amazonaws_evs_amazonelasticvmwareservice::ops::types::CONNECTOR_TYPE_VALUES;

/// The credential secret every connector here is created with: this suite varies the
/// appliance FQDN, so the secret is held fixed (`tests/all/connector_secret_arn.rs` owns
/// the secret class).
const SECRET_IDENTIFIER: &str = "evs/vcf-appliance-credentials";

/// An FQDN at the modelled `ApplianceFqdn` maximum of 253 characters: three 63-byte
/// labels (the longest a DNS label can be under that member's own pattern) and a 61-byte
/// one, dot-separated.
fn longest_fqdn() -> String {
    let label = "a".repeat(63);
    format!("{label}.{label}.{label}.{}", "a".repeat(61))
}

/// `fqdn` with the case of every label flipped label by label — the first label upper,
/// the next lower, and so on — so a row's requested spelling is neither the stored one
/// nor a uniform transformation of it.
fn mixed_case(fqdn: &str) -> String {
    fqdn.split('.')
        .enumerate()
        .map(|(index, label)| {
            if index % 2 == 0 {
                label.to_ascii_uppercase()
            } else {
                label.to_ascii_lowercase()
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// One row: the case, the connector type, the FQDN the connector is created with, the
/// FQDN the call under test names, and whether the two name the same appliance.
struct Row {
    case: &'static str,
    connector_type: &'static str,
    stored: String,
    requested: String,
    same_appliance: bool,
}

fn rows() -> Vec<Row> {
    let longest = longest_fqdn();
    vec![
        // The same appliance, spelled differently.
        Row {
            case: "a multi-label FQDN asked for in upper case",
            connector_type: "VCENTER",
            stored: "vcenter.evs.example.com".to_string(),
            requested: "VCENTER.EVS.EXAMPLE.COM".to_string(),
            same_appliance: true,
        },
        Row {
            case: "an upper-case stored FQDN asked for in lower case",
            connector_type: "SDDC_MANAGER",
            stored: "SDDC.EVS.EXAMPLE.COM".to_string(),
            requested: "sddc.evs.example.com".to_string(),
            same_appliance: true,
        },
        Row {
            case: "a stored FQDN asked for with its labels in mixed case",
            connector_type: "OPERATIONS_MANAGER",
            stored: "ops.evs.example.com".to_string(),
            requested: mixed_case("ops.evs.example.com"),
            same_appliance: true,
        },
        Row {
            case: "a single-label FQDN asked for in upper case",
            connector_type: "VCENTER",
            stored: "vcenter2".to_string(),
            requested: "VCENTER2".to_string(),
            same_appliance: true,
        },
        Row {
            case: "an FQDN at the modelled 1-character minimum asked for in upper case",
            connector_type: "VCENTER",
            stored: "a".to_string(),
            requested: "A".to_string(),
            same_appliance: true,
        },
        Row {
            case: "an FQDN at the modelled 253-character maximum asked for in upper case",
            connector_type: "SDDC_MANAGER",
            stored: longest.clone(),
            requested: longest.to_ascii_uppercase(),
            same_appliance: true,
        },
        Row {
            case: "a digit-and-hyphen FQDN asked for in mixed case",
            connector_type: "VCENTER",
            stored: "vcenter-01.evs-west.example.com".to_string(),
            requested: mixed_case("vcenter-01.evs-west.example.com"),
            same_appliance: true,
        },
        // ANOTHER appliance: the contrast rows, differing by more than case.
        Row {
            case: "another appliance in the same domain",
            connector_type: "VCENTER",
            stored: "vcenter.evs.example.com".to_string(),
            requested: "vcenter-2.evs.example.com".to_string(),
            same_appliance: false,
        },
        Row {
            case: "the same host name in another domain, upper case",
            connector_type: "OPERATIONS_MANAGER",
            stored: "ops.evs.example.com".to_string(),
            requested: "OPS.EVS.EXAMPLE.NET".to_string(),
            same_appliance: false,
        },
        Row {
            case: "a single-label FQDN differing in one character",
            connector_type: "SDDC_MANAGER",
            stored: "sddc2".to_string(),
            requested: "SDDC3".to_string(),
            same_appliance: false,
        },
    ]
}

/// A `CreateEnvironmentConnector` request.
fn create_request(environment_id: &str, connector_type: &str, appliance_fqdn: &str) -> Value {
    json!({
        "environmentId": environment_id,
        "type": connector_type,
        "applianceFqdn": appliance_fqdn,
        "secretIdentifier": SECRET_IDENTIFIER,
    })
}

/// Create a settled environment with one connector and return the environment id and the
/// reported connector.
async fn environment_with_connector(
    router: &Emulator,
    case: &str,
    connector_type: &str,
    appliance_fqdn: &str,
) -> (String, Value) {
    let environment = create_settled(router, None).await;
    let (status, body) = call(
        router,
        "CreateEnvironmentConnector",
        &create_request(&environment, connector_type, appliance_fqdn),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: create expected 200, got {status} ({}) {body}",
        error_type(&body)
    );
    (environment, body["connector"].clone())
}

/// The one connector of an environment, as `ListEnvironmentConnectors` reports it.
async fn only_connector(router: &Emulator, case: &str, environment_id: &str) -> Value {
    let (status, body) = call(
        router,
        "ListEnvironmentConnectors",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: list expected 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let listed = body["connectors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        listed.len(),
        1,
        "case {case:?}: expected exactly one stored connector, got {listed:?}"
    );
    listed[0].clone()
}

/// The `modifiedAt` `GetEnvironment` reports for one environment.
async fn environment_modified_at(router: &Emulator, case: &str, environment_id: &str) -> Value {
    let (status, body) = call(
        router,
        "GetEnvironment",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetEnvironment expected 200, got {status} ({}) {body}",
        error_type(&body)
    );
    body["environment"]["modifiedAt"].clone()
}

#[tokio::test]
async fn an_update_naming_the_appliance_the_connector_already_reaches_changes_nothing() {
    let router = emulator();
    for row in rows() {
        let case = row.case;
        let (environment, created) =
            environment_with_connector(&router, case, row.connector_type, &row.stored).await;
        let connector_id = created["connectorId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no connectorId in {created}"))
            .to_string();
        // Really wait out the connector's CREATING -> ACTIVE transition, so the record
        // under test is settled and every stamp it carries is in the past: this test
        // compares stamps a later call would write against them.
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let before = only_connector(&router, case, &environment).await;
        assert_eq!(
            before["state"],
            json!("ACTIVE"),
            "case {case:?}: the connector must be settled before the update, got {before}"
        );
        let environment_before = environment_modified_at(&router, case, &environment).await;

        let (status, body) = call(
            &router,
            "UpdateEnvironmentConnector",
            &json!({
                "environmentId": environment,
                "connectorId": connector_id,
                "applianceFqdn": row.requested,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: update expected 200, got {status} ({}) {body}",
            error_type(&body)
        );
        let updated = body["connector"].clone();
        let listed = only_connector(&router, case, &environment).await;
        assert_eq!(
            listed, updated,
            "case {case:?}: the list must report the record the update wrote, got {listed}"
        );
        let environment_after = environment_modified_at(&router, case, &environment).await;

        if row.same_appliance {
            // The request named the appliance the connector already reaches, so NOTHING
            // moved: not the stored spelling, not the state, not the health the state
            // decides, and not either modifiedAt.
            assert_eq!(
                updated, before,
                "case {case:?}: an update naming the appliance the connector already \
                 reaches must leave the record at {before}, got {updated}"
            );
            assert_eq!(
                environment_after, environment_before,
                "case {case:?}: an update that changed nothing must leave the \
                 environment's modifiedAt at {environment_before}, got {environment_after}"
            );
        } else {
            // The contrast: another appliance IS a reconfiguration, so the connector takes
            // the new value and the clock owes it ACTIVE again.
            assert_eq!(
                updated["applianceFqdn"],
                json!(row.requested),
                "case {case:?}: an update naming another appliance must store it, got {updated}"
            );
            assert_eq!(
                updated["state"],
                json!("UPDATING"),
                "case {case:?}: an update naming another appliance must restart the \
                 connector, got {updated}"
            );
            assert!(
                updated["modifiedAt"].as_f64().unwrap_or_default()
                    > before["modifiedAt"].as_f64().unwrap_or_default(),
                "case {case:?}: the connector's modifiedAt must move past {}, got {}",
                before["modifiedAt"],
                updated["modifiedAt"]
            );
            assert!(
                environment_after.as_f64().unwrap_or_default()
                    > environment_before.as_f64().unwrap_or_default(),
                "case {case:?}: the environment's modifiedAt must move past \
                 {environment_before}, got {environment_after}"
            );
        }
    }
}

#[tokio::test]
async fn a_create_repeated_for_the_same_appliance_in_another_case_reports_the_stored_connector() {
    let router = emulator();
    for row in rows() {
        let case = row.case;
        let (environment, created) =
            environment_with_connector(&router, case, row.connector_type, &row.stored).await;
        // Settled without sleeping: this test compares the records themselves, not the
        // span a transition takes.
        settle(&router);
        let before = only_connector(&router, case, &environment).await;
        let environment_before = environment_modified_at(&router, case, &environment).await;

        let (status, body) = call(
            &router,
            "CreateEnvironmentConnector",
            &create_request(&environment, row.connector_type, &row.requested),
        )
        .await;

        if row.same_appliance {
            assert_eq!(
                status, 200,
                "case {case:?}: a repeat naming the same appliance expected 200, got \
                 {status} ({}) {body}",
                error_type(&body)
            );
            assert_eq!(
                body["connector"], before,
                "case {case:?}: the repeat must report the stored connector {before}, got {}",
                body["connector"]
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a create naming another appliance for a type the \
                 environment already has a connector of expected 400, got {status} {body}"
            );
            assert_eq!(
                error_type(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body}"
            );
        }

        // Either way the environment still holds exactly the one connector it did, with
        // the record it carried, and its own stamp is where it was: neither a repeat nor a
        // refusal changes anything.
        let listed = only_connector(&router, case, &environment).await;
        assert_eq!(
            listed, before,
            "case {case:?}: the stored connector must hold at {before}, got {listed}"
        );
        assert_eq!(
            created["connectorId"], listed["connectorId"],
            "case {case:?}: no second connector may be minted, got {listed}"
        );
        assert_eq!(
            environment_modified_at(&router, case, &environment).await,
            environment_before,
            "case {case:?}: a call that changed nothing must leave the environment's \
             modifiedAt at {environment_before}"
        );
    }
}

#[tokio::test]
async fn every_connector_type_accepts_its_stored_appliance_in_another_case() {
    // The rule holds for every type the model declares, including one no row above
    // happens to pair with a given FQDN form: one environment per type, each connector
    // re-asked for in upper case.
    let router = emulator();
    for connector_type in CONNECTOR_TYPES {
        let case = *connector_type;
        let stored = format!("appliance.{}.evs.example.com", case.to_ascii_lowercase());
        // The modelled `ApplianceFqdn` pattern allows no underscore, which the type names
        // carry.
        let stored = stored.replace('_', "-");
        let (environment, created) =
            environment_with_connector(&router, case, connector_type, &stored).await;
        let connector_id = created["connectorId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no connectorId in {created}"))
            .to_string();
        settle(&router);
        let before = only_connector(&router, case, &environment).await;

        let (status, body) = call(
            &router,
            "UpdateEnvironmentConnector",
            &json!({
                "environmentId": environment,
                "connectorId": connector_id,
                "applianceFqdn": stored.to_ascii_uppercase(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: update expected 200, got {status} ({}) {body}",
            error_type(&body)
        );
        assert_eq!(
            body["connector"], before,
            "case {case:?}: an update naming the stored appliance in upper case must \
             leave the record at {before}, got {}",
            body["connector"]
        );
    }
}
