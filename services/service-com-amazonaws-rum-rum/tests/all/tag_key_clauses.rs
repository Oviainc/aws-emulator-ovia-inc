// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for how a refused tag key is STATED, driven through every
//! operation of this service that carries tag keys: `CreateAppMonitor$Tags` and
//! `TagResource$Tags`, which carry them in a MAP, and `UntagResource$TagKeys`, which
//! carries them in a LIST.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, deserialization, the modelled constraints, the handler
//! and the response envelope are all exercised rather than just a handler body.
//!
//! The assertions pin ONE rule over a SET of inputs: a refusal states one clause per
//! failing (key, requirement) pair, so a request naming several keys outside the modelled
//! `TagKey` is told about EVERY one of them in one answer, at that key's own member path.
//! The table's rows differ along what the rule generalises over — how many keys of the
//! request offend, which requirement each of them fails (`@length` at either bound, the
//! reserved namespace the `@pattern`'s lookahead excludes, a character outside its class),
//! where the offenders sort relative to the keys the shape admits, and whether any key is
//! admitted at all — and each row is sent through both map-carrying operations and, for the
//! same key set, through the list-carrying one, whose clause count must agree.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{call, emulator, message, percent_encoded, tag_map, Reply, ACCOUNT, REGION};

/// The status a successful `TagResource`/`UntagResource` answers with: 204 No Content
/// (`crate::state::TAGGING_NO_CONTENT_STATUS`).
const NO_CONTENT: u16 = 204;

/// The wire error code RUM's ARN-addressed tagging frontend answers a refused request with.
const BAD_REQUEST_CODE: &str = "BadRequestException";

/// The wire error code an app-monitor call refused on a modelled constraint answers with:
/// the one this service's model declares for `CreateAppMonitor`.
const VALIDATION_CODE: &str = "ValidationException";

/// `TagKey` is `@length(min: 1, max: 128)` with `@pattern("^(?!aws:)[a-zA-Z+-=._:/]+$")`.
/// Source: `smithy_query(query="shape", shape="TagKey")`.
const MAX_TAG_KEY_LENGTH: usize = 128;

/// The tag the monitor a `TagResource` row writes to is created with, so a refused write
/// can be shown to have changed nothing.
const SEED_KEY: &str = "Seed";
const SEED_VALUE: &str = "kept";

/// One row of the table: the keys of the request the modelled `TagKey` refuses, and the
/// keys it admits travelling in the same request.
struct KeySetCase {
    case: &'static str,
    offending: Vec<String>,
    admitted: Vec<String>,
}

impl KeySetCase {
    /// Every key the request carries, offending and admitted alike.
    fn keys(&self) -> Vec<&str> {
        self.offending
            .iter()
            .chain(self.admitted.iter())
            .map(String::as_str)
            .collect()
    }

    /// The request's tag map: each key with a value the `TagValue` shape admits, so a
    /// refusal is about the keys and nothing else.
    fn tags(&self) -> Value {
        self.keys()
            .into_iter()
            .map(|key| (key.to_string(), Value::String("value".to_string())))
            .collect::<serde_json::Map<String, Value>>()
            .into()
    }

    /// The clause opening the refusal must state for each offending key, in the order a
    /// map-carried refusal states them: the key's own value and the member path the walk
    /// spells a map entry as, for the keys in sorted order (which is the order the handler
    /// judges a map's keys in, so it is fixed by the request and not by a hash order).
    fn map_clauses(&self) -> Vec<String> {
        let mut sorted = self.offending.clone();
        sorted.sort();
        sorted
            .iter()
            .map(|key| format!("Value '{key}' at 'tags.{key}.member' failed to satisfy constraint:"))
            .collect()
    }

    /// Whether every offending key of this row fails the modelled `@pattern` rather than a
    /// `@length` bound.
    ///
    /// A `@length` on a LIST element is a constraint the boundary walk can apply itself, and
    /// this crate's documented limit (`crate::state::validation`) is that a request breaking
    /// a boundary-applied constraint is stated with the boundary's clauses alone — the
    /// handler that would state the rest never runs. So the removal path below is compared
    /// on the rows whose keys all reach that handler; the keys of a MAP reach it either way,
    /// since the walk validates no map key at all.
    fn offenders_reach_the_handler(&self) -> bool {
        self.offending
            .iter()
            .all(|key| (1..=MAX_TAG_KEY_LENGTH).contains(&key.chars().count()))
    }
}

/// The table. Rows differ along how many keys of one request offend, which requirement each
/// one fails, where the offenders sort against the admitted keys, and whether the request
/// holds an admitted key at all.
/// Source for every rejected key's reason: `smithy_query(query="shape", shape="TagKey")`.
fn key_set_cases() -> Vec<KeySetCase> {
    let past_max = "k".repeat(MAX_TAG_KEY_LENGTH + 1);
    vec![
        KeySetCase {
            case: "no key offends, so the write is applied",
            offending: Vec::new(),
            admitted: vec!["Env".to_string(), "Team".to_string()],
        },
        KeySetCase {
            case: "one key in the reserved namespace, beside an admitted one",
            offending: vec!["aws:owner".to_string()],
            admitted: vec!["Valid".to_string()],
        },
        KeySetCase {
            case: "two keys outside the pattern are both stated",
            offending: vec!["aws:owner".to_string(), "key with space".to_string()],
            admitted: vec!["Valid".to_string()],
        },
        KeySetCase {
            case: "three keys failing three different requirements are all stated",
            offending: vec![
                "aws:cloudformation:stack-name".to_string(),
                "key#hash".to_string(),
                past_max,
            ],
            admitted: vec!["Env".to_string()],
        },
        KeySetCase {
            case: "the empty key is shorter than TagKey's minimum, beside a key outside its class",
            offending: vec![String::new(), "key@at".to_string()],
            admitted: vec!["Team".to_string()],
        },
        KeySetCase {
            case: "non-ASCII keys in two scripts are both stated",
            offending: vec!["clé".to_string(), "キー".to_string()],
            admitted: vec!["Valid".to_string()],
        },
        KeySetCase {
            case: "the offenders sort after every admitted key",
            offending: vec!["zed|pipe".to_string(), "zulu~tilde".to_string()],
            admitted: vec!["Aaa".to_string(), "Bbb".to_string()],
        },
        KeySetCase {
            case: "every key of the request offends",
            offending: vec![
                "aws:".to_string(),
                "key!bang".to_string(),
                "key(paren)".to_string(),
                "key%percent".to_string(),
            ],
            admitted: Vec::new(),
        },
    ]
}

/// The ARN of the app monitor named `name`, as `crate::state::app_monitor_arn` derives it.
fn arn_of(name: &str) -> String {
    format!("arn:aws:rum:{REGION}:{ACCOUNT}:appmonitor/{name}")
}

/// `CreateAppMonitor` for the monitor named `name`, carrying `tags`.
async fn create_with_tags(router: &MultiServiceRouter, name: &str, tags: Value) -> Reply {
    call(
        router,
        "POST",
        "/appmonitor",
        Some(json!({"Name": name, "Domain": "example.com", "Tags": tags})),
    )
    .await
}

/// `TagResource`: merge `tags` onto the resource `resource_arn` names.
async fn tag_resource(router: &MultiServiceRouter, resource_arn: &str, tags: Value) -> Reply {
    call(
        router,
        "POST",
        &format!("/tags/{}", percent_encoded(resource_arn)),
        Some(json!({"Tags": tags})),
    )
    .await
}

/// `UntagResource`: name `keys` for removal from the resource `resource_arn` names.
async fn untag_resource(router: &MultiServiceRouter, resource_arn: &str, keys: &[&str]) -> Reply {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", percent_encoded(key)))
        .collect::<Vec<_>>()
        .join("&");
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{query}", percent_encoded(resource_arn)),
        None,
    )
    .await
}

/// The tag map `ListTagsForResource` reports for `resource_arn`, as a sorted Rust map.
async fn listed_tags(
    router: &MultiServiceRouter,
    case: &str,
    resource_arn: &str,
) -> BTreeMap<String, String> {
    let reply = call(
        router,
        "GET",
        &format!("/tags/{}", percent_encoded(resource_arn)),
        None,
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected ListTagsForResource to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    match tag_map(&reply.json(case)) {
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: tag {key:?} is not a string: {value}")
                        })
                        .to_string(),
                )
            })
            .collect(),
        other => panic!("case {case:?}: expected a tag map, got {other}"),
    }
}

/// `GetAppMonitor`'s raw answer for `name` — raw, because a case asserts the 404 a refused
/// create leaves behind as well as the 200 an accepted one does.
async fn get_monitor(router: &MultiServiceRouter, name: &str) -> Reply {
    call(router, "GET", &format!("/appmonitor/{name}"), None).await
}

/// Assert the refusal opens with the envelope AWS words a constraint refusal with
/// (`N validation error(s) detected: `), over `expected` clauses.
fn assert_envelope_over(case: &str, stated: &str, expected: usize) {
    let plural = if expected == 1 { "error" } else { "errors" };
    let opening = format!("{expected} validation {plural} detected: ");
    assert!(
        stated.starts_with(&opening),
        "case {case:?}: expected the refusal to open with {opening:?}, got {stated:?}"
    );
}

/// Assert `stated` carries each clause of `clauses`, once and in the order given.
fn assert_clauses_in_order(case: &str, stated: &str, clauses: &[String]) {
    let mut searched_from = 0;
    for clause in clauses {
        let found = stated[searched_from..].find(clause.as_str()).unwrap_or_else(|| {
            panic!("case {case:?}: expected the refusal to state {clause:?} next, got {stated:?}")
        });
        searched_from += found + clause.len();
    }
}

/// The refusal message of a 400 answer, asserting the status and the wire code first.
fn refusal_message(case: &str, reply: &Reply, expected_code: &str) -> String {
    assert_eq!(
        reply.status,
        400,
        "case {case:?}: expected HTTP 400, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.error_code(case),
        expected_code,
        "case {case:?}: expected {expected_code}, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
    let body = reply.json(case);
    message(&body)
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message: {body}"))
        .to_string()
}

#[tokio::test]
async fn a_tag_write_states_one_clause_for_every_key_the_modelled_shape_refuses() {
    let (router, _engine) = emulator();
    for (index, row) in key_set_cases().into_iter().enumerate() {
        // A fresh resource per row, holding one tag, so a refused write can be shown to
        // have left the map exactly as it was.
        let target = format!("tkc-target-{index:02}");
        let seed: Value = [(
            SEED_KEY.to_string(),
            Value::String(SEED_VALUE.to_string()),
        )]
        .into_iter()
        .collect::<serde_json::Map<String, Value>>()
        .into();
        let seeded = create_with_tags(&router, &target, seed).await;
        assert_eq!(
            seeded.status,
            200,
            "case {:?}: expected the target monitor to be created, got {} {:?}",
            row.case,
            seeded.status,
            String::from_utf8_lossy(&seeded.body)
        );
        let target_arn = arn_of(&target);
        let mut held: BTreeMap<String, String> = [(
            SEED_KEY.to_string(),
            SEED_VALUE.to_string(),
        )]
        .into_iter()
        .collect();

        let case = format!("TagResource: {}", row.case);
        let reply = tag_resource(&router, &target_arn, row.tags()).await;
        if row.offending.is_empty() {
            assert_eq!(
                reply.status,
                NO_CONTENT,
                "case {case:?}: expected HTTP {NO_CONTENT}, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            for key in &row.admitted {
                held.insert(key.clone(), "value".to_string());
            }
        } else {
            let stated = refusal_message(&case, &reply, BAD_REQUEST_CODE);
            assert_envelope_over(&case, &stated, row.offending.len());
            assert_clauses_in_order(&case, &stated, &row.map_clauses());
        }
        let listed = listed_tags(&router, &case, &target_arn).await;
        assert_eq!(
            listed, held,
            "case {case:?}: expected the resource to hold {held:?}, got {listed:?}"
        );

        // The SAME key set through the other operation that writes this map. The frontends
        // differ in the wire code; how completely they state the refusal must not.
        let create_case = format!("CreateAppMonitor: {}", row.case);
        let name = format!("tkc-create-{index:02}");
        let reply = create_with_tags(&router, &name, row.tags()).await;
        if row.offending.is_empty() {
            assert_eq!(
                reply.status,
                200,
                "case {create_case:?}: expected HTTP 200, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            let expected: BTreeMap<String, String> = row
                .admitted
                .iter()
                .map(|key| (key.clone(), "value".to_string()))
                .collect();
            let seeded = listed_tags(&router, &create_case, &arn_of(&name)).await;
            assert_eq!(
                seeded, expected,
                "case {create_case:?}: expected the created monitor to hold {expected:?}, got {seeded:?}"
            );
        } else {
            let stated = refusal_message(&create_case, &reply, VALIDATION_CODE);
            assert_envelope_over(&create_case, &stated, row.offending.len());
            assert_clauses_in_order(&create_case, &stated, &row.map_clauses());
            assert_eq!(
                get_monitor(&router, &name).await.status,
                404,
                "case {create_case:?}: a refused create must persist no monitor"
            );
        }

        // And the same keys NAMED for removal: the member that carries them is a list, so
        // the paths differ, but the number of failures stated is a property of the key set.
        if !row.offending.is_empty() && row.offenders_reach_the_handler() {
            let untag_case = format!("UntagResource: {}", row.case);
            let reply = untag_resource(&router, &target_arn, &row.keys()).await;
            let stated = refusal_message(&untag_case, &reply, BAD_REQUEST_CODE);
            assert_envelope_over(&untag_case, &stated, row.offending.len());
            for key in &row.offending {
                assert!(
                    stated.contains(&format!("Value '{key}' at 'tagKeys.")),
                    "case {untag_case:?}: expected the removal to state the key {key:?}, got {stated:?}"
                );
            }
        }
    }
}
