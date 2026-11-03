// SPDX-License-Identifier: CC0-1.0

//! Tagging the GameLift Streams resources over the wire: `TagResource` MERGES the
//! key-value pairs it is given into the tags the named resource already carries, and
//! `ListTagsForResource` reports exactly what is stored.
//!
//! The rule under test is one sentence: a tagging call assigns the pairs it names and
//! leaves every other stored key as it was, for each of the resource types this
//! service lets a caller tag. The tables below span that class — the number of pairs,
//! whether a key is new or already held, the value (including the shortest and
//! longest the model allows), and the resource type — rather than one recorded case.
//!
//! Reporting a stored set back is `ListTagsForResource`'s own suite
//! (`tests/all/resource_tags.rs`); this one is about what a tagging call WRITES,
//! read back through that reporter.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_gameliftstreams_gameliftstreams::state::TAGS_PER_RESOURCE;

use super::stream_workflow::{
    active_stream_group, application_body, as_label, call, emulator, ready_application, text,
    ACCOUNT, PAST_TRANSITION, REGION,
};

/// The resource types `TagResource` documents: "You can assign tags to the following
/// Amazon GameLift Streams resource types: Application StreamGroup".
const TAGGABLE: [&str; 2] = ["application", "streamgroup"];

/// The status a successful `TagResource` answers with: the write has nothing to
/// report, and this API answers its bodiless writes with 204 No Content.
const TAG_RESOURCE_SUCCESS: u16 = 204;

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

/// Create one taggable resource of `kind`, carrying `tags` from birth, and answer its
/// ARN. A stream group needs an application to host, so the `streamgroup` rows create
/// one and tag only the group.
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
        "LocationConfigurations": [{"LocationName": "us-west-2", "AlwaysOnCapacity": 1}],
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

/// One row of the merge table: what the resource is created with, what one
/// `TagResource` call assigns, and the map that must be stored afterwards.
struct MergeCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    assign: Vec<(String, String)>,
    expected: Vec<(String, String)>,
}

#[tokio::test]
async fn tagging_merges_its_pairs_into_the_tags_a_resource_already_carries() {
    let router = emulator();

    // `TagKey` is modelled `length 1..128` and `TagValue` `length 0..256`, so these
    // are the longest key and value and the shortest value the model allows.
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);

    // A single call carrying many pairs: `Tags` is modelled `length 1..50`.
    let many: Vec<(String, String)> = (0..20)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let mut many_expected = pairs(&[("env", "dev")]);
    many_expected.extend(many.clone());

    let cases = [
        MergeCase {
            name: "a first tag on an untagged resource",
            initial: Vec::new(),
            assign: pairs(&[("env", "dev")]),
            expected: pairs(&[("env", "dev")]),
        },
        MergeCase {
            name: "a new key joins the keys already held",
            initial: pairs(&[("env", "dev")]),
            assign: pairs(&[("team", "red")]),
            expected: pairs(&[("env", "dev"), ("team", "red")]),
        },
        MergeCase {
            name: "a key already held takes the new value",
            initial: pairs(&[("env", "dev")]),
            assign: pairs(&[("env", "prod")]),
            expected: pairs(&[("env", "prod")]),
        },
        MergeCase {
            name: "a partial overlap replaces only the keys it names",
            initial: pairs(&[("env", "dev"), ("team", "red")]),
            assign: pairs(&[("env", "prod"), ("owner", "ada")]),
            expected: pairs(&[("env", "prod"), ("team", "red"), ("owner", "ada")]),
        },
        MergeCase {
            name: "an empty value is a value, not a removal",
            initial: pairs(&[("env", "dev"), ("team", "red")]),
            assign: pairs(&[("env", "")]),
            expected: pairs(&[("env", ""), ("team", "red")]),
        },
        MergeCase {
            name: "the longest key and value the model allows",
            initial: pairs(&[("env", "dev")]),
            assign: vec![(longest_key.clone(), longest_value.clone())],
            expected: vec![
                ("env".to_string(), "dev".to_string()),
                (longest_key, longest_value),
            ],
        },
        MergeCase {
            name: "many pairs in one call",
            initial: pairs(&[("env", "dev")]),
            assign: many,
            expected: many_expected,
        },
    ];

    for kind in TAGGABLE {
        for (index, case) in cases.iter().enumerate() {
            let label = format!("{kind} / {}", case.name);
            // The resource's own `Description` is modelled
            // `pattern ^[a-zA-Z0-9-_.!+@/][a-zA-Z0-9-_.!+@/ ]*$`, so the row's index
            // names it rather than the prose label.
            let arn =
                taggable_resource(&router, kind, &format!("tag-case-{kind}-{index}"), &case.initial)
                    .await;

            let (status, body) = tag(&router, &arn, &case.assign).await;
            assert_eq!(
                status, TAG_RESOURCE_SUCCESS,
                "case {label:?}: expected TagResource {TAG_RESOURCE_SUCCESS}, got {status} {body}"
            );

            let (status, got) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected ListTagsForResource 200, got {status} {got}"
            );
            assert_eq!(
                got["Tags"],
                tag_map(&case.expected),
                "case {label:?}: expected the merged tags {}, got {got}",
                tag_map(&case.expected)
            );
        }
    }
}

#[tokio::test]
async fn concurrent_taggings_of_one_resource_lose_no_update() {
    let router = emulator();
    let arn = taggable_resource(&router, "application", "no-lost-updates", &[]).await;

    // Six taggings in flight at once, each naming a key of its own: every one of them
    // must survive, which a handler that REPLACED the stored map could not manage.
    let keys: Vec<Vec<(String, String)>> = (0..6)
        .map(|index| vec![(format!("key-{index}"), format!("value-{index}"))])
        .collect();
    let (a, b, c, d, e, f) = tokio::join!(
        tag(&router, &arn, &keys[0]),
        tag(&router, &arn, &keys[1]),
        tag(&router, &arn, &keys[2]),
        tag(&router, &arn, &keys[3]),
        tag(&router, &arn, &keys[4]),
        tag(&router, &arn, &keys[5]),
    );
    for (index, (status, body)) in [a, b, c, d, e, f].into_iter().enumerate() {
        assert_eq!(
            status, TAG_RESOURCE_SUCCESS,
            "case {index}: expected a concurrent TagResource {TAG_RESOURCE_SUCCESS}, \
             got {status} {body}"
        );
    }

    let expected: Vec<(String, String)> = keys.into_iter().flatten().collect();
    let (status, got) = list_tags(&router, &arn).await;
    assert_eq!(
        status, 200,
        "expected ListTagsForResource 200, got {status} {got}"
    );
    assert_eq!(
        got["Tags"],
        tag_map(&expected),
        "expected every concurrent tagging to survive: {}, got {got}",
        tag_map(&expected)
    );
}

#[tokio::test]
async fn tags_belong_to_one_resource_each() {
    let router = emulator();
    let application = taggable_resource(&router, "application", "own-tags-app", &[]).await;
    let group = taggable_resource(&router, "streamgroup", "own-tags-group", &[]).await;

    assert_eq!(
        tag(&router, &application, &pairs(&[("owner", "app")])).await.0,
        TAG_RESOURCE_SUCCESS,
        "tagging the application must succeed"
    );
    assert_eq!(
        tag(&router, &group, &pairs(&[("owner", "group")])).await.0,
        TAG_RESOURCE_SUCCESS,
        "tagging the stream group must succeed"
    );

    for (case, arn, expected) in [
        ("application", &application, "app"),
        ("stream group", &group, "group"),
    ] {
        let (status, got) = list_tags(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTagsForResource 200, got {status} {got}"
        );
        assert_eq!(
            got["Tags"],
            json!({"owner": expected}),
            "case {case:?}: a resource reports only its OWN tags, got {got}"
        );
    }
}

#[tokio::test]
async fn a_resource_holds_no_more_than_the_published_number_of_tags() {
    let router = emulator();
    let arn = taggable_resource(&router, "application", "tag-bound", &[]).await;

    // Exactly the bound, in one call: `Tags` is modelled `length 1..50` and the
    // General Reference's tag restrictions allow 50 tags per resource.
    let full: Vec<(String, String)> = (0..TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let (status, body) = tag(&router, &arn, &full).await;
    assert_eq!(
        status, TAG_RESOURCE_SUCCESS,
        "case \"at the bound\": expected TagResource {TAG_RESOURCE_SUCCESS} for \
         {TAGS_PER_RESOURCE} tags, got {status} {body}"
    );

    // One key past the bound is refused, and nothing it carried is stored.
    let (status, body) = tag(&router, &arn, &pairs(&[("one-too-many", "x")])).await;
    assert_eq!(
        status, 400,
        "case \"one past the bound\": expected 400, got {status} {body}"
    );

    // A key the resource already holds does not grow the map, so it is still accepted
    // at the bound.
    let (status, body) = tag(&router, &arn, &pairs(&[("key-0", "replaced")])).await;
    assert_eq!(
        status, TAG_RESOURCE_SUCCESS,
        "case \"replacing a held key at the bound\": expected {TAG_RESOURCE_SUCCESS}, \
         got {status} {body}"
    );

    let (status, got) = list_tags(&router, &arn).await;
    assert_eq!(
        status, 200,
        "expected ListTagsForResource 200, got {status} {got}"
    );
    let tags = got["Tags"]
        .as_object()
        .unwrap_or_else(|| panic!("expected a Tags map, got {got}"));
    assert_eq!(
        tags.len(),
        TAGS_PER_RESOURCE,
        "expected the store to hold exactly {TAGS_PER_RESOURCE} tags, got {got}"
    );
    assert_eq!(
        tags["key-0"], "replaced",
        "the accepted replacement must be stored, got {got}"
    );
    assert!(
        !tags.contains_key("one-too-many"),
        "the refused call must store nothing, got {got}"
    );
}

#[tokio::test]
async fn tagging_refuses_an_arn_that_names_no_taggable_resource() {
    let router = emulator();

    // A real stream session: an ARN of this service whose resource type is not one of
    // the two `TagResource` documents.
    let (application_id, application_arn) = ready_application(&router, "untaggable").await;
    let (group_id, _) = active_stream_group(
        &router,
        "untaggable-group",
        &application_id,
        &["us-west-2"],
    )
    .await;
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

    // An application whose record is gone: deleted, and the clock has since completed
    // the deletion, so there is nothing left to tag.
    let (deleted_id, deleted) = ready_application(&router, "deleted").await;
    let (status, body) =
        call(&router, "DELETE", &format!("/applications/{deleted_id}"), None).await;
    assert_eq!(
        status, 204,
        "expected DeleteApplication 204, got {status} {body}"
    );
    std::thread::sleep(PAST_TRANSITION);

    let cases: Vec<(&str, String)> = vec![
        (
            "an unknown application",
            format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:application/a-nosuchapp"),
        ),
        (
            "an unknown stream group",
            format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamgroup/sg-nosuchgroup"),
        ),
        ("a stream session, which is not taggable", session_arn),
        (
            "a resource type this service does not tag",
            format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamurl/su-whatever"),
        ),
        ("a deleted application", deleted),
    ];
    for (case, arn) in cases {
        let (status, body) = tag(&router, &arn, &pairs(&[("env", "dev")])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected TagResource 400 for {arn}, got {status} {body}"
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

        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected ListTagsForResource 400 for {arn}, got {status} {body}"
        );
    }
}
