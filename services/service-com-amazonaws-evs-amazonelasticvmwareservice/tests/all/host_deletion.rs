// SPDX-License-Identifier: CC0-1.0

//! `DeleteEnvironmentHost`: removing one ESX host from a stored Amazon EVS environment.
//!
//! The rule under test, from the operation's own model
//! (`smithy_query(query="operation",operation="DeleteEnvironmentHost")` and
//! `smithy_query(query="doc",shape="DeleteEnvironmentHostRequest",member="hostName")`):
//! the operation deletes the host the request NAMES from the environment it names, so
//! that host moves to `DELETING` and the clock then owes it the terminal `DELETED`
//! state; the record keeps reporting the deletion, every other host of the environment
//! is untouched, and the response reports the deleted host together with a summary of
//! the environment it was deleted from. A host that has reached `DELETED` no longer
//! occupies its environment, so its DNS hostname and its management address are free
//! for the next host. The operation is `@idempotent`: a host that is already going away
//! is reported as it stands. An `environmentId` this service holds no live environment
//! for, or a `hostName` that environment holds no record of, is refused with
//! `ResourceNotFoundException` — on HTTP 400, like every modelled client error of this
//! awsJson1_0 service (`tests/all/error_wire_status.rs` owns that rule) — and nothing
//! is written for it.
//!
//! What the rule generalises over, and so what the tables below span: the number of
//! hosts an environment holds, WHICH of them the request names (the first, a middle one,
//! the last, the only one), the state the named host is already in, the kind of resource
//! the request fails to name, and the `clientToken` the call carries (a parameter that
//! "is not used in Amazon EVS currently").
//!
//! An Amazon EVS host IS an EC2 bare metal instance ("An ESX host that runs on an Amazon
//! EC2 bare metal instance", `Host` doc), so the last test asserts the cross-service half
//! of the rule over the wire: the instance the host runs on is terminated in the
//! account's own Amazon EC2 store, the counterpart of the launch
//! `tests/all/host_ec2_instances.rs` pins for `CreateEnvironmentHost`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use crate::environment_lifecycle::{
    call, call_in_region, create, emulator, error_type, register_network, settle,
    vlans_with_prefix, Emulator, REGION,
};

/// Long enough for the transitions the two services owe a new or deleted host and its
/// instance to come due (each span is under a second by construction). Only the
/// cross-service test waits in wall-clock time; the rest settle the store instead.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The EC2 API version an ec2Query body carries.
const EC2_API_VERSION: &str = "2016-11-15";

/// A deployed environment with `count` hosts named `<prefix>-<index>`, every transition
/// the clock owed it settled: what a delete is driven against.
async fn deployed_with_hosts(
    router: &Emulator,
    name: &str,
    prefix: &str,
    count: usize,
) -> (String, Vec<String>) {
    let environment = create(router, Some(name)).await;
    settle(router);
    let mut hosts = Vec::new();
    for index in 0..count {
        let host_name = format!("{prefix}-{index}");
        let (status, body) = call(
            router,
            "CreateEnvironmentHost",
            &json!({
                "environmentId": &environment,
                "host": {
                    "hostName": &host_name,
                    "keyName": "evs-key",
                    "instanceType": "i4i.metal",
                },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: adding host {host_name} expected HTTP 200, got {status} ({}) {body}",
            error_type(&body)
        );
        hosts.push(host_name);
    }
    settle(router);
    (environment, hosts)
}

/// Every host of one environment, as `ListEnvironmentHosts` reports them, keyed by
/// `hostName`.
async fn listed_hosts(router: &Emulator, environment: &str) -> BTreeMap<String, Value> {
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
        .map(|host| {
            (
                host["hostName"]
                    .as_str()
                    .unwrap_or_else(|| panic!("every listed host carries a name, got {host}"))
                    .to_string(),
                host.clone(),
            )
        })
        .collect()
}

/// Delete one host and answer the call's status and body.
async fn delete_host(
    router: &Emulator,
    environment: &str,
    host_name: &str,
    client_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({ "environmentId": environment, "hostName": host_name });
    if let Some(client_token) = client_token {
        request["clientToken"] = json!(client_token);
    }
    call(router, "DeleteEnvironmentHost", &request).await
}

/// The `hostState` a listed or reported host carries.
fn host_state(host: &Value) -> String {
    host["hostState"]
        .as_str()
        .unwrap_or_else(|| panic!("a reported host carries a hostState, got {host}"))
        .to_string()
}

/// The `modifiedAt` a listed or reported host carries, as a comparable number.
fn modified_at(host: &Value, case: &str) -> f64 {
    host["modifiedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: a reported host carries modifiedAt, got {host}"))
}

#[tokio::test]
async fn the_named_host_goes_deleting_then_deleted_and_no_other_host_moves() {
    // Each row is a host population and WHICH host of it the request names: the only
    // one, the first of several, a middle one, and the last one.
    let rows: [(&str, usize, usize); 4] = [
        ("the only host", 1, 0),
        ("the first of two", 2, 0),
        ("a middle host of three", 3, 1),
        ("the last of four", 4, 3),
    ];

    for (case, count, target) in rows {
        let router = emulator();
        let (environment, hosts) = deployed_with_hosts(
            &router,
            &format!("del-{count}-{target}"),
            &format!("esx-d{count}{target}"),
            count,
        )
        .await;
        let before = listed_hosts(&router, &environment).await;
        let target_name = &hosts[target];
        let before_modified = modified_at(&before[target_name], case);

        let (status, body) = delete_host(&router, &environment, target_name, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
            error_type(&body)
        );
        // The response describes the host the request named, on its way out: the removal
        // takes a span, so the host a caller reads straight after the call is DELETING.
        assert_eq!(
            body["host"]["hostName"],
            json!(target_name),
            "case {case:?}: the response must describe the host the request named, got {}",
            body["host"]
        );
        assert_eq!(
            host_state(&body["host"]),
            "DELETING",
            "case {case:?}: expected DELETING, got {}",
            body["host"]["hostState"]
        );
        assert_ne!(
            modified_at(&body["host"], case),
            before_modified,
            "case {case:?}: the deletion must stamp modifiedAt, it still reads \
             {before_modified}"
        );
        assert!(
            modified_at(&body["host"], case)
                >= body["host"]["createdAt"].as_f64().unwrap_or_default(),
            "case {case:?}: a host is never modified before it was created, got {} and {}",
            modified_at(&body["host"], case),
            body["host"]["createdAt"]
        );
        // And a summary of the environment the host was deleted FROM, which deleting a
        // host of it does not modify.
        assert_eq!(
            body["environmentSummary"]["environmentId"],
            json!(environment),
            "case {case:?}: expected a summary of {environment}, got {}",
            body["environmentSummary"]
        );
        assert_eq!(
            body["environmentSummary"]["environmentState"],
            json!("CREATED"),
            "case {case:?}: deleting a host must not move the environment, got {}",
            body["environmentSummary"]["environmentState"]
        );

        // Every other host is exactly as it was, and the deleted record still reports
        // the deletion.
        let during = listed_hosts(&router, &environment).await;
        assert_eq!(
            during.len(),
            count,
            "case {case:?}: the deleted record is kept, so the list keeps every host, got {:?}",
            during.keys().collect::<Vec<_>>()
        );
        for (name, host) in &during {
            let expected = if name == target_name {
                "DELETING"
            } else {
                "CREATED"
            };
            assert_eq!(
                host_state(host),
                expected,
                "case {case:?}: host {name} expected {expected}, got {}",
                host["hostState"]
            );
            if name != target_name {
                assert_eq!(
                    modified_at(host, case),
                    modified_at(&before[name], case),
                    "case {case:?}: host {name} must not be touched by another host's delete"
                );
            }
        }

        // The move to the terminal DELETED state the clock owes the host then comes due.
        settle(&router);
        let after = listed_hosts(&router, &environment).await;
        for (name, host) in &after {
            let expected = if name == target_name {
                "DELETED"
            } else {
                "CREATED"
            };
            assert_eq!(
                host_state(host),
                expected,
                "case {case:?}: past the transition, host {name} expected {expected}, got {}",
                host["hostState"]
            );
        }
    }
}

#[tokio::test]
async fn a_repeated_delete_reports_the_host_as_it_stands_and_changes_nothing() {
    let router = emulator();
    let (environment, hosts) = deployed_with_hosts(&router, "del-idempotent", "esx-idem", 2).await;

    // Each row repeats the delete with a DIFFERENT client token, which "is not used in
    // Amazon EVS currently": what makes the call idempotent is the stored record.
    let (status, first) = delete_host(&router, &environment, &hosts[0], Some("token-1")).await;
    assert_eq!(
        status, 200,
        "the first delete: expected HTTP 200, got {status} ({}) {first}",
        error_type(&first)
    );
    let deleting_modified = modified_at(&first["host"], "the first delete");

    for token in ["token-2", "token-1", "ZZZ-3"] {
        let case = format!("repeat with {token}");
        let (status, body) = delete_host(&router, &environment, &hosts[0], Some(token)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
            error_type(&body)
        );
        assert_eq!(
            host_state(&body["host"]),
            "DELETING",
            "case {case:?}: a host already going away is reported as it stands, got {}",
            body["host"]["hostState"]
        );
        assert_eq!(
            modified_at(&body["host"], &case),
            deleting_modified,
            "case {case:?}: a repeated delete must not move modifiedAt"
        );
    }

    // Past the transition the host is DELETED, and a delete of it still answers with
    // that record rather than restarting anything.
    settle(&router);
    let settled = listed_hosts(&router, &environment).await;
    let settled_modified = modified_at(&settled[&hosts[0]], "settled");
    let (status, body) = delete_host(&router, &environment, &hosts[0], None).await;
    assert_eq!(
        status, 200,
        "the delete of a DELETED host: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    assert_eq!(
        host_state(&body["host"]),
        "DELETED",
        "a host that is already gone is reported as it stands, got {}",
        body["host"]["hostState"]
    );
    assert_eq!(
        modified_at(&body["host"], "the delete of a DELETED host"),
        settled_modified,
        "a delete of a DELETED host must not move modifiedAt"
    );
    // The other host never moved through any of it.
    let other = listed_hosts(&router, &environment).await;
    assert_eq!(
        host_state(&other[&hosts[1]]),
        "CREATED",
        "the environment's other host must stay CREATED, got {}",
        other[&hosts[1]]["hostState"]
    );
}

#[tokio::test]
async fn a_host_or_environment_this_service_does_not_hold_is_not_found() {
    let router = emulator();
    let (environment, hosts) = deployed_with_hosts(&router, "del-missing", "esx-miss", 2).await;
    // A second environment, so a request can name a host that exists but belongs
    // elsewhere.
    let (other, other_hosts) = deployed_with_hosts(&router, "del-other", "esx-other", 1).await;
    // And one that has been deleted outright, which `ResourceNotFoundException`'s own
    // doc names ("it may have a state of DELETED").
    let (gone, gone_hosts) = deployed_with_hosts(&router, "del-gone", "esx-gone", 1).await;
    let (status, body) = delete_host(&router, &gone, &gone_hosts[0], None).await;
    assert_eq!(status, 200, "emptying {gone}: got {status} {body}");
    settle(&router);
    let (status, body) =
        call(&router, "DeleteEnvironment", &json!({ "environmentId": &gone })).await;
    assert_eq!(status, 200, "deleting {gone}: got {status} {body}");
    settle(&router);

    // Each row is one request that names something the store does not hold, and the
    // resource type the refusal reports for it.
    let rows: Vec<(&str, String, String, &str, String)> = vec![
        (
            "an environment id this service holds no record of",
            "env-0000000000".to_string(),
            hosts[0].clone(),
            "environment",
            "env-0000000000".to_string(),
        ),
        (
            "an environment that has reached DELETED",
            gone.clone(),
            gone_hosts[0].clone(),
            "environment",
            gone.clone(),
        ),
        (
            "a host name this environment holds no record of",
            environment.clone(),
            "esx-absent".to_string(),
            "host",
            "esx-absent".to_string(),
        ),
        (
            "a host of ANOTHER environment",
            environment.clone(),
            other_hosts[0].clone(),
            "host",
            other_hosts[0].clone(),
        ),
        (
            "the empty host name the pattern admits",
            environment.clone(),
            String::new(),
            "host",
            String::new(),
        ),
    ];

    for (case, environment_id, host_name, resource_type, resource_id) in rows {
        let (status, body) = delete_host(&router, &environment_id, &host_name, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({}) {body}",
            error_type(&body)
        );
        assert!(
            error_type(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {} in {body}",
            error_type(&body)
        );
        assert_eq!(
            body["resourceType"],
            json!(resource_type),
            "case {case:?}: expected resourceType {resource_type:?}, got {}",
            body["resourceType"]
        );
        assert_eq!(
            body["resourceId"],
            json!(resource_id),
            "case {case:?}: the refusal names the resource the caller asked for, got {}",
            body["resourceId"]
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .contains(&resource_id),
            "case {case:?}: the message names the requested resource, got {}",
            body["message"]
        );
        assert_eq!(
            body["host"],
            Value::Null,
            "case {case:?}: a refused delete reports no host, got {}",
            body["host"]
        );
    }

    // Nothing was written for any of them: both live environments still hold every host
    // they held, all CREATED.
    for environment_id in [environment.as_str(), other.as_str()] {
        for (name, host) in listed_hosts(&router, environment_id).await {
            assert_eq!(
                host_state(&host),
                "CREATED",
                "a refused delete must leave host {name} of {environment_id} untouched, got {}",
                host["hostState"]
            );
        }
    }

    // The same request addressed to a Region this environment does not live in names no
    // environment either: an environment exists at the endpoint of the Region it was
    // created in.
    let (status, body) = call_in_region(
        &router,
        "us-east-1",
        "DeleteEnvironmentHost",
        &json!({ "environmentId": &environment, "hostName": &hosts[0] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "another Region: expected HTTP 400, got {status} ({}) {body}",
        error_type(&body)
    );
    assert_eq!(
        body["resourceType"],
        json!("environment"),
        "another Region: expected the environment to be the missing resource, got {body}"
    );
    assert_eq!(
        host_state(&listed_hosts(&router, &environment).await[&hosts[0]]),
        "CREATED",
        "a delete addressed to another Region must change nothing"
    );
}

#[tokio::test]
async fn a_deleted_host_frees_its_dns_hostname_and_its_management_address() {
    let router = emulator();
    let (environment, hosts) = deployed_with_hosts(&router, "del-reuse", "esx-reuse", 2).await;
    let before = listed_hosts(&router, &environment).await;
    let freed_address = before[&hosts[0]]["ipAddress"]
        .as_str()
        .unwrap_or_else(|| panic!("a host carries an ipAddress, got {}", before[&hosts[0]]))
        .to_string();

    let (status, body) = delete_host(&router, &environment, &hosts[0], None).await;
    assert_eq!(status, 200, "the delete: got {status} {body}");

    // While the host is still going away it OCCUPIES the environment, so its name is
    // still taken: "DNS hostnames for hosts must be unique across Amazon EVS
    // environments and within VCF".
    let (status, body) = call(
        &router,
        "CreateEnvironmentHost",
        &json!({
            "environmentId": &environment,
            "host": { "hostName": &hosts[0], "keyName": "evs-key", "instanceType": "i4i.metal" },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "while DELETING: the name is still taken, expected HTTP 400, got {status} ({}) {body}",
        error_type(&body)
    );

    // Once the host has reached DELETED it is gone from VCF, so the SAME name is
    // accepted again and the new host takes the address the old one left.
    settle(&router);
    let (status, body) = call(
        &router,
        "CreateEnvironmentHost",
        &json!({
            "environmentId": &environment,
            "host": { "hostName": &hosts[0], "keyName": "evs-key", "instanceType": "i4i.metal" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "after DELETED: the freed name is accepted, got {status} ({}) {body}",
        error_type(&body)
    );
    assert_eq!(
        body["host"]["ipAddress"],
        json!(freed_address),
        "the new host takes the management address the deleted host left, got {}",
        body["host"]["ipAddress"]
    );
    assert_eq!(
        host_state(&body["host"]),
        "CREATING",
        "a host added under a freed name is a NEW host, got {}",
        body["host"]["hostState"]
    );

    // And the environment now holds the new host under that name, not the deleted
    // record: one host per name.
    let after = listed_hosts(&router, &environment).await;
    assert_eq!(
        after.len(),
        2,
        "the environment holds one record per host name, got {:?}",
        after.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        host_state(&after[&hosts[0]]),
        "CREATING",
        "the record under the reused name is the new host, got {}",
        after[&hosts[0]]["hostState"]
    );
    assert_eq!(
        host_state(&after[&hosts[1]]),
        "CREATED",
        "the untouched host stays CREATED, got {}",
        after[&hosts[1]]["hostState"]
    );
}

/// One emulator serving BOTH services off one shared engine: Amazon EVS deletes the
/// host, Amazon EC2 is asked about the instance that host ran on.
struct CrossService {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl CrossService {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise behaviour, not signing.
        router.set_request_authentication(false);
        // An environment is created only for an account holding a Premium Support
        // subscription (`tests/all/premium_support_entitlement.rs` owns that rule).
        crate::premium_support_entitlement::grant_premium_support(&engine);
        router.register_service(
            service_com_amazonaws_evs_amazonelasticvmwareservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// A deployed environment in a VPC and subnet the account's own EC2 store holds.
    async fn deployed_environment(&self, name: &str) -> String {
        let (vpc_id, subnet_id) = {
            let ec2_state = ec2::state::service_state(&*self.engine)
                .expect("the in-memory engine carries the account's EC2 store");
            let mut store = ec2_state.lock();
            (store.next_id("vpc"), store.next_id("subnet"))
        };
        register_network(&self.engine, &vpc_id, &subnet_id);
        let (status, body) = call(
            &self.router,
            "CreateEnvironment",
            &json!({
                "vpcId": vpc_id,
                "serviceAccessSubnetId": subnet_id,
                "vcfVersion": "VCF-5.2.1",
                "termsAccepted": true,
                "environmentName": name,
                "initialVlans": vlans_with_prefix(28),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateEnvironment {name:?}: got {status} ({}) {body}",
            error_type(&body)
        );
        let environment = body["environment"]["environmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("no environmentId in {body}"))
            .to_string();
        std::thread::sleep(PAST_THE_TRANSITION);
        environment
    }

    /// Add one host and answer the `ec2InstanceId` it reports.
    async fn create_host(&self, environment: &str, host_name: &str) -> String {
        let (status, body) = call(
            &self.router,
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
            "CreateEnvironmentHost {host_name:?}: got {status} ({}) {body}",
            error_type(&body)
        );
        body["host"]["ec2InstanceId"]
            .as_str()
            .unwrap_or_else(|| panic!("a host reports the instance it runs on, got {body}"))
            .to_string()
    }

    /// The `instanceState/name` Amazon EC2 reports for one instance, over ec2Query.
    async fn instance_state(&self, instance_id: &str) -> String {
        let body = format!(
            "Action=DescribeInstances&Version={EC2_API_VERSION}&InstanceId.1={instance_id}"
        );
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("https://ec2.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            // The `ec2` signing name in the credential scope is what routes the request
            // to Amazon EC2 and selects the ec2Query envelope.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/ec2/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=0"
                ),
            )
            .body(::bytes::Bytes::from(body))
            .expect("a well-formed ec2Query request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let xml = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "DescribeInstances {instance_id}: expected 200, got {status} {xml}"
        );
        // `<instanceState><code>..</code><name>..</name></instanceState>` — the state
        // word of the one instance the describe is filtered to.
        let marker = "<instanceState>";
        let start = xml
            .find(marker)
            .unwrap_or_else(|| panic!("no instanceState for {instance_id} in {xml}"))
            + marker.len();
        let state = &xml[start..];
        let open = "<name>";
        let name_start = state
            .find(open)
            .unwrap_or_else(|| panic!("no state name for {instance_id} in {xml}"))
            + open.len();
        let name_end = state[name_start..]
            .find("</name>")
            .unwrap_or_else(|| panic!("unterminated state name for {instance_id} in {xml}"))
            + name_start;
        state[name_start..name_end].to_string()
    }
}

/// An Amazon EVS host IS an EC2 bare metal instance, so deleting the host terminates
/// that instance in the account's own Amazon EC2 store — and leaves every other host's
/// instance alone.
#[tokio::test]
async fn deleting_a_host_terminates_the_ec2_instance_it_ran_on() {
    let emulator = CrossService::new();
    let environment = emulator.deployed_environment("host-deletion-ec2").await;
    // Two hosts, so what the delete must NOT touch is observable beside what it must.
    let deleted_instance = emulator.create_host(&environment, "esx-term-1").await;
    let kept_instance = emulator.create_host(&environment, "esx-term-2").await;

    let (status, body) = call(
        &emulator.router,
        "DeleteEnvironmentHost",
        &json!({ "environmentId": &environment, "hostName": "esx-term-1" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the delete: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );

    // The instance is on its way out at once, on EC2's own span, and the other host's
    // instance is untouched.
    let state = emulator.instance_state(&deleted_instance).await;
    assert_eq!(
        state, "shutting-down",
        "the deleted host's instance must be terminating, got {state:?}"
    );
    let kept = emulator.instance_state(&kept_instance).await;
    assert!(
        kept == "pending" || kept == "running",
        "another host's instance must not be touched, got {kept:?}"
    );

    std::thread::sleep(PAST_THE_TRANSITION);
    let state = emulator.instance_state(&deleted_instance).await;
    assert_eq!(
        state, "terminated",
        "the deleted host's instance must reach terminated, got {state:?}"
    );
    let kept = emulator.instance_state(&kept_instance).await;
    assert_eq!(
        kept, "running",
        "another host's instance must still be running, got {kept:?}"
    );

    // A repeated delete terminates nothing a second time: the instance stays terminated.
    let (status, body) = call(
        &emulator.router,
        "DeleteEnvironmentHost",
        &json!({ "environmentId": &environment, "hostName": "esx-term-1" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the repeated delete: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let state = emulator.instance_state(&deleted_instance).await;
    assert_eq!(
        state, "terminated",
        "a repeated delete must leave the instance terminated, got {state:?}"
    );
}
