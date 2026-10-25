// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `TagResource`, driven over HTTP through the same router the
//! emulator serves with.
//!
//! The rule under test, asserted over a TABLE of inputs rather than the one recorded
//! request: `TagResource` MERGES the requested tag map into the tags persisted on the
//! resource its `resourceArn` names — a key the resource does not hold is added, a key it
//! already holds has its value replaced, and every other tag is left alone — for EVERY
//! resource family this service holds tags on; a request that would leave a resource
//! holding more tags than its ceiling is refused with HTTP 400 and the error code this
//! endpoint words a tag set past that ceiling with (`BadRequestException`), storing
//! nothing; and an ARN that names no resource this account holds is the modelled
//! `ResourceNotFoundException` (HTTP 404).
//!
//! The rows differ along every axis the rule generalises over:
//!
//! * the resource family the ARN belongs to — an assistant, a knowledge base, a session
//!   and an assistant association, the four families this model's implemented surface
//!   creates;
//! * what the resource already holds (nothing, a few pairs, one short of the ceiling,
//!   the ceiling itself) and what the merge carries (nothing, one pair, several, keys
//!   that collide with stored ones, keys that do not, both published length bounds of
//!   `TagKey` and `TagValue`, and the punctuation the modelled `TagKey` pattern admits);
//! * for the ceiling rows: a merge that lands one short of, exactly on, and one past the
//!   maximum, and a merge that only re-values stored keys;
//! * for a not-found row: the family label, the id, the region, the account and the
//!   partition the ARN names, and whether the resource it named was deleted.
//!
//! Every assertion reads the result back over the wire — through `ListTagsForResource`
//! and, for the two top-level families, through the resource's own `Get` — so a response
//! that is not computed from persisted state cannot pass. The harness (the router, the
//! creates, the ARN forms) is the one `resource_tags.rs` already drives this surface
//! with.

use std::collections::BTreeMap;

use serde_json::json;
use service_com_amazonaws_wisdom_wisdomservice::state;

use super::resource_tags::{
    create_tagged, label, tag_map, wisdom_arn, Emulator, Family, Reply, ABSENT_UUID, FAMILIES,
    OTHER_ACCOUNT, OTHER_REGION, REGION,
};

/// The published bounds of the modelled tag map's key and value
/// (`smithy_query(query="shape",shape="Tags")`: `TagKey` len 1..128, `TagValue` len
/// 1..256).
const TAG_KEY_MAX: usize = 128;
const TAG_VALUE_MAX: usize = 256;

/// `TagResource` on one resource ARN with one tag map.
async fn tag_resource(
    emulator: &Emulator,
    resource_arn: &str,
    tags: &BTreeMap<String, String>,
) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/tags/{}", label(resource_arn)),
            Some(json!({ "tags": tags })),
        )
        .await
}

/// The tag map `ListTagsForResource` reports for one ARN, as a sorted map, so a row
/// compares what is PERSISTED rather than what a call happened to answer.
async fn stored_tags(
    emulator: &Emulator,
    case: &str,
    resource_arn: &str,
) -> BTreeMap<String, String> {
    let listed = emulator.list_tags(resource_arn).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected ListTagsForResource to answer HTTP 200 for \
         {resource_arn}, got {} with {}",
        listed.status, listed.body
    );
    serde_json::from_value(listed.field("/tags").cloned().unwrap_or(json!({})))
        .unwrap_or_else(|error| panic!("case {case:?}: expected a tag object, got {error}"))
}

/// One row of the merge table.
#[derive(Debug, Clone)]
struct MergeCase {
    label: &'static str,
    /// What the create persists on the resource first.
    initial: BTreeMap<String, String>,
    /// What the `TagResource` call carries.
    merge: BTreeMap<String, String>,
}

impl MergeCase {
    /// The tag set the rule says the resource holds afterwards: the stored map with the
    /// merge upserted into it.
    fn expected(&self) -> BTreeMap<String, String> {
        let mut expected = self.initial.clone();
        for (key, value) in &self.merge {
            expected.insert(key.clone(), value.clone());
        }
        expected
    }
}

/// Keys and values written as a map, so a row reads as the pairs it is about.
fn pairs(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// The merge table: what the resource holds before, crossed with what the call carries.
fn merge_cases() -> Vec<MergeCase> {
    vec![
        MergeCase {
            label: "one pair onto an untagged resource",
            initial: BTreeMap::new(),
            merge: tag_map(1),
        },
        MergeCase {
            label: "several pairs onto an untagged resource",
            initial: BTreeMap::new(),
            merge: tag_map(3),
        },
        MergeCase {
            label: "a key the resource does not hold, beside stored keys",
            initial: tag_map(2),
            merge: pairs(&[("fresh", "added")]),
        },
        MergeCase {
            label: "a stored key, re-valued",
            initial: tag_map(2),
            merge: pairs(&[("key-0", "replaced")]),
        },
        MergeCase {
            label: "a new key and a re-valued one together",
            initial: tag_map(2),
            merge: pairs(&[("key-1", "replaced"), ("fresh", "added")]),
        },
        MergeCase {
            label: "an empty merge leaves the stored map alone",
            initial: tag_map(2),
            merge: BTreeMap::new(),
        },
        MergeCase {
            // Source: the modelled `TagKey` pattern, `^(?!aws:)[a-zA-Z+-=._:/]+$`
            // (`smithy_query(query="shape",shape="Tags")`).
            label: "the punctuation the modelled TagKey pattern admits",
            initial: pairs(&[("Env", "Staging")]),
            merge: pairs(&[
                ("Env", "Production"),
                ("a.b", "1"),
                ("team_1", "alpha"),
                ("path/sub", "nested"),
                ("plus+eq=", "signs"),
                ("ns:name", "colon"),
                ("dash-ed", "2"),
            ]),
        },
        MergeCase {
            label: "the published length bounds of TagKey and TagValue",
            initial: tag_map(1),
            merge: [
                ("k".repeat(TAG_KEY_MAX), "v".repeat(TAG_VALUE_MAX)),
                ("k".to_string(), "v".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        MergeCase {
            label: "a merge that fills the resource to its tag ceiling",
            initial: tag_map(1),
            merge: tag_map(state::MAX_TAGS_PER_RESOURCE),
        },
    ]
}

/// For every family and every row of the merge table, the tags the resource holds
/// afterwards are the stored map with the request upserted into it — read back over the
/// wire — and merging the same pairs again changes nothing.
#[tokio::test]
async fn tag_resource_merges_the_request_into_the_tags_stored_on_the_resource() {
    let emulator = Emulator::new();
    let host = emulator
        .create_assistant("merge-host-assistant", &BTreeMap::new())
        .await;
    assert_eq!(
        host.status, 200,
        "case \"host assistant\": expected HTTP 200, got {} with {}",
        host.status, host.body
    );
    let host_assistant = host.string("/assistant/assistantId");

    // A resource nothing in this test tags: no merge may reach it.
    let bystander_tags = tag_map(2);
    let bystander = emulator
        .create_knowledge_base("merge-bystander-kb", &bystander_tags)
        .await;
    let bystander_arn = bystander.string("/knowledgeBase/knowledgeBaseArn");

    for (seq, case) in merge_cases().into_iter().enumerate() {
        for family in FAMILIES {
            let seq = 1_000 + seq * FAMILIES.len() + family as usize;
            let (arn, _) = create_tagged(
                &emulator,
                family,
                seq,
                case.label,
                &host_assistant,
                &case.initial,
            )
            .await;

            let tagged = tag_resource(&emulator, &arn, &case.merge).await;
            assert_eq!(
                tagged.status, 200,
                "case {:?} ({family:?}): expected TagResource to answer HTTP 200 for \
                 {arn}, got {} with {}",
                case.label, tagged.status, tagged.body
            );
            assert!(
                tagged.error_type.is_empty(),
                "case {:?} ({family:?}): expected no modelled failure, got \
                 x-amzn-errortype {:?}",
                case.label,
                tagged.error_type
            );

            let expected = case.expected();
            let actual = stored_tags(&emulator, case.label, &arn).await;
            assert_eq!(
                actual, expected,
                "case {:?} ({family:?}): expected {arn} to hold {expected:?}, got {actual:?}",
                case.label
            );

            // `@idempotent`: merging the same pairs again leaves the same set.
            let repeated = tag_resource(&emulator, &arn, &case.merge).await;
            assert_eq!(
                repeated.status, 200,
                "case {:?} ({family:?}): expected a repeated merge to answer HTTP 200, \
                 got {} with {}",
                case.label, repeated.status, repeated.body
            );
            let after_repeat = stored_tags(&emulator, case.label, &arn).await;
            assert_eq!(
                after_repeat, expected,
                "case {:?} ({family:?}): expected a repeated merge to hold {expected:?}, \
                 got {after_repeat:?}",
                case.label
            );

            // The resource's own read reports the SAME map, because there is one copy of
            // a resource's tags: proved for the two families with a reader of their own.
            if let Some((uri, pointer)) = read_back(family, &arn) {
                let read = emulator.send("GET", &uri, None).await;
                assert_eq!(
                    read.status, 200,
                    "case {:?} ({family:?}): expected the resource's own read to answer \
                     HTTP 200, got {} with {}",
                    case.label, read.status, read.body
                );
                let reported: BTreeMap<String, String> =
                    serde_json::from_value(read.field(pointer).cloned().unwrap_or(json!({})))
                        .expect("the resource's tags member is an object");
                assert_eq!(
                    reported, expected,
                    "case {:?} ({family:?}): the resource's own read reported \
                     {reported:?} while its tags are {expected:?}",
                    case.label
                );
            }
        }
    }

    let untouched = stored_tags(&emulator, "the bystander", &bystander_arn).await;
    assert_eq!(
        untouched, bystander_tags,
        "case \"the bystander\": expected a resource nothing tagged to keep \
         {bystander_tags:?}, got {untouched:?}"
    );
}

/// The resource's own `Get` for the families that have one, as (URI, pointer to its
/// `tags` member). A session and an association are read through
/// `ListTagsForResource` alone here; both are addressed under their parent assistant,
/// which this table does not carry.
fn read_back(family: Family, arn: &str) -> Option<(String, &'static str)> {
    match family {
        Family::Assistant => Some((format!("/assistants/{}", label(arn)), "/assistant/tags")),
        Family::KnowledgeBase => Some((
            format!("/knowledgeBases/{}", label(arn)),
            "/knowledgeBase/tags",
        )),
        Family::Session | Family::Association => None,
    }
}

/// A merge is accepted exactly while the RESULTING tag set stays within the maximum a
/// resource carries: one short of it and on it are stored, one past it is refused and
/// stores nothing, and a merge that only re-values stored keys is accepted even at the
/// ceiling.
#[tokio::test]
async fn a_merge_past_the_tag_ceiling_is_refused_and_stores_nothing() {
    let maximum = state::MAX_TAGS_PER_RESOURCE;
    let emulator = Emulator::new();
    let host = emulator
        .create_assistant("ceiling-host-assistant", &BTreeMap::new())
        .await;
    let host_assistant = host.string("/assistant/assistantId");

    for family in FAMILIES {
        // One key short of the ceiling, so the next distinct key is the last that fits.
        let held = tag_map(maximum - 1);
        let (arn, _) = create_tagged(
            &emulator,
            family,
            2_000 + family as usize,
            "one key short of the ceiling",
            &host_assistant,
            &held,
        )
        .await;

        // Two distinct new keys would leave the resource one past the maximum.
        let case = format!("two new keys one short of the ceiling ({family:?})");
        let refused = tag_resource(
            &emulator,
            &arn,
            &pairs(&[("overflow-a", "1"), ("overflow-b", "2")]),
        )
        .await;
        assert_bad_request(&refused, &case);
        let after_refusal = stored_tags(&emulator, &case, &arn).await;
        assert_eq!(
            after_refusal, held,
            "case {case:?}: expected a refused merge to store nothing, so {} pairs \
             remain, got {after_refusal:?}",
            held.len()
        );

        // Re-valuing stored keys is accepted here and at the ceiling: it grows nothing.
        let case = format!("re-valuing a stored key ({family:?})");
        let revalued = tag_resource(&emulator, &arn, &pairs(&[("key-0", "revalued")])).await;
        assert_eq!(
            revalued.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            revalued.status, revalued.body
        );
        let mut expected = held.clone();
        expected.insert("key-0".to_string(), "revalued".to_string());
        let after_revalue = stored_tags(&emulator, &case, &arn).await;
        assert_eq!(
            after_revalue, expected,
            "case {case:?}: expected {expected:?}, got {after_revalue:?}"
        );

        // The one key that still fits reaches the ceiling exactly.
        let case = format!("the key that reaches the ceiling ({family:?})");
        let accepted = tag_resource(&emulator, &arn, &pairs(&[("last-slot", "taken")])).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            accepted.status, accepted.body
        );
        expected.insert("last-slot".to_string(), "taken".to_string());
        let at_ceiling = stored_tags(&emulator, &case, &arn).await;
        assert_eq!(
            at_ceiling, expected,
            "case {case:?}: expected {maximum} pairs, got {} ({at_ceiling:?})",
            at_ceiling.len()
        );

        // At the ceiling, one more distinct key is refused; re-valuing is not.
        let case = format!("one new key at the ceiling ({family:?})");
        let refused = tag_resource(&emulator, &arn, &pairs(&[("past-the-end", "1")])).await;
        assert_bad_request(&refused, &case);
        let unchanged = stored_tags(&emulator, &case, &arn).await;
        assert_eq!(
            unchanged, expected,
            "case {case:?}: expected the refused merge to leave {maximum} pairs, got \
             {unchanged:?}"
        );

        let case = format!("re-valuing at the ceiling ({family:?})");
        let revalued = tag_resource(&emulator, &arn, &pairs(&[("last-slot", "again")])).await;
        assert_eq!(
            revalued.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            revalued.status, revalued.body
        );
        expected.insert("last-slot".to_string(), "again".to_string());
        let after = stored_tags(&emulator, &case, &arn).await;
        assert_eq!(
            after, expected,
            "case {case:?}: expected {expected:?}, got {after:?}"
        );
    }

    // A single merge is judged on the resulting count too: exactly the maximum onto an
    // untagged resource stores, and a request carrying more pairs than a resource may
    // hold is not read at all.
    for (label_text, count, accepted) in [
        (
            "a single merge of one short of the maximum",
            maximum - 1,
            true,
        ),
        ("a single merge of exactly the maximum", maximum, true),
        ("a single merge of one past the maximum", maximum + 1, false),
        ("a single merge far past the maximum", maximum * 3, false),
    ] {
        let created = emulator
            .create_knowledge_base(&format!("single-merge-kb-{count}"), &BTreeMap::new())
            .await;
        assert_eq!(
            created.status, 200,
            "case {label_text:?}: expected the host knowledge base to be created, got {} with {}",
            created.status, created.body
        );
        let arn = created.string("/knowledgeBase/knowledgeBaseArn");
        let merge = tag_map(count);
        let reply = tag_resource(&emulator, &arn, &merge).await;
        let stored = stored_tags(&emulator, label_text, &arn).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {label_text:?}: expected HTTP 200, got {} with {}",
                reply.status, reply.body
            );
            assert_eq!(
                stored, merge,
                "case {label_text:?}: expected {count} pairs stored, got {} ({stored:?})",
                stored.len()
            );
        } else {
            assert_bad_request(&reply, label_text);
            assert!(
                stored.is_empty(),
                "case {label_text:?}: expected a refused merge to store nothing, got {stored:?}"
            );
        }
    }
}

/// A request carrying more pairs than a resource may hold is refused with the same code
/// and status, and is answered before the ARN is resolved, so an ARN that names nothing
/// gets the same answer as one that names a stored resource.
#[tokio::test]
async fn a_request_carrying_more_pairs_than_a_resource_may_hold_is_not_read() {
    let maximum = state::MAX_TAGS_PER_RESOURCE;
    let emulator = Emulator::new();
    let created = emulator
        .create_assistant("oversized-request-assistant", &tag_map(1))
        .await;
    let stored_arn = created.string("/assistant/assistantArn");
    let account = engine::EMULATOR_ACCOUNT_ID;
    let absent_arn = wisdom_arn("aws", REGION, account, &format!("assistant/{ABSENT_UUID}"));

    for (case, arn) in [
        ("a stored resource", stored_arn.as_str()),
        ("an ARN naming nothing", absent_arn.as_str()),
    ] {
        for count in [maximum + 1, maximum + 2, maximum * 10] {
            let case = format!("{case}, {count} pairs");
            let reply = tag_resource(&emulator, arn, &tag_map(count)).await;
            assert_bad_request(&reply, &case);
        }
    }

    // Nothing was written: the stored resource still carries what it was created with.
    let stored = stored_tags(&emulator, "the stored resource", &stored_arn).await;
    assert_eq!(
        stored,
        tag_map(1),
        "case \"the stored resource\": expected the created tags to be untouched, got \
         {stored:?}"
    );
}

/// One refusal of a tag set past the ceiling: HTTP 400 and the code this endpoint words
/// that class with.
fn assert_bad_request(reply: &Reply, case: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {}",
        reply.status, reply.body
    );
    assert!(
        reply.error_type.contains("BadRequestException"),
        "case {case:?}: expected the BadRequestException code, got x-amzn-errortype {:?}",
        reply.error_type
    );
}

/// An ARN this account holds no resource under is the modelled
/// `ResourceNotFoundException` (HTTP 404), whatever makes it miss — and the merge stores
/// nothing, so the ARN still names nothing afterwards.
#[tokio::test]
async fn tagging_an_arn_naming_no_resource_this_account_holds_is_not_found() {
    let emulator = Emulator::new();
    let assistant = emulator
        .create_assistant("miss-merge-assistant", &tag_map(1))
        .await;
    let assistant_id = assistant.string("/assistant/assistantId");
    let assistant_arn = assistant.string("/assistant/assistantArn");
    let session = emulator
        .create_session(&assistant_id, "miss-merge-session", &tag_map(1))
        .await;
    let session_id = session.string("/session/sessionId");
    let knowledge_base = emulator
        .create_knowledge_base("miss-merge-kb", &tag_map(1))
        .await;
    let knowledge_base_id = knowledge_base.string("/knowledgeBase/knowledgeBaseId");
    let account = engine::EMULATOR_ACCOUNT_ID;

    let mut cases = vec![
        (
            "the right region and account, an id nothing holds",
            wisdom_arn("aws", REGION, account, &format!("assistant/{ABSENT_UUID}")),
        ),
        (
            "a stored assistant's id under another family's label",
            wisdom_arn(
                "aws",
                REGION,
                account,
                &format!("knowledge-base/{assistant_id}"),
            ),
        ),
        (
            "a stored knowledge base's id under another family's label",
            wisdom_arn(
                "aws",
                REGION,
                account,
                &format!("assistant/{knowledge_base_id}"),
            ),
        ),
        (
            "a stored assistant in another region",
            wisdom_arn(
                "aws",
                OTHER_REGION,
                account,
                &format!("assistant/{assistant_id}"),
            ),
        ),
        (
            "a stored assistant under another account",
            wisdom_arn(
                "aws",
                REGION,
                OTHER_ACCOUNT,
                &format!("assistant/{assistant_id}"),
            ),
        ),
        (
            "a stored assistant in another partition",
            wisdom_arn(
                "aws-cn",
                REGION,
                account,
                &format!("assistant/{assistant_id}"),
            ),
        ),
        (
            "a stored session under another assistant as its parent",
            wisdom_arn(
                "aws",
                REGION,
                account,
                &format!("session/{ABSENT_UUID}/{session_id}"),
            ),
        ),
        (
            "a nested id nothing holds under a stored assistant",
            wisdom_arn(
                "aws",
                REGION,
                account,
                &format!("association/{assistant_id}/{ABSENT_UUID}"),
            ),
        ),
    ];

    // A resource a DELETE removed carries no tags any more: its ARN names nothing, so a
    // merge against it cannot resurrect one.
    assert_eq!(
        emulator
            .delete_knowledge_base(&knowledge_base_id)
            .await
            .status,
        204,
        "case \"delete the host knowledge base\": expected the modelled 204"
    );
    cases.push((
        "a knowledge base a delete removed",
        knowledge_base.string("/knowledgeBase/knowledgeBaseArn"),
    ));
    assert_eq!(
        emulator.delete_assistant(&assistant_id).await.status,
        204,
        "case \"delete the host assistant\": expected the modelled 204"
    );
    cases.push(("an assistant a delete removed", assistant_arn));
    cases.push((
        "a session whose assistant a delete removed",
        session.string("/session/sessionArn"),
    ));

    for (case, arn) in &cases {
        let reply = tag_resource(&emulator, arn, &tag_map(1)).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404 for {arn}, got {} with {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.contains("ResourceNotFoundException"),
            "case {case:?}: expected the modelled ResourceNotFoundException for {arn}, \
             got x-amzn-errortype {:?}",
            reply.error_type
        );
        assert_eq!(
            reply.string("/resourceName"),
            *arn,
            "case {case:?}: expected the refusal to name the ARN asked about in \
             resourceName, got {:?}",
            reply.string("/resourceName")
        );
        // Nothing was stored under it: the follow-up read misses in the same way.
        let listed = emulator.list_tags(arn).await;
        assert_eq!(
            listed.status, 404,
            "case {case:?}: expected the refused merge to leave {arn} naming nothing, \
             got {} with {}",
            listed.status, listed.body
        );
    }
}

/// A `resourceArn` outside the modelled `Arn` pattern is refused as a bad request
/// (HTTP 400 `ValidationException`), not as a resource that happens to be missing: the
/// pattern is a modelled constraint, so the refusal happens before the lookup and before
/// any write.
#[tokio::test]
async fn a_resource_arn_outside_the_modelled_pattern_is_refused_before_any_write() {
    let emulator = Emulator::new();
    let created = emulator
        .create_knowledge_base("constraint-host-kb", &tag_map(1))
        .await;
    let arn = created.string("/knowledgeBase/knowledgeBaseArn");
    let account = engine::EMULATOR_ACCOUNT_ID;

    let cases: Vec<(&str, String)> = vec![
        ("a bare uuid, not an ARN", ABSENT_UUID.to_string()),
        (
            "an ARN of another service",
            format!("arn:aws:sns:{REGION}:{account}:topic/{ABSENT_UUID}"),
        ),
        (
            "a wisdom ARN whose resource id is no uuid",
            wisdom_arn("aws", REGION, account, "assistant/not-a-uuid"),
        ),
        (
            "a wisdom ARN with no resource id at all",
            wisdom_arn("aws", REGION, account, "assistant"),
        ),
    ];

    for (case, resource_arn) in &cases {
        let reply = tag_resource(&emulator, resource_arn, &tag_map(1)).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 for {resource_arn}, got {} with {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.contains("ValidationException"),
            "case {case:?}: expected the modelled ValidationException, got \
             x-amzn-errortype {:?}",
            reply.error_type
        );
    }

    // None of those reached the store: the host knowledge base holds what it was created
    // with.
    let stored = stored_tags(&emulator, "the host knowledge base", &arn).await;
    assert_eq!(
        stored,
        tag_map(1),
        "case \"the host knowledge base\": expected the created tags to be untouched, \
         got {stored:?}"
    );
}
