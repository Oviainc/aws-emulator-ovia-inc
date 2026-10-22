// SPDX-License-Identifier: CC0-1.0

//! The target-management responses end to end over the wire (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: AWSEvents.<Operation>`), dispatched through the
//! real `MultiServiceRouter` — so every case exercises routing,
//! deserialization, the handler, the persisted store and serialization: the
//! same path a client takes.
//!
//! One RULE over a TABLE of inputs: a 2xx response SERIALIZES every modelled
//! output collection member whose value the call computed, as the empty list
//! when the computation produced nothing — so `FailedEntries` and `Targets` are
//! always present and always index-aligned with what the call did, and a client
//! iterating them never has to tell "nothing to report" from "member not
//! supported".
//!
//! The rows differ along the count of entries, the mix of outcomes within one
//! request, the stored target count and the page size — not along names.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::MAX_TARGETS_PER_RULE;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-west-2";

/// `LimitMax100`'s modelled `@range(min: 1, max: 100)`, from
/// `smithy_query(query="operation", operation="ListTargetsByRule")`.
const LIMIT_MIN: i64 = 1;
const LIMIT_MAX: i64 = 100;

/// The emulator: one router carrying only this service over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    Emulator { router }
}

async fn call(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/events/aws4_request, \
         SignedHeaders=host, Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "events.eu-west-2.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!("AWS4-HMAC-SHA256 Credential={credential}"),
        )
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

/// A well-formed target ARN pointing at some other service's resource.
fn target_arn(resource: &str) -> String {
    format!(
        "arn:aws:sqs:{REGION}:{}:{resource}",
        engine::emulator_account_id()
    )
}

/// Create `rule` on the default bus, so the target operations have something to
/// hang targets off.
async fn put_rule(emulator: &Emulator, rule: &str) {
    let (status, body) = call(
        emulator,
        "PutRule",
        &json!({ "Name": rule, "EventPattern": r#"{"source":["com.demo.widgets"]}"# }),
    )
    .await;
    assert_eq!(status, 200, "rule {rule}: PutRule answered {status} {body}");
}

/// The member a response must carry, as a JSON array — fails with the whole
/// body when the member is absent or is not a list.
fn collection<'body>(body: &'body Value, member: &str, case: &str) -> &'body Vec<Value> {
    body.get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} array, got {body}"))
}

/// Put `count` well-formed targets on `rule`, named `t1..tN`, and hand back
/// their ids in the order they were stored.
async fn fill(emulator: &Emulator, rule: &str, count: usize) -> Vec<String> {
    let ids: Vec<String> = (1..=count).map(|index| format!("t{index}")).collect();
    if ids.is_empty() {
        return ids;
    }
    let targets: Vec<Value> = ids
        .iter()
        .map(|id| json!({ "Id": id, "Arn": target_arn(&format!("queue-{id}")) }))
        .collect();
    let (status, body) = call(
        emulator,
        "PutTargets",
        &json!({ "Rule": rule, "Targets": targets }),
    )
    .await;
    assert_eq!(
        status, 200,
        "rule {rule}: PutTargets answered {status} {body}"
    );
    ids
}

/// `PutTargets` always reports `FailedEntries`, and its contents are the
/// entries that did NOT land: the rows differ along the entry count and along
/// the mix of outcomes inside one request, including the all-success row whose
/// list is empty.
#[tokio::test]
async fn put_targets_always_reports_a_failed_entries_list() {
    // (case, [(Id, the Arn is well formed)]) — an ill-formed ARN is the
    // per-entry failure the handler reports rather than refusing the call.
    let row = |ids: &[&str], good: &[bool]| -> Vec<(String, bool)> {
        ids.iter()
            .zip(good.iter().copied())
            .map(|(id, good)| ((*id).to_string(), good))
            .collect()
    };
    let cases: Vec<(&str, Vec<(String, bool)>)> = vec![
        ("one-entry-all-good", row(&["t1"], &[true])),
        ("two-entries-all-good", row(&["t1", "t2"], &[true, true])),
        (
            "at-the-per-rule-quota-all-good",
            (1..=MAX_TARGETS_PER_RULE)
                .map(|index| (format!("t{index}"), true))
                .collect(),
        ),
        ("one-entry-failing", row(&["t1"], &[false])),
        (
            "mixed-outcomes",
            row(&["t1", "t2", "t3"], &[true, false, true]),
        ),
        ("every-entry-failing", row(&["t1", "t2"], &[false, false])),
    ];

    for (case, entries) in cases {
        let emulator = emulator();
        let rule = "target-report";
        put_rule(&emulator, rule).await;

        let targets: Vec<Value> = entries
            .iter()
            .map(|(id, well_formed)| {
                let arn = if *well_formed {
                    target_arn(&format!("queue-{id}"))
                } else {
                    format!("not-an-arn-{id}")
                };
                json!({ "Id": id, "Arn": arn })
            })
            .collect();
        let (status, body) = call(
            &emulator,
            "PutTargets",
            &json!({ "Rule": rule, "Targets": targets }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutTargets answered {status} {body}"
        );

        let expected_failed: Vec<&str> = entries
            .iter()
            .filter(|(_, well_formed)| !well_formed)
            .map(|(id, _)| id.as_str())
            .collect();
        let failed = collection(&body, "FailedEntries", case);
        let reported: Vec<&str> = failed
            .iter()
            .map(|entry| entry["TargetId"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            reported, expected_failed,
            "case {case:?}: expected FailedEntries for {expected_failed:?}, got {body}"
        );
        assert_eq!(
            body["FailedEntryCount"],
            json!(expected_failed.len()),
            "case {case:?}: expected FailedEntryCount {}, got {body}",
            expected_failed.len()
        );

        // Round trip: exactly the entries that did not fail are the ones the
        // reader reports, and that reader also always carries its list.
        let expected_stored: Vec<&str> = entries
            .iter()
            .filter(|(_, well_formed)| *well_formed)
            .map(|(id, _)| id.as_str())
            .collect();
        let (status, body) =
            call(&emulator, "ListTargetsByRule", &json!({ "Rule": rule })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTargetsByRule answered {status} {body}"
        );
        let stored: Vec<&str> = collection(&body, "Targets", case)
            .iter()
            .map(|target| target["Id"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            stored, expected_stored,
            "case {case:?}: expected the stored targets {expected_stored:?}, got {body}"
        );
    }
}

/// `RemoveTargets` always reports `FailedEntries`, holding the ids the rule did
/// not carry: the rows differ along how many ids are asked for and how many of
/// them name a stored target, including the all-success row whose list is empty.
#[tokio::test]
async fn remove_targets_always_reports_a_failed_entries_list() {
    // (case, targets stored first, ids to remove)
    let cases: Vec<(&str, usize, Vec<&str>)> = vec![
        ("remove-the-only-target", 1, vec!["t1"]),
        ("remove-every-target", 3, vec!["t1", "t2", "t3"]),
        ("remove-some-of-them", 3, vec!["t2"]),
        ("remove-an-id-the-rule-never-had", 2, vec!["ghost"]),
        ("mixed-outcomes", 3, vec!["t1", "ghost", "t3"]),
        ("every-id-unknown", 1, vec!["ghost-a", "ghost-b"]),
        ("rule-already-drained", 0, vec!["t1"]),
    ];

    for (case, stored_count, ids) in cases {
        let emulator = emulator();
        let rule = "target-removal";
        put_rule(&emulator, rule).await;
        let stored = fill(&emulator, rule, stored_count).await;

        let (status, body) = call(
            &emulator,
            "RemoveTargets",
            &json!({ "Rule": rule, "Ids": ids }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: RemoveTargets answered {status} {body}"
        );

        let expected_failed: Vec<&str> = ids
            .iter()
            .filter(|id| !stored.iter().any(|held| held.as_str() == **id))
            .copied()
            .collect();
        let failed = collection(&body, "FailedEntries", case);
        let reported: Vec<&str> = failed
            .iter()
            .map(|entry| entry["TargetId"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            reported, expected_failed,
            "case {case:?}: expected FailedEntries for {expected_failed:?}, got {body}"
        );
        assert_eq!(
            body["FailedEntryCount"],
            json!(expected_failed.len()),
            "case {case:?}: expected FailedEntryCount {}, got {body}",
            expected_failed.len()
        );

        // Round trip: the ids that were removed are gone from the reader, and
        // the ids that were not asked for are still there.
        let expected_left: Vec<&str> = stored
            .iter()
            .map(String::as_str)
            .filter(|held| !ids.contains(held))
            .collect();
        let (status, body) =
            call(&emulator, "ListTargetsByRule", &json!({ "Rule": rule })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTargetsByRule answered {status} {body}"
        );
        let left: Vec<&str> = collection(&body, "Targets", case)
            .iter()
            .map(|target| target["Id"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            left, expected_left,
            "case {case:?}: expected the remaining targets {expected_left:?}, got {body}"
        );
    }
}

/// `ListTargetsByRule` always reports a `Targets` list, on EVERY page: the rows
/// differ along the stored target count — from a rule with none at all up to the
/// per-rule quota — and along the page size, including each published `Limit`
/// bound. The pages concatenated are the stored list however it is cut up.
#[tokio::test]
async fn list_targets_by_rule_always_reports_a_targets_list() {
    let limits: [Option<i64>; 4] = [None, Some(LIMIT_MIN), Some(2), Some(LIMIT_MAX)];
    for stored_count in 0..=MAX_TARGETS_PER_RULE {
        for limit in limits {
            let case = format!("{stored_count} stored, limit {limit:?}");
            let emulator = emulator();
            let rule = "target-listing";
            put_rule(&emulator, rule).await;
            let stored = fill(&emulator, rule, stored_count).await;

            let mut seen: Vec<String> = Vec::new();
            let mut next: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let mut request = json!({ "Rule": rule });
                if let Some(limit) = limit {
                    request["Limit"] = json!(limit);
                }
                if let Some(token) = &next {
                    request["NextToken"] = json!(token);
                }
                let (status, body) = call(&emulator, "ListTargetsByRule", &request).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: ListTargetsByRule answered {status} {body}"
                );
                let page = collection(&body, "Targets", &case);
                pages += 1;
                assert!(
                    pages <= stored_count + 1,
                    "case {case:?}: ListTargetsByRule issued {pages} pages for \
                     {stored_count} targets"
                );
                for target in page {
                    seen.push(target["Id"].as_str().unwrap_or_default().to_string());
                }
                next = body["NextToken"].as_str().map(ToString::to_string);
                if next.is_none() {
                    break;
                }
            }

            assert_eq!(
                seen, stored,
                "case {case:?}: expected the pages to concatenate to {stored:?}, got {seen:?}"
            );
            if stored_count == 0 {
                assert_eq!(
                    pages, 1,
                    "case {case:?}: a rule with no targets is ONE page of [], got {pages} pages"
                );
            }
        }
    }
}
