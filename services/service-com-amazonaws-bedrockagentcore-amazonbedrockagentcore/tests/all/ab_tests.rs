// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AgentCore A/B testing plane: `CreateABTest` and
//! `ListABTests`.
//!
//! The rule under test: a create MINTS an `abTestId` matching its modelled pattern and
//! the ARN that addresses it, persists the configuration the request asked for (name,
//! description, gateway, variants, filter, evaluation configuration, role and tags) with
//! the instant it was created, and reports that stored record back; and `ListABTests`
//! reports exactly the tests this emulator holds — newest first, one `maxResults`-sized
//! page at a time with an opaque `nextToken` resuming after it, each summary carrying
//! the status the clock implies (`CREATING` until the creation is provisioned, `ACTIVE`
//! from then on) — while a `maxResults` outside its published range, or a `nextToken`
//! this listing never issued, is refused with a 400 `ValidationException`.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator serves
//! with, so the wire shape (routes, status codes, `restJson1` bodies, modelled error
//! codes) is exercised, not just the handler functions.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

// The emulator, the request sender, the four routes of the family, the JSON readers and
// the fixture ARNs are shared with the lifecycle suite rather than written twice.
use super::ab_test_support::*;

/// `ABTestId`'s modelled pattern, `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`,
/// checked without a regex dependency: the discriminator is the last ten characters
/// after the final `-`, and everything before it is an identifier starting with a
/// letter.
fn is_modelled_ab_test_id(id: &str) -> bool {
    let Some((prefix, suffix)) = id.rsplit_once('-') else {
        return false;
    };
    suffix.len() == 10
        && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
        && (1..=100).contains(&prefix.len())
        && prefix.starts_with(|c: char| c.is_ascii_alphabetic())
        && prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// `ABTestArn`'s modelled pattern,
/// `^arn:aws[a-zA-Z-]*:bedrock-agentcore:[a-z0-9-]+:[0-9]{12}:ab-test/<abTestId>$`, over
/// the region the request was signed for and the test the ARN addresses.
fn is_modelled_ab_test_arn(arn: &str, ab_test_id: &str) -> bool {
    let segments: Vec<&str> = arn.splitn(6, ':').collect();
    segments.len() == 6
        && segments[0] == "arn"
        && segments[1].starts_with("aws")
        && segments[2] == "bedrock-agentcore"
        && segments[3] == "us-east-1"
        && segments[4].len() == 12
        && segments[4].bytes().all(|b| b.is_ascii_digit())
        && segments[5] == format!("ab-test/{ab_test_id}")
}

/// The summaries one `ListABTests` page carries.
fn page(body: &Value) -> &Vec<Value> {
    member(body, "abTests")
        .as_array()
        .unwrap_or_else(|| panic!("expected abTests to be a list, got {body:?}"))
}

/// Every A/B test the emulator lists, following `nextToken` until the last page, with
/// `maxResults` asked for on each request.
async fn list_every_ab_test(router: &MultiServiceRouter, max_results: i64) -> Vec<Value> {
    let mut summaries: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // One more request than there can be pages, so a token that never clears is caught
    // here instead of hanging the suite.
    for _ in 0..64 {
        let query = match token.as_deref() {
            None => format!("maxResults={max_results}"),
            Some(next) => format!("maxResults={max_results}&nextToken={next}"),
        };
        let (status, body) = list_ab_tests(router, &query).await;
        assert_eq!(
            status, 200,
            "paging with maxResults={max_results}: expected HTTP 200, got {status} with {body:?}"
        );
        let returned = page(&body);
        assert!(
            returned.len() as i64 <= max_results,
            "paging with maxResults={max_results}: a page carried {} summaries",
            returned.len()
        );
        summaries.extend(returned.iter().cloned());
        match member(&body, "nextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return summaries,
        }
    }
    panic!("paging with maxResults={max_results} never reached a last page");
}

/// Move a stored test's creation instant `by` further into the past, so a case can reach
/// the far side of the provisioning span without sleeping for it. This is the test
/// reaching into the store the handlers share, not an emulator behaviour.
fn age_ab_test(engine: &engine::InMemoryEngine, ab_test_id: &str, by: Duration) {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let mut store = state.lock();
    let test = store
        .ab_tests
        .get_mut(ab_test_id)
        .expect("the A/B test that was just created");
    test.created_at -= by;
}

/// How many A/B tests this emulator holds — the check that a refused or replayed create
/// wrote nothing new.
fn ab_test_count(engine: &engine::InMemoryEngine) -> usize {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let store = state.lock();
    store.ab_tests.len()
}

/// A created A/B test is reported from what was stored, and `ListABTests` reports the
/// same record back — for every combination of the optional members
/// `CreateABTestRequest` models, both variant configuration forms, both members of the
/// evaluation-configuration union and several traffic splits.
#[tokio::test]
async fn a_created_ab_test_round_trips_through_list_ab_tests() {
    let (router, _engine) = emulator();

    // Rows differ along the name's length (`ABTestName` is ^[a-zA-Z][a-zA-Z0-9_]{0,47}$,
    // so one character and 48 are its bounds), the description (absent and both
    // published bounds of `ABTestDescription` len 1..200), the traffic split, the
    // variant configuration form, the evaluation-configuration union member, whether a
    // gateway filter and tags were supplied, and whether the test was enabled on create.
    let cases: Vec<(&str, Value)> = vec![
        (
            "required-members-only",
            minimal_request("checkout_flow"),
        ),
        (
            "shortest-name-shortest-description-enabled-on-create",
            json!({
                "name": "a",
                "description": "d",
                "gatewayArn": GATEWAY_ARN,
                "roleArn": ROLE_ARN,
                "enableOnCreate": true,
                "variants": [
                    variant("C", 1, target_configuration("control")),
                    variant("T1", 99, bundle_configuration()),
                ],
                "evaluationConfig": {"onlineEvaluationConfigArn": EVALUATION_CONFIG_ARN},
            }),
        ),
        (
            "longest-name-longest-description-per-variant-evaluation",
            json!({
                "name": format!("n{}", "a".repeat(47)),
                "description": "d".repeat(200),
                "gatewayArn": OTHER_GATEWAY_ARN,
                "roleArn": ROLE_ARN,
                "enableOnCreate": false,
                "variants": [
                    variant("C", 99, bundle_configuration()),
                    variant("T1", 1, target_configuration("treatment")),
                ],
                "evaluationConfig": {"perVariantOnlineEvaluationConfig": [
                    {"name": "C", "onlineEvaluationConfigArn": EVALUATION_CONFIG_ARN},
                    {"name": "T1", "onlineEvaluationConfigArn": TREATMENT_EVALUATION_CONFIG_ARN},
                ]},
            }),
        ),
        (
            "filtered-gateway-with-tags-enabled-on-create",
            json!({
                "name": "search_ranking",
                "description": "Compares two ranking bundles",
                "gatewayArn": OTHER_GATEWAY_ARN,
                "roleArn": ROLE_ARN,
                "enableOnCreate": true,
                // `TargetPathList` is modelled exactly one long (source: smithy_query
                // query="shape" shape="TargetPathList").
                "gatewayFilter": {"targetPaths": ["/search"]},
                "tags": {"team": "relevance", "env": "test"},
                "variants": [
                    variant("T1", 25, bundle_configuration()),
                    variant("C", 75, target_configuration("control")),
                ],
                "evaluationConfig": {"onlineEvaluationConfigArn": EVALUATION_CONFIG_ARN},
            }),
        ),
        (
            "no-tags-and-an-empty-tag-map",
            json!({
                "name": "summarizer_v2",
                "gatewayArn": GATEWAY_ARN,
                "roleArn": ROLE_ARN,
                // `TagsMap` admits an empty map (len 0..50).
                "tags": {},
                "variants": [
                    variant("C", 50, bundle_configuration()),
                    variant("T1", 50, bundle_configuration()),
                ],
                "evaluationConfig": {"perVariantOnlineEvaluationConfig": [
                    {"name": "C", "onlineEvaluationConfigArn": EVALUATION_CONFIG_ARN},
                    {"name": "T1", "onlineEvaluationConfigArn": EVALUATION_CONFIG_ARN},
                ]},
            }),
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, request) in cases {
        let (status, created) = create_ab_test(&router, request.clone()).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateABTest, got {status} with {created:?}"
        );

        let ab_test_id = text(&created, "abTestId").to_string();
        assert!(
            is_modelled_ab_test_id(&ab_test_id),
            "case {case:?}: expected an abTestId matching ^[a-zA-Z][a-zA-Z0-9-_]{{0,99}}-[a-zA-Z0-9]{{10}}$, got {ab_test_id:?}"
        );
        let requested_name = request
            .get("name")
            .and_then(Value::as_str)
            .expect("every case names its test");
        assert!(
            ab_test_id.starts_with(&format!("{requested_name}-")),
            "case {case:?}: expected the id to be minted from the name {requested_name:?}, got {ab_test_id:?}"
        );
        assert!(
            minted.insert(ab_test_id.clone()),
            "case {case:?}: expected an abTestId no earlier create minted, got {ab_test_id:?}"
        );
        let ab_test_arn = text(&created, "abTestArn").to_string();
        assert!(
            is_modelled_ab_test_arn(&ab_test_arn, &ab_test_id),
            "case {case:?}: expected an ARN addressing {ab_test_id:?} in the signed region, got {ab_test_arn:?}"
        );
        assert_eq!(
            text(&created, "name"),
            requested_name,
            "case {case:?}: expected the name the request asked for, got {created:?}"
        );
        assert_eq!(
            text(&created, "status"),
            "CREATING",
            "case {case:?}: expected a just-created test to still be CREATING, got {created:?}"
        );
        let expected_execution = if request.get("enableOnCreate") == Some(&json!(true)) {
            "RUNNING"
        } else {
            "NOT_STARTED"
        };
        assert_eq!(
            text(&created, "executionStatus"),
            expected_execution,
            "case {case:?}: expected executionStatus {expected_execution} for enableOnCreate {:?}, got {created:?}",
            request.get("enableOnCreate")
        );
        let created_at = member(&created, "createdAt").clone();
        assert!(
            created_at.is_number(),
            "case {case:?}: expected createdAt as epoch seconds (smithy.api#Timestamp on restJson1), got {created_at:?}"
        );

        // The listing reports the record that was just stored.
        let summaries = list_every_ab_test(&router, 100).await;
        let listed = summaries
            .iter()
            .find(|summary| text(summary, "abTestId") == ab_test_id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected ListABTests to report {ab_test_id:?}, got {summaries:?}")
            });
        assert_eq!(
            text(listed, "abTestArn"),
            ab_test_arn,
            "case {case:?}: expected the ARN the create minted, got {listed:?}"
        );
        assert_eq!(
            text(listed, "name"),
            requested_name,
            "case {case:?}: expected the stored name, got {listed:?}"
        );
        assert_eq!(
            member(listed, "description"),
            request.get("description").unwrap_or(&Value::Null),
            "case {case:?}: expected the description the request asked for, got {listed:?}"
        );
        assert_eq!(
            member(listed, "gatewayArn"),
            request.get("gatewayArn").unwrap_or(&Value::Null),
            "case {case:?}: expected the gateway the request named, got {listed:?}"
        );
        assert_eq!(
            text(listed, "status"),
            "CREATING",
            "case {case:?}: expected the listing to report the test as CREATING, got {listed:?}"
        );
        assert_eq!(
            text(listed, "executionStatus"),
            expected_execution,
            "case {case:?}: expected the stored execution status, got {listed:?}"
        );
        assert_eq!(
            member(listed, "createdAt"),
            &created_at,
            "case {case:?}: expected the creation instant the create reported, got {listed:?}"
        );
        assert_eq!(
            member(listed, "updatedAt"),
            &created_at,
            "case {case:?}: expected a test nothing has changed to be updated at its creation instant, got {listed:?}"
        );
    }

    // Every test created above is listed, and nothing else is.
    let summaries = list_every_ab_test(&router, 100).await;
    let listed: BTreeSet<String> = summaries
        .iter()
        .map(|summary| text(summary, "abTestId").to_string())
        .collect();
    assert_eq!(
        listed, minted,
        "expected ListABTests to report exactly the tests that were created"
    );
}

/// `ListABTests` reports the tests newest first and pages them: whatever `maxResults` a
/// caller asks for, following `nextToken` to the end yields every test exactly once, in
/// the same order, and the last page carries no token.
#[tokio::test]
async fn list_ab_tests_pages_the_whole_list_newest_first() {
    let (router, _engine) = emulator();

    // Seven tests, so a page size that divides the list and one that does not are both
    // exercised.
    let mut created_order: Vec<String> = Vec::new();
    for index in 0..7 {
        let (status, created) = create_ab_test(&router, minimal_request(&format!("test_{index}")))
            .await;
        assert_eq!(
            status, 202,
            "creating test_{index}: expected HTTP 202, got {status} with {created:?}"
        );
        created_order.push(text(&created, "abTestId").to_string());
    }
    let mut newest_first = created_order.clone();
    newest_first.reverse();

    // `maxResults` is range 1..100 (source: smithy_query query="operation"
    // operation="ListABTests"): both published bounds, a size that divides the list, one
    // that does not, and one larger than it.
    for max_results in [1_i64, 2, 3, 7, 8, 100] {
        let summaries = list_every_ab_test(&router, max_results).await;
        let order: Vec<String> = summaries
            .iter()
            .map(|summary| text(summary, "abTestId").to_string())
            .collect();
        assert_eq!(
            order, newest_first,
            "case maxResults={max_results}: expected every test exactly once, newest first"
        );
    }

    // A request that names no `maxResults` is served the whole list in one page.
    let (status, body) = list_ab_tests(&router, "").await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 with no maxResults, got {status} with {body:?}"
    );
    let order: Vec<String> = page(&body)
        .iter()
        .map(|summary| text(summary, "abTestId").to_string())
        .collect();
    assert_eq!(
        order, newest_first,
        "expected the default page to carry the whole list, newest first, got {body:?}"
    );
    assert_eq!(
        member(&body, "nextToken"),
        &Value::Null,
        "expected no nextToken on a page that carries the whole list, got {body:?}"
    );
}

/// A listing is a READ: it reports the same page twice, and an emulator holding no A/B
/// test lists none rather than failing.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator();

    let (status, empty) = list_ab_tests(&router, "maxResults=100").await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from a listing with nothing to list, got {status} with {empty:?}"
    );
    assert!(
        page(&empty).is_empty(),
        "expected no summaries before anything was created, got {empty:?}"
    );
    assert_eq!(
        member(&empty, "nextToken"),
        &Value::Null,
        "expected no nextToken on an empty listing, got {empty:?}"
    );

    let (_, created) = create_ab_test(&router, minimal_request("read_only_check")).await;
    let ab_test_id = text(&created, "abTestId").to_string();
    let (_, first) = list_ab_tests(&router, "maxResults=100").await;
    let (_, again) = list_ab_tests(&router, "maxResults=100").await;
    assert_eq!(
        first, again,
        "expected a second listing to report the same page, got {again:?}"
    );
    assert_eq!(
        ab_test_count(&engine),
        1,
        "expected listing to store nothing of its own"
    );
    assert_eq!(
        text(&first["abTests"][0], "abTestId"),
        ab_test_id,
        "expected the created test to be the one listed, got {first:?}"
    );
}

/// A created test is `CREATING` until its creation is provisioned and `ACTIVE` from that
/// instant on, with no call needed to make it so — and the transition is reported by the
/// listing, which writes nothing to make it.
#[tokio::test]
async fn a_listed_status_follows_the_provisioning_the_clock_owes() {
    let (router, engine) = emulator();

    let (_, created) = create_ab_test(&router, minimal_request("provisioning")).await;
    let ab_test_id = text(&created, "abTestId").to_string();

    let span = agentcore::state::AB_TEST_PROVISIONING_SPAN;
    // One second short of the span the test is still being created.
    age_ab_test(&engine, &ab_test_id, span - Duration::from_secs(1));
    let (status, body) = list_ab_tests(&router, "maxResults=100").await;
    assert_eq!(
        (status, text(&body["abTests"][0], "status")),
        (200, "CREATING"),
        "expected CREATING one second before the provisioning span is up, got {status} with {body:?}"
    );

    // At the instant it is up the test is active.
    age_ab_test(&engine, &ab_test_id, Duration::from_secs(1));
    let (status, body) = list_ab_tests(&router, "maxResults=100").await;
    assert_eq!(
        (status, text(&body["abTests"][0], "status")),
        (200, "ACTIVE"),
        "expected ACTIVE once the provisioning span is up, got {status} with {body:?}"
    );
    assert_eq!(
        text(&body["abTests"][0], "executionStatus"),
        "NOT_STARTED",
        "expected a provisioned test that was not enabled on create to have never run, got {body:?}"
    );
}

/// `maxResults` outside the range the model publishes, and a `nextToken` this listing
/// never issued, are refused with a 400 `ValidationException`; the published bounds
/// themselves are served.
#[tokio::test]
async fn a_listing_outside_its_modelled_input_is_refused_with_400() {
    let (router, _engine) = emulator();
    let (status, created) = create_ab_test(&router, minimal_request("bounds")).await;
    assert_eq!(
        status, 202,
        "expected the test under the bounds cases to be created, got {status} with {created:?}"
    );

    // `maxResults` is range 1..100: one case on each side of it, and a negative one.
    let refused: Vec<(&str, &str)> = vec![
        ("max-results-below-the-range", "maxResults=0"),
        ("max-results-above-the-range", "maxResults=101"),
        ("negative-max-results", "maxResults=-1"),
        ("next-token-this-listing-never-issued", "nextToken=not-a-token"),
        // A token another paginated listing of this service minted does not resume this
        // one.
        ("next-token-of-another-listing", "nextToken=c2Vzc2lvbnM6MA=="),
    ];
    for (case, query) in refused {
        let (status, body) = list_ab_tests(&router, query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
    }

    for (case, query) in [
        ("lowest-max-results", "maxResults=1"),
        ("highest-max-results", "maxResults=100"),
    ] {
        let (status, body) = list_ab_tests(&router, query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a published bound to be served, got {status} with {body:?}"
        );
        assert_eq!(
            page(&body).len(),
            1,
            "case {case:?}: expected the one created test on the page, got {body:?}"
        );
    }
}

/// `clientToken` is an `@idempotencyToken`: replaying one answers with the test it
/// already created and creates nothing new, so the name it took is not reported as a
/// conflict with itself. A name another test already holds IS a 409
/// `ConflictException` — `name` "must be unique within your account".
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_test_it_already_created() {
    let (router, engine) = emulator();

    // `ClientToken` is len 33..256, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$ (source:
    // smithy_query query="operation" operation="CreateABTest"). Rows differ along the
    // token's length and the body the replay carries, which the service ignores for a
    // token it has already seen.
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "shortest-token-identical-replay",
            "a".repeat(33),
            minimal_request("first_test"),
        ),
        (
            "hyphenated-token-replayed-with-another-body",
            "replay-token-0123456789-abcdefghij-0".to_string(),
            json!({
                "name": "ignored_on_a_replay",
                "description": "ignored too",
                "gatewayArn": OTHER_GATEWAY_ARN,
                "roleArn": ROLE_ARN,
                "enableOnCreate": true,
                "variants": [
                    variant("C", 10, bundle_configuration()),
                    variant("T1", 90, bundle_configuration()),
                ],
                "evaluationConfig": {"onlineEvaluationConfigArn": EVALUATION_CONFIG_ARN},
            }),
        ),
        (
            "longest-token-identical-replay",
            "z".repeat(256),
            minimal_request("third_test"),
        ),
    ];

    let mut expected_tests = 0;
    for (case, token, replay_body) in cases {
        let mut first_request = minimal_request(&format!("tokened_{}", expected_tests));
        first_request["clientToken"] = json!(token);
        let (status, created) = create_ab_test(&router, first_request).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from the first create, got {status} with {created:?}"
        );
        expected_tests += 1;

        let mut replay_request = replay_body;
        replay_request["clientToken"] = json!(token);
        let (status, replayed) = create_ab_test(&router, replay_request).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from the replay, got {status} with {replayed:?}"
        );
        assert_eq!(
            text(&replayed, "abTestId"),
            text(&created, "abTestId"),
            "case {case:?}: expected the replay to answer with the test the token already created, got {replayed:?}"
        );
        assert_eq!(
            member(&replayed, "createdAt"),
            member(&created, "createdAt"),
            "case {case:?}: expected the creation instant of the first create, got {replayed:?}"
        );
        assert_eq!(
            ab_test_count(&engine),
            expected_tests,
            "case {case:?}: expected the replay to create no second test"
        );
    }

    // A name another test already holds is the documented conflict, and nothing is
    // stored for it.
    let (status, created) = create_ab_test(&router, minimal_request("unique_name")).await;
    assert_eq!(
        status, 202,
        "expected the first create of a name to succeed, got {status} with {created:?}"
    );
    expected_tests += 1;
    let (status, conflict) = create_ab_test(&router, minimal_request("unique_name")).await;
    assert_eq!(
        status, 409,
        "expected HTTP 409 for a name that already exists, got {status} with {conflict:?}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictException",
        "expected ConflictException for a duplicate name, got {conflict:?}"
    );
    assert_eq!(
        ab_test_count(&engine),
        expected_tests,
        "expected a refused create to store nothing"
    );
}

/// Input the service cannot route traffic with is refused with a 400
/// `ValidationException`, never accepted and never answered with a 5xx: variants that do
/// not name one control and one treatment, weights that do not sum to 100, and input
/// outside each modelled constraint of the create.
#[tokio::test]
async fn a_create_outside_its_modelled_input_is_refused_with_400() {
    let (router, engine) = emulator();

    let cases: Vec<(&str, Value)> = vec![
        (
            // "Weights across all variants must sum to 100" (source: smithy_query
            // query="shape" shape="Variant", member="weight").
            "weights-below-the-total",
            json!({
                "name": "weights_low",
                "gatewayArn": GATEWAY_ARN,
                "roleArn": ROLE_ARN,
                "variants": [
                    variant("C", 40, target_configuration("control")),
                    variant("T1", 50, target_configuration("treatment")),
                ],
                "evaluationConfig": {"onlineEvaluationConfigArn": EVALUATION_CONFIG_ARN},
            }),
        ),
        (
            "weights-above-the-total",
            json!({
                "name": "weights_high",
                "gatewayArn": GATEWAY_ARN,
                "roleArn": ROLE_ARN,
                "variants": [
                    variant("C", 60, target_configuration("control")),
                    variant("T1", 60, target_configuration("treatment")),
                ],
                "evaluationConfig": {"onlineEvaluationConfigArn": EVALUATION_CONFIG_ARN},
            }),
        ),
        (
            // "Must contain exactly two variants: a control (C) and a treatment (T1)".
            "two-control-variants",
            json!({
                "name": "two_controls",
                "gatewayArn": GATEWAY_ARN,
                "roleArn": ROLE_ARN,
                "variants": [
                    variant("C", 50, target_configuration("control")),
                    variant("C", 50, target_configuration("other")),
                ],
                "evaluationConfig": {"onlineEvaluationConfigArn": EVALUATION_CONFIG_ARN},
            }),
        ),
        (
            "two-treatment-variants",
            json!({
                "name": "two_treatments",
                "gatewayArn": GATEWAY_ARN,
                "roleArn": ROLE_ARN,
                "variants": [
                    variant("T1", 50, bundle_configuration()),
                    variant("T1", 50, bundle_configuration()),
                ],
                "evaluationConfig": {"onlineEvaluationConfigArn": EVALUATION_CONFIG_ARN},
            }),
        ),
        (
            // `VariantList` is modelled exactly two long.
            "one-variant",
            json!({
                "name": "one_variant",
                "gatewayArn": GATEWAY_ARN,
                "roleArn": ROLE_ARN,
                "variants": [variant("C", 100, target_configuration("control"))],
                "evaluationConfig": {"onlineEvaluationConfigArn": EVALUATION_CONFIG_ARN},
            }),
        ),
        (
            // `Variant$weight` is range 1..100.
            "weight-below-its-range",
            json!({
                "name": "weight_zero",
                "gatewayArn": GATEWAY_ARN,
                "roleArn": ROLE_ARN,
                "variants": [
                    variant("C", 0, target_configuration("control")),
                    variant("T1", 100, target_configuration("treatment")),
                ],
                "evaluationConfig": {"onlineEvaluationConfigArn": EVALUATION_CONFIG_ARN},
            }),
        ),
        (
            // `ABTestName` is ^[a-zA-Z][a-zA-Z0-9_]{0,47}$.
            "name-past-its-pattern",
            {
                let mut request = minimal_request("x");
                request["name"] = json!("1_starts_with_a_digit");
                request
            },
        ),
        (
            "name-past-its-length",
            {
                let mut request = minimal_request("x");
                request["name"] = json!(format!("n{}", "a".repeat(48)));
                request
            },
        ),
        (
            // `ABTestDescription` is len 1..200.
            "empty-description",
            {
                let mut request = minimal_request("empty_description");
                request["description"] = json!("");
                request
            },
        ),
        (
            "description-past-its-length",
            {
                let mut request = minimal_request("long_description");
                request["description"] = json!("d".repeat(201));
                request
            },
        ),
        (
            // `GatewayArn` publishes an ARN pattern.
            "gateway-arn-past-its-pattern",
            {
                let mut request = minimal_request("bad_gateway");
                request["gatewayArn"] = json!("not-an-arn");
                request
            },
        ),
        (
            // `ClientToken` is len 33..256.
            "client-token-below-its-length",
            {
                let mut request = minimal_request("short_token");
                request["clientToken"] = json!("a".repeat(32));
                request
            },
        ),
    ];

    for (case, request) in cases {
        let (status, body) = create_ab_test(&router, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
        assert_eq!(
            ab_test_count(&engine),
            0,
            "case {case:?}: a refused create must store nothing"
        );
    }

    // Nothing refused above is listed.
    let (status, body) = list_ab_tests(&router, "maxResults=100").await;
    assert_eq!(
        (status, page(&body).len()),
        (200, 0),
        "expected no refused create to be listed, got {status} with {body:?}"
    );
}
