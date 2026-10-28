// SPDX-License-Identifier: CC0-1.0

//! Region-qualified values this service DERIVES for a cluster that is already in
//! the store, read over the wire.
//!
//! RULE: a region-qualified value a handler derives for a resource already in the
//! store — the KMS key ARN a `kmsEncryptionKey` id or alias expands into, the ARN
//! of a resource nested under it — carries the region that resource's own ARN was
//! minted in, not the region the request happens to be signed for. Only the call
//! that MINTS a resource takes the region from the request.
//!
//! Two views of ONE store, signed for different regions, are exactly what the
//! production router builds (`harness::Dsql::sharing`), so a cluster minted in one
//! region is reachable by identifier through an endpoint signed for another; every
//! value derived for it has to stay in the cluster's region.
//!
//! The tables span the class the rule generalises over: both directions of a
//! region pair and region ids of different shapes, every form `kmsEncryptionKey`
//! admits (a bare key id, an alias, a full ARN that already names its own region,
//! and the reserved value that names no key at all), and the two derived values
//! this service builds for a persisted cluster.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::harness::{expected_stream_arn, wait_for_the_clock, Answer, Dsql};
use crate::stream_lifecycle::kinesis_target;

/// The region pairs the rule is exercised over: the cluster's home region and the
/// region the acting request is signed for. Both directions of one pair, plus ids
/// that differ in length and in the number of hyphens, so the region can only come
/// from the cluster.
const REGION_PAIRS: &[(&str, &str)] = &[
    ("us-east-1", "eu-west-1"),
    ("eu-west-1", "us-east-1"),
    ("ap-southeast-2", "us-gov-west-1"),
];

/// The reserved `kmsEncryptionKey` value that switches a cluster to the key owned
/// by Amazon Web Services, so it names no customer key and expands into no ARN.
/// Source: `smithy_query(query="doc", shape="UpdateClusterInput",
/// member="kmsEncryptionKey")`.
const RESERVED_AWS_OWNED: &str = "AWS_OWNED_KMS_KEY";

/// One `kmsEncryptionKey` value and the `kmsKeyArn` a cluster in `home` must report
/// after an update carrying it.
struct KeyCase {
    name: &'static str,
    key: String,
    /// The `kmsKeyArn` the cluster must report, or `None` when the value names no
    /// customer key.
    want_key_arn: Option<String>,
}

/// The whole class of forms `kmsEncryptionKey` admits ("You can specify the ARN,
/// ID, or alias of an existing key", `smithy_query(query="doc",
/// shape="UpdateClusterInput", member="kmsEncryptionKey")`), with the ARN each one
/// resolves to for a cluster living in `home`.
fn key_cases(home: &str) -> Vec<KeyCase> {
    let account = engine::EMULATOR_ACCOUNT_ID;
    vec![
        KeyCase {
            name: "a bare key id",
            key: "1234abcd-12ab-34cd-56ef".to_string(),
            want_key_arn: Some(format!(
                "arn:aws:kms:{home}:{account}:key/1234abcd-12ab-34cd-56ef"
            )),
        },
        KeyCase {
            name: "an alias",
            key: "alias/finance".to_string(),
            want_key_arn: Some(format!("arn:aws:kms:{home}:{account}:alias/finance")),
        },
        KeyCase {
            name: "a full key ARN, which already names its own region",
            key: format!("arn:aws:kms:ap-south-1:{account}:key/9f0abc"),
            want_key_arn: Some(format!("arn:aws:kms:ap-south-1:{account}:key/9f0abc")),
        },
        KeyCase {
            name: "the reserved value, which names no customer key",
            key: RESERVED_AWS_OWNED.to_string(),
            want_key_arn: None,
        },
    ]
}

/// The `kmsKeyArn` a response's `encryptionDetails` reports, if it reports one.
fn key_arn(answer: &Answer) -> Option<String> {
    answer
        .get("encryptionDetails")
        .get("kmsKeyArn")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Two views of one store: the region the cluster is minted in, and another.
fn two_views(home: &str, away: &str) -> (Dsql, Dsql) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let at_home = Dsql::sharing(&engine, home);
    let elsewhere = Dsql::sharing(&engine, away);
    (at_home, elsewhere)
}

/// RULE applied to `UpdateCluster`'s `encryptionDetails`: the KMS key ARN a key id
/// or alias expands into is qualified with the CLUSTER's region, so an update
/// signed for another region stores the same value an update at home stores.
#[tokio::test]
async fn the_updated_kms_key_arn_stays_in_the_clusters_region() {
    for (home, away) in REGION_PAIRS {
        let (at_home, elsewhere) = two_views(home, away);
        let cases = key_cases(home);
        // One fresh cluster per case per acting view, all created before the one
        // wait: an update needs a cluster the clock owes nothing.
        let clusters = at_home.create_settled_many(cases.len() * 2).await;

        for (index, case) in cases.iter().enumerate() {
            for (acting, view, identifier) in [
                (
                    "an update signed for the cluster's own region",
                    &at_home,
                    &clusters[index * 2],
                ),
                (
                    "an update signed for another region",
                    &elsewhere,
                    &clusters[index * 2 + 1],
                ),
            ] {
                let case_name = format!("{home}->{away} {:?} via {acting}", case.name);
                let arn = at_home.get(identifier).await.str("arn");
                assert!(
                    arn.contains(&format!(":dsql:{home}:")),
                    "case {case_name}: the cluster must live in {home}, got ARN {arn:?}"
                );

                let updated = view
                    .update(identifier, json!({"kmsEncryptionKey": case.key}))
                    .await;
                assert_eq!(
                    updated.status, 200,
                    "case {case_name}: UpdateCluster must answer 200, got {} {:?}",
                    updated.status, updated.body
                );

                // The value is written into stored state, so BOTH views read it back.
                for (reader, reading) in [(&at_home, home), (&elsewhere, away)] {
                    let after = reader.get(identifier).await;
                    assert_eq!(
                        key_arn(&after),
                        case.want_key_arn,
                        "case {case_name}: read in {reading}, kmsKeyArn must be \
                         {:?}, got {:?}",
                        case.want_key_arn,
                        after.get("encryptionDetails")
                    );
                }
            }
        }
    }
}

/// RULE as an invariant: whichever region an update is signed for, the cluster's
/// own region-qualified values are left alone — its ARN and its connection
/// endpoint still name the region it was minted in, and a later update does not
/// move them.
#[tokio::test]
async fn an_update_from_another_region_moves_nothing_into_that_region() {
    for (home, away) in REGION_PAIRS {
        let (at_home, elsewhere) = two_views(home, away);
        let identifier = at_home.create_settled().await;
        let before = at_home.get(&identifier).await;
        let (arn, endpoint) = (before.str("arn"), before.str("endpoint"));

        // Each step is an update signed for the OTHER region, carrying a member the
        // handler has to resolve.
        for (step, body) in [
            ("a key id", json!({"kmsEncryptionKey": "rotating-key"})),
            ("deletion protection", json!({"deletionProtectionEnabled": true})),
            ("the reserved value", json!({"kmsEncryptionKey": RESERVED_AWS_OWNED})),
        ] {
            let case = format!("{home}->{away} after {step}");
            let updated = elsewhere.update(&identifier, body).await;
            assert_eq!(
                updated.status, 200,
                "case {case}: UpdateCluster must answer 200, got {} {:?}",
                updated.status, updated.body
            );
            assert_eq!(
                updated.str("arn"),
                arn,
                "case {case}: the answer must carry the cluster's own ARN {arn:?}, got {:?}",
                updated.str("arn")
            );
            wait_for_the_clock().await;

            let after = at_home.get(&identifier).await;
            assert_eq!(
                after.str("arn"),
                arn,
                "case {case}: the stored ARN must stay {arn:?}, got {:?}",
                after.str("arn")
            );
            assert_eq!(
                after.str("endpoint"),
                endpoint,
                "case {case}: the stored endpoint must stay {endpoint:?}, got {:?}",
                after.str("endpoint")
            );
            assert!(
                key_arn(&after).is_none_or(|key| key.contains(&format!(":kms:{home}:"))),
                "case {case}: a derived kmsKeyArn must name {home}, got {:?}",
                after.get("encryptionDetails")
            );
        }
    }
}

/// RULE applied to `CreateStream`: a stream is nested under its cluster
/// (`cluster/{cluster}/stream/{stream}`), so the region segment of its ARN is the
/// CLUSTER's — whichever region the creating request is signed for — and every
/// reader reports that one ARN back.
#[tokio::test]
async fn a_streams_arn_carries_its_clusters_region() {
    for (home, away) in REGION_PAIRS {
        let (at_home, elsewhere) = two_views(home, away);
        let cluster = at_home.create_settled().await;

        for (acting, view) in [
            ("a create signed for the cluster's own region", &at_home),
            ("a create signed for another region", &elsewhere),
        ] {
            let case = format!("{home}->{away} via {acting}");
            let created = view
                .create_stream(
                    &cluster,
                    json!({
                        "format": "JSON",
                        "ordering": "UNORDERED",
                        "targetDefinition": kinesis_target(view.region(), "changes", "dsql-cdc"),
                    }),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case}: CreateStream must answer 200, got {} {:?}",
                created.status, created.body
            );

            let stream = created.str("streamIdentifier");
            let want = expected_stream_arn(home, &cluster, &stream);
            assert_eq!(
                created.str("arn"),
                want,
                "case {case}: the minted stream ARN must be {want:?}, got {:?}",
                created.str("arn")
            );

            // The ARN is persisted, so both views read the same one back.
            for (reader, reading) in [(&at_home, home), (&elsewhere, away)] {
                let read_back = reader.get_stream(&cluster, &stream).await;
                assert_eq!(
                    read_back.status, 200,
                    "case {case}: GetStream in {reading} must answer 200, got {} {:?}",
                    read_back.status, read_back.body
                );
                assert_eq!(
                    read_back.str("arn"),
                    want,
                    "case {case}: GetStream in {reading} must report {want:?}, got {:?}",
                    read_back.str("arn")
                );
            }
        }
    }
}
