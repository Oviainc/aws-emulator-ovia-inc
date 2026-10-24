// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListTagsForResource` as a READER of stored tag state.
//!
//! The rule under test: `ListTagsForResource` answers exactly the tag list stored on the
//! resource its `ResourceArn` names — whatever put those tags there (a create, a later
//! `TagResource`, an `UntagResource`) and whichever taggable kind the ARN names — and,
//! being a read, it changes nothing a later caller sees.
//!
//! The suite in `resource_tagging` drives the write side of the slice; this one pins the
//! read: provenance-independence, per-ARN isolation between co-existing resources,
//! repeat-stability, and the refusal once the named resource is gone. Every call goes
//! through the real `MultiServiceRouter`, and no test reaches into `crate::state`.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, Emulator};
use crate::resource_tagging::{
    create_taggable, list_tags, tag, tags_through_resource_reader, tags_uri, untag_uri, Taggable,
    DEVICE, GLOBAL_NETWORK, LINK, MAX_KEY_CHARS, MAX_VALUE_CHARS, SITE,
};

/// The whole resource record its own reader (`DescribeGlobalNetworks` / `GetSites` /
/// `GetLinks` / `GetDevices`) answers with, so a test can assert that a read left every
/// member — not just `Tags` — exactly as it was.
async fn resource_record(emulator: &Emulator, resource: &Taggable) -> Value {
    let (status, body) = emulator.get(&resource.read_uri).await;
    assert_eq!(
        status, 200,
        "reading {} back: expected 200, got {status} with {body}",
        resource.kind
    );
    body[resource.list]
        .as_array()
        .unwrap_or_else(|| panic!("expected a {} array in {body}", resource.list))
        .iter()
        .find(|item| item[resource.id_member] == json!(resource.id))
        .unwrap_or_else(|| panic!("expected {} in {body}", resource.id))
        .clone()
}

/// Apply `tags` with `TagResource` and assert it was accepted.
async fn tag_resource(emulator: &Emulator, resource: &Taggable, tags: &Value) {
    let (status, body) = emulator
        .post(&tags_uri(&resource.arn), json!({ "Tags": tags }))
        .await;
    assert_eq!(
        status, 200,
        "TagResource({tags}) on a {}: expected 200, got {status} with {body}",
        resource.kind
    );
}

/// Remove `keys` with `UntagResource` and assert it was accepted.
async fn untag_resource(emulator: &Emulator, resource: &Taggable, keys: &[&str]) {
    let (status, body) = emulator.delete(&untag_uri(&resource.arn, keys)).await;
    assert_eq!(
        status, 200,
        "UntagResource({keys:?}) on a {}: expected 200, got {status} with {body}",
        resource.kind
    );
}

/// One row of the read table: a taggable resource built along some path, and the tag list
/// `ListTagsForResource` must answer for it once that path has run.
struct ReadCase {
    name: &'static str,
    kind: &'static str,
    /// The `Tags` the create carries (`Value::Null` for "no `Tags` member").
    created_with: Value,
    /// The `Tags` a later `TagResource` applies (`Value::Null` for "no such call").
    then_tagged: Value,
    /// The keys a later `UntagResource` removes.
    then_untagged: &'static [&'static str],
    /// The list the operation must report afterwards.
    expected: Value,
}

/// The rows of the read table. They differ along the taggable kind (every kind this
/// service's state holds), along how the tags got there (create only, `TagResource` only,
/// both, and a removal afterwards), along how many tags the resource ends up with (none,
/// one, several, and a dozen whose key order must be reported stably) and along the form
/// of the keys and values (ASCII, non-ASCII, an empty value, and the documented maximum
/// key and value lengths).
fn read_cases() -> Vec<ReadCase> {
    vec![
        ReadCase {
            name: "a global network created with no Tags member",
            kind: GLOBAL_NETWORK,
            created_with: Value::Null,
            then_tagged: Value::Null,
            then_untagged: &[],
            expected: json!([]),
        },
        ReadCase {
            name: "a site created with an empty tag list",
            kind: SITE,
            created_with: json!([]),
            then_tagged: Value::Null,
            then_untagged: &[],
            expected: json!([]),
        },
        ReadCase {
            name: "a link whose single tag came from the create",
            kind: LINK,
            created_with: json!([tag("env", "test")]),
            then_tagged: Value::Null,
            then_untagged: &[],
            expected: json!([tag("env", "test")]),
        },
        ReadCase {
            name: "a device tagged on the create and again afterwards",
            kind: DEVICE,
            created_with: json!([tag("env", "dev")]),
            then_tagged: json!([tag("owner", "ops")]),
            then_untagged: &["env"],
            expected: json!([tag("owner", "ops")]),
        },
        ReadCase {
            name: "a global network whose tags came only from TagResource",
            kind: GLOBAL_NETWORK,
            created_with: Value::Null,
            then_tagged: json!([tag("env", "prod"), tag("tier", "edge")]),
            then_untagged: &[],
            expected: json!([tag("env", "prod"), tag("tier", "edge")]),
        },
        ReadCase {
            name: "a site tagged on the create and again afterwards",
            kind: SITE,
            created_with: json!([tag("env", "dev")]),
            then_tagged: json!([tag("owner", "ops")]),
            then_untagged: &[],
            expected: json!([tag("env", "dev"), tag("owner", "ops")]),
        },
        ReadCase {
            name: "a link that has had every tag removed again",
            kind: LINK,
            created_with: json!([tag("env", "dev"), tag("owner", "ops")]),
            then_tagged: Value::Null,
            then_untagged: &["env", "owner"],
            expected: json!([]),
        },
        ReadCase {
            name: "a site carrying a non-ASCII key and an empty value",
            kind: SITE,
            created_with: json!([tag("ключ", "значение")]),
            then_tagged: json!([tag("blank", "")]),
            then_untagged: &[],
            expected: json!([tag("ключ", "значение"), tag("blank", "")]),
        },
        ReadCase {
            name: "a global network carrying a key and value at the documented maxima",
            kind: GLOBAL_NETWORK,
            created_with: json!([tag(&"k".repeat(MAX_KEY_CHARS), &"v".repeat(MAX_VALUE_CHARS))]),
            then_tagged: Value::Null,
            then_untagged: &[],
            expected: json!([tag(&"k".repeat(MAX_KEY_CHARS), &"v".repeat(MAX_VALUE_CHARS))]),
        },
        ReadCase {
            name: "a link carrying a dozen tags, one of them removed",
            kind: LINK,
            created_with: json!([
                tag("k00", "v00"),
                tag("k01", "v01"),
                tag("k02", "v02"),
                tag("k03", "v03"),
                tag("k04", "v04"),
                tag("k05", "v05"),
            ]),
            then_tagged: json!([
                tag("k06", "v06"),
                tag("k07", "v07"),
                tag("k08", "v08"),
                tag("k09", "v09"),
                tag("k10", "v10"),
                tag("k11", "v11"),
            ]),
            then_untagged: &["k03"],
            expected: json!([
                tag("k00", "v00"),
                tag("k01", "v01"),
                tag("k02", "v02"),
                tag("k04", "v04"),
                tag("k05", "v05"),
                tag("k06", "v06"),
                tag("k07", "v07"),
                tag("k08", "v08"),
                tag("k09", "v09"),
                tag("k10", "v10"),
                tag("k11", "v11"),
            ]),
        },
    ]
}

#[tokio::test]
async fn list_tags_for_resource_answers_the_stored_tags_whatever_put_them_there() {
    for case in read_cases() {
        let name = case.name;
        let emulator = Emulator::new();
        let resource = create_taggable(&emulator, case.kind, &case.created_with).await;

        if !case.then_tagged.is_null() {
            tag_resource(&emulator, &resource, &case.then_tagged).await;
        }
        if !case.then_untagged.is_empty() {
            untag_resource(&emulator, &resource, case.then_untagged).await;
        }

        let (status, listed) = list_tags(&emulator, &resource.arn).await;
        assert_eq!(
            status, 200,
            "case {name}: ListTagsForResource expected 200, got {status} with {listed}"
        );
        assert_eq!(
            listed, case.expected,
            "case {name}: expected the stored list {}, got {listed}",
            case.expected
        );

        // The resource's own reader answers from the same stored list, so the two agree.
        let through_reader = tags_through_resource_reader(&emulator, &resource).await;
        assert_eq!(
            through_reader, case.expected,
            "case {name}: the resource's own reader must report the same stored tags, got {through_reader}"
        );
    }
}

#[tokio::test]
async fn listing_tags_changes_nothing_a_later_caller_sees() {
    // A read may not move any state: repeated calls answer identically, and every other
    // view of the resource — its own reader, and the lists of its neighbours — is the
    // same afterwards as before.
    for kind in [GLOBAL_NETWORK, SITE, LINK, DEVICE] {
        let emulator = Emulator::new();
        let resource =
            create_taggable(&emulator, kind, &json!([tag("env", "test"), tag("tier", "a")])).await;
        let neighbour = create_taggable(&emulator, kind, &json!([tag("env", "other")])).await;

        let before = resource_record(&emulator, &resource).await;
        let (_, first) = list_tags(&emulator, &resource.arn).await;

        for attempt in 1..=4u32 {
            let (status, listed) = list_tags(&emulator, &resource.arn).await;
            assert_eq!(
                status, 200,
                "case {kind} repeat {attempt}: expected 200, got {status} with {listed}"
            );
            assert_eq!(
                listed, first,
                "case {kind} repeat {attempt}: a read must answer the same list every time, got {listed}"
            );
        }

        let after = resource_record(&emulator, &resource).await;
        assert_eq!(
            after, before,
            "case {kind}: listing tags must leave the resource record untouched, got {after}"
        );

        let (status, neighbour_tags) = list_tags(&emulator, &neighbour.arn).await;
        assert_eq!(
            status, 200,
            "case {kind}: the neighbour must still be readable, got {status} with {neighbour_tags}"
        );
        assert_eq!(
            neighbour_tags,
            json!([tag("env", "other")]),
            "case {kind}: listing one resource's tags must not touch another's, got {neighbour_tags}"
        );
    }
}

#[tokio::test]
async fn every_arn_answers_only_the_tags_of_the_resource_it_names() {
    let emulator = Emulator::new();

    // Several resources, of every taggable kind and two of one kind, live side by side in
    // one store; each carries a tag set only it has.
    let rows: Vec<(&'static str, &'static str, Value)> = vec![
        ("the first global network", GLOBAL_NETWORK, json!([tag("which", "gn-1")])),
        (
            "the second global network",
            GLOBAL_NETWORK,
            json!([tag("which", "gn-2"), tag("extra", "yes")]),
        ),
        ("the site", SITE, json!([tag("which", "site")])),
        ("the link", LINK, json!([tag("which", "link"), tag("env", "test")])),
    ];

    let mut created: Vec<(&str, Taggable, Value)> = Vec::new();
    for (name, kind, tags) in rows {
        let resource = create_taggable(&emulator, kind, &tags).await;
        created.push((name, resource, tags));
    }

    for (name, resource, expected) in &created {
        let (status, listed) = list_tags(&emulator, &resource.arn).await;
        assert_eq!(
            status, 200,
            "case {name}: ListTagsForResource expected 200, got {status} with {listed}"
        );
        assert_eq!(
            &listed, expected,
            "case {name}: expected only its own tags {expected}, got {listed}"
        );
    }
}

#[tokio::test]
async fn the_arn_of_a_deleted_resource_is_not_found_while_its_neighbours_still_read() {
    let emulator = Emulator::new();

    // A global network with a tagged site under it, plus a second tagged global network
    // that nothing deletes.
    let kept = create_taggable(&emulator, GLOBAL_NETWORK, &json!([tag("keep", "me")])).await;
    let site = create_taggable(&emulator, SITE, &json!([tag("env", "test")])).await;
    let (status, sites) = emulator.get(&site.read_uri).await;
    assert_eq!(
        status, 200,
        "GetSites: expected 200, got {status} with {sites}"
    );
    let parent = sites["Sites"][0]["GlobalNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a GlobalNetworkId in {sites}"))
        .to_string();

    let (status, listed) = list_tags(&emulator, &site.arn).await;
    assert_eq!(
        status, 200,
        "the live site must list its tags, got {status} with {listed}"
    );

    let (status, deleted) = emulator
        .delete(&format!("/global-networks/{parent}/sites/{}", site.id))
        .await;
    assert_eq!(
        status, 200,
        "DeleteSite: expected 200, got {status} with {deleted}"
    );

    // The ARN is still well formed, but it now names no resource at all.
    let (status, body) = list_tags(&emulator, &site.arn).await;
    assert_eq!(
        status, 404,
        "a deleted site's ARN: expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "a deleted site's ARN: wrong error code in {body}"
    );

    let (status, listed) = list_tags(&emulator, &kept.arn).await;
    assert_eq!(
        status, 200,
        "the surviving global network must still list its tags, got {status} with {listed}"
    );
    assert_eq!(
        listed,
        json!([tag("keep", "me")]),
        "the surviving global network's tags must be untouched, got {listed}"
    );
}
