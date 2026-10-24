// SPDX-License-Identifier: CC0-1.0

//! A list addressed to an Amazon EVS environment that is STILL INITIALIZING.
//!
//! The rule under test: an operation that LISTS the resources of one environment is
//! refused with `ValidationException` — HTTP 400 — for exactly as long as that
//! environment is still initializing, and answered in every other state.
//!
//! "The input fails to satisfy the specified constraints. You will see this exception
//! if invalid inputs are provided for any of the Amazon EVS environment operations, or
//! if a list operation is performed on an environment resource that is still
//! initializing." (`ValidationException` doc, `smithy_query(query="shape",
//! shape="ValidationException")`.) An environment is still initializing while its
//! `EnvironmentState` is `CREATING`; the other four values the model declares —
//! `CREATED`, `DELETING`, `DELETED`, `CREATE_FAILED` (`smithy_query(query="shape",
//! shape="EnvironmentState")`) — are past initialization.
//!
//! The rule is about the CLASS of list operations addressed to an environment resource,
//! not about one of them, so every table here spans all four
//! ([`list_requests`]) — and the state table spans every published
//! `EnvironmentState`, including the ones no create produces on its own.
//!
//! The refusal is also all a refused list does: it is a read, so once the environment
//! has settled the SAME request reports the whole inventory, unchanged by having been
//! refused.
//!
//! The harness (`emulator`, `call`, `create`, `settle`) is the one
//! `environment_lifecycle` already owns, so every test drives the service the way a
//! client does: `POST /` with `X-Amz-Target: AmazonElasticVMwareService.<Operation>`.

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::entitlement_connector::connector_in;
use crate::environment_lifecycle::{call, create, emulator, error_type, settle, Emulator};

/// A well-formed `connectorId` no connector was ever minted at: it satisfies the
/// modelled `^(cnctr-[a-zA-Z0-9]{10})$`, which is all a row needs where the environment
/// is decided before the connector handle is resolved — the environment's state and its
/// absence are both answered first. A row that expects the list to be ANSWERED carries
/// a connector the environment really holds instead
/// (`tests/all/list_vm_entitlements_connector.rs` owns the connector-handle rule).
const UNKNOWN_CONNECTOR_ID: &str = "cnctr-0123456789";

/// The type of connector the `ListVmEntitlements` rows are listed through — any
/// modelled type resolves, so this picks the one an entitlement is created through.
const VCENTER: &str = "VCENTER";

/// The only `EntitlementType` the model declares
/// (`smithy_query(query="shape", shape="EntitlementType")`).
const WINDOWS_SERVER: &str = "WINDOWS_SERVER";

/// The `EnvironmentState` values the model declares
/// (`smithy_query(query="shape", shape="EnvironmentState")`), each paired with whether
/// an environment in that state is STILL INITIALIZING — `CREATING` and nothing else.
const ENVIRONMENT_STATES: [(&str, bool); 5] = [
    ("CREATING", true),
    ("CREATED", false),
    // `DELETED` answers a list as well: this rule is about the INITIALIZING state only,
    // and whether a `DELETED` environment should instead be reported absent ("it may
    // have a `state` of `DELETED`", `ResourceNotFoundException` doc) is a separate
    // question this suite deliberately does not decide — the handlers' lookup is
    // untouched here.
    ("DELETING", false),
    ("DELETED", false),
    ("CREATE_FAILED", false),
];

/// The number of VLAN subnets an environment is built with: `InitialVlans` declares ten
/// members, so a complete `ListEnvironmentVlans` answer reports ten.
const VLAN_COUNT: usize = 10;

/// Every LIST operation this service binds to one environment resource, with a request
/// that addresses `environment`, and the output member each one pages over.
///
/// These are the four operations whose input is an `environmentId` and whose output is a
/// page of that environment's resources
/// (`smithy_query(query="digest")`): the class the `ValidationException` doc's "a list
/// operation is performed on an environment resource" covers. `ListEnvironments` is not
/// among them — it addresses the account and Region, not one environment resource — and
/// neither is `ListTagsForResource`, which addresses a resource by ARN and declares no
/// `ValidationException` at all.
fn list_requests(environment: &str, connector_id: &str) -> Vec<(&'static str, Value, &'static str)> {
    vec![
        (
            "ListEnvironmentVlans",
            json!({ "environmentId": environment }),
            "environmentVlans",
        ),
        (
            "ListEnvironmentHosts",
            json!({ "environmentId": environment }),
            "environmentHosts",
        ),
        (
            "ListEnvironmentConnectors",
            json!({ "environmentId": environment }),
            "connectors",
        ),
        (
            "ListVmEntitlements",
            json!({
                "environmentId": environment,
                "connectorId": connector_id,
                "entitlementType": WINDOWS_SERVER,
            }),
            "entitlements",
        ),
    ]
}

/// Put one stored environment into `state` and drop the transition it owed, so a state
/// the lifecycle does not pass through on its own — or one it would leave while the test
/// runs — can be exercised too.
fn force_state(router: &Emulator, environment_id: &str, state: &str) {
    let store = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let mut store = store.lock();
    let environment = store
        .environments
        .get_mut(environment_id)
        .unwrap_or_else(|| panic!("the store holds the environment {environment_id}"));
    environment.state = state.to_string();
    environment.pending = None;
}

/// Assert that `body` is this service's refusal of a list against an initializing
/// environment: a `ValidationException` naming `environmentId`.
fn assert_initializing_refusal(case: &str, status: u16, body: &Value) {
    // Every modeled CLIENT error of this awsJson1_0 service goes on the wire as a 400 —
    // see `tests/all/error_wire_status.rs`, which owns that rule.
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_type(body).rsplit(['#', '/']).next().unwrap_or(""),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {}",
        error_type(body)
    );
    let field_names: Vec<&str> = body["fieldList"]
        .as_array()
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| field["name"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        field_names,
        vec!["environmentId"],
        "case {case:?}: the refusal must name the member that carried the environment, got {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("still initializing"),
        "case {case:?}: the refusal must say why the list was not performed, got {message:?}"
    );
}

#[tokio::test]
async fn every_list_of_an_environment_is_refused_while_it_is_initializing() {
    let router = emulator();
    // NOT settled: a freshly created environment is CREATING, which is the initializing
    // state.
    let id = create(&router, Some("initializing-env")).await;
    // A connector of that environment, so the `ListVmEntitlements` rows carry a handle
    // that resolves: the refusal under test must be the environment's state and not the
    // connector lookup. A connector can be created while the environment initializes.
    let connector = connector_in(&router, "initializing-env", &id, VCENTER).await;

    for (operation, request, _) in list_requests(&id, &connector) {
        let case = format!("{operation} while CREATING");
        let (status, body) = call(&router, operation, &request).await;
        assert_initializing_refusal(&case, status, &body);
    }

    // The same four requests, once the transition the clock owed the environment has
    // come due: the refusal is the initializing state's and not the operation's answer
    // to everything.
    settle(&router);
    for (operation, request, member) in list_requests(&id, &connector) {
        let case = format!("{operation} once CREATED");
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert!(
            body[member].is_array(),
            "case {case:?}: expected a page in {member}, got {body}"
        );
    }
}

#[tokio::test]
async fn a_list_is_refused_in_the_initializing_state_and_answered_in_every_other_one() {
    let router = emulator();
    let id = create(&router, Some("state-table-env")).await;
    let connector = connector_in(&router, "state-table-env", &id, VCENTER).await;
    // Settled first, so the state each row forces is the state the list sees: an
    // environment that still owes a transition would leave the forced state behind as
    // soon as that transition came due.
    settle(&router);

    for (state, initializing) in ENVIRONMENT_STATES {
        force_state(&router, &id, state);
        for (operation, request, member) in list_requests(&id, &connector) {
            let case = format!("{operation} against an environment in {state}");
            let (status, body) = call(&router, operation, &request).await;
            if initializing {
                assert_initializing_refusal(&case, status, &body);
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: {state} is not the initializing state, so the list is \
                     answered; got {status} {body}"
                );
                assert!(
                    body[member].is_array(),
                    "case {case:?}: expected a page in {member}, got {body}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_refused_list_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let id = create(&router, Some("refused-then-listed")).await;

    // A list refused while the environment initializes, repeated: a read consumes
    // nothing, so the second refusal is the same as the first.
    for attempt in 0..3 {
        let (status, body) = call(
            &router,
            "ListEnvironmentVlans",
            &json!({ "environmentId": id }),
        )
        .await;
        assert_initializing_refusal(&format!("attempt {attempt}"), status, &body);
    }

    // And once the environment has settled, the whole inventory is there: nothing the
    // refusals touched was spent.
    settle(&router);
    let (status, body) = call(
        &router,
        "ListEnvironmentVlans",
        &json!({ "environmentId": id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "after settling: expected 200, got {status} {body}"
    );
    let vlans = body["environmentVlans"]
        .as_array()
        .unwrap_or_else(|| panic!("after settling: no environmentVlans in {body}"));
    assert_eq!(
        vlans.len(),
        VLAN_COUNT,
        "after settling: the refused lists must not have consumed any VLAN, expected \
         {VLAN_COUNT}, got {}",
        vlans.len()
    );
}

#[tokio::test]
async fn an_absent_environment_is_still_reported_absent_rather_than_initializing() {
    // The refusal is about a resource this service HOLDS: an id that names nothing is
    // still the not-found answer, whichever order a reader might expect the two checks
    // in.
    let router = emulator();
    let absent = "env-0000000000";
    // One real, initializing environment, so the store is not empty.
    let _initializing = create(&router, Some("present-but-initializing")).await;

    for (operation, request, _) in list_requests(absent, UNKNOWN_CONNECTOR_ID) {
        let case = format!("{operation} against an absent environment");
        let (status, body) = call(&router, operation, &request).await;
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
    }
}
