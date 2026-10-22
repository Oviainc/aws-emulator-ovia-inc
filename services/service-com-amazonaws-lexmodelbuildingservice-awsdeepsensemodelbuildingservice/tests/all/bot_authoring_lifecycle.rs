// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the bot-authoring loop: `PutIntent`, `GetIntent`, `PutBot`,
//! `GetBot`, `GetBots` and `DeleteBot`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns.
//!
//! The tests assert RULES over sets of inputs rather than one recorded call: the
//! write-then-read round trip over a table of bot shapes (locales, session TTLs at both
//! published bounds, present and absent optional members, zero/one/two intents), the
//! build status over the `processBehavior` catalog crossed with resolvable and
//! unresolvable intent references, the conditional-write rule over BOTH resources that
//! carry a `checksum`, and pagination over a table of page sizes including the published
//! maximum.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_lexmodelbuildingservice_awsdeepsensemodelbuildingservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One emulator with this service mounted on a fresh shared engine.
struct Lex {
    router: MultiServiceRouter,
}

/// One answered request: the HTTP status, the `x-amzn-errortype` error code when the
/// answer is a failure, and the parsed JSON body (`Value::Null` for an empty body).
struct Answer {
    status: u16,
    error: Option<String>,
    body: Value,
}

impl Lex {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The tests exercise the service, not SigV4: requests carry a credential scope
        // so the router can name the target service, but no real signature.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body.map(|value| value.to_string()).unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "models.lex.us-east-1.amazonaws.com")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/lex/aws4_request, \
                 SignedHeaders=host, Signature=0",
            )
            .header("content-type", "application/json")
            .body(Bytes::from(payload))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
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

    /// `PutIntent` for `name`. The name is an `httpLabel`, so it travels in the URI.
    async fn put_intent(&self, name: &str, body: Value) -> Answer {
        self.call(
            "PUT",
            &format!("/intents/{name}/versions/$LATEST"),
            Some(body),
        )
        .await
    }

    /// `PutBot` for `name`.
    async fn put_bot(&self, name: &str, body: Value) -> Answer {
        self.call("PUT", &format!("/bots/{name}/versions/$LATEST"), Some(body))
            .await
    }

    async fn get_bot(&self, name: &str, version_or_alias: &str) -> Answer {
        self.call("GET", &format!("/bots/{name}/versions/{version_or_alias}"), None)
            .await
    }

    async fn get_intent(&self, name: &str, version: &str) -> Answer {
        self.call("GET", &format!("/intents/{name}/versions/{version}"), None)
            .await
    }
}

/// A required string member of a response body, with the case named on failure.
fn string_member(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a string {member}, got {body:?}"))
        .to_owned()
}

/// `PutIntent` for a minimal, buildable intent, answering its `$LATEST` checksum.
async fn store_intent(lex: &Lex, name: &str) -> String {
    let answer = lex
        .put_intent(
            name,
            json!({
                "sampleUtterances": ["I want flowers"],
                "fulfillmentActivity": { "type": "ReturnIntent" },
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case PutIntent({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "checksum", &format!("PutIntent({name:?})"))
}

/// One row of the round-trip table: a bot shape to write and read back.
#[derive(Debug)]
struct BotShape {
    case: &'static str,
    name: &'static str,
    /// From the model's `Locale` enum (`de-DE | en-AU | en-GB | en-IN | en-US | es-419 |
    /// es-ES | es-US | fr-CA | fr-FR | it-IT | ja-JP | ko-KR`).
    locale: &'static str,
    child_directed: bool,
    description: Option<&'static str>,
    /// `idleSessionTTLInSeconds`, whose modelled range is 60..=86400; `None` exercises
    /// the documented default of 300.
    idle_ttl: Option<i64>,
    voice_id: Option<&'static str>,
    detect_sentiment: Option<bool>,
    intents: &'static [&'static str],
}

#[tokio::test]
async fn a_written_bot_reads_back_exactly_as_it_was_persisted() {
    let lex = Lex::new();
    for intent in ["OrderFlowers", "CancelOrder"] {
        store_intent(&lex, intent).await;
    }

    // Rows differ along what the round-trip rule generalises over: the locale, the
    // session TTL (both published bounds and the absent-member default), which optional
    // members are present, and how many intents the bot references.
    let shapes = [
        BotShape {
            case: "minimal",
            name: "RoundTripAlpha",
            locale: "en-US",
            child_directed: false,
            description: None,
            idle_ttl: None,
            voice_id: None,
            detect_sentiment: None,
            intents: &[],
        },
        BotShape {
            case: "every member, lowest TTL",
            name: "RoundTripBeta",
            locale: "de-DE",
            child_directed: true,
            description: Some("Ein Blumenbot"),
            idle_ttl: Some(60),
            voice_id: Some("Marlene"),
            detect_sentiment: Some(true),
            intents: &["OrderFlowers"],
        },
        BotShape {
            case: "highest TTL, two intents",
            name: "RoundTripGamma",
            locale: "ja-JP",
            child_directed: false,
            description: Some("a bot with two intents"),
            idle_ttl: Some(86400),
            voice_id: Some("Mizuki"),
            detect_sentiment: Some(false),
            intents: &["OrderFlowers", "CancelOrder"],
        },
        BotShape {
            case: "mid TTL",
            name: "RoundTripDelta",
            locale: "es-US",
            child_directed: true,
            description: Some(""),
            idle_ttl: Some(3600),
            voice_id: None,
            detect_sentiment: None,
            intents: &["CancelOrder"],
        },
    ];

    for shape in &shapes {
        let case = shape.case;
        let mut body = json!({
            "locale": shape.locale,
            "childDirected": shape.child_directed,
            // SAVE keeps the status deterministic for this table; the build statuses have
            // a table of their own below.
            "processBehavior": "SAVE",
            "intents": shape
                .intents
                .iter()
                .map(|name| json!({ "intentName": name, "intentVersion": "$LATEST" }))
                .collect::<Vec<Value>>(),
        });
        if let Some(description) = shape.description {
            body["description"] = json!(description);
        }
        if let Some(ttl) = shape.idle_ttl {
            body["idleSessionTTLInSeconds"] = json!(ttl);
        }
        if let Some(voice) = shape.voice_id {
            body["voiceId"] = json!(voice);
        }
        if let Some(detect) = shape.detect_sentiment {
            body["detectSentiment"] = json!(detect);
        }

        let written = lex.put_bot(shape.name, body).await;
        assert_eq!(
            written.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            written.status, written.body
        );
        assert_eq!(
            written.body["version"].as_str(),
            Some("$LATEST"),
            "case {case}: a write targets $LATEST, got {:?}",
            written.body["version"]
        );
        let checksum = string_member(&written.body, "checksum", case);
        assert!(
            !checksum.is_empty(),
            "case {case}: a write must mint a checksum, got {checksum:?}"
        );
        assert!(
            written.body["createdDate"].is_number() && written.body["lastUpdatedDate"].is_number(),
            "case {case}: expected both dates, got {:?}",
            written.body
        );
        assert_eq!(
            written.body["createdDate"], written.body["lastUpdatedDate"],
            "case {case}: a created resource has equal created and last-updated dates, got {:?}",
            written.body
        );
        assert_eq!(
            written.body["createVersion"].as_bool(),
            Some(false),
            "case {case}: createVersion was not specified, so it is false, got {:?}",
            written.body["createVersion"]
        );

        // The read reports the persisted record, not a body synthesised from the
        // request: every member the write answered must come back identical.
        let read = lex.get_bot(shape.name, "$LATEST").await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetBot, got {} with {:?}",
            read.status, read.body
        );
        for member in [
            "name",
            "description",
            "intents",
            "locale",
            "childDirected",
            "idleSessionTTLInSeconds",
            "detectSentiment",
            "voiceId",
            "status",
            "checksum",
            "version",
            "createdDate",
            "lastUpdatedDate",
        ] {
            assert_eq!(
                read.body[member], written.body[member],
                "case {case}: GetBot must report the persisted {member}, wrote {:?} and read {:?}",
                written.body[member], read.body[member]
            );
        }
        assert_eq!(
            read.body["name"].as_str(),
            Some(shape.name),
            "case {case}: expected the written name, got {:?}",
            read.body["name"]
        );
        assert_eq!(
            read.body["locale"].as_str(),
            Some(shape.locale),
            "case {case}: expected the written locale, got {:?}",
            read.body["locale"]
        );
        assert_eq!(
            read.body["childDirected"].as_bool(),
            Some(shape.child_directed),
            "case {case}: expected the written childDirected, got {:?}",
            read.body["childDirected"]
        );
        // An absent `idleSessionTTLInSeconds` is stored as the documented default of 300
        // rather than left empty.
        assert_eq!(
            read.body["idleSessionTTLInSeconds"].as_i64(),
            Some(shape.idle_ttl.unwrap_or(300)),
            "case {case}: expected the written TTL or the 300-second default, got {:?}",
            read.body["idleSessionTTLInSeconds"]
        );
        // An absent `detectSentiment` is stored as the documented default of false.
        assert_eq!(
            read.body["detectSentiment"].as_bool(),
            Some(shape.detect_sentiment.unwrap_or(false)),
            "case {case}: expected the written detectSentiment or the false default, got {:?}",
            read.body["detectSentiment"]
        );
        match shape.description {
            Some(description) => assert_eq!(
                read.body["description"].as_str(),
                Some(description),
                "case {case}: expected the written description, got {:?}",
                read.body["description"]
            ),
            None => assert!(
                read.body["description"].is_null(),
                "case {case}: a bot written without a description reports none, got {:?}",
                read.body["description"]
            ),
        }
        match shape.voice_id {
            Some(voice) => assert_eq!(
                read.body["voiceId"].as_str(),
                Some(voice),
                "case {case}: expected the written voiceId, got {:?}",
                read.body["voiceId"]
            ),
            None => assert!(
                read.body["voiceId"].is_null(),
                "case {case}: a bot written without a voiceId reports none, got {:?}",
                read.body["voiceId"]
            ),
        }
        let referenced: Vec<String> = read.body["intents"]
            .as_array()
            .map(|intents| {
                intents
                    .iter()
                    .map(|intent| string_member(intent, "intentName", case))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            referenced,
            shape.intents.iter().map(|n| (*n).to_owned()).collect::<Vec<String>>(),
            "case {case}: expected the written intents in order, got {:?}",
            read.body["intents"]
        );
    }
}

#[tokio::test]
async fn an_intent_reads_back_exactly_as_it_was_persisted() {
    let lex = Lex::new();

    // Rows differ along the members an intent carries: a bare intent, one with slots and
    // utterances, and one built on a built-in parent signature.
    let cases: [(&str, &str, Value); 3] = [
        (
            "bare",
            "BareIntent",
            json!({ "fulfillmentActivity": { "type": "ReturnIntent" } }),
        ),
        (
            "utterances and a slot",
            "OrderFlowers",
            json!({
                "description": "Order flowers",
                "sampleUtterances": ["I want flowers", "Order flowers for me"],
                "fulfillmentActivity": { "type": "ReturnIntent" },
                "slots": [{
                    "name": "FlowerType",
                    "slotConstraint": "Required",
                    "slotType": "AMAZON.AlphaNumeric",
                    "slotTypeVersion": "$LATEST",
                    "priority": 1,
                    "sampleUtterances": ["I want {FlowerType}"],
                }],
            }),
        ),
        (
            "built-in parent",
            "HelpIntent",
            json!({
                "parentIntentSignature": "AMAZON.HelpIntent",
                "sampleUtterances": ["help me"],
            }),
        ),
    ];

    for (case, name, body) in &cases {
        let written = lex.put_intent(name, body.clone()).await;
        assert_eq!(
            written.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            written.status, written.body
        );
        assert_eq!(
            written.body["version"].as_str(),
            Some("$LATEST"),
            "case {case}: a write targets $LATEST, got {:?}",
            written.body["version"]
        );
        let read = lex.get_intent(name, "$LATEST").await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetIntent, got {} with {:?}",
            read.status, read.body
        );
        for member in [
            "name",
            "description",
            "sampleUtterances",
            "slots",
            "fulfillmentActivity",
            "parentIntentSignature",
            "checksum",
            "version",
            "createdDate",
            "lastUpdatedDate",
        ] {
            assert_eq!(
                read.body[member], written.body[member],
                "case {case}: GetIntent must report the persisted {member}, wrote {:?} and read {:?}",
                written.body[member], read.body[member]
            );
        }
    }

    // A name or a version that names no stored record is a 404, not an empty body.
    for (case, name, version) in [
        ("unknown name", "NoSuchIntent", "$LATEST"),
        ("unknown version", "OrderFlowers", "7"),
    ] {
        let answer = lex.get_intent(name, version).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("NotFoundException"),
            "case {case}: expected NotFoundException, got {:?}",
            answer.error
        );
    }
}

/// One row of the build-status table.
struct StatusCase {
    case: &'static str,
    name: &'static str,
    /// `processBehavior`, whose catalog is `BUILD | SAVE`; `None` exercises the
    /// documented default of `BUILD`.
    process_behavior: Option<&'static str>,
    intents: &'static [&'static str],
    expected: &'static str,
}

#[tokio::test]
async fn the_build_status_follows_process_behavior_and_the_intents_it_can_resolve() {
    let lex = Lex::new();
    store_intent(&lex, "OrderFlowers").await;

    // Rows span the whole `ProcessBehavior` catalog (plus the absent member), crossed
    // with intent references that can and cannot be resolved against stored intents.
    let cases = [
        StatusCase {
            case: "SAVE with an intent",
            name: "SaveWithIntent",
            process_behavior: Some("SAVE"),
            intents: &["OrderFlowers"],
            expected: "NOT_BUILT",
        },
        StatusCase {
            case: "SAVE with no intents",
            name: "SaveBare",
            process_behavior: Some("SAVE"),
            intents: &[],
            expected: "NOT_BUILT",
        },
        StatusCase {
            case: "BUILD with a stored intent",
            name: "BuildWithIntent",
            process_behavior: Some("BUILD"),
            intents: &["OrderFlowers"],
            expected: "BUILDING",
        },
        StatusCase {
            case: "processBehavior absent, defaulting to BUILD",
            name: "BuildByDefault",
            process_behavior: None,
            intents: &["OrderFlowers"],
            expected: "BUILDING",
        },
        StatusCase {
            case: "BUILD with no intents",
            name: "BuildBare",
            process_behavior: Some("BUILD"),
            intents: &[],
            expected: "FAILED",
        },
        StatusCase {
            case: "BUILD referencing an intent that does not exist",
            name: "BuildUnknownIntent",
            process_behavior: Some("BUILD"),
            intents: &["NoSuchIntent"],
            expected: "FAILED",
        },
    ];

    for case_row in &cases {
        let case = case_row.case;
        let mut body = json!({
            "locale": "en-US",
            "childDirected": false,
            "intents": case_row
                .intents
                .iter()
                .map(|name| json!({ "intentName": name, "intentVersion": "$LATEST" }))
                .collect::<Vec<Value>>(),
        });
        if let Some(behavior) = case_row.process_behavior {
            body["processBehavior"] = json!(behavior);
        }
        let written = lex.put_bot(case_row.name, body).await;
        let put_at = Instant::now();
        assert_eq!(
            written.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            written.status, written.body
        );
        assert_eq!(
            written.body["status"].as_str(),
            Some(case_row.expected),
            "case {case}: expected status {:?}, got {:?}",
            case_row.expected,
            written.body["status"]
        );
        if case_row.expected == "FAILED" {
            assert!(
                written.body["failureReason"]
                    .as_str()
                    .is_some_and(|reason| !reason.is_empty()),
                "case {case}: a FAILED build must say why, got {:?}",
                written.body["failureReason"]
            );
        } else {
            assert!(
                written.body["failureReason"].is_null(),
                "case {case}: only a FAILED build carries a failureReason, got {:?}",
                written.body["failureReason"]
            );
        }

        let read = lex.get_bot(case_row.name, "$LATEST").await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetBot, got {} with {:?}",
            read.status, read.body
        );
        if case_row.expected == "BUILDING" {
            // The build is a transition the clock owes, so a read taken before it is due
            // still sees the transient status, and a later read sees READY.
            if put_at.elapsed() < Duration::from_millis(250) {
                assert_eq!(
                    read.body["status"].as_str(),
                    Some("BUILDING"),
                    "case {case}: a build is not finished the instant it starts, got {:?}",
                    read.body["status"]
                );
            }
            let status = wait_for_status(&lex, case_row.name, "READY", case).await;
            assert_eq!(
                status, "READY",
                "case {case}: a started build must reach READY, got {status:?}"
            );
            // The build is the only thing the clock moves: a finished build still reports
            // the checksum and the dates the write persisted.
            let finished = lex.get_bot(case_row.name, "$LATEST").await;
            for member in ["checksum", "createdDate", "lastUpdatedDate", "version"] {
                assert_eq!(
                    finished.body[member], written.body[member],
                    "case {case}: finishing a build must not move {member}, wrote {:?} and read {:?}",
                    written.body[member], finished.body[member]
                );
            }
        } else {
            // A terminal status is terminal: it stays put.
            assert_eq!(
                read.body["status"].as_str(),
                Some(case_row.expected),
                "case {case}: expected the persisted status, got {:?}",
                read.body["status"]
            );
        }
    }
}

/// Poll `GetBot` until the bot reports `expected`, answering the last status it reported.
///
/// Bounded: the emulator's build span is sub-second, so a status that has not arrived
/// within a few seconds is a failure rather than a slow machine.
async fn wait_for_status(lex: &Lex, name: &str, expected: &str, case: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = String::new();
    while Instant::now() < deadline {
        let answer = lex.get_bot(name, "$LATEST").await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 while polling, got {} with {:?}",
            answer.status, answer.body
        );
        last = string_member(&answer.body, "status", case);
        if last == expected {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    last
}

/// `PutIntent` or `PutBot` for `kind`, so one table can state a rule both operations
/// follow instead of repeating it per resource.
async fn write_resource(lex: &Lex, kind: &str, name: &str, body: Value) -> Answer {
    if kind == "intent" {
        lex.put_intent(name, body).await
    } else {
        lex.put_bot(name, body).await
    }
}

/// `GetIntent` or `GetBot` for `kind`; see [`write_resource`].
async fn read_resource(lex: &Lex, kind: &str, name: &str, version: &str) -> Answer {
    if kind == "intent" {
        lex.get_intent(name, version).await
    } else {
        lex.get_bot(name, version).await
    }
}

#[tokio::test]
async fn a_conditional_write_requires_the_stored_revisions_checksum() {
    let lex = Lex::new();

    // Both resources this slice writes carry the same `checksum` rule, so both are
    // rows of this table rather than one being the special case.
    let resources: [(&str, &str, Value); 2] = [
        (
            "intent",
            "ConditionalIntent",
            json!({ "fulfillmentActivity": { "type": "ReturnIntent" } }),
        ),
        (
            "bot",
            "ConditionalBot",
            json!({ "locale": "en-US", "childDirected": false, "processBehavior": "SAVE" }),
        ),
    ];

    for (kind, name, base) in &resources {
        // A create leaves `checksum` blank.
        let created = write_resource(&lex, kind, name, base.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {kind} create: expected 200, got {} with {:?}",
            created.status, created.body
        );
        let first = string_member(&created.body, "checksum", &format!("{kind} create"));

        // A create that supplies one is a bad request: there is no revision to match.
        let mut with_checksum = base.clone();
        with_checksum["checksum"] = json!(first.clone());
        let fresh_name = format!("Fresh{}", if *kind == "intent" { "Intent" } else { "Bot" });
        let refused = write_resource(&lex, kind, &fresh_name, with_checksum.clone()).await;
        assert_eq!(
            refused.status, 400,
            "case {kind} create with a checksum: expected 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("BadRequestException"),
            "case {kind} create with a checksum: expected BadRequestException, got {:?}",
            refused.error
        );

        // An update without a checksum, and an update with a checksum that identifies no
        // revision, are both refused with 412 — and neither changes the stored record.
        let mut stale = base.clone();
        stale["checksum"] = json!("00000000-0000-0000-0000-000000000000");
        for (label, body) in [("no checksum", base.clone()), ("a stale checksum", stale)] {
            let answer = write_resource(&lex, kind, name, body).await;
            assert_eq!(
                answer.status, 412,
                "case {kind} update with {label}: expected 412, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("PreconditionFailedException"),
                "case {kind} update with {label}: expected PreconditionFailedException, got {:?}",
                answer.error
            );
            let read = read_resource(&lex, kind, name, "$LATEST").await;
            assert_eq!(
                read.body["checksum"].as_str(),
                Some(first.as_str()),
                "case {kind} update with {label}: a refused write must not change the record, got {:?}",
                read.body["checksum"]
            );
        }

        // An update carrying the stored revision's checksum is served, and mints the next
        // revision's checksum.
        let mut updated_body = base.clone();
        updated_body["checksum"] = json!(first.clone());
        updated_body["description"] = json!("revised");
        let updated = write_resource(&lex, kind, name, updated_body).await;
        assert_eq!(
            updated.status, 200,
            "case {kind} update with the stored checksum: expected 200, got {} with {:?}",
            updated.status, updated.body
        );
        let second = string_member(&updated.body, "checksum", &format!("{kind} update"));
        assert_ne!(
            first, second,
            "case {kind} update: a new revision must mint a new checksum, both were {first:?}"
        );
        assert_eq!(
            updated.body["description"].as_str(),
            Some("revised"),
            "case {kind} update: the request's fields replace the stored ones, got {:?}",
            updated.body["description"]
        );
        assert_eq!(
            updated.body["createdDate"], created.body["createdDate"],
            "case {kind} update: an update keeps the creation date, got {:?}",
            updated.body["createdDate"]
        );

        // The superseded checksum no longer identifies the stored revision.
        let replayed = write_resource(&lex, kind, name, with_checksum).await;
        assert_eq!(
            replayed.status, 412,
            "case {kind} replayed update: expected 412, got {} with {:?}",
            replayed.status, replayed.body
        );
    }
}

#[tokio::test]
async fn create_version_publishes_numbered_versions_of_what_was_written() {
    let lex = Lex::new();
    store_intent(&lex, "OrderFlowers").await;

    // A write that does not ask for a version publishes none.
    let first = lex
        .put_bot(
            "VersionedBot",
            json!({
                "locale": "en-US",
                "childDirected": false,
                "processBehavior": "SAVE",
                "description": "revision one",
            }),
        )
        .await;
    assert_eq!(
        first.status, 200,
        "case createVersion absent: expected 200, got {} with {:?}",
        first.status, first.body
    );
    assert_eq!(
        first.body["createVersion"].as_bool(),
        Some(false),
        "case createVersion absent: expected createVersion false, got {:?}",
        first.body["createVersion"]
    );
    let missing = lex.get_bot("VersionedBot", "1").await;
    assert_eq!(
        missing.status, 404,
        "case createVersion absent: no numbered version exists yet, got {} with {:?}",
        missing.status, missing.body
    );

    // Each write that asks for a version publishes the next number, 1 then 2, and the
    // response still describes `$LATEST`.
    let mut checksum = string_member(&first.body, "checksum", "createVersion absent");
    for (number, description) in [("1", "revision two"), ("2", "revision three")] {
        let answer = lex
            .put_bot(
                "VersionedBot",
                json!({
                    "locale": "en-US",
                    "childDirected": false,
                    "processBehavior": "SAVE",
                    "description": description,
                    "checksum": checksum,
                    "createVersion": true,
                }),
            )
            .await;
        let case = format!("createVersion true -> {number}");
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["createVersion"].as_bool(),
            Some(true),
            "case {case}: expected createVersion true, got {:?}",
            answer.body["createVersion"]
        );
        assert_eq!(
            answer.body["version"].as_str(),
            Some("$LATEST"),
            "case {case}: the body describes the $LATEST write, got {:?}",
            answer.body["version"]
        );
        checksum = string_member(&answer.body, "checksum", &case);

        // The published version is readable under its number and carries what was
        // written, with its own version and checksum.
        let published = lex.get_bot("VersionedBot", number).await;
        assert_eq!(
            published.status, 200,
            "case {case}: expected the numbered version to be readable, got {} with {:?}",
            published.status, published.body
        );
        assert_eq!(
            published.body["version"].as_str(),
            Some(number),
            "case {case}: expected version {number}, got {:?}",
            published.body["version"]
        );
        assert_eq!(
            published.body["description"].as_str(),
            Some(description),
            "case {case}: the published version carries what was written, got {:?}",
            published.body["description"]
        );
        assert_ne!(
            published.body["checksum"].as_str(),
            Some(checksum.as_str()),
            "case {case}: a published version is its own revision, so it has its own checksum"
        );
    }

    // The same rule on intents: the published number increments per intent.
    let created = lex
        .put_intent(
            "VersionedIntent",
            json!({ "fulfillmentActivity": { "type": "ReturnIntent" }, "createVersion": true }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case intent createVersion: expected 200, got {} with {:?}",
        created.status, created.body
    );
    let published = lex.get_intent("VersionedIntent", "1").await;
    assert_eq!(
        published.status, 200,
        "case intent createVersion: expected version 1 to be readable, got {} with {:?}",
        published.status, published.body
    );
    assert_eq!(
        published.body["version"].as_str(),
        Some("1"),
        "case intent createVersion: expected version 1, got {:?}",
        published.body["version"]
    );
}

#[tokio::test]
async fn get_bots_pages_the_stored_bots_and_filters_by_name_contains() {
    let lex = Lex::new();
    // Names chosen to satisfy the modelled `BotName` pattern `^([A-Za-z]_?)+$` (letters,
    // each optionally followed by one underscore).
    let names = [
        "AlphaBot",
        "BetaBot",
        "GammaBot",
        "DeltaBot",
        "EpsilonBot",
        "ZetaBot",
        "EtaBot",
    ];
    for name in names {
        let answer = lex
            .put_bot(
                name,
                json!({
                    "locale": "en-US",
                    "childDirected": false,
                    "processBehavior": "SAVE",
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case PutBot({name}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
    }
    let mut sorted: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
    sorted.sort();

    // Rows span the page-size range: one at a time, a size that does not divide the
    // count, the exact count, the published maximum of 50, and the absent member (the
    // documented default of 10).
    for size in [Some(1), Some(2), Some(3), Some(7), Some(50), None] {
        let case = match size {
            Some(size) => format!("GetBots(maxResults={size})"),
            None => "GetBots(maxResults absent)".to_string(),
        };
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(token) = &token {
                query.push(format!("nextToken={token}"));
            }
            let uri = if query.is_empty() {
                "/bots".to_string()
            } else {
                format!("/bots?{}", query.join("&"))
            };
            let answer = lex.call("GET", &uri, None).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            let bots = answer.body["bots"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected a bots array, got {:?}", answer.body))
                .clone();
            let effective = size.unwrap_or(10) as usize;
            assert!(
                bots.len() <= effective,
                "case {case}: a page holds at most {effective} bots, got {}",
                bots.len()
            );
            for bot in &bots {
                // Every `BotMetadata` member is read from the stored record.
                assert_eq!(
                    bot["version"].as_str(),
                    Some("$LATEST"),
                    "case {case}: a listing reports the $LATEST version, got {:?}",
                    bot["version"]
                );
                assert_eq!(
                    bot["status"].as_str(),
                    Some("NOT_BUILT"),
                    "case {case}: expected the persisted status, got {:?}",
                    bot["status"]
                );
                assert!(
                    bot["createdDate"].is_number() && bot["lastUpdatedDate"].is_number(),
                    "case {case}: expected both dates on a listing entry, got {bot:?}"
                );
                seen.push(string_member(bot, "name", &case));
            }
            pages += 1;
            assert!(pages <= 20, "case {case}: pagination did not terminate");
            match answer.body["nextToken"].as_str() {
                None => break,
                Some(cursor) => token = Some(cursor.to_owned()),
            }
        }
        assert_eq!(
            seen, sorted,
            "case {case}: paging must reassemble the whole name-sorted list {sorted:?}, got {seen:?}"
        );
        assert_eq!(
            seen.iter().collect::<BTreeSet<_>>().len(),
            sorted.len(),
            "case {case}: no bot may be listed twice, got {seen:?}"
        );
    }

    // `nameContains` is a substring test over the same sequence, matched
    // case-sensitively: "Eta" selects EtaBot and not ZetaBot.
    for (needle, expected) in [
        ("Bot", sorted.clone()),
        ("Alpha", vec!["AlphaBot".to_string()]),
        ("Eta", vec!["EtaBot".to_string()]),
        ("eta", vec!["BetaBot".to_string(), "ZetaBot".to_string()]),
        ("Nope", Vec::new()),
    ] {
        let case = format!("GetBots(nameContains={needle})");
        let answer = lex
            .call("GET", &format!("/bots?nameContains={needle}"), None)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let listed: Vec<String> = answer.body["bots"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a bots array, got {:?}", answer.body))
            .iter()
            .map(|bot| string_member(bot, "name", &case))
            .collect();
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case}: expected {expected:?}, got {listed:?}"
        );
        assert!(
            answer.body["nextToken"].is_null(),
            "case {case}: a single-page answer omits nextToken, got {:?}",
            answer.body["nextToken"]
        );
    }
}

#[tokio::test]
async fn deleting_a_bot_removes_every_version_and_is_visible_to_later_reads() {
    let lex = Lex::new();
    store_intent(&lex, "OrderFlowers").await;
    let kept = lex
        .put_bot(
            "KeptBot",
            json!({ "locale": "en-US", "childDirected": false, "processBehavior": "SAVE" }),
        )
        .await;
    assert_eq!(
        kept.status, 200,
        "case PutBot(KeptBot): expected 200, got {} with {:?}",
        kept.status, kept.body
    );
    let doomed = lex
        .put_bot(
            "DoomedBot",
            json!({
                "locale": "en-US",
                "childDirected": false,
                "processBehavior": "SAVE",
                "createVersion": true,
            }),
        )
        .await;
    assert_eq!(
        doomed.status, 200,
        "case PutBot(DoomedBot): expected 200, got {} with {:?}",
        doomed.status, doomed.body
    );
    for version in ["$LATEST", "1"] {
        let read = lex.get_bot("DoomedBot", version).await;
        assert_eq!(
            read.status, 200,
            "case before delete GetBot({version}): expected 200, got {} with {:?}",
            read.status, read.body
        );
    }

    let deleted = lex.call("DELETE", "/bots/DoomedBot", None).await;
    assert_eq!(
        deleted.status, 204,
        "case DeleteBot: expected 204, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert!(
        deleted.body.is_null(),
        "case DeleteBot: expected an empty body, got {:?}",
        deleted.body
    );

    // Every version of the deleted bot is gone, and deleting it again is a 404.
    for (case, method, uri) in [
        (
            "after delete GetBot($LATEST)",
            "GET",
            "/bots/DoomedBot/versions/$LATEST",
        ),
        ("after delete GetBot(1)", "GET", "/bots/DoomedBot/versions/1"),
        ("after delete DeleteBot", "DELETE", "/bots/DoomedBot"),
        ("DeleteBot(unknown)", "DELETE", "/bots/NeverExistedBot"),
    ] {
        let answer = lex.call(method, uri, None).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("NotFoundException"),
            "case {case}: expected NotFoundException, got {:?}",
            answer.error
        );
    }

    // The listing reports the delete too, and the bot that was not deleted survives.
    let listed = lex.call("GET", "/bots", None).await;
    let names: Vec<String> = listed.body["bots"]
        .as_array()
        .unwrap_or_else(|| panic!("case GetBots after delete: expected a bots array, got {:?}", listed.body))
        .iter()
        .map(|bot| string_member(bot, "name", "GetBots after delete"))
        .collect();
    assert_eq!(
        names,
        vec!["KeptBot".to_string()],
        "case GetBots after delete: expected only the surviving bot, got {names:?}"
    );

    // The intents are a separate resource: deleting a bot leaves them alone.
    let intent = lex.get_intent("OrderFlowers", "$LATEST").await;
    assert_eq!(
        intent.status, 200,
        "case GetIntent after DeleteBot: expected 200, got {} with {:?}",
        intent.status, intent.body
    );
}

#[tokio::test]
async fn an_unknown_bot_name_or_version_is_not_found() {
    let lex = Lex::new();
    lex.put_bot(
        "KnownBot",
        json!({ "locale": "en-US", "childDirected": false, "processBehavior": "SAVE" }),
    )
    .await;

    for (case, name, version_or_alias) in [
        ("unknown name", "NoSuchBot", "$LATEST"),
        ("unknown numbered version", "KnownBot", "3"),
        ("unknown alias", "KnownBot", "prod"),
    ] {
        let answer = lex.get_bot(name, version_or_alias).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("NotFoundException"),
            "case {case}: expected NotFoundException, got {:?}",
            answer.error
        );
    }

    // An empty store lists no bots rather than failing.
    let empty = Lex::new().call("GET", "/bots", None).await;
    assert_eq!(
        empty.status, 200,
        "case GetBots(empty store): expected 200, got {} with {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.body["bots"].as_array().map(Vec::len),
        Some(0),
        "case GetBots(empty store): expected an empty list, got {:?}",
        empty.body["bots"]
    );
}
