// SPDX-License-Identifier: CC0-1.0

//! Deleting one connector of an Amazon EVS environment, end to end over awsJson1_0.
//!
//! The rules under test, and where each comes from:
//!
//! * `DeleteEnvironmentConnector` writes the deletion of the connector the request
//!   names into the stored environment and reports that record together with a summary
//!   of the environment it was deleted from, whatever the connector's type, the form of
//!   its appliance FQDN, the form of its secret, or the state it was in when the delete
//!   arrived. `ListEnvironmentConnectors` reads the same record back, and
//!   `ListEnvironments` reports the same summary.
//! * The deleted connector is `DELETING` and the clock owes it `DELETED`: the
//!   connection "to the given appliance type" (`Connector` doc) that
//!   `CreateEnvironmentConnector` established has to be torn down, so a caller reading
//!   straight after the call still sees the transient state.
//! * "Before deleting a connector, you must remove all entitlements that are associated
//!   with the same vCenter" (`DeleteEnvironmentConnector` doc): while that vCenter's
//!   entitlements are still live the delete is refused with the operation's modelled
//!   `ValidationException` and nothing about the connector changes. The vCenter is the
//!   APPLIANCE, so an entitlement created through a connector that was replaced by
//!   another one for the same `applianceFqdn` is in the way of the replacement too.
//! * The operation is `@idempotent`
//!   (`smithy_query(query="operation",operation="DeleteEnvironmentConnector")`): a
//!   repeat reports the record as it stands and restarts nothing.
//! * An `environmentId` that names no live environment at this endpoint's Region, or a
//!   `connectorId` that environment holds no connector record for, is a
//!   `ResourceNotFoundException` naming the resource the request asked for, and nothing
//!   is written for it.
//! * The connector records an environment is FINISHED with are kept so a later read
//!   still reports the deletion, bounded by `MAX_CONNECTORS_PER_ENVIRONMENT`: a
//!   create/delete loop cannot grow the store without end.
//!
//! The harness (`emulator`, `call`, `call_in_region`, `create_settled`, `settle`) is the
//! one `environment_lifecycle` already owns, so every test drives the service the way a
//! client does: `POST /` with `X-Amz-Target: AmazonElasticVMwareService.<Operation>`.

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::entitlement_connector::environment_with_vcenter_connector;
use crate::environment_lifecycle::{
    call, call_in_region, create_settled, emulator, error_type, secret_arn_of, settle, Emulator,
};

/// Every `ConnectorType` the model declares, from the catalog this crate generated from
/// the model (`smithy_query(query="shape", shape="ConnectorType")`).
const CONNECTOR_TYPES: &[&str] = evs::ops::types::CONNECTOR_TYPE_VALUES;

/// The type of connector an entitlement is created through.
const VCENTER: &str = "VCENTER";

/// The only `EntitlementType` the model declares
/// (`smithy_query(query="shape", shape="EntitlementType")`).
const WINDOWS_SERVER: &str = "WINDOWS_SERVER";

/// The `resourceType` the not-found error names for a connector, and the one it names
/// for an environment.
const RESOURCE_TYPE_CONNECTOR: &str = "connector";
/// See [`RESOURCE_TYPE_CONNECTOR`].
const RESOURCE_TYPE_ENVIRONMENT: &str = "environment";

/// A secret given as a full Secrets Manager ARN, one form `secretIdentifier` accepts.
const SECRET_ARN: &str = "arn:aws:secretsmanager:us-west-2:000000000000:secret:evs/vcf-AbCd12";

/// A secret given as a bare name, the other form it accepts.
const SECRET_NAME: &str = "evs/vcf-appliance-credentials";

/// An FQDN at the modelled `ApplianceFqdn` maximum of 253 characters: three 63-byte
/// labels (the longest a DNS label can be under that member's own pattern) and a
/// 61-byte one, dot-separated.
fn longest_fqdn() -> String {
    let label = "a".repeat(63);
    format!("{label}.{label}.{label}.{}", "a".repeat(61))
}

/// A `DeleteEnvironmentConnector` request.
fn delete_request(environment_id: &str, connector_id: &str) -> Value {
    json!({
        "environmentId": environment_id,
        "connectorId": connector_id,
    })
}

/// Create one connector in `environment_id` and return the record the create reported.
async fn create_connector(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    connector_type: &str,
    appliance_fqdn: &str,
    secret_identifier: &str,
) -> Value {
    let (status, body) = call(
        router,
        "CreateEnvironmentConnector",
        &json!({
            "environmentId": environment_id,
            "type": connector_type,
            "applianceFqdn": appliance_fqdn,
            "secretIdentifier": secret_identifier,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating a {connector_type} connector expected 200, got {status} {body}"
    );
    body["connector"].clone()
}

/// Every connector record one environment holds, as `ListEnvironmentConnectors` reports
/// them. The suites here hold far fewer than one page, so a `nextToken` would be a
/// defect of the list rather than a page to follow.
async fn listed_connectors(router: &Emulator, case: &str, environment_id: &str) -> Vec<Value> {
    let (status, body) = call(
        router,
        "ListEnvironmentConnectors",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the list expected 200, got {status} {body}"
    );
    assert!(
        body["nextToken"].is_null(),
        "case {case:?}: the whole population fits one page, got {body}"
    );
    body["connectors"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no connectors list in {body}"))
        .clone()
}

/// The record `ListEnvironmentConnectors` reports for one connector of one environment.
async fn stored_connector(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    connector_id: &str,
) -> Value {
    let listed = listed_connectors(router, case, environment_id).await;
    listed
        .iter()
        .find(|connector| connector["connectorId"] == json!(connector_id))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the list reports no connector {connector_id}: {listed:?}")
        })
        .clone()
}

/// The summary `ListEnvironments` reports for one environment — what the delete's own
/// `environmentSummary` is checked against, so the two reads of one record agree.
async fn listed_summary(router: &Emulator, case: &str, environment_id: &str) -> Value {
    let (status, body) = call(router, "ListEnvironments", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListEnvironments expected 200, got {status} {body}"
    );
    body["environmentSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no environmentSummaries in {body}"))
        .iter()
        .find(|summary| summary["environmentId"] == json!(environment_id))
        .unwrap_or_else(|| panic!("case {case:?}: no summary for {environment_id} in {body}"))
        .clone()
}

/// The `connector` of a response body, which every accepted delete reports.
fn reported_connector(case: &str, body: &Value) -> Value {
    assert!(
        body["connector"].is_object(),
        "case {case:?}: no connector in {body}"
    );
    body["connector"].clone()
}

/// Assert one member of a reported connector.
fn assert_member(case: &str, reported: &Value, member: &str, expected: &Value) {
    assert_eq!(
        &reported[member], expected,
        "case {case:?}: {member}: expected {expected}, got {}",
        reported[member]
    );
}

/// A timestamp member as epoch seconds, for the ordering assertions.
fn seconds(case: &str, reported: &Value, member: &str) -> f64 {
    reported[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: {member} is no timestamp in {reported}"))
}

/// Create `vm_ids` entitlements through one vCenter connector.
async fn create_entitlements(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    connector_id: &str,
    vm_ids: &[String],
) {
    let (status, body) = call(
        router,
        "CreateEntitlement",
        &json!({
            "environmentId": environment_id,
            "connectorId": connector_id,
            "entitlementType": WINDOWS_SERVER,
            "vmIds": vm_ids,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating {} entitlement(s) expected 200, got {status} {body}",
        vm_ids.len()
    );
}

/// Force every stored entitlement of one environment to a terminal `EntitlementStatus`,
/// so the state a caller reaches is exercised without depending on how a record got
/// there (`DeleteEntitlement` removes the records it stops, and nothing writes
/// `ENTITLEMENT_REMOVED`). The two terminal values are alternated, because the
/// rule is about an entitlement that no longer tracks usage and not about one of them.
fn force_entitlements_removed(router: &Emulator, environment_id: &str) {
    let store = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let mut store = store.lock();
    let environment = store
        .environments
        .get_mut(environment_id)
        .unwrap_or_else(|| panic!("the store holds the environment {environment_id}"));
    for (ordinal, entitlement) in environment.entitlements.values_mut().enumerate() {
        entitlement.status = if ordinal % 2 == 0 {
            evs::state::ENTITLEMENT_DELETED.to_string()
        } else {
            evs::state::ENTITLEMENT_REMOVED.to_string()
        };
        entitlement.pending = None;
    }
}

#[tokio::test]
async fn a_connector_is_deleted_and_the_deletion_is_read_back() {
    let router = emulator();

    // Each row: the case, the connector type, the appliance FQDN, the secret identifier,
    // and whether the connector's own CREATING -> ACTIVE change has come due before the
    // delete. The rows differ along what the rule generalises over — the type (the whole
    // modelled catalog), the form and the length of the FQDN (both modelled bounds of
    // `ApplianceFqdn`, len 1..253), the form of the secret (an ARN and a bare name), and
    // the `ConnectorState` the connector is deleted FROM.
    let rows: Vec<(&str, &str, String, &str, bool)> = vec![
        (
            "an ACTIVE vCenter connector with a three-label FQDN and an ARN secret",
            "VCENTER",
            "vcenter.evs.example.com".to_string(),
            SECRET_ARN,
            true,
        ),
        (
            "a CREATING SDDC Manager connector with a named secret",
            "SDDC_MANAGER",
            "sddc-manager.evs.example.com".to_string(),
            SECRET_NAME,
            false,
        ),
        (
            "an ACTIVE Operations Manager connector with a single-label FQDN",
            "OPERATIONS_MANAGER",
            "operations-manager".to_string(),
            SECRET_NAME,
            true,
        ),
        (
            "a connector whose FQDN is at the modelled 1-character minimum",
            "VCENTER",
            "a".to_string(),
            SECRET_ARN,
            false,
        ),
        (
            "a connector whose FQDN is at the modelled 253-character maximum",
            "VCENTER",
            longest_fqdn(),
            SECRET_ARN,
            true,
        ),
    ];
    // Every modelled type is covered by the rows above, so the catalog and the table
    // cannot drift apart.
    for connector_type in CONNECTOR_TYPES {
        assert!(
            rows.iter().any(|(_, row_type, _, _, _)| row_type == connector_type),
            "the table must cover the modelled type {connector_type:?}"
        );
    }

    for (case, connector_type, appliance_fqdn, secret_identifier, settle_first) in rows {
        // A fresh environment per row, so each row reads back only its own record.
        let environment = create_settled(&router, None).await;
        let created = create_connector(
            &router,
            case,
            &environment,
            connector_type,
            &appliance_fqdn,
            secret_identifier,
        )
        .await;
        let connector_id = created["connectorId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no connectorId in {created}"))
            .to_string();
        if settle_first {
            settle(&router);
        }
        let before = stored_connector(&router, case, &environment, &connector_id).await;
        assert_member(
            case,
            &before,
            "state",
            &json!(if settle_first { "ACTIVE" } else { "CREATING" }),
        );

        let (status, body) = call(
            &router,
            "DeleteEnvironmentConnector",
            &delete_request(&environment, &connector_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete expected 200, got {status} {body}"
        );
        let deleted = reported_connector(case, &body);

        // The deleted connector is the record the create stored, in the state the
        // teardown of its connection puts it in.
        for (member, expected) in [
            ("environmentId", json!(environment)),
            ("connectorId", json!(connector_id)),
            ("type", json!(connector_type)),
            ("applianceFqdn", json!(appliance_fqdn)),
            ("secretArn", json!(secret_arn_of(secret_identifier))),
            // The connection is still being torn down: the span is never zero, so a
            // caller reading straight after the call sees the transient state.
            ("state", json!("DELETING")),
            // No check has run: this emulator contacts no appliance, so the
            // connectivity-health roll-up is UNKNOWN and there are no checks.
            ("status", json!("UNKNOWN")),
            ("checks", Value::Null),
            ("createdAt", created["createdAt"].clone()),
        ] {
            assert_member(case, &deleted, member, &expected);
        }
        assert!(
            seconds(case, &deleted, "modifiedAt") >= seconds(case, &deleted, "createdAt"),
            "case {case:?}: the deletion must stamp modifiedAt at or after createdAt, got {deleted}"
        );

        // The environment the connector was deleted FROM, as every other read of it
        // reports it.
        assert_eq!(
            body["environmentSummary"],
            listed_summary(&router, case, &environment).await,
            "case {case:?}: the summary must be the environment's own, got {body}"
        );

        // The read-back: the list reports the record the delete wrote.
        let listed = stored_connector(&router, case, &environment, &connector_id).await;
        assert_eq!(
            listed, deleted,
            "case {case:?}: the list must report the deleted connector, got {listed}"
        );

        // `@idempotent`: the same delete again reports the record as it stands, and
        // neither restarts the teardown nor moves a stamp.
        let (status, body) = call(
            &router,
            "DeleteEnvironmentConnector",
            &delete_request(&environment, &connector_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated delete expected 200, got {status} {body}"
        );
        assert_eq!(
            reported_connector(case, &body),
            deleted,
            "case {case:?}: a repeated delete must report the same record, got {body}"
        );

        // The clock owes the connector DELETED, and the record's other members hold
        // across the change.
        settle(&router);
        let settled = stored_connector(&router, case, &environment, &connector_id).await;
        assert_member(case, &settled, "state", &json!("DELETED"));
        assert_member(case, &settled, "createdAt", &created["createdAt"]);
        assert_member(case, &settled, "applianceFqdn", &json!(appliance_fqdn));
        assert_member(
            case,
            &settled,
            "secretArn",
            &json!(secret_arn_of(secret_identifier)),
        );
        // And a delete of the settled record is still the idempotent answer.
        let (status, body) = call(
            &router,
            "DeleteEnvironmentConnector",
            &delete_request(&environment, &connector_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting a DELETED connector expected 200, got {status} {body}"
        );
        assert_eq!(
            reported_connector(case, &body),
            settled,
            "case {case:?}: it must report the settled record, got {body}"
        );

        // The type the deleted connector held is free again: "You can create only one
        // connector of each type per environment" counts the LIVE ones.
        let replacement = create_connector(
            &router,
            case,
            &environment,
            connector_type,
            &appliance_fqdn,
            secret_identifier,
        )
        .await;
        assert_ne!(
            replacement["connectorId"], json!(connector_id),
            "case {case:?}: the replacement must be its own record, got {replacement}"
        );
        assert_member(case, &replacement, "state", &json!("CREATING"));
    }
}

#[tokio::test]
async fn a_connector_whose_vcenter_still_has_entitlements_is_not_deleted() {
    let router = emulator();

    // Each row: the case, how many virtual machines are entitled through the connector,
    // and whether their CREATING -> CREATED change has come due before the delete. The
    // rows differ along what the rule generalises over — the number of entitlements in
    // the way (one, a few, and the modelled `vmIds` maximum of 100 per call) and the
    // `EntitlementStatus` they are in, since an entitlement still being validated is as
    // present as one that is tracking usage.
    let rows: [(&str, usize, bool); 4] = [
        ("one CREATING entitlement", 1, false),
        ("one CREATED entitlement", 1, true),
        ("three CREATED entitlements", 3, true),
        ("a hundred CREATING entitlements", 100, false),
    ];

    for (case, vm_count, settle_first) in rows {
        let (environment, vcenter) = environment_with_vcenter_connector(&router, case).await;
        // A connector of another type in the same environment, which no entitlement is
        // associated with: it shows the refusal below is the precondition's answer and
        // not the operation's answer to everything.
        let other = create_connector(
            &router,
            case,
            &environment,
            "SDDC_MANAGER",
            "sddc-manager.evs.example.com",
            SECRET_NAME,
        )
        .await;
        let vm_ids: Vec<String> = (1..=vm_count).map(|ordinal| format!("vm-{ordinal}")).collect();
        create_entitlements(&router, case, &environment, &vcenter, &vm_ids).await;
        if settle_first {
            settle(&router);
        }
        let before = stored_connector(&router, case, &environment, &vcenter).await;

        let (status, body) = call(
            &router,
            "DeleteEnvironmentConnector",
            &delete_request(&environment, &vcenter),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_type(&body).rsplit(['#', '/']).next().unwrap_or(""),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {}",
            error_type(&body)
        );
        assert_eq!(
            body["fieldList"][0]["name"],
            json!("connectorId"),
            "case {case:?}: the refusal must name the member it refuses, got {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&vm_count.to_string()) && message.contains("DeleteEntitlement"),
            "case {case:?}: the message must say how many entitlements are in the way and \
             how to remove them, got {message:?}"
        );
        // NOTHING about the refused connector changed.
        assert_eq!(
            stored_connector(&router, case, &environment, &vcenter).await,
            before,
            "case {case:?}: the refused delete must change nothing about the connector"
        );
        // And the connector no entitlement is associated with is deleted meanwhile.
        let other_id = other["connectorId"].as_str().unwrap_or_default();
        let (status, body) = call(
            &router,
            "DeleteEnvironmentConnector",
            &delete_request(&environment, other_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting the connector with no entitlements expected 200, \
             got {status} {body}"
        );
        assert_member(case, &reported_connector(case, &body), "state", &json!("DELETING"));

        // Once the entitlements are removed, the delete the precondition was refusing is
        // accepted and nothing else about the connector changed in between.
        force_entitlements_removed(&router, &environment);
        let (status, body) = call(
            &router,
            "DeleteEnvironmentConnector",
            &delete_request(&environment, &vcenter),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: with the entitlements removed the delete expected 200, \
             got {status} {body}"
        );
        let deleted = reported_connector(case, &body);
        assert_member(case, &deleted, "state", &json!("DELETING"));
        assert_member(case, &deleted, "connectorId", &json!(vcenter));
        assert_member(case, &deleted, "createdAt", &before["createdAt"]);
    }
}

#[tokio::test]
async fn entitlements_of_the_same_vcenter_block_a_replacement_connector() {
    // The vCenter an entitlement is associated with is the APPLIANCE the connector
    // reaches, not the connector record: a connector created for an appliance whose
    // entitlements are still live is in the way of its own deletion, even though those
    // entitlements were created through the connector it replaced.
    let router = emulator();
    let case = "a replacement connector for the same appliance";
    let (environment, first) = environment_with_vcenter_connector(&router, case).await;
    create_entitlements(
        &router,
        case,
        &environment,
        &first,
        &["vm-1".to_string(), "vm-2".to_string()],
    )
    .await;
    // The first connector reaches its terminal state without its entitlements being
    // removed, which only the store can do: the rule under test is what refuses the
    // delete that would otherwise get it there.
    let appliance_fqdn = {
        let store = evs::state::service_state(&*router.engine)
            .expect("the in-memory engine carries this service's store");
        let mut store = store.lock();
        let environment = store
            .environments
            .get_mut(&environment)
            .expect("the store holds the environment under test");
        let connector = environment
            .connectors
            .get_mut(&first)
            .expect("the environment holds the connector under test");
        connector.state = evs::state::CONNECTOR_DELETED.to_string();
        connector.pending = None;
        connector.appliance_fqdn.clone()
    };

    // The replacement: the same type, for the SAME appliance, named by the other form of
    // its secret so nothing but the appliance ties the two records together.
    let replacement = create_connector(
        &router,
        case,
        &environment,
        VCENTER,
        // DNS names are case-insensitive, so the uppercase spelling is the same vCenter.
        &appliance_fqdn.to_uppercase(),
        SECRET_ARN,
    )
    .await;
    let replacement_id = replacement["connectorId"].as_str().unwrap_or_default();
    let (status, body) = call(
        &router,
        "DeleteEnvironmentConnector",
        &delete_request(&environment, replacement_id),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_type(&body).rsplit(['#', '/']).next().unwrap_or(""),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {}",
        error_type(&body)
    );
    assert_member(
        case,
        &stored_connector(&router, case, &environment, replacement_id).await,
        "state",
        &json!("CREATING"),
    );

    // A connector for ANOTHER appliance is not in the way of those entitlements, so it
    // is deleted while the replacement above is refused: the rule is about the vCenter.
    let elsewhere = create_connector(
        &router,
        case,
        &environment,
        "OPERATIONS_MANAGER",
        "operations-manager.evs.example.com",
        SECRET_NAME,
    )
    .await;
    let (status, body) = call(
        &router,
        "DeleteEnvironmentConnector",
        &delete_request(&environment, elsewhere["connectorId"].as_str().unwrap_or_default()),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: deleting a connector of another appliance expected 200, got {status} {body}"
    );
}

#[tokio::test]
async fn unknown_environments_and_connectors_are_reported_as_not_found() {
    let router = emulator();
    let case = "the environment under test";
    let (environment, connector) = environment_with_vcenter_connector(&router, case).await;
    // A second environment with its own connector, so a connector that is live in
    // ANOTHER environment is one of the ids under test: a connector id is scoped to its
    // environment.
    let (other_environment, other_connector) =
        environment_with_vcenter_connector(&router, "another environment").await;
    let absent_environment = "env-0000000000";

    // Each row: the case, the Region the client signs for, the environmentId and the
    // connectorId the request carries, and the resource the refusal must name. The rows
    // differ along what the rule generalises over — a never-minted connector id
    // (spanning the modelled `^(cnctr-[a-zA-Z0-9]{10})$`: digits, lower case, upper case
    // and mixed), a minted id belonging to another environment, an environment this
    // service holds no record of, and an environment that exists in another Region.
    let rows: Vec<(String, &str, String, String, &str, String)> = vec![
        (
            "a never-minted connector id, all digits".to_string(),
            "us-west-2",
            environment.clone(),
            "cnctr-0123456789".to_string(),
            RESOURCE_TYPE_CONNECTOR,
            "cnctr-0123456789".to_string(),
        ),
        (
            "a never-minted connector id, lower case".to_string(),
            "us-west-2",
            environment.clone(),
            "cnctr-abcdefghij".to_string(),
            RESOURCE_TYPE_CONNECTOR,
            "cnctr-abcdefghij".to_string(),
        ),
        (
            "a never-minted connector id, upper case".to_string(),
            "us-west-2",
            environment.clone(),
            "cnctr-ZZZZZZZZZZ".to_string(),
            RESOURCE_TYPE_CONNECTOR,
            "cnctr-ZZZZZZZZZZ".to_string(),
        ),
        (
            "a never-minted connector id, mixed".to_string(),
            "us-west-2",
            environment.clone(),
            "cnctr-0A1b2C3d4E".to_string(),
            RESOURCE_TYPE_CONNECTOR,
            "cnctr-0A1b2C3d4E".to_string(),
        ),
        (
            "another environment's connector".to_string(),
            "us-west-2",
            environment.clone(),
            other_connector.clone(),
            RESOURCE_TYPE_CONNECTOR,
            other_connector.clone(),
        ),
        (
            "an environment this service holds no record of".to_string(),
            "us-west-2",
            absent_environment.to_string(),
            connector.clone(),
            RESOURCE_TYPE_ENVIRONMENT,
            absent_environment.to_string(),
        ),
        (
            // A regional resource: an environment exists at the endpoint of the Region
            // it was created in, so from another Region it is no environment at all.
            "the environment and its connector from another Region".to_string(),
            "eu-west-1",
            environment.clone(),
            connector.clone(),
            RESOURCE_TYPE_ENVIRONMENT,
            environment.clone(),
        ),
    ];

    for (case, region, environment_id, connector_id, resource_type, resource_id) in &rows {
        let (status, body) = call_in_region(
            &router,
            region,
            "DeleteEnvironmentConnector",
            &delete_request(environment_id, connector_id),
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
            body["resourceType"],
            json!(resource_type),
            "case {case:?}: unexpected resourceType, got {body}"
        );
        assert_eq!(
            body["resourceId"],
            json!(resource_id),
            "case {case:?}: resourceId must echo the resource the request named, got {body}"
        );
        assert!(
            body["connector"].is_null() && body["environmentSummary"].is_null(),
            "case {case:?}: a refusal reports no record, got {body}"
        );
    }

    // Nothing was written for any of the refused requests: both environments still hold
    // their own connector, live and untouched.
    for (case, environment_id, connector_id) in [
        ("the environment under test", &environment, &connector),
        ("another environment", &other_environment, &other_connector),
    ] {
        let listed = listed_connectors(&router, case, environment_id).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected exactly one connector record, got {listed:?}"
        );
        assert_member(case, &listed[0], "connectorId", &json!(connector_id));
        let state = listed[0]["state"].as_str().unwrap_or_default();
        assert!(
            state == "CREATING" || state == "ACTIVE",
            "case {case:?}: the connector must still be live, got {state:?}"
        );
    }
}

#[tokio::test]
async fn the_connector_records_an_environment_keeps_are_bounded() {
    // The records a deletion is finished with are kept so a later read still reports the
    // deletion, but a create/delete loop must not grow the store without end: at the cap
    // the longest-deleted record makes room, and a live record is never dropped.
    let router = emulator();
    let case = "a store past the retained-connector cap";
    let cap = evs::state::MAX_CONNECTORS_PER_ENVIRONMENT;
    let environment = create_settled(&router, None).await;
    let live = create_connector(
        &router,
        case,
        &environment,
        VCENTER,
        "vcenter.evs.example.com",
        SECRET_ARN,
    )
    .await;
    let live_id = live["connectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no connectorId in {live}"))
        .to_string();

    // Past the cap by ten finished records, stamped in a known order so the ones that
    // make room are identifiable. Written through the store because reaching this many
    // deletions through the API would take one connector create and delete per record.
    let overshoot = 10;
    let finished: Vec<String> = (0..cap + overshoot)
        .map(|ordinal| format!("cnctr-{ordinal:010}"))
        .collect();
    {
        let store = evs::state::service_state(&*router.engine)
            .expect("the in-memory engine carries this service's store");
        let mut store = store.lock();
        let stored = store
            .environments
            .get_mut(&environment)
            .expect("the store holds the environment under test");
        for (ordinal, connector_id) in finished.iter().enumerate() {
            let mut connector = evs::state::StoredConnector::creating(
                connector_id.clone(),
                VCENTER,
                "retired.evs.example.com",
                SECRET_ARN.to_string(),
                ordinal as i64,
                evs::state::PendingConnectorState::starting_now(evs::state::CONNECTOR_DELETED),
            );
            connector.state = evs::state::CONNECTOR_DELETED.to_string();
            connector.modified_at_ms = ordinal as i64;
            connector.pending = None;
            stored.connectors.insert(connector_id.clone(), connector);
        }
    }

    let (status, body) = call(
        &router,
        "DeleteEnvironmentConnector",
        &delete_request(&environment, &live_id),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the delete expected 200, got {status} {body}"
    );

    let listed = listed_connectors(&router, case, &environment).await;
    assert!(
        listed.len() <= cap,
        "case {case:?}: an environment keeps at most {cap} connector records, got {}",
        listed.len()
    );
    let kept: Vec<&str> = listed
        .iter()
        .map(|connector| connector["connectorId"].as_str().unwrap_or_default())
        .collect();
    // The live record is never the one that makes room, and the records that made room
    // are the longest-deleted ones.
    assert!(
        kept.contains(&live_id.as_str()),
        "case {case:?}: the connector under deletion must be kept, got {kept:?}"
    );
    for purged in finished.iter().take(overshoot + 1) {
        assert!(
            !kept.contains(&purged.as_str()),
            "case {case:?}: the longest-deleted record {purged} must have made room, got {kept:?}"
        );
    }
    for held in finished.iter().skip(overshoot + 1) {
        assert!(
            kept.contains(&held.as_str()),
            "case {case:?}: a record inside the cap must be kept, got {kept:?}"
        );
    }
}
