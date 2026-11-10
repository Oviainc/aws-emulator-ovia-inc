// SPDX-License-Identifier: CC0-1.0

//! The client-error refusals this service answers with, worded as AWS words them.
//!
//! Three rules are pinned here, each over a TABLE of inputs rather than the one request
//! that exposed it:
//!
//! * a `ScalingPlanName` outside the shape's modelled `@pattern` is refused with HTTP 400
//!   `ValidationException` in the Smithy constraint-violation envelope — the `__type` /
//!   lowercase `message` pair, carrying
//!   `1 validation error detected: Value '<value>' at '<member path>' failed to satisfy
//!   constraint: Member must satisfy regular expression pattern: <pattern>` — on EVERY
//!   operation that carries one, because the constraint belongs to the shape and not to
//!   one operation. A name INSIDE the pattern is never refused for it, and the sibling
//!   `@length` bound on the same member keeps its own clause;
//! * a write whose `ScalingInstructions` list is empty is refused with the fixed sentence
//!   `Must provide at least 1 scaling instruction`, on both write paths;
//! * a `NextToken` this service cannot decode is refused with the fixed sentence
//!   `The NextToken value is not valid`, on both paginated reads.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::{
    FORECAST_SCALABLE_DIMENSION, FORECAST_SERVICE_NAMESPACE,
};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state as plan_state;

use crate::scaling_plan_lifecycle::{
    error_code, error_message, now_secs, simple_instructions, stack_source, Emulator,
};

/// The only scaling-plan version the model documents as valid.
const ONLY_VERSION: i64 = 1;

/// The Auto Scaling group [`simple_instructions`] instructs, which is also the resource
/// the forecast rows name.
const FORECAST_RESOURCE_ID: &str = "autoScalingGroup/my-asg";

/// The pattern body a constraint-violation refusal states for a `ScalingPlanName`.
///
/// Source: the model's own `@pattern` on the shape —
/// `smithy_query(query="shape",shape="ScalingPlanName")` reports
/// `^[\p{Print}&&[^|:/]]+$`, and a refusal states the expression between the anchors,
/// since the anchoring is how the model says "the whole value".
const PATTERN_BODY: &str = r"[\p{Print}&&[^|:/]]+";

/// The documented bounds of `ScalingPlanName`: `(string, len 1..128)`
/// (`smithy_query(query="shape",shape="ScalingPlanName")`).
const NAME_MIN: usize = 1;
const NAME_MAX: usize = 128;

/// The constraint-violation sentence the pattern rule requires for `value` at
/// `member_path`.
fn expected_pattern_refusal(member_path: &str, value: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member_path}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {PATTERN_BODY}"
    )
}

/// A `GetScalingPlanResourceForecastData` request carrying every member the model marks
/// required, over a window well inside the documented limits — so nothing but the name
/// can refuse it.
fn forecast_request(name: &str) -> Value {
    let start = now_secs();
    json!({
        "ScalingPlanName": name,
        "ScalingPlanVersion": ONLY_VERSION,
        "ServiceNamespace": FORECAST_SERVICE_NAMESPACE,
        "ResourceId": FORECAST_RESOURCE_ID,
        "ScalableDimension": FORECAST_SCALABLE_DIMENSION,
        "ForecastDataType": "CapacityForecast",
        "StartTime": start,
        "EndTime": start + 3_600,
    })
}

/// Every operation that carries a `ScalingPlanName`, with the request that carries it and
/// the validation path that names it.
///
/// Source: `smithy_query(query="shape",shape="CreateScalingPlanRequest,
/// UpdateScalingPlanRequest,DeleteScalingPlanRequest,DescribeScalingPlanResourcesRequest,
/// GetScalingPlanResourceForecastDataRequest,DescribeScalingPlansRequest")` — five
/// requests carry the shape as a required top-level member and one carries a LIST of it,
/// whose entries name themselves by their 1-based position.
fn name_carriers(name: &str) -> Vec<(&'static str, String, Value)> {
    vec![
        (
            "CreateScalingPlan",
            "scalingPlanName".to_string(),
            json!({
                "ScalingPlanName": name,
                "ApplicationSource": stack_source(9),
                "ScalingInstructions": simple_instructions(),
            }),
        ),
        (
            "UpdateScalingPlan",
            "scalingPlanName".to_string(),
            json!({
                "ScalingPlanName": name,
                "ScalingPlanVersion": ONLY_VERSION,
                "ScalingInstructions": simple_instructions(),
            }),
        ),
        (
            "DeleteScalingPlan",
            "scalingPlanName".to_string(),
            json!({ "ScalingPlanName": name, "ScalingPlanVersion": ONLY_VERSION }),
        ),
        (
            "DescribeScalingPlanResources",
            "scalingPlanName".to_string(),
            json!({ "ScalingPlanName": name, "ScalingPlanVersion": ONLY_VERSION }),
        ),
        (
            "GetScalingPlanResourceForecastData",
            "scalingPlanName".to_string(),
            forecast_request(name),
        ),
        (
            "DescribeScalingPlans",
            "scalingPlanNames.1.member".to_string(),
            json!({ "ScalingPlanNames": [name] }),
        ),
        (
            "DescribeScalingPlans",
            "scalingPlanNames.2.member".to_string(),
            json!({ "ScalingPlanNames": ["a-legal-name", name] }),
        ),
    ]
}

/// How a name can fall outside the pattern: the rows every carrier above is driven over.
///
/// They differ along what the rule generalises over — WHICH character the pattern's
/// printable set excludes ('|', ':' and '/', the three the member's own documentation
/// names), WHERE it sits in the name, HOW MANY of them there are, whether the offending
/// character is a control character rather than one of the three, and the LENGTH of the
/// name (including one at the modelled 128-character bound, so the row is refused for its
/// pattern and not for its length).
fn names_outside_the_pattern() -> Vec<(&'static str, String)> {
    vec![
        ("a vertical bar inside the name", "bad|name".to_string()),
        ("a colon inside the name", "bad:name".to_string()),
        ("a forward slash inside the name", "bad/name".to_string()),
        ("a vertical bar alone", "|".to_string()),
        ("a colon alone", ":".to_string()),
        ("a forward slash alone", "/".to_string()),
        ("a leading vertical bar", "|leading".to_string()),
        ("a trailing forward slash", "trailing/".to_string()),
        ("all three excluded characters", "all|three:at/once".to_string()),
        ("a line break", "line\nbreak".to_string()),
        ("a tab", "tab\there".to_string()),
        ("a control character", "bell\u{7}ring".to_string()),
        (
            "an excluded character beside non-ASCII text",
            "üñî|plan".to_string(),
        ),
        (
            "an excluded character at the length bound",
            format!("{}|", "a".repeat(NAME_MAX - 1)),
        ),
    ]
}

/// Names the pattern admits: no row below may be refused for the pattern.
///
/// They span the printable set the pattern keeps — the shortest and the longest name the
/// member's `@length` allows, spaces, punctuation (the backslash this crate escapes its
/// own identity keys with included), digits and non-ASCII text.
fn names_inside_the_pattern() -> Vec<(&'static str, String)> {
    vec![
        ("a plain name", "plan".to_string()),
        ("one character", "a".repeat(NAME_MIN)),
        ("the longest name allowed", "a".repeat(NAME_MAX)),
        ("spaces and punctuation", "my plan #2 (staging)".to_string()),
        ("a backslash", r"plan\with\backslash".to_string()),
        ("dots, dashes and tildes", "plan.with-dots_and~tilde".to_string()),
        ("non-ASCII text", "üñî-plan".to_string()),
    ]
}

#[tokio::test]
async fn a_scaling_plan_name_outside_its_pattern_is_refused_wherever_the_shape_is_carried() {
    let emulator = Emulator::new();

    for (label, name) in names_outside_the_pattern() {
        for (operation, member_path, request) in name_carriers(&name) {
            let (status, body) = emulator.call(operation, &request).await;
            let expected = expected_pattern_refusal(&member_path, &name);
            assert_eq!(
                status, 400,
                "case {operation} at {member_path} ({label}, name {name:?}): expected HTTP \
                 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {operation} at {member_path} ({label}, name {name:?}): expected a \
                 ValidationException, got {body}"
            );
            // The constraint-violation envelope keys the message LOWERCASE, which is a
            // different envelope from the `Message`-keyed service errors this API's own
            // modelled error shapes carry.
            let reported = body.get("message").and_then(Value::as_str);
            assert_eq!(
                reported,
                Some(expected.as_str()),
                "case {operation} at {member_path} ({label}, name {name:?}): expected \
                 $.message {expected:?}, got {body}"
            );
            assert!(
                body.get("Message").is_none(),
                "case {operation} at {member_path} ({label}, name {name:?}): the \
                 constraint-violation envelope carries no 'Message' member, got {body}"
            );
        }
    }

    // Nothing was stored under any of those names: a create AWS stops at the API
    // boundary leaves no plan behind.
    let body = emulator.ok("DescribeScalingPlans", json!({})).await;
    let plans = body["ScalingPlans"].as_array().cloned().unwrap_or_default();
    assert!(
        plans.is_empty(),
        "case refused creates: no plan may be stored under a refused name, got {plans:?}"
    );
}

#[tokio::test]
async fn a_scaling_plan_name_inside_its_pattern_is_never_refused_for_it() {
    for (label, name) in names_inside_the_pattern() {
        // A fresh emulator per row, so a row is judged on its own name alone: the
        // carriers include a create, and a plan one row stored would make the next row's
        // create answer a different refusal.
        let emulator = Emulator::new();
        for (operation, member_path, request) in name_carriers(&name) {
            let (status, body) = emulator.call(operation, &request).await;
            let reported = error_message(&body);
            assert!(
                !reported.contains("regular expression pattern"),
                "case {operation} at {member_path} ({label}, name {name:?}): a name the \
                 pattern admits must not earn the pattern refusal, got {status} with {body}"
            );
        }
    }

    // The same names ROUND TRIP: each is stored under a source of its own and reads back
    // through `DescribeScalingPlans` exactly as sent.
    let emulator = Emulator::new();
    let mut stored: Vec<String> = Vec::new();
    for (index, (label, name)) in names_inside_the_pattern().into_iter().enumerate() {
        let created = emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": name,
                    "ApplicationSource": stack_source(index + 1),
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
        assert_eq!(
            created["ScalingPlanVersion"].as_i64(),
            Some(ONLY_VERSION),
            "case {label} ({name:?}): a create answers with the only modelled version, got \
             {created}"
        );
        stored.push(name);
    }
    let body = emulator
        .ok("DescribeScalingPlans", json!({ "MaxResults": 50 }))
        .await;
    let mut read_back: Vec<String> = body["ScalingPlans"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|plan| plan["ScalingPlanName"].as_str().unwrap_or_default().to_string())
        .collect();
    read_back.sort();
    stored.sort();
    assert_eq!(
        read_back, stored,
        "case round trip: every legal name must read back as stored, got {read_back:?}"
    );
}

#[tokio::test]
async fn the_length_bound_on_a_scaling_plan_name_keeps_its_own_clause() {
    let emulator = Emulator::new();

    // The published bounds and one past the upper one. The `@length` minimum is covered
    // by the one-character row above; a name past the maximum is refused for its LENGTH,
    // with the length clause rather than the pattern clause.
    let past_the_bound = "a".repeat(NAME_MAX + 1);
    let (status, body) = emulator
        .call(
            "DescribeScalingPlanResources",
            &json!({ "ScalingPlanName": past_the_bound, "ScalingPlanVersion": ONLY_VERSION }),
        )
        .await;
    let reported = error_message(&body);
    assert_eq!(
        (status, error_code(&body)),
        (400, "ValidationException"),
        "case a name one character past the 128-character bound: expected a 400 \
         ValidationException, got {status} with {body}"
    );
    assert!(
        reported.contains(&format!(
            "Member must have length less than or equal to {NAME_MAX}"
        )),
        "case a name one character past the 128-character bound: expected the length \
         clause, got {reported:?}"
    );

    // Both published bounds are served: a name AT the maximum, and one at the minimum.
    for (label, name) in [
        ("the shortest name allowed", "a".repeat(NAME_MIN)),
        ("the longest name allowed", "a".repeat(NAME_MAX)),
    ] {
        let (status, body) = emulator
            .call(
                "DescribeScalingPlanResources",
                &json!({ "ScalingPlanName": name, "ScalingPlanVersion": ONLY_VERSION }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label}: a name inside the modelled bounds is served, got {status} with \
             {body}"
        );
    }
}

#[tokio::test]
async fn an_empty_scaling_instructions_list_is_refused_in_awss_words() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "stored plan",
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    emulator.make_all_due();

    // Both write paths take the list, so both refuse an empty one with the same fixed
    // sentence — which interpolates nothing from the request, so a row differs only in
    // which operation received it and what else the request carries.
    let cases: Vec<(&str, Value)> = vec![
        (
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "new plan",
                "ApplicationSource": stack_source(2),
                "ScalingInstructions": [],
            }),
        ),
        (
            "UpdateScalingPlan",
            json!({
                "ScalingPlanName": "stored plan",
                "ScalingPlanVersion": ONLY_VERSION,
                "ScalingInstructions": [],
            }),
        ),
        (
            "UpdateScalingPlan",
            json!({
                "ScalingPlanName": "stored plan",
                "ScalingPlanVersion": ONLY_VERSION,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": [],
            }),
        ),
    ];

    for (operation, request) in cases {
        let (status, body) = emulator.call(operation, &request).await;
        let reported = error_message(&body);
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {operation} with an empty instruction list: expected a 400 \
             ValidationException, got {status} with {body}"
        );
        assert_eq!(
            reported,
            plan_state::EMPTY_SCALING_INSTRUCTIONS,
            "case {operation} with an empty instruction list: expected {:?}, got {reported:?}",
            plan_state::EMPTY_SCALING_INSTRUCTIONS
        );
    }
}

#[tokio::test]
async fn an_undecodable_next_token_is_refused_in_awss_words() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "paged plan",
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    emulator.make_all_due();

    // Both paginated reads take a `NextToken`, and the sentence is fixed text that
    // echoes nothing of the rejected token — so the rows differ along the SHAPE of the
    // string offered (text that is not hex at all, hex that is not a cursor, a cursor
    // minted under the sibling read's namespace, a re-spelled cursor, the empty string)
    // and along which read received it.
    let plans_cursor = plan_state::encode_cursor("plans", "paged plan");
    let resources_cursor = plan_state::encode_cursor("plan-resources", "autoscaling/x/y");
    let tokens: Vec<(&str, String)> = vec![
        ("plain text", "not-a-token".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex of foreign text", hex::encode("other:value")),
        ("the empty string", String::new()),
        ("the sibling read's cursor", plans_cursor.clone()),
        ("the sibling read's cursor", resources_cursor.clone()),
        ("an upper-cased cursor", plans_cursor.to_uppercase()),
        ("an upper-cased cursor", resources_cursor.to_uppercase()),
    ];

    let reads: [(&str, Value); 2] = [
        ("DescribeScalingPlans", json!({})),
        (
            "DescribeScalingPlanResources",
            json!({ "ScalingPlanName": "paged plan", "ScalingPlanVersion": ONLY_VERSION }),
        ),
    ];

    for (label, token) in tokens {
        for (operation, base) in &reads {
            // A cursor this read mints itself is a token it must ACCEPT, so it is no row
            // of this table.
            if plan_state::decode_cursor(
                match *operation {
                    "DescribeScalingPlans" => "plans",
                    _ => "plan-resources",
                },
                &token,
            )
            .is_some()
            {
                continue;
            }
            let mut request = base.clone();
            request["NextToken"] = json!(token);
            let (status, body) = emulator.call(operation, &request).await;
            let reported = error_message(&body);
            assert_eq!(
                (status, error_code(&body)),
                (400, "InvalidNextTokenException"),
                "case {operation} NextToken={token:?} ({label}): expected a 400 \
                 InvalidNextTokenException, got {status} with {body}"
            );
            assert_eq!(
                reported,
                plan_state::INVALID_NEXT_TOKEN,
                "case {operation} NextToken={token:?} ({label}): expected {:?}, got \
                 {reported:?}",
                plan_state::INVALID_NEXT_TOKEN
            );
        }
    }
}
