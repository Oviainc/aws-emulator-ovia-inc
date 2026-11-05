// SPDX-License-Identifier: CC0-1.0

//! WHERE a `clientToken` stands for an earlier request, read over the wire.
//!
//! RULE: a `clientToken` names a request made AT one Region's endpoint, so the
//! idempotency ledger resolves it only among the calls that endpoint served. The
//! same token carried to another Region's endpoint stands for no earlier request
//! there, so that call is served as a fresh one and mints a resource of its own
//! endpoint's Region; at the endpoint that recorded the token it still replays
//! the call it served, whatever another endpoint has since done with the same
//! string.
//!
//! Two views of ONE store signed for different Regions are exactly what the
//! production router builds (`harness::Dsql::sharing`), so this is about which
//! endpoint a token belongs to rather than about two stores.
//!
//! The tables span what the rule generalises over: both directions of a Region
//! pair and Region ids of different shapes; the whole shape of `ClientToken`
//! (`@length(1, 128)`, `@pattern("^[!-~]+$")`, `smithy_query(query="shape",
//! shape="ClientToken")`) from one character to its maximum length; the request
//! bodies a create's token is bound to, including the members whose answer is
//! Region-derived (`kmsEncryptionKey`, which expands into a key ARN) and one
//! that is not; and every ledger this service keeps.
//!
//! Every wire case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::state::{
    ComAmazonawsDsqlDsqlState, RecordedCall, TokenScope, TokenVerdict,
};

use crate::cluster_lifecycle::cluster_id_for;
use crate::harness::{expected_arn, Dsql};

/// The Region pairs the rule is exercised over: the endpoint the token is first
/// used at, and the endpoint the same token is then carried to. Both directions
/// of one pair, plus ids that differ in length and in hyphen count, so a value
/// can only have come from the endpoint that served the call.
const REGION_PAIRS: &[(&str, &str)] = &[
    ("us-east-1", "eu-west-1"),
    ("eu-west-1", "us-east-1"),
    ("ap-southeast-2", "us-gov-west-1"),
];

/// A bare KMS key id, so the `encryptionDetails.kmsKeyArn` the answer reports is
/// a value the handler DERIVES in the Region it minted the cluster for rather
/// than one the request spelled out.
const BARE_KEY_ID: &str = "1234abcd-12ab-34cd-56ef";

/// One row: a `clientToken` form, and the `CreateCluster` body both endpoints
/// are asked with.
struct Row {
    name: &'static str,
    /// The token both creates carry.
    token: String,
    /// The body both creates carry, before the token is written into it.
    body: Value,
}

/// The rows: each token form `ClientToken` admits, carried with a different
/// request body — including bodies whose answer holds a Region-derived member.
fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "one character, ClientToken's minimum, on a plain create",
            token: "t".to_string(),
            body: json!({"deletionProtectionEnabled": false}),
        },
        Row {
            name: "an ordinary token, with tags",
            token: "tok-1".to_string(),
            body: json!({"deletionProtectionEnabled": false, "tags": {"env": "test"}}),
        },
        Row {
            name: "punctuation only, with a bare KMS key id",
            token: "~!*()-_.".to_string(),
            body: json!({"deletionProtectionEnabled": false, "kmsEncryptionKey": BARE_KEY_ID}),
        },
        Row {
            name: "ClientToken's maximum length, with a multi-Region configuration",
            token: "m".repeat(128),
            body: json!({
                "deletionProtectionEnabled": false,
                "multiRegionProperties": {"witnessRegion": "us-west-2"},
            }),
        },
        Row {
            name: "an ordinary token, with deletion protection left at its default",
            token: "tok-2".to_string(),
            body: json!({}),
        },
    ]
}

/// The connection endpoint `CreateCluster` derives for a cluster it mints in
/// `region` (`{identifier}.dsql.{region}.on.aws`, which `Endpoint`'s
/// `^[a-zA-Z0-9.-]+$` admits).
fn expected_endpoint(region: &str, identifier: &str) -> String {
    format!("{identifier}.dsql.{region}.on.aws")
}

/// The `kmsKeyArn` an answer's `encryptionDetails` reports, if it reports one.
fn key_arn(answer: &crate::harness::Answer) -> Option<String> {
    answer
        .get("encryptionDetails")
        .get("kmsKeyArn")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Assert every Region-derived member of a create's answer names `region`: the
/// ARN, the connection endpoint and the key ARN a bare `kmsEncryptionKey`
/// expanded into.
fn assert_minted_in(case: &str, answer: &crate::harness::Answer, region: &str) {
    let identifier = answer.str("identifier");
    assert_eq!(
        answer.str("arn"),
        expected_arn(region, &identifier),
        "case {case:?}: the cluster must carry {region}'s ARN, got {:?}",
        answer.str("arn")
    );
    assert_eq!(
        answer.str("endpoint"),
        expected_endpoint(region, &identifier),
        "case {case:?}: the connection endpoint must name {region}, got {:?}",
        answer.str("endpoint")
    );
    assert!(
        key_arn(answer).is_none_or(|key| key.contains(&format!(":kms:{region}:"))),
        "case {case:?}: a derived kmsKeyArn must name {region}, got {:?}",
        answer.get("encryptionDetails")
    );
}

/// The identifiers the store holds, read through `ListClusters`.
async fn listed_identifiers(view: &Dsql) -> Vec<String> {
    view.list("?max-results=100")
        .await
        .get("clusters")
        .as_array()
        .map(|clusters| {
            clusters
                .iter()
                .map(|cluster| {
                    cluster
                        .get("identifier")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RULE applied to `CreateCluster`, the one operation whose answer is minted in
/// the REQUEST's Region: a token recorded at one endpoint names no request at
/// another, so the same token and the same body asked of a second endpoint mint
/// a cluster of that endpoint's own Region — and each endpoint's own retry still
/// replays its own cluster.
#[tokio::test]
async fn a_create_cluster_token_is_resolved_only_at_the_endpoint_that_recorded_it() {
    for (home_region, away_region) in REGION_PAIRS {
        for row in rows() {
            let case = format!("{home_region}->{away_region} with {}", row.name);
            // One store, two endpoint views of it, as the production router
            // builds them.
            let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
            let home = Dsql::sharing(&engine, home_region);
            let away = Dsql::sharing(&engine, away_region);

            let mut body = row.body.clone();
            body["clientToken"] = json!(row.token.clone());

            let first = home.create(body.clone()).await;
            assert_eq!(
                first.status, 200,
                "case {case:?}: the create at {home_region} must answer 200, got {} {:?}",
                first.status, first.body
            );
            assert_minted_in(&case, &first, home_region);

            // The token still names that request AT the endpoint that recorded
            // it: the identical create replays the first cluster.
            let home_replay = home.create(body.clone()).await;
            assert_eq!(
                home_replay.str("identifier"),
                first.str("identifier"),
                "case {case:?}: the retry at {home_region} must answer the first cluster ({:?}), \
                 got {} {:?}",
                first.str("identifier"),
                home_replay.status,
                home_replay.body
            );

            // The same token at the OTHER endpoint names no earlier request, so
            // it is served as a fresh create — a cluster of that endpoint's own
            // Region, not the one the first endpoint minted.
            let second = away.create(body.clone()).await;
            assert_eq!(
                second.status, 200,
                "case {case:?}: the create at {away_region} must answer 200, got {} {:?}",
                second.status, second.body
            );
            assert_ne!(
                second.str("identifier"),
                first.str("identifier"),
                "case {case:?}: the create at {away_region} must mint a cluster of its own, got \
                 the one {home_region} minted ({:?})",
                second.str("identifier")
            );
            assert_minted_in(&case, &second, away_region);

            // The answer names a resource that very endpoint serves: its
            // ARN-addressed tagging resolves there, and nowhere else.
            let away_arn = second.str("arn");
            let served = away.list_tags_for_resource(&away_arn).await;
            assert_eq!(
                served.status, 200,
                "case {case:?}: {away_region} must serve the ARN it just answered ({away_arn:?}), \
                 got {} {:?}",
                served.status, served.body
            );
            let foreign = home.list_tags_for_resource(&away_arn).await;
            assert_eq!(
                (foreign.status, foreign.error.as_deref()),
                (404, Some("NotFoundException")),
                "case {case:?}: {home_region} holds no resource of {away_region} \
                 ({away_arn:?}), got {} {:?}",
                foreign.status,
                foreign.body
            );

            // Each endpoint's retry replays ITS OWN call: the second endpoint's
            // record did not displace the first endpoint's, and vice versa.
            let away_replay = away.create(body.clone()).await;
            assert_eq!(
                away_replay.str("identifier"),
                second.str("identifier"),
                "case {case:?}: the retry at {away_region} must answer its own cluster ({:?}), \
                 got {} {:?}",
                second.str("identifier"),
                away_replay.status,
                away_replay.body
            );
            let home_replay_again = home.create(body).await;
            assert_eq!(
                home_replay_again.str("identifier"),
                first.str("identifier"),
                "case {case:?}: the retry at {home_region} must still answer its own cluster \
                 ({:?}), got {} {:?}",
                first.str("identifier"),
                home_replay_again.status,
                home_replay_again.body
            );

            // Exactly the two clusters the two endpoints minted — every retry
            // minted nothing — and each is readable back through GetCluster
            // carrying the ARN its creating endpoint answered.
            let mut listed = listed_identifiers(&home).await;
            listed.sort();
            let mut minted = vec![first.str("identifier"), second.str("identifier")];
            minted.sort();
            assert_eq!(
                listed, minted,
                "case {case:?}: the store must hold exactly the two minted clusters ({minted:?}), \
                 got {listed:?}"
            );
            for (minting_region, created) in [(home_region, &first), (away_region, &second)] {
                let read_back = away.get(&created.str("identifier")).await;
                assert_eq!(
                    read_back.str("arn"),
                    created.str("arn"),
                    "case {case:?}: the cluster minted in {minting_region} must read back with \
                     the ARN it was minted with ({:?}), got {} {:?}",
                    created.str("arn"),
                    read_back.status,
                    read_back.body
                );
            }
        }
    }
}

/// The fingerprint both recorded calls below carry, so the verdict turns on the
/// endpoint rather than on the parameters.
const SAME_REQUEST: &str = "the-same-parameters";

/// RULE at the ledger, over EVERY `clientToken` this service keeps a ledger for:
/// a record is keyed by the token together with the Region of the endpoint that
/// served the call, so the same token answers `Replay` at that endpoint,
/// `Unused` at any other, and a second endpoint recording it leaves the first
/// endpoint's record standing.
///
/// Driven against the state directly: a ledger is per-operation, so covering
/// every one over the wire would need one suite per operation — the wire-level
/// case is the test above.
#[test]
fn every_ledger_keys_its_record_by_the_endpoint_that_served_the_call() {
    for (home_region, away_region) in REGION_PAIRS {
        for scope in TokenScope::ALL {
            // Token forms spanning `ClientToken`'s bounds, so the key is the
            // whole string beside the Region rather than a prefix of it.
            for token in ["t", "tok-1", &"m".repeat(128)] {
                let case = format!("{scope:?} at {home_region} with token {token:?}");
                let mut state = ComAmazonawsDsqlDsqlState::default();
                let home_resource = cluster_id_for(0);
                let away_resource = cluster_id_for(1);

                state.tokens(scope).record(
                    home_region,
                    token.to_string(),
                    SAME_REQUEST.to_string(),
                    RecordedCall::new(&home_resource),
                );

                assert_eq!(
                    state
                        .tokens_of(scope)
                        .verdict(home_region, token, SAME_REQUEST),
                    TokenVerdict::Replay(RecordedCall::new(&home_resource)),
                    "case {case:?}: the recording endpoint must replay its own call, got {:?}",
                    state
                        .tokens_of(scope)
                        .verdict(home_region, token, SAME_REQUEST)
                );
                assert_eq!(
                    state
                        .tokens_of(scope)
                        .verdict(away_region, token, SAME_REQUEST),
                    TokenVerdict::Unused,
                    "case {case:?}: the token names no request at {away_region}, got {:?}",
                    state
                        .tokens_of(scope)
                        .verdict(away_region, token, SAME_REQUEST)
                );
                // At the recording endpoint the token is still bound to the
                // parameters it served: another request wearing it is a conflict.
                assert_eq!(
                    state
                        .tokens_of(scope)
                        .verdict(home_region, token, "other-parameters"),
                    TokenVerdict::Mismatch,
                    "case {case:?}: another request under that token must be a mismatch, got {:?}",
                    state
                        .tokens_of(scope)
                        .verdict(home_region, token, "other-parameters")
                );

                // The other endpoint serving the same string records a call of
                // its own, and neither record is the other's.
                state.tokens(scope).record(
                    away_region,
                    token.to_string(),
                    SAME_REQUEST.to_string(),
                    RecordedCall::new(&away_resource),
                );
                for (region, resource) in [
                    (home_region, &home_resource),
                    (away_region, &away_resource),
                ] {
                    assert_eq!(
                        state.tokens_of(scope).verdict(region, token, SAME_REQUEST),
                        TokenVerdict::Replay(RecordedCall::new(resource)),
                        "case {case:?}: {region} must replay its own call on {resource}, got {:?}",
                        state.tokens_of(scope).verdict(region, token, SAME_REQUEST)
                    );
                }
            }
        }
    }
}
