// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for CloudWatch RUM's `UntagResource`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, the percent-encoded `ResourceArn` path label, the
//! `tagKeys` query binding, the modelled constraints, the handler and the response
//! envelope are all exercised, not just the handler body. Each removal is read back
//! through the implemented `GetAppMonitor`, whose `AppMonitor$Tags` is the same stored
//! map, so a case asserts PERSISTED state rather than the untag call's own answer.
//!
//! The assertions pin RULES over a SET of inputs rather than one request: the removal
//! table varies how many keys are named, which of the resource's keys they are, whether a
//! named key is carried at all, whether one is named twice and whether its spelling
//! differs only in case, and it spans the `TagKey` catalog (both length bounds and every
//! class of character the member's pattern admits); the targeting test asserts that only
//! the ARN's own resource loses tags; the idempotence test asserts the invariant the
//! model's `@idempotent` states; and the refusal table walks one ARN component per row.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::message;

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`rum`, per `OPERATIONS.md`'s `## Wire protocol`), which is what the router's
/// `can_handle` routes on, and the region a created monitor's ARN is derived from.
/// Request authentication is switched off below, so only the scope's shape matters.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/rum/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The wire error code RUM's tagging frontend answers for a `ResourceArn` that names no
/// existing resource, at the HTTP 404 the model's `ResourceNotFoundException` carries.
/// Source: the recorded corpus unit `rum.untag-resource-unknown-resource-rejected`, whose
/// check asserts `err["Code"] == "NotFoundException"`; the shape is not in the model, so
/// the code is not the shape name.
const NOT_FOUND_ERROR_CODE: &str = "NotFoundException";

/// The message that refusal carries: a constant the service words the same way whatever
/// ARN the request named, and the WHOLE body — RUM's `ResourceNotFoundException` puts
/// neither `resourceName` nor `resourceType` on the wire.
/// Source: the same recorded unit, whose check asserts the message for an unknown ARN.
const NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The wire error code RUM's tagging frontend answers a malformed tag request with, where
/// this service's app-monitor operations answer the modelled `ValidationException`.
/// Source: the same recorded unit, whose check sends a `ResourceArn` that is not an ARN
/// and asserts `err["Code"] == "BadRequestException"`.
const BAD_REQUEST_ERROR_CODE: &str = "BadRequestException";

/// The message that refusal carries for a `ResourceArn` which is not an ARN: another
/// constant — it names neither the value, nor the member, nor the pattern, and is the same
/// sentence for every value that fails the syntax.
/// Source: the same recorded unit, whose check asserts the message for `not-an-arn-at-all`.
const INVALID_RESOURCE_ARN_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// The status a successful `UntagResource` answers with: 204 No Content, the status the
/// service sends for a tag write whose modelled output declares no member
/// (`crate::state::TAGGING_NO_CONTENT_STATUS`; the dedicated suite `tagging_no_content.rs`
/// pins it over the whole class of tag writes).
const NO_CONTENT: u16 = 204;

/// `TagKey`'s modelled length bounds.
/// Source: `smithy_query(query="shape", shape="TagMap")` — the key is `@length(min: 1,
/// max: 128)`.
const TAG_KEY_BOUNDS: (usize, usize) = (1, 128);

/// One key per class of character `TagKey`'s pattern admits —
/// `^(?!aws:)[a-zA-Z+-=._:/]+$`, whose class is the letters plus the `+`..`=` range
/// (`+ , - . / 0-9 : ; < =`) plus `.`, `_`, `:` and `/`.
/// Source: `smithy_query(query="shape", shape="TagMap")`, the key's pattern.
const TAG_KEY_CATALOG: [&str; 9] = [
    "Team",
    "cost/center",
    "team:web",
    "Env.name",
    "dash-key",
    "plus+key",
    "digits123",
    "semi;colon",
    "under_score",
];

/// One emulator: a shared engine, the RUM service, and the router that dispatches to it.
/// Authentication is off so a test needs no signing key.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_rum_rum::EmulatedService::new(engine));
    router
}

/// One HTTP answer, kept as raw bytes so a case can assert on an EMPTY body as well as
/// on JSON.
struct Reply {
    status: u16,
    body: Bytes,
}

impl Reply {
    fn json(&self, case: &str) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "case {case:?}: expected a JSON body, got {:?} ({e})",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code: restJson1 carries it as the body's `__type`.
    fn error_code(&self, case: &str) -> String {
        self.json(case)["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: error body has no __type: {:?}", self.body))
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

/// Percent-encode a path label or a query value: the shared harness's encoder
/// (`crate::support::percent_encoded`), which every suite in this crate uses. This suite
/// brings its own router, `call` and `Reply` because it asserts on raw bytes, but the
/// encoding of a `@httpLabel` and of a `@httpQuery` value is the same question for all of
/// them, so it is asked in one place.
use crate::support::percent_encoded as percent_encode;

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let body_bytes = match body {
        Some(value) => Bytes::from(serde_json::to_vec(&value).expect("serializable request body")),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://rum.{REGION}.amazonaws.com{path_and_query}"))
        .header("authorization", authorization(REGION))
        .header("content-type", "application/json")
        .body(body_bytes)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    Reply {
        status,
        body: response.into_body(),
    }
}

/// The ARN a created app monitor carries: `arn:aws:rum:<region>:<account>:appmonitor/<name>`.
fn app_monitor_arn(region: &str, account: &str, name: &str) -> String {
    format!("arn:aws:rum:{region}:{account}:appmonitor/{name}")
}

/// Create one app monitor with `tags`, asserting the call succeeded.
async fn create_app_monitor(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    tags: &BTreeMap<String, String>,
) {
    let tag_object: serde_json::Map<String, Value> = tags
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    let reply = call(
        router,
        "POST",
        "/appmonitor",
        Some(json!({
            "Name": name,
            "Domain": "example.com",
            "Tags": Value::Object(tag_object),
        })),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected CreateAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// `UntagResource`: `DELETE /tags/{ResourceArn}?tagKeys=…`.
async fn untag(router: &MultiServiceRouter, arn: &str, keys: &[&str]) -> Reply {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", percent_encode(key)))
        .collect::<Vec<_>>()
        .join("&");
    let path = format!("/tags/{}", percent_encode(arn));
    let path_and_query = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    call(router, "DELETE", &path_and_query, None).await
}

/// The tags `GetAppMonitor` reports for `name` — the persisted map, read back through an
/// implemented operation rather than inferred from the untag call's own answer.
async fn stored_tags(router: &MultiServiceRouter, case: &str, name: &str) -> BTreeMap<String, String> {
    let reply = call(router, "GET", &format!("/appmonitor/{name}"), None).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected GetAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    match &reply.json(case)["AppMonitor"]["Tags"] {
        // restJson1 draws no distinction between an absent map member and an empty one.
        Value::Null => BTreeMap::new(),
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value.as_str().unwrap_or_default().to_string(),
                )
            })
            .collect(),
        other => panic!("case {case:?}: expected Tags to be a map, got {other:?}"),
    }
}

/// A tag map whose keys are `keys` and whose values are derived from them, so an
/// assertion can tell which key a surviving value came from.
fn tags_for(keys: &[&str]) -> BTreeMap<String, String> {
    keys.iter()
        .map(|key| ((*key).to_string(), format!("value-of-{key}")))
        .collect()
}

/// One row of the removal table.
struct RemovalCase {
    case: &'static str,
    /// The keys the monitor is created with.
    initial: Vec<String>,
    /// The keys the untag request names.
    removed: Vec<String>,
    /// The keys expected to survive.
    surviving: Vec<String>,
}

/// The removal table: each row differs along what the rule generalises over — how many
/// keys are named, which of the resource's keys they are, whether a named key is one the
/// resource carries at all, whether a key is named twice, whether its spelling differs
/// only in case, and which characters and lengths the keys use.
fn removal_cases() -> Vec<RemovalCase> {
    let owned = |keys: &[&str]| keys.iter().map(|k| (*k).to_string()).collect::<Vec<_>>();
    let shortest = "a".repeat(TAG_KEY_BOUNDS.0);
    let longest = "k".repeat(TAG_KEY_BOUNDS.1);
    let catalog = owned(&TAG_KEY_CATALOG);

    vec![
        RemovalCase {
            case: "one key of three",
            initial: owned(&["Alpha", "Beta", "Gamma"]),
            removed: owned(&["Beta"]),
            surviving: owned(&["Alpha", "Gamma"]),
        },
        RemovalCase {
            case: "two keys of three",
            initial: owned(&["Alpha", "Beta", "Gamma"]),
            removed: owned(&["Gamma", "Alpha"]),
            surviving: owned(&["Beta"]),
        },
        RemovalCase {
            case: "every key of three",
            initial: owned(&["Alpha", "Beta", "Gamma"]),
            removed: owned(&["Alpha", "Beta", "Gamma"]),
            surviving: Vec::new(),
        },
        RemovalCase {
            case: "the resource's only key",
            initial: owned(&["Alpha"]),
            removed: owned(&["Alpha"]),
            surviving: Vec::new(),
        },
        RemovalCase {
            case: "a key the resource does not carry",
            initial: owned(&["Alpha", "Beta"]),
            removed: owned(&["Delta"]),
            surviving: owned(&["Alpha", "Beta"]),
        },
        RemovalCase {
            case: "a carried key beside one that is not carried",
            initial: owned(&["Alpha", "Beta"]),
            removed: owned(&["Delta", "Alpha"]),
            surviving: owned(&["Beta"]),
        },
        RemovalCase {
            case: "the same key named twice",
            initial: owned(&["Alpha", "Beta"]),
            removed: owned(&["Alpha", "Alpha"]),
            surviving: owned(&["Beta"]),
        },
        RemovalCase {
            case: "a key differing only in case",
            initial: owned(&["Alpha", "Beta"]),
            removed: owned(&["alpha", "BETA"]),
            surviving: owned(&["Alpha", "Beta"]),
        },
        RemovalCase {
            case: "the shortest and the longest admitted key",
            initial: vec![shortest.clone(), longest.clone(), "Kept".to_string()],
            removed: vec![shortest, longest],
            surviving: owned(&["Kept"]),
        },
        RemovalCase {
            case: "every character class the TagKey pattern admits",
            initial: catalog.clone(),
            // Remove all but the last catalog entry, so one key of each request survives
            // and the removal cannot be confused with "clear everything".
            removed: catalog[..catalog.len() - 1].to_vec(),
            surviving: catalog[catalog.len() - 1..].to_vec(),
        },
    ]
}

#[tokio::test]
async fn untag_resource_removes_exactly_the_named_keys_from_the_stored_resource() {
    let router = emulator();

    for (index, row) in removal_cases().into_iter().enumerate() {
        let RemovalCase {
            case,
            initial,
            removed,
            surviving,
        } = row;
        let name = format!("monitor-{index}");
        let initial_tags = tags_for(
            &initial
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        create_app_monitor(&router, case, &name, &initial_tags).await;

        let arn = app_monitor_arn(REGION, ACCOUNT, &name);
        let keys = removed.iter().map(String::as_str).collect::<Vec<_>>();
        let reply = untag(&router, &arn, &keys).await;
        assert_eq!(
            reply.status,
            NO_CONTENT,
            "case {case:?}: expected UntagResource to answer {NO_CONTENT}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );

        let expected = tags_for(&surviving.iter().map(String::as_str).collect::<Vec<_>>());
        let actual = stored_tags(&router, case, &name).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored tags to be {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn untag_resource_touches_only_the_resource_its_arn_names() {
    let router = emulator();
    let case = "two monitors carrying the same tag keys";
    let tags = tags_for(&["Team", "Env"]);
    create_app_monitor(&router, case, "target", &tags).await;
    create_app_monitor(&router, case, "bystander", &tags).await;

    let reply = untag(
        &router,
        &app_monitor_arn(REGION, ACCOUNT, "target"),
        &["Team", "Env"],
    )
    .await;
    assert_eq!(
        reply.status,
        NO_CONTENT,
        "case {case:?}: expected UntagResource to answer {NO_CONTENT}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );

    let target = stored_tags(&router, case, "target").await;
    assert!(
        target.is_empty(),
        "case {case:?}: the named resource must have lost both tags, got {target:?}"
    );
    let bystander = stored_tags(&router, case, "bystander").await;
    assert_eq!(
        bystander, tags,
        "case {case:?}: another resource's tags must not move, expected {tags:?}, got \
         {bystander:?}"
    );
}

#[tokio::test]
async fn untag_resource_is_idempotent() {
    let router = emulator();
    // `@idempotent` on `UntagResource` (`smithy_query(query="operation",
    // operation="UntagResource")`): repeating the same request leaves the same tag set.
    for (case, keys) in [
        ("one carried key", vec!["Team"]),
        ("every carried key", vec!["Team", "Env"]),
        ("a key that is not carried", vec!["Absent"]),
    ] {
        let name = format!("monitor-{}", case.replace(' ', "-"));
        let tags = tags_for(&["Team", "Env"]);
        create_app_monitor(&router, case, &name, &tags).await;
        let arn = app_monitor_arn(REGION, ACCOUNT, &name);

        let first = untag(&router, &arn, &keys).await;
        assert_eq!(
            first.status,
            NO_CONTENT,
            "case {case:?}: expected the first UntagResource to answer {NO_CONTENT}, got {} {:?}",
            first.status,
            String::from_utf8_lossy(&first.body)
        );
        let after_first = stored_tags(&router, case, &name).await;

        let second = untag(&router, &arn, &keys).await;
        assert_eq!(
            second.status,
            NO_CONTENT,
            "case {case:?}: expected the repeated UntagResource to answer {NO_CONTENT}, got {} \
             {:?}",
            second.status,
            String::from_utf8_lossy(&second.body)
        );
        let after_second = stored_tags(&router, case, &name).await;

        assert_eq!(
            after_second, after_first,
            "case {case:?}: repeating the request must not change the tags again, expected \
             {after_first:?}, got {after_second:?}"
        );
    }
}

#[tokio::test]
async fn untag_resource_refuses_an_arn_that_names_no_stored_resource() {
    let router = emulator();
    let case = "the stored resource";
    let tags = tags_for(&["Team", "Env"]);
    create_app_monitor(&router, case, "tagged", &tags).await;

    // One row per ARN component: each is a well-formed ARN differing from the stored
    // one in exactly one place, so none of them names a resource this service holds.
    let cases: Vec<(&str, String)> = vec![
        (
            "an unknown resource name",
            app_monitor_arn(REGION, ACCOUNT, "no-such-monitor"),
        ),
        (
            "another region",
            app_monitor_arn("eu-west-1", ACCOUNT, "tagged"),
        ),
        (
            "another account",
            app_monitor_arn(REGION, "123456789012", "tagged"),
        ),
        (
            "another service",
            format!("arn:aws:cloudwatch:{REGION}:{ACCOUNT}:appmonitor/tagged"),
        ),
        (
            "another resource type",
            format!("arn:aws:rum:{REGION}:{ACCOUNT}:dashboard/tagged"),
        ),
        (
            "another partition",
            format!("arn:aws-cn:rum:{REGION}:{ACCOUNT}:appmonitor/tagged"),
        ),
    ];

    for (case, arn) in cases {
        let reply = untag(&router, &arn, &["Team"]).await;
        assert_eq!(
            reply.status,
            404,
            "case {case:?}: expected UntagResource to answer 404 for {arn:?}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let code = reply.error_code(case);
        assert_eq!(
            code, NOT_FOUND_ERROR_CODE,
            "case {case:?}: expected the wire error code {NOT_FOUND_ERROR_CODE:?}, got {code:?}"
        );
        let document = reply.json(case);
        // RUM's not-found body is `message` only — `resourceName`/`resourceType` are
        // modelled but never serialized, so a client gets no second copy of the ARN it
        // sent (see `crate::ops::types::ResourceNotFoundException`).
        assert_eq!(
            document["resourceName"],
            Value::Null,
            "case {case:?}: expected no resourceName member, got {:?}",
            document["resourceName"]
        );
        assert_eq!(
            document["resourceType"],
            Value::Null,
            "case {case:?}: expected no resourceType member, got {:?}",
            document["resourceType"]
        );
        // The message is the service's own constant for this condition — the same
        // sentence for every row of this table, which varies the ARN in every component.
        assert_eq!(
            message(&document),
            Some(NOT_FOUND_MESSAGE),
            "case {case:?}: expected the message {NOT_FOUND_MESSAGE:?}, got {:?}",
            message(&document)
        );

        let stored = stored_tags(&router, case, "tagged").await;
        assert_eq!(
            stored, tags,
            "case {case:?}: a refused request must remove nothing, expected {tags:?}, got \
             {stored:?}"
        );
    }
}

#[tokio::test]
async fn untag_resource_refuses_a_resource_arn_that_is_not_an_arn() {
    let router = emulator();
    let setup = "a monitor that keeps its tags";
    let tags = tags_for(&["Team", "Env"]);
    create_app_monitor(&router, setup, "tagged", &tags).await;

    // One row per way a value can fail `Arn`'s modelled pattern
    // `arn:[^:]*:[^:]*:[^:]*:[^:]*:.*`
    // (`smithy_query(query="operation", operation="UntagResource")`): no `arn:` prefix at
    // all, too few fields, a bare resource path, and the right field count under the
    // wrong prefix.
    for (case, arn) in [
        ("no ARN syntax at all", "not-an-arn-at-all"),
        ("too few fields", "arn:aws:rum"),
        ("the account field missing", "arn:aws:rum:us-east-1"),
        ("a bare resource path", "appmonitor/tagged"),
        ("the wrong prefix", "urn:aws:rum:us-east-1:000000000000:appmonitor/tagged"),
    ] {
        let reply = untag(&router, arn, &["Team"]).await;
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected UntagResource to refuse {arn:?} with 400, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let code = reply.error_code(case);
        assert_eq!(
            code, BAD_REQUEST_ERROR_CODE,
            "case {case:?}: expected the wire error code {BAD_REQUEST_ERROR_CODE:?}, got \
             {code:?}"
        );
        // The message is the frontend's constant for a `ResourceArn` that is not an ARN:
        // the same sentence for every row of this table, which fails the syntax in a
        // different way in each one.
        let document = reply.json(case);
        assert_eq!(
            message(&document),
            Some(INVALID_RESOURCE_ARN_MESSAGE),
            "case {case:?}: expected the message {INVALID_RESOURCE_ARN_MESSAGE:?}, got {:?}",
            message(&document)
        );
    }

    let stored = stored_tags(&router, setup, "tagged").await;
    assert_eq!(
        stored, tags,
        "case {setup:?}: a refused request must remove nothing, expected {tags:?}, got \
         {stored:?}"
    );
}

#[tokio::test]
async fn a_refusal_outside_the_tagging_operations_keeps_the_modelled_code() {
    // The tag frontend's code is scoped to the tag operations: a monitor call refused at
    // the boundary on one of its own modelled constraints still answers the
    // `ValidationException` this service's model declares.
    let router = emulator();

    // An absent required member: `CreateAppMonitorRequest$Name` is `@required`.
    // Source: `smithy_query(query="operation", operation="CreateAppMonitor")`.
    let reply = call(
        &router,
        "POST",
        "/appmonitor",
        Some(json!({"Domain": "example.com"})),
    )
    .await;
    let case = "an absent required member";
    assert_eq!(
        reply.status,
        400,
        "case {case:?}: expected CreateAppMonitor to refuse the request with 400, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let code = reply.error_code(case);
    assert_eq!(
        code, "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got {code:?}"
    );

    // A value outside a modelled range: `ListAppMonitorsRequest$MaxResults` is
    // `@range(min: 1, max: 100)`, bound to the `maxResults` query parameter — the
    // published bounds and one past each.
    // Source: `smithy_query(query="operation", operation="ListAppMonitors")`.
    for (case, max_results, refused) in [
        ("one below the range", 0, true),
        ("the range's minimum", 1, false),
        ("the range's maximum", 100, false),
        ("one above the range", 101, true),
    ] {
        let reply = call(
            &router,
            "POST",
            &format!("/appmonitors?maxResults={max_results}"),
            None,
        )
        .await;
        if !refused {
            assert_eq!(
                reply.status,
                200,
                "case {case:?}: expected ListAppMonitors to accept maxResults={max_results}, \
                 got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            continue;
        }
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected ListAppMonitors to refuse maxResults={max_results} with \
             400, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let code = reply.error_code(case);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {code:?}"
        );
    }
}
