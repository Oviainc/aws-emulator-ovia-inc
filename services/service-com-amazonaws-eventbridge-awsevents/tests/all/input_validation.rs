// SPDX-License-Identifier: CC0-1.0

//! What this service REFUSES, and how it words the refusal, driven over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through the real
//! `MultiServiceRouter` — the same path a client takes, so routing,
//! deserialization, the modelled-constraint validator, the handler and
//! serialization are all exercised.
//!
//! Seven rules, each asserted over a TABLE of inputs rather than one recorded request:
//!
//! 1. `PutTargets` refuses a request that names one target `Id` twice — whole, with
//!    400 `ValidationException`, and before anything is stored.
//! 2. `PutTargets` refuses a request whose target carries an ARN-valued member that
//!    holds no ARN, the same way: whole, and before anything is stored.
//! 3. A rule whose duplicate-`Id` `PutTargets` was refused therefore carries no
//!    targets, so `DeleteRule` deletes it without `Force`.
//! 4. A published event entry that does not provide each of `Detail`, `DetailType`
//!    and `Source` fails on its own, naming the first property it is missing; a
//!    request in which no entry provides all three fails entirely.
//! 5. A published event entry whose `Detail` is not a JSON object is failed per entry
//!    under the documented `MalformedDetail` code.
//! 6. A published event entry whose `DetailType` is longer than the documented
//!    128-character maximum is FAILED per entry (`InvalidArgument`) and counted in
//!    `FailedEntryCount`, while every other entry of the same request is published.
//! 7. A `@pattern` refusal reports the expression the value had to satisfy WITHOUT the
//!    anchors the model spells it with; every other constraint class keeps the wording
//!    it had.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::ComAmazonawsEventbridgeAwseventsState;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-west-2";

/// "Free-form string, with a maximum of 128 characters, used to decide what fields to
/// expect in the event detail" —
/// `smithy_query(query="shape", shape="PutEventsRequestEntry")`, member `DetailType`.
/// Written out here, not imported, so a wrong constant in the implementation cannot
/// make a row pass.
const DOCUMENTED_DETAIL_TYPE_MAX: usize = 128;

/// The pattern-constrained shapes of this service, as the model spells them
/// (`smithy_query(query="shape", shape="EventSourceName,AccountId,RuleName,TargetId,\
/// EndpointId,EventBusNameOrArn")`).
const EVENT_SOURCE_NAME_PATTERN: &str = r"^aws\.partner(/[\.\-_A-Za-z0-9]+){2,}$";
const ACCOUNT_ID_PATTERN: &str = r"^\d{12}$";
const RULE_NAME_PATTERN: &str = r"^[\.\-_A-Za-z0-9]+$";
const TARGET_ID_PATTERN: &str = r"^[\.\-_A-Za-z0-9]+$";
const ENDPOINT_ID_PATTERN: &str = r"^[A-Za-z0-9\-]+[\.][A-Za-z0-9\-]+$";
const EVENT_BUS_NAME_OR_ARN_PATTERN: &str =
    r"^(arn:aws[\w-]*:events:[a-z]+-[a-z]+-[\w-]+:[0-9]{12}:event-bus\/)?[/\.\-_A-Za-z0-9]+$";

/// `RuleName`'s modelled `@length` maximum, used as a NON-pattern control.
const RULE_NAME_MAX_LENGTH: usize = 64;

/// The emulator: one router carrying only this service over one shared engine, plus
/// that engine, so a test can read the store the handlers wrote.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

async fn call(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/events/aws4_request, SignedHeaders=host, \
         Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "events.eu-west-2.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header("authorization", format!("AWS4-HMAC-SHA256 Credential={credential}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`), without its namespace; the empty string on a success.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The `message` member of a failure body.
fn message(body: &Value) -> &str {
    body.get("message").and_then(Value::as_str).unwrap_or_default()
}

/// A target ARN pointing at some other service's resource.
fn target_arn(resource: &str) -> String {
    format!(
        "arn:aws:sqs:{REGION}:{}:{resource}",
        engine::emulator_account_id()
    )
}

/// How many events `PutEvents` appended to the routing log.
fn delivered_count(emulator: &Emulator) -> usize {
    emulator
        .engine
        .state::<ComAmazonawsEventbridgeAwseventsState>()
        .lock()
        .delivered
        .len()
}

/// The `Id` of every target `ListTargetsByRule` reports for `rule`.
async fn target_ids(emulator: &Emulator, rule: &str) -> Vec<String> {
    let (status, body) = call(emulator, "ListTargetsByRule", &json!({ "Rule": rule })).await;
    assert_eq!(status, 200, "ListTargetsByRule answered {status} {body}");
    body["Targets"]
        .as_array()
        .map(|targets| {
            targets
                .iter()
                .map(|target| target["Id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The refusal a duplicate target `Id` must carry, built from the request the way the
/// service builds it: the parameter and the `Id` that repeated.
fn duplicate_message(id: &str) -> String {
    format!("Parameter targets is not valid. Reason: More than one target with Id '{id}' is provided.")
}

/// Create `rule` on the default bus with one already-installed target, so the rows
/// below refuse against a rule that is NOT simply empty.
async fn rule_with_one_target(emulator: &Emulator, rule: &str) {
    let (status, body) = call(
        emulator,
        "PutRule",
        &json!({ "Name": rule, "EventPattern": r#"{"source":["com.demo.widgets"]}"# }),
    )
    .await;
    assert_eq!(status, 200, "PutRule answered {status} {body}");
    let (status, body) = call(
        emulator,
        "PutTargets",
        &json!({
            "Rule": rule,
            "Targets": [{ "Id": "existing", "Arn": target_arn("queue-existing") }],
        }),
    )
    .await;
    assert_eq!(status, 200, "PutTargets answered {status} {body}");
}

/// `Id` is the key a target is stored under, so a `Targets` list carrying one `Id`
/// twice names no single target set: the whole request is refused, and nothing it
/// asked for is stored. The rows differ along where the repeat sits, how many times
/// it repeats, how many entries surround it, and whether the repeated entries agree
/// on their `Arn`.
#[tokio::test]
async fn put_targets_refuses_a_targets_list_that_names_one_id_twice() {
    let emulator = emulator();

    // `TargetId`'s modelled 64-character maximum, bound once so the row below can
    // borrow it.
    let longest_id = "z".repeat(64);
    // (case, the Targets list, the Id the refusal must name)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "two entries sharing an Id, different Arns",
            json!([
                { "Id": "dup", "Arn": target_arn("queue-one") },
                { "Id": "dup", "Arn": target_arn("queue-two") },
            ]),
            "dup",
        ),
        (
            "two entries sharing an Id and an Arn",
            json!([
                { "Id": "dup", "Arn": target_arn("queue-one") },
                { "Id": "dup", "Arn": target_arn("queue-one") },
            ]),
            "dup",
        ),
        (
            "the repeat is the last of four entries",
            json!([
                { "Id": "t1", "Arn": target_arn("queue-one") },
                { "Id": "t2", "Arn": target_arn("queue-two") },
                { "Id": "t3", "Arn": target_arn("queue-three") },
                { "Id": "t2", "Arn": target_arn("queue-four") },
            ]),
            "t2",
        ),
        (
            "the repeat is in the middle of three entries",
            json!([
                { "Id": "t1", "Arn": target_arn("queue-one") },
                { "Id": "t1", "Arn": target_arn("queue-two") },
                { "Id": "t2", "Arn": target_arn("queue-three") },
            ]),
            "t1",
        ),
        (
            "three entries all sharing one Id",
            json!([
                { "Id": "thrice", "Arn": target_arn("queue-one") },
                { "Id": "thrice", "Arn": target_arn("queue-two") },
                { "Id": "thrice", "Arn": target_arn("queue-three") },
            ]),
            "thrice",
        ),
        (
            // Every character class `TargetId`'s pattern admits, so the refusal is
            // not keyed to a short alphabetic id.
            "an Id spelling every character class the pattern admits",
            json!([
                { "Id": "A.b-c_9", "Arn": target_arn("queue-one") },
                { "Id": "A.b-c_9", "Arn": target_arn("queue-two") },
            ]),
            "A.b-c_9",
        ),
        (
            // 64 characters is `TargetId`'s modelled maximum: the longest id a
            // request can carry still has to be unique.
            "an Id at the modelled 64-character maximum",
            json!([
                { "Id": longest_id, "Arn": target_arn("queue-one") },
                { "Id": longest_id, "Arn": target_arn("queue-two") },
            ]),
            &longest_id,
        ),
        (
            // An id the rule ALREADY carries, repeated within the request: the
            // upsert would be ambiguous even though one of the two is a replacement.
            "a repeat of the Id the rule already carries",
            json!([
                { "Id": "existing", "Arn": target_arn("queue-one") },
                { "Id": "existing", "Arn": target_arn("queue-two") },
            ]),
            "existing",
        ),
    ];

    for (index, (case, targets, duplicated)) in rows.iter().enumerate() {
        let rule = format!("dup-rule-{index}");
        rule_with_one_target(&emulator, &rule).await;
        let (status, body) = call(
            &emulator,
            "PutTargets",
            &json!({ "Rule": rule, "Targets": targets }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        let expected = duplicate_message(duplicated);
        let actual = message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        // Refused BEFORE any state change: the rule still carries exactly the target
        // it carried before the call, and no FailedEntries envelope was returned.
        let ids = target_ids(&emulator, &rule).await;
        assert_eq!(
            ids,
            vec!["existing".to_string()],
            "case {case:?}: a refused request must store nothing, got {ids:?}"
        );
        assert!(
            body.get("FailedEntryCount").is_none(),
            "case {case:?}: the whole request is refused, not reported per entry, got {body}"
        );
    }

    // Control: the same lists with DISTINCT ids are accepted and stored, so the rows
    // above are refused for the repeat and not for their shape.
    let rule = "distinct-rule";
    rule_with_one_target(&emulator, rule).await;
    let (status, body) = call(
        &emulator,
        "PutTargets",
        &json!({
            "Rule": rule,
            "Targets": [
                { "Id": "A.b-c_9", "Arn": target_arn("queue-one") },
                { "Id": "z".repeat(64), "Arn": target_arn("queue-two") },
                { "Id": "existing", "Arn": target_arn("queue-replaced") },
            ],
        }),
    )
    .await;
    assert_eq!(status, 200, "distinct ids answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "distinct ids: expected FailedEntryCount 0, got {body}"
    );
    let ids = target_ids(&emulator, rule).await;
    assert_eq!(
        ids,
        vec!["existing".to_string(), "A.b-c_9".to_string(), "z".repeat(64)],
        "distinct ids: expected the upsert to land in insertion order, got {ids:?}"
    );
}

/// A duplicate-`Id` `PutTargets` installs nothing, so the rule it named still carries
/// no targets of that request and `DeleteRule` deletes it WITHOUT `Force` — and the
/// deleted rule is gone from `DescribeRule`. The rows differ along the duplicate
/// request that was refused and along `Force`.
#[tokio::test]
async fn a_rule_is_deletable_after_its_duplicate_target_request_was_refused() {
    let emulator = emulator();

    // (case, the refused Targets list, whether DeleteRule passes Force)
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "two entries sharing an Id",
            json!([
                { "Id": "dup", "Arn": target_arn("queue-one") },
                { "Id": "dup", "Arn": target_arn("queue-two") },
            ]),
            false,
        ),
        (
            "two entries sharing an Id, Force on the delete",
            json!([
                { "Id": "dup", "Arn": target_arn("queue-one") },
                { "Id": "dup", "Arn": target_arn("queue-two") },
            ]),
            true,
        ),
        (
            "a four-entry list whose last entry repeats",
            json!([
                { "Id": "t1", "Arn": target_arn("queue-one") },
                { "Id": "t2", "Arn": target_arn("queue-two") },
                { "Id": "t3", "Arn": target_arn("queue-three") },
                { "Id": "t1", "Arn": target_arn("queue-four") },
            ]),
            false,
        ),
    ];

    for (index, (case, targets, force)) in rows.iter().enumerate() {
        let rule = format!("teardown-rule-{index}");
        let (status, body) = call(
            &emulator,
            "PutRule",
            &json!({ "Name": rule, "EventPattern": r#"{"source":["com.demo.widgets"]}"# }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: PutRule answered {status} {body}");

        let (status, body) = call(
            &emulator,
            "PutTargets",
            &json!({ "Rule": rule, "Targets": targets }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the duplicate request to be refused, got {status} {body}"
        );

        let ids = target_ids(&emulator, &rule).await;
        assert!(
            ids.is_empty(),
            "case {case:?}: the refused request installed {ids:?}"
        );

        let mut request = json!({ "Name": rule });
        if *force {
            request["Force"] = json!(true);
        }
        let (status, body) = call(&emulator, "DeleteRule", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteRule to answer 200, got {status} {body}"
        );

        let (status, body) = call(&emulator, "DescribeRule", &json!({ "Name": rule })).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected the deleted rule to be gone, got {status} {body}"
        );
    }
}

/// An ARN-valued member of a target has to hold an ARN: none of them is modelled
/// with a pattern, so the service checks the form itself and refuses the WHOLE
/// request — nothing it asked for is stored. The rows differ along which member
/// carries the bad value, which part of the ARN's six fields is wrong, and where in
/// the list the offending target sits.
#[tokio::test]
async fn put_targets_refuses_a_target_whose_arn_is_not_an_arn() {
    let emulator = emulator();
    let good = target_arn("queue-good");

    /// The refusal an ARN-valued member outside the ARN form must carry: the value
    /// that was rejected and the form that was expected.
    fn arn_message(value: &str) -> String {
        format!("Parameter {value} is not valid. Reason: Provided Arn is not in correct format.")
    }

    // (case, the Targets list, the value the refusal must name)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an Arn that is not an ARN at all",
            json!([{ "Id": "t1", "Arn": "not-an-arn" }]),
            "not-an-arn",
        ),
        (
            "an Arn that is a bare resource name",
            json!([{ "Id": "t1", "Arn": "queue-five" }]),
            "queue-five",
        ),
        (
            "an Arn with too few fields",
            json!([{ "Id": "t1", "Arn": "arn:aws:sqs" }]),
            "arn:aws:sqs",
        ),
        (
            "an Arn with an empty partition",
            json!([{ "Id": "t1", "Arn": "arn::sqs:eu-west-2:123456789012:queue" }]),
            "arn::sqs:eu-west-2:123456789012:queue",
        ),
        (
            "an Arn with an empty service",
            json!([{ "Id": "t1", "Arn": "arn:aws::eu-west-2:123456789012:queue" }]),
            "arn:aws::eu-west-2:123456789012:queue",
        ),
        (
            "an Arn with an empty resource",
            json!([{ "Id": "t1", "Arn": "arn:aws:sqs:eu-west-2:123456789012:" }]),
            "arn:aws:sqs:eu-west-2:123456789012:",
        ),
        (
            "a RoleArn that is not an ARN",
            json!([{ "Id": "t1", "Arn": good, "RoleArn": "not-a-role" }]),
            "not-a-role",
        ),
        (
            "a DeadLetterConfig Arn that is not an ARN",
            json!([{
                "Id": "t1",
                "Arn": good,
                "DeadLetterConfig": { "Arn": "not-a-queue" },
            }]),
            "not-a-queue",
        ),
        (
            "the second target of a list carries the bad Arn",
            json!([
                { "Id": "t1", "Arn": good },
                { "Id": "t2", "Arn": "still-not-an-arn" },
            ]),
            "still-not-an-arn",
        ),
    ];

    for (index, (case, targets, value)) in rows.iter().enumerate() {
        let rule = format!("arn-rule-{index}");
        rule_with_one_target(&emulator, &rule).await;
        let (status, body) = call(
            &emulator,
            "PutTargets",
            &json!({ "Rule": rule, "Targets": targets }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        let expected = arn_message(value);
        let actual = message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        let ids = target_ids(&emulator, &rule).await;
        assert_eq!(
            ids,
            vec!["existing".to_string()],
            "case {case:?}: a refused request must store nothing, got {ids:?}"
        );
    }

    // Control: every one of the three ARN-valued members holding an ARN is accepted
    // and stored, so the rows above are refused for the form of the value.
    let rule = "arn-rule-ok";
    rule_with_one_target(&emulator, rule).await;
    let (status, body) = call(
        &emulator,
        "PutTargets",
        &json!({
            "Rule": rule,
            "Targets": [{
                "Id": "t1",
                "Arn": good,
                "RoleArn": format!("arn:aws:iam::{}:role/demo", engine::emulator_account_id()),
                "DeadLetterConfig": { "Arn": target_arn("queue-dlq") },
            }],
        }),
    )
    .await;
    assert_eq!(status, 200, "well-formed ARNs answered {status} {body}");
    let ids = target_ids(&emulator, rule).await;
    assert_eq!(
        ids,
        vec!["existing".to_string(), "t1".to_string()],
        "well-formed ARNs: expected the target to be stored, got {ids:?}"
    );
}

/// `Detail`, `DetailType` and `Source` are all required for an entry to name an
/// event: an entry that does not provide one fails on its own, naming the first
/// property it is missing in the order the entry documents them — and a request in
/// which NO entry provides all three fails ENTIRELY, with 400. The rows differ along
/// which properties an entry omits and along how many entries the request carries.
#[tokio::test]
async fn put_events_requires_detail_detail_type_and_source_per_entry() {
    let emulator = emulator();
    let complete = json!({
        "Source": "com.demo.widgets",
        "DetailType": "Order Created",
        "Detail": r#"{"amount":1}"#,
    });

    // (case, the incomplete entry, the property the failure must name)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "no Detail",
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created" }),
            "Detail",
        ),
        (
            "no DetailType",
            json!({ "Source": "com.demo.widgets", "Detail": "{}" }),
            "DetailType",
        ),
        (
            "no Source",
            json!({ "DetailType": "Order Created", "Detail": "{}" }),
            "Source",
        ),
        ("none of the three", json!({}), "Detail"),
        (
            "only Detail",
            json!({ "Detail": "{}" }),
            "DetailType",
        ),
        (
            "only DetailType",
            json!({ "DetailType": "Order Created" }),
            "Detail",
        ),
        (
            "only Source",
            json!({ "Source": "com.demo.widgets" }),
            "Detail",
        ),
        (
            // Whitespace names no source, so it counts as absent.
            "a blank Source",
            json!({ "Source": "   ", "DetailType": "Order Created", "Detail": "{}" }),
            "Source",
        ),
        (
            "a blank DetailType",
            json!({ "Source": "com.demo.widgets", "DetailType": " ", "Detail": "{}" }),
            "DetailType",
        ),
        (
            "nothing but a Time",
            json!({ "Time": 1_767_225_600_i64 }),
            "Detail",
        ),
    ];

    for (case, incomplete, expected_member) in &rows {
        // Beside a COMPLETE entry, so the request as a whole still names an event
        // and the failure is reported per entry, index-aligned with the request.
        let before = delivered_count(&emulator);
        let (status, body) = call(
            &emulator,
            "PutEvents",
            &json!({ "Entries": [complete, incomplete] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 with a per-entry failure, got {status} {body}"
        );
        assert_eq!(
            body["FailedEntryCount"],
            json!(1),
            "case {case:?}: expected FailedEntryCount 1, got {body}"
        );
        let entry = &body["Entries"][1];
        assert_eq!(
            entry.get("ErrorCode").and_then(Value::as_str),
            Some("InvalidArgument"),
            "case {case:?}: expected ErrorCode InvalidArgument, got {entry}"
        );
        let expected = format!(
            "Parameter {expected_member} is not valid. Reason: {expected_member} is a \
             required argument."
        );
        let actual = entry.get("ErrorMessage").and_then(Value::as_str).unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            body["Entries"][0]
                .get("EventId")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty()),
            "case {case:?}: the complete entry beside it is still published, got {body}"
        );
        assert_eq!(
            delivered_count(&emulator),
            before + 1,
            "case {case:?}: exactly the complete entry is published"
        );
    }

    // A request in which NO entry provides all three properties fails entirely: 400,
    // nothing published, and the refusal names the first property the first entry is
    // missing. The rows differ along the entry count and the properties omitted.
    let whole_request: Vec<(&str, Value, &str)> = vec![
        ("one entry with nothing at all", json!([{}]), "Detail"),
        (
            "one entry without Detail",
            json!([{ "Source": "com.demo.widgets", "DetailType": "Order Created" }]),
            "Detail",
        ),
        (
            "one entry without DetailType",
            json!([{ "Source": "com.demo.widgets", "Detail": "{}" }]),
            "DetailType",
        ),
        (
            "one entry without Source",
            json!([{ "DetailType": "Order Created", "Detail": "{}" }]),
            "Source",
        ),
        (
            "two entries, each missing a different property",
            json!([
                { "Source": "com.demo.widgets", "DetailType": "Order Created" },
                { "DetailType": "Order Created", "Detail": "{}" },
            ]),
            "Detail",
        ),
        (
            "three entries, none complete",
            json!([
                { "Source": "com.demo.widgets", "Detail": "{}" },
                { "Detail": "{}" },
                {},
            ]),
            "DetailType",
        ),
    ];

    for (case, entries, expected_member) in &whole_request {
        let before = delivered_count(&emulator);
        let (status, body) = call(&emulator, "PutEvents", &json!({ "Entries": entries })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the whole request to be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        let expected = format!(
            "Parameter {expected_member} is not valid. Reason: {expected_member} is a \
             required argument."
        );
        let actual = message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert_eq!(
            delivered_count(&emulator),
            before,
            "case {case:?}: a refused request publishes nothing"
        );
    }
}

/// An entry whose `Detail` is not a JSON object is failed per entry under the
/// documented `MalformedDetail` code ("The JSON provided is not valid"), with the
/// wording this service uses for it. The rows span the JSON forms that are not an
/// object — a non-JSON string, an array, a scalar, a truncated object — plus objects
/// as controls.
#[tokio::test]
async fn put_events_fails_an_entry_whose_detail_is_not_a_json_object() {
    let emulator = emulator();

    // (case, the Detail text, the expected ErrorCode — None means accepted)
    let rows: Vec<(&str, &str, Option<&str>)> = vec![
        ("text that is not JSON at all", "not json", Some("MalformedDetail")),
        ("a JSON array", "[1,2]", Some("MalformedDetail")),
        ("a JSON string", "\"text\"", Some("MalformedDetail")),
        ("a JSON number", "42", Some("MalformedDetail")),
        ("a JSON boolean", "true", Some("MalformedDetail")),
        ("JSON null", "null", Some("MalformedDetail")),
        ("a truncated object", "{\"a\":1", Some("MalformedDetail")),
        // Whitespace is no more a JSON object than any other non-object: the entry
        // carried `Detail`, and what it carried is malformed.
        ("text that is only whitespace", "   ", Some("MalformedDetail")),
        ("the empty string", "", Some("MalformedDetail")),
        ("an empty object", "{}", None),
        ("an object with members", r#"{"amount":1,"nested":{"k":[1,2]}}"#, None),
    ];

    for (case, detail, expected_code) in &rows {
        let (status, body) = call(
            &emulator,
            "PutEvents",
            &json!({
                "Entries": [{
                    "Source": "com.demo.widgets",
                    "DetailType": "Order Created",
                    "Detail": detail,
                }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 with a per-entry outcome, got {status} {body}"
        );
        let expected_failures = usize::from(expected_code.is_some());
        assert_eq!(
            body["FailedEntryCount"],
            json!(expected_failures),
            "case {case:?}: expected FailedEntryCount {expected_failures}, got {body}"
        );
        let entry = &body["Entries"][0];
        assert_eq!(
            entry.get("ErrorCode").and_then(Value::as_str),
            *expected_code,
            "case {case:?}: expected ErrorCode {expected_code:?}, got {entry}"
        );
        if expected_code.is_some() {
            let actual = entry.get("ErrorMessage").and_then(Value::as_str).unwrap_or_default();
            assert_eq!(
                actual, "Detail is malformed.",
                "case {case:?}: expected \"Detail is malformed.\", got {actual:?}"
            );
        }
    }
}

/// `DetailType` is documented with a 128-character maximum its modelled shape does not
/// express, so an entry past it is FAILED per entry rather than published: the entry
/// carries `InvalidArgument` and a message naming the parameter and the limit, and
/// `FailedEntryCount` counts it. The rows span the published bound, one past it, well
/// past it, and multi-byte characters — the limit counts CHARACTERS, not bytes.
#[tokio::test]
async fn put_events_fails_an_entry_whose_detail_type_is_over_the_documented_maximum() {
    let emulator = emulator();
    let (status, body) = call(
        &emulator,
        "PutRule",
        &json!({ "Name": "catch-all", "EventPattern": r#"{"source":[{"exists":true}]}"# }),
    )
    .await;
    assert_eq!(status, 200, "PutRule answered {status} {body}");

    let expected_message = format!(
        "Parameter DetailType is not valid. Reason: DetailType can't have more than \
         {DOCUMENTED_DETAIL_TYPE_MAX} characters."
    );

    // (case, the DetailType) — accepted exactly when it is within the documented max.
    let rows: Vec<(String, String)> = vec![
        ("one character".to_string(), "a".to_string()),
        (
            "one character under the maximum".to_string(),
            "a".repeat(DOCUMENTED_DETAIL_TYPE_MAX - 1),
        ),
        (
            "exactly the maximum".to_string(),
            "a".repeat(DOCUMENTED_DETAIL_TYPE_MAX),
        ),
        (
            "one character over the maximum".to_string(),
            "a".repeat(DOCUMENTED_DETAIL_TYPE_MAX + 1),
        ),
        (
            "well over the maximum".to_string(),
            "a".repeat(DOCUMENTED_DETAIL_TYPE_MAX * 8),
        ),
        (
            // 2 bytes per character: within the documented limit in CHARACTERS,
            // over it in bytes.
            "the maximum in two-byte characters".to_string(),
            "é".repeat(DOCUMENTED_DETAIL_TYPE_MAX),
        ),
        (
            "one two-byte character over the maximum".to_string(),
            "é".repeat(DOCUMENTED_DETAIL_TYPE_MAX + 1),
        ),
        (
            // 4 bytes per character, same rule.
            "the maximum in four-byte characters".to_string(),
            "😀".repeat(DOCUMENTED_DETAIL_TYPE_MAX),
        ),
        (
            "one four-byte character over the maximum".to_string(),
            "😀".repeat(DOCUMENTED_DETAIL_TYPE_MAX + 1),
        ),
    ];

    for (case, detail_type) in &rows {
        let over_limit = detail_type.chars().count() > DOCUMENTED_DETAIL_TYPE_MAX;
        let before = delivered_count(&emulator);
        let (status, body) = call(
            &emulator,
            "PutEvents",
            &json!({
                "Entries": [{
                    "Source": "com.demo.widgets",
                    "DetailType": detail_type,
                    "Detail": r#"{"amount":1}"#,
                }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutEvents answers 200 and reports per entry, got {status} {body}"
        );
        let expected_failures = usize::from(over_limit);
        assert_eq!(
            body["FailedEntryCount"],
            json!(expected_failures),
            "case {case:?}: expected FailedEntryCount {expected_failures}, got {body}"
        );
        let entry = &body["Entries"][0];
        if over_limit {
            assert_eq!(
                entry.get("ErrorCode").and_then(Value::as_str),
                Some("InvalidArgument"),
                "case {case:?}: expected ErrorCode InvalidArgument, got {entry}"
            );
            let actual = entry.get("ErrorMessage").and_then(Value::as_str).unwrap_or_default();
            assert_eq!(
                actual, expected_message,
                "case {case:?}: expected {expected_message:?}, got {actual:?}"
            );
            assert!(
                entry.get("EventId").is_none_or(Value::is_null),
                "case {case:?}: a failed entry carries no EventId, got {entry}"
            );
            assert_eq!(
                delivered_count(&emulator),
                before,
                "case {case:?}: a failed entry must not be published"
            );
        } else {
            assert!(
                entry.get("ErrorCode").is_none_or(Value::is_null),
                "case {case:?}: expected no ErrorCode, got {entry}"
            );
            assert!(
                entry
                    .get("EventId")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.is_empty()),
                "case {case:?}: an accepted entry carries an EventId, got {entry}"
            );
            assert_eq!(
                delivered_count(&emulator),
                before + 1,
                "case {case:?}: an accepted entry must be published"
            );
        }
    }

    // One request mixing acceptable and over-long entries: the failure is per ENTRY,
    // index-aligned with the request, and the acceptable entries are still published.
    let batch: Vec<(&str, String)> = vec![
        ("within the maximum", "Order Created".to_string()),
        (
            "over the maximum",
            "o".repeat(DOCUMENTED_DETAIL_TYPE_MAX + 1),
        ),
        (
            "exactly the maximum",
            "m".repeat(DOCUMENTED_DETAIL_TYPE_MAX),
        ),
        (
            "far over the maximum",
            "f".repeat(DOCUMENTED_DETAIL_TYPE_MAX * 4),
        ),
    ];
    let before = delivered_count(&emulator);
    let entries: Vec<Value> = batch
        .iter()
        .map(|(_, detail_type)| {
            json!({
                "Source": "com.demo.widgets",
                "DetailType": detail_type,
                "Detail": r#"{"amount":2}"#,
            })
        })
        .collect();
    let (status, body) = call(&emulator, "PutEvents", &json!({ "Entries": entries })).await;
    assert_eq!(status, 200, "the mixed batch answered {status} {body}");
    let expected_failures = batch
        .iter()
        .filter(|(_, detail_type)| detail_type.chars().count() > DOCUMENTED_DETAIL_TYPE_MAX)
        .count();
    assert_eq!(
        body["FailedEntryCount"],
        json!(expected_failures),
        "mixed batch: expected FailedEntryCount {expected_failures}, got {body}"
    );
    for (index, (case, detail_type)) in batch.iter().enumerate() {
        let entry = &body["Entries"][index];
        let over_limit = detail_type.chars().count() > DOCUMENTED_DETAIL_TYPE_MAX;
        let expected_code = over_limit.then_some("InvalidArgument");
        assert_eq!(
            entry.get("ErrorCode").and_then(Value::as_str),
            expected_code,
            "mixed batch entry {index} {case:?}: expected {expected_code:?}, got {entry}"
        );
    }
    assert_eq!(
        delivered_count(&emulator),
        before + (batch.len() - expected_failures),
        "mixed batch: only the acceptable entries are published"
    );
}

/// The expression a `@pattern` refusal reports is the one a value has to satisfy,
/// without the `^`/`$` the model writes to demand a whole-string match. The rows span
/// six operations and six pattern-constrained members — a top-level member, a nested
/// list element and a list-of-strings element — plus one control row per OTHER
/// constraint class this service can be made to violate (a string's length maximum, a
/// list's length minimum on two operations, an integer's range bounds, and an absent
/// required member), which keep the wording they had.
#[tokio::test]
async fn a_pattern_refusal_reports_the_expression_without_the_models_anchors() {
    let emulator = emulator();

    // The expression as a refusal must report it: the model's spelling minus its outer
    // anchors. Written out here rather than reusing the service's helper, so the test
    // states the expectation independently of the code under test.
    fn expression(model_pattern: &str) -> String {
        model_pattern
            .trim_start_matches('^')
            .trim_end_matches('$')
            .to_string()
    }
    fn pattern_message(value: &str, path: &str, model_pattern: &str) -> String {
        format!(
            "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: {}",
            expression(model_pattern)
        )
    }

    let good_arn = target_arn("queue-one");
    // (case, operation, request, the whole message the refusal must carry)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "CreatePartnerEventSource Name that is no partner name at all",
            "CreatePartnerEventSource",
            json!({ "Name": "not-a-partner-name", "Account": "123456789012" }),
            pattern_message("not-a-partner-name", "name", EVENT_SOURCE_NAME_PATTERN),
        ),
        (
            "CreatePartnerEventSource Name with only one namespace segment",
            "CreatePartnerEventSource",
            json!({ "Name": "aws.partner/onlyone", "Account": "123456789012" }),
            pattern_message("aws.partner/onlyone", "name", EVENT_SOURCE_NAME_PATTERN),
        ),
        (
            "CreatePartnerEventSource Name carrying a space",
            "CreatePartnerEventSource",
            json!({ "Name": "aws.partner/my saas/events", "Account": "123456789012" }),
            pattern_message(
                "aws.partner/my saas/events",
                "name",
                EVENT_SOURCE_NAME_PATTERN,
            ),
        ),
        (
            "CreatePartnerEventSource Account that is not twelve digits",
            "CreatePartnerEventSource",
            json!({ "Name": "aws.partner/mysaas/myevents", "Account": "12345678901a" }),
            pattern_message("12345678901a", "account", ACCOUNT_ID_PATTERN),
        ),
        (
            "PutRule Name outside RuleName's pattern",
            "PutRule",
            json!({ "Name": "bad name!" }),
            pattern_message("bad name!", "name", RULE_NAME_PATTERN),
        ),
        (
            "DescribeRule Name outside RuleName's pattern",
            "DescribeRule",
            json!({ "Name": "bad name!" }),
            pattern_message("bad name!", "name", RULE_NAME_PATTERN),
        ),
        (
            "DeleteRule Name outside RuleName's pattern",
            "DeleteRule",
            json!({ "Name": "bad name!" }),
            pattern_message("bad name!", "name", RULE_NAME_PATTERN),
        ),
        (
            "PutRule EventBusName outside EventBusNameOrArn's pattern",
            "PutRule",
            json!({ "Name": "ok-rule", "EventBusName": "bad bus!" }),
            pattern_message("bad bus!", "eventBusName", EVENT_BUS_NAME_OR_ARN_PATTERN),
        ),
        (
            "PutTargets first target Id outside TargetId's pattern",
            "PutTargets",
            json!({
                "Rule": "ok-rule",
                "Targets": [{ "Id": "bad id!", "Arn": good_arn }],
            }),
            pattern_message("bad id!", "targets.1.member.id", TARGET_ID_PATTERN),
        ),
        (
            "PutTargets second target Id outside TargetId's pattern",
            "PutTargets",
            json!({
                "Rule": "ok-rule",
                "Targets": [
                    { "Id": "t1", "Arn": good_arn },
                    { "Id": "bad id!", "Arn": good_arn },
                ],
            }),
            pattern_message("bad id!", "targets.2.member.id", TARGET_ID_PATTERN),
        ),
        (
            "RemoveTargets Ids element outside TargetId's pattern",
            "RemoveTargets",
            json!({ "Rule": "ok-rule", "Ids": ["bad id!"] }),
            pattern_message("bad id!", "ids.1.member", TARGET_ID_PATTERN),
        ),
        (
            "PutEvents EndpointId outside EndpointId's pattern",
            "PutEvents",
            json!({
                "EndpointId": "nodot",
                "Entries": [{ "Source": "com.demo", "DetailType": "Thing", "Detail": "{}" }],
            }),
            pattern_message("nodot", "endpointId", ENDPOINT_ID_PATTERN),
        ),
        // Controls: a constraint class that is NOT a pattern keeps its wording, so the
        // rows above cannot have been produced by rewording the whole envelope. One row
        // per class this service can be made to violate: the length bounds of a string
        // and of a list, the range bounds of an integer, and an absent member.
        (
            "PutRule Name past RuleName's length maximum",
            "PutRule",
            json!({ "Name": "r".repeat(RULE_NAME_MAX_LENGTH + 1) }),
            format!(
                "1 validation error detected: Value '{}' at 'name' failed to satisfy \
                 constraint: Member must have length less than or equal to \
                 {RULE_NAME_MAX_LENGTH}",
                "r".repeat(RULE_NAME_MAX_LENGTH + 1)
            ),
        ),
        (
            // `TargetList` is modelled `@length(min: 1, max: 100)`.
            "PutTargets with an empty Targets list",
            "PutTargets",
            json!({ "Rule": "ok-rule", "Targets": [] }),
            "1 validation error detected: Value '0 items' at 'targets' failed to satisfy \
             constraint: Member must have length greater than or equal to 1"
                .to_string(),
        ),
        (
            // `PutEventsRequestEntryList` is modelled `@length(min: 1, max: 10)`.
            "PutEvents with an empty Entries list",
            "PutEvents",
            json!({ "Entries": [] }),
            "1 validation error detected: Value '0 items' at 'entries' failed to satisfy \
             constraint: Member must have length greater than or equal to 1"
                .to_string(),
        ),
        (
            // `LimitMax100` is modelled `@range(min: 1, max: 100)`.
            "ListRules with a Limit below the range minimum",
            "ListRules",
            json!({ "Limit": 0 }),
            "1 validation error detected: Value '0' at 'limit' failed to satisfy \
             constraint: Member must have value greater than or equal to 1"
                .to_string(),
        ),
        (
            "ListRules with a Limit above the range maximum",
            "ListRules",
            json!({ "Limit": 101 }),
            "1 validation error detected: Value '101' at 'limit' failed to satisfy \
             constraint: Member must have value less than or equal to 100"
                .to_string(),
        ),
        (
            "PutRule without the required Name",
            "PutRule",
            json!({ "EventPattern": r#"{"source":["com.demo"]}"# }),
            "1 validation error detected: Value null at 'name' failed to satisfy \
             constraint: Member must not be null"
                .to_string(),
        ),
    ];

    for (case, operation, request, expected) in &rows {
        let (status, body) = call(&emulator, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        let actual = message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // And the expression is usable as-is: whatever the member, the reported pattern
    // carries neither the leading `^` nor the trailing `$` of the model's spelling.
    const CLAUSE: &str = "Member must satisfy regular expression pattern: ";
    for (case, _, _, expected) in &rows {
        let Some((_, reported)) = expected.split_once(CLAUSE) else {
            continue;
        };
        assert!(
            !reported.starts_with('^') && !reported.ends_with('$'),
            "case {case:?}: the reported expression must carry no outer anchor, got \
             {reported:?}"
        );
    }
}
