// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Clean Rooms tagging slice: `TagResource`,
//! `ListTagsForResource` and `UntagResource`, all three bound to `/tags/{resourceArn}`.
//!
//! The rules under test:
//!
//! * the three operations share ONE tag set per resource, the one a create persisted:
//!   `ListTagsForResource` reports exactly the stored set (the empty map for a resource
//!   nobody has tagged, since `tags` is required), `TagResource` adds its keys and
//!   replaces the value of a key already carried while leaving every other key alone,
//!   and `UntagResource` removes exactly the keys it names — a key the resource does not
//!   carry removes nothing. Tagging one resource never changes another's tags, and a
//!   read never changes what a later read sees;
//! * the rule holds for EVERY kind of resource addressable by a `resourceArn`, not for
//!   one kind: the rows below cover each kind this service persists an ARN for whose ARN
//!   fits `CleanroomsArn`'s published 100-character bound;
//! * an ARN no resource is stored under is refused 404 on all three operations under the
//!   tagging plane's OWN error code — `NotFoundException`, not the
//!   `ResourceNotFoundException` the CRUD operations answer — with the plane's fixed
//!   sentence, reporting the requested ARN and the resource type the ARN names and
//!   changing nothing; an ARN whose resource segment names no kind of Clean Rooms
//!   resource at all is a `ValidationException` (400) naming `resourceArn`.
//!
//! The tables vary what those rules generalise over: the kind of resource, the number of
//! tags, keys and values at the published `TagKey`/`TagValue`/`TagMap` bounds, keys that
//! need percent-encoding, keys that are not carried, and whether the resource was created
//! with tags at all. Every request goes over the wire through the same
//! `MultiServiceRouter` the production router uses.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{
    account_id, error_code, error_message, minimal_collaboration, Emulator, ALL_LOG_STATUSES,
    REGION,
};
use super::cleanrooms_configured_table::{
    create_body, ALL_ANALYSIS_METHODS, ALL_TABLE_REFERENCE_TYPES,
};

/// The published bounds of the tag shapes (model shapes `TagKey` len 1..128, `TagValue`
/// len 0..256, `TagMap` len 0..200).
const TAG_KEY_MIN: usize = 1;
const TAG_KEY_MAX: usize = 128;
const TAG_VALUE_MIN: usize = 0;
const TAG_VALUE_MAX: usize = 256;
const TAG_MAP_MAX: usize = 200;

/// The published maximum length of `CleanroomsArn`, the shape every tagging operation's
/// `resourceArn` targets (model shape `CleanroomsArn`, len ..100).
const MAX_CLEANROOMS_ARN_LEN: usize = 100;

/// The message the tagging plane reports for an ARN that resolves to no resource. Fixed
/// text: the refusal interpolates no ARN, id or resource kind.
const TAGGED_RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// The wire error code the tagging plane names an absent resource by — the plane's own,
/// distinct from the `ResourceNotFoundException` the CRUD operations answer.
const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";

/// Ids in the model's `UUID` form that no create in these tests mints.
const NEVER_MINTED_IDS: [&str; 3] = [
    "00000000-0000-0000-0000-000000000000",
    "ffffffff-ffff-ffff-ffff-ffffffffffff",
    "deadbeef-0000-4000-8000-000000000001",
];

/// An ARN as ONE URI path segment, for the `@httpLabel`-bound `resourceArn`.
///
/// Only `/` is escaped: RFC 3986 lets a path segment carry `:` unescaped, and `/` is the
/// one character in a Clean Rooms ARN that would otherwise split the segment. The
/// framework percent-decodes the label before the handler sees it.
pub(crate) fn label(arn: &str) -> String {
    arn.replace('/', "%2F")
}

/// A query-string value with every byte outside RFC 3986's unreserved set
/// percent-encoded, for the `@httpQuery`-bound `UntagResource$tagKeys`.
///
/// `+` in particular has to be escaped: a query value is decoded with `+` meaning a
/// space, so an unescaped `+` would name a different key than the stored one.
fn query_value(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// A JSON object of string values as a sorted map, so a comparison does not depend on
/// map ordering.
fn as_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|members| {
            members
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

impl Emulator {
    /// `GET /tags/{resourceArn}` — `ListTagsForResource`.
    async fn list_tags(&self, resource_arn: &str) -> (u16, Value) {
        self.call("GET", &format!("/tags/{}", label(resource_arn)), None)
            .await
    }

    /// `POST /tags/{resourceArn}` — `TagResource`, with the map in the body.
    async fn tag_resource(&self, resource_arn: &str, tags: Value) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/tags/{}", label(resource_arn)),
            Some(json!({ "tags": tags })),
        )
        .await
    }

    /// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`. `tagKeys` is an
    /// `@httpQuery` list, so each key is its own query entry.
    async fn untag_resource(&self, resource_arn: &str, tag_keys: &[String]) -> (u16, Value) {
        let query: Vec<String> = tag_keys
            .iter()
            .map(|key| format!("tagKeys={}", query_value(key)))
            .collect();
        self.call(
            "DELETE",
            &format!("/tags/{}?{}", label(resource_arn), query.join("&")),
            None,
        )
        .await
    }
}

/// The tags `ListTagsForResource` reports for `resource_arn`, asserting the read itself
/// succeeded.
async fn listed_tags(emulator: &Emulator, case: &str, resource_arn: &str) -> BTreeMap<String, String> {
    let (status, body) = emulator.list_tags(resource_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListTagsForResource 200 for {resource_arn:?}, got {status} with {body}"
    );
    assert!(
        body.get("tags").is_some(),
        "case {case:?}: ListTagsForResourceOutput$tags is required, got {body}"
    );
    as_map(&body["tags"])
}

/// One taggable resource: how it is named in a failure message, the ARN the tagging
/// operations address it by, the tags its create persisted, and the write program this
/// row drives it through.
struct Taggable {
    kind: &'static str,
    arn: String,
    created_with: Value,
    /// The map `TagResource` adds in this row — at least one fresh key and, where the
    /// resource was created with tags, one key whose value is replaced.
    added: Value,
    /// The keys `UntagResource` removes in this row, including one the resource does not
    /// carry.
    removed: Vec<String>,
}

/// One resource of every kind this service persists an ARN for whose ARN fits
/// `CleanroomsArn`'s 100-character bound, each with its own tags and its own write
/// program.
///
/// A sub-resource ARN (an analysis template's, say) is longer than that bound, so it
/// cannot be sent as a `resourceArn` at all — see
/// [`a_resource_arn_past_the_published_bound_is_refused_before_the_handler`].
async fn taggable_resources(emulator: &Emulator) -> Vec<Taggable> {
    let mut resources = Vec::new();

    // A collaboration created WITH tags.
    let mut body = minimal_collaboration("tagged-collaboration");
    body["tags"] = json!({ "env": "test", "owner": "fixer" });
    let (status, created) = emulator.create_collaboration(body).await;
    assert_eq!(
        status, 200,
        "setup: expected CreateCollaboration 200, got {status} with {created}"
    );
    let collaboration_id = created["collaboration"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no collaboration.id in {created}"))
        .to_string();
    resources.push(Taggable {
        kind: "collaboration",
        arn: created["collaboration"]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("setup: no collaboration.arn in {created}"))
            .to_string(),
        created_with: json!({ "env": "test", "owner": "fixer" }),
        // One fresh key and one that replaces a value the create persisted.
        added: json!({ "env": "staging", "stage": "beta" }),
        removed: vec!["owner".to_string(), "never-set".to_string()],
    });

    // The caller's membership in that collaboration, created WITH one tag.
    let (status, joined) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": ALL_LOG_STATUSES[0],
            "tags": { "only": "one" },
        }))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateMembership 200, got {status} with {joined}"
    );
    let membership_id = joined["membership"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no membership.id in {joined}"))
        .to_string();
    resources.push(Taggable {
        kind: "membership",
        arn: joined["membership"]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("setup: no membership.arn in {joined}"))
            .to_string(),
        created_with: json!({ "only": "one" }),
        added: json!({ "second": "two" }),
        // Both of its keys, so this row ends with a resource carrying no tags at all.
        removed: vec!["only".to_string(), "second".to_string()],
    });

    // A configured table created WITHOUT tags: an untagged resource lists the empty map.
    let (status, table) = emulator
        .create_configured_table(create_body(
            "tagged-table",
            ALL_TABLE_REFERENCE_TYPES[0],
            ALL_ANALYSIS_METHODS[0],
            None,
            1,
        ))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateConfiguredTable 200, got {status} with {table}"
    );
    resources.push(Taggable {
        kind: "configured table",
        arn: table["configuredTable"]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("setup: no configuredTable.arn in {table}"))
            .to_string(),
        created_with: json!({}),
        added: json!({ "cost:center": "cc-1", "app/name+1": "search", "two words": "yes" }),
        removed: vec!["app/name+1".to_string(), "absent".to_string()],
    });

    // A privacy budget template of that membership, created WITH tags. Its ARN is the
    // longest this service mints that still fits `CleanroomsArn`.
    let (status, template) = emulator
        .call(
            "POST",
            &format!("/memberships/{membership_id}/privacybudgettemplates"),
            Some(json!({
                "privacyBudgetType": "DIFFERENTIAL_PRIVACY",
                "parameters": { "differentialPrivacy": { "epsilon": 7, "usersNoisePerQuery": 42 } },
                "tags": { "env": "test" },
            })),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreatePrivacyBudgetTemplate 200, got {status} with {template}"
    );
    resources.push(Taggable {
        kind: "privacy budget template",
        arn: template["privacyBudgetTemplate"]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("setup: no privacyBudgetTemplate.arn in {template}"))
            .to_string(),
        created_with: json!({ "env": "test" }),
        added: json!({ "env": "", "team": "analytics" }),
        removed: vec!["team".to_string()],
    });

    for resource in &resources {
        assert!(
            resource.arn.len() <= MAX_CLEANROOMS_ARN_LEN,
            "setup: the {} ARN must fit CleanroomsArn's {MAX_CLEANROOMS_ARN_LEN}-character bound, got {} for {:?}",
            resource.kind,
            resource.arn.len(),
            resource.arn
        );
    }
    resources
}

/// Every resource's tags, as the service reports them.
async fn all_listed_tags(
    emulator: &Emulator,
    case: &str,
    resources: &[Taggable],
) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut listed = BTreeMap::new();
    for resource in resources {
        listed.insert(
            resource.arn.clone(),
            listed_tags(emulator, case, &resource.arn).await,
        );
    }
    listed
}

#[tokio::test]
async fn tag_list_and_untag_share_one_tag_set_per_resource_for_every_kind_of_resource() {
    let emulator = Emulator::new();
    let resources = taggable_resources(&emulator).await;

    // What each resource must report, maintained alongside the service's own state.
    let mut expected: BTreeMap<String, BTreeMap<String, String>> = resources
        .iter()
        .map(|resource| (resource.arn.clone(), as_map(&resource.created_with)))
        .collect();

    // 1. A create's tags are what the list reports — including the empty map for the
    //    resource created without any.
    let listed = all_listed_tags(&emulator, "after the creates", &resources).await;
    assert_eq!(
        listed, expected,
        "after the creates: each resource must report the tags its create persisted; expected {expected:?}, got {listed:?}"
    );

    // 2. A read does not change what a later read sees.
    let relisted = all_listed_tags(&emulator, "a second read", &resources).await;
    assert_eq!(
        relisted, listed,
        "a second read must report the same tags; first {listed:?}, then {relisted:?}"
    );

    for resource in &resources {
        let case = format!("{}: TagResource {}", resource.kind, resource.added);

        let (status, body) = emulator.tag_resource(&resource.arn, resource.added.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected TagResource 200, got {status} with {body}"
        );

        // The added keys are carried, a key already carried takes the new value, and
        // every other key stays where it is.
        let entry = expected.get_mut(&resource.arn).expect("a tracked resource");
        entry.extend(as_map(&resource.added));
        let listed = all_listed_tags(&emulator, &case, &resources).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: tagging must add exactly its keys and leave every other resource alone; expected {expected:?}, got {listed:?}"
        );

        let case = format!("{}: UntagResource {:?}", resource.kind, resource.removed);
        let (status, body) = emulator.untag_resource(&resource.arn, &resource.removed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UntagResource 200, got {status} with {body}"
        );

        // Exactly the named keys go; a key the resource does not carry removes nothing.
        let entry = expected.get_mut(&resource.arn).expect("a tracked resource");
        for key in &resource.removed {
            entry.remove(key);
        }
        let listed = all_listed_tags(&emulator, &case, &resources).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: untagging must remove exactly the keys it names; expected {expected:?}, got {listed:?}"
        );

        // Removing the same keys again is idempotent: they are already gone.
        let case = format!("{}: UntagResource {:?} again", resource.kind, resource.removed);
        let (status, body) = emulator.untag_resource(&resource.arn, &resource.removed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UntagResource 200, got {status} with {body}"
        );
        let listed = all_listed_tags(&emulator, &case, &resources).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: a repeat removal must change nothing; expected {expected:?}, got {listed:?}"
        );
    }

    // 3. A resource whose whole tag set was removed accepts tags again, so an emptied
    //    resource is not a different resource.
    let emptied = resources
        .iter()
        .find(|resource| expected[&resource.arn].is_empty())
        .expect("at least one row ends with no tags");
    let case = format!("{}: tagged again after being emptied", emptied.kind);
    let (status, body) = emulator
        .tag_resource(&emptied.arn, json!({ "after": "empty" }))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected TagResource 200, got {status} with {body}"
    );
    let listed = listed_tags(&emulator, &case, &emptied.arn).await;
    assert_eq!(
        listed,
        BTreeMap::from([("after".to_string(), "empty".to_string())]),
        "case {case:?}: expected the newly added tag only, got {listed:?}"
    );
}

#[tokio::test]
async fn tag_keys_and_values_at_the_published_bounds_round_trip() {
    let emulator = Emulator::new();
    let (status, created) = emulator
        .create_collaboration(minimal_collaboration("bounds-collaboration"))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateCollaboration 200, got {status} with {created}"
    );
    let arn = created["collaboration"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no collaboration.arn in {created}"))
        .to_string();

    // Rows span the tag shapes' published domain: both `TagKey` bounds, both `TagValue`
    // bounds, one value past neither bound, keys outside RFC 3986's unreserved set, a
    // multi-key map, and the empty map `TagMap` admits (len 0..200).
    let rows: Vec<(&str, Value)> = vec![
        (
            "a single-character key (TagKey min)",
            json!({ "a".repeat(TAG_KEY_MIN): "one" }),
        ),
        (
            "a key at TagKey's maximum length",
            json!({ "k".repeat(TAG_KEY_MAX): "long-key" }),
        ),
        (
            "an empty value (TagValue min)",
            json!({ "empty": "v".repeat(TAG_VALUE_MIN) }),
        ),
        (
            "a value at TagValue's maximum length",
            json!({ "long": "v".repeat(TAG_VALUE_MAX) }),
        ),
        (
            "keys that need percent-encoding in the untag query",
            json!({ "cost:center": "cc-1", "app/name+1": "search", "two words": "yes" }),
        ),
        ("the empty map", json!({})),
    ];

    for (case, tags) in rows {
        let before = listed_tags(&emulator, case, &arn).await;

        let (status, body) = emulator.tag_resource(&arn, tags.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected TagResource 200 for {tags}, got {status} with {body}"
        );

        let mut want = before.clone();
        want.extend(as_map(&tags));
        let listed = listed_tags(&emulator, case, &arn).await;
        assert_eq!(
            listed, want,
            "case {case:?}: expected {want:?}, got {listed:?}"
        );

        // Removing the row's own keys returns the resource to where it started, so each
        // row is independent of the ones before it.
        let keys: Vec<String> = as_map(&tags).keys().cloned().collect();
        if !keys.is_empty() {
            let (status, body) = emulator.untag_resource(&arn, &keys).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UntagResource 200 for {keys:?}, got {status} with {body}"
            );
        }
        let listed = listed_tags(&emulator, case, &arn).await;
        assert_eq!(
            listed, before,
            "case {case:?}: removing the row's keys must restore {before:?}, got {listed:?}"
        );
    }
}

/// One ARN that addresses no stored resource, with the `resourceType` a refusal about it
/// must report.
struct UnheldArn {
    arn: String,
    resource_type: &'static str,
}

/// An ARN of every kind of resource the model declares an ARN shape for, built around an
/// id nothing minted. The resource segments come from those shapes' patterns
/// (`CollaborationArn` `:collaboration/<id>`, `ConfiguredTableAssociationArn`
/// `:configuredtableassociation/<id>/<id>`, `AnalysisTemplateArn`
/// `:membership/<id>/analysistemplate/<id>`, and so on for the other sub-resources).
fn unheld_arns() -> Vec<UnheldArn> {
    let prefix = format!("arn:aws:cleanrooms:{REGION}:{}:", account_id());
    // Short ids keep every row inside `CleanroomsArn`'s 100-character bound, which a
    // sub-resource ARN carrying two UUIDs would exceed.
    let rows: [(&str, &str); 10] = [
        ("collaboration/c1", "COLLABORATION"),
        ("membership/m1", "MEMBERSHIP"),
        ("configuredtable/t1", "CONFIGURED_TABLE"),
        (
            "configuredtableassociation/m1/a1",
            "CONFIGURED_TABLE_ASSOCIATION",
        ),
        ("membership/m1/analysistemplate/a1", "ANALYSIS_TEMPLATE"),
        ("privacybudgettemplate/p1", "PRIVACY_BUDGET_TEMPLATE"),
        (
            "membership/m1/idnamespaceassociation/i1",
            "ID_NAMESPACE_ASSOCIATION",
        ),
        ("membership/m1/idmappingtable/i1", "ID_MAPPING_TABLE"),
        (
            "membership/m1/configuredaudiencemodelassociation/c1",
            "CONFIGURED_AUDIENCE_MODEL_ASSOCIATION",
        ),
        ("membership/m1/intermediatetable/i1", "INTERMEDIATE_TABLE"),
    ];
    let mut arns: Vec<UnheldArn> = rows
        .iter()
        .map(|(segment, resource_type)| UnheldArn {
            arn: format!("{prefix}{segment}"),
            resource_type,
        })
        .collect();
    // The same kinds addressed with full `UUID` ids, so the rows are not only short ids.
    for id in NEVER_MINTED_IDS {
        arns.push(UnheldArn {
            arn: format!("{prefix}collaboration/{id}"),
            resource_type: "COLLABORATION",
        });
        arns.push(UnheldArn {
            arn: format!("{prefix}membership/{id}"),
            resource_type: "MEMBERSHIP",
        });
    }
    arns
}

#[tokio::test]
async fn every_tagging_operation_refuses_an_arn_no_resource_is_stored_under() {
    let emulator = Emulator::new();
    // One tagged resource exists, so each refusal below is about the ARN rather than
    // about an empty store, and the final check can show nothing else moved.
    let resources = taggable_resources(&emulator).await;
    let held = &resources[0];
    let held_tags = listed_tags(&emulator, "the held resource", &held.arn).await;

    for unheld in unheld_arns() {
        assert!(
            unheld.arn.len() <= MAX_CLEANROOMS_ARN_LEN,
            "case {:?}: the row must fit CleanroomsArn's {MAX_CLEANROOMS_ARN_LEN}-character bound, got {}",
            unheld.arn,
            unheld.arn.len()
        );
        assert!(
            !resources.iter().any(|resource| resource.arn == unheld.arn),
            "case {:?}: an unheld ARN must not be one of the created resources",
            unheld.arn
        );

        let (status, listed) = emulator.list_tags(&unheld.arn).await;
        let (status_tag, tagged) = emulator
            .tag_resource(&unheld.arn, json!({ "env": "test" }))
            .await;
        let (status_untag, untagged) = emulator
            .untag_resource(&unheld.arn, &["env".to_string()])
            .await;

        for (operation, status, body) in [
            ("ListTagsForResource", status, &listed),
            ("TagResource", status_tag, &tagged),
            ("UntagResource", status_untag, &untagged),
        ] {
            let case = format!("{operation} on {}", unheld.arn);
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} with {body}"
            );
            assert_eq!(
                error_code(body),
                TAGGING_NOT_FOUND_CODE,
                "case {case:?}: expected {TAGGING_NOT_FOUND_CODE}, got {body}"
            );
            assert_eq!(
                body["resourceId"].as_str(),
                Some(unheld.arn.as_str()),
                "case {case:?}: expected resourceId {:?}, got {body}",
                unheld.arn
            );
            assert_eq!(
                body["resourceType"].as_str(),
                Some(unheld.resource_type),
                "case {case:?}: expected resourceType {:?}, got {body}",
                unheld.resource_type
            );
            assert_eq!(
                error_message(body),
                TAGGED_RESOURCE_NOT_FOUND,
                "case {case:?}: the tagging plane's refusal is fixed text, got {:?}",
                error_message(body)
            );
        }

        // The refused write persisted nothing: the ARN still holds no resource, so a
        // later create of it could not inherit tags from a call that failed.
        let (status, body) = emulator.list_tags(&unheld.arn).await;
        assert_eq!(
            status, 404,
            "case {:?}: a refused TagResource must leave the ARN holding nothing, got {status} with {body}",
            unheld.arn
        );
    }

    let listed = listed_tags(&emulator, "the held resource", &held.arn).await;
    assert_eq!(
        listed, held_tags,
        "the refusals must leave the held resource's tags at {held_tags:?}, got {listed:?}"
    );
}

#[tokio::test]
async fn an_arn_naming_no_kind_of_clean_rooms_resource_is_a_validation_error() {
    let emulator = Emulator::new();
    let prefix = format!("arn:aws:cleanrooms:{REGION}:{}:", account_id());

    // Resource segments inside `CleanroomsArn`'s pattern (`[\d\w/-]+`) that name no kind
    // of Clean Rooms resource: an unknown kind, a bare id with no kind at all, the plural
    // of a kind, and a nesting whose every segment is unknown.
    let rows = [
        "widget/w1",
        "c1",
        "collaborations/c1",
        "foo/f1/bar/b1",
    ];

    for segment in rows {
        let arn = format!("{prefix}{segment}");
        let (status, listed) = emulator.list_tags(&arn).await;
        let (status_tag, tagged) = emulator.tag_resource(&arn, json!({ "env": "test" })).await;
        let (status_untag, untagged) = emulator
            .untag_resource(&arn, &["env".to_string()])
            .await;

        for (operation, status, body) in [
            ("ListTagsForResource", status, &listed),
            ("TagResource", status_tag, &tagged),
            ("UntagResource", status_untag, &untagged),
        ] {
            let case = format!("{operation} on {arn}");
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            assert!(
                error_message(body).contains(&arn),
                "case {case:?}: the message must name the ARN it refused, got {:?}",
                error_message(body)
            );
            assert_eq!(
                body["fieldList"][0]["name"].as_str(),
                Some("resourceArn"),
                "case {case:?}: the refusal must name the member that failed, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_resource_arn_past_the_published_bound_is_refused_before_the_handler() {
    let emulator = Emulator::new();

    // An analysis template's own ARN (`AnalysisTemplateArn`, len ..200) is longer than
    // `CleanroomsArn`'s 100-character bound, which is the shape every tagging
    // operation's `resourceArn` targets, so the boundary refuses it as a bad request
    // before any handler runs.
    let (status, created) = emulator
        .create_collaboration(minimal_collaboration("arn-bound-collaboration"))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateCollaboration 200, got {status} with {created}"
    );
    let collaboration_id = created["collaboration"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no collaboration.id in {created}"))
        .to_string();
    let (status, joined) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": ALL_LOG_STATUSES[0],
        }))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateMembership 200, got {status} with {joined}"
    );
    let membership_id = joined["membership"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no membership.id in {joined}"))
        .to_string();
    let (status, template) = emulator
        .call(
            "POST",
            &format!("/memberships/{membership_id}/analysistemplates"),
            Some(json!({
                "name": "bounded",
                "format": "SQL",
                "source": { "text": "SELECT id FROM sales" },
                "tags": { "env": "test" },
            })),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateAnalysisTemplate 200, got {status} with {template}"
    );
    let arn = template["analysisTemplate"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no analysisTemplate.arn in {template}"))
        .to_string();
    assert!(
        arn.len() > MAX_CLEANROOMS_ARN_LEN,
        "an analysis template ARN is longer than CleanroomsArn's {MAX_CLEANROOMS_ARN_LEN}-character bound; got {} for {arn:?}",
        arn.len()
    );

    let (status, body) = emulator.list_tags(&arn).await;
    assert_eq!(
        status, 400,
        "a resourceArn past CleanroomsArn's bound must be refused with 400, got {status} with {body}"
    );
    assert!(
        body.get("tags").is_none(),
        "a refused ListTagsForResource must report no tags, got {body}"
    );
}

#[tokio::test]
async fn a_resource_cannot_be_tagged_past_what_a_tag_map_can_hold() {
    let emulator = Emulator::new();
    let (status, created) = emulator
        .create_collaboration(minimal_collaboration("tag-count-collaboration"))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateCollaboration 200, got {status} with {created}"
    );
    let arn = created["collaboration"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no collaboration.arn in {created}"))
        .to_string();

    // `TagMap` is len 0..200, and the listed tags ARE a `TagMap`, so a resource can carry
    // at most that many: the whole map at the bound is accepted.
    let at_bound: Value = Value::Object(
        (0..TAG_MAP_MAX)
            .map(|index| (format!("key-{index}"), Value::String(format!("value-{index}"))))
            .collect(),
    );
    let (status, body) = emulator.tag_resource(&arn, at_bound.clone()).await;
    assert_eq!(
        status, 200,
        "a map at TagMap's maximum of {TAG_MAP_MAX} must be accepted, got {status} with {body}"
    );
    let listed = listed_tags(&emulator, "at the bound", &arn).await;
    assert_eq!(
        listed.len(),
        TAG_MAP_MAX,
        "at the bound: expected {TAG_MAP_MAX} tags, got {}",
        listed.len()
    );

    // Replacing the value of a key already carried does not grow the set, so it is still
    // accepted at the bound.
    let (status, body) = emulator
        .tag_resource(&arn, json!({ "key-0": "replaced" }))
        .await;
    assert_eq!(
        status, 200,
        "replacing a carried key's value must be accepted at the bound, got {status} with {body}"
    );
    let listed = listed_tags(&emulator, "a replacement at the bound", &arn).await;
    assert_eq!(
        listed.get("key-0").map(String::as_str),
        Some("replaced"),
        "a replacement at the bound: expected the new value, got {listed:?}"
    );
    assert_eq!(
        listed.len(),
        TAG_MAP_MAX,
        "a replacement at the bound: the set must not grow, got {}",
        listed.len()
    );

    // One key past the bound is refused, and the refusal changes nothing.
    let (status, body) = emulator
        .tag_resource(&arn, json!({ "one-too-many": "no" }))
        .await;
    assert_eq!(
        status, 400,
        "a key past TagMap's maximum of {TAG_MAP_MAX} must be refused with 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected ValidationException, got {body}"
    );
    assert_eq!(
        body["fieldList"][0]["name"].as_str(),
        Some("tags"),
        "the refusal must name the member that failed, got {body}"
    );
    let after = listed_tags(&emulator, "after the refusal", &arn).await;
    assert_eq!(
        after, listed,
        "after the refusal: the tags must be unchanged at {listed:?}, got {after:?}"
    );

    // Making room first is all it takes, so the bound is on the SET and not on the ARN.
    let (status, body) = emulator.untag_resource(&arn, &["key-0".to_string()]).await;
    assert_eq!(
        status, 200,
        "expected UntagResource 200, got {status} with {body}"
    );
    let (status, body) = emulator
        .tag_resource(&arn, json!({ "one-too-many": "now-fits" }))
        .await;
    assert_eq!(
        status, 200,
        "with room made, the same key must be accepted, got {status} with {body}"
    );
    let listed = listed_tags(&emulator, "after making room", &arn).await;
    assert_eq!(
        listed.get("one-too-many").map(String::as_str),
        Some("now-fits"),
        "after making room: expected the added tag, got {listed:?}"
    );
    assert_eq!(
        listed.len(),
        TAG_MAP_MAX,
        "after making room: expected {TAG_MAP_MAX} tags, got {}",
        listed.len()
    );
}

#[tokio::test]
async fn a_deleted_resource_is_no_longer_taggable_and_keeps_no_tags() {
    let emulator = Emulator::new();

    let mut body = minimal_collaboration("deleted-collaboration");
    body["tags"] = json!({ "env": "test" });
    let (status, created) = emulator.create_collaboration(body).await;
    assert_eq!(
        status, 200,
        "setup: expected CreateCollaboration 200, got {status} with {created}"
    );
    let id = created["collaboration"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no collaboration.id in {created}"))
        .to_string();
    let arn = created["collaboration"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no collaboration.arn in {created}"))
        .to_string();

    let listed = listed_tags(&emulator, "before the delete", &arn).await;
    assert_eq!(
        listed,
        BTreeMap::from([("env".to_string(), "test".to_string())]),
        "before the delete: expected the created tags, got {listed:?}"
    );

    let (status, deleted) = emulator.delete_collaboration(&id).await;
    assert_eq!(
        status, 204,
        "expected DeleteCollaboration 204, got {status} with {deleted}"
    );

    let (status, body) = emulator.list_tags(&arn).await;
    assert_eq!(
        status, 404,
        "a deleted resource's ARN holds nothing, so listing its tags must be 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        TAGGING_NOT_FOUND_CODE,
        "expected {TAGGING_NOT_FOUND_CODE}, got {body}"
    );

    let (status, body) = emulator.tag_resource(&arn, json!({ "env": "new" })).await;
    assert_eq!(
        status, 404,
        "a deleted resource cannot be tagged, got {status} with {body}"
    );
}
