// SPDX-License-Identifier: CC0-1.0

//! The connector `CreateEntitlement` creates an entitlement THROUGH, end to end over
//! awsJson1_0.
//!
//! The rule under test: a `connectorId` the addressed environment holds no live vCenter
//! Server connector for names no connector the entitlement could be tracked through, so
//! the create is refused with the operation's modelled `ResourceNotFoundException` and
//! persists nothing. Where it comes from:
//!
//! * "Creates a Windows Server License entitlement for virtual machines in an Amazon EVS
//!   environment using the provided vCenter Server connector" (`CreateEntitlement` doc)
//!   — the connector is a resource of the request, not a free-text label.
//! * "`VCENTER`: Connector to a vCenter Server appliance. Required for features that
//!   depend on vCenter, such as Windows Server license-included"
//!   (`smithy_query(query="doc", shape="CreateEnvironmentConnectorRequest",
//!   member="type")`) — and `EntitlementType` has the single value `WINDOWS_SERVER`
//!   (`smithy_query(query="shape", shape="EntitlementType")`), so the entitlement
//!   surface IS that feature.
//! * "A service resource associated with the request could not be found. The resource
//!   might not be specified correctly, or it may have a `state` of `DELETED`"
//!   (`ResourceNotFoundException` doc) — the refusal, reported with HTTP 400 like every
//!   other client error of this awsJson1_0 service (`tests/all/error_wire_status.rs`
//!   owns that rule).
//! * A connector id is scoped to its environment, exactly as
//!   `UpdateEnvironmentConnector` reads one (`tests/all/connectors.rs`).
//!
//! The harness (`emulator`, `call`, `create`) is the one `environment_lifecycle`
//! already owns, so every test drives the service the way a client does: `POST /` with
//! `X-Amz-Target: AmazonElasticVMwareService.<Operation>`.

use serde_json::{json, Value};

use crate::environment_lifecycle::{call, create, emulator, error_type, Emulator};

/// Every `ConnectorType` the model declares, from the catalog this crate generated from
/// the model (`smithy_query(query="shape", shape="ConnectorType")`).
const CONNECTOR_TYPES: &[&str] =
    service_com_amazonaws_evs_amazonelasticvmwareservice::ops::types::CONNECTOR_TYPE_VALUES;

/// The type of connector an entitlement is created through.
const VCENTER: &str = "VCENTER";

/// The only `EntitlementType` the model declares.
const WINDOWS_SERVER: &str = "WINDOWS_SERVER";

/// The `resourceType` the not-found error names for a connector.
const RESOURCE_TYPE_CONNECTOR: &str = "connector";

/// Create one connector of `connector_type` in `environment_id` and return its minted
/// `connectorId` — the id a request that addresses that connector carries.
pub(crate) async fn connector_in(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    connector_type: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreateEnvironmentConnector",
        &json!({
            "environmentId": environment_id,
            "type": connector_type,
            // The modelled `ApplianceFqdn` pattern allows no underscore, which the type
            // names carry.
            "applianceFqdn": format!(
                "{}.evs.example.com",
                connector_type.to_lowercase().replace('_', "-")
            ),
            "secretIdentifier": "evs/vcf-appliance-credentials",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating a {connector_type} connector expected 200, got {status} {body}"
    );
    body["connector"]["connectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no connectorId in {body}"))
        .to_string()
}

/// A fresh environment with its own vCenter Server connector: the setup every
/// entitlement test needs, because an entitlement is created through such a connector.
pub(crate) async fn environment_with_vcenter_connector(
    router: &Emulator,
    case: &str,
) -> (String, String) {
    let environment = create(router, None).await;
    let connector = connector_in(router, case, &environment, VCENTER).await;
    (environment, connector)
}

/// A `CreateEntitlement` request.
fn create_request(environment_id: &str, connector_id: &str, vm_ids: &[&str]) -> Value {
    json!({
        "environmentId": environment_id,
        "connectorId": connector_id,
        "entitlementType": WINDOWS_SERVER,
        "vmIds": vm_ids,
    })
}

/// A `ListVmEntitlements` request for one environment and connector.
fn list_request(environment_id: &str, connector_id: &str) -> Value {
    json!({
        "environmentId": environment_id,
        "connectorId": connector_id,
        "entitlementType": WINDOWS_SERVER,
    })
}

/// The `entitlements` a response reports.
fn entitlements(case: &str, body: &Value) -> Vec<Value> {
    body["entitlements"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no entitlements list in {body}"))
        .clone()
}

/// Assert one refusal: HTTP 400, a `ResourceNotFoundException` naming the connector the
/// REQUEST asked for, and no entitlement stored for it.
async fn assert_refused(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    connector_id: &str,
    vm_ids: &[&str],
) {
    let (status, body) = call(
        router,
        "CreateEntitlement",
        &create_request(environment_id, connector_id, vm_ids),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_type(&body).rsplit(['#', '/']).next().unwrap_or(""),
        "ResourceNotFoundException",
        "case {case:?}: expected a ResourceNotFoundException, got {}",
        error_type(&body)
    );
    assert_eq!(
        body["resourceId"],
        json!(connector_id),
        "case {case:?}: resourceId must echo the requested connector id, got {body}"
    );
    assert_eq!(
        body["resourceType"],
        json!(RESOURCE_TYPE_CONNECTOR),
        "case {case:?}: unexpected resourceType, got {body}"
    );
    // Nothing was persisted for the connector the refused request named.
    let (status, listed) = call(
        router,
        "ListVmEntitlements",
        &list_request(environment_id, connector_id),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the read-back expected 200, got {status} {listed}"
    );
    assert!(
        entitlements(case, &listed).is_empty(),
        "case {case:?}: the refused create must persist nothing, got {listed}"
    );
}

#[tokio::test]
async fn a_connector_id_the_environment_holds_no_vcenter_connector_for_is_refused() {
    let router = emulator();
    let (environment, vcenter) =
        environment_with_vcenter_connector(&router, "the environment under test").await;
    // A second environment, so a connector that exists in ANOTHER environment is one of
    // the ids under test: a connector id is scoped to its environment.
    let (other_environment, other_vcenter) =
        environment_with_vcenter_connector(&router, "another environment").await;

    // Each row: the case, the connector id the request carries, and the vmIds it asks
    // for. The rows differ along what the rule generalises over — a never-minted id
    // (spanning the modelled `^(cnctr-[a-zA-Z0-9]{10})$`: digits, lower case, upper
    // case and mixed), a minted id of every OTHER connector type, a minted vCenter id
    // of a different environment, and the number of VMs asked for.
    let hundred_vms: Vec<String> = (1..=100).map(|ordinal| format!("vm-{ordinal}")).collect();
    let at_the_vm_ids_bound: Vec<&str> = hundred_vms.iter().map(String::as_str).collect();
    let mut rows: Vec<(String, String, Vec<&str>)> = vec![
        (
            "a never-minted id, all digits".to_string(),
            "cnctr-0123456789".to_string(),
            vec!["vm-1"],
        ),
        (
            "a never-minted id, lower case".to_string(),
            "cnctr-abcdefghij".to_string(),
            vec!["vm-1", "vm-2", "vm-3"],
        ),
        (
            "a never-minted id, upper case".to_string(),
            "cnctr-ZZZZZZZZZZ".to_string(),
            vec!["vm-9"],
        ),
        (
            // The modelled maximum of `vmIds` is 100 entries: a request at that bound
            // is refused exactly as a one-VM request is.
            "a never-minted id, mixed, at the vmIds maximum".to_string(),
            "cnctr-0A1b2C3d4E".to_string(),
            at_the_vm_ids_bound,
        ),
        (
            "another environment's vCenter connector".to_string(),
            other_vcenter.clone(),
            vec!["vm-1"],
        ),
    ];

    // Every modelled connector type that is NOT vCenter, created in the environment
    // under test: the connector exists, but it is not the appliance the Windows Server
    // license-included feature runs on.
    for connector_type in CONNECTOR_TYPES.iter().filter(|name| **name != VCENTER) {
        let id = connector_in(&router, connector_type, &environment, connector_type).await;
        rows.push((
            format!("a live {connector_type} connector of this environment"),
            id,
            vec!["vm-1"],
        ));
    }
    // The table covers every modelled type: each non-vCenter one as a refusal row, and
    // vCenter through the accepted create below, so the catalog and the table cannot
    // drift apart.
    assert!(
        CONNECTOR_TYPES.contains(&VCENTER),
        "the modelled catalog must declare {VCENTER}, got {CONNECTOR_TYPES:?}"
    );
    assert_eq!(
        rows.len(),
        5 + CONNECTOR_TYPES.len() - 1,
        "every non-vCenter modelled type must have a row, got {rows:?}"
    );

    for (case, connector_id, vm_ids) in &rows {
        assert_refused(&router, case, &environment, connector_id, vm_ids).await;
    }

    // The connector that DOES exist in this environment is still served, so the
    // refusals above are the connector lookup's answer and not the operation's answer
    // to everything.
    let (status, body) = call(
        &router,
        "CreateEntitlement",
        &create_request(&environment, &vcenter, &["vm-1"]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the environment's own vCenter connector: expected 200, got {status} {body}"
    );
    assert_eq!(
        entitlements("the environment's own vCenter connector", &body).len(),
        1,
        "case the environment's own vCenter connector: expected one entitlement, got {body}"
    );
    // And the other environment's own connector serves ITS environment: the refusal
    // above was about the pair, not about that connector.
    let (status, body) = call(
        &router,
        "CreateEntitlement",
        &create_request(&other_environment, &other_vcenter, &["vm-1"]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the other environment's own connector: expected 200, got {status} {body}"
    );
}

#[tokio::test]
async fn a_vcenter_connector_is_usable_while_it_is_still_being_established() {
    // A connector is `CREATING` until the clock owes it `ACTIVE`, and both are live
    // states: the record exists, so an entitlement can be created through it either
    // way. The create below runs straight after the connector create (CREATING) and
    // again once the owed change has come due (ACTIVE).
    let router = emulator();
    let (environment, connector) =
        environment_with_vcenter_connector(&router, "a CREATING connector").await;

    // Each row: the case, the VM asked for, and whether to wait for the connector's
    // owed state change first.
    let rows: [(&str, &str, bool); 2] = [
        ("while the connector is CREATING", "vm-1", false),
        ("once the connector is ACTIVE", "vm-2", true),
    ];
    for (case, vm_id, settle) in rows {
        if settle {
            // Long enough for the connector's CREATING -> ACTIVE change to come due
            // (its span is under a second by construction).
            std::thread::sleep(std::time::Duration::from_millis(900));
        }
        let (status, body) = call(
            &router,
            "CreateEntitlement",
            &create_request(&environment, &connector, &[vm_id]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let created = entitlements(case, &body);
        assert_eq!(
            created.len(),
            1,
            "case {case:?}: expected one entitlement, got {body}"
        );
        assert_eq!(
            created[0]["connectorId"],
            json!(connector),
            "case {case:?}: the entitlement must carry the connector it was created \
             through, got {}",
            created[0]["connectorId"]
        );
    }
}

#[tokio::test]
async fn an_absent_environment_is_answered_before_the_connector_is_looked_up() {
    // The environment is the scope the connector id is resolved IN, so an environment
    // this service holds no record of is reported as the missing resource — whatever
    // the connector id the request carries.
    let router = emulator();
    let (environment, connector) =
        environment_with_vcenter_connector(&router, "a live environment").await;
    let absent = "env-0000000000";

    for (case, connector_id) in [
        ("a never-minted connector id", "cnctr-0123456789"),
        ("another environment's connector id", connector.as_str()),
    ] {
        let (status, body) = call(
            &router,
            "CreateEntitlement",
            &create_request(absent, connector_id, &["vm-1"]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            body["resourceType"],
            json!("environment"),
            "case {case:?}: the missing resource is the environment, got {body}"
        );
        assert_eq!(
            body["resourceId"],
            json!(absent),
            "case {case:?}: resourceId must echo the requested environment, got {body}"
        );
    }

    // The live environment with its own connector is still served.
    let (status, body) = call(
        &router,
        "CreateEntitlement",
        &create_request(&environment, &connector, &["vm-1"]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the live environment: expected 200, got {status} {body}"
    );
}
