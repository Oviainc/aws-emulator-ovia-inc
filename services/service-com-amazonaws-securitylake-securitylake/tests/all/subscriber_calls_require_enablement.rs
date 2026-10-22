// SPDX-License-Identifier: CC0-1.0

//! The ENABLEMENT precondition of the subscriber operations, end to end over HTTP.
//!
//! RULE under test: a subscriber-scoped Security Lake operation needs the account to have
//! Security Lake enabled in at least one Region (i.e. to hold at least one data lake);
//! until then there is no subscriber collection, so the call is refused with the modelled
//! `ResourceNotFoundException` at HTTP 404 — never with an empty success that a client
//! cannot tell apart from a configured-but-empty account. Once any Region is enabled the
//! gate lifts for every Region, and deleting the last data lake puts it back.
//!
//! Every case drives a real `restJson1` request through the router into this service, on
//! the shared harness in `data_lake_and_subscriber_lifecycle`.

use serde_json::{json, Value};

use super::data_lake_and_subscriber_lifecycle::{array, text, Emulator, Reply};

/// The wording the service answers the gate with
/// (`service_com_amazonaws_securitylake_securitylake::state::NOT_ENABLED_IN_ANY_REGION_MESSAGE`),
/// read from the service rather than copied, so there is one source for it.
const GATE_MESSAGE: &str =
    service_com_amazonaws_securitylake_securitylake::state::NOT_ENABLED_IN_ANY_REGION_MESSAGE;

/// A `UUID`-shaped subscriber id, matching the pattern the model's `UUID` shape carries
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`). No subscriber is
/// ever created in the gated phase, so this id is simply well-formed and absent.
const ABSENT_SUBSCRIBER_ID: &str = "0f1e2d3c-4b5a-4968-8776-65544332211f";

/// One subscriber-scoped call, as a row of the table below: the HTTP method, the path
/// (including any query string) and the request body.
struct Call {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

fn subscriber_body(name: &str) -> Value {
    json!({
        "subscriberIdentity": { "principal": "123456789012", "externalId": "ext-1" },
        "subscriberName": name,
        "sources": [{ "awsLogSource": { "sourceName": "VPC_FLOW", "sourceVersion": "latest" } }],
    })
}

/// Every subscriber-scoped call this service implements, in the forms the rule has to hold
/// over: each operation of the collection, and — for the paginated read — no query, each
/// `@range` bound of `maxResults`, and a `nextToken` of exactly the form this service
/// issues (hex of `<index>:<subscriberId>`), which the gate must precede.
fn subscriber_calls() -> Vec<Call> {
    vec![
        Call {
            case: "ListSubscribers, no query",
            method: "GET",
            path: "/v1/subscribers".to_string(),
            body: None,
        },
        Call {
            case: "ListSubscribers, maxResults at the range minimum",
            method: "GET",
            path: "/v1/subscribers?maxResults=1".to_string(),
            body: None,
        },
        Call {
            case: "ListSubscribers, maxResults at the range maximum",
            method: "GET",
            path: "/v1/subscribers?maxResults=100".to_string(),
            body: None,
        },
        Call {
            case: "ListSubscribers, a well-formed nextToken",
            method: "GET",
            path: format!(
                "/v1/subscribers?nextToken={}",
                hex::encode(format!("0:{ABSENT_SUBSCRIBER_ID}"))
            ),
            body: None,
        },
        Call {
            case: "GetSubscriber",
            method: "GET",
            path: format!("/v1/subscribers/{ABSENT_SUBSCRIBER_ID}"),
            body: None,
        },
        Call {
            case: "DeleteSubscriber",
            method: "DELETE",
            path: format!("/v1/subscribers/{ABSENT_SUBSCRIBER_ID}"),
            body: None,
        },
        Call {
            case: "CreateSubscriber",
            method: "POST",
            path: "/v1/subscribers".to_string(),
            body: Some(subscriber_body("sub-one")),
        },
    ]
}

/// The `message` an error reply carries, or `None` when it carries none.
fn message(reply: &Reply) -> Option<&str> {
    reply.body["message"].as_str()
}

/// RULE: with Security Lake enabled in no Region, every subscriber-scoped call is refused
/// 404 `ResourceNotFoundException` carrying the enablement message — whatever the
/// operation, the caller's Region and the query parameters.
#[tokio::test]
async fn subscriber_calls_on_a_never_enabled_account_are_not_found() {
    // Rows span the Region the request is signed for as well as the call itself: the
    // precondition is about the ACCOUNT, so no Region may answer a success.
    let regions = ["us-east-1", "eu-west-2", "ap-southeast-2"];

    for region in regions {
        for call in subscriber_calls() {
            let emulator = Emulator::new();
            let case = format!("{} in {region}", call.case);
            let reply = emulator
                .send(call.method, &call.path, region, call.body.clone())
                .await;

            assert_eq!(
                reply.status, 404,
                "case {case:?}: expected 404, got {} with {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: expected the x-amzn-errortype header to name \
                 ResourceNotFoundException, got {:?}",
                reply.error_type
            );
            assert_eq!(
                message(&reply),
                Some(GATE_MESSAGE),
                "case {case:?}: expected the enablement message, got {:?}",
                reply.body
            );
        }
    }
}

/// RULE: enabling Security Lake in ONE Region gives the account a subscriber collection,
/// so the gate lifts for every Region — the read then answers 200 with an empty list, and
/// the single-resource calls report the resource that is missing rather than the
/// enablement remedy.
#[tokio::test]
async fn enabling_one_region_lifts_the_gate_for_every_region() {
    // Rows: the Region Security Lake is enabled in differs from the Region the calls are
    // signed for in two of the three.
    let cases: Vec<(&str, &str)> = vec![
        ("enabled and called in the same Region", "us-east-1"),
        ("enabled elsewhere, called in us-east-1", "us-east-1"),
        ("enabled elsewhere, called in ap-southeast-2", "ap-southeast-2"),
    ];
    let enabled_regions = ["us-east-1", "eu-west-2", "eu-west-2"];

    for ((case, calling_region), enabled_region) in cases.iter().zip(enabled_regions) {
        let emulator = Emulator::new();
        let created = emulator
            .create_data_lake(enabled_region, json!({ "region": enabled_region }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: enabling {enabled_region:?} got {} with {:?}",
            created.status, created.body
        );

        for call in subscriber_calls() {
            let row = format!("{case}: {}", call.case);
            let reply = emulator
                .send(call.method, &call.path, calling_region, call.body.clone())
                .await;

            assert_ne!(
                message(&reply),
                Some(GATE_MESSAGE),
                "case {row:?}: the account IS enabled, so the enablement message must not \
                 be served, got {} with {:?}",
                reply.status,
                reply.body
            );

            if call.case.starts_with("ListSubscribers") {
                assert_eq!(
                    reply.status, 200,
                    "case {row:?}: expected 200, got {} with {:?}",
                    reply.status, reply.body
                );
                assert!(
                    array(&reply, "subscribers", &row).is_empty(),
                    "case {row:?}: an enabled account with no subscribers lists none, got {:?}",
                    reply.body
                );
            }
        }
    }
}

/// RULE: the gate tracks the stored data lakes, so deleting the last one disables the
/// account again and the subscriber calls go back to 404 with the enablement message —
/// and the subscriber created while it was enabled is gone with it.
#[tokio::test]
async fn deleting_the_last_data_lake_puts_the_gate_back() {
    // Rows differ in how many Regions are enabled before the deletes, so the gate is seen
    // returning only once the LAST data lake goes.
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("one enabled Region", vec!["us-east-1"]),
        ("two enabled Regions", vec!["us-east-1", "eu-west-2"]),
        (
            "three enabled Regions",
            vec!["us-east-1", "eu-west-2", "ap-southeast-2"],
        ),
    ];

    for (case, regions) in &cases {
        let emulator = Emulator::new();
        for region in regions {
            let created = emulator
                .create_data_lake(region, json!({ "region": region }))
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: enabling {region:?} got {}",
                created.status
            );
        }

        let first = regions[0];
        let created = emulator
            .create_subscriber(first, subscriber_body("sub-one"))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: creating the subscriber got {} with {:?}",
            created.status, created.body
        );
        let subscriber_id = text(&created.body["subscriber"], "subscriberId", case).to_string();

        // Remove the data lakes one at a time: while ANY remains the account is still
        // enabled, so the read keeps answering 200.
        for (removed, region) in regions.iter().enumerate() {
            let deleted = emulator.delete_data_lake(region, &[region]).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: deleting the {region:?} data lake got {} with {:?}",
                deleted.status, deleted.body
            );

            let listed = emulator.list_subscribers(first, "").await;
            let last_one_gone = removed + 1 == regions.len();
            if last_one_gone {
                assert_eq!(
                    listed.status, 404,
                    "case {case:?}: with no data lake left the read must be 404, got {} with {:?}",
                    listed.status, listed.body
                );
                assert_eq!(
                    listed.error_type.as_deref(),
                    Some("ResourceNotFoundException"),
                    "case {case:?}: wrong error code, got {:?}",
                    listed.error_type
                );
                assert_eq!(
                    message(&listed),
                    Some(GATE_MESSAGE),
                    "case {case:?}: expected the enablement message, got {:?}",
                    listed.body
                );
            } else {
                assert_eq!(
                    listed.status, 200,
                    "case {case:?}: {} Region(s) are still enabled, so the read stays 200, got {} with {:?}",
                    regions.len() - removed - 1,
                    listed.status,
                    listed.body
                );
                // The subscriber went with its own Region's data lake on the first delete.
                let reported: Vec<String> = array(&listed, "subscribers", case)
                    .iter()
                    .map(|subscriber| text(subscriber, "subscriberId", case).to_string())
                    .collect();
                assert!(
                    !reported.contains(&subscriber_id),
                    "case {case:?}: the subscriber of the deleted Region must be gone, got {:?}",
                    listed.body
                );
            }
        }
    }
}
