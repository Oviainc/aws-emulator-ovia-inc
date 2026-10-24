// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Firewall Manager's tagging operations across EVERY KIND
//! of taggable resource this service stores.
//!
//! The rule asserted here, in one sentence: a Firewall Manager tagging operation
//! names its resource by a generic `ResourceArn`, so it resolves that ARN
//! against every kind of taggable resource the service holds and reports (or
//! writes) the tags that resource carries — an empty `TagList` for a resource
//! that carries none, and the modelled `ResourceNotFoundException` only for an
//! ARN that names no stored resource at all.
//!
//! `ListTagsForResourceRequest$ResourceArn` is the model's generic `ResourceArn`
//! and `PutPolicy`, `PutResourceSet`, `PutAppsList` and `PutProtocolsList` each
//! model a `TagList` input, so each of those resources carries tags a reader must
//! report.
//!
//! The table below is the class the rule ranges over: the rows are the CARTESIAN
//! product of the resource KIND (policy, resource set, protocols list,
//! applications list) and the tag set (absent, empty, one tag, several tags with
//! a key overwritten, keys and values spanning the modelled `TagKey` / `TagValue`
//! pattern and length bounds). Every assertion is made on every row.
//!
//! Applications lists are seeded through the store because `PutAppsList` is
//! still a 501 stub tracked by its own issue; every other kind is created
//! through its own wire API.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::{AppsListData, Tag};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::policy_lifecycle::{call, emulator_with_engine, error_code, ACCOUNT, REGION};
// The tagging harness — how a `TagList` is built, read back off the wire and
// compared as a key:value set, and how `ListTagsForResource` is called — lives
// in the sibling `tagging_and_managing_admins` suite, so the two cannot drift in
// what they consider the same tag set.
use crate::tagging_and_managing_admins::{list_tags, minimal_policy, owned, tag_list, tag_pairs};

/// The kinds of Firewall Manager resource that carry tags and that this service
/// persists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Policy,
    ResourceSet,
    ProtocolsList,
    AppsList,
}

/// Every taggable kind, so a row of the table is a (kind, tag set) pair.
const KINDS: [Kind; 4] = [
    Kind::Policy,
    Kind::ResourceSet,
    Kind::ProtocolsList,
    Kind::AppsList,
];

impl Kind {
    /// A well-formed ARN of this kind naming the resource id `id`, built by the
    /// service's own ARN helpers so the test writes no ARN literal.
    fn arn_of(self, id: &str) -> String {
        match self {
            Kind::Policy => state::policy_arn(REGION, id),
            Kind::ResourceSet => state::resource_set_arn(REGION, id),
            Kind::ProtocolsList => state::protocols_list_arn(REGION, id),
            Kind::AppsList => state::apps_list_arn(REGION, id),
        }
    }
}

/// A wire `TagList` as the stored `Tag` structs, for the kinds this test seeds
/// through the store.
fn tag_structs(value: Option<&Value>) -> Vec<Tag> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|tag| Tag {
                    key: tag["Key"].as_str().unwrap_or_default().to_string(),
                    value: tag["Value"].as_str().unwrap_or_default().to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Call `operation` with `body`, assert it answered 200 and return the ARN it
/// reported under `arn_member`.
async fn create_through_the_api(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    arn_member: &str,
    case: &str,
) -> String {
    let (status, answered) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {answered:?}"
    );
    answered[arn_member]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: {operation} reported no {arn_member}: {answered:?}")
        })
        .to_string()
}

/// Create one resource of `kind` carrying `tags` (or no `TagList` at all when
/// `tags` is `None`) and return its ARN.
async fn create_resource(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    kind: Kind,
    name: &str,
    tags: Option<&Value>,
    case: &str,
) -> String {
    // An absent `TagList` is a resource created with no tags at all, which is a
    // different request from one carrying an empty list.
    let with_tags = |mut body: Value| {
        if let Some(tags) = tags {
            body["TagList"] = tags.clone();
        }
        body
    };
    match kind {
        Kind::Policy => {
            let body = with_tags(json!({ "Policy": minimal_policy(name) }));
            create_through_the_api(router, "PutPolicy", body, "PolicyArn", case).await
        }
        Kind::ResourceSet => {
            let body = with_tags(json!({
                "ResourceSet": { "Name": name, "ResourceTypeList": ["AWS::EC2::VPC"] },
            }));
            create_through_the_api(router, "PutResourceSet", body, "ResourceSetArn", case).await
        }
        Kind::ProtocolsList => {
            let body = with_tags(json!({
                "ProtocolsList": { "ListName": name, "ProtocolsList": ["TCP", "UDP"] },
            }));
            create_through_the_api(router, "PutProtocolsList", body, "ProtocolsListArn", case).await
        }
        // `PutAppsList` is still a 501 stub tracked by its own issue, so the
        // applications list is seeded the way that writer will write it: the tags
        // on the record, and the ARN-keyed index refreshed from the same value.
        Kind::AppsList => {
            let list_id = state::mint_identifier();
            let arn = state::apps_list_arn(REGION, &list_id);
            let held = tag_structs(tags);
            let now = SystemTime::now();
            with_store(engine, |store| {
                store.apps_lists.insert(
                    list_id.clone(),
                    state::AppsListRecord {
                        list_id: list_id.clone(),
                        list_arn: arn.clone(),
                        update_token: state::mint_identifier(),
                        default_list: false,
                        list: AppsListData {
                            list_name: name.to_string(),
                            ..AppsListData::default()
                        },
                        tags: held.clone(),
                        created_at: now,
                        updated_at: now,
                    },
                );
                store.tags_by_arn.insert(arn.clone(), held.clone());
            });
            arn
        }
    }
}

/// One row's tag set: what the create sends, what a later `TagResource` adds,
/// and the set the resource must hold afterwards.
struct TagCase {
    name: &'static str,
    /// The `TagList` the create sends, or `None` to send no `TagList` at all.
    created: Option<Value>,
    added: Value,
    expected: Vec<(String, String)>,
}

/// The tag-set class the rule is asserted over, on EVERY resource kind.
///
/// The rows differ in tag count (absent, empty, one, several), in whether a
/// written key is already held, and in key and value FORM across the modelled
/// `TagKey` / `TagValue` pattern (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, from
/// `smithy_query(shape="Tag")`) and at those members' published length bounds
/// (`TagKey` 1..128, `TagValue` 0..256).
fn tag_cases() -> Vec<TagCase> {
    // The published maxima of TagKey and TagValue, exactly.
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    vec![
        TagCase {
            name: "no TagList at all, then one tag written",
            created: None,
            added: tag_list(&[("owner", "sec")]),
            expected: owned(&[("owner", "sec")]),
        },
        TagCase {
            name: "an empty TagList, then one tag written",
            created: Some(tag_list(&[])),
            added: tag_list(&[("owner", "sec")]),
            expected: owned(&[("owner", "sec")]),
        },
        TagCase {
            name: "one tag at create, nothing written",
            created: Some(tag_list(&[("owner", "sec")])),
            added: tag_list(&[]),
            expected: owned(&[("owner", "sec")]),
        },
        TagCase {
            name: "several tags at create, one key overwritten and one added",
            created: Some(tag_list(&[("stage", "test"), ("team", "net")])),
            added: tag_list(&[("stage", "prod"), ("owner", "ops")]),
            expected: owned(&[("stage", "prod"), ("team", "net"), ("owner", "ops")]),
        },
        TagCase {
            name: "keys and values spanning the modelled pattern",
            created: Some(tag_list(&[("dot.key", "1"), ("empty.value", "")])),
            added: tag_list(&[
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
            expected: owned(&[
                ("dot.key", "1"),
                ("empty.value", ""),
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
        },
        TagCase {
            name: "the longest modelled key and value",
            created: Some(tag_list(&[])),
            added: Value::Array(vec![json!({ "Key": longest_key, "Value": longest_value })]),
            expected: owned(&[(longest_key.as_str(), longest_value.as_str())]),
        },
    ]
}

/// The tags a resource of ANY taggable kind carries are reported by
/// `ListTagsForResource` — the ones its create sent, and the ones a later
/// `TagResource` merged in — and the read changes nothing a later caller sees.
#[tokio::test]
async fn every_taggable_resource_kind_reports_the_tags_it_carries() {
    for kind in KINDS {
        for tags in tag_cases() {
            let case = format!("{kind:?} / {}", tags.name);
            let (router, engine) = emulator_with_engine();
            associate_admin(&router).await;

            let arn = create_resource(
                &router,
                &engine,
                kind,
                "tagged.resource",
                tags.created.as_ref(),
                &case,
            )
            .await;

            // The create's own TagList is what the resource starts from; a
            // resource created with no tags reports an EMPTY list, not an error
            // and not an absent member.
            let (status, code, before) = list_tags(&router, &arn).await;
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case {case:?}: expected ListTagsForResource({arn:?}) to answer 200, got \
                 {status} / {code:?}"
            );
            let created = tags.created.clone().unwrap_or_else(|| tag_list(&[]));
            assert_eq!(
                before,
                tag_pairs(&created),
                "case {case:?}: expected the created resource to hold the TagList it was \
                 created with, got {before:?}"
            );

            let (status, body) = call(
                &router,
                "TagResource",
                &json!({ "ResourceArn": arn, "TagList": tags.added }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected TagResource({arn:?}) to answer 200, got {status} with \
                 {body:?}"
            );

            let (status, code, after) = list_tags(&router, &arn).await;
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case {case:?}: expected ListTagsForResource to answer 200 after the write, got \
                 {status} / {code:?}"
            );
            assert_eq!(
                after, tags.expected,
                "case {case:?}: expected the merged tag set {:?}, got {after:?}",
                tags.expected
            );

            // A read changes nothing a later caller sees.
            let (_, _, again) = list_tags(&router, &arn).await;
            assert_eq!(
                again, after,
                "case {case:?}: a repeated ListTagsForResource must answer identically, got \
                 {again:?} after {after:?}"
            );

            // The authoritative copy (the tags on the resource's own record) and
            // the ARN-keyed index agree with what the wire reported.
            let (recorded, indexed) = with_store(&engine, |store| {
                (
                    store.tags_for_arn(&arn).map(|held| {
                        tag_pairs(&Value::Array(
                            held.iter()
                                .map(|tag| json!({ "Key": tag.key, "Value": tag.value }))
                                .collect(),
                        ))
                    }),
                    store.tags_by_arn.get(&arn).map(|held| {
                        tag_pairs(&Value::Array(
                            held.iter()
                                .map(|tag| json!({ "Key": tag.key, "Value": tag.value }))
                                .collect(),
                        ))
                    }),
                )
            });
            assert_eq!(
                recorded.as_ref(),
                Some(&after),
                "case {case:?}: the resource's own record must hold what the wire reported, got \
                 {recorded:?}"
            );
            assert_eq!(
                indexed.as_ref(),
                Some(&after),
                "case {case:?}: the ARN-keyed tag index must agree with the record, got \
                 {indexed:?}"
            );
        }
    }
}

/// One created resource: the kind it is, its ARN and the tag set it holds.
#[derive(Debug, Clone)]
struct Created {
    kind: Kind,
    arn: String,
    tags: Vec<(String, String)>,
}

/// Each kind's resource carries its OWN tags: one emulator holding a resource of
/// every kind reports each one's tag set, and a write against one leaves the
/// others untouched.
#[tokio::test]
async fn tags_belong_to_the_resource_the_arn_names_not_to_its_kind() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    // One resource per kind, each with a tag set no other kind holds.
    let mut resources: Vec<Created> = Vec::new();
    for kind in KINDS {
        let case = format!("{kind:?}");
        let held = [("kind", case.as_str()), ("shared", "same-for-every-kind")];
        let arn = create_resource(
            &router,
            &engine,
            kind,
            "one.per.kind",
            Some(&tag_list(&held)),
            &case,
        )
        .await;
        resources.push(Created {
            kind,
            arn,
            tags: owned(&held),
        });
    }

    for Created { kind, arn, tags } in &resources {
        let expected = tags;
        let (status, code, got) = list_tags(&router, arn).await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {kind:?}: expected ListTagsForResource({arn:?}) to answer 200, got {status} / \
             {code:?}"
        );
        assert_eq!(
            &got, expected,
            "case {kind:?}: expected the resource's own tag set {expected:?}, got {got:?}"
        );
    }

    // Writing to ONE resource changes that resource only.
    let target = resources[1].clone();
    let target_arn = target.arn.clone();
    let target_kind = target.kind;
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "ResourceArn": target_arn, "TagList": tag_list(&[("shared", "rewritten")]) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {target_kind:?}: expected TagResource to answer 200, got {status} with {body:?}"
    );
    for Created { kind, arn, tags } in &resources {
        let (_, _, got) = list_tags(&router, arn).await;
        let expected = if arn == &target_arn {
            let mut rewritten: Vec<(String, String)> = tags
                .iter()
                .filter(|(key, _)| key != "shared")
                .cloned()
                .collect();
            rewritten.push(("shared".to_string(), "rewritten".to_string()));
            rewritten.sort();
            rewritten
        } else {
            tags.clone()
        };
        assert_eq!(
            got, expected,
            "case {kind:?}: a write against {target_arn:?} must leave this resource holding \
             {expected:?}, got {got:?}"
        );
    }
}

/// A well-formed ARN in a taggable kind's own namespace that names no stored
/// resource is the modelled `ResourceNotFoundException` — including while other
/// resources of that same kind exist — and a tag write against it stores
/// nothing.
#[tokio::test]
async fn an_arn_naming_no_stored_resource_of_any_kind_is_not_found() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    // Ids that are well formed for their kind but name nothing: a `PolicyId` /
    // `ListId` (36 characters of `^[a-z0-9A-Z-]{36}$`) and a `Base62Id` (22
    // characters of `^[a-z0-9A-Z]{22}$`).
    let unknown_ids = [
        "99999999-8888-7777-6666-555555555555",
        "0123456789AbCdEfGhIjKl",
    ];

    for with_stored_resources in [false, true] {
        if with_stored_resources {
            for kind in KINDS {
                let case = format!("{kind:?}");
                create_resource(
                    &router,
                    &engine,
                    kind,
                    "stored.resource",
                    Some(&tag_list(&[("stage", "test")])),
                    &case,
                )
                .await;
            }
        }

        for kind in KINDS {
            for id in unknown_ids {
                let case = format!("{kind:?} / {id} / stored resources: {with_stored_resources}");
                let unknown = kind.arn_of(id);
                let (status, code, _) = list_tags(&router, &unknown).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, "ResourceNotFoundException"),
                    "case {case:?}: expected ListTagsForResource({unknown:?}) to answer 400 \
                     ResourceNotFoundException, got {status} / {code:?}"
                );

                let (status, body) = call(
                    &router,
                    "TagResource",
                    &json!({ "ResourceArn": unknown, "TagList": tag_list(&[("k", "v")]) }),
                )
                .await;
                let code = error_code(&body);
                assert_eq!(
                    (status, code.as_str()),
                    (400, "ResourceNotFoundException"),
                    "case {case:?}: expected TagResource({unknown:?}) to answer 400 \
                     ResourceNotFoundException, got {status} / {code:?} with {body:?}"
                );

                // The refused write created nothing for the ARN.
                let (status, code, _) = list_tags(&router, &unknown).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, "ResourceNotFoundException"),
                    "case {case:?}: a refused TagResource must create no resource, but \
                     ListTagsForResource({unknown:?}) now answers {status} / {code:?}"
                );
            }
        }

        // An ARN of another service, another partition and another account names
        // no Firewall Manager resource either.
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
        for (case, arn) in foreign {
            let (status, code, _) = list_tags(&router, &arn).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?} (stored resources: {with_stored_resources}): expected \
                 ListTagsForResource({arn:?}) to answer 400 ResourceNotFoundException, got \
                 {status} / {code:?}"
            );
        }
    }
}
