// SPDX-License-Identifier: CC0-1.0

//! `GetConnector` and `ListConnectors` project ONE stored connector record onto the connector
//! output shapes, so they can never report that record differently.
//!
//! `Connector` and `ConnectorSummary` model exactly the same ten members (smithy_query
//! shape=Connector,ConnectorSummary: Arn, CertificateAuthorityArn, Type,
//! MobileDeviceManagement, OpenIdConfiguration, Status, StatusReason, Endpoint, CreatedAt,
//! UpdatedAt). The rule these cases assert, over a table that spans the class rather than one
//! request: for any stored connector, whatever the inputs its create carried and whatever
//! status it is in, the single read and the listing entry for it carry the same ten members
//! with the same values — including the members that are absent, which are absent from both.
//!
//! Rows differ along what the projection generalises over: the connector `Type` (which decides
//! whether `MobileDeviceManagement` and `OpenIdConfiguration` are reported at all), the access
//! path (which decides the `Endpoint` host), and the `ConnectorStatus` the record is in,
//! including the one status that also reports a `StatusReason`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router};
use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::state;

/// A region that is NOT the emulator default, so nothing can pass by a hardcoded default.
const REGION: &str = "ap-southeast-2";

/// Every member `Connector` and `ConnectorSummary` model — the full projection
/// (smithy_query shape=Connector,ConnectorSummary).
const CONNECTOR_MEMBERS: &[&str] = &[
    "Arn",
    "CertificateAuthorityArn",
    "Type",
    "MobileDeviceManagement",
    "OpenIdConfiguration",
    "Status",
    "StatusReason",
    "Endpoint",
    "CreatedAt",
    "UpdatedAt",
];

/// The members a stored connector always reports, whatever its inputs or status were.
const ALWAYS_REPORTED: &[&str] = &[
    "Arn",
    "CertificateAuthorityArn",
    "Type",
    "Status",
    "Endpoint",
    "CreatedAt",
    "UpdatedAt",
];

/// Private-CA ARNs matching `CertificateAuthorityArn`'s own pattern, one per row, so no
/// assertion can pass by echoing a single value.
const CA_ARNS: &[&str] = &[
    "arn:aws:acm-pca:ap-southeast-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555",
    "arn:aws:acm-pca:ap-southeast-2:000000000000:certificate-authority/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
    "arn:aws:acm-pca:ap-southeast-2:000000000000:certificate-authority/0123456f-89ab-cdef-0123-456789abcdef",
    "arn:aws:acm-pca:ap-southeast-2:000000000000:certificate-authority/deadbeef-0000-1111-2222-333344445555",
    "arn:aws:acm-pca:ap-southeast-2:000000000000:certificate-authority/99999999-8888-7777-6666-555544443333",
    "arn:aws:acm-pca:ap-southeast-2:000000000000:certificate-authority/12345678-90ab-cdef-1234-567890abcdef",
];

/// A `ConnectorStatusReason` value, from that enum's own values (smithy_query
/// shape=ConnectorStatusReason): INTERNAL_FAILURE, PRIVATECA_ACCESS_DENIED,
/// PRIVATECA_INVALID_STATE, PRIVATECA_RESOURCE_NOT_FOUND, VPC_ENDPOINT_RESOURCE_NOT_FOUND,
/// VPC_ENDPOINT_DNS_ENTRIES_NOT_FOUND.
const STATUS_REASON: &str = "PRIVATECA_RESOURCE_NOT_FOUND";

/// Which status the row's record must be in when it is read and listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    /// Still provisioning: the transition the create scheduled is not due yet.
    Creating,
    /// Settled: the transition the create scheduled has come due.
    Active,
    /// `DeleteConnector` has been accepted and the teardown is not due yet.
    Deleting,
    /// The terminal failure status, which is also the one that reports a `StatusReason`.
    Failed,
}

/// One row: the create request's inputs and the status to read the record in.
#[derive(Debug)]
struct Row {
    name: &'static str,
    ca_arn: &'static str,
    vpc_endpoint_id: Option<&'static str>,
    mobile_device_management: Option<Value>,
    status: Status,
}

/// An `IntuneConfiguration` body, so a row can choose the Intune `Type` — the input that
/// decides whether `MobileDeviceManagement` and `OpenIdConfiguration` are reported.
fn intune(application_id: &str, domain: &str) -> Value {
    json!({ "Intune": { "AzureApplicationId": application_id, "Domain": domain } })
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "general purpose, public endpoint, ACTIVE",
            ca_arn: CA_ARNS[0],
            vpc_endpoint_id: None,
            mobile_device_management: None,
            status: Status::Active,
        },
        Row {
            name: "intune, public endpoint, ACTIVE (MDM and OIDC both reported)",
            ca_arn: CA_ARNS[1],
            vpc_endpoint_id: None,
            mobile_device_management: Some(intune(
                "01234567-89ab-cdef-0123-456789abcdef",
                "example.onmicrosoft.com",
            )),
            status: Status::Active,
        },
        Row {
            name: "general purpose, VPC endpoint, ACTIVE",
            ca_arn: CA_ARNS[2],
            vpc_endpoint_id: Some("vpce-0123abcd"),
            mobile_device_management: None,
            status: Status::Active,
        },
        Row {
            name: "intune, VPC endpoint, still CREATING",
            ca_arn: CA_ARNS[3],
            vpc_endpoint_id: Some("vpce-fedcba9887654321f"),
            mobile_device_management: Some(intune(
                "fedcba98-7654-3210-fedc-ba9876543210",
                "contoso.example.com",
            )),
            status: Status::Creating,
        },
        Row {
            name: "general purpose, public endpoint, DELETING",
            ca_arn: CA_ARNS[4],
            vpc_endpoint_id: None,
            mobile_device_management: None,
            status: Status::Deleting,
        },
        Row {
            name: "intune, VPC endpoint, FAILED with a StatusReason",
            ca_arn: CA_ARNS[5],
            vpc_endpoint_id: Some("vpce-0123abcd456789abc"),
            mobile_device_management: Some(intune(
                "aaaabbbb-cccc-dddd-eeee-ffff00001111",
                "fabrikam.example.org",
            )),
            status: Status::Failed,
        },
    ]
}

/// Make the transition the clock owes `arn` due now, so a read does not have to race the
/// transition span.
fn make_transition_due(engine: &engine::InMemoryEngine, case: &str, arn: &str) {
    assert!(
        harness::release_pending_if_owed(engine, arn),
        "case {case:?}: expected a transition the clock owes {arn:?}"
    );
}

/// Hold the transition the clock owes `arn` off, so the record stays in its non-terminal
/// status for the whole case however long the test takes.
fn hold_transition(engine: &engine::InMemoryEngine, case: &str, arn: &str) {
    assert!(
        harness::hold_pending_if_owed(engine, arn),
        "case {case:?}: expected a transition the clock owes {arn:?}"
    );
}

/// Put `arn` in the terminal failure status, the one status that also carries a
/// `StatusReason`. The emulator's own creates always succeed, so this record is arranged
/// through the service's state rather than through a handler.
fn force_failed(engine: &engine::InMemoryEngine, case: &str, arn: &str) {
    with_connector(engine, case, arn, |connector| {
        connector.pending = None;
        connector.status = "FAILED".to_string();
        connector.status_reason = Some(STATUS_REASON.to_string());
    });
}

fn with_connector(
    engine: &engine::InMemoryEngine,
    case: &str,
    arn: &str,
    edit: impl FnOnce(&mut state::StoredConnector),
) {
    let shared = state::service_state(engine).expect("the service's own state");
    let mut store = shared.lock();
    let connector = store
        .connectors
        .get_mut(arn)
        .unwrap_or_else(|| panic!("case {case:?}: the connector {arn:?} that was just created"));
    edit(connector);
}

/// The `Connector` `GetConnector` reports for `arn`.
async fn get_connector(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let read = harness::call(
        router,
        REGION,
        "GET",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected 200 from GetConnector, got {} / {:?}",
        read.status, read.body
    );
    read.body["Connector"].clone()
}

/// The `ConnectorSummary` `ListConnectors` reports for `arn`.
async fn list_connector(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let page = harness::call(router, REGION, "GET", "/connectors", None).await;
    assert_eq!(
        page.status, 200,
        "case {case:?}: expected 200 from ListConnectors, got {} / {:?}",
        page.status, page.body
    );
    let summaries = page.body["Connectors"].as_array().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a Connectors array in {:?}",
            page.body
        )
    });
    summaries
        .iter()
        .find(|entry| entry["Arn"] == json!(arn))
        .unwrap_or_else(|| panic!("case {case:?}: no ConnectorSummary for {arn:?} in {summaries:?}"))
        .clone()
}

/// Create the row's connector and arrange its status, answering its ARN.
async fn arrange(router: &MultiServiceRouter, engine: &engine::InMemoryEngine, row: &Row) -> String {
    let mut request = json!({ "CertificateAuthorityArn": row.ca_arn });
    if let Some(vpce) = row.vpc_endpoint_id {
        request["VpcEndpointId"] = json!(vpce);
    }
    if let Some(mdm) = &row.mobile_device_management {
        request["MobileDeviceManagement"] = mdm.clone();
    }

    let created = harness::call(router, REGION, "POST", "/connectors", Some(request)).await;
    assert_eq!(
        created.status, 202,
        "case {:?}: expected 202 from CreateConnector, got {} / {:?}",
        row.name, created.status, created.body
    );
    let arn = created.body["ConnectorArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: expected a ConnectorArn in {:?}",
                row.name, created.body
            )
        })
        .to_string();

    match row.status {
        Status::Creating => hold_transition(engine, row.name, &arn),
        Status::Active => make_transition_due(engine, row.name, &arn),
        Status::Deleting => {
            make_transition_due(engine, row.name, &arn);
            let deleted = harness::call(
                router,
                REGION,
                "DELETE",
                &format!("/connectors/{}", label(&arn)),
                None,
            )
            .await;
            assert_eq!(
                deleted.status, 202,
                "case {:?}: expected 202 from DeleteConnector, got {} / {:?}",
                row.name, deleted.status, deleted.body
            );
            hold_transition(engine, row.name, &arn);
        }
        Status::Failed => {
            make_transition_due(engine, row.name, &arn);
            // Settle the create through a handler first, so the forced status is not undone
            // by a transition the clock still owed.
            let _ = get_connector(router, row.name, &arn).await;
            force_failed(engine, row.name, &arn);
        }
    }
    arn
}

/// The expected `ConnectorStatus` string for a row.
fn expected_status(row: &Row) -> &'static str {
    match row.status {
        Status::Creating => "CREATING",
        Status::Active => "ACTIVE",
        Status::Deleting => "DELETING",
        Status::Failed => "FAILED",
    }
}

/// Whatever the stored record carries, the single read and the listing entry report it with
/// the same ten members and the same values.
#[tokio::test]
async fn get_and_list_report_one_connector_identically() {
    for row in rows() {
        let (router, engine) = new_router();
        let arn = arrange(&router, &engine, &row).await;

        let read = get_connector(&router, row.name, &arn).await;
        let listed = list_connector(&router, row.name, &arn).await;

        // The row's inputs and status really did reach the record, so the agreement below is
        // asserted over a record that differs from the other rows'.
        assert_eq!(
            read["Arn"],
            json!(arn),
            "case {:?}: the read must report the ARN it was asked for, got {read:?}",
            row.name
        );
        assert_eq!(
            read["CertificateAuthorityArn"],
            json!(row.ca_arn),
            "case {:?}: expected the stored CA {:?}, got {:?}",
            row.name,
            row.ca_arn,
            read["CertificateAuthorityArn"]
        );
        assert_eq!(
            read["Status"],
            json!(expected_status(&row)),
            "case {:?}: expected Status {:?}, got {:?}",
            row.name,
            expected_status(&row),
            read["Status"]
        );
        let expected_type = if row.mobile_device_management.is_some() {
            "INTUNE"
        } else {
            "GENERAL_PURPOSE"
        };
        assert_eq!(
            read["Type"],
            json!(expected_type),
            "case {:?}: expected Type {expected_type:?}, got {:?}",
            row.name,
            read["Type"]
        );
        for member in ALWAYS_REPORTED {
            assert!(
                !read[member].is_null(),
                "case {:?}: {member} must be reported, got {read:?}",
                row.name
            );
        }
        assert_eq!(
            !read["StatusReason"].is_null(),
            row.status == Status::Failed,
            "case {:?}: StatusReason is reported only for a FAILED connector, got {:?}",
            row.name,
            read["StatusReason"]
        );
        assert_eq!(
            !read["MobileDeviceManagement"].is_null(),
            row.mobile_device_management.is_some(),
            "case {:?}: MobileDeviceManagement is reported only for an Intune connector, got {:?}",
            row.name,
            read["MobileDeviceManagement"]
        );

        // The rule: the same ten members, present or absent together, with the same values.
        for member in CONNECTOR_MEMBERS {
            assert_eq!(
                read[member], listed[member],
                "case {:?}: GetConnector and ListConnectors must agree on {member}, got {:?} vs \
                 {:?}",
                row.name, read[member], listed[member]
            );
        }
        assert_eq!(
            read, listed,
            "case {:?}: the two projections of one stored connector must be identical, got \
             {read:?} vs {listed:?}",
            row.name
        );

        // Neither shape models anything beyond those ten members.
        for (shape, value) in [("Connector", &read), ("ConnectorSummary", &listed)] {
            let object = value.as_object().unwrap_or_else(|| {
                panic!("case {:?}: expected a {shape} object, got {value:?}", row.name)
            });
            let mut unexpected: Vec<&String> = object
                .keys()
                .filter(|key| !CONNECTOR_MEMBERS.contains(&key.as_str()))
                .collect();
            unexpected.sort();
            assert!(
                unexpected.is_empty(),
                "case {:?}: {shape} models only {CONNECTOR_MEMBERS:?}, got the extra members \
                 {unexpected:?} in {value:?}",
                row.name
            );
        }

        // Reading and listing change nothing a later caller sees: both answer again, still
        // agreeing with each other.
        let read_again = get_connector(&router, row.name, &arn).await;
        let listed_again = list_connector(&router, row.name, &arn).await;
        assert_eq!(
            (&read_again, &listed_again),
            (&read, &listed),
            "case {:?}: a repeated read and listing must answer identically, got {read_again:?} \
             and {listed_again:?}",
            row.name
        );
    }
}
