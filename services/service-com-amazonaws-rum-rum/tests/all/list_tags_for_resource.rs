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
//! ARN component per row; the after-a-write test asserts the read follows the store; and
//! the window table holds a read open at its authorization decision — the one point where
//! the handler holds no lock of this service's — while a concurrent commit changes the
//! store under it, so what the read projects its answer from is asserted rather than
//! assumed.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use service_com_amazonaws_rum_rum::state as rum_store;
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
async fn an_arn_resolves_only_from_the_region_that_owns_the_resource() {
    // The ARN is the whole identity, and the region it carries is part of it: a RUM
    // endpoint serves the resources of the region it is signed for, so the ARN of a
    // monitor created in another region names nothing here — the same scope
    // `GetAppMonitor` resolves a NAME in, so the two resolutions agree.
    let (router, _engine) = emulator();
    // (case, the region the create is signed for, the region the reads are signed for)
    let rows: [(&str, &str, &str); 4] = [
        ("read where it was created", "us-east-1", "us-east-1"),
        ("read where another region's was created", "eu-west-1", "eu-west-1"),
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
        if create_region != read_region {
            // The read is signed for a region that owns no such resource.
            assert_eq!(
                reply.status,
                404,
                "case {case:?}: expected ListTagsForResource signed for {read_region} to \
                 answer 404 for a {create_region} ARN, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(case),
                NOT_FOUND_ERROR_CODE,
                "case {case:?}: expected {NOT_FOUND_ERROR_CODE}, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                message(&reply.json(case)),
                Some(NOT_FOUND_MESSAGE),
                "case {case:?}: expected the refusal to carry {NOT_FOUND_MESSAGE:?}, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
            continue;
        }
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

/// How long a case leaves a read parked in its authorization decision before the
/// concurrent commit lands.
///
/// Not a correctness requirement — the lock below is what pins the window, and a read that
/// has not reached the decision yet resolves the ALREADY-committed store and answers the
/// same. It is only long enough that the read has got past its own resolution, which is
/// what makes the difference between answering a pre-decision snapshot and answering the
/// store observable.
const DECISION_WINDOW: Duration = Duration::from_millis(50);

/// Drive one `ListTagsForResource` for `arn` whose authorization decision is held open
/// while `commit` changes RUM's store, and answer what that read replied.
///
/// The window is PINNED rather than raced for. Every RUM authorization decision resolves
/// the calling principal out of the IAM service's own store, so holding that store's lock
/// parks a read that has passed its own resolution exactly at the decision — the point
/// where the handler holds no lock of RUM's, which is what lets `commit` take RUM's lock
/// and land strictly inside the window. The read is driven on its own thread because it
/// blocks there.
fn read_with_a_commit_in_the_decision_window(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    arn: &str,
    commit: impl FnOnce(&mut rum_store::ComAmazonawsRumRumState),
) -> crate::support::Reply {
    let iam = iam_store::service_state(engine).expect("the IAM store a decision is made from");
    let rum = rum_store::service_state(engine).expect("RUM's own store");
    let decision_held_open = iam.lock();

    let reading = {
        let router = router.clone();
        let path = tags_path(arn);
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime for the parked read")
                .block_on(async move { call(&router, "GET", &path, None).await })
        })
    };

    std::thread::sleep(DECISION_WINDOW);
    commit(&mut rum.lock());
    drop(decision_held_open);
    reading.join().expect("the parked read answered")
}

/// One row of the window table: what a concurrent caller commits to the store while the
/// read is parked in its decision, and what the read must then answer.
struct WindowCase {
    case: &'static str,
    /// The commit, taking RUM's store and the monitor's name.
    commit: fn(&mut rum_store::ComAmazonawsRumRumState, &str),
    /// The tag map the read must report, or `None` when it must answer the frontend's 404
    /// because the store no longer holds the resource.
    expected: Option<&'static [(&'static str, &'static str)]>,
}

/// The tags the monitor each row reads is created with.
const SEEDED_TAGS: [(&str, &str); 2] = [("Team", "web"), ("Env", "prod")];

/// The window table. Rows differ along WHAT the concurrent commit does to the record the
/// read resolved — nothing, removes it, adds a tag, removes one, empties the map, replaces
/// it — because the rule under test is about WHICH store state the answer is projected
/// from, not about any one change: whatever the store holds once the decision is made is
/// what the read reports, and a resource it no longer holds is refused.
fn window_cases() -> Vec<WindowCase> {
    vec![
        WindowCase {
            case: "nothing commits in the window",
            commit: |_, _| {},
            expected: Some(&SEEDED_TAGS),
        },
        WindowCase {
            case: "the resource is removed in the window, as DeleteAppMonitor removes it",
            commit: |store, name| {
                store
                    .remove_app_monitor(REGION, name)
                    .expect("the monitor the case created");
            },
            expected: None,
        },
        WindowCase {
            case: "a tag is added in the window, as TagResource adds one",
            commit: |store, name| {
                store
                    .app_monitor_mut(REGION, name)
                    .expect("the monitor the case created")
                    .tags
                    .insert("Owner".to_string(), "rum".to_string());
            },
            expected: Some(&[("Team", "web"), ("Env", "prod"), ("Owner", "rum")]),
        },
        WindowCase {
            case: "a tag is removed in the window, as UntagResource removes one",
            commit: |store, name| {
                store
                    .app_monitor_mut(REGION, name)
                    .expect("the monitor the case created")
                    .tags
                    .remove("Env");
            },
            expected: Some(&[("Team", "web")]),
        },
        WindowCase {
            case: "every tag is removed in the window",
            commit: |store, name| {
                store
                    .app_monitor_mut(REGION, name)
                    .expect("the monitor the case created")
                    .tags
                    .clear();
            },
            expected: Some(&[]),
        },
        WindowCase {
            case: "the whole map is replaced in the window",
            commit: |store, name| {
                let record = store
                    .app_monitor_mut(REGION, name)
                    .expect("the monitor the case created");
                record.tags.clear();
                record.tags.insert("Stage".to_string(), "beta".to_string());
                record.tags.insert("Tier".to_string(), "gold".to_string());
            },
            expected: Some(&[("Stage", "beta"), ("Tier", "gold")]),
        },
    ]
}

#[tokio::test]
async fn the_read_answers_the_store_as_it_stands_once_the_decision_is_made() {
    // The rule: a reporting operation projects its answer from the record it resolves from
    // the store AFTER its authorization decision — the decision is made with no lock of
    // this service's held — so a resource the store no longer holds by then is refused with
    // the frontend's 404 and a change committed in that window is what the read reports.
    // Each row drives the real wire path with the decision held open, so the commit lands
    // inside the window every time instead of being raced for.
    for WindowCase {
        case,
        commit,
        expected,
    } in window_cases()
    {
        let (router, engine) = emulator();
        let name = "monitor-in-the-window";
        create(
            &router,
            case,
            json!({
                "Name": name,
                "Domain": "example.com",
                "Tags": tag_object(SEEDED_TAGS.iter().copied()),
            }),
        )
        .await;
        let arn = app_monitor_arn(REGION, ACCOUNT, name);

        let reply = read_with_a_commit_in_the_decision_window(&router, &engine, &arn, |store| {
            commit(store, name)
        });

        let Some(expected) = expected else {
            assert_eq!(
                reply.status,
                404,
                "case {case:?}: expected 404 for a resource the store no longer holds, got \
                 {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            let code = reply.error_code(case);
            assert_eq!(
                code, NOT_FOUND_ERROR_CODE,
                "case {case:?}: expected the wire error code {NOT_FOUND_ERROR_CODE:?}, got \
                 {code:?}"
            );
            let document = reply.json(case);
            assert_eq!(
                message(&document),
                Some(NOT_FOUND_MESSAGE),
                "case {case:?}: expected the message {NOT_FOUND_MESSAGE:?}, got {:?}",
                message(&document)
            );
            // The sibling name-addressed reader answers for the same instant, and the two
            // readers of one record cannot disagree about whether it is there.
            let gone = call(&router, "GET", &format!("/appmonitor/{name}"), None).await;
            assert_eq!(
                gone.status,
                404,
                "case {case:?}: expected GetAppMonitor to answer 404 for the same instant, \
                 got {} {:?}",
                gone.status,
                String::from_utf8_lossy(&gone.body)
            );
            continue;
        };

        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let document = reply.json(case);
        assert_eq!(
            document["ResourceArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected ResourceArn {arn:?}, got {:?}",
            document["ResourceArn"]
        );
        let answered = tag_map(&document);
        let expected = tag_object(expected.iter().copied());
        assert_eq!(
            answered, expected,
            "case {case:?}: expected Tags {expected:?}, got {answered:?}"
        );
        // And the answer is the state the store holds, not a copy taken before the
        // decision: the other reader of that record reports the same map.
        let from_get = tag_map(&get_app_monitor(&router, case, name).await);
        assert_eq!(
            from_get, expected,
            "case {case:?}: GetAppMonitor must report the same tags, got {from_get:?}"
        );
    }
}
