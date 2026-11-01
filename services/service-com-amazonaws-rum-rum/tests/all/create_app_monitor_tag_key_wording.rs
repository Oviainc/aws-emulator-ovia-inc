// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the WORDS an app-monitor call is refused on a tag key in.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, deserialization, the modelled constraints, the handler
//! and the response envelope are all exercised rather than just a handler body.
//!
//! The rule pinned, over a TABLE spanning the class rather than the one key a report cited:
//! a constraint refusal an app-monitor operation answers states AWS's
//! `N validation error(s) detected: ` envelope over one clause per failing
//! (member, requirement), each quoting a modelled `@pattern` WITHOUT the `^` / `$` pair the
//! model writes around it to demand a whole-string match, at the dotted member path the
//! framework's validating walk spells that position as — for a TERMINAL map value, the map
//! member followed by the entry key alone. That holds for every constrained member of such a
//! call, so the same expression a refusal on `Name` quotes and the one a refusal on a tag key
//! quotes are both the unanchored form, and a key refused inside the tag map is reported at
//! `tags.<key>`.
//!
//! The rows differ along what the rule generalises over: which requirement each offending key
//! fails (`@length` at either bound, the reserved namespace the `@pattern`'s look-ahead
//! excludes, a character outside its class, a non-ASCII script), how many keys of one request
//! offend, whether an admitted key travels beside them, and the SHAPE of the key — one
//! carrying the `.` the path is dotted with, and one spelled exactly like the step the walk
//! writes for a list element, so a path assembled from the key cannot be mistaken for a
//! nested position.
//!
//! Every pattern and bound below is cited from the model
//! (`smithy_query(query="shape", shape="TagKey")` and `shape="AppMonitorName"` against
//! `services/service-com-amazonaws-rum-rum/smithy-model/com-amazonaws-rum-rum.json`) and is
//! spelled out here rather than read back out of the crate, so the test pins the rendering
//! instead of agreeing with it by construction.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{call, emulator, message, percent_encoded, tag_map, Reply, ACCOUNT, REGION};

/// The wire error code an app-monitor call refused on a modelled constraint answers with:
/// the one this service's model declares for `CreateAppMonitor` (`@httpError(400)`).
const VALIDATION_CODE: &str = "ValidationException";

/// The wire error code RUM's ARN-addressed tagging frontend answers a refused request with.
const BAD_REQUEST_CODE: &str = "BadRequestException";

/// `TagKey`'s `@pattern` as the model declares it: anchored, to demand a whole-string match.
/// Source: `smithy_query(query="shape", shape="TagKey")`.
const MODELLED_TAG_KEY_PATTERN: &str = r"^(?!aws:)[a-zA-Z+-=._:/]+$";

/// `TagKey`'s `@pattern` as a refusal of this service quotes it: the modelled expression
/// without the `^` / `$` pair the model writes around it.
const TAG_KEY_PATTERN: &str = r"(?!aws:)[a-zA-Z+-=._:/]+";

/// `TagKey`'s modelled `@length` bounds.
/// Source: `smithy_query(query="shape", shape="TagKey")`.
const MIN_TAG_KEY_LENGTH: usize = 1;
const MAX_TAG_KEY_LENGTH: usize = 128;

/// `AppMonitorName`'s `@pattern` as a refusal quotes it, for the cross-member half of the
/// rule. Source: `smithy_query(query="shape", shape="AppMonitorName")`.
const NAME_PATTERN: &str = r"(?!\.)[\.\-_#A-Za-z0-9]+";

/// The requirement a `@pattern` violation states.
fn pattern_requirement(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// The requirement a `@length` minimum violation states.
fn min_length_requirement(min: usize) -> String {
    format!("Member must have length greater than or equal to {min}")
}

/// The requirement a `@length` maximum violation states.
fn max_length_requirement(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// One offending key of a row: the key, and the requirement the modelled `TagKey` refuses it
/// on.
struct Offender {
    key: String,
    requirement: String,
}

/// A key the modelled `@pattern` rejects.
fn outside_pattern(key: &str) -> Offender {
    Offender {
        key: key.to_string(),
        requirement: pattern_requirement(TAG_KEY_PATTERN),
    }
}

/// One row of the table: the keys the modelled `TagKey` refuses, with the requirement each
/// fails, and the keys it admits travelling in the same request.
struct KeyCase {
    case: &'static str,
    offending: Vec<Offender>,
    admitted: Vec<String>,
}

impl KeyCase {
    /// The request's tag map: every key of the row, each with a value `TagValue` admits, so
    /// a refusal is about the keys and nothing else.
    fn tags(&self) -> Value {
        self.offending
            .iter()
            .map(|offender| offender.key.clone())
            .chain(self.admitted.iter().cloned())
            .map(|key| (key, Value::String("value".to_string())))
            .collect::<serde_json::Map<String, Value>>()
            .into()
    }

    /// The WHOLE message this row must be refused with: the envelope over one clause per
    /// offending key, the keys in sorted order (the order the handler judges a map's keys
    /// in, so it is fixed by the request rather than by a hash order), each clause at the
    /// path the walk spells a terminal map value as.
    fn expected_message(&self) -> String {
        let mut offenders: Vec<&Offender> = self.offending.iter().collect();
        offenders.sort_by(|left, right| left.key.cmp(&right.key));
        let clauses: Vec<String> = offenders
            .iter()
            .map(|offender| {
                format!(
                    "Value '{key}' at 'tags.{key}' failed to satisfy constraint: {requirement}",
                    key = offender.key,
                    requirement = offender.requirement
                )
            })
            .collect();
        let plural = if clauses.len() == 1 { "error" } else { "errors" };
        format!(
            "{} validation {plural} detected: {}",
            clauses.len(),
            clauses.join("; ")
        )
    }
}

/// The table. Source for every rejected key's reason:
/// `smithy_query(query="shape", shape="TagKey")`.
fn key_cases() -> Vec<KeyCase> {
    vec![
        KeyCase {
            case: "no key offends, so the create is applied",
            offending: Vec::new(),
            admitted: vec!["Env".to_string(), "Team".to_string()],
        },
        KeyCase {
            case: "a key in the reserved namespace the look-ahead excludes",
            offending: vec![outside_pattern("aws:owner")],
            admitted: vec!["Valid".to_string()],
        },
        KeyCase {
            case: "a key carrying a character outside the pattern's class",
            offending: vec![outside_pattern("key with space")],
            admitted: Vec::new(),
        },
        KeyCase {
            case: "a reserved key whose own text is DOTTED, like the path separator",
            offending: vec![outside_pattern("aws:cloudformation.stack.name")],
            admitted: vec!["Stage".to_string()],
        },
        KeyCase {
            case: "a reserved key spelled like the walk's own list-element step",
            offending: vec![outside_pattern("aws:1.member")],
            admitted: vec!["Tier".to_string()],
        },
        KeyCase {
            case: "a non-ASCII key, one script per key",
            offending: vec![outside_pattern("clé"), outside_pattern("キー")],
            admitted: vec!["Valid".to_string()],
        },
        KeyCase {
            case: "the empty key, one character short of the minimum length",
            offending: vec![Offender {
                key: String::new(),
                requirement: min_length_requirement(MIN_TAG_KEY_LENGTH),
            }],
            admitted: vec!["Env".to_string()],
        },
        KeyCase {
            case: "a key one character past the maximum length",
            offending: vec![Offender {
                key: "k".repeat(MAX_TAG_KEY_LENGTH + 1),
                requirement: max_length_requirement(MAX_TAG_KEY_LENGTH),
            }],
            admitted: Vec::new(),
        },
        KeyCase {
            case: "an all-digit key past the maximum length: a path that ends in digits",
            offending: vec![Offender {
                key: "7".repeat(MAX_TAG_KEY_LENGTH + 9),
                requirement: max_length_requirement(MAX_TAG_KEY_LENGTH),
            }],
            admitted: vec!["Env".to_string()],
        },
        KeyCase {
            case: "three keys failing three different requirements are all stated",
            offending: vec![
                outside_pattern("aws:owner"),
                outside_pattern("key@at"),
                Offender {
                    key: "z".repeat(MAX_TAG_KEY_LENGTH + 1),
                    requirement: max_length_requirement(MAX_TAG_KEY_LENGTH),
                },
            ],
            admitted: vec!["Env".to_string()],
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

/// The message of a 400 answer, asserting the status and the wire code first.
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

/// The tags `ListTagsForResource` reports for `resource_arn`, as a sorted Rust map.
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
                let text = value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: tag {key:?} is not a string"));
                (key.clone(), text.to_string())
            })
            .collect(),
        other => panic!("case {case:?}: expected a tag map, got {other}"),
    }
}

#[tokio::test]
async fn a_created_monitors_tag_key_refusal_is_worded_the_way_this_service_words_one() {
    let (router, _engine) = emulator();
    for (index, row) in key_cases().into_iter().enumerate() {
        let case = row.case;
        let name = format!("catk-{index:02}");
        let reply = create_with_tags(&router, &name, row.tags()).await;

        if row.offending.is_empty() {
            assert_eq!(
                reply.status,
                200,
                "case {case:?}: expected HTTP 200, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            // The round trip: the admitted keys are what the created monitor holds.
            let expected: BTreeMap<String, String> = row
                .admitted
                .iter()
                .map(|key| (key.clone(), "value".to_string()))
                .collect();
            let held = listed_tags(&router, case, &arn_of(&name)).await;
            assert_eq!(
                held, expected,
                "case {case:?}: expected the created monitor to hold {expected:?}, got {held:?}"
            );
            continue;
        }

        let stated = refusal_message(case, &reply, VALIDATION_CODE);
        let expected = row.expected_message();
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the message {expected:?}, got {stated:?}"
        );
        assert!(
            !stated.contains(MODELLED_TAG_KEY_PATTERN),
            "case {case:?}: the refusal must not quote the pattern with its anchor pair, \
             got {stated:?}"
        );
        // A refused create writes nothing: the monitor the request named does not exist.
        let read = call(&router, "GET", &format!("/appmonitor/{name}"), None).await;
        assert_eq!(
            read.status,
            404,
            "case {case:?}: a refused create must persist no monitor, got {} {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
    }
}

#[tokio::test]
async fn one_app_monitor_call_quotes_every_modelled_pattern_the_same_unanchored_way() {
    let (router, _engine) = emulator();

    // The same operation, refused on two different constrained members: a scalar member the
    // walk reports under its own name, and a key inside the tag map. Both clauses quote the
    // modelled expression without the anchor pair.
    let refused_name = create_with_tags(&router, ".bad", json!({"Env": "prod"})).await;
    let stated = refusal_message("refused on Name", &refused_name, VALIDATION_CODE);
    let expected = format!(
        "1 validation error detected: Value '.bad' at 'name' failed to satisfy constraint: {}",
        pattern_requirement(NAME_PATTERN)
    );
    assert_eq!(
        stated, expected,
        "case {:?}: expected the message {expected:?}, got {stated:?}",
        "refused on Name"
    );

    let refused_key = create_with_tags(&router, "catk-pair", json!({"aws:owner": "x"})).await;
    let stated = refusal_message("refused on a tag key", &refused_key, VALIDATION_CODE);
    let expected = format!(
        "1 validation error detected: Value 'aws:owner' at 'tags.aws:owner' failed to satisfy \
         constraint: {}",
        pattern_requirement(TAG_KEY_PATTERN)
    );
    assert_eq!(
        stated, expected,
        "case {:?}: expected the message {expected:?}, got {stated:?}",
        "refused on a tag key"
    );
}

#[tokio::test]
async fn a_tag_key_is_reported_at_the_same_map_path_by_the_tagging_frontend() {
    let (router, _engine) = emulator();
    let name = "catk-frontends";
    let created = create_with_tags(&router, name, json!({"Env": "prod"})).await;
    assert_eq!(
        created.status,
        200,
        "case {:?}: expected the target monitor to be created, got {} {:?}",
        "frontend comparison",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let resource_arn = arn_of(name);

    // The path a clause reports a tag key at is the walk's spelling of that position and so
    // the same for both frontends; what differs is the dialect each words a refusal in (the
    // ARN-addressed tagging frontend answers its own error code).
    for key in ["aws:owner", "key with space", "aws:1.member"] {
        let case = format!("TagResource refused on {key:?}");
        let reply = call(
            &router,
            "POST",
            &format!("/tags/{}", percent_encoded(&resource_arn)),
            Some(json!({"Tags": {key: "value"}})),
        )
        .await;
        let stated = refusal_message(&case, &reply, BAD_REQUEST_CODE);
        let opening = format!("Value '{key}' at 'tags.{key}' failed to satisfy constraint:");
        assert!(
            stated.contains(&opening),
            "case {case:?}: expected the refusal to state {opening:?}, got {stated:?}"
        );
        let held = listed_tags(&router, &case, &resource_arn).await;
        let expected: BTreeMap<String, String> =
            [("Env".to_string(), "prod".to_string())].into_iter().collect();
        assert_eq!(
            held, expected,
            "case {case:?}: a refused tag write must change nothing, got {held:?}"
        );
    }
}
