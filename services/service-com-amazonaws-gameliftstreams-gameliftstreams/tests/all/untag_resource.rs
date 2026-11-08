// SPDX-License-Identifier: CC0-1.0

//! `UntagResource` over the wire: removing tags from this service's taggable
//! resources.
//!
//! The rule under test is one sentence: `UntagResource` removes exactly the keys
//! the request names from the named resource and leaves every other key it carries
//! untouched, so a key the resource does not carry is removed by removing nothing.
//!
//! The tables below span that class — how many keys are named, whether each is held
//! or not, the key itself (the longest the model allows, one needing
//! percent-encoding), and the resource type — rather than one recorded case. What a
//! removal WROTE is read back through `ListTagsForResource`, which has its own
//! suite (`tests/all/resource_tags.rs`).

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_gameliftstreams_gameliftstreams::state::TAGS_PER_RESOURCE;

use super::stream_workflow::{
    active_stream_group, application_body, as_label, call, emulator, ready_application, text,
    ACCOUNT, PAST_TRANSITION, REGION,
};

/// The resource types this service lets a caller tag, and therefore untag: "You can
/// assign tags to the following Amazon GameLift Streams resource types: Application
/// StreamGroup" (`TagResource`'s model documentation).
const TAGGABLE: [&str; 2] = ["application", "streamgroup"];

/// The status a successful `UntagResource` answers with: the write has nothing to
/// report, and this API answers its bodiless writes with 204 No Content.
const UNTAG_RESOURCE_SUCCESS: u16 = 204;

/// `[(key, value)]` as the `Tags` map the wire carries.
fn tag_map(pairs: &[(String, String)]) -> Value {
    let map: Map<String, Value> = pairs
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    Value::Object(map)
}

fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `UntagResource`: `DELETE /tags/{ResourceArn}?tagKeys=...`, where `TagKeys` is
/// bound to the `tagKeys` query parameter, one repetition per key.
async fn untag(router: &MultiServiceRouter, arn: &str, keys: &[String]) -> (u16, Value) {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", as_label(key)))
        .collect::<Vec<_>>()
        .join("&");
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{query}", as_label(arn)),
        None,
    )
    .await
}

/// `TagResource`: `POST /tags/{ResourceArn}`.
async fn tag(router: &MultiServiceRouter, arn: &str, tags: &[(String, String)]) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/tags/{}", as_label(arn)),
        Some(json!({"Tags": tag_map(tags)})),
    )
    .await
}

/// `ListTagsForResource`: `GET /tags/{ResourceArn}`.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags/{}", as_label(arn)), None).await
}

/// The tag set the resource carries, as `ListTagsForResource` reports it: a
/// resource holding none reports the member absent (`Tags` is modelled
/// `length 1..50`), which is the empty map here.
async fn stored_tags(router: &MultiServiceRouter, arn: &str, case: &str) -> Value {
    let (status, got) = list_tags(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListTagsForResource 200, got {status} {got}"
    );
    got.get("Tags").cloned().unwrap_or_else(|| json!({}))
}

/// Create one taggable resource of `kind` carrying `tags`, and answer its ARN. A
/// stream group needs an application to host, so the `streamgroup` rows create one
/// and tag only the group.
async fn taggable_resource(
    router: &MultiServiceRouter,
    kind: &str,
    case: &str,
    tags: &[(String, String)],
) -> String {
    let mut application = application_body(case);
    // `Tags` is modelled `length 1..50`, so an empty map is never sent.
    if kind == "application" && !tags.is_empty() {
        application["Tags"] = tag_map(tags);
    }
    let (status, created) = call(router, "POST", "/applications", Some(application)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateApplication 201, got {status} {created}"
    );
    let application_arn = text(&created, "/Arn", case);
    if kind == "application" {
        return application_arn;
    }
    assert_eq!(
        kind, "streamgroup",
        "case {case:?}: unknown taggable resource type {kind:?}"
    );
    let mut group = json!({
        "Description": case,
        "StreamClass": "gen5n_high",
        "DefaultApplicationIdentifier": application_arn,
        "LocationConfigurations": [{"LocationName": REGION, "AlwaysOnCapacity": 1}],
    });
    if !tags.is_empty() {
        group["Tags"] = tag_map(tags);
    }
    let (status, created) = call(router, "POST", "/streamgroups", Some(group)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateStreamGroup 201, got {status} {created}"
    );
    text(&created, "/Arn", case)
}

/// One row of the removal table: what the resource carries, which keys one
/// `UntagResource` call names, and what must be left afterwards.
struct RemovalCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    remove: Vec<String>,
    expected: Vec<(String, String)>,
}

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn removal_table() -> Vec<RemovalCase> {
    // `TagKey` is modelled `length 1..128`, so this is the longest key the model
    // allows; `TagKeyList` is modelled `length 0..50`, so a call may name up to 50.
    let longest_key = "k".repeat(128);
    // A key that is not URL-safe: `TagKey` carries no pattern, so a `/` is a legal
    // key character and reaches the service percent-encoded.
    let slashed_key = "ns/sub".to_string();

    let held_fifty: Vec<(String, String)> = (0..TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let all_fifty: Vec<String> = held_fifty.iter().map(|(key, _)| key.clone()).collect();

    vec![
        RemovalCase {
            name: "the only key a resource carries",
            initial: pairs(&[("env", "dev")]),
            remove: keys(&["env"]),
            expected: Vec::new(),
        },
        RemovalCase {
            name: "one key of several, leaving the rest",
            initial: pairs(&[("env", "dev"), ("team", "red"), ("owner", "ada")]),
            remove: keys(&["team"]),
            expected: pairs(&[("env", "dev"), ("owner", "ada")]),
        },
        RemovalCase {
            name: "several keys in one call",
            initial: pairs(&[("env", "dev"), ("team", "red"), ("owner", "ada")]),
            remove: keys(&["env", "owner"]),
            expected: pairs(&[("team", "red")]),
        },
        RemovalCase {
            name: "a key the resource does not carry removes nothing",
            initial: pairs(&[("env", "dev")]),
            remove: keys(&["team"]),
            expected: pairs(&[("env", "dev")]),
        },
        RemovalCase {
            name: "a mix of held and unheld keys removes only the held ones",
            initial: pairs(&[("env", "dev"), ("team", "red")]),
            remove: keys(&["team", "nosuchkey"]),
            expected: pairs(&[("env", "dev")]),
        },
        RemovalCase {
            name: "a key whose value is empty is still removed by its key",
            initial: pairs(&[("env", ""), ("team", "red")]),
            remove: keys(&["env"]),
            expected: pairs(&[("team", "red")]),
        },
        RemovalCase {
            name: "the longest key the model allows",
            initial: vec![
                (longest_key.clone(), "v".repeat(256)),
                ("env".to_string(), "dev".to_string()),
            ],
            remove: vec![longest_key],
            expected: pairs(&[("env", "dev")]),
        },
        RemovalCase {
            name: "a key that needs percent-encoding on the wire",
            initial: vec![
                (slashed_key.clone(), "shared".to_string()),
                ("env".to_string(), "dev".to_string()),
            ],
            remove: vec![slashed_key],
            expected: pairs(&[("env", "dev")]),
        },
        RemovalCase {
            name: "every key of a resource at the tag bound, in one call",
            initial: held_fifty,
            remove: all_fifty,
            expected: Vec::new(),
        },
    ]
}

/// Whatever the resource carries, one `UntagResource` call removes exactly the keys
/// it names and leaves every other key — on each resource type this service tags.
#[tokio::test]
async fn untagging_removes_exactly_the_keys_it_names() {
    let router = emulator();
    for kind in TAGGABLE {
        for (index, case) in removal_table().into_iter().enumerate() {
            let label = format!("{kind} / {}", case.name);
            // The resource's own `Description` is modelled
            // `pattern ^[a-zA-Z0-9-_.!+@/][a-zA-Z0-9-_.!+@/ ]*$`, so the row's index
            // names it rather than the prose label.
            let arn = taggable_resource(
                &router,
                kind,
                &format!("untag-case-{kind}-{index}"),
                &case.initial,
            )
            .await;

            let (status, body) = untag(&router, &arn, &case.remove).await;
            assert_eq!(
                status, UNTAG_RESOURCE_SUCCESS,
                "case {label:?}: expected UntagResource {UNTAG_RESOURCE_SUCCESS}, got {status} \
                 {body}"
            );

            let expected = tag_map(&case.expected);
            let got = stored_tags(&router, &arn, &label).await;
            assert_eq!(
                got, expected,
                "case {label:?}: expected the remaining tags {expected}, got {got}"
            );

            // `UntagResource` is modelled `@idempotent`: the same call again
            // removes nothing more and still succeeds.
            let (status, body) = untag(&router, &arn, &case.remove).await;
            assert_eq!(
                status, UNTAG_RESOURCE_SUCCESS,
                "case {label:?}: expected the repeated UntagResource \
                 {UNTAG_RESOURCE_SUCCESS}, got {status} {body}"
            );
            let again = stored_tags(&router, &arn, &label).await;
            assert_eq!(
                again, expected,
                "case {label:?}: a repeated removal must leave the same tags {expected}, got \
                 {again}"
            );
        }
    }
}

/// A removal is scoped to the one resource it names: two resources carrying the
/// same keys keep their own sets.
#[tokio::test]
async fn untagging_one_resource_leaves_every_other_resource_alone() {
    let router = emulator();
    let shared = pairs(&[("env", "dev"), ("team", "red")]);
    let application = taggable_resource(&router, "application", "untag-scope-app", &shared).await;
    let group = taggable_resource(&router, "streamgroup", "untag-scope-group", &shared).await;

    let (status, body) = untag(&router, &application, &keys(&["env"])).await;
    assert_eq!(
        status, UNTAG_RESOURCE_SUCCESS,
        "expected UntagResource {UNTAG_RESOURCE_SUCCESS} on the application, got {status} {body}"
    );

    let got = stored_tags(&router, &application, "the untagged application").await;
    assert_eq!(
        got,
        json!({"team": "red"}),
        "the named resource loses only the named key, got {got}"
    );
    let got = stored_tags(&router, &group, "the untouched stream group").await;
    assert_eq!(
        got,
        tag_map(&shared),
        "an untouched resource keeps every key, got {got}"
    );
}

/// Removing a key and assigning it again compose: `UntagResource` deletes the
/// mapping rather than blanking it, so a later `TagResource` of the same key stores
/// the new value and `ListTagsForResource` reports it.
#[tokio::test]
async fn a_removed_key_can_be_assigned_again() {
    let router = emulator();
    let arn = taggable_resource(
        &router,
        "application",
        "untag-then-tag",
        &pairs(&[("env", "dev"), ("team", "red")]),
    )
    .await;

    assert_eq!(
        untag(&router, &arn, &keys(&["env"])).await.0,
        UNTAG_RESOURCE_SUCCESS,
        "the removal must succeed"
    );
    let got = stored_tags(&router, &arn, "after the removal").await;
    assert_eq!(
        got,
        json!({"team": "red"}),
        "the removed key must be gone, not blank: got {got}"
    );

    assert_eq!(
        tag(&router, &arn, &pairs(&[("env", "prod")])).await.0,
        204,
        "the re-tagging must succeed"
    );
    let got = stored_tags(&router, &arn, "after the re-tagging").await;
    assert_eq!(
        got,
        json!({"env": "prod", "team": "red"}),
        "a removed key takes the value it is next assigned, got {got}"
    );
}

/// Untagging does not change the resource itself: a `Get*` of a resource whose only
/// change was a tag removal still reports the instant its state last moved.
#[tokio::test]
async fn untagging_does_not_move_the_resource_s_last_updated_instant() {
    let router = emulator();
    let arn = taggable_resource(
        &router,
        "application",
        "untag-last-updated",
        &pairs(&[("env", "dev")]),
    )
    .await;
    // Past the transition the create owes, so the only change left that could move
    // `LastUpdatedAt` is the removal itself.
    std::thread::sleep(PAST_TRANSITION);

    let before = stored_tags(&router, &arn, "before").await;
    assert_eq!(
        before,
        json!({"env": "dev"}),
        "the resource must start out tagged, got {before}"
    );
    let id = arn.rsplit('/').next().unwrap_or_default().to_string();
    let (status, described) = call(&router, "GET", &format!("/applications/{id}"), None).await;
    assert_eq!(
        status, 200,
        "expected GetApplication 200, got {status} {described}"
    );
    let last_updated = described["LastUpdatedAt"].clone();

    assert_eq!(
        untag(&router, &arn, &keys(&["env"])).await.0,
        UNTAG_RESOURCE_SUCCESS,
        "the removal must succeed"
    );
    let (status, after) = call(&router, "GET", &format!("/applications/{id}"), None).await;
    assert_eq!(
        status, 200,
        "expected GetApplication 200 after the removal, got {status} {after}"
    );
    assert_eq!(
        after["LastUpdatedAt"], last_updated,
        "a tag removal is not a change to the resource: expected LastUpdatedAt \
         {last_updated}, got {after}"
    );
}

/// A `ResourceArn` that names no taggable resource this service holds is refused
/// with the modelled `ValidationException` (400) — `UntagResource` models no
/// `ResourceNotFoundException` — never answered with an empty success.
#[tokio::test]
async fn untagging_refuses_an_arn_that_names_no_taggable_resource() {
    let router = emulator();

    // A real stream session: an ARN of this service whose resource type is not one
    // of the two this API tags.
    let (application_id, application_arn) = ready_application(&router, "untag-untaggable").await;
    let (group_id, _) =
        active_stream_group(&router, "untag-untaggable-group", &application_id, &[REGION]).await;
    let (status, session) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/streamsessions"),
        Some(json!({
            "Protocol": "WebRTC",
            "SignalRequest": "offer",
            "ApplicationIdentifier": application_arn,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected StartStreamSession 201, got {status} {session}"
    );
    let session_arn = text(&session, "/Arn", "session");

    // An application whose record is gone: deleted, and the clock has since
    // completed the deletion, so there is nothing left to untag.
    let (deleted_id, deleted_arn) = ready_application(&router, "untag-deleted").await;
    let (status, body) =
        call(&router, "DELETE", &format!("/applications/{deleted_id}"), None).await;
    assert_eq!(
        status, 204,
        "expected DeleteApplication 204, got {status} {body}"
    );
    std::thread::sleep(PAST_TRANSITION);

    let prefix = format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}");
    let cases: Vec<(&str, String)> = vec![
        ("an application id nothing holds", format!("{prefix}:application/a-nosuchapp")),
        (
            "a stream group id nothing holds",
            format!("{prefix}:streamgroup/sg-nosuchgroup"),
        ),
        ("a stream session, which is not taggable", session_arn),
        (
            "a resource type this service does not tag",
            format!("{prefix}:streamurl/su-whatever"),
        ),
        ("a deleted application", deleted_arn),
    ];
    for (case, arn) in cases {
        let (status, body) = untag(&router, &arn, &keys(&["env"])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected UntagResource 400 for {arn}, got {status} {body}"
        );
        assert_eq!(
            body["__type"]
                .as_str()
                .or_else(|| body["Code"].as_str())
                .unwrap_or_default()
                .rsplit('#')
                .next()
                .unwrap_or_default(),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
    }
}
