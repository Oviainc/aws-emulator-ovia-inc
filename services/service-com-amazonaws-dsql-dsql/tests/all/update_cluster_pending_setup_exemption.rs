// SPDX-License-Identifier: CC0-1.0

//! How far the one status-gate exemption `UpdateCluster` grants reaches.
//!
//! RULE: a status-gate exemption granted for ONE change covers only a request that
//! asks for nothing but that change. A cluster waiting in `PENDING_SETUP` takes the
//! multi-Region configuration call its peering is waiting for, so a request carrying
//! `multiRegionProperties` and nothing else is accepted; a request that also carries
//! another member that writes the cluster asks for an ordinary mutation of a cluster
//! whose status accepts none, so it is refused with the gate's 409
//! `ConflictException` and writes nothing. A cluster whose status accepts a mutation
//! needs no exemption, so it takes the same request in any combination.
//!
//! The table is the whole class rather than one request: every combination of the
//! members `UpdateClusterInput` carries that write the stored cluster
//! (`smithy_query(query="operation", operation="UpdateCluster")` —
//! `deletionProtectionEnabled`, `kmsEncryptionKey` and `multiRegionProperties`;
//! `identifier` addresses the cluster and `clientToken` only names the request), each
//! member driven with several values and two shapes of configuration.
//!
//! Every case goes over the wire through the `Dsql` harness in `tests/all/harness.rs`,
//! this crate's one test client.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::state::{
    STATUS_ACTIVE, STATUS_PENDING_SETUP, STATUS_UPDATING,
};

use crate::cluster_lifecycle::cluster_id_for;
use crate::harness::{expected_arn, wait_for_the_clock, Answer, Dsql};
use crate::multi_region_cluster_status::{FIRST_REGION, SECOND_REGION, WITNESS_REGION};

/// The `EncryptionType` a cluster that was never given a key of its own reports —
/// the fixture cluster's encryption, which a refused `kmsEncryptionKey` must leave
/// in place. Source: `smithy_query(query="shape", shape="EncryptionType")`
/// (`AWS_OWNED_KMS_KEY | CUSTOMER_MANAGED_KMS_KEY`).
const AWS_OWNED: &str = "AWS_OWNED_KMS_KEY";

/// The `kmsEncryptionKey` values the table is driven with: an alias, a bare key id,
/// a full key ARN, and the reserved value that asks for the Amazon-owned key the
/// fixture cluster is already on. Source: `smithy_query(query="doc",
/// shape="UpdateClusterInput", member="kmsEncryptionKey")` — "You can specify the
/// ARN, ID, or alias of an existing key ... To switch to the key owned by Amazon Web
/// Services, specify the reserved value AWS_OWNED_KMS_KEY".
fn key_values() -> Vec<String> {
    vec![
        "alias/other".to_string(),
        "1234abcd-12ab-34cd-56ef".to_string(),
        format!(
            "arn:aws:kms:{FIRST_REGION}:{}:key/9f0abc",
            engine::EMULATOR_ACCOUNT_ID
        ),
        AWS_OWNED.to_string(),
    ]
}

/// A peer ARN naming a cluster this store does not hold, as `ClusterArn`'s pattern
/// requires one.
fn peer_arn() -> String {
    expected_arn(SECOND_REGION, &cluster_id_for(777))
}

/// The shapes of `multiRegionProperties` the table sends: a witness Region alone and
/// a witness Region with a peer, so the exempted change is driven with more than one
/// configuration.
fn configurations() -> Vec<(&'static str, Value)> {
    vec![
        ("a witness Region", json!({"witnessRegion": WITNESS_REGION})),
        (
            "a witness Region and a peer",
            json!({"witnessRegion": WITNESS_REGION, "clusters": [peer_arn()]}),
        ),
    ]
}

/// One `UpdateCluster` request and whether a cluster waiting in `PENDING_SETUP`
/// takes it.
struct Case {
    name: String,
    request: Value,
    /// True only when the request asks for nothing but the multi-Region
    /// configuration — the change the wait is for.
    accepted: bool,
}

/// Every combination of the members that write a cluster, crossed with the values
/// each is driven with: no configuration or one of two, no `deletionProtectionEnabled`
/// or either value, no `kmsEncryptionKey` or one of four.
fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    let mut configuration_choices: Vec<(&'static str, Option<Value>)> = vec![("none", None)];
    for (name, configuration) in configurations() {
        configuration_choices.push((name, Some(configuration)));
    }
    let mut key_choices: Vec<Option<String>> = vec![None];
    key_choices.extend(key_values().into_iter().map(Some));

    for (configuration_name, configuration) in &configuration_choices {
        for protection in [None, Some(true), Some(false)] {
            for key in &key_choices {
                let mut request = serde_json::Map::new();
                if let Some(configuration) = configuration {
                    request.insert(
                        "multiRegionProperties".to_string(),
                        configuration.clone(),
                    );
                }
                if let Some(protection) = protection {
                    request.insert("deletionProtectionEnabled".to_string(), json!(protection));
                }
                if let Some(key) = key {
                    request.insert("kmsEncryptionKey".to_string(), json!(key));
                }
                cases.push(Case {
                    name: format!(
                        "multiRegionProperties: {configuration_name}, \
                         deletionProtectionEnabled: {protection:?}, kmsEncryptionKey: {key:?}"
                    ),
                    request: Value::Object(request),
                    accepted: configuration.is_some() && protection.is_none() && key.is_none(),
                });
            }
        }
    }

    // `clientToken` names the request rather than asking for a change, so it does
    // not take the exemption away from the configuration call.
    cases.push(Case {
        name: "the configuration alone, with a clientToken".to_string(),
        request: json!({
            "multiRegionProperties": {"witnessRegion": WITNESS_REGION},
            "clientToken": "exemption-scope-token",
        }),
        accepted: true,
    });
    cases
}

/// A cluster of `dsql` left waiting in `PENDING_SETUP`: created with a witness
/// Region, so its peering is established on one side only, with deletion protection
/// off and no key of its own.
async fn waiting_cluster(dsql: &Dsql) -> String {
    let created = dsql
        .create(json!({
            "deletionProtectionEnabled": false,
            "multiRegionProperties": {"witnessRegion": WITNESS_REGION},
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "the fixture cluster must be created, got {} {:?}",
        created.status, created.body
    );
    created.str("identifier")
}

/// The members a refused request must have left exactly as they were: the status,
/// deletion protection, the encryption the cluster carries and its configuration.
fn untouched_members(answer: &Answer) -> (String, Value, String, Option<String>, Value) {
    let details = answer.get("encryptionDetails");
    (
        answer.str("status"),
        answer.get("deletionProtectionEnabled"),
        details
            .get("encryptionType")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        details
            .get("kmsKeyArn")
            .and_then(Value::as_str)
            .map(str::to_string),
        answer.get("multiRegionProperties"),
    )
}

/// RULE: while a cluster waits in `PENDING_SETUP` the exemption covers the
/// configuration call alone — a request carrying any other member that writes the
/// cluster is refused with the gate's 409, whatever else it carries, and leaves the
/// record exactly as it was.
#[tokio::test]
async fn the_waiting_exemption_covers_the_configuration_call_alone() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let dsql = Dsql::sharing(&engine, FIRST_REGION);

    // A refused request writes nothing, so every refused case shares one fixture —
    // which the assertions below check is still untouched. The accepted cases each
    // move their cluster to `UPDATING`, so they get one of their own.
    let all = cases();
    let shared = waiting_cluster(&dsql).await;
    let mut accepted_fixtures = Vec::new();
    for _ in all.iter().filter(|case| case.accepted) {
        accepted_fixtures.push(waiting_cluster(&dsql).await);
    }
    wait_for_the_clock().await;

    let before = dsql.get(&shared).await;
    assert_eq!(
        before.str("status"),
        STATUS_PENDING_SETUP,
        "the fixture cluster must be waiting for its peering, got {:?}",
        before.body
    );
    let untouched = untouched_members(&before);

    let mut fixtures = accepted_fixtures.into_iter();
    for case in all {
        if case.accepted {
            let identifier = fixtures.next().expect("one fixture per accepted case");
            let accepted = dsql.update(&identifier, case.request.clone()).await;
            assert_eq!(
                (accepted.status, accepted.str("status")),
                (200, STATUS_UPDATING.to_string()),
                "case {:?}: the configuration call the wait is for must be accepted, got {} {:?}",
                case.name,
                accepted.status,
                accepted.body
            );
            // The accepted call wrote the configuration it carried, and nothing else.
            wait_for_the_clock().await;
            let after = dsql.get(&identifier).await;
            assert_eq!(
                after.get("multiRegionProperties"),
                case.request["multiRegionProperties"],
                "case {:?}: the accepted configuration must read back, got {:?}",
                case.name,
                after.get("multiRegionProperties")
            );
            assert_eq!(
                (
                    after.get("deletionProtectionEnabled"),
                    after
                        .get("encryptionDetails")
                        .get("encryptionType")
                        .cloned()
                        .unwrap_or(Value::Null)
                ),
                (json!(false), json!(AWS_OWNED)),
                "case {:?}: the accepted call must write nothing but the configuration, got {:?}",
                case.name,
                after.body
            );
            continue;
        }

        let refused = dsql.update(&shared, case.request.clone()).await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (409, Some("ConflictException")),
            "case {:?}: a request asking for more than the configuration call must be \
             refused with 409 ConflictException, got {} {:?}",
            case.name,
            refused.status,
            refused.body
        );
        assert!(
            refused.message().contains(STATUS_PENDING_SETUP) && refused.message().contains(&shared),
            "case {:?}: the refusal must name the cluster and the status that refused it, got {:?}",
            case.name,
            refused.message()
        );
        assert_eq!(
            untouched_members(&dsql.get(&shared).await),
            untouched,
            "case {:?}: the refused request must leave the record exactly as it was, got {:?}",
            case.name,
            dsql.get(&shared).await.body
        );
    }
}

/// RULE, the other side of it: the restriction belongs to the exemption, not to the
/// operation — a cluster whose status accepts a mutation takes every one of these
/// requests and writes every member it carries.
#[tokio::test]
async fn a_settled_cluster_takes_the_same_requests_in_any_combination() {
    let dsql = Dsql::in_region(FIRST_REGION);
    let all = cases();
    let identifiers = dsql.create_settled_many(all.len()).await;

    // Every case is driven on its own cluster, so the whole table is issued before
    // the one wait and read back after it.
    let cases: Vec<(Case, String)> = all.into_iter().zip(identifiers).collect();
    for (case, identifier) in &cases {
        let answer = dsql.update(identifier, case.request.clone()).await;
        assert_eq!(
            (answer.status, answer.str("status")),
            (200, STATUS_UPDATING.to_string()),
            "case {:?}: a settled cluster needs no exemption, so it must take the request, \
             got {} {:?}",
            case.name,
            answer.status,
            answer.body
        );
    }
    wait_for_the_clock().await;

    for (case, identifier) in &cases {
        let after = dsql.get(identifier).await;
        let want_status = if case.request.get("multiRegionProperties").is_some() {
            STATUS_PENDING_SETUP
        } else {
            STATUS_ACTIVE
        };
        assert_eq!(
            after.str("status"),
            want_status,
            "case {:?}: the clock must settle the cluster into {want_status}, got {:?}",
            case.name,
            after.body
        );
        if let Some(protection) = case.request.get("deletionProtectionEnabled") {
            assert_eq!(
                &after.get("deletionProtectionEnabled"),
                protection,
                "case {:?}: the accepted deletionProtectionEnabled must read back, got {:?}",
                case.name,
                after.get("deletionProtectionEnabled")
            );
        }
        if let Some(key) = case.request.get("kmsEncryptionKey").and_then(Value::as_str) {
            let reported = after
                .get("encryptionDetails")
                .get("encryptionType")
                .cloned()
                .unwrap_or(Value::Null);
            let want = if key == AWS_OWNED {
                json!(AWS_OWNED)
            } else {
                json!("CUSTOMER_MANAGED_KMS_KEY")
            };
            assert_eq!(
                reported, want,
                "case {:?}: the accepted kmsEncryptionKey must decide the reported \
                 encryptionType, got {:?}",
                case.name,
                after.get("encryptionDetails")
            );
        }
        if let Some(configuration) = case.request.get("multiRegionProperties") {
            assert_eq!(
                &after.get("multiRegionProperties"),
                configuration,
                "case {:?}: the accepted configuration must read back, got {:?}",
                case.name,
                after.get("multiRegionProperties")
            );
        }
    }
}
