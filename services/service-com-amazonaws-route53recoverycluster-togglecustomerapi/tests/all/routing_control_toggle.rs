// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Route 53 ARC cluster (data-plane) routing-control
//! toggle loop: `ListRoutingControls`, `GetRoutingControlState`,
//! `UpdateRoutingControlState` and `UpdateRoutingControlStates`.
//!
//! Every call goes over the wire the real SDK uses — `POST /` with
//! `X-Amz-Target: ToggleCustomerAPI.<Op>` and an awsJson1_0 body — through the
//! same `MultiServiceRouter` the emulator boots, so what is asserted is the
//! service's HTTP behaviour and not an internal helper.
//!
//! The rules under test are asserted over a TABLE of inputs (every seeded routing
//! control, every modelled state value, every page size, every control panel and
//! every refusal class), not over one recorded request.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_route53recoverycluster_togglecustomerapi::ops::types::ROUTING_CONTROL_STATE_VALUES;
use service_com_amazonaws_route53recoverycluster_togglecustomerapi::state::{
    control_panel_arn, routing_control_arn, MAX_UPDATE_ROUTING_CONTROL_STATE_ENTRIES, SEED_FLEET,
};
use service_com_amazonaws_route53recoverycluster_togglecustomerapi::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name, which is how every signed SDK request routes. Request
/// authentication is off in these tests, so only the scope matters.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEMULATORTEST/20260101/us-east-1/route53-recovery-cluster/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// One emulator over its own engine: the state starts from the seed fleet, so one
/// test's toggles are invisible to the next.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return its HTTP status and parsed JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "route53-recovery-cluster.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("ToggleCustomerAPI.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "operation {operation}: response body is not JSON ({e}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code of a failure body: the `__type` discriminator, without any
/// `namespace#` prefix.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|t| t.rsplit('#').next().unwrap_or(t).to_string())
        .unwrap_or_default()
}

/// Every routing control ARN of the seed fleet, in the registry's ARN order —
/// which is the order `ListRoutingControls` must report them in.
fn all_seeded_arns() -> Vec<String> {
    let mut arns: Vec<String> = SEED_FLEET
        .iter()
        .flat_map(|panel| {
            panel
                .routing_control_ids
                .iter()
                .map(|id| routing_control_arn(panel.control_panel_id, id))
        })
        .collect();
    arns.sort();
    arns
}

/// A cursor in the encoding `ListRoutingControls` issues (an unfiltered listing at
/// `offset`), so a token pointing past the end of the set can be handed back.
fn cursor_for_offset(offset: usize) -> String {
    hex::encode(format!("v1||{offset}"))
}

/// `GetRoutingControlState` for `arn`, expecting success.
async fn get_state(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, body) = call(
        router,
        "GetRoutingControlState",
        &json!({ "RoutingControlArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case GetRoutingControlState({arn}): expected 200, got {status} with {body}"
    );
    body
}

/// Page through `ListRoutingControls` with `max_results`, following `NextToken`,
/// and return every `RoutingControl` reported, page by page.
async fn list_all_pages(
    router: &MultiServiceRouter,
    control_panel_arn: Option<&str>,
    max_results: Option<u32>,
) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut next_token: Option<String> = None;
    // Bounded so a token that never terminates fails the test instead of hanging.
    for _ in 0..(all_seeded_arns().len() + 2) {
        let mut request = serde_json::Map::new();
        if let Some(panel) = control_panel_arn {
            request.insert("ControlPanelArn".to_string(), json!(panel));
        }
        if let Some(size) = max_results {
            request.insert("MaxResults".to_string(), json!(size));
        }
        if let Some(token) = &next_token {
            request.insert("NextToken".to_string(), json!(token));
        }
        let case = format!("ListRoutingControls(panel={control_panel_arn:?}, max={max_results:?})");
        let (status, body) = call(router, "ListRoutingControls", &Value::Object(request)).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        let page = body
            .get("RoutingControls")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: RoutingControls is required, got {body}"))
            .clone();
        if let Some(size) = max_results {
            assert!(
                page.len() <= size as usize,
                "case {case}: a page must not exceed MaxResults, got {} entries",
                page.len()
            );
        }
        collected.extend(page);
        next_token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return collected;
        }
    }
    panic!("ListRoutingControls kept issuing a NextToken past the end of the set");
}

/// RULE: a routing control's reported state is the state last written to it, for
/// every control and every modelled state value — never an echo of the request
/// and never a canned value.
///
/// The table spans every seeded routing control (5, across 3 control panels and 2
/// clusters) crossed with every value of the modelled `RoutingControlState` enum,
/// including setting a control to the state it is already in.
#[tokio::test]
async fn update_then_get_reports_the_state_that_was_written() {
    let router = emulator();

    // What every control's state must be, as the table is walked. A seeded control
    // starts Off, the state a newly created routing control has.
    let mut expected: std::collections::BTreeMap<String, &str> = all_seeded_arns()
        .into_iter()
        .map(|arn| (arn, "Off"))
        .collect();

    for (arn, state) in &expected {
        let before = get_state(&router, arn).await;
        assert_eq!(
            before.get("RoutingControlState").and_then(Value::as_str),
            Some(*state),
            "case {arn}: a seeded control starts {state}, got {before}"
        );
    }

    for arn in all_seeded_arns() {
        // The value set comes from the model's own `RoutingControlState` enum, and
        // is walked twice so each control is also set to the state it is already in.
        for state in ROUTING_CONTROL_STATE_VALUES
            .iter()
            .chain(ROUTING_CONTROL_STATE_VALUES.iter())
        {
            let (status, body) = call(
                &router,
                "UpdateRoutingControlState",
                &json!({ "RoutingControlArn": arn, "RoutingControlState": state }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {arn} -> {state}: expected 200, got {status} with {body}"
            );
            assert_eq!(
                body,
                json!({}),
                "case {arn} -> {state}: UpdateRoutingControlState has an empty output shape, got {body}"
            );
            expected.insert(arn.clone(), *state);

            // EVERY control is re-read: the one written reports the new state, and a
            // toggle of one control moves no other.
            for (other, other_state) in &expected {
                let read_back = get_state(&router, other).await;
                assert_eq!(
                    read_back.get("RoutingControlState").and_then(Value::as_str),
                    Some(*other_state),
                    "case {arn} -> {state}: {other} must report {other_state}, got {read_back}"
                );
                assert_eq!(
                    read_back.get("RoutingControlArn").and_then(Value::as_str),
                    Some(other.as_str()),
                    "case {other}: the response echoes the looked-up ARN, got {read_back}"
                );
                assert!(
                    read_back
                        .get("RoutingControlName")
                        .and_then(Value::as_str)
                        .is_some_and(|name| other.ends_with(name)),
                    "case {other}: the control's name must come from the registry, got {read_back}"
                );
            }
        }
    }
}

/// RULE: a state value outside the modelled `RoutingControlState` value set is
/// refused with HTTP 400 `ValidationException`, and writes nothing.
///
/// The rows differ in form (wrong case, a prefix/suffix of a legal value, empty,
/// an unrelated word), and each is sent to BOTH update operations.
#[tokio::test]
async fn a_state_outside_the_modelled_value_set_is_refused() {
    let router = emulator();
    let arn = routing_control_arn(
        SEED_FLEET[0].control_panel_id,
        SEED_FLEET[0].routing_control_ids[0],
    );

    for bogus in ["ON", "on", "OFF", "off", "Offline", "On ", "", "Unknown"] {
        for (operation, body) in [
            (
                "UpdateRoutingControlState",
                json!({ "RoutingControlArn": arn, "RoutingControlState": bogus }),
            ),
            (
                "UpdateRoutingControlStates",
                json!({ "UpdateRoutingControlStateEntries": [
                    { "RoutingControlArn": arn, "RoutingControlState": bogus }
                ]}),
            ),
        ] {
            let (status, response) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {operation}({bogus:?}): expected 400, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {operation}({bogus:?}): expected ValidationException, got {response}"
            );
        }
    }

    // Nothing was written by any refused request.
    let state = get_state(&router, &arn).await;
    assert_eq!(
        state.get("RoutingControlState").and_then(Value::as_str),
        Some("Off"),
        "a refused update must write nothing, got {state}"
    );
}

/// RULE: an ARN the registry does not hold is `ResourceNotFoundException` (404),
/// and a read never registers it.
///
/// The rows are well-formed-but-unknown ARNs of several shapes: an unknown control
/// panel, an unknown control in a known panel, a bare resource id, and a
/// one-character ARN (the shortest the model's `@length(1..255)` allows).
#[tokio::test]
async fn an_unknown_routing_control_is_not_found_and_is_not_created() {
    let router = emulator();
    let unknown = [
        routing_control_arn("EmulatorControlPanelZ", "EmulatorRoutingControl1"),
        routing_control_arn(SEED_FLEET[0].control_panel_id, "NoSuchRoutingControl"),
        control_panel_arn(SEED_FLEET[0].control_panel_id),
        "x".to_string(),
    ];

    for arn in &unknown {
        let (status, body) = call(
            &router,
            "GetRoutingControlState",
            &json!({ "RoutingControlArn": arn }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {arn}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {arn}: expected ResourceNotFoundException, got {body}"
        );

        let (update_status, update_body) = call(
            &router,
            "UpdateRoutingControlState",
            &json!({ "RoutingControlArn": arn, "RoutingControlState": "On" }),
        )
        .await;
        assert_eq!(
            update_status, 404,
            "case update {arn}: expected 404, got {update_status} with {update_body}"
        );
        assert_eq!(
            error_code(&update_body),
            "ResourceNotFoundException",
            "case update {arn}: expected ResourceNotFoundException, got {update_body}"
        );
    }

    // No read or refused write registered anything: the fleet is unchanged.
    let listed: BTreeSet<String> = list_all_pages(&router, None, None)
        .await
        .iter()
        .filter_map(|control| {
            control
                .get("RoutingControlArn")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert_eq!(
        listed,
        all_seeded_arns().into_iter().collect::<BTreeSet<_>>(),
        "a reference to an unknown ARN must not add anything to the registry"
    );
}

/// RULE: `ListRoutingControls` reports exactly the routing controls held in state,
/// in a stable order, and its pages walk the whole set without repeats or
/// omissions for ANY page size.
///
/// The rows are every page size from 1 to one past the size of the set, plus
/// "unspecified" (the modelled default of 500).
#[tokio::test]
async fn list_pages_walk_the_whole_set_for_every_page_size() {
    let router = emulator();
    let expected = all_seeded_arns();

    let mut page_sizes: Vec<Option<u32>> = (1..=(expected.len() as u32 + 1)).map(Some).collect();
    page_sizes.push(None);

    for max_results in page_sizes {
        let reported: Vec<String> = list_all_pages(&router, None, max_results)
            .await
            .iter()
            .map(|control| {
                control
                    .get("RoutingControlArn")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case MaxResults={max_results:?}: the pages must walk the whole set in order, got {reported:?}"
        );
    }
}

/// RULE: each reported routing control carries the control panel that hosts it,
/// and a `ControlPanelArn` filter reports exactly that panel's controls; a control
/// panel the registry does not hold is `ResourceNotFoundException` (404).
///
/// The rows are every control panel of the seed fleet (3, one of them in a
/// different cluster) plus an unknown panel.
#[tokio::test]
async fn listing_filters_by_control_panel() {
    let router = emulator();

    for panel in SEED_FLEET {
        let panel_arn = control_panel_arn(panel.control_panel_id);
        let expected: Vec<String> = {
            let mut arns: Vec<String> = panel
                .routing_control_ids
                .iter()
                .map(|id| routing_control_arn(panel.control_panel_id, id))
                .collect();
            arns.sort();
            arns
        };

        // Page size 1 as well as unpaged, so the filter is proven to survive the cursor.
        for max_results in [None, Some(1)] {
            let page = list_all_pages(&router, Some(&panel_arn), max_results).await;
            let reported: Vec<String> = page
                .iter()
                .map(|control| {
                    control
                        .get("RoutingControlArn")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            assert_eq!(
                reported, expected,
                "case panel {} (MaxResults={max_results:?}): expected only that panel's controls, got {reported:?}",
                panel.control_panel_id
            );
            for control in &page {
                assert_eq!(
                    control.get("ControlPanelArn").and_then(Value::as_str),
                    Some(panel_arn.as_str()),
                    "case panel {}: every reported control names its panel, got {control}",
                    panel.control_panel_id
                );
                assert_eq!(
                    control.get("ControlPanelName").and_then(Value::as_str),
                    Some(panel.control_panel_id),
                    "case panel {}: every reported control names its panel, got {control}",
                    panel.control_panel_id
                );
                assert_eq!(
                    control.get("Owner").and_then(Value::as_str),
                    Some(engine::emulator_account_id()),
                    "case panel {}: the owner is the emulator account, got {control}",
                    panel.control_panel_id
                );
            }
        }
    }

    let (status, body) = call(
        &router,
        "ListRoutingControls",
        &json!({ "ControlPanelArn": control_panel_arn("EmulatorControlPanelZ") }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case unknown control panel: expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case unknown control panel: expected ResourceNotFoundException, got {body}"
    );
}

/// RULE: `MaxResults` is modelled with `@range(min: 1)`, so a value below that
/// bound is refused with HTTP 400 `ValidationException` rather than silently read
/// as "unspecified"; a value at or above the bound is served.
///
/// The rows are the published bound (1), one past it (0), one further past it
/// (-1, which an SDK can send for an `integer` member), and a value far above the
/// size of the set.
#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let router = emulator();
    let total = all_seeded_arns().len();

    for refused in [0, -1, i32::MIN] {
        let (status, body) = call(
            &router,
            "ListRoutingControls",
            &json!({ "MaxResults": refused }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case MaxResults={refused}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case MaxResults={refused}: expected ValidationException, got {body}"
        );
    }

    for accepted in [1, total as i32, 1_000, i32::MAX] {
        let (status, body) = call(
            &router,
            "ListRoutingControls",
            &json!({ "MaxResults": accepted }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case MaxResults={accepted}: expected 200, got {status} with {body}"
        );
        let page = body
            .get("RoutingControls")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case MaxResults={accepted}: RoutingControls is required, got {body}"));
        let expected = (accepted as usize).min(total);
        assert_eq!(
            page.len(),
            expected,
            "case MaxResults={accepted}: expected {expected} control(s) on the page, got {}",
            page.len()
        );
    }
}

/// RULE: a `NextToken` is only valid for the call that issued it — it carries the
/// filter it was issued for, and a token this service did not issue is refused
/// with HTTP 400 `ValidationException`.
#[tokio::test]
async fn a_next_token_is_only_valid_for_the_call_that_issued_it() {
    let router = emulator();

    let (status, first_page) = call(
        &router,
        "ListRoutingControls",
        &json!({ "MaxResults": 1 }),
    )
    .await;
    assert_eq!(status, 200, "the first page must succeed, got {first_page}");
    let token = first_page
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("a set larger than one page issues a NextToken")
        .to_string();

    // Rows: the unfiltered token replayed under a filter, under another filter,
    // and tokens this service never issued at all.
    let cases: Vec<(String, Value)> = vec![
        (
            "a token issued unfiltered, replayed with a filter".to_string(),
            json!({ "MaxResults": 1, "NextToken": token,
                    "ControlPanelArn": control_panel_arn(SEED_FLEET[0].control_panel_id) }),
        ),
        (
            "a token issued unfiltered, replayed with another filter".to_string(),
            json!({ "MaxResults": 1, "NextToken": token,
                    "ControlPanelArn": control_panel_arn(SEED_FLEET[2].control_panel_id) }),
        ),
        (
            "not a token at all".to_string(),
            json!({ "NextToken": "not-a-token" }),
        ),
        (
            "a token whose offset is past the end of the set".to_string(),
            json!({ "NextToken": cursor_for_offset(99_999) }),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = call(&router, "ListRoutingControls", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );
    }
}

/// RULE: `UpdateRoutingControlStates` applies every entry or none — it validates
/// the whole batch first and commits under one lock, so a refused batch leaves the
/// registry exactly as it was.
///
/// The accepted rows differ in size and in the panels they span (one entry, two
/// entries in one panel, two entries across two panels of ONE cluster, the whole
/// cluster at once, and mixed target states). The refused rows are one per refusal
/// class, each checked to have written nothing.
#[tokio::test]
async fn a_batch_update_applies_every_entry_or_none() {
    let cluster_one: Vec<String> = SEED_FLEET
        .iter()
        .filter(|panel| panel.cluster_id == SEED_FLEET[0].cluster_id)
        .flat_map(|panel| {
            panel
                .routing_control_ids
                .iter()
                .map(|id| routing_control_arn(panel.control_panel_id, id))
        })
        .collect();
    let other_cluster = routing_control_arn(
        SEED_FLEET[2].control_panel_id,
        SEED_FLEET[2].routing_control_ids[0],
    );

    // Accepted batches: (case, entries as (arn, state) pairs).
    let accepted: Vec<(&str, Vec<(String, &str)>)> = vec![
        (
            "one entry",
            vec![(cluster_one[0].clone(), "On")],
        ),
        (
            "two entries in one control panel",
            vec![
                (cluster_one[0].clone(), "Off"),
                (cluster_one[1].clone(), "On"),
            ],
        ),
        (
            "two entries across two control panels of one cluster",
            vec![
                (cluster_one[0].clone(), "On"),
                (cluster_one[3].clone(), "On"),
            ],
        ),
        (
            "every control of one cluster, mixed states",
            cluster_one
                .iter()
                .enumerate()
                .map(|(index, arn)| {
                    (
                        arn.clone(),
                        if index % 2 == 0 { "Off" } else { "On" },
                    )
                })
                .collect(),
        ),
        ("an empty batch", vec![]),
    ];

    for (case, entries) in accepted {
        let router = emulator();
        let request = json!({
            "UpdateRoutingControlStateEntries": entries
                .iter()
                .map(|(arn, state)| json!({ "RoutingControlArn": arn, "RoutingControlState": state }))
                .collect::<Vec<Value>>(),
        });
        let (status, body) = call(&router, "UpdateRoutingControlStates", &request).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {case}: UpdateRoutingControlStates has an empty output shape, got {body}"
        );

        for (arn, state) in &entries {
            let read_back = get_state(&router, arn).await;
            assert_eq!(
                read_back.get("RoutingControlState").and_then(Value::as_str),
                Some(*state),
                "case {case}: every entry of the batch must be applied, got {read_back} for {arn}"
            );
        }
        // Controls the batch never named keep their state.
        for arn in all_seeded_arns()
            .into_iter()
            .filter(|arn| !entries.iter().any(|(named, _)| named == arn))
        {
            let read_back = get_state(&router, &arn).await;
            assert_eq!(
                read_back.get("RoutingControlState").and_then(Value::as_str),
                Some("Off"),
                "case {case}: a control the batch did not name must keep its state, got {read_back} for {arn}"
            );
        }
    }

    // Refused batches: (case, entries, expected status, expected error code).
    let over_the_limit: Vec<Value> = (0..=MAX_UPDATE_ROUTING_CONTROL_STATE_ENTRIES)
        .map(|index| {
            json!({
                "RoutingControlArn": routing_control_arn(
                    SEED_FLEET[0].control_panel_id,
                    &format!("EmulatorRoutingControl{index}"),
                ),
                "RoutingControlState": "On",
            })
        })
        .collect();
    let refused: Vec<(&str, Vec<Value>, u16, &str)> = vec![
        (
            "one entry past the batch limit",
            over_the_limit,
            402,
            "ServiceLimitExceededException",
        ),
        (
            "the same routing control twice",
            vec![
                json!({ "RoutingControlArn": cluster_one[0], "RoutingControlState": "On" }),
                json!({ "RoutingControlArn": cluster_one[0], "RoutingControlState": "Off" }),
            ],
            400,
            "ValidationException",
        ),
        (
            "entries spanning two clusters",
            vec![
                json!({ "RoutingControlArn": cluster_one[0], "RoutingControlState": "On" }),
                json!({ "RoutingControlArn": other_cluster, "RoutingControlState": "On" }),
            ],
            400,
            "ValidationException",
        ),
        (
            "an unknown routing control after a known one",
            vec![
                json!({ "RoutingControlArn": cluster_one[0], "RoutingControlState": "On" }),
                json!({
                    "RoutingControlArn": routing_control_arn(
                        SEED_FLEET[0].control_panel_id, "NoSuchRoutingControl"),
                    "RoutingControlState": "On",
                }),
            ],
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, entries, expected_status, expected_code) in refused {
        let router = emulator();
        let (status, body) = call(
            &router,
            "UpdateRoutingControlStates",
            &json!({ "UpdateRoutingControlStateEntries": entries }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected {expected_status}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case}: expected {expected_code}, got {body}"
        );

        // ATOMICITY: not one entry of a refused batch was committed.
        for arn in all_seeded_arns() {
            let read_back = get_state(&router, &arn).await;
            assert_eq!(
                read_back.get("RoutingControlState").and_then(Value::as_str),
                Some("Off"),
                "case {case}: a refused batch must leave no partial mutation, got {read_back} for {arn}"
            );
        }
    }
}

/// The states this service stores are exactly the values the model's
/// `RoutingControlState` enum declares (`ops::types::ROUTING_CONTROL_STATE_VALUES`),
/// and each round-trips through the stored form.
#[test]
fn every_modelled_state_value_round_trips_through_the_stored_form() {
    use service_com_amazonaws_route53recoverycluster_togglecustomerapi::state::RoutingControlState;

    for value in ROUTING_CONTROL_STATE_VALUES {
        let parsed = RoutingControlState::from_wire(value)
            .unwrap_or_else(|| panic!("case {value}: the modelled value must parse"));
        assert_eq!(
            parsed.as_wire(),
            *value,
            "case {value}: the stored state must round-trip to its wire value, got {}",
            parsed.as_wire()
        );
    }
    assert_eq!(
        ROUTING_CONTROL_STATE_VALUES.len(),
        2,
        "the model declares exactly On and Off, got {ROUTING_CONTROL_STATE_VALUES:?}"
    );
}
