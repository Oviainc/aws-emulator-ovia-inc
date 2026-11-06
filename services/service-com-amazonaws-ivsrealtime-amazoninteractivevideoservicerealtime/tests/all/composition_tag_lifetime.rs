// SPDX-License-Identifier: CC0-1.0

//! The tagging surface over a composition's EPHEMERAL lifetime.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call.
//!
//! The rule under test, exercised over a SET of inputs rather than on one request: a
//! composition's position in its lifecycle is decided by the instants its start and its
//! stop recorded, so every reader of it answers the same at the same instant and no
//! unrelated call in between can change that answer. Concretely, for a composition the
//! clock has reclaimed (the model's `StartComposition` doc: the "Composition stops and the
//! resource is deleted … When StopComposition is called"), `/tags/{resourceArn}` answers a
//! 404 exactly when `GetComposition` does
//! — whether or not any call has touched the store since the stop, and however many times
//! the same request is repeated. Each surface answers the 404 with its OWN error code: the
//! tagging surface's `NotFoundException`, and the modelled `ResourceNotFoundException` for
//! the resource read.
//!
//! The rows differ along what the rule generalises over: HOW LONG after the stop the read
//! is made (inside the stop span, once the stop has finished, past the retention of the
//! stopped record, and long past it), WHETHER an unrelated call ran in between, WHICH
//! reader is asked first, and the tag map the composition carries.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The account id every ARN this emulator mints carries, and so the account every ARN
/// built by hand here carries too (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The region every test in this suite signs for.
const REGION: &str = "us-east-1";

/// The span the service gives the `STOPPING` -> `STOPPED` step of a stop, read from the
/// crate so this suite cannot drift from the span the service actually enforces.
const STOP_SPAN: Duration = state::COMPOSITION_STOP_SPAN;

/// How long the service goes on reporting a stopped composition before the ephemeral
/// resource is deleted, read from the crate for the same reason.
const RETENTION: Duration = state::COMPOSITION_RETENTION_SPAN;

// The `CompositionState` values this suite asserts, from
// `smithy_query(query="shape",shape="CompositionState")`:
//   STARTING | ACTIVE | STOPPING | FAILED | STOPPED
const STATE_STOPPING: &str = "STOPPING";
const STATE_STOPPED: &str = "STOPPED";

/// The modelled error a read of a composition the clock has reclaimed is answered with.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The error code the TAGGING surface answers the same reclaim with — its own, which is
/// not the modelled one the resource read above answers (see `tagging.rs`).
const TAGGING_NOT_FOUND: &str = "NotFoundException";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it, plus the engine handle the clock-driven expiry of a stopped record is run out
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

    /// An operation bound to `POST /<Operation>`, which is every operation of this service
    /// except the three tagging ones.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.send(
            "POST",
            &format!("/{operation}"),
            Some(bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    /// One of the three operations bound to `/tags/{resourceArn}`, carrying the ARN in the
    /// URI the way a client does: percent-encoded into one path segment.
    async fn tagging(
        &self,
        method: &str,
        arn: &str,
        query: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        self.send(
            method,
            &format!("/tags/{}{query}", encode_path_label(arn)),
            body.map(|body| bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    /// `ListTagsForResource`, bound to `GET /tags/{resourceArn}`.
    async fn list_tags(&self, arn: &str) -> (u16, Value) {
        self.tagging("GET", arn, "", None).await
    }

    async fn send(&self, method: &str, path: &str, body: Option<bytes::Bytes>) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://ivsrealtime.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(body.unwrap_or_default())
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
        let (status, response) = self.call("CreateStage", &json!({ "name": "composited" })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// `StartComposition` of `stage`, carrying `tags`, asserting the modelled success and
    /// answering with the ARN the service minted.
    async fn start(&self, case: &str, stage: &str, tags: &Value) -> String {
        let body = json!({
            "stageArn": stage,
            "destinations": [{ "channel": { "channelArn": arn("channel", "live-channel") } }],
            "tags": tags,
        });
        let (status, response) = self.call("StartComposition", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartComposition {body} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["composition"], "arn")
    }

    /// `StopComposition`, asserting the modelled empty success, and answering with the
    /// instant the stop was made.
    async fn stop(&self, case: &str, composition_arn: &str) -> Instant {
        let made = Instant::now();
        let (status, response) = self
            .call("StopComposition", &json!({ "arn": composition_arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: StopComposition {composition_arn} must answer HTTP 200, \
             got {status} {response}"
        );
        made
    }

    /// `GetComposition` without asserting the read succeeded — a reclaimed composition is
    /// read here too, and that read is a 404.
    async fn read(&self, composition_arn: &str) -> (u16, Value) {
        self.call("GetComposition", &json!({ "arn": composition_arn }))
            .await
    }

    /// Move the end the service recorded for `composition_arn` one whole [`RETENTION`] into
    /// the past, so what the service does when a stopped composition's record expires is
    /// observable without waiting that span out.
    ///
    /// Written straight onto the store rather than through an operation, so no call of this
    /// service has touched it: whichever reader is asked first is still the one that has to
    /// answer for the reclaim.
    fn run_out_the_retention(&self, case: &str, composition_arn: &str) {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let mut store = handle.lock();
        let composition = store
            .compositions
            .get_mut(composition_arn)
            .unwrap_or_else(|| panic!("case {case:?}: the store must hold {composition_arn}"));
        let ended_at = composition.ended_at.unwrap_or_else(|| {
            panic!("case {case:?}: {composition_arn} must already carry the end its stop recorded")
        });
        composition.ended_at = Some(ended_at - (RETENTION + STOP_SPAN));
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> String {
    member(response, "__type")
}

/// A string member, read as the empty string when absent — what a client sees for a member
/// the wire omits.
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

/// Percent-encode one `@httpLabel` value the way an SDK encodes a non-greedy label: an
/// ARN's `:` and `/` are escaped, so the whole ARN is ONE path segment.
fn encode_path_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// A tag map, as a request carries it.
fn tags(entries: &[(&str, &str)]) -> Value {
    Value::Object(
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect(),
    )
}

/// One row of the lifetime table.
struct Row {
    /// What this row is probing, named in every assertion it makes.
    case: &'static str,
    /// How long after the stop the reads are made.
    waited: Duration,
    /// Whether the retention of the stopped record is run out before the reads, so the
    /// clock has reclaimed the ephemeral resource by the time either reader is asked.
    retention_run_out: bool,
    /// Whether an unrelated call — one that holds the store mutably, and so materialises
    /// everything the clock owes — runs between the stop and the reads.
    unrelated_call_in_between: bool,
    /// Whether the tagging surface is asked BEFORE `GetComposition` or after it: neither
    /// reader may be the one that decides the other's answer.
    tagging_first: bool,
    /// The tags the composition carries, so the round trip is pinned over the map's size
    /// too.
    tags: Value,
}

/// The rows the rule is asserted over.
fn rows() -> Vec<Row> {
    let plain = tags(&[]);
    let one = tags(&[("env", "test")]);
    let several = tags(&[("env", "test"), ("team", "media"), ("note", "")]);
    vec![
        Row {
            case: "inside the stop span, nothing called in between, tags read first",
            waited: Duration::from_millis(1),
            retention_run_out: false,
            unrelated_call_in_between: false,
            tagging_first: true,
            tags: one.clone(),
        },
        Row {
            case: "inside the stop span, an unrelated call in between, composition read \
                   first",
            waited: Duration::from_millis(1),
            retention_run_out: false,
            unrelated_call_in_between: true,
            tagging_first: false,
            tags: several.clone(),
        },
        Row {
            case: "once the stop has finished, nothing called in between, tags read first",
            waited: STOP_SPAN + Duration::from_millis(200),
            retention_run_out: false,
            unrelated_call_in_between: false,
            tagging_first: true,
            tags: several,
        },
        Row {
            case: "once the stop has finished, an unrelated call in between, composition \
                   read first",
            waited: STOP_SPAN + Duration::from_millis(200),
            retention_run_out: false,
            unrelated_call_in_between: true,
            tagging_first: false,
            tags: plain,
        },
        Row {
            case: "past the retention of the stopped record, nothing called in between, \
                   tags read first",
            waited: STOP_SPAN + Duration::from_millis(200),
            retention_run_out: true,
            unrelated_call_in_between: false,
            tagging_first: true,
            tags: one.clone(),
        },
        Row {
            case: "past the retention of the stopped record, an unrelated call in between, \
                   composition read first",
            waited: STOP_SPAN + Duration::from_millis(200),
            retention_run_out: true,
            unrelated_call_in_between: true,
            tagging_first: false,
            tags: one.clone(),
        },
        Row {
            case: "long past the retention of the stopped record, nothing called in \
                   between, tags read first",
            waited: STOP_SPAN * 4,
            retention_run_out: true,
            unrelated_call_in_between: false,
            tagging_first: true,
            tags: one,
        },
    ]
}

/// A composition the clock has reclaimed is gone for the tagging surface exactly as it is
/// gone for `GetComposition`, and one it has not is served by both — whatever else has or
/// has not been called since the stop.
#[tokio::test]
async fn the_tagging_surface_and_get_composition_agree_over_a_compositions_whole_life() {
    let service = RealTime::new();
    let stage = service.stage("lifetime").await;

    for row in rows() {
        let Row {
            case,
            waited,
            retention_run_out,
            unrelated_call_in_between,
            tagging_first,
            tags,
        } = row;

        // A second composition of the same stage, which the unrelated call reads: reading
        // it holds the store mutably, so it materialises — and would drop — every
        // composition the clock has reclaimed.
        let bystander = service.start(case, &stage, &json!({})).await;
        let composition_arn = service.start(case, &stage, &tags).await;
        let stopped = service.stop(case, &composition_arn).await;

        tokio::time::sleep(waited).await;
        if retention_run_out {
            service.run_out_the_retention(case, &composition_arn);
        }
        if unrelated_call_in_between {
            let (status, response) = service.read(&bystander).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the composition nobody stopped must still be read, \
                 got {status} {response}"
            );
        }

        // Both readers, at as near the same instant as two calls can be, bracketed by the
        // elapsed time either side: an answer is only required where the whole bracket
        // lies on one side of a span boundary.
        let before = stopped.elapsed();
        let (listed, read) = if tagging_first {
            let listed = service.list_tags(&composition_arn).await;
            let read = service.read(&composition_arn).await;
            (listed, read)
        } else {
            let read = service.read(&composition_arn).await;
            let listed = service.list_tags(&composition_arn).await;
            (listed, read)
        };
        // The same tag listing again, to pin that no call made in between changed its
        // answer.
        let repeated = service.list_tags(&composition_arn).await;
        let after = stopped.elapsed();

        for (what, (status, response)) in [
            ("ListTagsForResource", &listed),
            ("GetComposition", &read),
            ("the repeated ListTagsForResource", &repeated),
        ] {
            assert!(
                [200, 404].contains(status),
                "case {case:?}: {what} on a stopped composition must answer HTTP 200 or \
                 the modelled 404, got {status} {response}"
            );
        }

        if !retention_run_out {
            // The clock has not reclaimed the resource yet, so every reader serves it and
            // the tags are the ones the start persisted.
            for (what, (status, response)) in [
                ("ListTagsForResource", &listed),
                ("GetComposition", &read),
                ("the repeated ListTagsForResource", &repeated),
            ] {
                assert_eq!(
                    *status, 200,
                    "case {case:?}: {what} read {before:?}..{after:?} after the stop, \
                     before the resource is reclaimed, must answer HTTP 200, \
                     got {status} {response}"
                );
            }
            for (what, listing) in [
                ("ListTagsForResource", &listed.1),
                ("the repeated ListTagsForResource", &repeated.1),
            ] {
                assert_eq!(
                    listing["tags"], tags,
                    "case {case:?}: {what} must report the tags the start persisted \
                     ({tags}), got {listing}"
                );
            }
            let listing: HashMap<String, String> =
                serde_json::from_value(listed.1["tags"].clone()).unwrap_or_default();
            let on_composition: HashMap<String, String> =
                serde_json::from_value(read.1["composition"]["tags"].clone()).unwrap_or_default();
            assert_eq!(
                listing, on_composition,
                "case {case:?}: the two readers must report the same tags, \
                 got {listing:?} against {on_composition:?}"
            );
            let state = member(&read.1["composition"], "state");
            let admissible: &[&str] = if before >= STOP_SPAN {
                &[STATE_STOPPED]
            } else if after < STOP_SPAN {
                &[STATE_STOPPING]
            } else {
                &[STATE_STOPPING, STATE_STOPPED]
            };
            assert!(
                admissible.contains(&state.as_str()),
                "case {case:?}: a composition read {before:?}..{after:?} after the stop \
                 must be in one of {admissible:?}, got {}",
                read.1
            );
        } else {
            // The clock reclaimed the ephemeral resource before either reader was asked:
            // both must answer the modelled not-found, and the repeat of the listing must
            // answer it too rather than changing its mind because another call ran.
            for (what, expected_code, (status, response)) in [
                ("ListTagsForResource", TAGGING_NOT_FOUND, &listed),
                ("GetComposition", NOT_FOUND, &read),
                ("the repeated ListTagsForResource", TAGGING_NOT_FOUND, &repeated),
            ] {
                assert_eq!(
                    (*status, error_code(response)),
                    (404, expected_code.to_string()),
                    "case {case:?}: {what} read {before:?}..{after:?} after the stop, past \
                     the reclaim of the ephemeral resource, must answer HTTP 404 \
                     {expected_code}, got {status} {response}"
                );
                assert!(
                    !member(response, "message").is_empty(),
                    "case {case:?}: the refusal from {what} must state why, got {response}"
                );
            }
            assert!(
                listed.1.get("tags").is_none() && repeated.1.get("tags").is_none(),
                "case {case:?}: a refused tag listing must report no tags, got {} and {}",
                listed.1,
                repeated.1
            );
        }
    }
}

/// The agreement holds across the whole `/tags/{resourceArn}` surface, whichever of its
/// operations is the FIRST call made after the reclaim: the rows are the three methods it
/// binds, each asked before anything else has touched the store since the stop.
#[tokio::test]
async fn no_tagging_operation_serves_a_composition_the_clock_has_reclaimed() {
    let service = RealTime::new();
    let stage = service.stage("reclaimed").await;

    // (case, the operation the method is bound to, method, query, body)
    let rows: [(&str, &str, &str, &str, Option<Value>); 3] = [
        ("a tag listing", "ListTagsForResource", "GET", "", None),
        (
            "a tag write",
            "TagResource",
            "POST",
            "",
            Some(json!({ "tags": { "team": "video" } })),
        ),
        (
            "a tag removal",
            "UntagResource",
            "DELETE",
            "?tagKeys=env",
            None,
        ),
    ];

    for (case, operation, method, query, body) in rows {
        let composition_arn = service
            .start(case, &stage, &json!({ "env": "test" }))
            .await;
        let stopped = service.stop(case, &composition_arn).await;
        tokio::time::sleep(STOP_SPAN + Duration::from_millis(400)).await;
        service.run_out_the_retention(case, &composition_arn);

        // The first call since the stop is the one under test, so nothing else can have
        // written the reclaim down for it.
        let before = stopped.elapsed();
        let (status, response) = service.tagging(method, &composition_arn, query, body).await;
        assert!(
            before >= STOP_SPAN,
            "case {case:?}: the call must be made once the stop has finished, \
             made {before:?} after the stop"
        );
        assert_eq!(
            (status, error_code(&response)),
            (404, TAGGING_NOT_FOUND.to_string()),
            "case {case:?}: {operation} on a composition the clock has reclaimed must \
             answer HTTP 404 {TAGGING_NOT_FOUND}, got {status} {response}"
        );

        let (status, response) = service.read(&composition_arn).await;
        assert_eq!(
            (status, error_code(&response)),
            (404, NOT_FOUND.to_string()),
            "case {case:?}: GetComposition must answer the same 404 as {operation}, under \
             the modelled code, got {status} {response}"
        );
    }
}
