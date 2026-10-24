// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Firewall Manager's `UntagResource`.
//!
//! The rule asserted here, in one sentence: a tag-removal call drops from the
//! named resource's persisted tag set exactly the keys the request lists —
//! matched by key alone and case-sensitively — leaving every other key it
//! carries, and the values of those keys, untouched; a listed key the resource
//! does not carry is no error, so the call is idempotent.
//!
//! The tables below are the class the rule ranges over. The rows differ in the
//! NUMBER of keys named (none, one, several, every key held), in whether each
//! named key is held, in whether a key is named twice, in key FORM across the
//! modelled `TagKey` pattern (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, from
//! `smithy_query(shape="TagKey")`) and at that member's published length maximum
//! (128), and in the case of the key named against the case of the one held.
//! Every row is asserted on EVERY taggable resource kind, because the removal
//! names its resource by the model's generic `ResourceArn` and so cannot be keyed
//! to one kind.
//!
//! The refusal rows are the same class on the other side: a `ResourceArn` that is
//! not an ARN at all is a `ValidationException`, a well-formed ARN naming no
//! stored resource is the modelled `ResourceNotFoundException`, and either
//! refusal leaves every stored tag set exactly as it was. The organisation-scoped
//! administrator gate is asserted to come before all of it.
//!
//! The wire harness ([`crate::policy_lifecycle`]), the tag helpers
//! ([`crate::tagging_and_managing_admins`]) and the per-kind resource seeding
//! ([`crate::tagging_across_resource_kinds`]) are the shared ones, so this suite
//! cannot drift from its siblings in what it considers the same tag set.

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::Tag;
use service_com_amazonaws_fms_awsfms_20180101::state;

use crate::administrator_gated_operations::{associate_admin, seed_protocols_list, with_store};
use crate::error_message_members::message_member;
use crate::policy_lifecycle::{call, call_in, emulator_with_engine, error_code, ACCOUNT, REGION};
use crate::tagging_across_resource_kinds::{create_resource, Kind, KINDS};
use crate::tagging_and_managing_admins::{list_tags, tag_list};

/// A `ListId` (`com.amazonaws.fms#ListId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) for the protocols list the gate test seeds.
const PROTOCOLS_LIST_ID: &str = "44444444-5555-6666-7777-888888888888";

/// A `PolicyId` (`com.amazonaws.fms#PolicyId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) that names no policy.
const UNKNOWN_POLICY_ID: &str = "11111111-2222-3333-4444-555555555555";

/// One row of the removal table: the tag set the resource is created with, the
/// `TagKeys` the removal names, and the set the resource must hold afterwards.
struct RemovalCase {
    name: String,
    held: Vec<(String, String)>,
    removed: Vec<String>,
    expected: Vec<(String, String)>,
}

/// Owned, sorted key/value pairs — the form every assertion compares.
fn pairs(of: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut owned: Vec<(String, String)> = of
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    owned.sort();
    owned
}

/// Owned tag KEYS, in the order the request lists them.
fn keys(of: &[&str]) -> Vec<String> {
    of.iter().map(|key| (*key).to_string()).collect()
}

/// An owned pair set in the sorted order every assertion compares — tags are a
/// key:value MAP, so the comparison is order-insensitive.
fn sorted(mut of: Vec<(String, String)>) -> Vec<(String, String)> {
    of.sort();
    of
}

/// A `TagList` wire value for an owned tag set, through the shared [`tag_list`]
/// builder so the wire shape is written in one place only.
fn as_tag_list(of: &[(String, String)]) -> Value {
    let borrowed: Vec<(&str, &str)> = of
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    tag_list(&borrowed)
}

/// An owned tag set as the stored `Tag` records a writer persists.
fn as_tags(of: &[(String, String)]) -> Vec<Tag> {
    of.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// Stored `Tag` records as the same sorted pair set the wire assertions use.
fn stored_pairs(tags: &[Tag]) -> Vec<(String, String)> {
    let mut owned: Vec<(String, String)> = tags
        .iter()
        .map(|tag| (tag.key.clone(), tag.value.clone()))
        .collect();
    owned.sort();
    owned
}

/// The removal class the rule is asserted over, on every taggable kind.
fn removal_cases() -> Vec<RemovalCase> {
    // The published maximum lengths of `TagKey` (128) and `TagValue` (256),
    // exactly, from `smithy_query(shape="TagKey")` / `shape="Tag"`.
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    vec![
        RemovalCase {
            name: "no keys named leaves every tag held".to_string(),
            held: pairs(&[("stage", "test"), ("team", "net")]),
            removed: keys(&[]),
            expected: pairs(&[("stage", "test"), ("team", "net")]),
        },
        RemovalCase {
            name: "one of several keys named".to_string(),
            held: pairs(&[("stage", "test"), ("team", "net"), ("owner", "ops")]),
            removed: keys(&["team"]),
            expected: pairs(&[("stage", "test"), ("owner", "ops")]),
        },
        RemovalCase {
            name: "every key held is named".to_string(),
            held: pairs(&[("stage", "test"), ("team", "net")]),
            removed: keys(&["team", "stage"]),
            expected: pairs(&[]),
        },
        RemovalCase {
            name: "a key the resource does not carry is ignored".to_string(),
            held: pairs(&[("stage", "test")]),
            removed: keys(&["never.written"]),
            expected: pairs(&[("stage", "test")]),
        },
        RemovalCase {
            name: "held and unheld keys named together".to_string(),
            held: pairs(&[("stage", "test"), ("team", "net")]),
            removed: keys(&["team", "never.written"]),
            expected: pairs(&[("stage", "test")]),
        },
        RemovalCase {
            name: "a key named twice is removed, not an error".to_string(),
            held: pairs(&[("stage", "test"), ("team", "net")]),
            removed: keys(&["team", "team"]),
            expected: pairs(&[("stage", "test")]),
        },
        RemovalCase {
            // "Tag keys are case-sensitive" (`com.amazonaws.fms#Tag$Key`), so
            // naming one of these removes exactly one of them.
            name: "keys differing only in case are distinct".to_string(),
            held: pairs(&[("Env", "a"), ("env", "b")]),
            removed: keys(&["env"]),
            expected: pairs(&[("Env", "a")]),
        },
        RemovalCase {
            // `TagValue`'s minimum length is 0, and a tag is identified by its
            // KEY, so an empty value is removed by naming its key like any other.
            name: "a tag whose value is empty is removed by its key".to_string(),
            held: pairs(&[("empty", ""), ("held", "x")]),
            removed: keys(&["empty"]),
            expected: pairs(&[("held", "x")]),
        },
        RemovalCase {
            name: "keys spanning the modelled TagKey pattern".to_string(),
            held: pairs(&[
                ("dot.key", "1"),
                ("colon:key", "2"),
                ("slash/key", "3"),
                ("plus+key", "4"),
                ("equals=key", "5"),
                ("at@key", "6"),
                ("dash-key", "7"),
                ("under_key", "8"),
                ("Ünïcødé", "9"),
                ("digits123", "10"),
            ]),
            removed: keys(&["colon:key", "slash/key", "plus+key", "Ünïcødé", "dash-key"]),
            expected: pairs(&[
                ("dot.key", "1"),
                ("equals=key", "5"),
                ("at@key", "6"),
                ("under_key", "8"),
                ("digits123", "10"),
            ]),
        },
        RemovalCase {
            name: "the longest modelled key and value".to_string(),
            held: sorted(vec![(longest_key.clone(), longest_value.clone())]),
            removed: vec![longest_key.clone()],
            expected: pairs(&[]),
        },
        RemovalCase {
            // The longest key is held and a one-character key is removed: the
            // long key survives, so the match is on the whole key, not a prefix.
            name: "a short key named beside the longest held key".to_string(),
            held: sorted(vec![
                (longest_key.clone(), longest_value.clone()),
                ("k".to_string(), "short".to_string()),
            ]),
            removed: keys(&["k"]),
            expected: sorted(vec![(longest_key, longest_value)]),
        },
    ]
}

/// `UntagResource` drops exactly the named keys from the resource's persisted
/// tag set, on every taggable kind: `ListTagsForResource` reports the remaining
/// set, a repeat of the same removal changes nothing, a removal against one
/// resource leaves the others alone, and the resource's own record and the
/// ARN-keyed index agree with what the wire reported.
#[tokio::test]
async fn untag_resource_removes_exactly_the_keys_the_request_names() {
    for case in removal_cases() {
        let (router, engine) = emulator_with_engine();
        associate_admin(&router).await;

        // One resource of EVERY taggable kind, each created holding the same tag
        // set, so the row is asserted across the kinds a generic `ResourceArn`
        // can name.
        let held_list = as_tag_list(&case.held);
        let mut resources: Vec<(Kind, String)> = Vec::new();
        for kind in KINDS {
            let label = format!("{} / {kind:?}", case.name);
            let arn = create_resource(
                &router,
                &engine,
                kind,
                "tagged.resource",
                Some(&held_list),
                &label,
            )
            .await;
            resources.push((kind, arn));
        }

        for (index, (kind, arn)) in resources.iter().enumerate() {
            let label = format!("{} / {kind:?}", case.name);

            // The precondition, read through an implemented operation rather
            // than assumed.
            let (status, code, before) = list_tags(&router, arn).await;
            assert_eq!(
                (status, code.as_str(), &before),
                (200, "", &case.held),
                "case {label:?}: expected the created resource to hold {:?}, got {status} / \
                 {code:?} / {before:?}",
                case.held
            );

            let request = json!({ "ResourceArn": arn, "TagKeys": &case.removed });
            let (status, body) = call(&router, "UntagResource", &request).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected UntagResource to answer 200, got {status} with {body:?}"
            );
            assert_eq!(
                body,
                json!({}),
                "case {label:?}: UntagResourceResponse models no members, so the body must be \
                 empty, got {body:?}"
            );

            // Idempotent: the same removal repeated is accepted, because a key
            // the resource no longer carries is not an error.
            let (status, body) = call(&router, "UntagResource", &request).await;
            assert_eq!(
                status, 200,
                "case {label:?}: a repeated UntagResource must answer 200, got {status} with \
                 {body:?}"
            );

            // Every resource: the ones already untagged hold the expected set,
            // the ones not yet untagged still hold everything they were created
            // with — so the removal reached the ARN it named and nothing else.
            for (position, (other_kind, other_arn)) in resources.iter().enumerate() {
                let expected = if position <= index {
                    &case.expected
                } else {
                    &case.held
                };
                let (status, code, got) = list_tags(&router, other_arn).await;
                assert_eq!(
                    (status, code.as_str()),
                    (200, ""),
                    "case {label:?}: expected ListTagsForResource({other_arn:?}) to answer 200, \
                     got {status} / {code:?}"
                );
                assert_eq!(
                    &got, expected,
                    "case {label:?}: after untagging {arn:?}, the {other_kind:?} resource must \
                     hold {expected:?}, got {got:?}"
                );
            }
        }

        // The authoritative copy (the tags on the resource's own record) and the
        // ARN-keyed index agree with what the wire reported.
        with_store(&engine, |store| {
            for (kind, arn) in &resources {
                let label = format!("{} / {kind:?}", case.name);
                let recorded = store.tags_for_arn(arn).map(|held| stored_pairs(&held));
                let indexed = store.tags_by_arn.get(arn).map(|held| stored_pairs(held));
                assert_eq!(
                    recorded.as_ref(),
                    Some(&case.expected),
                    "case {label:?}: the resource's own record must hold the remaining tags, got \
                     {recorded:?}"
                );
                assert_eq!(
                    indexed.as_ref(),
                    Some(&case.expected),
                    "case {label:?}: the ARN-keyed tag index must agree with the record, got \
                     {indexed:?}"
                );
            }
        });
    }
}

/// A `ResourceArn` that is not an ARN at all is refused with 400
/// `ValidationException`, and a well-formed ARN naming no stored resource of any
/// taggable kind is the modelled 400 `ResourceNotFoundException` — and either
/// refusal leaves the tag sets of the resources that DO exist untouched and
/// creates nothing for the ARN it refused.
#[tokio::test]
async fn untag_resource_refuses_an_arn_that_names_no_stored_resource() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    // One stored, tagged resource of every kind, so "not found" cannot be a
    // consequence of an empty store.
    let held = pairs(&[("stage", "test"), ("team", "net")]);
    let held_list = as_tag_list(&held);
    let mut resources: Vec<(Kind, String)> = Vec::new();
    for kind in KINDS {
        let label = format!("{kind:?}");
        let arn = create_resource(
            &router,
            &engine,
            kind,
            "stored.resource",
            Some(&held_list),
            &label,
        )
        .await;
        resources.push((kind, arn));
    }

    // Strings that are not ARNs at all: the form check refuses them before
    // anything is looked up.
    let not_arns = [
        ("no arn prefix", "this-is-not-an-arn-at-all".to_string()),
        ("too few fields", format!("arn:aws:fms:{REGION}")),
        (
            "an empty service field",
            format!("arn::{REGION}:{ACCOUNT}:policy/abc"),
        ),
        (
            "an empty resource field",
            format!("arn:aws:fms:{REGION}:{ACCOUNT}:"),
        ),
    ];
    for (case, arn) in &not_arns {
        let (status, body) = call(
            &router,
            "UntagResource",
            &json!({ "ResourceArn": arn, "TagKeys": keys(&["stage"]) }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected UntagResource({arn:?}) to answer 400 ValidationException, \
             got {status} with {body:?}"
        );
    }

    // Well-formed ARNs in each taggable kind's own namespace that name nothing:
    // a `PolicyId` / `ListId` (36 characters of `^[a-z0-9A-Z-]{36}$`) and a
    // `Base62Id` (22 characters of `^[a-z0-9A-Z]{22}$`).
    let unknown_ids = [UNKNOWN_POLICY_ID, "0123456789AbCdEfGhIjKl"];
    for kind in KINDS {
        for id in unknown_ids {
            let case = format!("{kind:?} / {id}");
            let unknown = kind.arn_of(id);
            let (status, body) = call(
                &router,
                "UntagResource",
                &json!({ "ResourceArn": &unknown, "TagKeys": keys(&["stage"]) }),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?}: expected UntagResource({unknown:?}) to answer 400 \
                 ResourceNotFoundException, got {status} with {body:?}"
            );

            // The refused removal created nothing for that ARN.
            let (status, code, _) = list_tags(&router, &unknown).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?}: a refused UntagResource must create no resource, but \
                 ListTagsForResource({unknown:?}) now answers {status} / {code:?}"
            );
        }
    }

    // An ARN of another service, another partition and another account names no
    // Firewall Manager resource either.
    let foreign = [
        ("another service", "arn:aws:s3:::some-bucket".to_string()),
        (
            "another partition",
            format!("arn:aws-cn:fms:cn-north-1:{ACCOUNT}:policy/abc"),
        ),
        (
            "another account",
            format!("arn:aws:fms:{REGION}:999988887777:resource-set/abc"),
        ),
    ];
    for (case, arn) in &foreign {
        let (status, body) = call(
            &router,
            "UntagResource",
            &json!({ "ResourceArn": arn, "TagKeys": keys(&["stage"]) }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected UntagResource({arn:?}) to answer 400 \
             ResourceNotFoundException, got {status} with {body:?}"
        );
    }

    // Every refusal above left the resources that DO exist holding exactly what
    // they were created with.
    for (kind, arn) in &resources {
        let (status, code, got) = list_tags(&router, arn).await;
        assert_eq!(
            (status, code.as_str(), &got),
            (200, "", &held),
            "case {kind:?}: a refused UntagResource must change no stored tag set, got {status} / \
             {code:?} / {got:?}"
        );
    }
}

/// `UntagResource` is organisation-scoped, so with no Firewall Manager
/// administrator association it answers the one 400 `AccessDeniedException`
/// whatever its payload carries and whichever Region it is signed for — the
/// decision taken before the ARN's form and before any lookup — and it removes
/// nothing. Associating an administrator is what lets the same call through.
#[tokio::test]
async fn untag_resource_is_refused_with_no_administrator_association() {
    // Regions every row is asserted in, so a message composed from the request
    // can be told from a constant.
    let regions = [REGION, "eu-central-1", "ap-southeast-4"];
    let held = pairs(&[("stage", "test"), ("team", "net")]);
    let arn = state::protocols_list_arn(REGION, PROTOCOLS_LIST_ID);

    for region in regions {
        let (router, engine) = emulator_with_engine();
        // The tagged resource is seeded the way its own writer writes it —
        // through the store — so the gate under test is never opened to create
        // it: `PutProtocolsList` is itself administrator-gated.
        with_store(&engine, |store| {
            seed_protocols_list(store, PROTOCOLS_LIST_ID, "gate.protocols", &["TCP"], false);
            let written = store.set_tags(
                &state::TaggedResource::ProtocolsList(PROTOCOLS_LIST_ID.to_string()),
                as_tags(&held),
            );
            assert!(
                written,
                "region {region}: the seeded protocols list must be there to tag"
            );
        });

        // The precondition, read through an implemented operation.
        let (status, body) = call_in(&router, "GetAdminAccount", &json!({}), region).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "region {region}: expected no administrator association, got {status} with {body}"
        );

        // The payload forms whose answers differ from each other once the gate
        // is open: a malformed ARN, an ARN naming nothing, a stored resource,
        // and an empty `TagKeys`.
        let rows = [
            (
                "a string that is not an ARN",
                json!({ "ResourceArn": "not-an-arn", "TagKeys": keys(&["stage"]) }),
            ),
            (
                "an ARN naming no policy",
                json!({
                    "ResourceArn": state::policy_arn(REGION, UNKNOWN_POLICY_ID),
                    "TagKeys": keys(&["stage"]),
                }),
            ),
            (
                "the ARN of a stored protocols list",
                json!({ "ResourceArn": &arn, "TagKeys": keys(&["stage"]) }),
            ),
            (
                "an empty TagKeys on a stored protocols list",
                json!({ "ResourceArn": &arn, "TagKeys": keys(&[]) }),
            ),
        ];
        for (case, body) in &rows {
            let (status, answered) = call_in(&router, "UntagResource", body, region).await;
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, "AccessDeniedException"),
                "case {case:?} in {region}: the authorization decision comes first, so every \
                 payload must get the administrator refusal, got {status} with {answered}"
            );
            let Some(message) = message_member(&answered) else {
                panic!(
                    "case {case:?} in {region}: the refusal must carry its message under the \
                     member name this service's error shapes declare (`Message`), got {answered}"
                );
            };
            assert!(
                message.contains(ACCOUNT) && message.contains(region),
                "case {case:?} in {region}: the refusal's message is composed from the request, \
                 so it must name the calling account and the Region the administrator was looked \
                 for in, got {message:?}"
            );
        }

        // Nothing was removed: the seeded list still carries every tag.
        with_store(&engine, |store| {
            let recorded = store.tags_for_arn(&arn).map(|tags| stored_pairs(&tags));
            assert_eq!(
                recorded.as_ref(),
                Some(&held),
                "region {region}: a refused UntagResource must remove no tag, got {recorded:?}"
            );
        });

        // The same call, with an administrator associated, is answered — and
        // removes the key it names.
        associate_admin(&router).await;
        let (status, body) = call_in(
            &router,
            "UntagResource",
            &json!({ "ResourceArn": &arn, "TagKeys": keys(&["stage"]) }),
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region}: an authorised caller must be answered, got {status} with {body}"
        );
        let (status, code, got) = list_tags(&router, &arn).await;
        assert_eq!(
            (status, code.as_str(), &got),
            (200, "", &pairs(&[("team", "net")])),
            "region {region}: expected the named key to be gone, got {status} / {code:?} / {got:?}"
        );
    }
}
