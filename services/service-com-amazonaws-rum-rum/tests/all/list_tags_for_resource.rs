// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for CloudWatch RUM's `ListTagsForResource`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, the percent-encoded `ResourceArn` path label, the
//! modelled constraints, the handler and the response envelope are all exercised, not
//! just the handler body. What a read reports is cross-checked against the persisted
//! record through `GetAppMonitor`'s `AppMonitor$Tags` and against what `UntagResource`
//! leaves behind, so a case asserts PERSISTED state rather than one call's own answer.
//!
//! The assertions pin RULES over a SET of inputs rather than one request: the tag table
//! varies how MANY tags a resource carries (none, one, several, the documented maximum of
//! 50) and what a key and a value may be (every class of character `TagKey`'s pattern
//! admits, both of its length bounds, an empty value, a maximal value); the invariance
//! test asserts a read changes nothing a later caller sees; the refusal table walks one
//! ARN component per row; and the after-a-write test asserts the read follows the store.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

// The ONE wire harness, shared with this crate's app-monitor suite.
use crate::support::{
    call, call_in, create, emulator, get_app_monitor, message, tag_map, ACCOUNT, REGION,
};

/// The wire error code RUM's tagging frontend answers for a `ResourceArn` that names no
/// existing resource, at the HTTP 404 the model's `ResourceNotFoundException` carries.
/// Source: the recorded corpus unit `rum.untag-resource-unknown-resource-rejected`, whose
/// check asserts `err["Code"] == "NotFoundException"` for the sibling tagging operation;
/// the shape is not in the model, so the code is not the shape name.
const NOT_FOUND_ERROR_CODE: &str = "NotFoundException";

/// The message that refusal carries: a constant the service words the same way whatever
/// ARN the request named, and the WHOLE body — RUM's `ResourceNotFoundException` puts
/// neither `resourceName` nor `resourceType` on the wire.
/// Source: the same recorded unit.
const NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The wire error code that frontend answers a malformed tag request with, where this
/// service's app-monitor operations answer the modelled `ValidationException`.
/// Source: the same recorded unit, whose check sends a `ResourceArn` that is not an ARN.
const BAD_REQUEST_ERROR_CODE: &str = "BadRequestException";

/// The most tags AWS documents a CloudWatch RUM resource takes.
/// Source: `smithy_query(query="operation", operation="TagResource")` doc — "You can
/// associate as many as 50 tags with a resource."
const MAX_DOCUMENTED_TAGS: usize = 50;

/// `TagKey`'s modelled length bounds.
/// Source: `smithy_query(query="shape", shape="TagMap")` — the key is `@length(min: 1,
/// max: 128)`.
const TAG_KEY_BOUNDS: (usize, usize) = (1, 128);

/// `TagValue`'s modelled length bounds.
/// Source: `smithy_query(query="shape", shape="TagMap")` — the value is `@length(min: 0,
/// max: 256)`.
const TAG_VALUE_BOUNDS: (usize, usize) = (0, 256);

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

/// The ARN a created app monitor carries: `arn:aws:rum:<region>:<account>:appmonitor/<name>`.
fn app_monitor_arn(region: &str, account: &str, name: &str) -> String {
    format!("arn:aws:rum:{region}:{account}:appmonitor/{name}")
}

/// `ResourceArn` is bound to a NON-greedy `@httpLabel` (`GET /tags/{ResourceArn}`), so an
/// ARN travels as ONE path segment: every byte outside RFC 3986's unreserved set is
/// percent-encoded, which is how an SDK writes it.
fn tags_path(resource_arn: &str) -> String {
    let encoded: String = resource_arn
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect();
    format!("/tags/{encoded}")
}

/// `ListTagsForResource`: `GET /tags/{ResourceArn}`, signed for `region`.
async fn list_tags_in(
    router: &MultiServiceRouter,
    region: &str,
    resource_arn: &str,
) -> crate::support::Reply {
    call_in(router, region, "GET", &tags_path(resource_arn), None).await
}

async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> crate::support::Reply {
    list_tags_in(router, REGION, resource_arn).await
}

/// The `Tags` a successful `ListTagsForResource` reported, asserting the call answered 200
/// and reported the resolved resource's own ARN.
async fn reported_tags(
    router: &MultiServiceRouter,
    case: &str,
    resource_arn: &str,
) -> Value {
    let reply = list_tags(router, resource_arn).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected ListTagsForResource to answer 200 for {resource_arn:?}, \
         got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let document = reply.json(case);
    assert_eq!(
        document["ResourceArn"].as_str(),
        Some(resource_arn),
        "case {case:?}: expected ResourceArn {resource_arn:?}, got {:?}",
        document["ResourceArn"]
    );
    tag_map(&document)
}

/// A JSON object from `(key, value)` pairs.
fn tag_object<I, K, V>(pairs: I) -> Value
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    Value::Object(
        pairs
            .into_iter()
            .map(|(key, value)| (key.into(), Value::String(value.into())))
            .collect::<Map<String, Value>>(),
    )
}

/// One row of the tag table: a monitor to create and the map a read must report for it.
struct TagCase {
    case: &'static str,
    name: String,
    /// The `Tags` member the create request carries, or `None` when it carries none.
    requested: Option<Value>,
    /// The map `ListTagsForResource` must report.
    expected: Value,
}

/// The tag table. Rows differ along what the rule generalises over: how many tags there
/// are (0, 1, 3, the documented 50), which characters a key uses (the whole `TagKey`
/// catalog), how long a key and a value are (both modelled bounds), and whether a value
/// is empty.
fn tag_cases() -> Vec<TagCase> {
    let many = (0..MAX_DOCUMENTED_TAGS)
        .map(|index| (format!("key{index:02}"), format!("value{index:02}")))
        .collect::<Vec<_>>();
    let catalog = TAG_KEY_CATALOG
        .iter()
        .map(|key| ((*key).to_string(), format!("value-of-{key}")))
        .collect::<Vec<_>>();
    let bounds = vec![
        ("k".repeat(TAG_KEY_BOUNDS.0), "v".repeat(TAG_VALUE_BOUNDS.1)),
        ("K".repeat(TAG_KEY_BOUNDS.1), "v".repeat(TAG_VALUE_BOUNDS.0)),
    ];

    vec![
        TagCase {
            case: "a create carrying no Tags member at all",
            name: "monitor-untagged".to_string(),
            requested: None,
            expected: json!({}),
        },
        TagCase {
            case: "a create carrying an explicitly empty tag map",
            name: "monitor-empty-tags".to_string(),
            requested: Some(json!({})),
            expected: json!({}),
        },
        TagCase {
            case: "one tag",
            name: "monitor-one-tag".to_string(),
            requested: Some(json!({"Team": "web"})),
            expected: json!({"Team": "web"}),
        },
        TagCase {
            case: "three tags",
            name: "monitor-three-tags".to_string(),
            requested: Some(json!({"Team": "web", "Env": "prod", "Owner": "rum"})),
            expected: json!({"Team": "web", "Env": "prod", "Owner": "rum"}),
        },
        TagCase {
            case: "every character class the TagKey pattern admits",
            name: "monitor-key-catalog".to_string(),
            requested: Some(tag_object(catalog.clone())),
            expected: tag_object(catalog),
        },
        TagCase {
            case: "both key and both value length bounds",
            name: "monitor-bounded-tags".to_string(),
            requested: Some(tag_object(bounds.clone())),
            expected: tag_object(bounds),
        },
        TagCase {
            case: "the 50 tags AWS documents as the most a resource takes",
            name: "monitor-fifty-tags".to_string(),
            requested: Some(tag_object(many.clone())),
            expected: tag_object(many),
        },
    ]
}

#[tokio::test]
async fn list_tags_for_resource_reports_the_map_each_resource_carries() {
    let (router, _engine) = emulator();

    for TagCase {
        case,
        name,
        requested,
        expected,
    } in tag_cases()
    {
        let mut body = json!({"Name": name, "Domain": "example.com"});
        if let Some(tags) = requested {
            body.as_object_mut()
                .expect("a JSON object")
                .insert("Tags".to_string(), tags);
        }
        create(&router, case, body).await;

        let arn = app_monitor_arn(REGION, ACCOUNT, &name);
        let answered = reported_tags(&router, case, &arn).await;
        assert_eq!(
            answered, expected,
            "case {case:?}: expected Tags {expected:?}, got {answered:?}"
        );

        // One projection: the tags `GetAppMonitor` reports inside `AppMonitor$Tags` and
        // the ones `ListTagsForResource` reports come off the same stored record, so the
        // two readers cannot disagree about one resource.
        let from_get = tag_map(&get_app_monitor(&router, case, &name).await);
        assert_eq!(
            from_get, expected,
            "case {case:?}: GetAppMonitor must report the same tags, got {from_get:?}"
        );
    }
}

#[tokio::test]
async fn reading_the_tags_changes_nothing_a_later_caller_sees() {
    // "Displays the tags associated with a CloudWatch RUM resource" is a report: repeated
    // reads answer identically and leave the resource exactly as it was.
    let (router, _engine) = emulator();
    let name = "monitor-read-repeatedly";
    let tags = json!({"Team": "web", "Env": "prod"});
    create(
        &router,
        "repeated read",
        json!({"Name": name, "Domain": "example.com", "Tags": tags}),
    )
    .await;
    let arn = app_monitor_arn(REGION, ACCOUNT, name);

    for attempt in 0..3 {
        let case = format!("read {attempt}");
        let answered = reported_tags(&router, &case, &arn).await;
        assert_eq!(
            answered, tags,
            "case {case:?}: expected Tags {tags:?}, got {answered:?}"
        );
    }

    let monitor = get_app_monitor(&router, "after the reads", name).await;
    assert_eq!(
        tag_map(&monitor),
        tags,
        "case \"after the reads\": the resource's tags must be unchanged, got {:?}",
        monitor["Tags"]
    );
    assert_eq!(
        monitor["State"].as_str(),
        Some(service_com_amazonaws_rum_rum::state::CREATED_STATE),
        "case \"after the reads\": a read must not move the resource's state, got {:?}",
        monitor["State"]
    );
}

#[tokio::test]
async fn the_reported_map_follows_the_stored_one_through_a_removal() {
    // The read reports the map the store holds NOW, not the one the create supplied: an
    // `UntagResource` between two reads changes what the second one answers.
    let (router, _engine) = emulator();
    let case = "a removal between two reads";
    let name = "monitor-after-untag";
    create(
        &router,
        case,
        json!({"Name": name, "Domain": "example.com", "Tags": {"Team": "web", "Env": "prod"}}),
    )
    .await;
    let arn = app_monitor_arn(REGION, ACCOUNT, name);

    let before = reported_tags(&router, case, &arn).await;
    assert_eq!(
        before,
        json!({"Team": "web", "Env": "prod"}),
        "case {case:?}: expected both tags before the removal, got {before:?}"
    );

    let removed = call(
        &router,
        "DELETE",
        &format!("{}?tagKeys=Env", tags_path(&arn)),
        None,
    )
    .await;
    assert_eq!(
        removed.status,
        204,
        "case {case:?}: expected UntagResource to answer 204 No Content, got {} {:?}",
        removed.status,
        String::from_utf8_lossy(&removed.body)
    );

    let after = reported_tags(&router, case, &arn).await;
    assert_eq!(
        after,
        json!({"Team": "web"}),
        "case {case:?}: expected only the key that was not removed, got {after:?}"
    );
}

#[tokio::test]
async fn list_tags_refuses_an_arn_that_names_no_stored_resource() {
    let (router, _engine) = emulator();
    let setup = "the stored resource";
    create(
        &router,
        setup,
        json!({"Name": "tagged", "Domain": "example.com", "Tags": {"Team": "web"}}),
    )
    .await;
    create(
        &router,
        "the resource that gets deleted",
        json!({"Name": "doomed", "Domain": "example.com", "Tags": {"Team": "web"}}),
    )
    .await;
    let deleted = call(&router, "DELETE", "/appmonitor/doomed", None).await;
    assert_eq!(
        deleted.status,
        200,
        "expected DeleteAppMonitor to answer 200, got {} {:?}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );

    // One row per ARN component: each is a well-formed ARN differing from a stored one in
    // exactly one place, so none of them names a resource this service holds. The last
    // row is the ARN of a resource that existed and was deleted — its tags left the store
    // with the record.
    let rows: Vec<(&str, String)> = vec![
        (
            "an unknown resource name",
            app_monitor_arn(REGION, ACCOUNT, "no-such-monitor"),
        ),
        (
            "a stored name in another case",
            app_monitor_arn(REGION, ACCOUNT, "TAGGED"),
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
        (
            "a resource that was deleted",
            app_monitor_arn(REGION, ACCOUNT, "doomed"),
        ),
    ];

    for (case, arn) in rows {
        let reply = list_tags(&router, &arn).await;
        assert_eq!(
            reply.status,
            404,
            "case {case:?}: expected 404 for {arn:?}, got {} {:?}",
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
        assert_eq!(
            message(&document),
            Some(NOT_FOUND_MESSAGE),
            "case {case:?}: expected the message {NOT_FOUND_MESSAGE:?}, got {:?}",
            message(&document)
        );
    }

    // The refused reads left the resource that does exist exactly as it was.
    let still_there = reported_tags(
        &router,
        setup,
        &app_monitor_arn(REGION, ACCOUNT, "tagged"),
    )
    .await;
    assert_eq!(
        still_there,
        json!({"Team": "web"}),
        "case {setup:?}: the stored resource's tags must be untouched, got {still_there:?}"
    );
}

#[tokio::test]
async fn list_tags_refuses_a_resource_arn_that_is_not_an_arn() {
    let (router, _engine) = emulator();
    // One row per way a value can fail `Arn`'s modelled pattern
    // `arn:[^:]*:[^:]*:[^:]*:[^:]*:.*`
    // (`smithy_query(query="operation", operation="ListTagsForResource")`): no `arn:`
    // prefix at all, too few fields, a bare resource path, and the right field count
    // under the wrong prefix. The tag frontend words these as `BadRequestException`,
    // which is the service-wide rewording the sibling operations share.
    for (case, arn) in [
        ("no ARN syntax at all", "not-an-arn-at-all"),
        ("too few fields", "arn:aws:rum"),
        ("the account field missing", "arn:aws:rum:us-east-1"),
        ("a bare resource path", "appmonitor/tagged"),
        (
            "the wrong prefix",
            "urn:aws:rum:us-east-1:000000000000:appmonitor/tagged",
        ),
    ] {
        let reply = list_tags(&router, arn).await;
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected 400 for {arn:?}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let code = reply.error_code(case);
        assert_eq!(
            code, BAD_REQUEST_ERROR_CODE,
            "case {case:?}: expected the wire error code {BAD_REQUEST_ERROR_CODE:?}, got \
             {code:?}"
        );
    }
}

#[tokio::test]
async fn one_resources_tags_are_not_another_resources_tags() {
    // Tags live on the resource the ARN names: creating further resources never changes
    // an earlier one's map, and each ARN reports only its own.
    let (router, _engine) = emulator();
    let mut expected: BTreeMap<String, Value> = BTreeMap::new();
    for index in 0..4 {
        let name = format!("monitor-isolated-{index}");
        let tags = tag_object([
            ("Index".to_string(), index.to_string()),
            ("Shared".to_string(), "yes".to_string()),
        ]);
        create(
            &router,
            "isolation",
            json!({"Name": name, "Domain": "example.com", "Tags": tags.clone()}),
        )
        .await;
        expected.insert(name, tags);

        for (stored, map) in &expected {
            let case = format!("after creating {index}, reading {stored}");
            let answered =
                reported_tags(&router, &case, &app_monitor_arn(REGION, ACCOUNT, stored)).await;
            assert_eq!(
                answered, *map,
                "case {case:?}: expected Tags {map:?}, got {answered:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_resource_an_arn_names_is_reachable_from_any_signing_region() {
    // The ARN is the whole identity, and it is the one the create minted: the region the
    // READ is signed for plays no part, just as it plays none in `GetAppMonitor`.
    let (router, _engine) = emulator();
    // (case, the region the create is signed for, the region the reads are signed for)
    let rows: [(&str, &str, &str); 3] = [
        ("one region", "us-east-1", "us-east-1"),
        ("created in another region", "us-west-2", "us-east-1"),
        ("read from another region", "eu-west-1", "ap-south-1"),
    ];

    for (index, (case, create_region, read_region)) in rows.into_iter().enumerate() {
        let name = format!("monitor-region-{index}");
        let tags = json!({"Env": create_region});
        let created = call_in(
            &router,
            create_region,
            "POST",
            "/appmonitor",
            Some(json!({"Name": name, "Domain": "example.com", "Tags": tags})),
        )
        .await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: expected CreateAppMonitor to answer 200, got {} {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        let minted = app_monitor_arn(create_region, ACCOUNT, &name);
        let reply = list_tags_in(&router, read_region, &minted).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected ListTagsForResource to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let document = reply.json(case);
        assert_eq!(
            tag_map(&document),
            tags,
            "case {case:?}: expected Tags {tags:?}, got {:?}",
            document["Tags"]
        );
        assert_eq!(
            document["ResourceArn"].as_str(),
            Some(minted.as_str()),
            "case {case:?}: expected ResourceArn {minted:?}, got {:?}",
            document["ResourceArn"]
        );
    }
}
