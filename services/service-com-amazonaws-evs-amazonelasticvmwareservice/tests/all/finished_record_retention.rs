// SPDX-License-Identifier: CC0-1.0

//! The retention rule this service applies to every record kind it KEEPS after it is
//! finished with, driven through the operations that apply it.
//!
//! THE RULE, stated once: while a record map holds more than the cap its kind declares,
//! the record that has been FINISHED with longest is dropped to bring the map back
//! inside the cap — ties going to the lower key — and a record that is still live is
//! never dropped, so a create/delete loop cannot grow the store without end while a
//! caller's own records survive. A read of a purged record then answers
//! `ResourceNotFoundException`, which that error's own doc sanctions ("The resource
//! might not be specified correctly, or it may have a `state` of `DELETED`").
//!
//! Three record kinds are subject to it, and one operation per kind applies it:
//! `DeleteEnvironmentHost` to an environment's host map (capped by
//! `MAX_HOSTS_PER_ENVIRONMENT`), `DeleteEnvironmentConnector` to its connector map
//! (`MAX_CONNECTORS_PER_ENVIRONMENT`), and `CreateEnvironment` to the environment store
//! (`MAX_ENVIRONMENTS`), which asks the rule for the room the new record needs.
//!
//! What the rule generalises over, and so what the cases below span: the record kind and
//! its cap, the operation that applies the rule, how far past the cap the map is, and
//! whether the finished records carry distinct finish stamps — the host case, where the
//! key order is the REVERSE of the finish order, so a selection by key would drop the
//! wrong records — or one shared stamp, the connector case, where the tie-break on the
//! key is what decides.
//!
//! The finished records are written through the store, because reaching a cap of this
//! size over the wire would take one create and one delete per record; every assertion
//! then reads the store back through the service's own list operations, the way a client
//! does.
//!
//! The harness (`emulator`, `call`, `create_settled`, `settle`) is the one
//! `environment_lifecycle` already owns.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::connector_health_checks::create_connector;
use crate::environment_lifecycle::{call, create_settled, emulator, settle, Emulator, REGION};

/// The `ConnectorType` the connector cases use, one value of the modelled catalog.
const VCENTER: &str = "VCENTER";

/// A secret given as a full Secrets Manager ARN, one form `secretIdentifier` accepts.
const SECRET_ARN: &str = "arn:aws:secretsmanager:us-west-2:000000000000:secret:evs/vcf-AbCd12";

/// The keys one paginated list operation of this service reports, every page followed —
/// the whole population the store still holds for that record kind.
async fn listed_keys(
    router: &Emulator,
    case: &str,
    operation: &str,
    request: &Value,
    list_member: &str,
    key_member: &str,
) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut page_request = request.clone();
        if let Some(token) = &next_token {
            page_request["nextToken"] = json!(token);
        }
        let (status, body) = call(router, operation, &page_request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} expected HTTP 200, got {status} {body}"
        );
        let page = body[list_member]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no {list_member} in {body}"))
            .clone();
        for record in &page {
            let key = record[key_member]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no {key_member} in {record}"))
                .to_string();
            keys.insert(key);
        }
        match body["nextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    keys
}

/// THE rule, asserted against the keys the store reports after the operation under test
/// applied it: the records that made room are exactly the `purged` records that had been
/// finished with longest, every other finished record is still kept, and no live record
/// was dropped.
fn assert_retention_rule(
    case: &str,
    finished_in_purge_order: &[String],
    purged: usize,
    live: &[&str],
    kept: &BTreeSet<String>,
) {
    for key in &finished_in_purge_order[..purged] {
        assert!(
            !kept.contains(key),
            "case {case:?}: the record {key} finished with longest must have made room, \
             got {} records kept including it",
            kept.len()
        );
    }
    for key in &finished_in_purge_order[purged..] {
        assert!(
            kept.contains(key),
            "case {case:?}: the finished record {key} is inside the cap and must be kept, \
             got {} records kept without it",
            kept.len()
        );
    }
    for key in live {
        assert!(
            kept.contains(*key),
            "case {case:?}: the live record {key} must never be dropped, got {kept:?}"
        );
    }
    let expected = live.len() + finished_in_purge_order.len() - purged;
    assert_eq!(
        kept.len(),
        expected,
        "case {case:?}: expected {expected} records kept, got {}",
        kept.len()
    );
}

#[tokio::test]
async fn deleting_a_host_purges_the_host_records_finished_with_longest() {
    let case = "a host map ten records past its cap, keyed against the finish order";
    let cap = evs::state::MAX_HOSTS_PER_ENVIRONMENT;
    let overshoot = 10;
    let router = emulator();
    let environment = create_settled(&router, Some("host-retention")).await;

    let live_host = "esx-live";
    let (status, body) = call(
        &router,
        "CreateEnvironmentHost",
        &json!({
            "environmentId": &environment,
            "host": {
                "hostName": live_host,
                "keyName": "evs-key",
                "instanceType": "i4i.metal",
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: adding the host expected HTTP 200, got {status} {body}"
    );
    settle(&router);

    // The finished records in the order the rule must purge them, longest-finished
    // first: their finish stamps ASCEND with that order while their keys DESCEND, so a
    // rule that selected by key would drop the wrong records. Every name satisfies the
    // modelled `HostName` pattern `^([a-zA-Z0-9\-]*)$`.
    let finished: Vec<String> = (0..cap + overshoot)
        .map(|ordinal| format!("retired-{:010}", cap + overshoot - 1 - ordinal))
        .collect();
    {
        let store = evs::state::service_state(&*router.engine)
            .expect("the in-memory engine carries this service's store");
        let mut store = store.lock();
        let stored = store
            .environments
            .get_mut(&environment)
            .expect("the store holds the environment under test");
        let template = stored
            .hosts
            .get(live_host)
            .expect("the store holds the host the create added")
            .clone();
        for (ordinal, host_name) in finished.iter().enumerate() {
            let mut host = template.clone();
            host.host_name = host_name.clone();
            host.state = evs::state::HOST_DELETED.to_string();
            host.modified_at_ms = ordinal as i64 + 1;
            host.pending = None;
            // The EC2 instance of a host that is gone is gone with it, so these records
            // name none.
            host.ec2 = None;
            stored.hosts.insert(host_name.clone(), host);
        }
    }

    // The delete is what applies the rule — and leaves the host it names DELETING, so
    // that record is live and the rule may not drop it.
    let (status, body) = call(
        &router,
        "DeleteEnvironmentHost",
        &json!({ "environmentId": &environment, "hostName": live_host }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the delete expected HTTP 200, got {status} {body}"
    );

    let kept = listed_keys(
        &router,
        case,
        "ListEnvironmentHosts",
        &json!({ "environmentId": &environment }),
        "environmentHosts",
        "hostName",
    )
    .await;
    // One live record beside the finished ones, trimmed back to the cap.
    let purged = (finished.len() + 1) - cap;
    assert_retention_rule(case, &finished, purged, &[live_host], &kept);
}

#[tokio::test]
async fn deleting_a_connector_purges_the_connector_records_finished_with_longest() {
    let case = "a connector map five records past its cap, every finish stamp equal";
    let cap = evs::state::MAX_CONNECTORS_PER_ENVIRONMENT;
    let overshoot = 5;
    let router = emulator();
    let environment = create_settled(&router, Some("connector-retention")).await;
    let live = create_connector(
        &router,
        case,
        &environment,
        VCENTER,
        "vcenter.evs.example.com",
        SECRET_ARN,
    )
    .await;
    let live_connector = live["connectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no connectorId in {live}"))
        .to_string();

    // Every finished record carries ONE finish stamp, so nothing but the tie-break
    // decides: the records that make room are the lowest keys, which is the order this
    // vector is in. Every id satisfies the modelled `ConnectorId` pattern
    // `^(cnctr-[a-zA-Z0-9]{10})$`.
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
        let template = stored
            .connectors
            .get(&live_connector)
            .expect("the store holds the connector the create added")
            .clone();
        for connector_id in &finished {
            let mut connector = template.clone();
            connector.connector_id = connector_id.clone();
            connector.appliance_fqdn = "retired.evs.example.com".to_string();
            connector.state = evs::state::CONNECTOR_DELETED.to_string();
            connector.modified_at_ms = 1;
            connector.pending = None;
            stored.connectors.insert(connector_id.clone(), connector);
        }
    }

    let (status, body) = call(
        &router,
        "DeleteEnvironmentConnector",
        &json!({ "environmentId": &environment, "connectorId": &live_connector }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the delete expected HTTP 200, got {status} {body}"
    );

    let kept = listed_keys(
        &router,
        case,
        "ListEnvironmentConnectors",
        &json!({ "environmentId": &environment }),
        "connectors",
        "connectorId",
    )
    .await;
    let purged = (finished.len() + 1) - cap;
    assert_retention_rule(case, &finished, purged, &[&live_connector], &kept);
}

#[tokio::test]
async fn creating_an_environment_purges_the_environment_record_finished_with_longest() {
    let case = "an environment store at its cap, one record's room to reclaim";
    let cap = evs::state::MAX_ENVIRONMENTS;
    let router = emulator();
    let live_environment = create_settled(&router, Some("environment-retention")).await;

    // The store is filled to the cap with finished records, stamped so the purge order
    // is this vector's order. Every id satisfies the modelled `EnvironmentId` pattern
    // `^(env-[a-zA-Z0-9]{10})$`.
    let finished: Vec<String> = (0..cap - 1)
        .map(|ordinal| format!("env-d{ordinal:09}"))
        .collect();
    {
        let store = evs::state::service_state(&*router.engine)
            .expect("the in-memory engine carries this service's store");
        let mut store = store.lock();
        let template = store
            .environments
            .get(&live_environment)
            .expect("the store holds the environment the create added")
            .clone();
        for (ordinal, environment_id) in finished.iter().enumerate() {
            let mut environment = template.clone();
            environment.environment_id = environment_id.clone();
            environment.environment_arn = evs::state::environment_arn(REGION, environment_id);
            environment.environment_name = Some(format!("retired-{ordinal}"));
            environment.state = evs::state::STATE_DELETED.to_string();
            environment.modified_at_ms = ordinal as i64 + 1;
            environment.pending = None;
            // A record that is gone holds no VPC, no license keys and no resources of
            // its own, so none of them is in the way of the create below.
            environment.vpc_id = format!("vpc-{ordinal:08}");
            environment.vcf_hostnames = None;
            environment.license_info.clear();
            environment.vlans.clear();
            environment.checks.clear();
            environment.impaired_since_ms.clear();
            environment.connectors.clear();
            environment.hosts.clear();
            environment.entitlements.clear();
            store
                .environments
                .insert(environment_id.clone(), environment);
        }
    }

    // The create is what applies the rule here: at the cap it asks for the room ONE more
    // record needs, so exactly one finished record makes way and the create is served
    // rather than refused.
    let (status, body) = call(&router, "CreateEnvironment", &router.create_request(None)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the create expected HTTP 200, got {status} {body}"
    );
    let created = body["environment"]["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no environmentId in {body}"))
        .to_string();

    let kept = listed_keys(
        &router,
        case,
        "ListEnvironments",
        &json!({}),
        "environmentSummaries",
        "environmentId",
    )
    .await;
    assert_retention_rule(case, &finished, 1, &[&live_environment, &created], &kept);

    // A read of the purged record answers the not-found error its retention rule names,
    // while the record that was finished with next-longest still reports its deletion.
    let (status, body) = call(
        &router,
        "GetEnvironment",
        &json!({ "environmentId": &finished[0] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: a get of the purged record expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        body["__type"]
            .as_str()
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default(),
        "ResourceNotFoundException",
        "case {case:?}: a get of the purged record expected a ResourceNotFoundException, \
         got {body}"
    );
    let (status, body) = call(
        &router,
        "GetEnvironment",
        &json!({ "environmentId": &finished[1] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a get of a kept record expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        body["environment"]["environmentState"],
        json!(evs::state::STATE_DELETED),
        "case {case:?}: a kept record still reports its deletion, got {body}"
    );
}
