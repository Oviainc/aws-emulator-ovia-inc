// SPDX-License-Identifier: CC0-1.0

//! `UntagGlobalResource` end to end over HTTP, against every global resource the operation
//! documents: router inputs, router outputs and router network interfaces.
//!
//! The rule under test: `DELETE /tags/global/{ResourceArn}?tagKeys=..` REMOVES exactly the
//! keys it names from the stored tag map of the ONE global resource that ARN addresses — a
//! key it names stops being carried, a key it does not name keeps the value it had — and the
//! operation is `@idempotent`, so naming a key the resource does not carry removes nothing
//! and is still a 204. An ARN that addresses no global resource this service holds is the
//! modelled `NotFoundException` (404), never a 501 and never a success.
//!
//! The rows vary what those rules generalise over: which of the three global resource kinds
//! carries the tags, how many keys one request names (one, several), whether a named key is
//! carried or absent, the form of the key (ASCII, non-ASCII, URI-significant, at the
//! documented key-length maximum), and the ways an ARN can address nothing.
//!
//! What the removal left is read back through `ListTagsForGlobalResource` — the report whose
//! own suite pins it against the record — so a handler that answered its own request rather
//! than writing the store cannot pass.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization.

use std::collections::BTreeMap;

use serde_json::json;

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
// The three global-resource creates, the kinds they cover, the global tag WRITE and the
// per-kind record reader: the global tagging suite owns them, so this suite removes tags
// from exactly the subjects that one writes to.
use crate::global_resource_tags::{
    create_subject, read_back, tag_global, timestamp, GlobalKind, EVERY_KIND,
};
// The global tag REPORT, which is how what a removal left is read back.
use crate::global_resource_tag_listing::reported_tags;
// The account id, the percent-encoder, the length-bound filler, the tag-map builder and the
// reply triple the REGIONAL tagging suite owns: one definition of each in this test binary.
use crate::resource_tags::{filler, pct, tag_map, Reply, ACCOUNT};
use crate::router_network_interfaces::UNKNOWN_ARN;

/// `DELETE /tags/global/{ResourceArn}?tagKeys=..&tagKeys=..` — `TagKeys` is bound to
/// `@httpQuery("tagKeys")`, so each key travels as its own query pair, and both the ARN label
/// and every key are percent-encoded, which is what an SDK client sends.
async fn untag_global(emulator: &Emulator, resource_arn: &str, keys: &[String]) -> Reply {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", pct(key)))
        .collect::<Vec<_>>()
        .join("&");
    let path = format!("/tags/global/{}", pct(resource_arn));
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    emulator.send("DELETE", &uri, None).await
}

/// `DELETE /tags/{ResourceArn}?tagKeys=..` — the REGIONAL removal, for the case that pins
/// which operation reaches which resource.
async fn untag_regional(emulator: &Emulator, resource_arn: &str, keys: &[String]) -> Reply {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", pct(key)))
        .collect::<Vec<_>>()
        .join("&");
    emulator
        .send(
            "DELETE",
            &format!("/tags/{}?{query}", pct(resource_arn)),
            None,
        )
        .await
}

/// A tag key at the documented maximum character length, from the `Tags` member
/// documentation (`mc::MAX_TAG_KEY_LENGTH`): a removal addresses a key of any length the
/// service stores, so the longest storable key is one of the rows.
fn longest_key() -> String {
    filler(mc::MAX_TAG_KEY_LENGTH, 3)
}

/// The tag map every subject starts from. The keys span the forms a key can take, because
/// the removal addresses a key by its exact bytes: ASCII, non-ASCII, URI-significant, empty
/// value, and the documented maximum key length.
fn starting_tags() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("env".to_string(), "prod".to_string()),
        ("team".to_string(), "video".to_string()),
        ("owner".to_string(), "ops".to_string()),
        ("cost-center".to_string(), "cc-42".to_string()),
        ("проект".to_string(), "видео-поток".to_string()),
        ("path/key".to_string(), "a:b c&d=e".to_string()),
        ("blank".to_string(), String::new()),
        (longest_key(), "at the key bound".to_string()),
    ])
}

/// The removals one subject is driven through. Each row differs from the others along what
/// the rule generalises over: how many keys the request names, whether a named key is
/// carried or absent, and the form of the key.
fn removal_rows() -> Vec<(&'static str, Vec<String>)> {
    vec![
        ("one key the resource carries", vec!["env".to_string()]),
        ("a key the resource does not carry", vec!["absent".to_string()]),
        (
            "the same absent key again — removal is idempotent",
            vec!["absent".to_string()],
        ),
        (
            "two keys, one carried and one absent",
            vec!["team".to_string(), "also-absent".to_string()],
        ),
        ("a non-ASCII key", vec!["проект".to_string()]),
        (
            "a key with URI-significant characters",
            vec!["path/key".to_string()],
        ),
        (
            "a key whose value is empty — the key is what addresses the tag",
            vec!["blank".to_string()],
        ),
        (
            "a key at the documented maximum character length",
            vec![longest_key()],
        ),
        (
            "every remaining key at once",
            vec!["owner".to_string(), "cost-center".to_string()],
        ),
        (
            "a key against an already empty tag map",
            vec!["owner".to_string()],
        ),
    ]
}

/// A removal takes exactly the keys it names off the ONE global resource it addresses,
/// whichever of the three kinds that is, and the report afterwards answers what is left. The
/// expected map is maintained by the rule itself (remove the named keys, touch nothing else),
/// so each row asserts the rule rather than a recorded answer — and every other subject is
/// re-read each round, so a removal that reached the wrong resource fails here.
#[tokio::test]
async fn untagging_a_global_resource_removes_exactly_the_named_keys() {
    let emulator = Emulator::new();

    let initial = starting_tags();
    let requested = tag_map(
        &initial
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>(),
    );

    let mut created = Vec::new();
    for (index, kind) in EVERY_KIND.into_iter().enumerate() {
        let arn = create_subject(
            &emulator,
            kind,
            &format!("untagged-{index}"),
            Some(requested.clone()),
        )
        .await;
        created.push((kind, arn));
    }

    let mut expected: BTreeMap<String, BTreeMap<String, String>> = created
        .iter()
        .map(|(_, arn)| (arn.clone(), initial.clone()))
        .collect();

    for (kind, arn) in &created {
        for (row, keys) in removal_rows() {
            let case = format!("{kind:?} / {row}");
            let (status, code, body) = untag_global(&emulator, arn, &keys).await;
            assert_eq!(
                (status, code.as_deref()),
                (204, None),
                "case {case:?}: UntagGlobalResource expected 204 with no error, got {status} \
                 {code:?}: {body}"
            );

            let want = expected
                .get_mut(arn)
                .expect("every subject has an expected tag map");
            for key in &keys {
                want.remove(key);
            }

            // Every subject, not just the one the removal addressed: the request named one
            // resource, and reading a resource changes nothing a later caller sees.
            for (_, other_arn) in &created {
                let reported = reported_tags(&emulator, &case, other_arn).await;
                let want = expected
                    .get(other_arn)
                    .expect("every subject has an expected tag map");
                assert_eq!(
                    &reported, want,
                    "case {case:?}: {other_arn} must report the keys it has left, got \
                     {reported:?}"
                );
            }
        }
    }

    // A tag write after the removals starts from what is left, not from what was once there.
    for (kind, arn) in &created {
        let case = format!("{kind:?} / a write after the removals");
        let (status, code, body) = tag_global(&emulator, arn, json!({ "env": "back" })).await;
        assert_eq!(
            (status, code.as_deref()),
            (204, None),
            "case {case:?}: the tag write expected 204, got {status} {code:?}: {body}"
        );
        let reported = reported_tags(&emulator, &case, arn).await;
        assert_eq!(
            reported,
            BTreeMap::from([("env".to_string(), "back".to_string())]),
            "case {case:?}: re-tagging an emptied resource leaves only the new pair, got \
             {reported:?}"
        );
    }
}

/// A `ResourceArn` that addresses no GLOBAL resource this service holds is refused with the
/// modelled `NotFoundException` (404) — never a 501 and never a success. The rows are the
/// ways an ARN can address nothing: each of the three global ARN forms with an id no create
/// minted, a REGIONAL resource this service really holds and really tags (a flow, which the
/// regional removal reaches instead), another MediaConnect resource type, another service's
/// ARN, and a string that is no ARN at all.
#[tokio::test]
async fn untagging_a_global_arn_that_addresses_nothing_is_not_found() {
    let emulator = Emulator::new();

    let (status, code, created) = emulator
        .create_flow(json!({
            "name": "global-untag-flow",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
            "flowTags": { "env": "prod" },
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "creating the fixture flow expected 201, got {status} {code:?}: {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create answers a FlowArn, got {created}"))
        .to_string();

    let rows = [
        (
            "a router input ARN no create minted",
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:routerInput:ffffffffffff"),
        ),
        (
            "a router output ARN no create minted",
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:routerOutput:eeeeeeeeeeee"),
        ),
        (
            "a router network interface ARN no create minted",
            UNKNOWN_ARN.to_string(),
        ),
        (
            "a flow ARN this service does hold: a flow is not a global resource",
            flow_arn.clone(),
        ),
        (
            "a gateway ARN",
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:gateway:1-0000beef:some-gateway"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:medialive:{REGION}:{ACCOUNT}:channel:1234567"),
        ),
        ("a string that is no ARN at all", "not-an-arn".to_string()),
    ];

    for (case, arn) in rows {
        let (status, code, body) = untag_global(&emulator, &arn, &["env".to_string()]).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: expected 404 NotFoundException, got {status} {code:?}: {body}"
        );
    }

    // The flow's own tags were never touched by the refusals above, and the REGIONAL removal
    // is what reaches them.
    let (status, code, body) = untag_regional(&emulator, &flow_arn, &["env".to_string()]).await;
    assert_eq!(
        (status, code.as_deref()),
        (204, None),
        "UntagResource reaches a flow, got {status} {code:?}: {body}"
    );

    // The other half of the split, pinned so it cannot drift: the REGIONAL removal does not
    // reach a global resource, while the global one does.
    let router_input =
        create_subject(&emulator, GlobalKind::Input, "not-regional", Some(json!({}))).await;
    let (status, code, body) = untag_regional(&emulator, &router_input, &["k".to_string()]).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "UntagResource does not reach a router input, got {status} {code:?}: {body}"
    );
    let (status, code, body) = untag_global(&emulator, &router_input, &["k".to_string()]).await;
    assert_eq!(
        (status, code.as_deref()),
        (204, None),
        "UntagGlobalResource reaches a router input, got {status} {code:?}: {body}"
    );
}

/// The tags are a member of the global resource, so a removal that TAKES a key away changes
/// the record and its `UpdatedAt` moves with it — and a removal that names only keys the
/// resource does not carry changed nothing, so `UpdatedAt` stays where it is. Nothing else
/// the create stored moves either way.
#[tokio::test]
async fn a_removal_moves_updated_at_only_when_it_takes_a_key_away() {
    let emulator = Emulator::new();

    let mut created = Vec::new();
    for (index, kind) in EVERY_KIND.into_iter().enumerate() {
        let arn = create_subject(
            &emulator,
            kind,
            &format!("untag-stamped-{index}"),
            Some(json!({ "env": "prod", "team": "video" })),
        )
        .await;
        let case = format!("{kind:?} / created");
        let resource = read_back(&emulator, &case, kind, &arn).await;
        created.push((kind, arn, resource));
    }

    // Enough for the clock to have moved past the instant the creates stamped.
    tokio::time::sleep(mc::TRANSITION_SPAN * 2).await;

    let mut stamped = Vec::new();
    for (kind, arn, at_create) in &created {
        let case = format!("{kind:?} / a removal that takes a key away");
        let (status, code, body) = untag_global(&emulator, arn, &["env".to_string()]).await;
        assert_eq!(
            (status, code.as_deref()),
            (204, None),
            "case {case:?}: expected 204, got {status} {code:?}: {body}"
        );
        let resource = read_back(&emulator, &case, *kind, arn).await;
        let moved = timestamp(&case, "updatedAt", &resource);
        let at_create_stamp = timestamp(&case, "updatedAt", at_create);
        assert!(
            moved > at_create_stamp,
            "case {case:?}: a removal that changed the tags moves UpdatedAt, got {moved} and \
             {at_create_stamp}"
        );
        assert_eq!(
            resource["name"], at_create["name"],
            "case {case:?}: a removal leaves the Name as the create stored it, got {:?}",
            resource["name"]
        );
        assert_eq!(
            resource["createdAt"], at_create["createdAt"],
            "case {case:?}: a removal leaves CreatedAt where it was, got {:?}",
            resource["createdAt"]
        );
        stamped.push((*kind, arn.clone(), moved));
    }

    tokio::time::sleep(mc::TRANSITION_SPAN * 2).await;

    for (kind, arn, moved) in &stamped {
        let case = format!("{kind:?} / a removal that takes nothing away");
        let (status, code, body) =
            untag_global(&emulator, arn, &["env".to_string(), "never-set".to_string()]).await;
        assert_eq!(
            (status, code.as_deref()),
            (204, None),
            "case {case:?}: expected 204, got {status} {code:?}: {body}"
        );
        let resource = read_back(&emulator, &case, *kind, arn).await;
        let still = timestamp(&case, "updatedAt", &resource);
        assert_eq!(
            still, *moved,
            "case {case:?}: naming only keys the resource does not carry leaves UpdatedAt \
             where it was, got {still} and {moved}"
        );
        // And the key the first removal did not name is still carried.
        let reported = reported_tags(&emulator, &case, arn).await;
        assert_eq!(
            reported,
            BTreeMap::from([("team".to_string(), "video".to_string())]),
            "case {case:?}: a key no request named keeps its value, got {reported:?}"
        );
    }
}

/// A global resource the clock has already taken away is a resource no removal reaches: once
/// DeleteRouterInput's removal has come due, the ARN addresses nothing and the request is the
/// modelled 404 — while the input is still on its way out it is still there.
#[tokio::test]
async fn untagging_a_router_input_the_clock_removed_is_not_found() {
    let emulator = Emulator::new();
    let arn = create_subject(
        &emulator,
        GlobalKind::Input,
        "untag-doomed",
        Some(json!({ "env": "prod" })),
    )
    .await;

    let (status, code, body) = emulator
        .send("DELETE", &format!("/v1/routerInput/{}", pct(&arn)), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "deleting the fixture router input expected 200, got {status} {code:?}: {body}"
    );

    // Still on its way out: the record exists, so the removal is served.
    let (status, code, body) = untag_global(&emulator, &arn, &["env".to_string()]).await;
    assert_eq!(
        (status, code.as_deref()),
        (204, None),
        "untagging an input that is still being deleted expected 204, got {status} {code:?}: \
         {body}"
    );

    // Past the span the removal was owed at, the record is gone and so is its tag map.
    tokio::time::sleep(mc::TRANSITION_SPAN * 2).await;
    let (status, code, body) = untag_global(&emulator, &arn, &["env".to_string()]).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "untagging a removed router input expected 404 NotFoundException, got {status} \
         {code:?}: {body}"
    );
}
