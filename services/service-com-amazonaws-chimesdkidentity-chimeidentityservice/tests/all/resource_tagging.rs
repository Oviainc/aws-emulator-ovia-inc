// SPDX-License-Identifier: CC0-1.0

//! The generic tagging surface of this service, end to end over HTTP.
//!
//! The RULE pinned here: a resource's tags are state. `TagResource` MERGES the pairs it
//! carries into what the resource already has (a repeated key is re-valued, an untouched
//! one kept), `UntagResource` removes exactly the keys it names and nothing else, and
//! `ListTagsForResource` reports back the tags those two and the create left — an empty
//! list for a resource carrying none. A tagging call keyed by a resource of this service
//! that does not exist is refused with the modelled client error carrying the `NotFound`
//! code, and one that would take a resource past the modelled maximum number of tags with
//! the modelled resource-limit error.
//!
//! The tables span what the rule generalises over: how many pairs one call carries (one,
//! several, the modelled maximum, one past it), the form of a key and a value (short,
//! non-ASCII, at the modelled length bounds), which resource is tagged (an `AppInstance`,
//! an `AppInstanceUser`, each tagged at CREATE time and afterwards), how many keys an
//! untag names (one, several, one the resource never carried), and the order the calls
//! arrive in.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::app_instance_lifecycle::{uri_encode, Emulator};

/// `POST /tags?operation=tag-resource` with the pairs this call applies.
async fn tag(emulator: &Emulator, arn: &str, pairs: &[(String, String)]) -> (u16, Value) {
    let tags: Vec<Value> = pairs
        .iter()
        .map(|(key, value)| json!({ "Key": key, "Value": value }))
        .collect();
    emulator
        .call(
            "POST",
            "/tags?operation=tag-resource",
            Some(json!({ "ResourceARN": arn, "Tags": tags })),
        )
        .await
}

/// `POST /tags?operation=untag-resource` with the keys this call removes.
async fn untag(emulator: &Emulator, arn: &str, tag_keys: &[String]) -> (u16, Value) {
    emulator
        .call(
            "POST",
            "/tags?operation=untag-resource",
            Some(json!({ "ResourceARN": arn, "TagKeys": tag_keys })),
        )
        .await
}

/// `GET /tags?arn=…`, as the map a client reads the answer as.
async fn list(emulator: &Emulator, arn: &str) -> (u16, BTreeMap<String, String>) {
    let (status, body) = emulator
        .call("GET", &format!("/tags?arn={}", uri_encode(arn)), None)
        .await;
    let tags = body["Tags"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|tag| {
                    Some((
                        tag["Key"].as_str()?.to_string(),
                        tag["Value"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    (status, tags)
}

/// Owned pairs, so a row can carry a value it builds (a key at the modelled length
/// bound) beside the literals.
fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn as_map(pairs: &[(String, String)]) -> BTreeMap<String, String> {
    pairs.iter().cloned().collect()
}

/// One row of the round-trip table: the pairs each successive tagging call applies, the
/// keys untagged afterwards, and the tags the resource is left with.
struct TagCase {
    case: &'static str,
    calls: Vec<Vec<(String, String)>>,
    removed: Vec<String>,
    left: Vec<(String, String)>,
}

/// RULE: tagging merges, untagging removes only the keys it names, and the listing
/// reports exactly the tags the writes left.
#[tokio::test]
async fn tags_round_trip_through_the_shared_tagging_routes() {
    let emulator = Emulator::new();

    // The pairs below stay inside the model's own bounds: `TagKey` is len 1..128 and
    // `TagValue` len 1..256 (`smithy_query(query="shape",shape="Tag")`).
    let long_key = "k".repeat(128);
    let long_value = "v".repeat(256);
    let at_bounds = owned(&[
        (long_key.as_str(), long_value.as_str()),
        ("équipe", "ノート"),
    ]);

    let rows: Vec<TagCase> = vec![
        TagCase {
            case: "one pair, kept",
            calls: vec![owned(&[("env", "dev")])],
            removed: Vec::new(),
            left: owned(&[("env", "dev")]),
        },
        TagCase {
            case: "several pairs in one call, one of them removed again",
            calls: vec![owned(&[("env", "dev"), ("team", "core"), ("tier", "gold")])],
            removed: keys(&["team"]),
            left: owned(&[("env", "dev"), ("tier", "gold")]),
        },
        TagCase {
            case: "two calls merging, the repeated key re-valued",
            calls: vec![
                owned(&[("env", "dev"), ("team", "core")]),
                owned(&[("env", "prod"), ("tier", "gold")]),
            ],
            removed: Vec::new(),
            left: owned(&[("env", "prod"), ("team", "core"), ("tier", "gold")]),
        },
        TagCase {
            case: "a key the resource never carried is removed as a no-op",
            calls: vec![owned(&[("env", "dev")])],
            removed: keys(&["absent", "neither"]),
            left: owned(&[("env", "dev")]),
        },
        TagCase {
            case: "every key removed, leaving no tags at all",
            calls: vec![owned(&[("env", "dev"), ("team", "core")])],
            removed: keys(&["env", "team"]),
            left: Vec::new(),
        },
        TagCase {
            case: "keys and values at the modelled length bounds, and non-ASCII text",
            calls: vec![at_bounds.clone()],
            removed: Vec::new(),
            left: at_bounds,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let TagCase {
            case,
            calls,
            removed,
            left,
        } = row;
        // Both resource kinds this service mints, so the rule is pinned over the resource
        // as well as over the tags.
        let instance = emulator
            .app_instance("tagging", &format!("tok-tag-{index}"))
            .await;
        let user = emulator
            .user(&instance, &format!("u{index}"), &format!("tok-tag-{index}-u"))
            .await;

        for arn in [instance.as_str(), user.as_str()] {
            let (status, tags) = list(&emulator, arn).await;
            assert_eq!(
                (status, tags.len()),
                (200, 0),
                "case {case:?} ({arn}): an untagged resource reports no tags, got {tags:?}"
            );

            for pairs in &calls {
                let (status, body) = tag(&emulator, arn, pairs).await;
                assert_eq!(
                    status, 204,
                    "case {case:?} ({arn}): expected TagResource to answer 204, got \
                     {status} {body:?}"
                );
            }
            if !removed.is_empty() {
                let (status, body) = untag(&emulator, arn, &removed).await;
                assert_eq!(
                    status, 204,
                    "case {case:?} ({arn}): expected UntagResource to answer 204, got \
                     {status} {body:?}"
                );
            }

            let (status, tags) = list(&emulator, arn).await;
            assert_eq!(
                (status, &tags),
                (200, &as_map(&left)),
                "case {case:?} ({arn}): expected exactly the tags the writes left"
            );

            // A read does not change what the next reader sees.
            let (status, again) = list(&emulator, arn).await;
            assert_eq!(
                (status, &again),
                (200, &tags),
                "case {case:?} ({arn}): a second listing differed from the first"
            );
        }
    }
}

/// One row of the create-time table: the tags the create carries, what the later calls do
/// to them, and the tags the resource is left with.
struct CreateTagCase {
    case: &'static str,
    created_with: Vec<(String, String)>,
    then_tagged: Vec<(String, String)>,
    untagged: Vec<String>,
    left: Vec<(String, String)>,
}

/// RULE: tags given at CREATE time are the same state the tagging routes read and write,
/// so a created resource lists them, the tagging routes change them, and deleting the
/// resource takes them with it.
#[tokio::test]
async fn tags_supplied_at_create_time_are_listed_and_can_be_changed() {
    let emulator = Emulator::new();

    // Rows differ in how many tags the create carries and which of them the later calls
    // touch.
    let rows: Vec<CreateTagCase> = vec![
        CreateTagCase {
            case: "one create-time tag, left alone",
            created_with: owned(&[("env", "dev")]),
            then_tagged: Vec::new(),
            untagged: Vec::new(),
            left: owned(&[("env", "dev")]),
        },
        CreateTagCase {
            case: "three create-time tags, one re-valued and one dropped",
            created_with: owned(&[("env", "dev"), ("team", "core"), ("tier", "gold")]),
            then_tagged: owned(&[("env", "prod")]),
            untagged: keys(&["tier"]),
            left: owned(&[("env", "prod"), ("team", "core")]),
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let CreateTagCase {
            case,
            created_with,
            then_tagged,
            untagged,
            left,
        } = row;
        let tags: Vec<Value> = created_with
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect();
        let (status, body) = emulator
            .call(
                "POST",
                "/app-instances",
                Some(json!({
                    "Name": "create-tagged",
                    "ClientRequestToken": format!("tok-create-tag-{index}"),
                    "Tags": tags,
                })),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateAppInstance to answer 201, got {status} {body:?}"
        );
        let arn = body["AppInstanceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateAppInstance returned no ARN: {body:?}"))
            .to_string();

        let (status, listed) = list(&emulator, &arn).await;
        assert_eq!(
            (status, &listed),
            (200, &as_map(&created_with)),
            "case {case:?}: expected the create-time tags to be listed"
        );

        if !then_tagged.is_empty() {
            tag(&emulator, &arn, &then_tagged).await;
        }
        if !untagged.is_empty() {
            untag(&emulator, &arn, &untagged).await;
        }

        let (status, listed) = list(&emulator, &arn).await;
        assert_eq!(
            (status, &listed),
            (200, &as_map(&left)),
            "case {case:?}: expected the create-time tags as the later calls left them"
        );

        // The resource's tags go with the resource: once the instance is deleted, the ARN
        // is one this caller may no longer address and the listing refuses it with the
        // modelled permanent refusal.
        let (status, _) = emulator
            .call(
                "DELETE",
                &format!("/app-instances/{}", uri_encode(&arn)),
                None,
            )
            .await;
        assert_eq!(status, 204, "case {case:?}: expected the delete to answer 204");
        let (status, body) = emulator
            .call("GET", &format!("/tags?arn={}", uri_encode(&arn)), None)
            .await;
        assert_eq!(
            (status, body["Code"].as_str()),
            (403, Some("Forbidden")),
            "case {case:?}: expected a deleted resource's tags to be refused, got {body:?}"
        );
    }
}

/// RULE: a tagging call that names a resource of this service that does not exist is
/// refused with the `NotFound` code and writes nothing, and a call past the modelled
/// maximum number of tags per resource is refused with the modelled resource-limit error,
/// leaving the resource's tags as they were.
#[tokio::test]
async fn a_tagging_call_this_service_cannot_serve_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("tag-refusals", "tok-tag-refuse").await;
    let user = emulator.user(&instance, "tagged", "tok-tag-refuse-u").await;

    // Rows: each kind of ARN of THIS service that names nothing — an app instance, a
    // user, and a bot (a resource type this service mints but holds no record of) — plus
    // an ARN outside this family's ARN namespace altogether, which names no resource this
    // service serves tagging for.
    let absent: Vec<(&str, String)> = vec![
        ("an app instance that never existed", format!("{instance}x")),
        ("a user that never existed", format!("{user}x")),
        ("a bot of a live app instance", format!("{instance}/bot/b1")),
        (
            "another vendor's resource ARN",
            "arn:aws:sqs:us-east-1:000000000000:queue/q1".to_string(),
        ),
    ];

    for (case, arn) in absent {
        let refusals = [
            ("TagResource", tag(&emulator, &arn, &owned(&[("k", "v")])).await),
            ("UntagResource", untag(&emulator, &arn, &keys(&["k"])).await),
            (
                "ListTagsForResource",
                emulator
                    .call("GET", &format!("/tags?arn={}", uri_encode(&arn)), None)
                    .await,
            ),
        ];
        for (operation, (status, payload)) in refusals {
            assert_eq!(
                (status, payload["Code"].as_str()),
                (400, Some("NotFound")),
                "case {case:?} ({operation}): expected 400 NotFound for {arn}, got \
                 {status} {payload:?}"
            );
        }
    }

    // The tag count is bounded by the model's own `TagList` maximum (50): a call that
    // would take the resource past it is refused and writes nothing.
    let at_maximum: Vec<(String, String)> = (0..50)
        .map(|index| (format!("k{index:02}"), "v".to_string()))
        .collect();
    let (status, body) = tag(&emulator, &instance, &at_maximum).await;
    assert_eq!(
        status, 204,
        "case the modelled maximum of tags: expected it accepted, got {status} {body:?}"
    );

    let (status, body) = tag(&emulator, &instance, &owned(&[("one-too-many", "v")])).await;
    assert_eq!(
        (status, body["Code"].as_str()),
        (400, Some("ResourceLimitExceeded")),
        "case one tag past the maximum: expected the modelled resource-limit refusal, got \
         {status} {body:?}"
    );
    let (status, listed) = list(&emulator, &instance).await;
    assert_eq!(
        (status, listed.len()),
        (200, at_maximum.len()),
        "case one tag past the maximum: expected the refused call to write nothing"
    );

    // Re-valuing a key the resource already carries adds none, so it is accepted even at
    // the maximum.
    let (status, body) = tag(&emulator, &instance, &owned(&[("k00", "re-valued")])).await;
    assert_eq!(
        status, 204,
        "case re-valuing a key at the maximum: expected it accepted, got {status} {body:?}"
    );
    let (status, listed) = list(&emulator, &instance).await;
    assert_eq!(
        (status, listed.get("k00").map(String::as_str)),
        (200, Some("re-valued")),
        "case re-valuing a key at the maximum: expected the new value, got {listed:?}"
    );
}
