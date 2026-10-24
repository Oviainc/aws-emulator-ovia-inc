// SPDX-License-Identifier: CC0-1.0

//! `CreateCluster` — the bounds the operation holds a request to, and the
//! ceiling the cluster store holds the account to — end to end over the
//! restJson1 wire.
//!
//! The round trip itself (what a create persists and what `DescribeCluster`
//! reports back out of the store), the `@idempotencyToken` retry and the
//! account's name uniqueness are asserted by [`crate::cluster_alerts`]; the
//! `InterfaceMappings` bound by [`crate::networks`]. What this suite adds are
//! the two refusals nothing else covers, each over a table that spans its class
//! rather than one request:
//!
//! 1. `Name` and `RequestId` are held to the lengths this emulator stores
//!    (`MAX_RESOURCE_NAME_LEN`, `MAX_REQUEST_ID_LEN`): a value AT the bound is
//!    stored and readable, and one past it — or below the minimum of one
//!    character — is refused with the operation's modelled 400
//!    `BadRequestException`, storing no cluster.
//! 2. The cluster store is bounded at `MAX_CLUSTERS` clusters: the create past
//!    the ceiling answers the operation's modelled 429
//!    `TooManyRequestsException` and stores nothing, so the listing still
//!    reports exactly the clusters that were accepted.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted
//! is the emulator's wire behaviour rather than an internal function's return
//! value.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    MAX_CLUSTERS, MAX_LIST_PAGE_SIZE, MAX_REQUEST_ID_LEN, MAX_RESOURCE_NAME_LEN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, error_message, medialive, ACCOUNT};

/// The cluster collection URI `CreateCluster` and `ListClusters` are bound to
/// (`smithy_query(query="operation",operation="CreateCluster,ListClusters")`:
/// POST / GET /prod/clusters).
const CLUSTERS: &str = "/prod/clusters";

/// The only `ClusterType` the model declares
/// (`smithy_query(query="shape",shape="ClusterType")`: ON_PREMISES).
const ON_PREMISES: &str = "ON_PREMISES";

/// `CreateCluster` with `body`.
async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", CLUSTERS, Some(body)).await
}

/// The `Id` a response reports, which the store minted. A response that carries
/// none fails HERE, naming the body, rather than later as a confusing 404 on a
/// URI with an empty id in it.
fn minted_id(case: &str, body: &Value) -> String {
    body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a minted cluster id, got {body}"))
        .to_string()
}

/// The ids every page of `ListClusters` reports, in the order it reported them.
async fn listed_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            Some(next) => format!("?nextToken={next}"),
            None => String::new(),
        };
        let (status, body) = call(router, "GET", &format!("{CLUSTERS}{query}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListClusters to answer 200, got {status}: {body}"
        );
        if let Some(Value::Array(page)) = body.get("clusters") {
            for summary in page {
                ids.push(
                    summary["id"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {summary}"))
                        .to_string(),
                );
            }
        }
        token = body["nextToken"].as_str().map(ToString::to_string);
        if token.is_none() {
            break;
        }
        // A walk over a bounded store takes a bounded number of pages.
        assert!(
            ids.len() <= MAX_CLUSTERS + MAX_LIST_PAGE_SIZE,
            "case {case:?}: the listing walk did not terminate, at {} ids",
            ids.len()
        );
    }
    ids
}

/// Rule 1: `CreateCluster` holds `Name` and `RequestId` to the lengths this
/// emulator stores — a value at the bound is stored, one past it or below the
/// one-character minimum is refused with the modelled `BadRequestException`
/// (400) and stores no cluster.
///
/// The rows differ along WHICH member carries the out-of-bounds value and along
/// which side of its own bound the length falls, because the two members target
/// bare `__string` (so the request boundary refuses neither) and carry different
/// bounds: the name's is `MAX_RESOURCE_NAME_LEN` and the token's is
/// `MAX_REQUEST_ID_LEN`.
#[tokio::test]
async fn create_cluster_holds_its_strings_to_the_lengths_the_store_keeps() {
    // (case, the member the row varies, that member's bound, the length in
    //  characters the row gives it, expected status)
    let table: [(&str, &str, usize, usize, u16); 8] = [
        ("name one character long", "name", MAX_RESOURCE_NAME_LEN, 1, 201),
        (
            "name at the bound",
            "name",
            MAX_RESOURCE_NAME_LEN,
            MAX_RESOURCE_NAME_LEN,
            201,
        ),
        (
            "name one past the bound",
            "name",
            MAX_RESOURCE_NAME_LEN,
            MAX_RESOURCE_NAME_LEN + 1,
            400,
        ),
        ("name below the minimum", "name", MAX_RESOURCE_NAME_LEN, 0, 400),
        (
            "requestId one character long",
            "requestId",
            MAX_REQUEST_ID_LEN,
            1,
            201,
        ),
        (
            "requestId at the bound",
            "requestId",
            MAX_REQUEST_ID_LEN,
            MAX_REQUEST_ID_LEN,
            201,
        ),
        (
            "requestId one past the bound",
            "requestId",
            MAX_REQUEST_ID_LEN,
            MAX_REQUEST_ID_LEN + 1,
            400,
        ),
        (
            "requestId below the minimum",
            "requestId",
            MAX_REQUEST_ID_LEN,
            0,
            400,
        ),
    ];

    for (case, member, bound, length, expected_status) in table {
        let router = medialive();
        let value = "c".repeat(length);
        // Every row supplies the OTHER optional members too, so the read-back
        // below compares projections that carry a value rather than two
        // absences.
        let mut body = json!({
            "clusterType": ON_PREMISES,
            "instanceRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/MediaLiveAnywhere"),
            "networkSettings": {
                "defaultRoute": "net-1",
                "interfaceMappings": [
                    { "logicalInterfaceName": "primary", "networkId": "net-1" },
                    { "logicalInterfaceName": "backup", "networkId": "net-2" }
                ]
            }
        });
        // The row's own member carries the varied value; the other keeps a value
        // well inside its bound, so the refusal is about the one being varied.
        body["name"] = json!(if member == "name" {
            value.clone()
        } else {
            format!("cluster-{case}").replace(' ', "-")
        });
        if member == "requestId" {
            body["requestId"] = json!(value.clone());
        }

        let (status, answer) = create(&router, body.clone()).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected CreateCluster to answer {expected_status}, got {status}: {answer}"
        );

        if expected_status == 400 {
            assert_eq!(
                error_code(status, &answer),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {answer}"
            );
            // The message is built from the request, so its content IS the rule:
            // it names the member, the bound it broke and the length that broke
            // it, each read off this request.
            let message = error_message(&answer);
            for expected in [member.to_string(), bound.to_string(), length.to_string()] {
                assert!(
                    message.contains(&expected),
                    "case {case:?}: expected the message to carry {expected:?}, got {message:?}"
                );
            }
            // A refused create stores nothing.
            let ids = listed_ids(&router, case).await;
            assert!(
                ids.is_empty(),
                "case {case:?}: expected a refused create to store no cluster, got {ids:?}"
            );
            continue;
        }

        // An accepted create is readable back out of the store, carrying the
        // value the request supplied at that length.
        let id = minted_id(case, &answer);
        let (status, described) = call(&router, "GET", &format!("{CLUSTERS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeCluster to answer 200, got {status}: {described}"
        );
        // Every member the create persisted, read back. `State` is left out on
        // purpose: a created cluster owes CREATING -> ACTIVE at an instant the
        // clock makes due, so the two readings may legitimately differ and the
        // transition is pinned by the round-trip suite rather than here.
        for stored in ["id", "arn", "name", "clusterType", "instanceRoleArn", "networkSettings"] {
            assert_eq!(
                described.get(stored),
                answer.get(stored),
                "case {case:?}: expected the read to report the {stored} the create persisted, \
                 got {:?} after {:?}",
                described.get(stored),
                answer.get(stored)
            );
        }
        // The members the REQUEST supplied, pinned against the request rather
        // than against the other response, so the comparison above cannot be
        // satisfied by two absences.
        for supplied in ["name", "clusterType", "instanceRoleArn", "networkSettings"] {
            assert_eq!(
                described.get(supplied),
                body.get(supplied),
                "case {case:?}: expected the read to report the {supplied} the request supplied, \
                 got {:?} for {:?}",
                described.get(supplied),
                body.get(supplied)
            );
        }
        if member == "name" {
            assert_eq!(
                described["name"],
                json!(value),
                "case {case:?}: expected the stored name of {length} characters, got {:?}",
                described["name"]
            );
        }
        // `RequestId` is an idempotency token, not a member of the cluster, so
        // it is never reported back.
        assert!(
            described.get("requestId").is_none(),
            "case {case:?}: the idempotency token is not a member of the cluster, got {described}"
        );
    }
}

/// Rule 2: the cluster store is bounded at [`MAX_CLUSTERS`] clusters — the
/// emulator's own ceiling, so a create loop cannot grow its heap without bound —
/// and the create past it answers the modelled `TooManyRequestsException` (429)
/// without storing a cluster.
///
/// The rows differ along the shape of the request that is refused once the store
/// is full, because the ceiling is about the STORE rather than about the request:
/// a fresh name, a name the account already holds, and a retry carrying the
/// `@idempotencyToken` of a cluster that was never created.
#[tokio::test]
async fn a_create_past_the_cluster_ceiling_is_refused() {
    let router = medialive();

    // Fill the store to exactly the ceiling.
    let case = "filling the store";
    let mut accepted: Vec<String> = Vec::new();
    for index in 0..MAX_CLUSTERS {
        let (status, created) = create(
            &router,
            json!({ "name": format!("cluster-{index}"), "clusterType": ON_PREMISES }),
        )
        .await;
        assert_eq!(
            status, 201,
            "setup: cluster {index} of {MAX_CLUSTERS}: expected 201, got {status}: {created}"
        );
        accepted.push(minted_id(case, &created));
    }

    // (case, the body of the create that is refused, the error it answers)
    let table: [(&str, Value, &str); 3] = [
        (
            "a fresh name",
            json!({ "name": "one-too-many", "clusterType": ON_PREMISES }),
            "TooManyRequestsException",
        ),
        (
            "an unused idempotency token",
            json!({
                "name": "one-too-many-with-a-token",
                "clusterType": ON_PREMISES,
                "requestId": "never-used-before"
            }),
            "TooManyRequestsException",
        ),
        (
            // The account's name uniqueness is checked before the ceiling, so a
            // name it already holds is still a conflict rather than a throttle:
            // either way no cluster is stored.
            "a name the account already holds",
            json!({ "name": "cluster-0", "clusterType": ON_PREMISES }),
            "ConflictException",
        ),
    ];

    for (case, body, expected_code) in table {
        let (status, refused) = create(&router, body).await;
        let expected_status = if expected_code == "ConflictException" {
            409
        } else {
            429
        };
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected CreateCluster to answer {expected_status}, got {status}: {refused}"
        );
        assert_eq!(
            error_code(status, &refused),
            expected_code,
            "case {case:?}: expected {expected_code}, got {refused}"
        );
    }

    // The refusals stored nothing: the listing still reports exactly the
    // clusters that were accepted, in the order their ids were minted.
    let ids = listed_ids(&router, "after the refusals").await;
    assert_eq!(
        ids, accepted,
        "expected the store to still hold exactly the {MAX_CLUSTERS} accepted clusters"
    );

    // And each of them is still readable.
    for (index, id) in accepted.iter().enumerate() {
        let (status, described) = call(&router, "GET", &format!("{CLUSTERS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "cluster {index}: expected DescribeCluster to answer 200, got {status}: {described}"
        );
        assert_eq!(
            described["name"],
            json!(format!("cluster-{index}")),
            "cluster {index}: expected the stored name, got {:?}",
            described["name"]
        );
    }
}
