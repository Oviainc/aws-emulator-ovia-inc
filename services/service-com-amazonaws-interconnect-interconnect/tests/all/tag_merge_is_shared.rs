// SPDX-License-Identifier: CC0-1.0

//! The ONE merge both of this service's tag writers apply, driven over the wire through
//! the shared [`crate::emulator`] harness.
//!
//! RULE under test: a write of a tag set MERGES, never replaces, whichever operation
//! makes it — the tagging operation or the acceptance that writes tags onto the
//! connection it accepts: every key the request carries is written over whatever the
//! resource held for it, every key the request does not name keeps the value it had, and
//! a request carrying no tag at all adds nothing and leaves "no tags" in the single
//! representation the store holds it in (the `tags` member absent), so the set a resource
//! is left holding depends only on what it held and what the request carried — not on
//! which writer wrote it, nor on how the keys were split across calls. The bound on how
//! many tags one resource holds is judged against that same merged set, so rewriting
//! held keys is accepted even at the bound while a write that would leave the resource
//! holding more is refused with nothing written — again on both writers alike.
//!
//! The rows differ along what the rule generalises over: how many tags the resource
//! already holds (none, one, two, ten), how many the write carries (the member left out,
//! a map carrying no entries, one, two, ten), whether a carried key is fresh, already
//! held with another value or already held with the same value, the value written (plain,
//! empty — `TagValue` allows a zero-length value), the key length (each modelled bound),
//! and the environment and bandwidth the resource sits on.
//!
//! The lengths come from the model:
//! `smithy_query(query="shape",shape="TagKey,TagValue")` gives `TagKey`
//! `@length(min: 1, max: 128)` and `TagValue` `@length(min: 0, max: 256)`.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::emulator::{
    code, create_with_tag_member, message, placements, tags_of, text, Emulator,
    DEFAULT_SIGNING_REGION,
};

/// `TagKey` is modelled `@length(min: 1, max: 128)`, so these are the shortest and the
/// longest key a row can carry.
const TAG_KEY_MIN: usize = 1;
const TAG_KEY_MAX: usize = 128;

/// One row: the tag set the resource already holds, and the `tags` member the write
/// carries — `None` for the member left out of the request altogether.
#[derive(Debug, Clone)]
struct MergeCase {
    name: String,
    held: BTreeMap<String, String>,
    supplied: Option<BTreeMap<String, String>>,
}

impl MergeCase {
    fn new(name: &str, held: &[(&str, &str)], supplied: Option<&[(&str, &str)]>) -> Self {
        Self {
            name: name.to_string(),
            held: pairs(held),
            supplied: supplied.map(pairs),
        }
    }

    /// What the rule says the resource is left holding: the set it held with every
    /// carried key written over it, each unnamed key keeping its value.
    fn expected(&self) -> BTreeMap<String, String> {
        let mut expected = self.held.clone();
        if let Some(supplied) = self.supplied.as_ref() {
            expected.extend(
                supplied
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        expected
    }

    /// The `tags` member the create carries to put the row's held set in place, left out
    /// for a row that holds none.
    fn held_member(&self) -> Option<Value> {
        (!self.held.is_empty()).then(|| json!(self.held))
    }

    /// The `tags` member the tagging operation sends for this row. That member is
    /// REQUIRED there (`smithy_query(query="operation",operation="TagResource")`), so a
    /// row whose acceptance leaves it out sends the other spelling of the same state
    /// here — a map carrying no entries, which asks for the same write.
    fn tagging_member(&self) -> Value {
        json!(self.supplied.clone().unwrap_or_default())
    }
}

/// A `(key, value)` table as the map the wire carries.
fn pairs(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// The table both writers are driven over.
fn merge_table() -> Vec<MergeCase> {
    let shortest_key = "k".repeat(TAG_KEY_MIN);
    let longest_key = "k".repeat(TAG_KEY_MAX);
    let mut rows = vec![
        MergeCase::new("nothing held, no tags member at all", &[], None),
        MergeCase::new("nothing held, a member carrying no entries", &[], Some(&[])),
        MergeCase::new("two keys held, no tags member at all", &[("env", "test"), ("team", "network")], None),
        MergeCase::new(
            "two keys held, a member carrying no entries",
            &[("env", "test"), ("team", "network")],
            Some(&[]),
        ),
        MergeCase::new("one fresh key onto nothing held", &[], Some(&[("env", "test")])),
        MergeCase::new(
            "one fresh key onto a held key",
            &[("team", "network")],
            Some(&[("env", "test")]),
        ),
        MergeCase::new(
            "a held key takes the carried value",
            &[("env", "staging"), ("team", "network")],
            Some(&[("env", "production")]),
        ),
        MergeCase::new(
            "a held key rewritten with the value it already has",
            &[("env", "test")],
            Some(&[("env", "test")]),
        ),
        MergeCase::new(
            "a fresh key and an overwriting one together",
            &[("env", "staging"), ("owner", "ops")],
            Some(&[("env", "production"), ("stage", "live")]),
        ),
        MergeCase::new(
            "a held key rewritten to an empty value",
            &[("env", "test"), ("owner", "ops")],
            Some(&[("env", "")]),
        ),
    ];
    // Keys at each modelled length bound, the shorter of them already held — so the
    // bounds are exercised on both halves of the merge.
    rows.push(MergeCase {
        name: "keys at each modelled length bound, the shortest already held".to_string(),
        held: BTreeMap::from([(shortest_key.clone(), "held".to_string())]),
        supplied: Some(BTreeMap::from([
            (shortest_key, "rewritten".to_string()),
            (longest_key, "at the maximum".to_string()),
        ])),
    });
    // A size no other row reaches, with part of what is carried overlapping what is
    // held: ten keys written over ten held keys, four of them the same key.
    rows.push(MergeCase {
        name: "ten keys carried onto ten held keys, four of them overlapping".to_string(),
        held: (0..10)
            .map(|index| (format!("key-{index:02}"), format!("held-{index:02}")))
            .collect(),
        supplied: Some(
            (6..16)
                .map(|index| (format!("key-{index:02}"), format!("carried-{index:02}")))
                .collect(),
        ),
    });
    rows
}

/// The `tags` member of a response verbatim — the WIRE FORM of a tag set, which
/// `crate::emulator::tags_of` deliberately flattens.
fn tag_member(value: &Value) -> Value {
    value["tags"].clone()
}

/// What a writer LEFT: the resource it wrote, the stored tag set, the wire form the
/// store reports it in, and what the tag listing reports for the same resource.
#[derive(Debug, Clone)]
struct Written {
    id: String,
    arn: String,
    tags: BTreeMap<String, String>,
    member: Value,
    listed: BTreeMap<String, String>,
}

/// `ListTagsForResource` for `arn`, required to succeed, as the tag set it reports.
async fn listed_tags(emulator: &Emulator, arn: &str, case: &str) -> BTreeMap<String, String> {
    let (status, body) = emulator
        .ic("ListTagsForResource", json!({ "arn": arn }))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListTagsForResource, got {status} with {body:?}"
    );
    tags_of(&body)
}

/// Everything the store reports about the tag set of the connection `id`/`arn` names.
async fn written(emulator: &Emulator, id: &str, arn: &str, case: &str) -> Written {
    let (status, body) = emulator.get_connection(id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetConnection, got {status} with {body:?}"
    );
    let connection = body["connection"].clone();
    Written {
        id: id.to_string(),
        arn: arn.to_string(),
        tags: tags_of(&connection),
        member: tag_member(&connection),
        listed: listed_tags(emulator, arn, case).await,
    }
}

/// `TagResource` for `arn`, carrying `tags` verbatim, required to succeed.
async fn tag(emulator: &Emulator, arn: &str, tags: &Value, case: &str) {
    let (status, body) = emulator
        .ic("TagResource", json!({ "arn": arn, "tags": tags }))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from TagResource, got {status} with {body:?}"
    );
}

/// A connection holding the row's tag set, as `(id, arn)`.
async fn holding(
    emulator: &Emulator,
    placement: &(String, String),
    seed: usize,
    case: &MergeCase,
) -> (String, String) {
    let created = create_with_tag_member(
        emulator,
        DEFAULT_SIGNING_REGION,
        placement,
        seed,
        case.held_member().as_ref(),
    )
    .await;
    (text(&created, "id"), text(&created, "arn"))
}

/// The row's write made by the TAGGING writer, and what it left.
async fn through_tagging(
    emulator: &Emulator,
    placement: &(String, String),
    seed: usize,
    case: &MergeCase,
) -> Written {
    let (id, arn) = holding(emulator, placement, seed, case).await;
    tag(emulator, &arn, &case.tagging_member(), &case.name).await;
    written(emulator, &id, &arn, &case.name).await
}

/// The row's write made by the ACCEPTANCE writer — the same tag set carried on the call
/// that accepts the proposal — and what it left.
async fn through_acceptance(
    emulator: &Emulator,
    placement: &(String, String),
    seed: usize,
    case: &MergeCase,
    gateway: &str,
) -> Written {
    let proposal = create_with_tag_member(
        emulator,
        DEFAULT_SIGNING_REGION,
        placement,
        seed,
        case.held_member().as_ref(),
    )
    .await;
    let mut request = Map::new();
    request.insert(
        "activationKey".to_string(),
        json!(text(&proposal, "activationKey")),
    );
    request.insert(
        "attachPoint".to_string(),
        json!({ "directConnectGateway": gateway }),
    );
    // The member is OPTIONAL on this writer, so the row that leaves it out is sent
    // exactly as the row asks.
    if let Some(supplied) = case.supplied.as_ref() {
        request.insert("tags".to_string(), json!(supplied));
    }
    let (status, body) = emulator
        .ic("AcceptConnectionProposal", Value::Object(request))
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from AcceptConnectionProposal, got {status} with {body:?}",
        case.name
    );
    let accepted = body["connection"].clone();
    let left = written(
        emulator,
        &text(&accepted, "id"),
        &text(&accepted, "arn"),
        &case.name,
    )
    .await;
    assert_eq!(
        left.member,
        tag_member(&accepted),
        "case {:?}: expected the store to hold the tag set the acceptance reported, got {:?} for {:?}",
        case.name,
        left.member,
        tag_member(&accepted)
    );
    left
}

/// RULE: both tag writers leave the SAME tag set, in the same wire form, for the same
/// held set and the same carried map — every carried key written over what was held,
/// every unnamed key keeping its value, and a request carrying no tag leaving "no tags"
/// in its single representation.
#[tokio::test]
async fn both_tag_writers_merge_one_tag_set_the_same_way() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let gateway = emulator.attach_point().await;
    let table = merge_table();
    assert!(
        table.len() >= 8,
        "the table must span the forms a tag write can take, got {} rows",
        table.len()
    );

    for (index, case) in table.iter().enumerate() {
        let placement = &placements[index % placements.len()];
        let expected = case.expected();
        let tagged = through_tagging(&emulator, placement, index, case).await;
        let accepted =
            through_acceptance(&emulator, placement, 100 + index, case, &gateway).await;

        for (writer, left) in [("TagResource", &tagged), ("AcceptConnectionProposal", &accepted)] {
            assert_eq!(
                left.tags, expected,
                "case {:?} / {writer}: expected the merged tag set {expected:?}, got {:?}",
                case.name, left.tags
            );
            assert_eq!(
                left.listed, expected,
                "case {:?} / {writer}: expected ListTagsForResource to report {expected:?}, got {:?}",
                case.name, left.listed
            );
            // The one representation "no tags" has: the member ABSENT, never a map
            // carrying no entries — which is what a write that adds nothing must leave.
            assert_eq!(
                left.member.is_null(),
                expected.is_empty(),
                "case {:?} / {writer}: expected a resource holding {expected:?} to report its tags as {}, got {:?}",
                case.name,
                if expected.is_empty() { "no member at all" } else { "those entries" },
                left.member
            );
        }
        assert_eq!(
            tagged.member, accepted.member,
            "case {:?}: expected both tag writers to leave the same wire form, got {:?} from the tagging operation and {:?} from the acceptance",
            case.name, tagged.member, accepted.member
        );
    }
}

/// RULE: a merge adds only what the request carries, so the set a resource is left
/// holding does not depend on how the keys were split across calls or on which writer
/// wrote them — the row's keys written one at a time reach the same set as the whole map
/// at once, and writing them again, through either writer, changes nothing.
#[tokio::test]
async fn the_merge_does_not_depend_on_the_writer_or_on_how_the_keys_are_split() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let gateway = emulator.attach_point().await;

    for (index, case) in merge_table().into_iter().enumerate() {
        let Some(supplied) = case.supplied.clone().filter(|map| !map.is_empty()) else {
            // A row carrying no entries has no keys to split; the suite above already
            // holds both writers to leaving such a resource alone.
            continue;
        };
        let placement = &placements[index % placements.len()];
        let expected = case.expected();

        // One key per call through the tagging writer.
        let (id, arn) = holding(&emulator, placement, 200 + index, &case).await;
        for (key, value) in &supplied {
            tag(
                &emulator,
                &arn,
                &json!({ key.clone(): value.clone() }),
                &case.name,
            )
            .await;
        }
        let one_at_a_time = written(&emulator, &id, &arn, &case.name).await;
        assert_eq!(
            one_at_a_time.tags, expected,
            "case {:?}: expected the keys written one at a time to reach {expected:?}, got {:?}",
            case.name, one_at_a_time.tags
        );

        // The whole map again over that same resource: a repeat adds nothing.
        tag(&emulator, &arn, &json!(supplied), &case.name).await;
        let repeated = written(&emulator, &id, &arn, &case.name).await;
        assert_eq!(
            repeated.member, one_at_a_time.member,
            "case {:?}: expected rewriting the whole map to change nothing, got {:?} for {:?}",
            case.name, repeated.member, one_at_a_time.member
        );

        // The whole map at once through the acceptance writer, then rewritten through
        // the tagging writer over the SAME resource: one merge, two writers.
        let accepted =
            through_acceptance(&emulator, placement, 300 + index, &case, &gateway).await;
        assert_eq!(
            accepted.tags, one_at_a_time.tags,
            "case {:?}: expected the acceptance to reach the set the tagging operation reached ({:?}), got {:?}",
            case.name, one_at_a_time.tags, accepted.tags
        );
        tag(&emulator, &accepted.arn, &json!(supplied), &case.name).await;
        let accepted_then_tagged = written(
            &emulator,
            &accepted.id,
            &accepted.arn,
            &case.name,
        )
        .await;
        assert_eq!(
            accepted_then_tagged.member, accepted.member,
            "case {:?}: expected the tagging operation to add nothing the acceptance had already written, got {:?} for {:?}",
            case.name, accepted_then_tagged.member, accepted.member
        );
        assert_eq!(
            accepted_then_tagged.member, repeated.member,
            "case {:?}: expected both writers to leave one wire form for the same merge, got {:?} and {:?}",
            case.name, accepted_then_tagged.member, repeated.member
        );
    }
}

/// The most tags one resource holds at once, as `state.rs` documents the bound.
const MAX_TAGS: usize = 50;

/// RULE: because a carried key the resource already holds takes the new value instead of
/// adding an entry, the bound on a resource's tag set is judged against the set the
/// merge would LEAVE — so rewriting held keys is accepted even at the bound, while a
/// request that would leave the resource holding more than the bound is refused with the
/// modelled 400, worded the same way and writing nothing, on both writers alike.
#[tokio::test]
async fn both_tag_writers_judge_the_bound_against_the_merged_set() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let gateway = emulator.attach_point().await;
    let at_bound: BTreeMap<String, String> = (0..MAX_TAGS)
        .map(|index| (format!("key-{index:02}"), format!("held-{index:02}")))
        .collect();
    // A rewrite of keys the resource already holds: it names no new key, so the merged
    // set is the same size as the held one.
    let rewrite: BTreeMap<String, String> = at_bound
        .keys()
        .take(3)
        .map(|key| (key.clone(), "rewritten".to_string()))
        .collect();
    let mut after_rewrite = at_bound.clone();
    after_rewrite.extend(rewrite.iter().map(|(key, value)| (key.clone(), value.clone())));
    // One key past the bound, which is what the merged count has to refuse.
    let one_too_many = BTreeMap::from([("one-too-many".to_string(), "x".to_string())]);

    // The tagging writer, on a resource created at the bound.
    let held = MergeCase {
        name: format!("{MAX_TAGS} tags held"),
        held: at_bound.clone(),
        supplied: None,
    };
    let (tagged_id, tagged_arn) = holding(&emulator, &placements[0], 400, &held).await;
    let (status, tagging_refusal) = emulator
        .ic(
            "TagResource",
            json!({ "arn": tagged_arn, "tags": one_too_many }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"one fresh key past the bound / TagResource\": expected the modelled 400, got {status} with {tagging_refusal:?}"
    );
    assert!(
        code(&tagging_refusal).contains("InterconnectValidationException"),
        "case \"one fresh key past the bound / TagResource\": expected InterconnectValidationException, got {:?}",
        code(&tagging_refusal)
    );
    let after_refusal = written(
        &emulator,
        &tagged_id,
        &tagged_arn,
        "one fresh key past the bound / TagResource",
    )
    .await;
    assert_eq!(
        after_refusal.tags, at_bound,
        "case \"one fresh key past the bound / TagResource\": expected the refusal to write nothing, got {} entries",
        after_refusal.tags.len()
    );
    tag(
        &emulator,
        &tagged_arn,
        &json!(rewrite),
        "rewriting held keys at the bound / TagResource",
    )
    .await;
    let tagged = written(
        &emulator,
        &tagged_id,
        &tagged_arn,
        "rewriting held keys at the bound / TagResource",
    )
    .await;
    assert_eq!(
        tagged.tags, after_rewrite,
        "case \"rewriting held keys at the bound / TagResource\": expected {after_rewrite:?}, got {:?}",
        tagged.tags
    );

    // The acceptance writer, on a proposal created at the bound: the same two requests,
    // the refused one first, so what the acceptance then applies is judged on the set
    // the refusal left.
    let proposal = create_with_tag_member(
        &emulator,
        DEFAULT_SIGNING_REGION,
        &placements[1 % placements.len()],
        401,
        held.held_member().as_ref(),
    )
    .await;
    let activation_key = text(&proposal, "activationKey");
    let acceptance = |tags: BTreeMap<String, String>| {
        json!({
            "activationKey": activation_key.clone(),
            "attachPoint": { "directConnectGateway": gateway.clone() },
            "tags": tags,
        })
    };
    let (status, acceptance_refusal) = emulator
        .ic("AcceptConnectionProposal", acceptance(one_too_many))
        .await;
    assert_eq!(
        status, 400,
        "case \"one fresh key past the bound / AcceptConnectionProposal\": expected the modelled 400, got {status} with {acceptance_refusal:?}"
    );
    assert!(
        code(&acceptance_refusal).contains("InterconnectValidationException"),
        "case \"one fresh key past the bound / AcceptConnectionProposal\": expected InterconnectValidationException, got {:?}",
        code(&acceptance_refusal)
    );
    assert_eq!(
        message(&acceptance_refusal),
        message(&tagging_refusal),
        "case \"one fresh key past the bound\": expected both writers to word the refusal identically, got {:?} and {:?}",
        message(&acceptance_refusal),
        message(&tagging_refusal)
    );
    let proposal_after_refusal = written(
        &emulator,
        &text(&proposal, "id"),
        &text(&proposal, "arn"),
        "one fresh key past the bound / AcceptConnectionProposal",
    )
    .await;
    assert_eq!(
        proposal_after_refusal.tags, at_bound,
        "case \"one fresh key past the bound / AcceptConnectionProposal\": expected the refusal to write nothing, got {} entries",
        proposal_after_refusal.tags.len()
    );

    let (status, body) = emulator
        .ic("AcceptConnectionProposal", acceptance(rewrite))
        .await;
    assert_eq!(
        status, 200,
        "case \"rewriting held keys at the bound / AcceptConnectionProposal\": expected HTTP 200, got {status} with {body:?}"
    );
    assert_eq!(
        tags_of(&body["connection"]),
        after_rewrite,
        "case \"rewriting held keys at the bound / AcceptConnectionProposal\": expected {after_rewrite:?}, got {:?}",
        tags_of(&body["connection"])
    );
    assert_eq!(
        tag_member(&body["connection"]),
        tagged.member,
        "case \"rewriting held keys at the bound\": expected both writers to leave the same wire form, got {:?} and {:?}",
        tag_member(&body["connection"]),
        tagged.member
    );
}
