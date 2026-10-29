// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the ONE representation a tag set has in this service, driven
//! over the wire through the shared [`crate::emulator`] harness.
//!
//! RULE under test: a `tags` map carrying no entries asks for the same tag set as no
//! `tags` member at all, so the service holds and reports that set in exactly ONE wire
//! form — the `tags` member ABSENT — however the resource reached it (a create, a tag
//! write that adds nothing, an untag that removes the last key, an acceptance), and a
//! request carrying either spelling of "no tags" is the same request as one carrying
//! the other.
//!
//! Every assertion is about that rule over a TABLE whose rows differ in the FORM of
//! the `tags` member a create carries (absent, carrying no entries, one entry, several
//! entries, an entry with an empty value, entries at the modelled key-length bounds,
//! many entries) and in the environment and bandwidth the connection sits on. The
//! lengths come from the model: `smithy_query(query="shape",shape="TagKey,TagValue")`
//! gives `TagKey` `@length(min: 1, max: 128)` and `TagValue` `@length(min: 0, max: 256)`.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::emulator::{
    code, create_request, create_with_tag_member, placements, tags_of, text, without_tags, Emulator,
    DEFAULT_SIGNING_REGION,
};

/// `TagKey` is modelled `@length(min: 1, max: 128)`, so these are the shortest and
/// longest key a row can carry.
const TAG_KEY_MIN: usize = 1;
const TAG_KEY_MAX: usize = 128;

/// One row: the `tags` member a create carries, verbatim, and the tag set it asks for.
#[derive(Debug, Clone)]
struct MemberCase {
    name: String,
    /// The `tags` member as the request carries it — `None` leaves it out altogether.
    member: Option<Value>,
    /// The set the row asks for, which is what every later read must report.
    expected: BTreeMap<String, String>,
}

impl MemberCase {
    fn new(name: &str, member: Option<Value>) -> Self {
        let expected = member
            .as_ref()
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .map(|(key, value)| {
                        (
                            key.clone(),
                            value
                                .as_str()
                                .expect("a tag value is a string")
                                .to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            name: name.to_string(),
            member,
            expected,
        }
    }

    /// Whether this row asks for no tags at all — the state the rule is about.
    fn asks_for_no_tags(&self) -> bool {
        self.expected.is_empty()
    }
}

/// The table: the forms a `tags` member can take on a create. The two spellings of
/// "no tags" (the member absent, and a member carrying no entries) are the pair the
/// rule is about; the rest hold the rule's other half — a set with entries is stored
/// as it is — at one entry, several, an entry whose value is empty (`TagValue` allows
/// a zero-length value), each modelled key-length bound, and a size no other row
/// reaches.
fn member_table() -> Vec<MemberCase> {
    let shortest_key = "k".repeat(TAG_KEY_MIN);
    let longest_key = "k".repeat(TAG_KEY_MAX);
    let bounds: Map<String, Value> = [
        (shortest_key, json!("at the minimum")),
        (longest_key, json!("at the maximum")),
    ]
    .into_iter()
    .collect();
    let many: Map<String, Value> = (0..12)
        .map(|index| (format!("key-{index:02}"), json!(format!("value-{index:02}"))))
        .collect();
    vec![
        MemberCase::new("no tags member at all", None),
        MemberCase::new("a tags member carrying no entries", Some(json!({}))),
        MemberCase::new("one tag", Some(json!({ "env": "test" }))),
        MemberCase::new(
            "three tags",
            Some(json!({ "env": "test", "team": "network", "owner": "ops" })),
        ),
        MemberCase::new("one tag whose value is empty", Some(json!({ "env": "" }))),
        MemberCase::new(
            "keys at each modelled length bound",
            Some(Value::Object(bounds)),
        ),
        MemberCase::new("twelve tags", Some(Value::Object(many))),
    ]
}

/// The `tags` member of a response, verbatim — the WIRE FORM the rule is about, which
/// `crate::emulator::tags_of` deliberately flattens.
fn tag_member(value: &Value) -> Value {
    value["tags"].clone()
}

/// A `Connection` with its tag set and its `state` projected out, so two readings
/// taken at different moments compare on everything a tag write must not touch and
/// the clock does not move on its own.
fn without_tags_or_state(connection: &Value) -> Value {
    let mut copy = without_tags(connection);
    if let Some(map) = copy.as_object_mut() {
        map.remove("state");
    }
    copy
}

/// `TagResource` for `arn` with `tags` supplied verbatim.
async fn tag(emulator: &Emulator, arn: &str, tags: &Value) -> (u16, Value) {
    emulator
        .ic("TagResource", json!({ "arn": arn, "tags": tags }))
        .await
}

/// `UntagResource` for `arn`, removing `keys`.
async fn untag(emulator: &Emulator, arn: &str, keys: &[String]) -> (u16, Value) {
    emulator
        .ic("UntagResource", json!({ "arn": arn, "tagKeys": keys }))
        .await
}

/// `ListTagsForResource` for `arn`, as its reported tag set.
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

/// RULE: a create's `tags` member carrying no entries asks for the same set as no
/// `tags` member at all, so both are held and reported as "no tags" in the ONE form
/// that state has — the member absent — while a member carrying entries is reported
/// with exactly those entries.
#[tokio::test]
async fn a_create_holds_a_tag_set_in_one_representation() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let table = member_table();
    assert!(
        table.len() >= 5,
        "the table must span the forms a tags member can take, got {} rows",
        table.len()
    );

    for (seed, case) in table.iter().enumerate() {
        let placement = &placements[seed % placements.len()];
        let created = create_with_tag_member(
            &emulator,
            DEFAULT_SIGNING_REGION,
            placement,
            seed,
            case.member.as_ref(),
        )
        .await;
        let id = text(&created, "id");
        let arn = text(&created, "arn");

        // What the create reports, and what the store kept: the same record.
        let fetched = emulator.get_connection(&id).await.1["connection"].clone();
        assert_eq!(
            tag_member(&fetched),
            tag_member(&created),
            "case {:?}: expected the stored tag set to read back as the create reported it, got {:?} for {:?}",
            case.name,
            tag_member(&fetched),
            tag_member(&created)
        );
        assert_eq!(
            tags_of(&fetched),
            case.expected,
            "case {:?}: expected the tag set {:?}, got {:?}",
            case.name,
            case.expected,
            tag_member(&fetched)
        );
        assert_eq!(
            listed_tags(&emulator, &arn, &case.name).await,
            case.expected,
            "case {:?}: expected ListTagsForResource to report {:?}, got something else",
            case.name,
            case.expected
        );

        // The one representation of "no tags": the member is ABSENT, never a map
        // carrying no entries — whichever spelling the create used.
        if case.asks_for_no_tags() {
            assert!(
                tag_member(&created).is_null(),
                "case {:?}: expected a connection with no tags to report no tags member at all, got {:?}",
                case.name,
                tag_member(&created)
            );
        } else {
            assert_eq!(
                tag_member(&created),
                json!(case.expected),
                "case {:?}: expected the created connection to carry the supplied entries, got {:?}",
                case.name,
                tag_member(&created)
            );
        }
    }
}

/// RULE: the wire form of "no tags" does not depend on the path that reached it — two
/// connections holding the same tag set report it identically whether the set came
/// from a create, from a tag write that adds nothing, or from an untag that removed
/// the last key.
#[tokio::test]
async fn the_representation_does_not_depend_on_the_write_path() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;

    // The baseline: a create that carries no `tags` member at all, which is how a
    // resource with no tags is held.
    let baseline = create_with_tag_member(&emulator, DEFAULT_SIGNING_REGION, &placements[0], 0, None)
        .await;
    let no_tags_form = tag_member(&baseline);

    for (seed, case) in member_table().into_iter().enumerate() {
        let placement = &placements[seed % placements.len()];
        let created = create_with_tag_member(
            &emulator,
            DEFAULT_SIGNING_REGION,
            placement,
            100 + seed,
            case.member.as_ref(),
        )
        .await;
        let id = text(&created, "id");
        let arn = text(&created, "arn");
        let created_form = tag_member(&created);

        // A tag write carrying no entries adds nothing and leaves the representation
        // the create left.
        let (status, body) = tag(&emulator, &arn, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from a TagResource carrying no entries, got {status} with {body:?}",
            case.name
        );
        let after_empty_write = emulator.get_connection(&id).await.1["connection"].clone();
        assert_eq!(
            tag_member(&after_empty_write),
            created_form,
            "case {:?}: expected a tag write carrying no entries to leave the tag set exactly as it was, got {:?} for {created_form:?}",
            case.name,
            tag_member(&after_empty_write)
        );

        // A tag write and the untag of exactly what it wrote: a round trip back to
        // the same logical set, which must report the same wire form as before it.
        let round_trip_key = "round-trip".to_string();
        let (status, body) = tag(
            &emulator,
            &arn,
            &json!({ round_trip_key.clone(): "written then removed" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from TagResource, got {status} with {body:?}",
            case.name
        );
        let (status, body) = untag(&emulator, &arn, std::slice::from_ref(&round_trip_key)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from UntagResource, got {status} with {body:?}",
            case.name
        );
        let after_round_trip = emulator.get_connection(&id).await.1["connection"].clone();
        assert_eq!(
            tag_member(&after_round_trip),
            created_form,
            "case {:?}: expected a tag/untag round trip to leave the same wire form the create left, got {:?} for {created_form:?}",
            case.name,
            tag_member(&after_round_trip)
        );
        assert_eq!(
            without_tags_or_state(&after_round_trip),
            without_tags_or_state(&created),
            "case {:?}: expected the tag round trip to touch nothing else about the record, got {after_round_trip:?}",
            case.name
        );

        // Emptying the set entirely reaches the one representation "no tags" has,
        // whatever the set was before it.
        let keys: Vec<String> = case.expected.keys().cloned().collect();
        if !keys.is_empty() {
            let (status, body) = untag(&emulator, &arn, &keys).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected HTTP 200 removing every key, got {status} with {body:?}",
                case.name
            );
        }
        let emptied = emulator.get_connection(&id).await.1["connection"].clone();
        assert_eq!(
            tag_member(&emptied),
            no_tags_form,
            "case {:?}: expected a connection left with no tags to report them the way one created without any does ({no_tags_form:?}), got {:?}",
            case.name,
            tag_member(&emptied)
        );
        assert_eq!(
            listed_tags(&emulator, &arn, &case.name).await,
            BTreeMap::new(),
            "case {:?}: expected ListTagsForResource to report no tags once every key is gone",
            case.name
        );
    }
}

/// One acceptance row: the `tags` member the proposal was created with, the one its
/// acceptance supplies, and the set the accepted connection must hold.
#[derive(Debug, Clone)]
struct AcceptanceCase {
    name: &'static str,
    proposal_tags: Option<Value>,
    acceptance_tags: Option<Value>,
    expected: BTreeMap<String, String>,
}

/// The acceptance table: the two spellings of "no tags" on the acceptance, over a
/// proposal that holds no tags and one that holds some, plus an acceptance that does
/// carry entries.
fn acceptance_table() -> Vec<AcceptanceCase> {
    vec![
        AcceptanceCase {
            name: "no tags on either call",
            proposal_tags: None,
            acceptance_tags: None,
            expected: BTreeMap::new(),
        },
        AcceptanceCase {
            name: "no entries on the acceptance of an untagged proposal",
            proposal_tags: None,
            acceptance_tags: Some(json!({})),
            expected: BTreeMap::new(),
        },
        AcceptanceCase {
            name: "no entries on the acceptance of a tagged proposal",
            proposal_tags: Some(json!({ "stage": "proposal" })),
            acceptance_tags: Some(json!({})),
            expected: BTreeMap::from([("stage".to_string(), "proposal".to_string())]),
        },
        AcceptanceCase {
            name: "entries on the acceptance of an untagged proposal",
            proposal_tags: None,
            acceptance_tags: Some(json!({ "stage": "accepted" })),
            expected: BTreeMap::from([("stage".to_string(), "accepted".to_string())]),
        },
    ]
}

/// RULE: an acceptance's `tags` member carrying no entries asks for the same set as no
/// `tags` member at all, so neither turns the accepted connection's "no tags" into a
/// second representation, while a member carrying entries is merged onto the set the
/// proposal held.
#[tokio::test]
async fn an_acceptance_holds_a_tag_set_in_one_representation() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let gateway = emulator.attach_point().await;

    for (seed, case) in acceptance_table().into_iter().enumerate() {
        let name = case.name;
        let expected = case.expected;
        let placement = &placements[seed % placements.len()];
        let proposal = create_with_tag_member(
            &emulator,
            DEFAULT_SIGNING_REGION,
            placement,
            200 + seed,
            case.proposal_tags.as_ref(),
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
        if let Some(tags) = case.acceptance_tags.as_ref() {
            request.insert("tags".to_string(), tags.clone());
        }
        let (status, body) = emulator
            .ic("AcceptConnectionProposal", Value::Object(request))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from AcceptConnectionProposal, got {status} with {body:?}"
        );
        let accepted = body["connection"].clone();
        assert_eq!(
            tags_of(&accepted),
            expected,
            "case {name:?}: expected the tag set {expected:?}, got {:?}",
            tag_member(&accepted)
        );
        if expected.is_empty() {
            assert!(
                tag_member(&accepted).is_null(),
                "case {name:?}: expected an accepted connection with no tags to report no tags member at all, got {:?}",
                tag_member(&accepted)
            );
        }
        // The acceptance was persisted in that one form.
        let fetched = emulator.get_connection(&text(&accepted, "id")).await.1["connection"].clone();
        assert_eq!(
            tag_member(&fetched),
            tag_member(&accepted),
            "case {name:?}: expected the stored tag set to read back as the acceptance reported it, got {:?} for {:?}",
            tag_member(&fetched),
            tag_member(&accepted)
        );
    }
}

/// RULE: because a `tags` member carrying no entries asks for the same set as no
/// `tags` member at all, a repeated `clientToken` carrying one spelling of "no tags"
/// is the SAME request as the create that carried the other, and is replayed — while a
/// repeat asking for a set with entries is still a different request and is refused.
#[tokio::test]
async fn either_spelling_of_no_tags_is_the_same_request() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let no_entries = json!({});
    let with_entries = json!({ "env": "test" });

    // Each row: the `tags` member the first create carried, the one the repeat
    // carries, and whether the repeat is the same request.
    let rows: Vec<(&str, Option<&Value>, Option<&Value>, bool)> = vec![
        ("no member, then no entries", None, Some(&no_entries), true),
        ("no entries, then no member", Some(&no_entries), None, true),
        (
            "no entries, then no entries",
            Some(&no_entries),
            Some(&no_entries),
            true,
        ),
        (
            "no entries, then entries",
            Some(&no_entries),
            Some(&with_entries),
            false,
        ),
        (
            "entries, then no entries",
            Some(&with_entries),
            Some(&no_entries),
            false,
        ),
        (
            "entries, then no member",
            Some(&with_entries),
            None,
            false,
        ),
        (
            "entries, then the same entries",
            Some(&with_entries),
            Some(&with_entries),
            true,
        ),
    ];

    for (seed, (name, first, repeat, same_request)) in rows.into_iter().enumerate() {
        let placement = &placements[seed % placements.len()];
        let token = format!("one-representation-{seed}");
        // The two calls of a row differ ONLY in the `tags` member: same placement,
        // same description (the `seed`), same identifying information, same token.
        let seed = 300 + seed;
        let created = emulator
            .ic_ok(
                "CreateConnection",
                create_request(&emulator, placement, seed, first, Some(&token)).await,
            )
            .await["connection"]
            .clone();
        let id = text(&created, "id");

        let (status, body) = emulator
            .ic(
                "CreateConnection",
                create_request(&emulator, placement, seed, repeat, Some(&token)).await,
            )
            .await;
        if same_request {
            assert_eq!(
                status, 200,
                "case {name:?}: expected the repeat to replay the connection the token created, got {status} with {body:?}"
            );
            assert_eq!(
                text(&body["connection"], "id"),
                id,
                "case {name:?}: expected the replay to report connection {id:?}, got {:?}",
                body["connection"]
            );
            assert_eq!(
                tag_member(&body["connection"]),
                tag_member(&created),
                "case {name:?}: expected the replay to report the tag set the create left, got {:?} for {:?}",
                tag_member(&body["connection"]),
                tag_member(&created)
            );
        } else {
            assert_eq!(
                status, 400,
                "case {name:?}: expected a repeat asking for a different tag set to be refused, got {status} with {body:?}"
            );
            assert!(
                code(&body).contains("InterconnectValidationException"),
                "case {name:?}: expected InterconnectValidationException, got {:?}",
                code(&body)
            );
        }
    }
}
