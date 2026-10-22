// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` over this service's taggable resources, end to end
//! through the same router the emulator serves.
//!
//! The rule under test: `ListTagsForResource` reports exactly the tag set the
//! named resource carries in state — for every taggable resource type, for any
//! tag set the model admits — and reports the `Tags` member absent when the
//! resource carries none; a `ResourceArn` that names no taggable resource the
//! service holds is refused with the modelled client-input error rather than
//! answered with an empty success.
//!
//! The recorded case is one row of the tables below.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::stream_workflow::{
    application_body, as_label, call, emulator, text, ACCOUNT, PAST_TRANSITION, REGION,
};

/// The resource types tags can be assigned to: "You can assign tags to the
/// following Amazon GameLift Streams resource types: Application StreamGroup"
/// (`TagResource`'s model documentation). A stream session is NOT one of them.
#[derive(Debug, Clone, Copy)]
enum Taggable {
    Application,
    StreamGroup,
}

impl Taggable {
    fn case(self) -> &'static str {
        match self {
            Taggable::Application => "Application",
            Taggable::StreamGroup => "StreamGroup",
        }
    }
}

/// Create one taggable resource carrying `tags`, and answer its ARN.
///
/// `description` is what the resource is created with (`Description` is modelled
/// `pattern ^[a-zA-Z0-9-_.!+@/][a-zA-Z0-9-_.!+@/ ]*$`, which a prose case label
/// does not satisfy); `case` only names the row in the assertions.
async fn with_tags(
    router: &MultiServiceRouter,
    resource: Taggable,
    case: &str,
    description: &str,
    tags: &BTreeMap<String, String>,
) -> String {
    let (path, mut body) = match resource {
        Taggable::Application => ("/applications", application_body(description)),
        Taggable::StreamGroup => (
            "/streamgroups",
            json!({
                "Description": description,
                "StreamClass": "gen5n_high",
                "LocationConfigurations": [{"LocationName": REGION, "AlwaysOnCapacity": 1}],
            }),
        ),
    };
    // `Tags` is modelled `length 1..50`, so an untagged create omits the member
    // rather than sending an empty map.
    if !tags.is_empty() {
        body["Tags"] = tags_value(tags);
    }
    let (status, created) = call(router, "POST", path, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected the {} create to answer 201, got {status} {created}",
        resource.case()
    );
    text(&created, "/Arn", case)
}

/// The tag map as the wire carries it.
fn tags_value(tags: &BTreeMap<String, String>) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<Map<String, Value>>(),
    )
}

/// `GET /tags/{ResourceArn}`.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags/{}", as_label(arn)), None).await
}

/// One row of the tag table: the set a resource is created with, which is exactly
/// what `ListTagsForResource` must report back.
fn tag_table() -> Vec<(&'static str, BTreeMap<String, String>)> {
    // `TagKey` is modelled `length 1..128`, `TagValue` `length 0..256` and `Tags`
    // itself `length 1..50`, so these rows span the whole admitted class: none,
    // one, several (including an empty value), the longest key and value, and the
    // largest map.
    let rows: Vec<(&'static str, Vec<(String, String)>)> = vec![
        ("no tags at all", Vec::new()),
        ("a single tag", vec![("env".to_string(), "test".to_string())]),
        (
            "several tags, one of them with an empty value",
            vec![
                ("env".to_string(), "prod".to_string()),
                ("owner".to_string(), String::new()),
                ("Cost-Center".to_string(), "cc/42 +team".to_string()),
            ],
        ),
        (
            "a key and a value at their modelled maximum length",
            vec![("k".repeat(128), "v".repeat(256))],
        ),
        (
            "the modelled maximum of 50 tags",
            (0..50)
                .map(|index| (format!("key-{index}"), format!("value-{index}")))
                .collect(),
        ),
    ];
    rows.into_iter()
        .map(|(case, pairs)| (case, pairs.into_iter().collect()))
        .collect()
}

/// Whatever tag set a taggable resource was created with, `ListTagsForResource`
/// reports exactly that set back — and nothing when it carries none.
#[tokio::test]
async fn listing_tags_reports_the_set_the_resource_carries() {
    let router = emulator();
    for (row, (case, tags)) in tag_table().into_iter().enumerate() {
        for resource in [Taggable::Application, Taggable::StreamGroup] {
            let case = format!("{case} on a {}", resource.case());
            let description = format!("tag-row-{row}-{}", resource.case());
            let arn = with_tags(&router, resource, &case, &description, &tags).await;
            let (status, listed) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected ListTagsForResource 200, got {status} {listed}"
            );
            if tags.is_empty() {
                assert_eq!(
                    listed.get("Tags"),
                    None,
                    "case {case:?}: a resource carrying no tags reports no Tags member, \
                     got {listed}"
                );
            } else {
                assert_eq!(
                    listed["Tags"],
                    tags_value(&tags),
                    "case {case:?}: expected the stored tags {}, got {listed}",
                    tags_value(&tags)
                );
            }

            // A read reports, it does not change: the second read agrees with the
            // first.
            let (status, again) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the repeated ListTagsForResource 200, got \
                 {status} {again}"
            );
            assert_eq!(
                again, listed,
                "case {case:?}: two reads of the same resource must agree, got {again} \
                 after {listed}"
            );
        }
    }
}

/// A `ResourceArn` that names no taggable resource this service holds is refused
/// with the modelled `ValidationException` (400) — the operation models no
/// `ResourceNotFoundException` — never answered with an empty success.
#[tokio::test]
async fn listing_tags_for_a_resource_that_is_not_there_is_refused() {
    let router = emulator();
    let tags: BTreeMap<String, String> =
        [("env".to_string(), "test".to_string())].into_iter().collect();
    let app_arn =
        with_tags(&router, Taggable::Application, "present-app", "present-app", &tags).await;
    let group_arn = with_tags(
        &router,
        Taggable::StreamGroup,
        "present-group",
        "present-group",
        &tags,
    )
    .await;
    let app_id = app_arn.rsplit('/').next().unwrap_or_default().to_string();
    let group_id = group_arn.rsplit('/').next().unwrap_or_default().to_string();

    let prefix = format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}");
    let cases: Vec<(&str, String)> = vec![
        ("an application id nothing holds", format!("{prefix}:application/a-nosuch")),
        ("a stream group id nothing holds", format!("{prefix}:streamgroup/sg-nosuch")),
        // The resource TYPE is part of the ARN: an application's id under the
        // stream group type names nothing, and the other way round.
        (
            "an application id under the stream group type",
            format!("{prefix}:streamgroup/{app_id}"),
        ),
        (
            "a stream group id under the application type",
            format!("{prefix}:application/{group_id}"),
        ),
        // A stream session is not a taggable resource type.
        (
            "a stream session ARN",
            format!("{prefix}:streamsession/{group_id}/ss-1"),
        ),
        ("a resource type this service does not name", format!("{prefix}:widget/w-1")),
    ];
    for (case, arn) in cases {
        let (status, got) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 ValidationException for {arn}, got {status} {got}"
        );
    }

    // And once a resource is gone, so are its tags.
    std::thread::sleep(PAST_TRANSITION);
    let (status, got) = call(&router, "DELETE", &format!("/applications/{app_id}"), None).await;
    assert_eq!(
        status, 204,
        "DeleteApplication: expected 204, got {status} {got}"
    );
    std::thread::sleep(PAST_TRANSITION);
    let (status, got) = list_tags(&router, &app_arn).await;
    assert_eq!(
        status, 400,
        "case \"a deleted application\": expected 400 ValidationException, got {status} {got}"
    );
}
