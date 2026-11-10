// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UntagResource`: the rule that it removes EXACTLY the
//! tag keys the request names from the tag set of the resource its `ResourceArn`
//! names, leaves every other tag of that resource — and every other resource —
//! untouched, and succeeds whether or not a named key was there to remove, so every
//! later read of the resource reports what the call left.
//!
//! The rule is asserted over a TABLE whose rows differ along what it generalises
//! over: the resource type the ARN names (every type this service mints an ARN for —
//! a registry, one of a registry's schemas, and a discoverer), the number of tags the
//! resource carries, the alphabet and form of the keys (ASCII, non-ASCII, a space, a
//! `/`, a `+`, keys differing only in case), and the number of keys the request names
//! (one, several, all of them, one that is not there, the same key twice).
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The
//! HTTP harness is the crate's one harness, imported from
//! `registry_schema_lifecycle`, and the tag-resource helpers (the ARN route, the
//! `@httpLabel` percent-encoding, the create-tagged table and the reported tag map)
//! are the ones `resource_tags.rs` already carries for the read side. Nothing here
//! touches the state struct directly, so a handler that answered without writing the
//! store would still have to make the following read agree.

use std::collections::BTreeMap;

use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_schemas, AWS_MANAGED_REGISTRIES, CODE_FORBIDDEN, CODE_MALFORMED_REQUEST,
    CODE_RESOURCE_NOT_FOUND,
};

use crate::registry_schema_lifecycle::{registry_uri, Emulator, ACCOUNT, REGION};
use crate::resource_tags::{
    create_tagged, percent_encode, reported_tags, resource_name, tags, tags_now, tags_uri, Created,
    Kind,
};

/// `UntagResource`'s route (`DELETE /tags/{ResourceArn}?tagKeys=…`): the ARN in its
/// one path segment, and one `tagKeys` value per key the request names.
fn untag_uri(resource_arn: &str, keys: &[&str]) -> String {
    let mut uri = tags_uri(resource_arn);
    for (index, key) in keys.iter().enumerate() {
        uri.push(if index == 0 { '?' } else { '&' });
        uri.push_str("tagKeys=");
        uri.push_str(&percent_encode(key));
    }
    uri
}

/// One row: a resource carrying `tags`, and the keys one `UntagResource` names.
#[derive(Debug)]
struct Case {
    case: &'static str,
    tags: &'static [(&'static str, &'static str)],
    remove: &'static [&'static str],
}

/// The table. Rows differ along the number of tags the resource carries, the
/// alphabet and form of its keys, and how many of them — if any — the request names.
/// `Tags` is a map of plain `__string` to plain `__string` with no length, pattern or
/// count constraint on either (`smithy_query(query="shape",shape="Tags")`), so no row
/// sits outside a modelled bound.
const CASES: &[Case] = &[
    Case {
        case: "one-key-of-three",
        tags: &[("team", "payments"), ("env", "test"), ("owner", "caf\u{e9}")],
        remove: &["env"],
    },
    Case {
        case: "two-keys-of-three",
        tags: &[("team", "payments"), ("env", "test"), ("owner", "caf\u{e9}")],
        remove: &["env", "team"],
    },
    Case {
        case: "every-key-the-resource-carries",
        tags: &[("team", "payments"), ("env", "test"), ("owner", "caf\u{e9}")],
        remove: &["team", "env", "owner"],
    },
    Case {
        case: "the-only-key",
        tags: &[("only", "one")],
        remove: &["only"],
    },
    Case {
        case: "a-key-the-resource-does-not-carry",
        tags: &[("team", "payments"), ("env", "test")],
        remove: &["absent"],
    },
    Case {
        case: "a-resource-that-carries-no-tags-at-all",
        tags: &[],
        remove: &["absent"],
    },
    Case {
        case: "keys-differing-only-in-case",
        tags: &[("team", "lower"), ("Team", "title"), ("TEAM", "upper")],
        remove: &["team", "TEAM"],
    },
    Case {
        case: "the-same-key-named-twice",
        tags: &[("team", "payments"), ("env", "test")],
        remove: &["team", "team"],
    },
    Case {
        case: "keys-that-have-to-be-percent-encoded",
        tags: &[
            ("path/like", "v1"),
            ("with space", "v2"),
            ("plus+key", "v3"),
            ("dot.ted", "v4"),
        ],
        remove: &["path/like", "with space", "plus+key"],
    },
    Case {
        case: "a-non-ascii-key-and-an-empty-value",
        tags: &[("\u{e9}quipe", "paiements"), ("blank", ""), ("keep", "v")],
        remove: &["\u{e9}quipe", "blank"],
    },
    Case {
        case: "three-keys-of-eight",
        tags: &[
            ("k1", "v1"),
            ("k2", "v2"),
            ("k3", "v3"),
            ("k4", "v4"),
            ("k5", "v5"),
            ("k6", "v6"),
            ("k7", "v7"),
            ("k8", "v8"),
        ],
        remove: &["k2", "k5", "k8"],
    },
];

/// The tag set a row's resource is left with, computed from the row itself rather
/// than from anything the handler did.
fn expected_tags(case: &Case) -> BTreeMap<String, String> {
    case.tags
        .iter()
        .filter(|(key, _)| !case.remove.contains(key))
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// RULE: `UntagResource` removes exactly the keys the request names from the resource
/// its ARN names, keeps every other tag, and answers 204 with no body — for every
/// resource type this service mints an ARN for, and whether or not a named key was
/// there to remove. Sent TWICE: removing a tag is not a toggle, so the repeat answers
/// the same way and leaves the same tag set.
#[tokio::test]
async fn untag_resource_removes_exactly_the_named_keys_from_every_taggable_resource() {
    let emulator = Emulator::new();

    for kind in Kind::ALL {
        for (row, case) in CASES.iter().enumerate() {
            let label = format!("{kind:?}/{}", case.case);
            let created = create_tagged(
                &emulator,
                &label,
                kind,
                &resource_name(kind, row),
                &tags(case.tags),
            )
            .await;
            assert_eq!(
                tags_now(&emulator, &label, &created).await,
                tags(case.tags),
                "case {label:?}: the create must store the row's tags"
            );

            for attempt in 1..=2 {
                let removed = emulator
                    .call("DELETE", &untag_uri(&created.arn, case.remove), None)
                    .await;
                assert_eq!(
                    removed.status, 204,
                    "case {label:?} attempt {attempt}: expected 204 from UntagResource, \
                     got {} {:?}",
                    removed.status, removed.body
                );
                assert!(
                    removed.raw.is_empty(),
                    "case {label:?} attempt {attempt}: a 204 carries no body, got {:?}",
                    removed.raw
                );
                assert_eq!(
                    removed.header("content-length"),
                    Some("0"),
                    "case {label:?} attempt {attempt}: an answer with no body must state \
                     its length, got {:?}",
                    removed.header("content-length")
                );

                // The read-after-write: this is what the stub answered nothing about.
                assert_eq!(
                    tags_now(&emulator, &label, &created).await,
                    expected_tags(case),
                    "case {label:?} attempt {attempt}: the resource must report exactly \
                     the tags the removal left"
                );
            }
        }
    }
}

/// RULE: the tags removed are those of the ONE resource the ARN names; every other
/// resource keeps the keys it carries, even where it carries the same keys.
#[tokio::test]
async fn untagging_one_resource_leaves_every_other_resources_tags_alone() {
    let shared = tags(&[("team", "payments"), ("env", "test"), ("keep", "v")]);
    // Rows: each resource type in turn is the one untagged, while all three are read
    // back — one of them the resource written to, the other two not.
    for (row, untagged) in Kind::ALL.into_iter().enumerate() {
        let emulator = Emulator::new();
        let label = format!("untagging-the-{untagged:?}");
        let mut created: Vec<(Kind, Created)> = Vec::new();
        for kind in Kind::ALL {
            created.push((
                kind,
                create_tagged(
                    &emulator,
                    &label,
                    kind,
                    &resource_name(kind, 200 + row),
                    &shared,
                )
                .await,
            ));
        }

        let arn = &created
            .iter()
            .find(|(kind, _)| *kind == untagged)
            .expect("every resource type is in the table")
            .1
            .arn;
        let removed = emulator
            .call("DELETE", &untag_uri(arn, &["team"]), None)
            .await;
        assert_eq!(
            removed.status, 204,
            "case {label:?}: expected 204 from UntagResource, got {} {:?}",
            removed.status, removed.body
        );

        for (kind, resource) in &created {
            let expected: BTreeMap<String, String> = shared
                .iter()
                .filter(|(key, _)| !(*kind == untagged && key.as_str() == "team"))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            let read_label = format!("{label}/reading-the-{kind:?}");
            assert_eq!(
                tags_now(&emulator, &read_label, resource).await,
                expected,
                "case {read_label:?}: only the untagged resource loses the key"
            );
        }
    }
}

/// A `ResourceArn` a refusal row sends, and the answer it owes.
#[derive(Debug)]
struct RefusalCase {
    case: &'static str,
    arn: String,
    status: u16,
    error_type: &'static str,
    code: &'static str,
}

/// The refusal table: a `ResourceArn` that is no resource ARN of this service is a
/// 400 on its form, and one naming a resource this store does not hold — including
/// the stored resource's ARN with another region, account or partition, and a schema
/// of the service-owned catalogue, which carries no ARN at all — is a 404.
fn refusal_cases(stored_registry_arn: &str, stored_schema_arn: &str) -> Vec<RefusalCase> {
    let bad_request = |case: &'static str, arn: String| RefusalCase {
        case,
        arn,
        status: 400,
        error_type: "BadRequestException",
        code: CODE_MALFORMED_REQUEST,
    };
    let not_found = |case: &'static str, arn: String| RefusalCase {
        case,
        arn,
        status: 404,
        error_type: "NotFoundException",
        code: CODE_RESOURCE_NOT_FOUND,
    };
    let managed_schema = aws_managed_schemas()
        .first()
        .expect("the service-owned catalogue holds schemas")
        .name
        .clone();
    let managed_registry = AWS_MANAGED_REGISTRIES
        .first()
        .expect("the store seeds a service-owned registry");
    vec![
        bad_request("not-an-arn-at-all", "untagged-registry".to_string()),
        bad_request(
            "an-arn-with-an-empty-resource-field",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:"),
        ),
        bad_request(
            "another-services-resource-arn",
            format!("arn:aws:events:{REGION}:{ACCOUNT}:event-bus/default"),
        ),
        bad_request(
            "a-resource-type-this-service-does-not-have",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:codebinding/untagged-registry"),
        ),
        bad_request(
            "a-registry-arn-naming-no-registry",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:registry/"),
        ),
        bad_request(
            "a-schema-arn-naming-no-schema",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:schema/untagged-registry"),
        ),
        not_found(
            "a-registry-the-store-does-not-hold",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:registry/absent-registry"),
        ),
        not_found(
            "the-stored-registry-in-another-region",
            stored_registry_arn.replace(REGION, "eu-west-1"),
        ),
        not_found(
            "the-stored-registry-in-another-account",
            stored_registry_arn.replace(ACCOUNT, "999999999999"),
        ),
        not_found(
            "the-stored-registry-in-another-partition",
            stored_registry_arn.replacen("arn:aws:", "arn:aws-cn:", 1),
        ),
        not_found(
            "a-schema-the-registry-does-not-hold",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:schema/untagged/absent.schema"),
        ),
        not_found(
            "the-stored-schema-in-another-region",
            stored_schema_arn.replace(REGION, "eu-west-1"),
        ),
        not_found(
            "a-schema-of-the-service-owned-catalogue-which-has-no-arn",
            format!(
                "arn:aws:schemas:{REGION}:{ACCOUNT}:schema/{managed_registry}/{managed_schema}"
            ),
        ),
        not_found(
            "a-discoverer-id-the-service-never-minted",
            format!(
                "arn:aws:schemas:{REGION}:{ACCOUNT}:discoverer/00000000000000000000000000000000"
            ),
        ),
    ]
}

/// RULE: a `ResourceArn` that names no tag set this caller may write is refused — 400
/// for a value that is no resource ARN of this service, 404 for one naming a resource
/// this store does not hold — and a refused call removes nothing.
#[tokio::test]
async fn untagging_an_arn_that_names_no_writable_resource_is_refused_and_removes_nothing() {
    let emulator = Emulator::new();
    let stored = tags(&[("team", "payments"), ("env", "test")]);
    let registry = create_tagged(&emulator, "refusals", Kind::Registry, "untagged", &stored).await;
    let schema = create_tagged(
        &emulator,
        "refusals",
        Kind::Schema,
        "untagged.schema",
        &stored,
    )
    .await;

    for case in refusal_cases(&registry.arn, &schema.arn) {
        let refused = emulator
            .call("DELETE", &untag_uri(&case.arn, &["team", "env"]), None)
            .await;
        assert_eq!(
            refused.status, case.status,
            "case {:?}: expected {} from UntagResource, got {} {:?}",
            case.case, case.status, refused.status, refused.body
        );
        assert_eq!(
            refused.error_type(),
            case.error_type,
            "case {:?}: expected the {} shape, got {:?}",
            case.case,
            case.error_type,
            refused.header("x-amzn-errortype")
        );
        assert_eq!(
            refused.str_at("Code"),
            case.code,
            "case {:?}: expected the {} condition code, got {:?}",
            case.case,
            case.code,
            refused.body
        );

        // Nothing was removed: both resources still carry every tag they were
        // created with.
        for (label, resource) in [("registry", &registry), ("schema", &schema)] {
            let read_label = format!("{}/{label}", case.case);
            assert_eq!(
                tags_now(&emulator, &read_label, resource).await,
                stored,
                "case {read_label:?}: a refused removal must leave the stored tags alone"
            );
        }
    }
}

/// RULE: what is in a service-owned registry is AWS's, so a caller may read it but
/// not write to it: removing a tag from one is refused 403 and the registry is still
/// there to read.
#[tokio::test]
async fn untagging_a_service_owned_registry_is_refused() {
    let emulator = Emulator::new();
    // Source: `crate::state::AWS_MANAGED_REGISTRIES` — the service-owned registries
    // every account sees.
    for registry in AWS_MANAGED_REGISTRIES {
        let described = emulator.call("GET", &registry_uri(registry), None).await;
        assert_eq!(
            described.status, 200,
            "case {registry:?}: expected 200 from DescribeRegistry, got {} {:?}",
            described.status, described.body
        );
        let arn = described.str_at("RegistryArn");

        // Rows: a key the registry does not carry and one it would carry — a write to
        // AWS-owned content is refused on who owns it, not on what it would change.
        for keys in [&["team"][..], &["team", "env"][..]] {
            let refused = emulator.call("DELETE", &untag_uri(&arn, keys), None).await;
            assert_eq!(
                refused.status, 403,
                "case {registry:?}/{keys:?}: expected 403 from UntagResource, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type(),
                "ForbiddenException",
                "case {registry:?}/{keys:?}: expected the ForbiddenException shape, got {:?}",
                refused.header("x-amzn-errortype")
            );
            assert_eq!(
                refused.str_at("Code"),
                CODE_FORBIDDEN,
                "case {registry:?}/{keys:?}: expected the {CODE_FORBIDDEN} condition code, \
                 got {:?}",
                refused.body
            );
        }

        let listed = emulator.call("GET", &tags_uri(&arn), None).await;
        assert_eq!(
            listed.status, 200,
            "case {registry:?}: the service-owned registry is still readable, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            reported_tags(&listed, registry),
            BTreeMap::new(),
            "case {registry:?}: a refused removal leaves the seeded registry as it was, \
             got {:?}",
            listed.body.get("tags")
        );
    }
}

/// RULE: a removal is a write to the resource's tag set and to nothing else — the
/// resource's other members read back exactly as its create stored them.
#[tokio::test]
async fn a_removal_changes_no_member_but_the_tags() {
    let emulator = Emulator::new();
    let created = create_tagged(
        &emulator,
        "other-members",
        Kind::Schema,
        "untouched.schema",
        &tags(&[("team", "payments"), ("env", "test")]),
    )
    .await;
    let before = emulator.call("GET", &created.describe_uri, None).await;

    let removed = emulator
        .call("DELETE", &untag_uri(&created.arn, &["team"]), None)
        .await;
    assert_eq!(
        removed.status, 204,
        "expected 204 from UntagResource, got {} {:?}",
        removed.status, removed.body
    );

    let after = emulator.call("GET", &created.describe_uri, None).await;
    for member in [
        "SchemaArn",
        "SchemaName",
        "SchemaVersion",
        "Type",
        "Content",
        "LastModified",
        "VersionCreatedDate",
    ] {
        assert_eq!(
            after.body.get(member),
            before.body.get(member),
            "member {member:?}: a tag removal must not touch it, got {:?} where it was \
             {:?}",
            after.body.get(member),
            before.body.get(member)
        );
    }
    assert_eq!(
        reported_tags(&after, "other-members"),
        tags(&[("env", "test")]),
        "the tag set is the one member the removal changes, got {:?}",
        after.body.get("tags")
    );
}
