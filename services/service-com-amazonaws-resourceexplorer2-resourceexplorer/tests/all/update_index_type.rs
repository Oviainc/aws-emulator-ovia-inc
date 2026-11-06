// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UpdateIndexType`, the operation that switches an index
//! between the two values the model's `IndexType` enum admits.
//!
//! The suite asserts the rule rather than one switch: every ordered pair of index types
//! is driven through the operation, in different Regions, and each one is read back
//! through `GetIndex` — so a type the handler reports is only accepted when the store
//! really holds it. The account-wide rules the operation documents ("You can have only
//! one aggregator index per Amazon Web Services account", and the wait before another
//! index may take a demoted aggregator's place) are exercised over several Regions, with
//! the refused call shown to leave the index as it was.
//!
//! Every assertion goes over the wire: the suite mounts the service on a
//! [`MultiServiceRouter`] over ONE shared engine and dispatches restJson1 requests at it.
//! The Region of a request is the Region of its SigV4 credential scope, which is why
//! every call carries an `Authorization` header — and why one row signs the call for a
//! Region OTHER than the index's, since `UpdateIndexType` names its index by ARN.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Comfortably past `crate::state::INDEX_SETTLE_DELAY`, so an index left in `CREATING`,
/// `UPDATING` or `DELETING` is certain to have settled by the next call.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// One emulator: the service mounted on a router over a single shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

/// What one call answered: the HTTP status, the restJson1 error code (the
/// `x-amzn-errortype` header), and the decoded JSON body.
struct Answer {
    status: u16,
    error: Option<String>,
    body: Value,
}

impl Answer {
    /// The body of a call that had to succeed.
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} ({:?}) with body {}",
            self.status, self.error, self.body
        );
        &self.body
    }

    /// Assert this call failed with `status` and the error code `code`.
    fn failed(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error.as_deref()),
            (status, Some(code)),
            "case {case:?}: expected HTTP {status} {code}, got {} ({:?}) with body {}",
            self.status,
            self.error,
            self.body
        );
    }

    /// The error's `Message` member, however the envelope cases it.
    fn error_message(&self) -> String {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_resourceexplorer2_resourceexplorer::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// `POST /{operation}` as a client signed for `region` would, with `body` when the
    /// operation has input members and no body at all when it does not.
    async fn send(&self, region: &str, operation: &str, body: Option<Value>) -> Answer {
        let payload = match body {
            Some(value) => serde_json::to_vec(&value).expect("the request body serializes"),
            None => Vec::new(),
        };
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://resource-explorer-2.{region}.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}\
                     /resource-explorer-2/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error,
            body,
        }
    }

    async fn call(&self, region: &str, operation: &str, body: Value) -> Answer {
        self.send(region, operation, Some(body)).await
    }

    /// `POST /GetIndex`, carrying NO body: the operation's input is `smithy.api#Unit`.
    async fn get_index(&self, region: &str) -> Answer {
        self.send(region, "GetIndex", None).await
    }

    /// `POST /UpdateIndexType`, signed for `signing_region`, naming `arn`.
    async fn update_type(&self, signing_region: &str, arn: &str, index_type: &str) -> Answer {
        self.call(
            signing_region,
            "UpdateIndexType",
            json!({ "Arn": arn, "Type": index_type }),
        )
        .await
    }
}

/// `CreateIndex` in `region`, asserting success, and the ARN of the index it minted.
async fn create_index(emulator: &Emulator, region: &str) -> String {
    let answer = emulator
        .call(region, "CreateIndex", json!({ "ClientToken": region }))
        .await;
    answer.ok(region)["Arn"]
        .as_str()
        .expect("CreateIndex answers an Arn")
        .to_string()
}

/// An ARN in this service's own namespace: the emulator's own account, with whatever
/// resource segment the caller names.
fn own_arn(region: &str, resource: &str) -> String {
    engine::emulator_arn("resource-explorer-2", region, resource)
}

/// A timestamp member as seconds since the epoch, for comparing two answers' instants.
fn epoch(case: &str, value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected an epoch timestamp, got {value}"))
}

/// Wait out the transient index state, then report what `GetIndex` says.
async fn settled_index(emulator: &Emulator, region: &str) -> Value {
    tokio::time::sleep(PAST_SETTLE).await;
    let answer = emulator.get_index(region).await;
    answer.ok(&format!("{region} settled")).clone()
}

/// One row of the switch table: the type the index starts at, the type the call asks
/// for, the Region the index lives in, and the Region the call is SIGNED for.
struct Switch {
    from: &'static str,
    to: &'static str,
    region: &'static str,
    signed_for: &'static str,
}

/// Every ordered pair of the two values the model's `IndexType` enum admits
/// (`smithy_query(query="shape", shape="IndexType")`: LOCAL, AGGREGATOR), each in a
/// different Region. The last row signs the call for a Region that is NOT the index's,
/// because the operation names its index by ARN and an ARN carries its own Region.
const SWITCHES: &[Switch] = &[
    Switch {
        from: "LOCAL",
        to: "AGGREGATOR",
        region: "us-east-1",
        signed_for: "us-east-1",
    },
    Switch {
        from: "AGGREGATOR",
        to: "LOCAL",
        region: "eu-west-1",
        signed_for: "eu-west-1",
    },
    Switch {
        from: "LOCAL",
        to: "LOCAL",
        region: "ap-southeast-2",
        signed_for: "ap-southeast-2",
    },
    Switch {
        from: "AGGREGATOR",
        to: "AGGREGATOR",
        region: "eu-central-1",
        signed_for: "us-west-2",
    },
];

/// `UpdateIndexType` stores the type the request asks for and reports the index
/// `UPDATING` until the clock settles it back to `ACTIVE`; a request for the type the
/// index already has changes nothing.
///
/// Each row runs on its own emulator, so one row's aggregator index cannot stand in the
/// way of the next row's promotion.
#[tokio::test]
async fn update_index_type_switches_an_index_between_every_modelled_type_pair() {
    for switch in SWITCHES {
        let case = format!(
            "{} -> {} in {} signed for {}",
            switch.from, switch.to, switch.region, switch.signed_for
        );
        let emulator = Emulator::new();
        let arn = create_index(&emulator, switch.region).await;

        // Bring the index to the row's starting type. `CreateIndex` "creates only a
        // local index", so a row that starts at AGGREGATOR gets there through this same
        // operation.
        if switch.from != "LOCAL" {
            let promoted = emulator
                .update_type(switch.region, &arn, switch.from)
                .await;
            promoted.ok(&format!("{case}: reaching {}", switch.from));
        }
        let before = settled_index(&emulator, switch.region).await;
        assert_eq!(
            (before["Type"].as_str(), before["State"].as_str()),
            (Some(switch.from), Some("ACTIVE")),
            "case {case:?}: the row starts from a settled {} index, got {before}",
            switch.from
        );
        let last_updated_before = epoch(&case, &before["LastUpdatedAt"]);

        let answer = emulator
            .update_type(switch.signed_for, &arn, switch.to)
            .await;
        let body = answer.ok(&case).clone();
        let changed = switch.from != switch.to;
        let expected_state = if changed { "UPDATING" } else { "ACTIVE" };
        assert_eq!(
            body["Arn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: the answer names the index the request named, got {body}"
        );
        assert_eq!(
            body["Type"].as_str(),
            Some(switch.to),
            "case {case:?}: Type is the type the index has after the operation \
             completes, got {body}"
        );
        assert_eq!(
            body["State"].as_str(),
            Some(expected_state),
            "case {case:?}: a type change is asynchronous and reports {expected_state}, \
             got {body}"
        );
        let last_updated = epoch(&case, &body["LastUpdatedAt"]);
        if changed {
            assert!(
                last_updated >= last_updated_before,
                "case {case:?}: a switch updates the index, so LastUpdatedAt cannot go \
                 backwards: {last_updated} < {last_updated_before}"
            );
        } else {
            assert_eq!(
                last_updated, last_updated_before,
                "case {case:?}: asking for the type the index already has changes \
                 nothing, so LastUpdatedAt stays where it was"
            );
        }

        // Read back through GetIndex: the type the answer reported is the type the store
        // holds, and the transient state is observable to a caller that polls.
        let polled = emulator.get_index(switch.region).await;
        let polled_body = polled.ok(&format!("{case}: GetIndex")).clone();
        assert_eq!(
            (polled_body["Type"].as_str(), polled_body["State"].as_str()),
            (Some(switch.to), Some(expected_state)),
            "case {case:?}: GetIndex reports the stored type and state, got {polled_body}"
        );

        // "When the asynchronous tasks complete… the Status response of that operation
        // changes from UPDATING to ACTIVE."
        let after = settled_index(&emulator, switch.region).await;
        assert_eq!(
            (after["Type"].as_str(), after["State"].as_str()),
            (Some(switch.to), Some("ACTIVE")),
            "case {case:?}: the settled index keeps its new type and is ACTIVE, got {after}"
        );
    }
}

/// The account holds at most one aggregator index: promoting a second one is the
/// modelled `ConflictException`, and the Region that already holds one is reported as a
/// replication peer by `GetIndex` in every Region.
#[tokio::test]
async fn only_one_index_per_account_can_be_the_aggregator() {
    let emulator = Emulator::new();
    let regions = ["us-east-1", "eu-west-1", "ap-northeast-1"];
    for region in regions {
        create_index(&emulator, region).await;
    }
    let aggregator_arn = emulator
        .get_index(regions[0])
        .await
        .ok(regions[0])["Arn"]
        .as_str()
        .expect("GetIndex answers an Arn")
        .to_string();

    let promoted = emulator
        .update_type(regions[0], &aggregator_arn, "AGGREGATOR")
        .await;
    assert_eq!(
        promoted.ok("first promotion")["Type"].as_str(),
        Some("AGGREGATOR"),
        "case {:?}: the first promotion succeeds, got {}",
        "first promotion",
        promoted.body
    );
    tokio::time::sleep(PAST_SETTLE).await;

    // Every OTHER Region's index is refused the promotion, with the account's rule in the
    // message, and is left exactly as it was.
    for region in &regions[1..] {
        let case = format!("second aggregator in {region}");
        let arn = emulator.get_index(region).await.ok(region)["Arn"]
            .as_str()
            .expect("GetIndex answers an Arn")
            .to_string();
        let refused = emulator.update_type(region, &arn, "AGGREGATOR").await;
        refused.failed(&case, 409, "ConflictException");
        let message = refused.error_message();
        assert!(
            message.contains("AGGREGATOR") && message.contains("LOCAL"),
            "case {case:?}: the refusal states the account's one-aggregator rule and how \
             to make room, got {message:?}"
        );
        let unchanged = emulator.get_index(region).await.ok(&case).clone();
        assert_eq!(
            (unchanged["Type"].as_str(), unchanged["State"].as_str()),
            (Some("LOCAL"), Some("ACTIVE")),
            "case {case:?}: a refused promotion writes nothing, got {unchanged}"
        );
        // A LOCAL index replicates its content to the account's aggregator Region, and
        // reports only that Region.
        assert_eq!(
            unchanged["ReplicatingTo"],
            json!([regions[0]]),
            "case {case:?}: a local index reports the aggregator's Region, got {unchanged}"
        );
    }

    // The aggregator reports the Regions that replicate into it, in order.
    let aggregator = emulator.get_index(regions[0]).await.ok("aggregator").clone();
    let mut peers: Vec<&str> = regions[1..].to_vec();
    peers.sort_unstable();
    assert_eq!(
        aggregator["ReplicatingFrom"],
        json!(peers),
        "case {:?}: the aggregator reports its replication peers, got {aggregator}",
        "aggregator"
    );
    assert!(
        aggregator.get("ReplicatingTo").is_none(),
        "case {:?}: ReplicatingTo is present only on a local index, got {aggregator}",
        "aggregator"
    );
}

/// After the aggregator index is demoted, "you must wait 24 hours before you can promote
/// another index to be the new aggregator index for the account" — while the index that
/// was demoted is not another index, so it can take the role straight back.
#[tokio::test]
async fn promoting_another_index_waits_out_the_demoted_aggregator() {
    let emulator = Emulator::new();
    let regions = ["us-east-1", "eu-west-1"];
    let mut arns = Vec::new();
    for region in regions {
        arns.push(create_index(&emulator, region).await);
    }
    emulator
        .update_type(regions[0], &arns[0], "AGGREGATOR")
        .await
        .ok("promote the first index");
    tokio::time::sleep(PAST_SETTLE).await;
    let demoted = emulator.update_type(regions[0], &arns[0], "LOCAL").await;
    assert_eq!(
        demoted.ok("demote the aggregator")["Type"].as_str(),
        Some("LOCAL"),
        "case {:?}: the demotion stores LOCAL, got {}",
        "demote the aggregator",
        demoted.body
    );
    tokio::time::sleep(PAST_SETTLE).await;

    let case = "another index inside the wait";
    let refused = emulator.update_type(regions[1], &arns[1], "AGGREGATOR").await;
    refused.failed(case, 409, "ConflictException");
    assert!(
        refused.error_message().contains("24 hours"),
        "case {case:?}: the refusal states the wait the operation documents, got {:?}",
        refused.error_message()
    );
    let unchanged = emulator.get_index(regions[1]).await.ok(case).clone();
    assert_eq!(
        unchanged["Type"].as_str(),
        Some("LOCAL"),
        "case {case:?}: the refused index keeps its type, got {unchanged}"
    );

    // The index that was demoted is not "another index": it can be promoted again.
    let repromoted = emulator
        .update_type(regions[0], &arns[0], "AGGREGATOR")
        .await;
    assert_eq!(
        repromoted.ok("re-promote the demoted index")["Type"].as_str(),
        Some("AGGREGATOR"),
        "case {:?}: the demoted index can take the role straight back, got {}",
        "re-promote the demoted index",
        repromoted.body
    );
}

/// An `Arn` this account holds no index for is the modelled `ResourceNotFoundException`,
/// a `Type` outside the modelled enum is the modelled `ValidationException`, and an index
/// on its way out has no type to switch — none of which writes anything.
#[tokio::test]
async fn update_index_type_refuses_what_it_cannot_switch() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let arn = create_index(&emulator, region).await;
    let before = settled_index(&emulator, region).await;

    // Values that name no index this account holds: an index id nothing minted, an index
    // in a Region with no index at all, a view ARN, another service's resource, and a
    // value that is not an ARN.
    let missing = [
        own_arn(region, "index/11111111-2222-4333-8444-555555555555"),
        own_arn("eu-west-1", "index/11111111-2222-4333-8444-555555555555"),
        own_arn(region, "view/absent/11111111-2222-4333-8444-555555555555"),
        engine::emulator_arn("s3", region, "bucket-that-is-not-an-index"),
        "not-an-arn".to_string(),
    ];
    for candidate in &missing {
        let case = format!("missing {candidate}");
        let refused = emulator.update_type(region, candidate, "AGGREGATOR").await;
        refused.failed(&case, 404, "ResourceNotFoundException");
        assert!(
            refused.error_message().contains(candidate),
            "case {case:?}: the refusal names what the request named, got {:?}",
            refused.error_message()
        );
    }

    // Values outside the modelled `IndexType` enum: its two values differ in case and in
    // spelling from each of these.
    for candidate in ["local", "aggregator", "GLOBAL", "LOCAL ", ""] {
        let case = format!("type {candidate:?}");
        emulator
            .update_type(region, &arn, candidate)
            .await
            .failed(&case, 400, "ValidationException");
    }

    // Not one of those refusals touched the index.
    let after = emulator.get_index(region).await.ok("after refusals").clone();
    assert_eq!(
        (
            after["Type"].as_str(),
            after["State"].as_str(),
            after["LastUpdatedAt"].clone()
        ),
        (
            Some("LOCAL"),
            Some("ACTIVE"),
            before["LastUpdatedAt"].clone()
        ),
        "case {:?}: a refused call leaves the index exactly as it was, got {after}",
        "after refusals"
    );

    // An index being torn down has no type left to switch.
    emulator
        .call(region, "DeleteIndex", json!({ "Arn": arn }))
        .await
        .ok("DeleteIndex");
    let case = "deleting index";
    let refused = emulator.update_type(region, &arn, "AGGREGATOR").await;
    refused.failed(case, 409, "ConflictException");
    assert!(
        refused.error_message().contains("DELETING"),
        "case {case:?}: the refusal names the state the index is in, got {:?}",
        refused.error_message()
    );
}
