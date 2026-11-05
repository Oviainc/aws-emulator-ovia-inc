// SPDX-License-Identifier: CC0-1.0

//! Resource tags, end to end over the awsJson1_1 wire.
//!
//! Every case drives a real `MultiServiceRouter` with this service mounted on one shared
//! engine (the router and the wire helper are [`crate::workspace_lifecycle`]'s), so each
//! assertion is about what a client would see: the request is serialized, routed,
//! validated, handled and serialized back.
//!
//! The rules under test, all of them about resources this emulator persisted rather than
//! about any one recorded request:
//!
//! * `CreateTags`, `DescribeTags` and `DeleteTags` address ONE tag set per resource,
//!   resolved from the kind-agnostic `ResourceId` — the very set the create that minted
//!   the resource wrote, whatever kind of resource it is.
//! * `CreateTags` adds the keys the resource does not hold and REPLACES the value of the
//!   keys it does, so a resource holds at most one tag per key; `DescribeTags` reports
//!   that set and changes nothing a later caller sees; `DeleteTags` removes exactly the
//!   keys it names and keeps every other tag in place.
//! * A resource holds at most `MAX_TAGS_PER_RESOURCE` tags; a `CreateTags` that would
//!   leave it holding more is refused with `ResourceLimitExceededException` and writes
//!   nothing.
//! * An id that names no resource this emulator holds is refused with
//!   `ResourceNotFoundException` by all three operations, with the requested id reported
//!   in the error's own `ResourceId` member.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_workspaces_workspacesservice::state::{
    MAX_TAGS_PER_RESOURCE, MESSAGE_TAG_RESOURCE_NOT_FOUND,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::account_links::error_code;
use crate::workspace_lifecycle::{call, emulator_with_engine};

/// The directory every WorkSpace below is created in, and one taggable resource in its
/// own right (`DirectoryId` is `^(d-[0-9a-f]{8,63}$)|(wsd-[0-9a-z]{8,63}$)$`).
const DIRECTORY_ID: &str = "d-1234567890";

/// Call `CreateTags` and return `(status, body)`.
async fn create_tags(router: &MultiServiceRouter, resource_id: &str, tags: &Value) -> (u16, Value) {
    call(
        router,
        "CreateTags",
        &json!({ "ResourceId": resource_id, "Tags": tags }),
    )
    .await
}

/// Call `DescribeTags` and return `(status, body)`.
async fn describe_tags(router: &MultiServiceRouter, resource_id: &str) -> (u16, Value) {
    call(router, "DescribeTags", &json!({ "ResourceId": resource_id })).await
}

/// Call `DeleteTags` and return `(status, body)`.
async fn delete_tags(
    router: &MultiServiceRouter,
    resource_id: &str,
    tag_keys: &[&str],
) -> (u16, Value) {
    call(
        router,
        "DeleteTags",
        &json!({ "ResourceId": resource_id, "TagKeys": tag_keys }),
    )
    .await
}

/// The tag set a `DescribeTags` response reports, as key -> the entry's `Value` member
/// (`Value::Null` when the entry carries none).
///
/// A map rather than a list: the model documents no order for `TagList`, so a case
/// asserts WHICH tags a resource holds, and that it holds one entry per key.
fn reported_tags(case: &str, body: &Value) -> BTreeMap<String, Value> {
    let list = body["TagList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: TagList must be a list, got {body}"));
    let mut tags = BTreeMap::new();
    for entry in list {
        let key = entry["Key"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: every tag carries a Key, got {body}"))
            .to_string();
        let previous = tags.insert(key.clone(), entry["Value"].clone());
        assert!(
            previous.is_none(),
            "case {case:?}: a resource holds at most one tag per key, got {key:?} twice in {body}"
        );
    }
    tags
}

/// The same shape as [`reported_tags`], for the tag list a request supplied: what a
/// `DescribeTags` must report for a resource that holds exactly these tags.
fn expected_tags(tags: &Value) -> BTreeMap<String, Value> {
    let mut expected = BTreeMap::new();
    for entry in tags.as_array().expect("a tag list") {
        // A key repeated inside one request leaves the LAST value, so inserting in
        // request order yields what the resource must hold.
        expected.insert(
            entry["Key"].as_str().expect("a tag key").to_string(),
            entry["Value"].clone(),
        );
    }
    expected
}

/// `count` tags, keys `<prefix>-<index>` and values `<prefix>-value-<index>`.
fn tags(prefix: &str, count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                json!({
                    "Key": format!("{prefix}-{index}"),
                    "Value": format!("{prefix}-value-{index}"),
                })
            })
            .collect(),
    )
}

/// Create one IP access control group named `group_name`, carrying `creation_tags`, and
/// return its `GroupId`.
async fn ip_group(router: &MultiServiceRouter, group_name: &str, creation_tags: &Value) -> String {
    let request = json!({ "GroupName": group_name, "Tags": creation_tags });
    let (status, body) = call(router, "CreateIpGroup", &request).await;
    assert_eq!(
        status, 200,
        "case {group_name:?}: CreateIpGroup must answer 200, got {status} with {body}"
    );
    body["GroupId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {group_name:?}: a minted GroupId, got {body}"))
        .to_string()
}

/// One resource of every kind this emulator persists a tag set for, each created
/// through the wire carrying `creation_tags`: the rows a tag case spans.
///
/// The kinds come from the model's `CreateTagsRequest$ResourceId` doc — "The supported
/// resource types are WorkSpaces, registered directories, images, custom bundles, IP
/// access control groups, and connection aliases" — restricted to the ones this emulator
/// holds a record for.
async fn taggable_resources(
    router: &MultiServiceRouter,
    creation_tags: &Value,
) -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = Vec::new();

    rows.push((
        "an IP access control group",
        ip_group(router, "tagged-group", creation_tags).await,
    ));

    let (status, body) = call(
        router,
        "CreateConnectionAlias",
        &json!({ "ConnectionString": "tags.example.com", "Tags": creation_tags }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateConnectionAlias must answer 200, got {status} with {body}"
    );
    rows.push((
        "a connection alias",
        body["AliasId"]
            .as_str()
            .unwrap_or_else(|| panic!("a minted AliasId, got {body}"))
            .to_string(),
    ));

    let (status, body) = call(
        router,
        "RegisterWorkspaceDirectory",
        &json!({
            "DirectoryId": DIRECTORY_ID,
            "SubnetIds": ["subnet-11111111", "subnet-22222222"],
            "Tags": creation_tags,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "RegisterWorkspaceDirectory must answer 200, got {status} with {body}"
    );
    rows.push(("a registered directory", DIRECTORY_ID.to_string()));

    let (status, body) = call(
        router,
        "CreateWorkspaces",
        &json!({
            "Workspaces": [{
                "DirectoryId": DIRECTORY_ID,
                "UserName": "alice",
                "BundleId": "wsb-amznstandard",
                "Tags": creation_tags,
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateWorkspaces must answer 200, got {status} with {body}"
    );
    rows.push((
        "a WorkSpace",
        body["PendingRequests"][0]["WorkspaceId"]
            .as_str()
            .unwrap_or_else(|| panic!("a minted WorkspaceId, got {body}"))
            .to_string(),
    ));

    rows
}

/// Assert `body` is the empty structure an operation with no output members answers with.
fn assert_no_output_members(case: &str, body: &Value) {
    assert_eq!(
        body.as_object().map(serde_json::Map::len),
        Some(0),
        "case {case:?}: the modelled output carries no members, got {body}"
    );
}

/// Assert `(status, body)` is the refusal of an id that names no stored resource.
fn assert_not_found(case: &str, resource_id: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: an unknown ResourceId must answer 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        body["Message"].as_str().or_else(|| body["message"].as_str()),
        Some(MESSAGE_TAG_RESOURCE_NOT_FOUND),
        "case {case:?}: expected the fixed not-found sentence, got {body}"
    );
    assert_eq!(
        body["ResourceId"].as_str(),
        Some(resource_id),
        "case {case:?}: the refusal names the requested resource, got {body}"
    );
}

#[tokio::test]
async fn tags_round_trip_on_every_taggable_resource_kind() {
    let (router, _engine) = emulator_with_engine();
    let creation_tags = json!([
        { "Key": "owner", "Value": "platform" },
        { "Key": "cost-centre", "Value": "4711" },
    ]);
    let resources = taggable_resources(&router, &creation_tags).await;

    for (case, resource_id) in &resources {
        // The tags the create supplied are the set DescribeTags reports: one tag set
        // per resource, not one per operation.
        let (status, body) = describe_tags(&router, resource_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTags must answer 200, got {status} with {body}"
        );
        assert_eq!(
            reported_tags(case, &body),
            expected_tags(&creation_tags),
            "case {case:?}: DescribeTags must report the tags the create wrote, got {body}"
        );

        // CreateTags adds the keys the resource does not hold and replaces the value
        // of the one it does.
        let added = json!([
            { "Key": "owner", "Value": "platform-two" },
            { "Key": "stage", "Value": "test" },
            { "Key": "unvalued", "Value": "" },
        ]);
        let (status, body) = create_tags(&router, resource_id, &added).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateTags must answer 200, got {status} with {body}"
        );
        assert_no_output_members(case, &body);

        let merged: BTreeMap<String, Value> = expected_tags(&creation_tags)
            .into_iter()
            .chain(expected_tags(&added))
            .collect();
        let (_, body) = describe_tags(&router, resource_id).await;
        assert_eq!(
            reported_tags(case, &body),
            merged,
            "case {case:?}: a created key is added and a held key replaced, got {body}"
        );

        // DescribeTags is a read: asking twice reports the same set.
        let (_, again) = describe_tags(&router, resource_id).await;
        assert_eq!(
            reported_tags(case, &again),
            merged,
            "case {case:?}: DescribeTags must change nothing a later caller sees, got {again}"
        );

        // DeleteTags removes exactly the keys it names — a key the resource never
        // held removes nothing and is not a refusal.
        let (status, body) = delete_tags(&router, resource_id, &["cost-centre", "never-written"]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteTags must answer 200, got {status} with {body}"
        );
        assert_no_output_members(case, &body);
        let mut remaining = merged.clone();
        remaining.remove("cost-centre");
        let (_, body) = describe_tags(&router, resource_id).await;
        assert_eq!(
            reported_tags(case, &body),
            remaining,
            "case {case:?}: only the named key is removed, got {body}"
        );

        // A resource rid of every tag still exists, and reports an empty TagList.
        let keys: Vec<&str> = remaining.keys().map(String::as_str).collect();
        let (status, _) = delete_tags(&router, resource_id, &keys).await;
        assert_eq!(status, 200, "case {case:?}: DeleteTags must answer 200");
        let (status, body) = describe_tags(&router, resource_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a resource holding no tags is still described, got {status}"
        );
        assert_eq!(
            body["TagList"],
            json!([]),
            "case {case:?}: a resource holding no tags reports an empty TagList, got {body}"
        );
    }

    // Each resource carries its OWN set: emptying one did not empty another, which the
    // loop above already proved by reading each kind's creation tags back after the
    // kinds before it were emptied.
    assert_eq!(
        resources.len(),
        4,
        "every taggable resource kind this emulator holds is covered, got {resources:?}"
    );
}

/// Every tag the model admits round-trips byte for byte.
///
/// The rows span `Tag`'s own constraints — from
/// `smithy_query(query="shape", shape="Tag")`: `Key` is `len 1..127` and required,
/// `Value` is `len 0..255` and optional — along length (both published bounds), the
/// empty value, an absent `Value` member, symbols, non-ASCII text and a key repeated
/// inside one request.
#[tokio::test]
async fn created_tags_round_trip_across_the_modelled_tag_space() {
    let (router, _engine) = emulator_with_engine();

    let rows: Vec<(&str, Value)> = vec![
        ("a plain key and value", json!([{ "Key": "stage", "Value": "production" }])),
        ("an empty value", json!([{ "Key": "empty", "Value": "" }])),
        ("no Value member at all", json!([{ "Key": "bare" }])),
        (
            "the longest value the model admits",
            json!([{ "Key": "longest-value", "Value": "v".repeat(255) }]),
        ),
        (
            "the longest key the model admits",
            json!([{ "Key": "k".repeat(127), "Value": "x" }]),
        ),
        (
            "symbols a tag value carries",
            json!([{ "Key": "arn", "Value": "arn:aws:iam::123456789012:role/ws admin+test=1" }]),
        ),
        (
            "non-ASCII text",
            json!([{ "Key": "besitzer", "Value": "Gesch\u{e4}ftsf\u{fc}hrung \u{30d7}\u{30ed}" }]),
        ),
        (
            "one key twice in one request",
            json!([{ "Key": "repeated", "Value": "first" }, { "Key": "repeated", "Value": "last" }]),
        ),
        ("no tags at all", json!([])),
    ];

    for (index, (case, requested)) in rows.iter().enumerate() {
        let group = ip_group(&router, &format!("round-trip-{index}"), &json!([])).await;
        let (status, body) = create_tags(&router, &group, requested).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateTags must answer 200, got {status} with {body}"
        );
        let (_, body) = describe_tags(&router, &group).await;
        assert_eq!(
            reported_tags(case, &body),
            expected_tags(requested),
            "case {case:?}: DescribeTags must report what CreateTags wrote, got {body}"
        );
    }
}

/// A tag outside `Tag`'s own constraints is refused before anything is written.
///
/// The bounds are the model's (see
/// [`created_tags_round_trip_across_the_modelled_tag_space`]); each row is one character
/// past a published bound, which the modelled constraint refuses with
/// `ValidationException`.
#[tokio::test]
async fn tags_outside_the_modelled_bounds_are_refused_and_write_nothing() {
    let (router, _engine) = emulator_with_engine();
    let held = json!([{ "Key": "owner", "Value": "platform" }]);
    let group = ip_group(&router, "bounded-tags", &held).await;

    let rows: Vec<(&str, Value)> = vec![
        ("an empty key", json!([{ "Key": "", "Value": "x" }])),
        (
            "a key one character past the maximum",
            json!([{ "Key": "k".repeat(128), "Value": "x" }]),
        ),
        (
            "a value one character past the maximum",
            json!([{ "Key": "too-long-value", "Value": "v".repeat(256) }]),
        ),
    ];

    for (case, requested) in &rows {
        let (status, body) = create_tags(&router, &group, requested).await;
        assert_eq!(
            status, 400,
            "case {case:?}: CreateTags must refuse the tag, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        let (_, body) = describe_tags(&router, &group).await;
        assert_eq!(
            reported_tags(case, &body),
            expected_tags(&held),
            "case {case:?}: a refused request writes nothing, got {body}"
        );
    }
}

/// A resource holds at most `MAX_TAGS_PER_RESOURCE` tags.
///
/// From the model's `CreateTagsRequest$Tags` doc: "Each WorkSpaces resource can have a
/// maximum of 50 tags." The limit is measured against the resource, so each row states
/// how many tags it already holds and how many NEW keys the request adds; a request that
/// only replaces values of keys the resource holds never reaches it.
#[tokio::test]
async fn create_tags_refuses_more_tags_than_a_resource_can_hold() {
    let (router, _engine) = emulator_with_engine();

    // (held, added new keys): the accepted rows bracket the maximum, the refused ones
    // start one past it, and the last is a request far larger than any resource can
    // hold — refused on its own size, before the resource is even consulted.
    let rows: [(usize, usize); 9] = [
        (0, 1),
        (0, MAX_TAGS_PER_RESOURCE),
        (1, MAX_TAGS_PER_RESOURCE - 1),
        (MAX_TAGS_PER_RESOURCE, 0),
        (0, MAX_TAGS_PER_RESOURCE + 1),
        (1, MAX_TAGS_PER_RESOURCE),
        (MAX_TAGS_PER_RESOURCE, 1),
        (MAX_TAGS_PER_RESOURCE / 2, MAX_TAGS_PER_RESOURCE),
        (0, MAX_TAGS_PER_RESOURCE * 4),
    ];

    for (index, (held, added)) in rows.iter().copied().enumerate() {
        let case = format!("{held} held + {added} added");
        let held_tags = tags("held", held);
        let group = ip_group(&router, &format!("limit-{index}"), &held_tags).await;
        let added_tags = tags("added", added);
        let (status, body) = create_tags(&router, &group, &added_tags).await;

        let within_limit = held + added <= MAX_TAGS_PER_RESOURCE;
        if within_limit {
            assert_eq!(
                status, 200,
                "case {case:?}: at most {MAX_TAGS_PER_RESOURCE} tags is accepted, got {status} \
                 with {body}"
            );
            let expected: BTreeMap<String, Value> = expected_tags(&held_tags)
                .into_iter()
                .chain(expected_tags(&added_tags))
                .collect();
            let (_, body) = describe_tags(&router, &group).await;
            assert_eq!(
                reported_tags(&case, &body),
                expected,
                "case {case:?}: every accepted tag is reported, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: over {MAX_TAGS_PER_RESOURCE} tags is refused, got {status} with \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceLimitExceededException",
                "case {case:?}: expected ResourceLimitExceededException, got {body}"
            );
            let (_, body) = describe_tags(&router, &group).await;
            assert_eq!(
                reported_tags(&case, &body),
                expected_tags(&held_tags),
                "case {case:?}: a refused request writes none of its tags, got {body}"
            );
        }
    }

    // Replacing the value of every key a full resource holds adds no key, so it is
    // accepted where adding one more would not be.
    let held_tags = tags("held", MAX_TAGS_PER_RESOURCE);
    let group = ip_group(&router, "limit-replacements", &held_tags).await;
    let replacements = Value::Array(
        held_tags
            .as_array()
            .expect("a tag list")
            .iter()
            .map(|tag| json!({ "Key": tag["Key"], "Value": "replaced" }))
            .collect(),
    );
    let case = "replacing every key a full resource holds";
    let (status, body) = create_tags(&router, &group, &replacements).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a replacement adds no key, got {status} with {body}"
    );
    let (_, body) = describe_tags(&router, &group).await;
    assert_eq!(
        reported_tags(case, &body),
        expected_tags(&replacements),
        "case {case:?}: every value is replaced in place, got {body}"
    );

    // The maximum counts a resource's TAGS, so entries that repeat a key inside one
    // request count once: twice the maximum in entries, the maximum in distinct keys,
    // is accepted and leaves the resource holding exactly the maximum.
    let distinct = tags("distinct", MAX_TAGS_PER_RESOURCE);
    let mut repeated = distinct.as_array().expect("a tag list").clone();
    repeated.extend(
        distinct
            .as_array()
            .expect("a tag list")
            .iter()
            .map(|tag| json!({ "Key": tag["Key"], "Value": "second" })),
    );
    let repeated = Value::Array(repeated);
    let group = ip_group(&router, "limit-repeated-keys", &json!([])).await;
    let case = "twice the maximum in entries, the maximum in distinct keys";
    let (status, body) = create_tags(&router, &group, &repeated).await;
    assert_eq!(
        status, 200,
        "case {case:?}: repeated keys count once, got {status} with {body}"
    );
    let (_, body) = describe_tags(&router, &group).await;
    let stored = reported_tags(case, &body);
    assert_eq!(
        stored.len(),
        MAX_TAGS_PER_RESOURCE,
        "case {case:?}: the resource holds one tag per distinct key, got {body}"
    );
    assert_eq!(
        stored,
        expected_tags(&repeated),
        "case {case:?}: the last entry for a key is the value held, got {body}"
    );
}

/// An id that names no resource this emulator holds is refused by all three operations.
#[tokio::test]
async fn tag_requests_naming_no_stored_resource_are_refused() {
    let (router, _engine) = emulator_with_engine();
    // One stored resource of each kind exists, so the refusals below are about the id
    // naming nothing rather than about an empty store.
    let resources = taggable_resources(&router, &json!([{ "Key": "owner", "Value": "platform" }]))
        .await;

    // An Amazon-provided bundle: the model's supported kinds name CUSTOM bundles, and
    // this emulator holds none (the catalog bundles are AWS's own and
    // `CreateWorkspaceBundle` is still a scaffold stub), so no bundle id names a tag set.
    let (status, body) = call(&router, "DescribeWorkspaceBundles", &json!({ "Owner": "AMAZON" })).await;
    assert_eq!(
        status, 200,
        "DescribeWorkspaceBundles must answer 200, got {status} with {body}"
    );
    let catalog_bundle = body["Bundles"][0]["BundleId"]
        .as_str()
        .unwrap_or_else(|| panic!("the Amazon catalog carries a bundle, got {body}"))
        .to_string();

    let unknown: Vec<(&str, String)> = vec![
        ("a WorkSpace id naming no WorkSpace", "ws-00000000".to_string()),
        (
            "an IP access control group id naming no group",
            "wsipg-00000000".to_string(),
        ),
        (
            "a connection alias id naming no alias",
            "wsca-00000000".to_string(),
        ),
        (
            "a directory id naming no registered directory",
            "d-0000000000".to_string(),
        ),
        ("an Amazon-provided bundle id", catalog_bundle),
        ("an id of no WorkSpaces resource kind", "not-a-resource".to_string()),
    ];

    for (case, resource_id) in &unknown {
        let (status, body) =
            create_tags(&router, resource_id, &json!([{ "Key": "owner", "Value": "platform" }]))
                .await;
        assert_not_found(&format!("CreateTags on {case}"), resource_id, status, &body);

        let (status, body) = describe_tags(&router, resource_id).await;
        assert_not_found(&format!("DescribeTags on {case}"), resource_id, status, &body);

        let (status, body) = delete_tags(&router, resource_id, &["owner"]).await;
        assert_not_found(&format!("DeleteTags on {case}"), resource_id, status, &body);
    }

    // Every stored resource still holds the tags its create wrote: a refused request
    // touched none of them.
    for (case, resource_id) in &resources {
        let (status, body) = describe_tags(&router, resource_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTags must answer 200, got {status} with {body}"
        );
        assert_eq!(
            reported_tags(case, &body),
            expected_tags(&json!([{ "Key": "owner", "Value": "platform" }])),
            "case {case:?}: a refused request left this resource untouched, got {body}"
        );
    }
}

/// A resource that is gone takes its tag set with it.
#[tokio::test]
async fn a_deleted_resource_holds_no_tag_set() {
    let (router, _engine) = emulator_with_engine();
    let group = ip_group(
        &router,
        "deleted-group",
        &json!([{ "Key": "owner", "Value": "platform" }]),
    )
    .await;

    let (status, body) = call(&router, "DeleteIpGroup", &json!({ "GroupId": &group })).await;
    assert_eq!(
        status, 200,
        "DeleteIpGroup must answer 200, got {status} with {body}"
    );

    let (status, body) = describe_tags(&router, &group).await;
    assert_not_found("DescribeTags on a deleted group", &group, status, &body);
    let (status, body) =
        create_tags(&router, &group, &json!([{ "Key": "owner", "Value": "platform" }])).await;
    assert_not_found("CreateTags on a deleted group", &group, status, &body);
}
