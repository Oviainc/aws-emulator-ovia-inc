// SPDX-License-Identifier: CC0-1.0

//! `DeleteEnvironment` against an environment that still holds ESX hosts.
//!
//! The rule under test, from the operation's own documentation
//! (`smithy_query(query="doc",shape="DeleteEnvironment")`): "Amazon EVS environments
//! will only be enabled for deletion once the hosts are deleted. You can delete hosts
//! using the DeleteEnvironmentHost action." So a delete of an environment that still
//! holds any host that has not reached the terminal `DELETED` state is REFUSED with
//! this operation's modelled client error (`ValidationException`, HTTP 400 like every
//! client error of this awsJson1_0 service — see `tests/all/error_wire_status.rs`,
//! which owns that rule), and nothing about the environment changes: it keeps its
//! state, its `modifiedAt` and its hosts. Only an environment that holds no live host
//! is enabled for deletion.
//!
//! What the rule generalises over, and so what the tables below span: the NUMBER of
//! hosts an environment holds, the `HostState` each one is in, and the way a host stops
//! occupying the environment (its record reaching `DELETED`, or the record being gone).
//!
//! `DeleteEnvironmentHost` — the action the doc names — is still an unimplemented 501
//! stub (`issues/things-that-do-not-work/20261024T000435Z-evs-deleteenvironmenthost-stub.md`
//! owns it), so a test cannot remove a host over the wire yet. The host records are
//! therefore moved through the store this service's own handlers read and write, the
//! way [`crate::environment_lifecycle::register_network`] seeds the Amazon EC2 store;
//! everything else here is driven as a client drives it.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::environment_lifecycle::{call, create, emulator, error_type, Emulator};

/// Long enough for the transition this service owes a new environment or host to come
/// due (its span is under a second by construction).
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// Every `HostState` the model declares, source:
/// `smithy_query(query="shape", shape="HostState")` —
/// `CREATING | CREATED | UPDATING | DELETING | DELETED | CREATE_FAILED | UPDATE_FAILED`.
const EVERY_HOST_STATE: [&str; 7] = [
    "CREATING",
    "CREATED",
    "UPDATING",
    "DELETING",
    "DELETED",
    "CREATE_FAILED",
    "UPDATE_FAILED",
];

/// Add `count` hosts to `environment` over the wire, named after `prefix`, and return
/// their host names. Host DNS names are unique across environments, so the prefix is
/// what keeps two environments of one test apart.
async fn add_hosts(router: &Emulator, environment: &str, prefix: &str, count: usize) -> Vec<String> {
    let mut names = Vec::new();
    for index in 0..count {
        let host_name = format!("{prefix}-{index}");
        let (status, body) = call(
            router,
            "CreateEnvironmentHost",
            &json!({
                "environmentId": environment,
                "host": {
                    "hostName": host_name,
                    "keyName": "evs-key",
                    "instanceType": "i4i.metal",
                },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "add host {host_name}: expected HTTP 200, got {status} ({}) {body}",
            error_type(&body)
        );
        names.push(host_name);
    }
    names
}

/// Put the stored host record into `host_state` and cancel the change the clock owed
/// it, so the state under test is the one the handler sees. The store is this service's
/// own ([`evs::state::service_state`]) — the same one every handler reads.
fn set_host_state(router: &Emulator, environment: &str, host_name: &str, host_state: &str) {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let mut store = state.lock();
    let stored = store
        .environments
        .get_mut(environment)
        .unwrap_or_else(|| panic!("the store holds environment {environment}"))
        .hosts
        .get_mut(host_name)
        .unwrap_or_else(|| panic!("environment {environment} holds host {host_name}"));
    stored.state = host_state.to_string();
    stored.pending = None;
}

/// Drop the stored host record outright — the other way a host stops occupying its
/// environment.
fn forget_host(router: &Emulator, environment: &str, host_name: &str) {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let mut store = state.lock();
    let hosts = &mut store
        .environments
        .get_mut(environment)
        .unwrap_or_else(|| panic!("the store holds environment {environment}"))
        .hosts;
    assert!(
        hosts.remove(host_name).is_some(),
        "environment {environment} holds host {host_name}"
    );
}

/// `GetEnvironment`'s view of one environment.
async fn get_environment(router: &Emulator, environment: &str) -> Value {
    let (status, body) = call(
        router,
        "GetEnvironment",
        &json!({ "environmentId": environment }),
    )
    .await;
    assert_eq!(
        status, 200,
        "get {environment}: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    body["environment"].clone()
}

/// The host names `ListEnvironmentHosts` reports for one environment.
async fn listed_hosts(router: &Emulator, environment: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListEnvironmentHosts",
        &json!({ "environmentId": environment }),
    )
    .await;
    assert_eq!(
        status, 200,
        "list hosts of {environment}: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    body["environmentHosts"]
        .as_array()
        .unwrap_or_else(|| panic!("list hosts of {environment}: no environmentHosts in {body}"))
        .iter()
        .filter_map(|host| host["hostName"].as_str().map(str::to_string))
        .collect()
}

/// Assert that a refused delete is the operation's modelled client error, naming the
/// member that carried the environment.
fn assert_refused(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} ({}) {body}",
        error_type(body)
    );
    assert!(
        error_type(body).contains("ValidationException"),
        "case {case:?}: expected a ValidationException, got {} in {body}",
        error_type(body)
    );
    assert_eq!(
        body["reason"],
        json!("fieldValidationFailed"),
        "case {case:?}: expected reason fieldValidationFailed, got {}",
        body["reason"]
    );
    let named: Vec<&str> = body["fieldList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no fieldList in {body}"))
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert_eq!(
        named,
        vec!["environmentId"],
        "case {case:?}: the refusal must name the member that carried the environment, \
         got {named:?} in {body}"
    );
    assert_eq!(
        body["environment"],
        Value::Null,
        "case {case:?}: a refused delete must report no environment, got {}",
        body["environment"]
    );
}

#[tokio::test]
async fn a_host_in_any_state_but_deleted_keeps_its_environment_from_being_deleted() {
    let router = emulator();

    // One environment per declared `HostState`, each holding a single host that is put
    // into that state. Only the terminal DELETED state means the host is gone, so every
    // other state must keep the environment from being deleted.
    let mut environments: Vec<(&str, String)> = Vec::new();
    for host_state in EVERY_HOST_STATE {
        let environment = create(&router, Some(&format!("state-{host_state}"))).await;
        environments.push((host_state, environment));
    }
    // One wait for every environment: the deployment the clock owes them is due at the
    // same span, and a host can only be added to a deployed environment.
    std::thread::sleep(PAST_THE_TRANSITION);

    for (host_state, environment) in &environments {
        // Host DNS names match `([a-zA-Z0-9\-]*)`, so the state's underscore cannot go
        // into one.
        let prefix = format!("esx-{}", host_state.to_lowercase().replace('_', "-"));
        let hosts = add_hosts(&router, environment, &prefix, 1).await;
        set_host_state(&router, environment, &hosts[0], host_state);
    }

    for (host_state, environment) in &environments {
        let case = format!("one host in {host_state}");
        let before = get_environment(&router, environment).await;
        let (status, body) = call(
            &router,
            "DeleteEnvironment",
            &json!({ "environmentId": environment }),
        )
        .await;
        if *host_state == "DELETED" {
            assert_eq!(
                status, 200,
                "case {case:?}: a deleted host no longer occupies the environment, so the \
                 delete is accepted — got {status} ({}) {body}",
                error_type(&body)
            );
            assert_eq!(
                body["environment"]["environmentState"],
                json!("DELETING"),
                "case {case:?}: expected DELETING, got {}",
                body["environment"]["environmentState"]
            );
            continue;
        }
        assert_refused(&case, status, &body);
        // Nothing about the refused environment changed.
        let after = get_environment(&router, environment).await;
        assert_eq!(
            after["environmentState"],
            json!("CREATED"),
            "case {case:?}: a refused delete must leave the environment CREATED, got {}",
            after["environmentState"]
        );
        assert_eq!(
            after["modifiedAt"].as_f64(),
            before["modifiedAt"].as_f64(),
            "case {case:?}: a refused delete must not move modifiedAt, got {} from {}",
            after["modifiedAt"],
            before["modifiedAt"]
        );
    }
}

#[tokio::test]
async fn the_host_count_decides_the_delete_and_the_refusal_reports_it() {
    let router = emulator();

    // Each row is a host count, which is what the rule generalises over: none, one, and
    // several — up to a count greater than one so the refusal's plural case is covered.
    let counts: [usize; 4] = [0, 1, 2, 4];
    let mut environments: Vec<(usize, String)> = Vec::new();
    for count in counts {
        let environment = create(&router, Some(&format!("count-{count}"))).await;
        environments.push((count, environment));
    }
    std::thread::sleep(PAST_THE_TRANSITION);

    for (count, environment) in &environments {
        add_hosts(&router, environment, &format!("esx-count-{count}"), *count).await;
    }

    for (count, environment) in &environments {
        let case = format!("{count} host(s)");
        let (status, body) = call(
            &router,
            "DeleteEnvironment",
            &json!({ "environmentId": environment, "clientToken": "delete-1" }),
        )
        .await;
        if *count == 0 {
            assert_eq!(
                status, 200,
                "case {case:?}: an environment with no host is enabled for deletion — got \
                 {status} ({}) {body}",
                error_type(&body)
            );
            assert_eq!(
                body["environment"]["environmentState"],
                json!("DELETING"),
                "case {case:?}: expected DELETING, got {}",
                body["environment"]["environmentState"]
            );
            continue;
        }
        assert_refused(&case, status, &body);
        // The refusal is built from the request and the store: it names the environment
        // the caller asked for and how many hosts are still in the way.
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&count.to_string()) && message.contains("DeleteEnvironmentHost"),
            "case {case:?}: the refusal must report the hosts still in the way and the \
             action that removes them, got {message:?}"
        );
        // The hosts are still there: a refused delete drops nothing.
        let listed = listed_hosts(&router, environment).await;
        assert_eq!(
            listed.len(),
            *count,
            "case {case:?}: a refused delete must keep every host, got {listed:?}"
        );
        // And a repeat is refused the same way, rather than succeeding second time.
        let (status, body) = call(
            &router,
            "DeleteEnvironment",
            &json!({ "environmentId": environment, "clientToken": "delete-2" }),
        )
        .await;
        assert_refused(&format!("{case}, repeated"), status, &body);
    }
}

#[tokio::test]
async fn an_environment_is_deletable_once_every_host_is_gone() {
    let router = emulator();
    let environment = create(&router, Some("hosts-then-delete")).await;
    std::thread::sleep(PAST_THE_TRANSITION);
    let hosts = add_hosts(&router, &environment, "esx-gone", 3).await;

    // Each row removes one more host, in one of the two ways a host stops occupying its
    // environment: its record reaching the terminal DELETED state, or the record being
    // gone. Until the last one goes, the delete stays refused.
    let (status, body) = call(
        &router,
        "DeleteEnvironment",
        &json!({ "environmentId": environment }),
    )
    .await;
    assert_refused("three hosts", status, &body);

    set_host_state(&router, &environment, &hosts[0], "DELETED");
    let (status, body) = call(
        &router,
        "DeleteEnvironment",
        &json!({ "environmentId": environment }),
    )
    .await;
    assert_refused("one host DELETED, two left", status, &body);

    forget_host(&router, &environment, &hosts[1]);
    let (status, body) = call(
        &router,
        "DeleteEnvironment",
        &json!({ "environmentId": environment }),
    )
    .await;
    assert_refused("one host DELETED, one gone, one left", status, &body);

    // The last host goes, and now the environment is enabled for deletion.
    set_host_state(&router, &environment, &hosts[2], "DELETED");
    let (status, body) = call(
        &router,
        "DeleteEnvironment",
        &json!({ "environmentId": environment }),
    )
    .await;
    assert_eq!(
        status, 200,
        "every host gone: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    assert_eq!(
        body["environment"]["environmentState"],
        json!("DELETING"),
        "every host gone: expected DELETING, got {}",
        body["environment"]["environmentState"]
    );
    // The VLAN subnets go with the environment ("Environment deletion also deletes the
    // associated Amazon EVS VLAN subnets").
    let (status, vlans) = call(
        &router,
        "ListEnvironmentVlans",
        &json!({ "environmentId": environment }),
    )
    .await;
    assert_eq!(status, 200, "list vlans: got {status} {vlans}");
    for vlan in vlans["environmentVlans"]
        .as_array()
        .unwrap_or_else(|| panic!("list vlans: no environmentVlans in {vlans}"))
    {
        assert_eq!(
            vlan["vlanState"],
            json!("DELETING"),
            "every host gone: the environment's VLANs go with it, got {}",
            vlan["vlanState"]
        );
    }

    // The move to DELETED the clock owes it then comes due.
    std::thread::sleep(PAST_THE_TRANSITION);
    let after = get_environment(&router, &environment).await;
    assert_eq!(
        after["environmentState"],
        json!("DELETED"),
        "past the transition: expected DELETED, got {}",
        after["environmentState"]
    );
}
