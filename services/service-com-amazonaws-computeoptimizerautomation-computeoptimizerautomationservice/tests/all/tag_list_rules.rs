// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the judgement every operation makes of a caller-supplied
//! `tags` list.
//!
//! The rule under test: wherever this service accepts a `tags` list, the list is
//! judged by the same rules in the same order, so one list is refused with one
//! and the same HTTP status, error code and message whichever operation received
//! it. Two rules apply to a `tags` member — the list as a whole may not name the
//! same key twice (a tag list states one value per key), and no key may be
//! outside the modelled form or inside the provider-reserved namespace — and a
//! list that breaks more than one of them has exactly one answer, because the
//! order the rules are applied in is the service's
//! ([`state::tag_list_violation`]) and not each handler's.
//!
//! The two operations that take a `tags` list are `CreateAutomationRule` and
//! `TagResource` (`UntagResource` takes bare keys, where the same-key-twice rule
//! has nothing to say: a key named twice for removal names one key). Every case
//! drives both of them over the wire through the same router the emulator binary
//! mounts (the harness lives in [`super::automation_rule_lifecycle`]).

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::Tag;
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;

use super::automation_rule_lifecycle::{
    call, call_ok, emulator, error_code, member, minimal_rule, string_member,
};

/// Which rule a row's `tags` list is expected to be refused by.
#[derive(Debug, Clone, Copy)]
enum Refusal {
    /// The rule about the LIST: it names the same key twice.
    SameKeyTwice,
    /// The rule about ONE KEY: it is in the provider-reserved namespace.
    ReservedKey,
    /// The modelled FORM of a key, which the protocol boundary owns (`Tag$key`
    /// targets `TagKey`, `@length(1..128)` with pattern
    /// `^[\w\s\.\-\:\/\=\+\@]+$` —
    /// `smithy_query(query="shape",shape="Tag,TagKey")`). The body is the
    /// validator's, not this crate's, so such a row pins only that the two
    /// paths answer it identically.
    ModelledKeyForm,
}

/// One row of a table below: a `tags` list, as the key/value pairs the request
/// states and in that order.
struct TagList {
    case: String,
    pairs: Vec<(String, String)>,
}

impl TagList {
    fn new(case: &str, pairs: &[(&str, &str)]) -> Self {
        Self {
            case: case.to_string(),
            pairs: pairs
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        }
    }

    /// The list as the wire sends it.
    fn json(&self) -> Value {
        Value::Array(
            self.pairs
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect(),
        )
    }

    /// The same list as the shapes the service's own judgement reads.
    fn shapes(&self) -> Vec<Tag> {
        self.pairs
            .iter()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

/// The message the service words for `refusal` over `tags`, where this crate
/// owns the rule — taken from the service's own message builders, so no wording
/// is hand-copied here.
fn expected_message(refusal: Refusal, tags: &[Tag]) -> Option<String> {
    match refusal {
        Refusal::SameKeyTwice => Some(
            state::duplicate_tag_key_violation(tags)
                .expect("a same-key-twice row names one key twice"),
        ),
        Refusal::ReservedKey => Some(state::reserved_tag_key_message()),
        Refusal::ModelledKeyForm => None,
    }
}

/// The wording of each of the two refusals, pinned LITERALLY: an error message
/// is part of what a caller parses, so a drift in either sentence has to fail
/// here rather than pass because the expectation asked the service what it says.
/// One row per rule, over a list each rule alone refuses.
#[test]
fn each_tag_list_refusal_carries_its_sentence() {
    let reserved = format!("{}owner", state::RESERVED_TAG_KEY_PREFIX);
    let rows = [
        (
            TagList::new("a-reserved-key", &[(reserved.as_str(), "x")]),
            "Tag keys beginning with aws: are reserved for Amazon Web Services use and cannot be \
             specified",
        ),
        (
            TagList::new("the-same-key-twice", &[("team", "a"), ("team", "b")]),
            "1 validation error detected: Value at 'tags' failed to satisfy constraint: Member \
             must not name the same tag key more than once",
        ),
    ];
    for (row, sentence) in rows {
        let judged = state::tag_list_violation(&row.shapes());
        assert_eq!(
            judged.as_deref(),
            Some(sentence),
            "case {:?}: expected the refusal to read {sentence:?}, got {judged:?}",
            row.case
        );
    }
}

/// Tag lists a write path must refuse, spanning what the rule generalises over:
/// which rules the list breaks, where in the list the offending entries sit, how
/// many entries the list carries, the case of the reserved prefix and whether the
/// repeated key is itself the reserved one.
///
/// The reserved prefix is never spelled here: it comes from the service's own
/// constant `state::RESERVED_TAG_KEY_PREFIX`.
fn refused_rows() -> Vec<(TagList, Refusal)> {
    let prefix = state::RESERVED_TAG_KEY_PREFIX;
    let mixed_case_prefix: String = prefix
        .chars()
        .enumerate()
        .map(|(index, c)| {
            if index % 2 == 0 {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect();
    let reserved = format!("{prefix}owner");
    let reserved_mixed = format!("{mixed_case_prefix}owner");

    let mut rows = vec![
        (
            TagList::new("the-same-key-twice", &[("team", "a"), ("team", "b")]),
            Refusal::SameKeyTwice,
        ),
        (
            TagList::new("a-reserved-key-alone", &[(reserved.as_str(), "x")]),
            Refusal::ReservedKey,
        ),
        (
            TagList::new(
                "a-reserved-key-among-distinct-keys",
                &[("team", "a"), ("env", "prod"), (reserved.as_str(), "x")],
            ),
            Refusal::ReservedKey,
        ),
        // Both rules at once, with the reserved key FIRST in the list…
        (
            TagList::new(
                "both-rules-reserved-key-first",
                &[(reserved.as_str(), "x"), ("team", "a"), ("team", "b")],
            ),
            Refusal::SameKeyTwice,
        ),
        // …and with the repeat first, so neither path can win by position.
        (
            TagList::new(
                "both-rules-repeat-first",
                &[("team", "a"), ("team", "b"), (reserved.as_str(), "x")],
            ),
            Refusal::SameKeyTwice,
        ),
        (
            TagList::new(
                "both-rules-the-reserved-key-repeated",
                &[(reserved.as_str(), "x"), (reserved.as_str(), "y")],
            ),
            Refusal::SameKeyTwice,
        ),
        (
            TagList::new(
                "both-rules-with-the-prefix-in-mixed-case",
                &[(reserved_mixed.as_str(), "x"), ("env", "a"), ("env", "b")],
            ),
            Refusal::SameKeyTwice,
        ),
        // A key whose FORM the model refuses, beside a repeat: the body belongs
        // to the boundary validator, and the two paths must still agree.
        (
            TagList::new(
                "a-malformed-key-beside-a-repeat",
                &[("bad!key", "x"), ("team", "a"), ("team", "b")],
            ),
            Refusal::ModelledKeyForm,
        ),
    ];

    // A longer list, so the count the rule holds over is not always two or
    // three: nine ordinary tags, then a reserved key, then a repeat of the
    // first ordinary key.
    let mut long: Vec<(String, String)> = (0..9)
        .map(|index| (format!("team-{index}"), format!("{index}")))
        .collect();
    long.push((reserved.clone(), "x".to_string()));
    long.push(("team-0".to_string(), "again".to_string()));
    rows.push((
        TagList {
            case: "both-rules-in-a-longer-list".to_string(),
            pairs: long,
        },
        Refusal::SameKeyTwice,
    ));

    rows
}

/// Tag lists both rules admit, spanning the near misses: keys differing only in
/// case (a tag key is case-sensitive, so those are different keys), the reserved
/// prefix's characters somewhere other than the start of the key, and a list of
/// one as well as of several.
fn accepted_rows() -> Vec<TagList> {
    let prefix = state::RESERVED_TAG_KEY_PREFIX;
    let undelimited: String = prefix.chars().filter(|c| c.is_alphanumeric()).collect();
    let inside = format!("team{prefix}owner");
    let without_delimiter = format!("{undelimited}-managed");
    let mut rows = vec![
        TagList::new("one-ordinary-tag", &[("team", "platform")]),
        TagList::new(
            "keys-differing-only-in-case",
            &[("env", "prod"), ("Env", "staging"), ("ENV", "dev")],
        ),
        TagList::new(
            "the-reserved-prefix-inside-the-key",
            &[(inside.as_str(), "x"), (without_delimiter.as_str(), "y")],
        ),
    ];
    rows.push(TagList {
        case: "several-distinct-keys".to_string(),
        pairs: (0..5)
            .map(|index| (format!("key-{index}"), format!("{index}")))
            .collect(),
    });
    rows
}

/// The tag the rule a tagging case starts from already carries.
fn initial_tags() -> Value {
    json!([{"key": "keep", "value": "me"}])
}

/// Create a rule carrying `tags` and answer `(ruleArn, ruleRevision)`.
async fn rule_with_tags(
    router: &smithy_server_runtime::MultiServiceRouter,
    tags: &Value,
    case: &str,
) -> (String, i64) {
    let mut request = minimal_rule("tag-list-rules");
    request["tags"] = tags.clone();
    let created = call_ok(router, "CreateAutomationRule", &request, case).await;
    (
        string_member(&created, "ruleArn"),
        member(&created, "ruleRevision")
            .as_i64()
            .expect("the created rule carries a revision"),
    )
}

/// The tag list `ListTagsForResource` reports for `rule_arn`.
async fn listed_tags(
    router: &smithy_server_runtime::MultiServiceRouter,
    rule_arn: &str,
    case: &str,
) -> Value {
    let body = call_ok(
        router,
        "ListTagsForResource",
        &json!({"resourceArn": rule_arn}),
        case,
    )
    .await;
    member(&body, "tags").clone()
}

/// `(status, error code, message)` of a `CreateAutomationRule` carrying `tags`.
/// A refused create must store no rule, which is asserted here.
async fn create_answer(tags: &Value, case: &str) -> (u16, String, String) {
    let router = emulator();
    let mut request = minimal_rule("tag-list-rules");
    request["tags"] = tags.clone();

    let (status, body) = call(&router, "CreateAutomationRule", &request).await;
    if status != 200 {
        let listed = call_ok(&router, "ListAutomationRules", &json!({}), case).await;
        let stored = member(&listed, "automationRules")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            stored, 0,
            "case {case:?}: a refused create must store no rule, got {listed}"
        );
    }
    (status, error_code(&body), string_member(&body, "message"))
}

/// `(status, error code, message)` of a `TagResource` carrying the same `tags`,
/// applied to a rule that already holds one ordinary tag. A refused tagging must
/// leave that stored list exactly as it was, which is asserted here.
async fn tag_answer(tags: &Value, case: &str) -> (u16, String, String) {
    let router = emulator();
    let initial = initial_tags();
    let (rule_arn, revision) = rule_with_tags(&router, &initial, case).await;

    let (status, body) = call(
        &router,
        "TagResource",
        &json!({"resourceArn": &rule_arn, "ruleRevision": revision, "tags": tags}),
    )
    .await;
    if status != 200 {
        let listed = listed_tags(&router, &rule_arn, case).await;
        assert_eq!(
            listed, initial,
            "case {case:?}: a refused tagging must add nothing, got {listed}"
        );
    }
    (status, error_code(&body), string_member(&body, "message"))
}

/// One bad `tags` list has ONE answer: the create path and the tagging path
/// refuse it with the same status, the same modelled error code and the same
/// message — including a list that breaks both of this crate's tag-list rules at
/// once, whichever of the two offending entries the list states first.
#[tokio::test]
async fn both_write_paths_refuse_one_tag_list_the_same_way() {
    for (row, refusal) in refused_rows() {
        let case = row.case.as_str();
        let tags = row.json();

        let created = create_answer(&tags, case).await;
        let tagged = tag_answer(&tags, case).await;

        assert_eq!(
            created, tagged,
            "case {case:?}: CreateAutomationRule and TagResource must refuse one list the same \
             way, got create {created:?} and tagging {tagged:?}"
        );
        assert_eq!(
            created.0, 400,
            "case {case:?}: expected HTTP 400, got {created:?}"
        );
        assert_eq!(
            created.1, "InvalidParameterValueException",
            "case {case:?}: expected the modelled client-input error, got {created:?}"
        );

        // Where this crate owns the rule, the message is the one the service
        // words for it — so the two paths agree on a BODY, not merely on a code.
        match expected_message(refusal, &row.shapes()) {
            Some(expected) => {
                assert_eq!(
                    created.2, expected,
                    "case {case:?}: expected the message for {refusal:?}, got {created:?}"
                );
                assert_eq!(
                    state::tag_list_violation(&row.shapes()),
                    Some(expected),
                    "case {case:?}: the service's own judgement must word the same refusal, got \
                     {created:?}"
                );
            }
            None => assert!(
                !created.2.is_empty(),
                "case {case:?}: a refusal must carry a message, got {created:?}"
            ),
        }
    }
}

/// A list both rules admit is accepted by both paths and round-trips: the create
/// path stores it as sent, and the tagging path merges it into what the rule
/// already holds.
#[tokio::test]
async fn both_write_paths_accept_a_list_that_breaks_neither_rule() {
    for row in accepted_rows() {
        let case = row.case.as_str();
        let tags = row.json();

        assert_eq!(
            state::tag_list_violation(&row.shapes()),
            None,
            "case {case:?}: expected {tags} to break neither rule, got a refusal"
        );

        // The create path: stored as sent, read back through the tag reader.
        let router = emulator();
        let (rule_arn, _) = rule_with_tags(&router, &tags, case).await;
        let listed = listed_tags(&router, &rule_arn, case).await;
        assert_eq!(
            listed, tags,
            "case {case:?}: a create must store the list it was sent, got {listed}"
        );

        // The tagging path: merged into the one tag the rule already holds.
        let router = emulator();
        let initial = initial_tags();
        let (rule_arn, revision) = rule_with_tags(&router, &initial, case).await;
        call_ok(
            &router,
            "TagResource",
            &json!({"resourceArn": &rule_arn, "ruleRevision": revision, "tags": tags}),
            case,
        )
        .await;
        let mut expected = initial.as_array().expect("the initial list").clone();
        expected.extend(tags.as_array().expect("the row is a list").iter().cloned());
        let listed = listed_tags(&router, &rule_arn, case).await;
        assert_eq!(
            listed,
            Value::Array(expected),
            "case {case:?}: tagging must append each key the rule did not carry, got {listed}"
        );
    }
}
