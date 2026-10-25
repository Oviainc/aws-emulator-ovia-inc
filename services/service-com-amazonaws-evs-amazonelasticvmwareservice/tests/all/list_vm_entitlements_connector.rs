// SPDX-License-Identifier: CC0-1.0

//! The `connectorId` `ListVmEntitlements` reports the entitlements OF, end to end over
//! awsJson1_0.
//!
//! The rule under test: a required resource handle an operation's input carries is
//! resolved against the store before it selects anything, and one that resolves to no
//! live resource is refused with the operation's modelled `ResourceNotFoundException`
//! naming that resource — never answered with a success a caller cannot tell from a
//! resource that holds no records yet. Where it comes from:
//!
//! * `connectorId` is a **required** member of `ListVmEntitlementsRequest`, patterned
//!   `^(cnctr-[a-zA-Z0-9]{10})$` (`smithy_query(query="operation",
//!   operation="ListVmEntitlements")`) — a handle on a connector resource, not a
//!   free-text label.
//! * "A service resource associated with the request could not be found. The resource
//!   might not be specified correctly, or it may have a `state` of `DELETED`"
//!   (`ResourceNotFoundException` doc) — the modelled refusal for such a handle, which
//!   this operation declares, reported with HTTP 400 like every other client error of
//!   this awsJson1_0 service (`tests/all/error_wire_status.rs` owns that rule).
//! * "Returns existing entitlements for virtual machines associated with the specified
//!   environment and connector" (`ListVmEntitlements` doc) — with both resources
//!   resolved, the pair selects which records are reported, so a connector the
//!   environment DOES hold and that has no entitlement of the requested type is an
//!   empty page.
//! * A connector id is scoped to its environment, exactly as
//!   `UpdateEnvironmentConnector` and `CreateEntitlement` read one
//!   (`tests/all/connectors.rs`, `tests/all/entitlement_connector.rs`).
//!
//! The harness (`emulator`, `call`, `create_settled`) is the one
//! `environment_lifecycle` already owns, so every test drives the service the way a
//! client does: `POST /` with `X-Amz-Target: AmazonElasticVMwareService.<Operation>`.

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::entitlement_connector::{connector_in, environment_with_vcenter_connector};
use crate::environment_lifecycle::{call, create_settled, emulator, error_type, Emulator};

/// Every `ConnectorType` the model declares, from the catalog this crate generated from
/// the model (`smithy_query(query="shape", shape="ConnectorType")`).
const CONNECTOR_TYPES: &[&str] = evs::ops::types::CONNECTOR_TYPE_VALUES;

/// The type of connector an entitlement is created through.
const VCENTER: &str = "VCENTER";

/// The only `EntitlementType` the model declares
/// (`smithy_query(query="shape", shape="EntitlementType")`).
const WINDOWS_SERVER: &str = "WINDOWS_SERVER";

/// The `resourceType` the not-found error names for a connector.
const RESOURCE_TYPE_CONNECTOR: &str = "connector";

/// A `ListVmEntitlements` request for one environment and connector.
fn list_request(environment_id: &str, connector_id: &str) -> Value {
    json!({
        "environmentId": environment_id,
        "connectorId": connector_id,
        "entitlementType": WINDOWS_SERVER,
    })
}

/// A `CreateEntitlement` request for `vm_ids`, through `connector_id`.
fn create_request(environment_id: &str, connector_id: &str, vm_ids: &[&str]) -> Value {
    json!({
        "environmentId": environment_id,
        "connectorId": connector_id,
        "entitlementType": WINDOWS_SERVER,
        "vmIds": vm_ids,
    })
}

/// The `entitlements` a response reports.
fn entitlements(case: &str, body: &Value) -> Vec<Value> {
    body["entitlements"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no entitlements list in {body}"))
        .clone()
}

/// Assert that `status`/`body` is this service's refusal of a connector handle that
/// resolves to nothing: HTTP 400, a `ResourceNotFoundException`, and the connector the
/// REQUEST named reported as the missing resource.
fn assert_connector_not_found(case: &str, connector_id: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_type(body).rsplit(['#', '/']).next().unwrap_or(""),
        "ResourceNotFoundException",
        "case {case:?}: expected a ResourceNotFoundException, got {}",
        error_type(body)
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
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains(connector_id),
        "case {case:?}: the message must name the connector the request asked for, got {body}"
    );
    assert!(
        body["entitlements"].is_null(),
        "case {case:?}: a refusal reports no page, got {body}"
    );
}

/// Force one stored connector of one environment to a `ConnectorState` and drop the
/// change the clock owed it, so a state is reached without the operation that reaches it
/// having to run first (`DeleteEnvironmentConnector` is what a caller uses).
fn force_connector_state(router: &Emulator, environment_id: &str, connector_id: &str, state: &str) {
    let store = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let mut store = store.lock();
    let environment = store
        .environments
        .get_mut(environment_id)
        .unwrap_or_else(|| panic!("the store holds the environment {environment_id}"));
    let connector = environment
        .connectors
        .get_mut(connector_id)
        .unwrap_or_else(|| panic!("the environment holds the connector {connector_id}"));
    connector.state = state.to_string();
    connector.pending = None;
}

#[tokio::test]
async fn a_connector_id_the_environment_holds_no_connector_for_is_refused() {
    let router = emulator();
    let (environment, vcenter) =
        environment_with_vcenter_connector(&router, "the environment under test").await;
    // A second environment with its own vCenter connector, so a connector that is live
    // in ANOTHER environment is one of the ids under test: a connector id is scoped to
    // its environment.
    let (other_environment, other_vcenter) =
        environment_with_vcenter_connector(&router, "another environment").await;
    // An environment with no connector at all, so the refusal does not depend on the
    // addressed environment holding some other connector.
    let bare_environment = create_settled(&router, Some("bare-env")).await;

    // Each row: the case, the environment listed, and the connector id the request
    // carries. The rows differ along what the rule generalises over — the FORM of the
    // well-formed-but-unknown id (spanning the modelled `^(cnctr-[a-zA-Z0-9]{10})$`:
    // digits, lower case, upper case, mixed), whether the addressed environment holds
    // other connectors, and whether the id is live somewhere ELSE.
    let rows: Vec<(String, &str, String)> = vec![
        (
            "a never-minted id, all digits".to_string(),
            environment.as_str(),
            "cnctr-0123456789".to_string(),
        ),
        (
            "a never-minted id, lower case".to_string(),
            environment.as_str(),
            "cnctr-abcdefghij".to_string(),
        ),
        (
            "a never-minted id, upper case".to_string(),
            environment.as_str(),
            "cnctr-ZZZZZZZZZZ".to_string(),
        ),
        (
            "a never-minted id, mixed".to_string(),
            environment.as_str(),
            "cnctr-0A1b2C3d4E".to_string(),
        ),
        (
            "a never-minted id against an environment with no connector".to_string(),
            bare_environment.as_str(),
            "cnctr-9999999999".to_string(),
        ),
        (
            "another environment's live vCenter connector".to_string(),
            environment.as_str(),
            other_vcenter.clone(),
        ),
        (
            "this environment's connector, asked for in the other environment".to_string(),
            other_environment.as_str(),
            vcenter.clone(),
        ),
    ];

    for (case, listed_environment, connector_id) in &rows {
        let (status, body) = call(
            &router,
            "ListVmEntitlements",
            &list_request(listed_environment, connector_id),
        )
        .await;
        assert_connector_not_found(case, connector_id, status, &body);

        // The paging members do not change the answer: the handle is resolved before
        // any page is cut.
        let mut paged = list_request(listed_environment, connector_id);
        paged["maxResults"] = json!(1);
        let (status, body) = call(&router, "ListVmEntitlements", &paged).await;
        assert_connector_not_found(&format!("{case} with maxResults"), connector_id, status, &body);
    }

    // The connector each environment DOES hold is still served, so the refusals above
    // are the connector lookup's answer and not the operation's answer to everything.
    for (case, listed_environment, connector_id) in [
        ("the environment's own connector", &environment, &vcenter),
        (
            "the other environment's own connector",
            &other_environment,
            &other_vcenter,
        ),
    ] {
        let (status, body) = call(
            &router,
            "ListVmEntitlements",
            &list_request(listed_environment, connector_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert!(
            entitlements(case, &body).is_empty(),
            "case {case:?}: a connector with no entitlement yet is an empty page, got {body}"
        );
        assert_eq!(
            body["nextToken"],
            Value::Null,
            "case {case:?}: an exhausted list carries no nextToken, got {body}"
        );
    }
}

#[tokio::test]
async fn every_modelled_connector_type_resolves_and_reports_its_own_records() {
    // A connector the environment holds resolves whichever TYPE it is: the type decides
    // what an entitlement can be created THROUGH (`tests/all/entitlement_connector.rs`),
    // not whether the handle names a resource. One environment holds at most one live
    // connector per type ("You can create only one connector of each type per
    // environment", `CreateEnvironmentConnector` doc), so each row gets its own.
    let router = emulator();
    assert!(
        CONNECTOR_TYPES.contains(&VCENTER),
        "the modelled catalog must declare {VCENTER}, got {CONNECTOR_TYPES:?}"
    );

    for connector_type in CONNECTOR_TYPES {
        let case = format!("a live {connector_type} connector");
        let environment = create_settled(&router, Some(&format!("typed-{connector_type}"))).await;
        let connector = connector_in(&router, &case, &environment, connector_type).await;

        let (status, body) = call(
            &router,
            "ListVmEntitlements",
            &list_request(&environment, &connector),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert!(
            entitlements(&case, &body).is_empty(),
            "case {case:?}: this connector holds no entitlement, got {body}"
        );

        // And the id one character away from it — the same id with its last character
        // changed — names no connector, so it is refused rather than reported empty.
        let mut characters: Vec<char> = connector.chars().collect();
        let last = characters.len() - 1;
        characters[last] = if characters[last] == 'z' { 'y' } else { 'z' };
        let near_miss: String = characters.into_iter().collect();
        let (status, body) = call(
            &router,
            "ListVmEntitlements",
            &list_request(&environment, &near_miss),
        )
        .await;
        assert_connector_not_found(
            &format!("{case}: a one-character-off id"),
            &near_miss,
            status,
            &body,
        );
    }
}

#[tokio::test]
async fn a_connector_that_has_reached_deleted_resolves_to_nothing() {
    // "The resource might not be specified correctly, or it may have a `state` of
    // `DELETED`" (`ResourceNotFoundException` doc): a DELETED connector is gone as far
    // as a lookup is concerned, even though the store still holds its record and the
    // entitlements created through it.
    let router = emulator();
    let (environment, connector) =
        environment_with_vcenter_connector(&router, "a deleted connector").await;
    let (status, body) = call(
        &router,
        "CreateEntitlement",
        &create_request(&environment, &connector, &["vm-1", "vm-2"]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the create: expected 200, got {status} {body}"
    );

    // While the connector is live its records are reported.
    let (status, body) = call(
        &router,
        "ListVmEntitlements",
        &list_request(&environment, &connector),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the live connector: expected 200, got {status} {body}"
    );
    assert_eq!(
        entitlements("the live connector", &body).len(),
        2,
        "case the live connector: expected both stored entitlements, got {body}"
    );

    force_connector_state(
        &router,
        &environment,
        &connector,
        evs::state::CONNECTOR_DELETED,
    );
    let (status, body) = call(
        &router,
        "ListVmEntitlements",
        &list_request(&environment, &connector),
    )
    .await;
    assert_connector_not_found("a DELETED connector", &connector, status, &body);
}

#[tokio::test]
async fn an_absent_environment_is_answered_before_the_connector_is_resolved() {
    // The environment is the scope the connector id is resolved IN, so an environment
    // this service holds no record of is reported as the missing resource — whatever
    // connector id the request carries. A read is also all this is: the refusals change
    // nothing a later caller sees.
    let router = emulator();
    let (environment, connector) =
        environment_with_vcenter_connector(&router, "a live environment").await;
    let (status, body) = call(
        &router,
        "CreateEntitlement",
        &create_request(&environment, &connector, &["vm-1"]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the create: expected 200, got {status} {body}"
    );
    let absent = "env-0000000000";

    for (case, connector_id) in [
        ("a never-minted connector id", "cnctr-0123456789"),
        ("a live connector of another environment", connector.as_str()),
    ] {
        let (status, body) = call(
            &router,
            "ListVmEntitlements",
            &list_request(absent, connector_id),
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

    // Repeating the refused list, then listing the real pair: nothing the refusals
    // touched was spent.
    for attempt in 0..3 {
        let (status, body) = call(
            &router,
            "ListVmEntitlements",
            &list_request(&environment, "cnctr-1111111111"),
        )
        .await;
        assert_connector_not_found(
            &format!("attempt {attempt}"),
            "cnctr-1111111111",
            status,
            &body,
        );
    }
    let (status, body) = call(
        &router,
        "ListVmEntitlements",
        &list_request(&environment, &connector),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the real pair: expected 200, got {status} {body}"
    );
    assert_eq!(
        entitlements("the real pair", &body).len(),
        1,
        "case the real pair: the refused lists must have consumed nothing, got {body}"
    );
}
