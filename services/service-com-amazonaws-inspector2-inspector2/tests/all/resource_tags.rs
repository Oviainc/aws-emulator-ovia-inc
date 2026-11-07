// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Inspector tagging surface: `TagResource`,
//! `ListTagsForResource` and `UntagResource`.
//!
//! The rule under test: a resource's tags are part of that resource's stored record, so
//! `resourceArn` is resolved against the resources this service stores —
//! `ListTagsForResource` reports exactly the stored tag set, `TagResource` merges the tags
//! it carries into it (a key already present takes the new value, every other stored key
//! stays) and `UntagResource` removes exactly the keys it names, while an ARN naming no
//! stored resource is refused by all three as a missing resource.
//!
//! Every request goes through the real `MultiServiceRouter` — routing, restJson1
//! deserialization, modelled-constraint validation, the handler, and restJson1
//! serialization — so what is asserted here is the wire behaviour a client sees.

use std::collections::BTreeMap;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_inspector2_inspector2::state::{
    INVALID_TAGS_MESSAGE, MAX_TAGS_PER_RESOURCE, TAGGED_RESOURCE_NOT_FOUND_MESSAGE,
};

// The signed-request harness this crate's suites share: `router()` builds a router over
// its own engine, `post_in`/`send_in` send one signed restJson1 request for a region.
use crate::filters_and_enablement::{post_in, router, send_in};

/// The single account id every generated emulator presents
/// (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// `value` as ONE URI path or query segment, RFC 3986 percent-encoding applied over
/// everything but the ASCII alphanumerics.
///
/// That is what an AWS SDK sends for a non-greedy `@httpLabel`, and encoding `/` and `:`
/// is what makes an ARN ONE path segment rather than several.
fn percent_encode_segment(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

/// `ListTagsForResource` — `GET /tags/{resourceArn}`.
async fn list_tags(
    router: &MultiServiceRouter,
    region: &str,
    resource_arn: &str,
) -> (u16, Option<String>, Value) {
    let uri = format!("/tags/{}", percent_encode_segment(resource_arn));
    send_in(router, "GET", region, &uri, None).await
}

/// `TagResource` — `POST /tags/{resourceArn}` with the tag map in the body.
async fn tag(
    router: &MultiServiceRouter,
    region: &str,
    resource_arn: &str,
    tags: &Value,
) -> (u16, Option<String>, Value) {
    let uri = format!("/tags/{}", percent_encode_segment(resource_arn));
    send_in(
        router,
        "POST",
        region,
        &uri,
        Some(&json!({ "tags": tags.clone() })),
    )
    .await
}

/// `UntagResource` — `DELETE /tags/{resourceArn}?tagKeys=…`, every key its own
/// `@httpQuery` pair, as an SDK sends a list bound to the query string.
async fn untag(
    router: &MultiServiceRouter,
    region: &str,
    resource_arn: &str,
    keys: &[&str],
) -> (u16, Option<String>, Value) {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", percent_encode_segment(key)))
        .collect();
    let uri = format!(
        "/tags/{}?{}",
        percent_encode_segment(resource_arn),
        query.join("&")
    );
    send_in(router, "DELETE", region, &uri, None).await
}

/// The `tags` member of a `ListTagsForResource` answer, as a sorted map.
fn tags_of(case: &str, body: &Value) -> BTreeMap<String, String> {
    match body.get("tags") {
        Some(Value::Object(entries)) => entries
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case:?}: a non-string tag in {body}"))
                        .to_string(),
                )
            })
            .collect(),
        other => panic!("case {case:?}: expected a tags map, got {other:?} in {body}"),
    }
}

/// A tag map from `(key, value)` pairs, as the wire carries it.
fn tag_json(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect::<Map<String, Value>>(),
    )
}

/// A sorted tag map from `(key, value)` pairs.
fn expected_tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The taggable resources of this service: every record kind that stores a `tags` member,
/// so the rule is asserted on each of them rather than on one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Filter,
    CisScanConfiguration,
    CodeSecurityScanConfiguration,
}

const KINDS: [Kind; 3] = [
    Kind::Filter,
    Kind::CisScanConfiguration,
    Kind::CodeSecurityScanConfiguration,
];

impl Kind {
    /// The create route that mints a resource of this kind.
    fn create_uri(self) -> &'static str {
        match self {
            Kind::Filter => "/filters/create",
            Kind::CisScanConfiguration => "/cis/scan-configuration/create",
            Kind::CodeSecurityScanConfiguration => "/codesecurity/scan-configuration/create",
        }
    }

    /// The delete route that removes one.
    fn delete_uri(self) -> &'static str {
        match self {
            Kind::Filter => "/filters/delete",
            Kind::CisScanConfiguration => "/cis/scan-configuration/delete",
            Kind::CodeSecurityScanConfiguration => "/codesecurity/scan-configuration/delete",
        }
    }

    /// The member the create answers the minted ARN under, which is also the member its
    /// delete takes.
    fn arn_member(self) -> &'static str {
        match self {
            Kind::Filter => "arn",
            Kind::CisScanConfiguration | Kind::CodeSecurityScanConfiguration => {
                "scanConfigurationArn"
            }
        }
    }

    /// A create body for a resource of this kind named `name`, carrying `tags` when the
    /// case asks for a `tags` member at all.
    fn create_body(self, name: &str, tags: Option<&Value>) -> Value {
        let mut body = match self {
            Kind::Filter => json!({
                "action": "NONE",
                "name": name,
                "filterCriteria": { "title": [{ "comparison": "EQUALS", "value": name }] },
            }),
            Kind::CisScanConfiguration => json!({
                "scanName": name,
                "securityLevel": "LEVEL_1",
                "schedule": { "oneTime": {} },
                "targets": {
                    "accountIds": ["SELF"],
                    "targetResourceTags": { "Environment": ["production"] },
                },
            }),
            Kind::CodeSecurityScanConfiguration => json!({
                "name": name,
                "level": "ACCOUNT",
                "configuration": { "ruleSetCategories": ["SAST"] },
            }),
        };
        if let Some(tags) = tags {
            body["tags"] = tags.clone();
        }
        body
    }

    /// A well-formed ARN of this kind that this emulator never minted, for the paths that
    /// need an ARN naming no stored resource.
    ///
    /// Sources: `smithy_query(query="shape", shape="FilterArn,CisScanConfigurationArn,CodeSecurityScanConfigurationArn")`.
    fn unminted_arn(self) -> String {
        match self {
            Kind::Filter => {
                format!("arn:aws:inspector2:us-east-1:{ACCOUNT}:owner/{ACCOUNT}/filter/deadbeef")
            }
            Kind::CisScanConfiguration => format!(
                "arn:aws:inspector2:us-east-1:{ACCOUNT}:owner/{ACCOUNT}/cis-configuration/\
                 aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
            ),
            Kind::CodeSecurityScanConfiguration => format!(
                "arn:aws:inspector2:us-east-1:{ACCOUNT}:owner/{ACCOUNT}/\
                 codesecurity-configuration/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
            ),
        }
    }
}

/// Create a resource of `kind` in `region`, which must succeed, and answer its ARN.
async fn create(
    router: &MultiServiceRouter,
    case: &str,
    kind: Kind,
    region: &str,
    name: &str,
    tags: Option<&Value>,
) -> String {
    let (status, error_type, created) =
        post_in(router, region, kind.create_uri(), &kind.create_body(name, tags)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating a {kind:?}: expected 200, got {status} / {created} \
         ({error_type:?})"
    );
    created[kind.arn_member()]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no {} in {created}", kind.arn_member()))
        .to_string()
}

/// One row of the round-trip table: a resource kind, the tag member its create carried,
/// the tags a `TagResource` then merges in, the keys an `UntagResource` then removes, and
/// the tag set the resource must carry after each step.
struct Row {
    case: &'static str,
    kind: Kind,
    region: &'static str,
    /// The `tags` the create carried, or `None` for a create with no `tags` member at all.
    created_with: Option<&'static [(&'static str, &'static str)]>,
    /// What `ListTagsForResource` must report right after the create.
    after_create: &'static [(&'static str, &'static str)],
    /// The tags `TagResource` merges in.
    merged: &'static [(&'static str, &'static str)],
    /// What `ListTagsForResource` must report after that merge.
    after_merge: &'static [(&'static str, &'static str)],
    /// The keys `UntagResource` then removes.
    removed: &'static [&'static str],
    /// What `ListTagsForResource` must report after that removal.
    after_removal: &'static [(&'static str, &'static str)],
}

/// The rows the round-trip test drives, spanning what the rule generalises over: each
/// taggable resource kind, a create with no `tags` member / an empty map / tags present,
/// a merge that only adds keys / only overwrites keys / does both / changes nothing (an
/// empty map), a removal of one key / several keys / a key the resource never carried /
/// every key it carries, and four regions.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a filter created with no tags, tagged then untagged",
            kind: Kind::Filter,
            region: "us-east-1",
            created_with: None,
            after_create: &[],
            merged: &[("env", "prod"), ("owner", "security-team")],
            after_merge: &[("env", "prod"), ("owner", "security-team")],
            removed: &["env"],
            after_removal: &[("owner", "security-team")],
        },
        Row {
            case: "a filter created with tags, a merge that overwrites and adds",
            kind: Kind::Filter,
            region: "eu-central-1",
            created_with: Some(&[("env", "dev"), ("costCenter", "4711")]),
            after_create: &[("costCenter", "4711"), ("env", "dev")],
            merged: &[("env", "prod"), ("team", "platform")],
            after_merge: &[("costCenter", "4711"), ("env", "prod"), ("team", "platform")],
            removed: &["costCenter", "team"],
            after_removal: &[("env", "prod")],
        },
        Row {
            case: "a CIS scan configuration created with an empty tag map, one tag added",
            kind: Kind::CisScanConfiguration,
            region: "us-west-2",
            created_with: Some(&[]),
            after_create: &[],
            merged: &[("purpose", "benchmark")],
            after_merge: &[("purpose", "benchmark")],
            // A key the resource never carried is nothing to remove.
            removed: &["absent"],
            after_removal: &[("purpose", "benchmark")],
        },
        Row {
            case: "a CIS scan configuration created with tags, every key removed",
            kind: Kind::CisScanConfiguration,
            region: "ap-southeast-1",
            created_with: Some(&[("owner", "security-team")]),
            after_create: &[("owner", "security-team")],
            merged: &[("severity", "high")],
            after_merge: &[("owner", "security-team"), ("severity", "high")],
            removed: &["owner", "severity"],
            after_removal: &[],
        },
        Row {
            case: "a code security scan configuration created with no tags, an empty merge",
            kind: Kind::CodeSecurityScanConfiguration,
            region: "eu-west-2",
            created_with: None,
            after_create: &[],
            // An empty map changes nothing, which a later read has to agree with.
            merged: &[],
            after_merge: &[],
            removed: &["env"],
            after_removal: &[],
        },
        Row {
            case: "a code security scan configuration created with tags, a merge that only \
                   overwrites",
            kind: Kind::CodeSecurityScanConfiguration,
            region: "us-east-1",
            created_with: Some(&[("env", "staging"), ("owner", "appsec")]),
            after_create: &[("env", "staging"), ("owner", "appsec")],
            merged: &[("env", "prod"), ("owner", "appsec")],
            after_merge: &[("env", "prod"), ("owner", "appsec")],
            removed: &["owner"],
            after_removal: &[("env", "prod")],
        },
    ]
}

/// RULE: the tag set a resource carries is part of its stored record — a create's `tags`
/// are what the tagging surface reports, `TagResource` merges into that set and
/// `UntagResource` removes exactly the keys it names, and every read in between reports
/// what is stored.
#[tokio::test]
async fn tags_written_through_the_tagging_surface_round_trip_as_stored() {
    for row in rows() {
        let case = row.case;
        let router = router();

        let created_tags = row.created_with.map(tag_json);
        let arn = create(
            &router,
            case,
            row.kind,
            row.region,
            "tagged-resource",
            created_tags.as_ref(),
        )
        .await;

        // What the create stored is what the tagging surface reports.
        let (status, error_type, listed) = list_tags(&router, row.region, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource after the create: expected 200, got {status} / \
             {listed} ({error_type:?})"
        );
        assert_eq!(
            tags_of(case, &listed),
            expected_tags(row.after_create),
            "case {case:?}: expected the tags the create stored, got {listed}"
        );

        // The merge.
        let (status, error_type, tagged) =
            tag(&router, row.region, &arn, &tag_json(row.merged)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource: expected 200, got {status} / {tagged} ({error_type:?})"
        );
        let (status, _, listed) = list_tags(&router, row.region, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource after the merge: expected 200, got {status} / \
             {listed}"
        );
        assert_eq!(
            tags_of(case, &listed),
            expected_tags(row.after_merge),
            "case {case:?}: expected the merged tag set, got {listed}"
        );

        // A second read reports the same thing: reporting tags writes nothing.
        let (status, _, again) = list_tags(&router, row.region, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the second ListTagsForResource: expected 200, got {status} / {again}"
        );
        assert_eq!(
            tags_of(case, &again),
            expected_tags(row.after_merge),
            "case {case:?}: a read must not change the stored tag set, got {again}"
        );

        // The removal.
        let (status, error_type, untagged) = untag(&router, row.region, &arn, row.removed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource: expected 200, got {status} / {untagged} \
             ({error_type:?})"
        );
        let (status, _, listed) = list_tags(&router, row.region, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource after the removal: expected 200, got {status} / \
             {listed}"
        );
        assert_eq!(
            tags_of(case, &listed),
            expected_tags(row.after_removal),
            "case {case:?}: expected exactly the named keys removed, got {listed}"
        );

        // Repeating the removal removes nothing more: a key the resource does not carry is
        // nothing to remove.
        let (status, _, repeated) = untag(&router, row.region, &arn, row.removed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeated UntagResource: expected 200, got {status} / {repeated}"
        );
        let (status, _, listed) = list_tags(&router, row.region, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource after the repeated removal: expected 200, got \
             {status} / {listed}"
        );
        assert_eq!(
            tags_of(case, &listed),
            expected_tags(row.after_removal),
            "case {case:?}: a repeated removal must change nothing, got {listed}"
        );
    }
}

/// RULE: the tags the tagging surface writes live on the resource's own record, so the
/// operation that reads that record reports the very same tag set.
#[tokio::test]
async fn a_tag_written_through_tag_resource_is_part_of_the_resources_own_record() {
    let router = router();
    let region = "us-east-1";
    let case = "a code security scan configuration read back through its own get";

    let arn = create(
        &router,
        case,
        Kind::CodeSecurityScanConfiguration,
        region,
        "record-tags",
        Some(&tag_json(&[("env", "dev")])),
    )
    .await;

    let (status, error_type, tagged) = tag(
        &router,
        region,
        &arn,
        &tag_json(&[("env", "prod"), ("owner", "appsec")]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: TagResource: expected 200, got {status} / {tagged} ({error_type:?})"
    );

    // The resource's own read, not the tagging surface.
    let (status, _, read) = post_in(
        &router,
        region,
        "/codesecurity/scan-configuration/get",
        &json!({ "scanConfigurationArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCodeSecurityScanConfiguration: expected 200, got {status} / {read}"
    );
    assert_eq!(
        read["tags"],
        tag_json(&[("env", "prod"), ("owner", "appsec")]),
        "case {case:?}: expected the record to carry the tags TagResource wrote, got {read}"
    );

    let (status, _, untagged) = untag(&router, region, &arn, &["env"]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UntagResource: expected 200, got {status} / {untagged}"
    );
    let (status, _, read) = post_in(
        &router,
        region,
        "/codesecurity/scan-configuration/get",
        &json!({ "scanConfigurationArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCodeSecurityScanConfiguration after the removal: expected 200, got \
         {status} / {read}"
    );
    assert_eq!(
        read["tags"],
        tag_json(&[("owner", "appsec")]),
        "case {case:?}: expected the record to have lost exactly the removed key, got {read}"
    );
}

/// RULE: a `resourceArn` naming no resource this service stores is refused as a missing
/// resource by every tagging call — a 404 `ResourceNotFoundException`, never an empty tag
/// set or a silent success.
#[tokio::test]
async fn every_tagging_call_refuses_an_arn_that_names_no_stored_resource() {
    let region = "us-east-1";
    // Rows differ along WHY the ARN names nothing: never minted (one per resource kind), a
    // resource that was deleted, an ARN of another service, and a value that is no ARN at
    // all — admitted here because `Arn` is modelled with a length only, no pattern
    // (`smithy_query(query="shape", shape="Arn")`).
    let mut cases: Vec<(String, String)> = KINDS
        .iter()
        .map(|kind| (format!("a never-minted {kind:?} ARN"), kind.unminted_arn()))
        .collect();
    cases.push((
        "an ARN of another service".to_string(),
        format!("arn:aws:sqs:us-east-1:{ACCOUNT}:queue-name"),
    ));
    cases.push(("no ARN at all".to_string(), "not-an-arn".to_string()));

    for (case, arn) in cases {
        let router = router();
        for (operation, (status, error_type, body)) in [
            (
                "ListTagsForResource",
                list_tags(&router, region, &arn).await,
            ),
            (
                "TagResource",
                tag(&router, region, &arn, &tag_json(&[("env", "prod")])).await,
            ),
            ("UntagResource", untag(&router, region, &arn, &["env"]).await),
        ] {
            assert_eq!(
                status, 404,
                "case {case:?}: {operation}: expected 404, got {status} / {body}"
            );
            assert_eq!(
                error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: {operation}: expected ResourceNotFoundException, got \
                 {error_type:?} / {body}"
            );
            assert_eq!(
                body["message"],
                json!(TAGGED_RESOURCE_NOT_FOUND_MESSAGE),
                "case {case:?}: {operation}: expected the tagging path's not-found message, got \
                 {body}"
            );
        }
    }

    // And a resource that WAS stored stops answering the moment it is deleted.
    for kind in KINDS {
        let case = format!("a deleted {kind:?}");
        let router = router();
        let arn = create(&router, &case, kind, region, "to-be-deleted", None).await;
        let (status, _, listed) = list_tags(&router, region, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource before the delete: expected 200, got {status} / \
             {listed}"
        );

        let (status, _, deleted) = post_in(
            &router,
            region,
            kind.delete_uri(),
            &json!({ kind.arn_member(): arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete: expected 200, got {status} / {deleted}"
        );

        let (status, error_type, body) = list_tags(&router, region, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: ListTagsForResource after the delete: expected 404, got {status} / \
             {body}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {error_type:?} / {body}"
        );
    }
}

/// RULE: a tag key carrying the prefix Amazon Web Services reserves for its own tags is
/// not a valid tag key — the model's `TagKey` pattern excludes it with a negative
/// lookahead — so a `TagResource` carrying one is refused and writes nothing, while a key
/// that merely starts with those letters is accepted.
#[tokio::test]
async fn tag_resource_refuses_a_reserved_tag_key_and_writes_nothing() {
    let region = "us-east-1";
    // Rows differ along WHERE the reserved key sits in the request and what follows the
    // prefix; the accepted rows are keys the lookahead does NOT exclude.
    let refused: [(&str, &[(&str, &str)]); 4] = [
        ("the reserved prefix alone", &[("aws:", "x")]),
        (
            "a reserved key",
            &[("aws:cloudformation:stack-name", "fleet")],
        ),
        (
            "a reserved key beside a valid one",
            &[("env", "prod"), ("aws:service", "inspector2")],
        ),
        ("a reserved key in upper case value", &[("aws:Env", "PROD")]),
    ];
    let accepted: [(&str, &[(&str, &str)]); 2] = [
        ("a key that only starts with the letters", &[("awsome", "1")]),
        ("a key carrying the colon elsewhere", &[("team:aws", "core")]),
    ];

    for kind in KINDS {
        for (case, tags) in refused {
            let router = router();
            let arn = create(&router, case, kind, region, "reserved-keys", None).await;
            // Seed a tag set a refused write must leave untouched.
            let (status, _, seeded) = tag(&router, region, &arn, &tag_json(&[("kept", "yes")])).await;
            assert_eq!(
                status, 200,
                "case {case:?} ({kind:?}): seeding a tag: expected 200, got {status} / {seeded}"
            );

            let (status, error_type, body) = tag(&router, region, &arn, &tag_json(tags)).await;
            assert_eq!(
                status, 400,
                "case {case:?} ({kind:?}): expected 400, got {status} / {body}"
            );
            assert_eq!(
                error_type.as_deref(),
                Some("BadRequestException"),
                "case {case:?} ({kind:?}): expected BadRequestException, got {error_type:?} / \
                 {body}"
            );
            assert_eq!(
                body["message"],
                json!(INVALID_TAGS_MESSAGE),
                "case {case:?} ({kind:?}): expected the invalid-tag message, got {body}"
            );

            let (status, _, listed) = list_tags(&router, region, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?} ({kind:?}): ListTagsForResource: expected 200, got {status} / \
                 {listed}"
            );
            assert_eq!(
                tags_of(case, &listed),
                expected_tags(&[("kept", "yes")]),
                "case {case:?} ({kind:?}): a refused write must store nothing, got {listed}"
            );
        }

        for (case, tags) in accepted {
            let router = router();
            let arn = create(&router, case, kind, region, "allowed-keys", None).await;
            let (status, error_type, body) = tag(&router, region, &arn, &tag_json(tags)).await;
            assert_eq!(
                status, 200,
                "case {case:?} ({kind:?}): expected 200, got {status} / {body} ({error_type:?})"
            );
            let (status, _, listed) = list_tags(&router, region, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?} ({kind:?}): ListTagsForResource: expected 200, got {status} / \
                 {listed}"
            );
            assert_eq!(
                tags_of(case, &listed),
                expected_tags(tags),
                "case {case:?} ({kind:?}): expected the tags as written, got {listed}"
            );
        }
    }
}

/// RULE: a resource carries at most `MAX_TAGS_PER_RESOURCE` tags — the size of the tag set
/// one `UntagResource` call can clear (`TagKeyList`, length 0..50) — so the write that
/// would take it past that is refused and stores nothing, while a write that only
/// overwrites keys already there stays within the bound, and removing a key makes room
/// again.
#[tokio::test]
async fn tag_resource_refuses_past_the_tag_cap_and_writes_nothing() {
    let region = "us-east-1";
    let router = router();
    let case = "the tag cap";
    let arn = create(&router, case, Kind::Filter, region, "capped", None).await;

    let keys: Vec<String> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| format!("key-{index:02}"))
        .collect();
    let at_cap = Value::Object(
        keys.iter()
            .map(|key| (key.clone(), Value::String("v".to_string())))
            .collect::<Map<String, Value>>(),
    );

    // Exactly the bound is accepted.
    let (status, error_type, filled) = tag(&router, region, &arn, &at_cap).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {MAX_TAGS_PER_RESOURCE} tags are within the bound: expected 200, got \
         {status} / {filled} ({error_type:?})"
    );
    let (_, _, listed) = list_tags(&router, region, &arn).await;
    assert_eq!(
        tags_of(case, &listed).len(),
        MAX_TAGS_PER_RESOURCE,
        "case {case:?}: expected {MAX_TAGS_PER_RESOURCE} stored tags, got {listed}"
    );

    // A write that only overwrites keys already stored stays within the bound.
    let (status, _, overwritten) = tag(
        &router,
        region,
        &arn,
        &tag_json(&[("key-00", "replaced"), ("key-01", "replaced")]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: overwriting at the bound: expected 200, got {status} / {overwritten}"
    );
    let (_, _, listed) = list_tags(&router, region, &arn).await;
    let stored = tags_of(case, &listed);
    assert_eq!(
        stored.len(),
        MAX_TAGS_PER_RESOURCE,
        "case {case:?}: an overwrite must not grow the set, got {listed}"
    );
    assert_eq!(
        stored.get("key-00").map(String::as_str),
        Some("replaced"),
        "case {case:?}: expected the overwritten value, got {listed}"
    );

    // One key past the bound is refused, and the refused write stores nothing.
    for over in [1_usize, 7] {
        let past: Vec<String> = (0..over).map(|index| format!("over-{index}")).collect();
        let body_tags = Value::Object(
            past.iter()
                .map(|key| (key.clone(), Value::String("v".to_string())))
                .collect::<Map<String, Value>>(),
        );
        let (status, error_type, refused) = tag(&router, region, &arn, &body_tags).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {over} tags past the bound: expected 400, got {status} / {refused}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: {over} tags past the bound: expected BadRequestException, got \
             {error_type:?} / {refused}"
        );
        let (_, _, listed) = list_tags(&router, region, &arn).await;
        let stored = tags_of(case, &listed);
        assert_eq!(
            stored.len(),
            MAX_TAGS_PER_RESOURCE,
            "case {case:?}: a refused write must store nothing, got {listed}"
        );
        assert!(
            past.iter().all(|key| !stored.contains_key(key)),
            "case {case:?}: none of {past:?} may be stored, got {listed}"
        );
    }

    // Removing a key makes room for another.
    let (status, _, untagged) = untag(&router, region, &arn, &["key-00"]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UntagResource: expected 200, got {status} / {untagged}"
    );
    let (status, _, added) = tag(&router, region, &arn, &tag_json(&[("fresh", "v")])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the freed room: expected 200, got {status} / {added}"
    );
    let (_, _, listed) = list_tags(&router, region, &arn).await;
    let stored = tags_of(case, &listed);
    assert_eq!(
        stored.len(),
        MAX_TAGS_PER_RESOURCE,
        "case {case:?}: expected the set back at the bound, got {listed}"
    );
    assert!(
        stored.contains_key("fresh") && !stored.contains_key("key-00"),
        "case {case:?}: expected the fresh key in place of the removed one, got {listed}"
    );
}
