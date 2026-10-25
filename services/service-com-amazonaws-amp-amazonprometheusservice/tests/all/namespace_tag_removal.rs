// SPDX-License-Identifier: CC0-1.0

//! `UntagResource` against a LIVE rule groups namespace, end to end over HTTP.
//!
//! The rule under test: the ARN-addressed tagging operations identify their target by
//! the ARN the resource's OWN reader reports, for every kind of resource the store
//! holds — "The only resources that can be tagged are rule groups namespaces, scrapers,
//! and workspaces" (`UntagResource`) — so `UntagResource` removes the keys it names from
//! THAT resource's stored tag map and answers 200, and, being `@idempotent`, answers 200
//! again for a key the resource no longer carries; only an ARN that matches no stored
//! resource is refused.
//!
//! `tests/all/taggable_resource_kinds.rs` spans the resource KINDS; this suite spans what
//! the rule generalises over for the namespace the issue reported — the shape of the
//! `tagKeys` list, each key across the modelled `TagKey` bounds and character class, and
//! the ARN's identity (two workspaces holding a namespace of the same name, and a
//! namespace the clock has released).
//!
//! The request harness (`Amp`, `text`, `tags_of`, `wait_for_due_transition`) is the one
//! `tests/all/workspace_lifecycle.rs` builds, and the create/describe calls are
//! `tests/all/rule_groups_namespace_lifecycle.rs`'s.

use crate::rule_groups_namespace_lifecycle::{create_body, create_namespace, describe_namespace};
use crate::workspace_lifecycle::{tags_of, text, wait_for_due_transition, Amp};
use service_com_amazonaws_amp_amazonprometheusservice::state::RESOURCE_NOT_FOUND_CODE;
use std::collections::BTreeMap;

const REGION: &str = "us-west-2";

/// A rules file every namespace here is created with: the bytes are irrelevant to
/// tagging, so one body serves every row.
const RULES: &[u8] = b"groups: []\n";

/// The tag keys a namespace is seeded with, spanning the modelled `TagKey` (length 1..128,
/// pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, from
/// `smithy_query(query="shape",shape="com.amazonaws.amp#TagKey")`): a plain word, the
/// length minimum of 1, the length maximum of 128, and one carrying every punctuation and
/// separator the pattern allows.
fn seed_keys() -> Vec<String> {
    vec![
        "team".to_string(),
        "a".to_string(),
        "k".repeat(128),
        "cost:center/eu-west=1+a@b_c.d e".to_string(),
        "owner".to_string(),
    ]
}

/// The seeded map: each key above with a value derived from it, so a wrong key's removal
/// shows up as a wrong value too.
fn seed_tags() -> Vec<(String, String)> {
    seed_keys()
        .into_iter()
        .enumerate()
        .map(|(index, key)| (key, format!("value-{index}")))
        .collect()
}

/// Create a namespace carrying `tags`, let the clock settle it, and answer with the ARN
/// **its own reader reports** — the string the tagging path has to resolve.
async fn live_namespace(
    case: &str,
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    name: &str,
    tags: &[(String, String)],
) -> String {
    let created = create_namespace(amp, region, workspace_id, &create_body(name, RULES, tags)).await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: CreateRuleGroupsNamespace must answer its modelled 202, got {created:?}"
    );
    wait_for_due_transition();
    let described = describe_namespace(amp, region, workspace_id, name).await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: DescribeRuleGroupsNamespace must answer 200, got {described:?}"
    );
    let arn = text(&described.body["ruleGroupsNamespace"]["arn"]);
    assert!(
        !arn.is_empty(),
        "case {case:?}: the namespace's reader must report its ARN, got {described:?}"
    );
    arn
}

/// The tags the namespace's own reader reports, which must agree with what
/// `ListTagsForResource` reports for the same ARN.
async fn namespace_tags(
    case: &str,
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    name: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let described = describe_namespace(amp, region, workspace_id, name).await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: DescribeRuleGroupsNamespace must answer 200, got {described:?}"
    );
    let reported = tags_of(&described.body["ruleGroupsNamespace"]["tags"]);
    let listed = amp.list_tags(region, arn).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: ListTagsForResource must answer 200 for a live namespace, got {listed:?}"
    );
    assert_eq!(
        tags_of(&listed.body["tags"]),
        reported,
        "case {case:?}: ListTagsForResource must report the map the namespace's own reader \
         reports ({reported:?}), got {listed:?}"
    );
    reported
}

/// One row: the `tagKeys` one `UntagResource` call names.
struct Row {
    case: &'static str,
    keys: Vec<String>,
}

/// The rows span the shape of the `tagKeys` list — one key, several, a key the namespace
/// does not carry, a key repeated in one call, keys at each modelled `TagKey` length bound
/// and across its character class, and every key the namespace carries.
fn removal_rows() -> Vec<Row> {
    let keys = seed_keys();
    vec![
        Row {
            case: "one key the namespace carries",
            keys: vec![keys[0].clone()],
        },
        Row {
            case: "several keys in one call",
            keys: vec![keys[1].clone(), keys[4].clone()],
        },
        Row {
            case: "a key the namespace does not carry",
            keys: vec!["never-set".to_string()],
        },
        Row {
            case: "a mix of a carried and an uncarried key",
            keys: vec![keys[0].clone(), "never-set".to_string()],
        },
        Row {
            case: "the key at the length minimum of 1",
            keys: vec![keys[1].clone()],
        },
        Row {
            case: "the key at the length maximum of 128",
            keys: vec![keys[2].clone()],
        },
        Row {
            case: "the key carrying every character the pattern allows",
            keys: vec![keys[3].clone()],
        },
        Row {
            case: "the same key named twice in one call",
            keys: vec![keys[0].clone(), keys[0].clone()],
        },
        Row {
            case: "every key the namespace carries",
            keys: keys.clone(),
        },
    ]
}

/// `DELETE /tags/{namespace ARN}?tagKeys=…` removes exactly the keys it names from the
/// namespace's stored tag map, answers 200, and answers 200 again — changing nothing — when
/// replayed for keys the namespace no longer carries.
#[tokio::test]
async fn untagging_a_live_namespace_removes_the_keys_it_names() {
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(REGION, "namespace-untagging").await;
    let seeded = seed_tags();

    for (index, row) in removal_rows().iter().enumerate() {
        let name = format!("rulesfile{index}");
        let arn = live_namespace(row.case, &amp, REGION, &workspace_id, &name, &seeded).await;

        let before = namespace_tags(row.case, &amp, REGION, &workspace_id, &name, &arn).await;
        let expected: BTreeMap<String, String> = before
            .iter()
            .filter(|(key, _)| !row.keys.contains(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();

        let borrowed: Vec<&str> = row.keys.iter().map(String::as_str).collect();
        let removed = amp.untag(REGION, &arn, &borrowed).await;
        assert_eq!(
            removed.status, 200,
            "case {:?}: UntagResource on a live namespace must answer 200, got {removed:?}",
            row.case
        );

        let after = namespace_tags(row.case, &amp, REGION, &workspace_id, &name, &arn).await;
        assert_eq!(
            after, expected,
            "case {:?}: the namespace must carry {expected:?} afterwards, got {after:?}",
            row.case
        );

        // `@idempotent`: the same removal again is not an error and changes nothing.
        let again = amp.untag(REGION, &arn, &borrowed).await;
        assert_eq!(
            again.status, 200,
            "case {:?}: replaying the removal must answer 200, got {again:?}",
            row.case
        );
        let settled = namespace_tags(row.case, &amp, REGION, &workspace_id, &name, &arn).await;
        assert_eq!(
            settled, expected,
            "case {:?}: the replay must leave {expected:?} in place, got {settled:?}",
            row.case
        );
    }
}

/// The ARN names ONE resource: two workspaces each holding a namespace of the same name
/// are different resources, so untagging one leaves the other's map exactly as it was.
#[tokio::test]
async fn untagging_one_namespace_leaves_its_namesake_in_another_workspace_alone() {
    let amp = Amp::new();
    let seeded = seed_tags();
    let expected_seeded: BTreeMap<String, String> = seeded.iter().cloned().collect();
    let name = "rulesfile1";

    let first = amp.active_workspace(REGION, "first-host").await;
    let second = amp.active_workspace(REGION, "second-host").await;
    let first_arn = live_namespace("the first workspace", &amp, REGION, &first, name, &seeded).await;
    let second_arn =
        live_namespace("the second workspace", &amp, REGION, &second, name, &seeded).await;
    assert_ne!(
        first_arn, second_arn,
        "case \"two namesakes\": each workspace's namespace must have its own ARN, \
         got {first_arn:?} twice"
    );

    let removed = amp.untag(REGION, &first_arn, &["team", "owner"]).await;
    assert_eq!(
        removed.status, 200,
        "case \"two namesakes\": the removal must answer 200, got {removed:?}"
    );

    let mut expected_first = expected_seeded.clone();
    expected_first.remove("team");
    expected_first.remove("owner");
    let after_first =
        namespace_tags("two namesakes / first", &amp, REGION, &first, name, &first_arn).await;
    assert_eq!(
        after_first, expected_first,
        "case \"two namesakes\": the addressed namespace must carry {expected_first:?}, \
         got {after_first:?}"
    );
    let after_second = namespace_tags(
        "two namesakes / second",
        &amp,
        REGION,
        &second,
        name,
        &second_arn,
    )
    .await;
    assert_eq!(
        after_second, expected_seeded,
        "case \"two namesakes\": the namesake in the other workspace must be untouched \
         ({expected_seeded:?}), got {after_second:?}"
    );
}

/// An ARN resolves only while the resource it names is stored: once the clock has released
/// a deleted namespace its ARN is refused, while the sibling that survived still resolves.
#[tokio::test]
async fn a_released_namespaces_arn_is_refused_while_its_siblings_still_resolves() {
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(REGION, "release-host").await;
    let seeded = seed_tags();
    let doomed_arn =
        live_namespace("the deleted namespace", &amp, REGION, &workspace_id, "doomed", &seeded)
            .await;
    let kept_arn =
        live_namespace("the surviving namespace", &amp, REGION, &workspace_id, "kept", &seeded)
            .await;

    let deleted = amp
        .call(
            "DELETE",
            &format!("/workspaces/{workspace_id}/rulegroupsnamespaces/doomed"),
            REGION,
            "",
        )
        .await;
    assert_eq!(
        deleted.status, 202,
        "case \"a released namespace\": DeleteRuleGroupsNamespace must answer 202, got {deleted:?}"
    );
    wait_for_due_transition();

    let refused = amp.untag(REGION, &doomed_arn, &["team"]).await;
    assert_eq!(
        refused.status, 404,
        "case \"a released namespace\": its ARN must be refused 404, got {refused:?}"
    );
    assert_eq!(
        refused.error_code(),
        RESOURCE_NOT_FOUND_CODE,
        "case \"a released namespace\": expected the code this service answers on the \
         ARN-addressed path, got {refused:?}"
    );

    let removed = amp.untag(REGION, &kept_arn, &["team"]).await;
    assert_eq!(
        removed.status, 200,
        "case \"the surviving namespace\": its ARN must still resolve, got {removed:?}"
    );
    let mut expected: BTreeMap<String, String> = seeded.iter().cloned().collect();
    expected.remove("team");
    let after = namespace_tags(
        "the surviving namespace",
        &amp,
        REGION,
        &workspace_id,
        "kept",
        &kept_arn,
    )
    .await;
    assert_eq!(
        after, expected,
        "case \"the surviving namespace\": it must carry {expected:?}, got {after:?}"
    );
}
