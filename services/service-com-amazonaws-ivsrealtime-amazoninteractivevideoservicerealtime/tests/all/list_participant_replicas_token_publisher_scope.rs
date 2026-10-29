// SPDX-License-Identifier: CC0-1.0

//! A `ListParticipantReplicas` `nextToken` names a position among the replicas of ONE
//! publisher of ONE source stage, so only that same publisher's listing can resume it.
//!
//! The rule, over a set of inputs rather than on one request: a page position is an ordinal
//! among the records the resuming request's own scope selects — here the region, the source
//! stage and the PUBLISHER whose replicas the page held — so a token offered under any
//! other scope names no position among the records that call would page, and is refused
//! with the modelled `ValidationException` (HTTP 400) carrying this service's one
//! foreign-token sentence. It is never served as a page that silently omits the replicas
//! lying before the position it names.
//!
//! `tests/all/page_token_scope.rs` pins that rule across every listing of this service,
//! varying the region and the stage/session identity terms. The rows here vary the term
//! those rows do not reach for this listing: the `participantId`, with the `sourceStageArn`
//! and the region held fixed, in BOTH directions — the resuming publisher's replicas lying
//! BEFORE the position the token names (the direction under which an unscoped token turns
//! into a short or empty page) and AFTER it — and for a `participantId` the store holds no
//! replica of at all.
//!
//! Replica positions come off the store-wide counter in `StartParticipantReplication`, so
//! the fixture starts the publishers' replications in a fixed order and each row knows
//! whether its resuming publisher's replicas sit below or above the issuing publisher's.
//!
//! Every assertion is made over the router's own `dispatch`, so each is about what an SDK
//! client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The region every call of this suite is signed for.
const REGION: &str = "us-east-1";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches it.
struct RealTime {
    router: MultiServiceRouter,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Pin authentication off so the suite does not depend on the process setting.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential scope
    /// naming `ivs` in [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.{REGION}.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// A stage, created the way a client creates one, reported by its ARN.
    async fn stage(&self, name: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({ "name": name })).await;
        assert_eq!(
            status, 200,
            "fixture CreateStage {name:?}: expected HTTP 200, got {status} {response}"
        );
        text(&response["stage"], "arn")
    }

    /// A publisher of `stage_arn`: the participant of a token, which is where the model says
    /// a `participantId` comes from, with the default capabilities (`PUBLISH, SUBSCRIBE`).
    async fn publisher(&self, stage_arn: &str) -> String {
        let (status, response) = self
            .call("CreateParticipantToken", &json!({ "stageArn": stage_arn }))
            .await;
        assert_eq!(
            status, 200,
            "fixture CreateParticipantToken for {stage_arn}: expected HTTP 200, \
             got {status} {response}"
        );
        text(&response["participantToken"], "participantId")
    }

    /// Replicate the publisher `participant_id` of `source` into `destination`.
    async fn replicate(&self, source: &str, participant_id: &str, destination: &str) {
        let request = json!({
            "sourceStageArn": source,
            "destinationStageArn": destination,
            "participantId": participant_id,
        });
        let (status, response) = self.call("StartParticipantReplication", &request).await;
        assert_eq!(
            status, 200,
            "fixture StartParticipantReplication {request}: expected HTTP 200, \
             got {status} {response}"
        );
    }

    /// One `ListParticipantReplicas` page for one publisher of one source stage, with any
    /// extra request members (`maxResults`, `nextToken`) merged in. Answers the status and
    /// the body unchanged.
    async fn list(&self, source: &str, participant_id: &str, extra: &Value) -> (u16, Value) {
        let mut request = json!({
            "sourceStageArn": source,
            "participantId": participant_id,
        });
        if let Some(extra) = extra.as_object() {
            let target = request
                .as_object_mut()
                .expect("the request under construction is an object");
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        self.call("ListParticipantReplicas", &request).await
    }

    /// The destination stages one served page reports, in page order — the identity of a
    /// replica inside one publisher's listing.
    async fn page(
        &self,
        case: &str,
        source: &str,
        participant_id: &str,
        extra: &Value,
    ) -> Vec<String> {
        let (status, response) = self.list(source, participant_id, extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListParticipantReplicas with {extra} must answer HTTP 200, \
             got {status} {response}"
        );
        response["replicas"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: replicas must be a list, got {response}"))
            .iter()
            .map(|replica| text(replica, "destinationStageArn"))
            .collect()
    }
}

/// A string member of a response, or a marker so a failing assertion names the missing
/// member rather than panicking inside the reader.
fn text(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
        .to_string()
}

/// The sentence every listing of this service refuses a token naming no position in it
/// with — the service's one wording, kept here as a client reads it.
fn foreign_page_token_message(operation: &str) -> String {
    format!("{operation}.nextToken is required to be a pagination token issued by {operation}")
}

/// One publisher of the fixture's source stage: the id it publishes under, and the
/// destination stages its replications were started into, in start order.
struct Publisher {
    participant_id: String,
    destinations: Vec<String>,
}

/// One row: the publisher whose page issues the token, and the publisher of the SAME source
/// stage the token is then offered to, with what that second listing holds of its own.
struct Row {
    case: &'static str,
    issuer: usize,
    resuming: String,
    resuming_holds: Vec<String>,
}

/// A publisher of the fixture's source stage with `replicas` replications started for it,
/// each into a destination stage of its own named after `name`.
async fn build_publisher(
    service: &RealTime,
    source: &str,
    name: &str,
    replicas: usize,
) -> Publisher {
    let participant_id = service.publisher(source).await;
    let mut destinations = Vec::new();
    for index in 0..replicas {
        let destination = service.stage(&format!("{name}-destination-{index}")).await;
        service
            .replicate(source, &participant_id, &destination)
            .await;
        destinations.push(destination);
    }
    Publisher {
        participant_id,
        destinations,
    }
}

/// A token one publisher's page issued names no position among ANOTHER publisher's replicas
/// of the same source stage, so that listing refuses it — and goes on resuming it for the
/// publisher that issued it.
#[tokio::test]
async fn a_replica_token_resumes_only_the_publisher_that_issued_it() {
    let service = RealTime::new();
    let source = service.stage("source").await;

    // Replications are started publisher by publisher, so each publisher's replicas occupy
    // a contiguous band of store positions: `first`'s lie below `second`'s, which lie below
    // `last`'s. `first` and `second` hold more than one replica each, so either can issue a
    // page token; `last` holds one, so it can only be resumed against.
    let first = build_publisher(&service, &source, "first", 2).await;
    let second = build_publisher(&service, &source, "second", 3).await;
    let last = build_publisher(&service, &source, "last", 1).await;
    let publishers = [&first, &second, &last];

    // A well-formed `ParticipantId` (`@length 0..64`, `@pattern ^[a-zA-Z0-9-_]*$`, from
    // `smithy_query(query="operation",operation="ListParticipantReplicas")`) that this store
    // holds no replica of: its own listing is an empty page, so only the token tells the
    // refusal from a short page.
    let unknown = "no-such-publisher".to_string();

    // The rows differ along the term this listing's own scope adds to the region and the
    // stage: which publisher resumes the token, and whether that publisher's replicas lie
    // before the position the token names (so an unscoped token would answer a short or
    // empty page), after it, or do not exist.
    let rows = vec![
        Row {
            case: "a sibling publisher whose replicas all lie BEFORE the position",
            issuer: 1,
            resuming: first.participant_id.clone(),
            resuming_holds: first.destinations.clone(),
        },
        Row {
            case: "a sibling publisher whose replicas all lie AFTER the position",
            issuer: 0,
            resuming: second.participant_id.clone(),
            resuming_holds: second.destinations.clone(),
        },
        Row {
            case: "a sibling publisher holding a single replica, lying AFTER the position",
            issuer: 0,
            resuming: last.participant_id.clone(),
            resuming_holds: last.destinations.clone(),
        },
        Row {
            case: "a sibling publisher holding a single replica, lying BEFORE the position",
            issuer: 1,
            resuming: last.participant_id.clone(),
            resuming_holds: last.destinations.clone(),
        },
        Row {
            case: "a publisher of this stage the store holds no replica of",
            issuer: 1,
            resuming: unknown.clone(),
            resuming_holds: Vec::new(),
        },
    ];

    for row in rows {
        let case = row.case;
        let issuer = publishers[row.issuer];
        assert!(
            issuer.destinations.len() > 1,
            "case {case:?}: the issuing publisher must hold more than one replica for a page \
             of one to leave any behind, got {:?}",
            issuer.destinations
        );

        // What the resuming publisher's listing holds before any token is in play.
        let before = service
            .page(case, &source, &row.resuming, &json!({}))
            .await;
        assert_eq!(
            before, row.resuming_holds,
            "case {case:?}: expected the resuming publisher to hold {:?}, got {before:?}",
            row.resuming_holds
        );

        // A page of one for the issuing publisher, and the token it ends with.
        let (status, page) = service
            .list(&source, &issuer.participant_id, &json!({ "maxResults": 1 }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a page of one must be served, got {status} {page}"
        );
        let token = text(&page, "nextToken");
        assert!(
            token != "<absent>" && !token.is_empty(),
            "case {case:?}: a page of one with replicas left over must carry a nextToken, \
             got {page}"
        );

        // Inside the publisher that issued it, the token resumes exactly where that page
        // ended.
        let resumed = service
            .page(
                case,
                &source,
                &issuer.participant_id,
                &json!({ "nextToken": token }),
            )
            .await;
        assert_eq!(
            resumed,
            issuer.destinations[1..].to_vec(),
            "case {case:?}: the issuing publisher must resume after the replica its page \
             ended at, expected {:?}, got {resumed:?}",
            &issuer.destinations[1..]
        );

        // Offered to ANOTHER publisher of the same source stage, it names no position among
        // the replicas that call pages, so it is refused — not served as a short or empty
        // page.
        let (status, refused) = service
            .list(&source, &row.resuming, &json!({ "nextToken": token }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a token of another publisher must be refused, got {status} {refused}"
        );
        assert_eq!(
            text(&refused, "Code"),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {refused}"
        );
        assert_eq!(
            text(&refused, "Message"),
            foreign_page_token_message("ListParticipantReplicas"),
            "case {case:?}: expected this service's one foreign-token sentence, got {refused}"
        );
        assert!(
            refused.get("replicas").is_none(),
            "case {case:?}: a refused page must report no replicas, got {refused}"
        );

        // And the refusal changed nothing a later caller sees.
        let after = service
            .page(case, &source, &row.resuming, &json!({}))
            .await;
        assert_eq!(
            after, before,
            "case {case:?}: the refusal must leave the resuming publisher's replicas as they \
             were, expected {before:?}, got {after:?}"
        );
    }
}
