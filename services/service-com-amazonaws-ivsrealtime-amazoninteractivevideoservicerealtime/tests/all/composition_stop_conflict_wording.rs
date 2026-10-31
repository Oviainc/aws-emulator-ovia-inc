// SPDX-License-Identifier: CC0-1.0

//! How `StopComposition` WORDS its refusal of a composition that cannot be stopped.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body.
//! What the stop DOES is covered by `tests/all/composition_stop.rs`; this suite is about
//! the sentence the refusal carries.
//!
//! The rule under test, exercised over a SET of inputs rather than on one request:
//!
//! * A STOP OF A COMPOSITION THAT IS NOT COMPOSING carries ONE fixed sentence, whose only
//!   variable part is the composition ARN the request sent: "Resource: <arn> is in a
//!   transitioning state". The sentence does not vary with the composition's own
//!   `CompositionState`, so no state value appears in it whichever non-composing state the
//!   stop found the composition in — a client that keys the "this composition can no
//!   longer be stopped" condition on the sentence matches every one of them.
//! * The refusal is the modelled `ConflictException` (HTTP 409) and changes nothing: the
//!   composition keeps the state and the end time it had.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The region every test in this suite signs for.
const REGION: &str = "us-east-1";

/// The span the service gives the `STOPPING` -> `STOPPED` step of a stop, read from the
/// crate so this suite cannot drift from the span the service actually enforces.
const STOP_SPAN: Duration = state::COMPOSITION_STOP_SPAN;

/// Added to a sleep, so a span this suite waits out has certainly elapsed whatever
/// scheduling delay the test host adds.
const SLACK: Duration = Duration::from_millis(400);

// The whole `CompositionState` value list, from
// `smithy_query(query="shape",shape="CompositionState")`:
//   STARTING | ACTIVE | STOPPING | FAILED | STOPPED
// A stop is the next thing that can happen to a composition only from the two states it
// reaches on the way up; `NOT_COMPOSING` below are the three it is refused from, and the
// full list is what the refusal's sentence is checked against for a leaked state value.
const COMPOSITION_STATES: [&str; 5] = ["STARTING", "ACTIVE", "STOPPING", "FAILED", "STOPPED"];
const STATE_STOPPING: &str = "STOPPING";
const STATE_STOPPED: &str = "STOPPED";
const STATE_FAILED: &str = "FAILED";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it, plus the engine handle a state the clock does not reach on its own is written
/// through.
struct RealTime {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs` in [`REGION`].
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

    /// Create a stage and answer its ARN.
    async fn stage(&self, case: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// Start a composition of `stage` and answer its ARN.
    async fn start(&self, case: &str, stage: &str) -> String {
        let (status, response) = self
            .call(
                "StartComposition",
                &json!({
                    "stageArn": stage,
                    "destinations": [{
                        "channel": { "channelArn": arn("channel", "live-channel") },
                    }],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartComposition must answer HTTP 200, got {status} {response}"
        );
        member(&response["composition"], "arn")
    }

    /// `StopComposition`, asserting the modelled empty success.
    async fn stop(&self, case: &str, composition: &str) {
        let (status, response) = self
            .call("StopComposition", &json!({ "arn": composition }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first StopComposition {composition} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// `GetComposition`, asserting the modelled success.
    async fn get(&self, case: &str, composition: &str) -> Value {
        let (status, response) = self
            .call("GetComposition", &json!({ "arn": composition }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetComposition {composition} must answer HTTP 200, \
             got {status} {response}"
        );
        response["composition"].clone()
    }

    /// Put `composition` into `composition_state` directly, for a state the clock reaches
    /// on its own from no call this service serves.
    fn force_state(&self, case: &str, composition: &str, composition_state: &str) {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let mut store = handle.lock();
        let record = store
            .compositions
            .get_mut(composition)
            .unwrap_or_else(|| panic!("case {case:?}: the store must hold {composition}"));
        record.state = composition_state.to_string();
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// A string member, read as the empty string when absent — what a client sees for a
/// member the wire omits.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An IVS ARN of `resource_type` in [`REGION`], naming the resource `id`.
fn arn(resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{REGION}:{ACCOUNT}:{resource_type}/{id}")
}

/// How a composition reaches a state a stop is refused from. The rows differ in the state
/// the stop finds — and with it in whether the composition carries an end time at all —
/// which is what the rule generalises over.
#[derive(Debug, Clone, Copy)]
enum HowItGotThere {
    /// Stopped, and read back inside the stop span: `STOPPING`.
    StopJustMade,
    /// Stopped, and read back after the span the clock owes the rest at: `STOPPED`.
    StopFinished,
    /// Never stopped, but no longer composing: `FAILED`.
    Failed,
}

/// Every later stop of a composition is refused with one fixed sentence that names the
/// ARN the request sent and no state of the record.
#[tokio::test]
async fn a_refused_stop_states_the_request_s_arn_and_no_composition_state() {
    let service = RealTime::new();
    let stage = service.stage("stop conflict wording").await;

    // Each row is one way of being past composing, with the `CompositionState` values the
    // read before the refused stop admits: a stop just made is `STOPPING` until the clock
    // owes `STOPPED` one [`STOP_SPAN`] later, so a read that straddles that boundary
    // admits either, and every other row admits exactly one.
    let rows: [(HowItGotThere, &[&str]); 3] = [
        (HowItGotThere::StopJustMade, &[STATE_STOPPING, STATE_STOPPED]),
        (HowItGotThere::StopFinished, &[STATE_STOPPED]),
        (HowItGotThere::Failed, &[STATE_FAILED]),
    ];

    let mut sentences: Vec<String> = Vec::new();
    for (how, admissible) in rows {
        let case = format!("{how:?} -> one of {admissible:?}");
        // Each row gets its OWN composition, so the ARN — the sentence's only variable
        // part — differs from every other row's.
        let composition = service.start(&case, &stage).await;
        match how {
            HowItGotThere::StopJustMade => service.stop(&case, &composition).await,
            HowItGotThere::StopFinished => {
                service.stop(&case, &composition).await;
                tokio::time::sleep(STOP_SPAN + SLACK).await;
            }
            HowItGotThere::Failed => service.force_state(&case, &composition, STATE_FAILED),
        }

        // What the composition looks like before the refused stop, so "the refusal
        // changed nothing" is asserted against the state and the end time it really had.
        let before = service.get(&case, &composition).await;
        let state_before = member(&before, "state");
        assert!(
            admissible.contains(&state_before.as_str()),
            "case {case:?}: the composition must be in one of {admissible:?} before the \
             refused stop, got {before}"
        );

        let (status, response) = service
            .call("StopComposition", &json!({ "arn": composition }))
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: a stop of a composition that is not composing must answer \
             HTTP 409, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected the modelled ConflictException, got {response}"
        );

        let message = member(&response, "message");
        let expected = format!("Resource: {composition} is in a transitioning state");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal to state {expected:?}, got {message:?}"
        );
        for leaked in COMPOSITION_STATES {
            assert!(
                !message.contains(leaked),
                "case {case:?}: the refusal must name no CompositionState value, \
                 found {leaked:?} in {message:?}"
            );
        }
        sentences.push(message);

        // The refusal has no effect: the composition keeps its end time, and its state
        // moves no further than the step the clock already owed it (`STOPPING` ->
        // `STOPPED`) — never back to a composing one and never to a second end.
        let after = service.get(&case, &composition).await;
        let state_after = member(&after, "state");
        let still_admissible: Vec<&str> = if state_before == STATE_STOPPING {
            vec![STATE_STOPPING, STATE_STOPPED]
        } else {
            vec![state_before.as_str()]
        };
        assert!(
            still_admissible.contains(&state_after.as_str()),
            "case {case:?}: the refused stop must leave the composition in one of \
             {still_admissible:?}, got {after}"
        );
        assert_eq!(
            after.get("endTime"),
            before.get("endTime"),
            "case {case:?}: the refused stop must not move endTime, got {} against {}",
            after.get("endTime").unwrap_or(&Value::Null),
            before.get("endTime").unwrap_or(&Value::Null)
        );
    }

    // One template, filled from the request: the rows named different compositions in
    // different states, so the sentences differ exactly where the ARNs do.
    sentences.sort();
    let distinct = {
        let mut unique = sentences.clone();
        unique.dedup();
        unique.len()
    };
    assert_eq!(
        distinct,
        rows.len(),
        "case {sentences:?}: each refusal must carry its own request's ARN, so the \
         {} sentences must be distinct, got {distinct} distinct",
        rows.len()
    );
}
