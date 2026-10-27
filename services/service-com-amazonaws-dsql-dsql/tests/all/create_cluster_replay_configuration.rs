// SPDX-License-Identifier: CC0-1.0

//! What a `CreateCluster` retry ANSWERS once the cluster has been reconfigured,
//! end to end over the wire.
//!
//! RULE: a retry carrying a `clientToken` the ledger already served answers the
//! result of the ORIGINAL call — every member of it — so an output member a
//! later call can write again (`status`, `deletionProtectionEnabled`,
//! `encryptionDetails`, `multiRegionProperties`) carries the value the first
//! call reported rather than the one the cluster holds when the retry arrives,
//! and the retry itself changes nothing.
//!
//! The table spans what an intervening `UpdateCluster` changes between the first
//! call and its retry, which is what the rule generalises over: which output
//! member moves, in which direction, in which form the request names it (a key
//! by alias, by id, by ARN, or the reserved Amazon-owned key), a witness Region,
//! a peer list, and every mutable member at once. The members `UpdateCluster`
//! writes are its own input members (`smithy_query(query="operation",
//! operation="UpdateCluster")`: `deletionProtectionEnabled`,
//! `kmsEncryptionKey` — which the answer reports as `encryptionDetails` — and
//! `multiRegionProperties`).
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use serde_json::{json, Value};

use crate::harness::{expected_arn, wait_for_the_clock, Dsql};

/// One row: the `CreateCluster` body the token is bound to, and the
/// `UpdateCluster` body that lands between the first call and its retry.
struct Row {
    name: &'static str,
    create: Value,
    update: Value,
}

/// A cluster ARN in another Region, as a `multiRegionProperties.clusters` entry
/// names a peer.
fn peer(identifier: &str) -> String {
    expected_arn("us-west-2", identifier)
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "deletion protection enabled, then turned off",
            create: json!({"deletionProtectionEnabled": true}),
            update: json!({"deletionProtectionEnabled": false}),
        },
        Row {
            name: "deletion protection disabled, then turned on",
            create: json!({"deletionProtectionEnabled": false}),
            update: json!({"deletionProtectionEnabled": true}),
        },
        Row {
            name: "the Amazon-owned key, then a customer key by alias",
            create: json!({"deletionProtectionEnabled": false}),
            update: json!({"kmsEncryptionKey": "alias/adopted-later"}),
        },
        Row {
            name: "a customer key by alias, then another alias",
            create: json!({
                "deletionProtectionEnabled": false,
                "kmsEncryptionKey": "alias/created-with"
            }),
            update: json!({"kmsEncryptionKey": "alias/replaced-with"}),
        },
        Row {
            name: "a customer key by id, then a key by ARN",
            create: json!({
                "deletionProtectionEnabled": false,
                "kmsEncryptionKey": "11111111-2222-3333-4444-555555555555"
            }),
            update: json!({
                "kmsEncryptionKey": format!(
                    "arn:aws:kms:us-east-1:{}:key/99999999-8888-7777-6666-555555555555",
                    engine::EMULATOR_ACCOUNT_ID
                )
            }),
        },
        Row {
            name: "a customer key, then back to the Amazon-owned key",
            create: json!({
                "deletionProtectionEnabled": false,
                "kmsEncryptionKey": "alias/created-with"
            }),
            update: json!({"kmsEncryptionKey": "AWS_OWNED_KMS_KEY"}),
        },
        Row {
            name: "single-Region, then a witness Region configured",
            create: json!({"deletionProtectionEnabled": false}),
            update: json!({"multiRegionProperties": {"witnessRegion": "us-west-2"}}),
        },
        Row {
            name: "a witness Region, then another witness Region",
            create: json!({
                "deletionProtectionEnabled": false,
                "multiRegionProperties": {"witnessRegion": "us-west-2"}
            }),
            update: json!({"multiRegionProperties": {"witnessRegion": "eu-west-1"}}),
        },
        Row {
            name: "one peer cluster, then three peers",
            create: json!({
                "deletionProtectionEnabled": false,
                "multiRegionProperties": {
                    "witnessRegion": "us-west-2",
                    "clusters": [peer("aaaaaaaaaaaaaaaaaaaaaaaaaa")]
                }
            }),
            update: json!({
                "multiRegionProperties": {
                    "witnessRegion": "us-west-2",
                    "clusters": [
                        peer("bbbbbbbbbbbbbbbbbbbbbbbbbb"),
                        peer("cccccccccccccccccccccccccc"),
                        peer("dddddddddddddddddddddddddd")
                    ]
                }
            }),
        },
        Row {
            name: "every mutable member changed at once",
            create: json!({
                "deletionProtectionEnabled": true,
                "kmsEncryptionKey": "alias/created-with"
            }),
            update: json!({
                "deletionProtectionEnabled": false,
                "kmsEncryptionKey": "alias/replaced-with",
                "multiRegionProperties": {"witnessRegion": "eu-west-1"}
            }),
        },
    ]
}

/// The `CreateCluster` output members an intervening update's body writes, so a
/// row can assert the update really moved the cluster off what the first call
/// answered: `UpdateCluster`'s own input members, each under the name the create
/// answers it with (`kmsEncryptionKey` is reported as `encryptionDetails`).
fn changed_members(update: &Value) -> Vec<&'static str> {
    let mut members = Vec::new();
    if update.get("deletionProtectionEnabled").is_some() {
        members.push("deletionProtectionEnabled");
    }
    if update.get("kmsEncryptionKey").is_some() {
        members.push("encryptionDetails");
    }
    if update.get("multiRegionProperties").is_some() {
        members.push("multiRegionProperties");
    }
    members
}

/// Every member of a cluster's configuration a later call can write, under the
/// name both `CreateCluster` and `GetCluster` answer it with.
const CONFIGURATION_MEMBERS: [&str; 3] = [
    "deletionProtectionEnabled",
    "encryptionDetails",
    "multiRegionProperties",
];

/// RULE: a `CreateCluster` retry under a `clientToken` the service already
/// served answers the result the FIRST call produced — identity, `status` and
/// the whole configuration it reported — whatever an `UpdateCluster` has changed
/// since, and mints and changes nothing.
#[tokio::test]
async fn a_create_cluster_retry_answers_the_configuration_the_first_call_answered() {
    let dsql = Dsql::new();
    let rows = rows();

    // Every row's cluster is created before the one wait, so the suite waits for
    // the clock once rather than once per row.
    let mut first_answers = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let mut body = row.create.clone();
        body["clientToken"] = json!(format!("create-replay-{index}"));
        let first = dsql.create(body).await;
        assert_eq!(
            first.status, 200,
            "case {:?}: the first call must answer 200, got {} {:?}",
            row.name, first.status, first.body
        );
        first_answers.push(first);
    }
    wait_for_the_clock().await;

    for (index, row) in rows.iter().enumerate() {
        let first = &first_answers[index];
        let identifier = first.str("identifier");

        let updated = dsql.update(&identifier, row.update.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: the intervening update must answer 200, got {} {:?}",
            row.name, updated.status, updated.body
        );
        let live = dsql.get(&identifier).await;
        assert_eq!(
            live.status, 200,
            "case {:?}: the reconfigured cluster must still be readable, got {} {:?}",
            row.name, live.status, live.body
        );
        // The row only tests the rule if the update moved the member off what
        // the first call answered, so answering the live cluster is
        // distinguishable from answering that call's result.
        for member in changed_members(&row.update) {
            assert_ne!(
                live.get(member),
                first.get(member),
                "case {:?}: the intervening update must move {member} off {:?}, got {:?}",
                row.name,
                first.get(member),
                live.get(member)
            );
        }

        let mut replay_body = row.create.clone();
        replay_body["clientToken"] = json!(format!("create-replay-{index}"));
        let replay = dsql.create(replay_body).await;
        assert_eq!(
            replay.status, 200,
            "case {:?}: the retry must answer 200, got {} {:?}",
            row.name, replay.status, replay.body
        );
        for member in CONFIGURATION_MEMBERS
            .iter()
            .chain(["status", "identifier", "arn", "endpoint", "creationTime"].iter())
        {
            assert_eq!(
                replay.get(member),
                first.get(member),
                "case {:?}: the retry must answer the {member} the first call answered ({:?}), \
                 got {:?}",
                row.name,
                first.get(member),
                replay.get(member)
            );
        }
        assert_eq!(
            replay.body, first.body,
            "case {:?}: the retry must answer the result the first call produced ({:?}), got {:?}",
            row.name, first.body, replay.body
        );

        // The retry had no effect of its own: the configuration the update left
        // is still the cluster's.
        let after = dsql.get(&identifier).await;
        for member in CONFIGURATION_MEMBERS {
            assert_eq!(
                after.get(member),
                live.get(member),
                "case {:?}: the retry must leave {member} at {:?}, got {:?}",
                row.name,
                live.get(member),
                after.get(member)
            );
        }
    }

    // Nor did any retry mint a second cluster.
    let listed = dsql
        .list("?max-results=100")
        .await
        .get("clusters")
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(
        listed,
        rows.len(),
        "the retries must mint nothing second: expected {} cluster(s), got {listed}",
        rows.len()
    );
}
