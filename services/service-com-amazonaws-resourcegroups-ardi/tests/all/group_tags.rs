// SPDX-License-Identifier: CC0-1.0

//! `Tag` and `GetTags`, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suite's `Emulator`), so these exercise
//! the whole restJson1 path: routing, the URI-bound `Arn` label, boundary
//! validation, the handler, and serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * `Tag` MERGES the pairs it carries into the addressed group's stored tag map —
//!   a key the request names is set or overwritten, a key it does not name keeps
//!   the value it had — and reports the group's stored ARN beside the pairs now
//!   stored for the keys it named;
//! * `GetTags` reports that same stored map and changes nothing a later caller
//!   sees, whatever region the reader's credential scope names;
//! * a group is addressed by its own stored ARN, so an ARN for another region or
//!   another account addresses no group and is `NotFoundException` (404);
//! * a pair this service will not store — outside the modelled key/value lengths
//!   or character set, or in the reserved key namespace — is
//!   `BadRequestException` (400) and stores NONE of its batch, on `Tag` and on the
//!   `Tags` member of `CreateGroup` alike;
//! * the store's own ceiling on tags per group is enforced the same way.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use service_com_amazonaws_resourcegroups_ardi::state::{
    MAX_TAGS_PER_GROUP, MAX_TAG_KEY_LENGTH, MAX_TAG_VALUE_LENGTH, RESERVED_TAG_KEY_PREFIX,
};

use super::resource_group_lifecycle::{error_code, Emulator, ACCOUNT};

/// Percent-encode one path segment: everything outside RFC 3986 §2.3's unreserved
/// set is escaped, which is what an SDK client sends for an ARN bound to a
/// NON-greedy `@httpLabel` (`/resources/{Arn}/tags`).
///
/// Shared with the sibling untagging suite, which addresses the same path.
pub(crate) fn pct(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The ARN every operation of this service reports for the group `name` in `region`.
pub(crate) fn group_arn(region: &str, name: &str) -> String {
    format!("arn:aws:resource-groups:{region}:{ACCOUNT}:group/{name}")
}

/// A deterministic string of `length` ASCII letters, for exercising a modelled
/// length bound rather than one hand-picked spelling.
pub(crate) fn filler(length: usize, seed: u8) -> String {
    (0..length)
        .map(|index| char::from(b'a' + ((index as u8).wrapping_add(seed) % 26)))
        .collect()
}

/// The wire form of a `Tags` map.
pub(crate) fn tag_json(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), Value::from(value.clone())))
            .collect::<Map<String, Value>>(),
    )
}

/// A `Tags` map from a response, as an ordered map so two answers compare.
pub(crate) fn tag_pairs(case: &str, tags: &Value) -> BTreeMap<String, String> {
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags object, got {tags}"))
        .iter()
        .map(|(key, value)| {
            let text = value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a string tag value, got {value}"));
            (key.clone(), text.to_string())
        })
        .collect()
}

/// `PUT /resources/{Arn}/tags` with `tags` as the request's `Tags` map, signed (in
/// scope only) for `region`.
pub(crate) async fn tag(
    emulator: &Emulator,
    region: &str,
    arn: &str,
    tags: &Value,
) -> (u16, Value) {
    emulator
        .request_in(
            region,
            "PUT",
            &format!("/resources/{}/tags", pct(arn)),
            Some(&json!({ "Tags": tags })),
        )
        .await
}

/// `GET /resources/{Arn}/tags`, which carries no payload at all.
pub(crate) async fn get_tags(emulator: &Emulator, region: &str, arn: &str) -> (u16, Value) {
    emulator
        .request_in(
            region,
            "GET",
            &format!("/resources/{}/tags", pct(arn)),
            None,
        )
        .await
}

/// The map `GetTags` reports for `arn`, which must be a 200 reporting that same ARN.
pub(crate) async fn stored_tags(
    emulator: &Emulator,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let (status, body) = get_tags(emulator, "us-east-1", arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetTags expected 200, got {status} {body}",
    );
    assert_eq!(
        body["Arn"].as_str(),
        Some(arn),
        "case {case:?}: expected GetTags to report the stored ARN {arn}, got {body}",
    );
    tag_pairs(case, &body["Tags"])
}

/// Batches applied one after another to ONE group, differing along what the merge
/// rule generalises over: how many pairs a batch carries (none, one, several),
/// whether a key is new or overwrites a stored one, and the spelling of the pair.
///
/// The key and value lengths come from the modelled `TagKey` (`length 1..128`) and
/// `TagValue` (`length 0..256`), and the character rows from the pattern those two
/// shapes share, `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`
/// (`smithy_query(query="shape",shape="TagKey,TagValue")`).
fn merge_batches() -> Vec<(&'static str, Vec<(String, String)>)> {
    vec![
        (
            "one new pair",
            vec![("Stage".to_string(), "Test".to_string())],
        ),
        (
            "several new pairs at once",
            vec![
                ("Team".to_string(), "Platform".to_string()),
                ("Owner".to_string(), "sre".to_string()),
            ],
        ),
        (
            "a key that overwrites a stored one, beside a new key",
            vec![
                ("Stage".to_string(), "Prod".to_string()),
                ("CostCenter".to_string(), "1234".to_string()),
            ],
        ),
        ("no pairs at all, which adds nothing", Vec::new()),
        (
            "a key at the modelled one-character minimum",
            vec![("k".to_string(), "short".to_string())],
        ),
        (
            "a key at the modelled 128-character maximum",
            vec![(filler(MAX_TAG_KEY_LENGTH, 1), "at-the-key-bound".to_string())],
        ),
        (
            "a value at the modelled 256-character maximum",
            vec![(
                "long-value".to_string(),
                filler(MAX_TAG_VALUE_LENGTH, 2),
            )],
        ),
        (
            "an empty value, which the modelled zero-character minimum allows",
            vec![("Empty".to_string(), String::new())],
        ),
        (
            "every punctuation character the modelled pattern admits",
            vec![(
                "ns:key_part.one/two=three+four-five@six".to_string(),
                "value with spaces and _.:/=+-@".to_string(),
            )],
        ),
        (
            "non-ASCII letters and numbers on both halves of the pair",
            vec![("Équipe".to_string(), "Ωμέγα 42".to_string())],
        ),
        (
            "a key that merely contains the reserved prefix later on",
            vec![("team-aws:billing".to_string(), "kept".to_string())],
        ),
    ]
}

/// Tagging merges into the stored map, reports the pairs it stored, and reads back
/// through `GetTags` — in whichever region the group was created in, and however
/// many times the same batch is repeated.
#[tokio::test]
async fn tagging_merges_pairs_into_the_stored_map_and_reads_back() {
    for (region, name) in [("us-east-1", "tagged-east"), ("eu-west-1", "tagged.west_1")] {
        let emulator = Emulator::new();
        let (created_status, created) = emulator
            .call_in(region, "/groups", &json!({ "Name": name }))
            .await;
        assert_eq!(
            created_status, 200,
            "case {name:?}: CreateGroup expected 200, got {created_status} {created}",
        );
        let arn = group_arn(region, name);
        assert_eq!(
            created["Group"]["GroupArn"].as_str(),
            Some(arn.as_str()),
            "case {name:?}: expected the created group's ARN to be {arn}, got {created}",
        );

        // The map every batch so far has built up: the merge rule's running result.
        let mut expected: BTreeMap<String, String> = BTreeMap::new();
        for (case, batch) in merge_batches() {
            let sent = tag_json(&batch);
            let (status, answered) = tag(&emulator, region, &arn, &sent).await;
            assert_eq!(
                status, 200,
                "case {case:?}: Tag expected 200, got {status} {answered}",
            );
            assert_eq!(
                answered["Arn"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: expected Tag to report the stored ARN {arn}, got {answered}",
            );
            let batch_map: BTreeMap<String, String> = batch.iter().cloned().collect();
            assert_eq!(
                tag_pairs(case, &answered["Tags"]),
                batch_map,
                "case {case:?}: expected Tag to report the pairs it added {batch_map:?}, \
                 got {answered}",
            );

            expected.extend(batch.iter().cloned());
            let stored = stored_tags(&emulator, case, &arn).await;
            assert_eq!(
                stored, expected,
                "case {case:?}: expected the stored map {expected:?}, got {stored:?}",
            );

            // The same batch again is the same answer and the same map: tagging a
            // pair that is already stored changes nothing.
            let (repeat_status, repeated) = tag(&emulator, region, &arn, &sent).await;
            assert_eq!(
                repeat_status, 200,
                "case {case:?}: repeating the batch expected 200, got {repeat_status} {repeated}",
            );
            let after = stored_tags(&emulator, case, &arn).await;
            assert_eq!(
                after, expected,
                "case {case:?}: repeating the batch must leave the map at {expected:?}, \
                 got {after:?}",
            );
        }
    }
}

/// One row of the reporting table: a whole `CreateGroup` request, and the tag map
/// `GetTags` must report for the group it creates.
struct ReportingCase {
    case: &'static str,
    name: &'static str,
    request: Value,
    expected: Vec<(String, String)>,
}

/// `GetTags` reports the map the store holds — the tags a create carried included —
/// and reporting changes nothing a later caller sees.
#[tokio::test]
async fn get_tags_reports_the_stored_map_and_changes_nothing() {
    let emulator = Emulator::new();
    let cases = vec![
        ReportingCase {
            case: "a group created with no Tags member at all",
            name: "untagged",
            request: json!({ "Name": "untagged" }),
            expected: Vec::new(),
        },
        ReportingCase {
            case: "a group created with an empty tag map",
            name: "empty-tag-map",
            request: json!({ "Name": "empty-tag-map", "Tags": {} }),
            expected: Vec::new(),
        },
        ReportingCase {
            case: "a group created with several tags",
            name: "created-tagged",
            request: json!({
                "Name": "created-tagged",
                "Tags": { "Stage": "Test", "Team": "Platform" }
            }),
            expected: vec![
                ("Stage".to_string(), "Test".to_string()),
                ("Team".to_string(), "Platform".to_string()),
            ],
        },
    ];
    for row in cases {
        let ReportingCase {
            case,
            name,
            request,
            expected: pairs,
        } = row;
        let created = emulator.ok("/groups", &request).await;
        let arn = group_arn("us-east-1", name);
        assert_eq!(
            created["Group"]["GroupArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the created group's ARN to be {arn}, got {created}",
        );
        let expected: BTreeMap<String, String> = pairs.into_iter().collect();

        // The map is computed from the store, so every reader sees it — including
        // one whose credential scope names another region.
        for region in ["us-east-1", "eu-central-1", "ap-southeast-2"] {
            let (status, body) = get_tags(&emulator, region, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?} (read from {region}): expected 200, got {status} {body}",
            );
            assert_eq!(
                tag_pairs(case, &body["Tags"]),
                expected,
                "case {case:?} (read from {region}): expected {expected:?}, got {body}",
            );
            assert_eq!(
                body["Arn"].as_str(),
                Some(arn.as_str()),
                "case {case:?} (read from {region}): expected the stored ARN {arn}, got {body}",
            );
        }

        // Reporting is not a mutation: the group still reads back as itself.
        let group = emulator.ok("/get-group", &json!({ "Group": name })).await;
        assert_eq!(
            group["Group"]["Name"].as_str(),
            Some(name),
            "case {case:?}: expected GetGroup to still report the group, got {group}",
        );
    }
}

/// An `Arn` that addresses no stored group is refused, and refused without
/// touching the group that IS stored: a 404 for a well-formed group ARN nothing
/// answers to, and a 400 for an identifier that is not a group ARN at all.
#[tokio::test]
async fn an_arn_that_addresses_no_stored_group_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "/groups",
            &json!({ "Name": "present", "Tags": { "Stage": "Test" } }),
        )
        .await;
    let present = group_arn("us-east-1", "present");
    let expected: BTreeMap<String, String> =
        [("Stage".to_string(), "Test".to_string())].into_iter().collect();

    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a group name nothing was created under",
            group_arn("us-east-1", "absent"),
            404,
            "NotFoundException",
        ),
        (
            "the stored group's name in another region",
            group_arn("eu-west-1", "present"),
            404,
            "NotFoundException",
        ),
        (
            "the stored group's name in another account",
            "arn:aws:resource-groups:us-east-1:111111111111:group/present".to_string(),
            404,
            "NotFoundException",
        ),
        (
            "a group ARN in another partition, whose account is this one",
            format!("arn:aws-us-gov:resource-groups:us-east-1:{ACCOUNT}:group/present"),
            404,
            "NotFoundException",
        ),
        (
            "an ARN of another service, which is not a group ARN",
            format!("arn:aws:ec2:us-east-1:{ACCOUNT}:group/present"),
            400,
            "BadRequestException",
        ),
        (
            "a bare group name, which is not an ARN",
            "present".to_string(),
            400,
            "BadRequestException",
        ),
    ];
    for (case, arn, status, code) in rows {
        let (tag_status, tag_body) = tag(
            &emulator,
            "us-east-1",
            &arn,
            &json!({ "Stage": "Overwritten" }),
        )
        .await;
        assert_eq!(
            (tag_status, error_code(&tag_body)),
            (status, code),
            "case {case:?}: expected Tag to answer {status} {code}, got {tag_status} {tag_body}",
        );
        let (read_status, read_body) = get_tags(&emulator, "us-east-1", &arn).await;
        assert_eq!(
            (read_status, error_code(&read_body)),
            (status, code),
            "case {case:?}: expected GetTags to answer {status} {code}, \
             got {read_status} {read_body}",
        );
        let stored = stored_tags(&emulator, case, &present).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: the refusal must leave the stored group's tags at {expected:?}, \
             got {stored:?}",
        );
    }
}

/// Pairs this service will not store, each refused with HTTP 400
/// `BadRequestException` — and refused ATOMICALLY: a batch holding one of them
/// stores none of its pairs.
///
/// The rows differ along WHY the pair is refused: past a modelled length bound (by
/// one character, so the accepted row in `merge_batches` and the refused row here
/// differ by that character alone), outside the modelled character set, and inside
/// the reserved key namespace.
fn refused_batches() -> Vec<(String, Vec<(String, String)>)> {
    let reserved = format!("{RESERVED_TAG_KEY_PREFIX}cloudformation");
    vec![
        (
            "a key in the reserved namespace".to_string(),
            vec![(reserved.clone(), "stack".to_string())],
        ),
        (
            "the reserved prefix in upper case".to_string(),
            vec![(
                format!("{}billing", RESERVED_TAG_KEY_PREFIX.to_uppercase()),
                "x".to_string(),
            )],
        ),
        (
            "a reserved key beside a legal one".to_string(),
            vec![
                ("Legal".to_string(), "yes".to_string()),
                (reserved, "stack".to_string()),
            ],
        ),
        (
            format!(
                "a {}-character key, one past the modelled maximum",
                MAX_TAG_KEY_LENGTH + 1
            ),
            vec![(filler(MAX_TAG_KEY_LENGTH + 1, 3), "x".to_string())],
        ),
        (
            "an empty key, which names no tag".to_string(),
            vec![(String::new(), "orphan".to_string())],
        ),
        (
            format!(
                "a {}-character value, one past the modelled maximum",
                MAX_TAG_VALUE_LENGTH + 1
            ),
            vec![(
                "value-past-the-bound".to_string(),
                filler(MAX_TAG_VALUE_LENGTH + 1, 4),
            )],
        ),
        (
            "a key carrying a character outside the modelled set".to_string(),
            vec![("star*key".to_string(), "x".to_string())],
        ),
        (
            "a key carrying a control character, which is no separator".to_string(),
            vec![("tab\tkey".to_string(), "x".to_string())],
        ),
        (
            "a value carrying a character outside the modelled set".to_string(),
            vec![("Legal".to_string(), "percent%value".to_string())],
        ),
    ]
}

/// A batch holding a pair this service refuses is HTTP 400 `BadRequestException`
/// on `Tag`, and stores none of its pairs.
#[tokio::test]
async fn a_pair_this_service_refuses_stores_nothing() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "/groups",
            &json!({ "Name": "refusals", "Tags": { "Stage": "Test" } }),
        )
        .await;
    let arn = group_arn("us-east-1", "refusals");
    let expected: BTreeMap<String, String> =
        [("Stage".to_string(), "Test".to_string())].into_iter().collect();
    for (case, batch) in refused_batches() {
        let (status, body) = tag(&emulator, "us-east-1", &arn, &tag_json(&batch)).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {case:?}: expected Tag to answer 400 BadRequestException, got {status} {body}",
        );
        let stored = stored_tags(&emulator, &case, &arn).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: a refused batch must store nothing, expected {expected:?}, \
             got {stored:?}",
        );
    }

    // An absent `Tags` is refused at the boundary: the model makes it `@required`.
    let (status, body) = emulator
        .request_in(
            "us-east-1",
            "PUT",
            &format!("/resources/{}/tags", pct(&arn)),
            Some(&json!({})),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "a request carrying no Tags must be refused, got {status} {body}",
    );
}

/// Every pair `Tag` refuses, `CreateGroup`'s own `Tags` member refuses the same
/// way — and the refused create creates nothing.
#[tokio::test]
async fn create_group_refuses_the_same_pairs_and_creates_nothing() {
    let emulator = Emulator::new();
    for (index, (case, batch)) in refused_batches().into_iter().enumerate() {
        let name = format!("create-refusal-{index}");
        let (status, body) = emulator
            .call(
                "/groups",
                &json!({ "Name": name, "Tags": tag_json(&batch) }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {case:?}: expected CreateGroup to answer 400 BadRequestException, \
             got {status} {body}",
        );
        let (read_status, read_body) = emulator
            .call("/get-group", &json!({ "Group": name }))
            .await;
        assert_eq!(
            (read_status, error_code(&read_body)),
            (404, "NotFoundException"),
            "case {case:?}: a refused create must create nothing, got {read_status} {read_body}",
        );
    }
}

/// The store's ceiling on how many tags one group holds: a batch that fills it is
/// accepted, one that would exceed it is refused and stores nothing, and a batch
/// that only overwrites stored keys is accepted at the ceiling because it grows
/// the map by nothing.
#[tokio::test]
async fn the_tags_per_group_ceiling_is_enforced_atomically() {
    let emulator = Emulator::new();
    emulator.ok("/groups", &json!({ "Name": "bounded" })).await;
    let arn = group_arn("us-east-1", "bounded");

    let full: Vec<(String, String)> = (0..MAX_TAGS_PER_GROUP)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let case = "a batch exactly at the ceiling";
    let (status, body) = tag(&emulator, "us-east-1", &arn, &tag_json(&full)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body}",
    );
    let at_the_bound = stored_tags(&emulator, case, &arn).await;
    assert_eq!(
        at_the_bound.len(),
        MAX_TAGS_PER_GROUP,
        "case {case:?}: expected {MAX_TAGS_PER_GROUP} stored tags, got {at_the_bound:?}",
    );

    let case = "overwriting a stored key at the ceiling";
    let (status, body) = tag(
        &emulator,
        "us-east-1",
        &arn,
        &tag_json(&[("key-0".to_string(), "changed".to_string())]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body}",
    );
    let overwritten = stored_tags(&emulator, case, &arn).await;
    assert_eq!(
        overwritten.get("key-0").map(String::as_str),
        Some("changed"),
        "case {case:?}: expected the overwrite to be stored, got {overwritten:?}",
    );
    assert_eq!(
        overwritten.len(),
        MAX_TAGS_PER_GROUP,
        "case {case:?}: an overwrite must not grow the map, got {overwritten:?}",
    );

    let case = "one new key past the ceiling";
    let (status, body) = tag(
        &emulator,
        "us-east-1",
        &arn,
        &tag_json(&[("one-too-many".to_string(), "x".to_string())]),
    )
    .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case {case:?}: expected 400 BadRequestException, got {status} {body}",
    );
    let after = stored_tags(&emulator, case, &arn).await;
    assert_eq!(
        after, overwritten,
        "case {case:?}: the refusal must store nothing, expected {overwritten:?}, got {after:?}",
    );

    // `CreateGroup`'s own `Tags` member answers to the same ceiling, and a refused
    // create creates nothing.
    let case = "a create one pair past the ceiling";
    let over: Vec<(String, String)> = (0..=MAX_TAGS_PER_GROUP)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let (status, body) = emulator
        .call(
            "/groups",
            &json!({ "Name": "over-the-ceiling", "Tags": tag_json(&over) }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case {case:?}: expected 400 BadRequestException, got {status} {body}",
    );
    let (read_status, read_body) = emulator
        .call("/get-group", &json!({ "Group": "over-the-ceiling" }))
        .await;
    assert_eq!(
        (read_status, error_code(&read_body)),
        (404, "NotFoundException"),
        "case {case:?}: a refused create must create nothing, got {read_status} {read_body}",
    );
}
