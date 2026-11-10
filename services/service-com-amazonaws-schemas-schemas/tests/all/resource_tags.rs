// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListTagsForResource`.
//!
//! The rule under test: the operation reports the tag set this store holds for the
//! resource its `ResourceArn` NAMES — a registry, a schema or a discoverer — where a
//! resource is named by the ARN this service itself reports for it; an ARN of one of
//! those resource types that names nothing this store holds is `NotFoundException`
//! (404), and a value that is no resource ARN of this service is `BadRequestException`
//! (400).
//!
//! The rule is asserted over a TABLE of tag sets crossed with every resource type that
//! carries one, so the rows differ along what the rule generalises over — which
//! resource type is named, how many tags it carries, and the alphabet, length and
//! emptiness of the keys and values. Each case reads the tags back through
//! `ListTagsForResource` AND through that resource's own Describe, so a handler that
//! answered from somewhere other than the store would have to make both agree.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The HTTP
//! harness is the crate's one harness, imported from `registry_schema_lifecycle`.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_schemas, AWS_MANAGED_REGISTRY, CODE_MALFORMED_REQUEST, CODE_RESOURCE_NOT_FOUND,
};

use crate::registry_schema_lifecycle::{
    create_registry, registry_uri, schema_uri, Answer, Emulator, ACCOUNT, REGION,
};

/// An OpenAPI 3 document, which is what a schema's `Content` must be.
pub(crate) const CONTENT: &str = r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"t"},"paths":{}}"#;

/// `CreateDiscoverer`'s route.
pub(crate) const DISCOVERERS_URI: &str = "/v1/discoverers";

/// Percent-encode one value into a single path segment, the way an SDK encodes a
/// non-greedy `@httpLabel`: everything outside RFC 3986's unreserved set
/// (`ALPHA / DIGIT / "-" / "." / "_" / "~"`) is escaped, so an ARN's `:` and `/` travel
/// as `%3A` and `%2F` — a raw `/` would address a different route — and a value carrying
/// a space or a `%` still makes a well-formed request line.
pub(crate) fn percent_encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => {
                encoded.push('%');
                encoded.push(char::from(HEX[usize::from(byte >> 4)]));
                encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
            }
        }
    }
    encoded
}

/// `ListTagsForResource`'s route for one resource ARN (`GET /tags/{ResourceArn}`).
pub(crate) fn tags_uri(resource_arn: &str) -> String {
    format!("/tags/{}", percent_encode(resource_arn))
}

/// `DescribeDiscoverer`'s route.
pub(crate) fn discoverer_uri(discoverer_id: &str) -> String {
    format!("/v1/discoverers/id/{discoverer_id}")
}

/// An event-bus ARN to discover from. One per case, because a source may be discovered
/// by at most one discoverer.
pub(crate) fn source_arn(bus: &str) -> String {
    format!("arn:aws:events:{REGION}:{ACCOUNT}:event-bus/{bus}")
}

/// The tag map a response reports, from the `tags` member (`Tags` carries
/// `jsonName: "tags"`). An answer that reports no member at all reads as no tags,
/// which is what an untagged resource holds.
pub(crate) fn reported_tags(answer: &Answer, case: &str) -> BTreeMap<String, String> {
    match answer.body.get("tags") {
        None | Some(Value::Null) => BTreeMap::new(),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: tag {key:?} must carry a string, got {value:?}")
                        })
                        .to_string(),
                )
            })
            .collect(),
        Some(other) => panic!("case {case:?}: `tags` must be a JSON object, got {other:?}"),
    }
}

/// Add a `Tags` member to a create's body, under the `tags` wire name the model binds
/// it to. An empty set is left OFF the request, which is how a caller creates an
/// untagged resource.
pub(crate) fn with_tags(mut body: Value, tags: &BTreeMap<String, String>) -> Value {
    if !tags.is_empty() {
        body["tags"] = json!(tags);
    }
    body
}

/// A tag set a create stores and `ListTagsForResource` must report back.
#[derive(Debug)]
struct TagCase {
    case: &'static str,
    tags: BTreeMap<String, String>,
}

/// Build a tag map from borrowed pairs, so a row reads as the pairs it is.
pub(crate) fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The table: rows differ along what the rule generalises over — the NUMBER of tags
/// (none, one, a few, many), and the alphabet, length and emptiness of the keys and
/// values. `Tags` is modelled as a map of plain `__string` to plain `__string` with no
/// length, pattern or count constraint on either
/// (`smithy_query(query="shape",shape="Tags")`), so none of these rows sits outside a
/// modelled bound.
fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            case: "no tags at all",
            tags: BTreeMap::new(),
        },
        TagCase {
            case: "a single tag",
            tags: tags(&[("team", "payments")]),
        },
        TagCase {
            case: "several tags, in no order a caller would have sorted them",
            tags: tags(&[
                ("zone", "c"),
                ("Env", "test"),
                ("cost-center", "1234"),
                ("owner", "platform"),
            ]),
        },
        TagCase {
            case: "keys and values outside ASCII, and an empty value",
            tags: tags(&[
                ("owner", "caf\u{e9} \u{2014} \u{30b9}\u{30ad}\u{30fc}\u{30de}"),
                ("note", ""),
                ("release/channel", "beta"),
                ("\u{30c1}\u{30fc}\u{30e0}", "\u{6c5f}\u{6238}"),
            ]),
        },
        TagCase {
            case: "a key and a value far longer than one path segment",
            tags: BTreeMap::from([(
                "k".repeat(128),
                "v".repeat(256),
            )]),
        },
        TagCase {
            case: "many tags at once",
            tags: (0..32)
                .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
                .collect(),
        },
    ]
}

/// The resource types of this service that carry a tag set — the three its ARNs name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Registry,
    Schema,
    Discoverer,
}

impl Kind {
    pub(crate) const ALL: [Kind; 3] = [Kind::Registry, Kind::Schema, Kind::Discoverer];
}

/// One created resource: the ARN its create reported, and the URI its own Describe
/// reads it back on.
#[derive(Debug)]
pub(crate) struct Created {
    pub(crate) arn: String,
    pub(crate) describe_uri: String,
}

/// Create one resource of `kind`, tagged with `tags`, asserting the create succeeded.
pub(crate) async fn create_tagged(
    emulator: &Emulator,
    case: &str,
    kind: Kind,
    name: &str,
    tags: &BTreeMap<String, String>,
) -> Created {
    match kind {
        Kind::Registry => {
            let created = emulator
                .call("POST", &registry_uri(name), Some(with_tags(json!({}), tags)))
                .await;
            assert_eq!(
                created.status, 201,
                "case {case:?}: expected 201 from CreateRegistry, got {} {:?}",
                created.status, created.body
            );
            Created {
                arn: created.str_at("RegistryArn"),
                describe_uri: registry_uri(name),
            }
        }
        Kind::Schema => {
            let registry = format!("{name}-registry");
            create_registry(emulator, &registry).await;
            let created = emulator
                .call(
                    "POST",
                    &schema_uri(&registry, name),
                    Some(with_tags(
                        json!({"Content": CONTENT, "Type": "OpenApi3"}),
                        tags,
                    )),
                )
                .await;
            assert_eq!(
                created.status, 201,
                "case {case:?}: expected 201 from CreateSchema, got {} {:?}",
                created.status, created.body
            );
            Created {
                arn: created.str_at("SchemaArn"),
                describe_uri: schema_uri(&registry, name),
            }
        }
        Kind::Discoverer => {
            let created = emulator
                .call(
                    "POST",
                    DISCOVERERS_URI,
                    Some(with_tags(json!({"SourceArn": source_arn(name)}), tags)),
                )
                .await;
            assert_eq!(
                created.status, 201,
                "case {case:?}: expected 201 from CreateDiscoverer, got {} {:?}",
                created.status, created.body
            );
            Created {
                arn: created.str_at("DiscovererArn"),
                describe_uri: discoverer_uri(&created.str_at("DiscovererId")),
            }
        }
    }
}

/// The tags a resource holds now, read through BOTH readers of one stored tag map —
/// `ListTagsForResource` and the resource's own Describe — which must agree.
///
/// The crate's one reader for a tag-write test: the suites that drive `TagResource` and
/// `UntagResource` both check what a write LEFT through this, so the two cannot drift
/// apart on what "the resource's tags" means.
pub(crate) async fn tags_now(
    emulator: &Emulator,
    case: &str,
    created: &Created,
) -> BTreeMap<String, String> {
    let listed = emulator.call("GET", &tags_uri(&created.arn), None).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from ListTagsForResource, got {} {:?}",
        listed.status, listed.body
    );
    let described = emulator.call("GET", &created.describe_uri, None).await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: expected 200 from the resource's Describe, got {} {:?}",
        described.status, described.body
    );
    assert_eq!(
        reported_tags(&described, case),
        reported_tags(&listed, case),
        "case {case:?}: the resource's Describe and ListTagsForResource must report \
         one stored tag set, got {:?} and {:?}",
        described.body.get("tags"),
        listed.body.get("tags")
    );
    reported_tags(&listed, case)
}

/// A resource name that is legal for every one of the three types and distinct per row.
pub(crate) fn resource_name(kind: Kind, row: usize) -> String {
    let kind = match kind {
        Kind::Registry => "registry",
        Kind::Schema => "schema",
        Kind::Discoverer => "discoverer",
    };
    format!("tagged-{kind}-{row}")
}

/// RULE: `ListTagsForResource` reports the tag set the store holds for the resource its
/// `ResourceArn` names, whichever of this service's taggable resource types that is,
/// and a read neither changes it nor disagrees with the resource's own Describe.
#[tokio::test]
async fn the_tags_a_create_stored_are_reported_for_every_taggable_resource_type() {
    let emulator = Emulator::new();

    for kind in Kind::ALL {
        for (row, expected) in tag_cases().into_iter().enumerate() {
            let case = format!("{kind:?}/{}", expected.case);
            let created = create_tagged(
                &emulator,
                &case,
                kind,
                &resource_name(kind, row),
                &expected.tags,
            )
            .await;

            // Two reads in a row: the tags come from the store, so reading them does
            // not change what the next caller sees.
            for read in 0..2 {
                let listed = emulator.call("GET", &tags_uri(&created.arn), None).await;
                assert_eq!(
                    listed.status, 200,
                    "case {case:?} (read {read}): expected 200 from ListTagsForResource, got {} {:?}",
                    listed.status, listed.body
                );
                assert_eq!(
                    reported_tags(&listed, &case),
                    expected.tags,
                    "case {case:?} (read {read}): expected the stored tags, got {:?}",
                    listed.body.get("tags")
                );
            }

            // And the resource's own Describe reports the same set, so the two readers
            // of one stored tag map cannot drift apart.
            let described = emulator.call("GET", &created.describe_uri, None).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: expected 200 from the resource's Describe, got {} {:?}",
                described.status, described.body
            );
            assert_eq!(
                reported_tags(&described, &case),
                expected.tags,
                "case {case:?}: Describe must report the same tags as ListTagsForResource, got {:?}",
                described.body.get("tags")
            );
        }
    }
}

/// RULE: the tag set belongs to the ONE resource the ARN names — several resources of
/// several types coexist, each reporting only its own tags.
#[tokio::test]
async fn each_resource_reports_only_its_own_tags() {
    let emulator = Emulator::new();

    let mut expected: Vec<(String, BTreeMap<String, String>, String)> = Vec::new();
    for (row, kind) in Kind::ALL.into_iter().enumerate() {
        for (index, pairs) in [
            vec![("owner", "a"), ("env", "prod")],
            vec![("owner", "b")],
            vec![],
        ]
        .into_iter()
        .enumerate()
        {
            let case = format!("{kind:?}-{index}");
            let wanted = tags(&pairs);
            let name = resource_name(kind, 100 + row * 10 + index);
            let created = create_tagged(&emulator, &case, kind, &name, &wanted).await;
            expected.push((created.arn, wanted, case));
        }
    }

    for (arn, wanted, case) in &expected {
        let listed = emulator.call("GET", &tags_uri(arn), None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200 from ListTagsForResource, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            &reported_tags(&listed, case),
            wanted,
            "case {case:?}: a resource reports its own tags and no other's, got {:?}",
            listed.body.get("tags")
        );
    }
}

/// RULE: a resource is named by the ARN this service REPORTS for it — so the ARN a read
/// of the resource hands back resolves to that resource, and an ARN the service never
/// issued (another region's, for a resource whose ARN is regional) names none.
///
/// A service-owned registry is created by no request, so the ARN it reports names the
/// region the asking request is signed for; a caller's registry reports the ARN its
/// create minted, whichever region a later request is signed for.
#[tokio::test]
async fn a_resource_is_named_by_the_arn_the_service_reports_for_it() {
    let emulator = Emulator::new();
    let registry = "cross-region-registry";
    create_registry(&emulator, registry).await;

    let regions = ["us-west-2", "eu-west-1", "ap-southeast-2"];

    // A caller's registry: one ARN, minted by its create, and every region's request
    // resolves it, because that is the ARN every region's read of it reports.
    let minted = emulator
        .call("GET", &registry_uri(registry), None)
        .await
        .str_at("RegistryArn");
    for region in regions {
        let described = emulator
            .call_signed_for(region, "GET", &registry_uri(registry), None)
            .await;
        assert_eq!(
            described.str_at("RegistryArn"),
            minted,
            "case {region:?}: a caller's registry reports the ARN its create minted"
        );
        let listed = emulator
            .call_signed_for(region, "GET", &tags_uri(&minted), None)
            .await;
        assert_eq!(
            listed.status, 200,
            "case {region:?}: the minted ARN names the registry in every region, got {} {:?}",
            listed.status, listed.body
        );
    }

    // A service-owned registry: the ARN it reports is the asking region's, and that is
    // exactly the ARN that resolves — another region's does not name it.
    let managed_uri = registry_uri(AWS_MANAGED_REGISTRY);
    for region in regions {
        let reported = emulator
            .call_signed_for(region, "GET", &managed_uri, None)
            .await
            .str_at("RegistryArn");
        assert!(
            reported.contains(region),
            "case {region:?}: a service-owned registry reports the asking region, got {reported:?}"
        );

        let listed = emulator
            .call_signed_for(region, "GET", &tags_uri(&reported), None)
            .await;
        assert_eq!(
            listed.status, 200,
            "case {region:?}: the reported ARN names the service-owned registry, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            reported_tags(&listed, region),
            BTreeMap::new(),
            "case {region:?}: a service-owned registry carries no tags, got {:?}",
            listed.body.get("tags")
        );

        for other in regions.into_iter().filter(|other| *other != region) {
            let elsewhere = emulator
                .call_signed_for(other, "GET", &tags_uri(&reported), None)
                .await;
            assert_eq!(
                elsewhere.status, 404,
                "case {region:?} read from {other:?}: an ARN of another region names no \
                 service-owned registry, got {} {:?}",
                elsewhere.status, elsewhere.body
            );
        }
    }
}

/// RULE: a resource ARN of this service that names nothing this store holds is refused
/// with HTTP 404 `NotFoundException`.
#[tokio::test]
async fn a_resource_arn_that_names_nothing_is_not_found() {
    let emulator = Emulator::new();
    let registry = "present-registry";
    create_registry(&emulator, registry).await;
    let schema = "order.placed";
    let created = emulator
        .call(
            "POST",
            &schema_uri(registry, schema),
            Some(json!({"Content": CONTENT, "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "seeding a schema must succeed, got {} {:?}",
        created.status, created.body
    );
    let deleted_registry = "deleted-registry";
    create_registry(&emulator, deleted_registry).await;
    let gone = emulator
        .call("DELETE", &registry_uri(deleted_registry), None)
        .await;
    assert_eq!(
        gone.status, 204,
        "deleting a registry must succeed, got {} {:?}",
        gone.status, gone.body
    );

    // A schema of the service-owned catalogue: the service reports NO ARN for one
    // (every read of it answers an empty `SchemaArn`), so no ARN names it.
    let managed_schema = aws_managed_schemas()
        .first()
        .expect("the service-owned catalogue holds schemas")
        .name
        .clone();

    // Rows: each of the three resource types named with an identifier nothing holds, a
    // resource that was deleted, a resource whose container exists, a resource the
    // service reports no ARN for, and an ARN of the right form in another account.
    for (case, arn) in [
        (
            "a registry no caller created",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:registry/absent-registry"),
        ),
        (
            "a registry that was deleted",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:registry/{deleted_registry}"),
        ),
        (
            "a registry differing from a stored one only in case",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:registry/Present-Registry"),
        ),
        (
            "a schema nothing created, in a registry that exists",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:schema/{registry}/absent.schema"),
        ),
        (
            "a schema of a registry nothing created",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:schema/absent-registry/{schema}"),
        ),
        (
            "a schema of the service-owned catalogue, which has no ARN",
            format!(
                "arn:aws:schemas:{REGION}:{ACCOUNT}:schema/{AWS_MANAGED_REGISTRY}/{managed_schema}"
            ),
        ),
        (
            "a discoverer id the service never minted",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:discoverer/00000000000000000000000000000000"),
        ),
        (
            "a registry of another account",
            format!("arn:aws:schemas:{REGION}:999999999999:registry/{registry}"),
        ),
        (
            "a registry of another partition",
            format!("arn:aws-cn:schemas:{REGION}:{ACCOUNT}:registry/{registry}"),
        ),
    ] {
        let refused = emulator.call("GET", &tags_uri(&arn), None).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected 404 from ListTagsForResource, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.str_at("Code"),
            CODE_RESOURCE_NOT_FOUND,
            "case {case:?}: expected the service's not-found code, got {:?}",
            refused.body
        );
    }
}

/// RULE: a `ResourceArn` that is no resource ARN of this service — not an ARN at all,
/// another service's, or naming no resource type this service has — is refused on its
/// form with HTTP 400 `BadRequestException`.
#[tokio::test]
async fn a_value_that_is_no_resource_arn_of_this_service_is_refused() {
    let emulator = Emulator::new();
    create_registry(&emulator, "form-registry").await;

    for (case, value) in [
        ("a bare name", "form-registry".to_string()),
        ("free text", "not an arn at all".to_string()),
        (
            "an ARN missing its resource field",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}"),
        ),
        (
            "an ARN whose resource field is empty",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:"),
        ),
        (
            "another service's resource ARN",
            format!("arn:aws:events:{REGION}:{ACCOUNT}:event-bus/default"),
        ),
        (
            "a resource type this service does not have",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:codebinding/form-registry"),
        ),
        (
            "a registry ARN with no registry name",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:registry/"),
        ),
        (
            "a registry ARN carrying a second path segment",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:registry/form-registry/extra"),
        ),
        (
            "a schema ARN naming no schema",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:schema/form-registry"),
        ),
        (
            "a schema ARN with an empty schema name",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:schema/form-registry/"),
        ),
        (
            "a schema ARN carrying a segment past the schema name",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:schema/form-registry/order.placed/1"),
        ),
        (
            "a discoverer ARN naming no discoverer",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:discoverer/"),
        ),
        (
            "the service name in the wrong case",
            format!("arn:aws:Schemas:{REGION}:{ACCOUNT}:registry/form-registry"),
        ),
    ] {
        let refused = emulator.call("GET", &tags_uri(&value), None).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 from ListTagsForResource, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.str_at("Code"),
            CODE_MALFORMED_REQUEST,
            "case {case:?}: expected the service's malformed-request code, got {:?}",
            refused.body
        );
        assert!(
            !refused.str_at("Message").contains(&value),
            "case {case:?}: the refusal names the member and the form it must take, \
             not the value sent, got {:?}",
            refused.str_at("Message")
        );
    }
}
