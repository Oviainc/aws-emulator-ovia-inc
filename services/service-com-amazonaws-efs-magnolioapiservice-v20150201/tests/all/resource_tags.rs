// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EFS resource-tag endpoint `/2015-02-01/resource-tags/{ResourceId}`:
//! `TagResource` writes the tag set a resource holds, `UntagResource` removes keys from
//! it, and `ListTagsForResource` reports that same stored set, driven over HTTP through
//! the same `MultiServiceRouter` the production entrypoint serves.
//!
//! The rule is asserted over a TABLE rather than on one call. The rows vary what the rule
//! generalises over: the resource the endpoint addresses (a file system and an access
//! point — "You can retrieve tags for EFS file systems and access points using this API
//! endpoint"), the form of the id (bare id and ARN, the two forms the modelled
//! `ResourceId` pattern admits), the tag count (none, one, several, the published 50-tag
//! quota), the key and value forms the modelled `TagKey`/`TagValue` patterns allow, and
//! whether a written key is new or already held. `MaxResults` is varied over the modelled
//! lower bound, the documented default and the values either side of it.
//!
//! The harness (`Emulator`, `Reply`, `settle`) and the `Tags` helpers are the ones
//! `file_system_lifecycle.rs` and `tags.rs` already drive this service with.

use serde_json::json;
use service_com_amazonaws_efs_magnolioapiservice_v20150201::state;

use crate::file_system_lifecycle::{Emulator, Reply};
use crate::tags::{as_set, numbered_tags, reported, tags_json};

/// `POST /2015-02-01/resource-tags/{ResourceId}` — the `TagResource` HTTP binding
/// (`smithy_query(query="operation",operation="TagResource")`).
async fn tag_resource(efs: &Emulator, resource_id: &str, tags: &[(String, String)]) -> Reply {
    efs.send(
        "POST",
        &format!("/2015-02-01/resource-tags/{resource_id}"),
        Some(json!({ "Tags": tags_json(tags) })),
    )
    .await
}

/// `GET /2015-02-01/resource-tags/{ResourceId}` — the `ListTagsForResource` HTTP binding
/// (`smithy_query(query="operation",operation="ListTagsForResource")`).
async fn list_tags(efs: &Emulator, resource_id: &str, query: &str) -> Reply {
    let path = if query.is_empty() {
        format!("/2015-02-01/resource-tags/{resource_id}")
    } else {
        format!("/2015-02-01/resource-tags/{resource_id}?{query}")
    };
    efs.send("GET", &path, None).await
}

/// An ARN as an SDK puts it in a non-greedy path label: the `/` percent-encoded.
fn as_label(arn: &str) -> String {
    arn.replace('/', "%2F")
}

/// A value percent-encoded for a query string, as an SDK encodes an `httpQuery` member:
/// every byte outside the unreserved set (RFC 3986) escaped.
fn query_escape(value: &str) -> String {
    let mut escaped = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                escaped.push(char::from(byte))
            }
            _ => escaped.push_str(&format!("%{byte:02X}")),
        }
    }
    escaped
}

/// `DELETE /2015-02-01/resource-tags/{ResourceId}?tagKeys=…` — the `UntagResource` HTTP
/// binding, whose `TagKeys` is an `httpQuery` list
/// (`smithy_query(query="operation",operation="UntagResource")`).
async fn untag_resource(efs: &Emulator, resource_id: &str, keys: &[&str]) -> Reply {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", query_escape(key)))
        .collect::<Vec<String>>()
        .join("&");
    efs.send(
        "DELETE",
        &format!("/2015-02-01/resource-tags/{resource_id}?{query}"),
        None,
    )
    .await
}

/// The two resources this endpoint addresses. Each carries the modelled error its
/// absence is reported with and ids that satisfy its modelled pattern but name nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    FileSystem,
    AccessPoint,
}

impl Kind {
    /// The pair the rule is asserted over.
    const ALL: [Kind; 2] = [Kind::FileSystem, Kind::AccessPoint];

    /// The modelled 404 a `ResourceId` of this kind that names nothing is refused with.
    fn not_found(self) -> &'static str {
        match self {
            Kind::FileSystem => "FileSystemNotFound",
            Kind::AccessPoint => "AccessPointNotFound",
        }
    }

    /// Ids of this kind that satisfy the modelled `ResourceId` pattern — `fs-`/`fsap-`
    /// then 8..40 hex digits, here at both published bounds — but name no resource.
    fn unknown_ids(self) -> Vec<String> {
        let prefix = match self {
            Kind::FileSystem => "fs",
            Kind::AccessPoint => "fsap",
        };
        let resource = match self {
            Kind::FileSystem => "file-system",
            Kind::AccessPoint => "access-point",
        };
        vec![
            format!("{prefix}-00000000"),
            format!("{prefix}-deadbeef"),
            format!("{prefix}-0123456789abcdef0123456789abcdef01234567"),
            as_label(&format!(
                "arn:aws:elasticfilesystem:us-east-1:000000000000:{resource}/{prefix}-deadbeef"
            )),
        ]
    }
}

/// One resource under test: the two `ResourceId` forms that name it.
struct Resource {
    id: String,
    arn_label: String,
}

impl Resource {
    /// Both forms the modelled pattern admits for this resource.
    fn forms(&self) -> [String; 2] {
        [self.id.clone(), self.arn_label.clone()]
    }
}

/// Create one taggable resource of `kind`, carrying `tags`. `host` is an `available`
/// file system an access point can be created on.
async fn create_tagged(
    efs: &Emulator,
    kind: Kind,
    host: &str,
    token: &str,
    tags: &[(String, String)],
) -> Resource {
    match kind {
        Kind::FileSystem => {
            let created = efs
                .create_file_system(json!({
                    "CreationToken": token,
                    "Tags": tags_json(tags),
                }))
                .await;
            assert_eq!(
                created.status,
                201,
                "case {token:?}: CreateFileSystem expected 201, got {} {:?}",
                created.status,
                created.body
            );
            Resource {
                id: created.string("FileSystemId"),
                arn_label: as_label(&created.string("FileSystemArn")),
            }
        }
        Kind::AccessPoint => {
            let created = efs
                .create_access_point(json!({
                    "FileSystemId": host,
                    "ClientToken": token,
                    "Tags": tags_json(tags),
                }))
                .await;
            assert_eq!(
                created.status,
                200,
                "case {token:?}: CreateAccessPoint expected 200, got {} {:?}",
                created.status,
                created.body
            );
            Resource {
                id: created.string("AccessPointId"),
                arn_label: as_label(&created.string("AccessPointArn")),
            }
        }
    }
}

/// A `(&str, &str)` table as owned pairs.
fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One row of the tag-write table: the tags the resource is created with, the tags one
/// `TagResource` call adds, and the set the resource must then report. `expected` is
/// written out per row rather than computed, so the test does not restate the merge the
/// handler performs.
struct WriteCase {
    case: &'static str,
    initial: Vec<(String, String)>,
    written: Vec<(String, String)>,
    expected: Vec<(String, String)>,
}

/// The writes the round-trip rule is asserted over. The key and value forms come from the
/// modelled patterns (`smithy_query(query="shape",shape="TagKey,TagValue")`): `TagKey` is
/// 1..128 of `[\p{L}\p{Z}\p{N}_.:/=+\-@]` and `TagValue` the same class at 0..256, so an
/// empty value and a non-ASCII letter are both in the class. The last row fills the
/// published quota [`state::MAX_TAGS_PER_RESOURCE`] exactly.
fn write_table() -> Vec<WriteCase> {
    vec![
        WriteCase {
            case: "a first tag on an untagged resource",
            initial: Vec::new(),
            written: owned(&[("env", "prod")]),
            expected: owned(&[("env", "prod")]),
        },
        WriteCase {
            case: "several new keys appended to a held set",
            initial: owned(&[("Name", "analytics")]),
            written: owned(&[("env", "prod"), ("owner", "data-platform")]),
            expected: owned(&[
                ("Name", "analytics"),
                ("env", "prod"),
                ("owner", "data-platform"),
            ]),
        },
        WriteCase {
            case: "a held key's value replaced rather than duplicated",
            initial: owned(&[("env", "dev"), ("tier", "gold")]),
            written: owned(&[("env", "prod")]),
            expected: owned(&[("env", "prod"), ("tier", "gold")]),
        },
        WriteCase {
            case: "a new key and a replacing key in one call",
            initial: owned(&[("env", "dev")]),
            written: owned(&[("env", "prod"), ("team", "core")]),
            expected: owned(&[("env", "prod"), ("team", "core")]),
        },
        WriteCase {
            case: "key and value forms the modelled patterns allow",
            initial: owned(&[("empty-value", "set")]),
            written: owned(&[
                ("cost:center/eu", "team=platform+core"),
                ("Équipe.de_test", "valeur naïve"),
                ("owner@example.com", "on-call"),
                ("empty-value", ""),
            ]),
            expected: owned(&[
                ("empty-value", ""),
                ("cost:center/eu", "team=platform+core"),
                ("Équipe.de_test", "valeur naïve"),
                ("owner@example.com", "on-call"),
            ]),
        },
        WriteCase {
            case: "the write that fills the 50-tag quota exactly",
            initial: numbered_tags(state::MAX_TAGS_PER_RESOURCE / 2),
            written: numbered_tags(state::MAX_TAGS_PER_RESOURCE)
                [state::MAX_TAGS_PER_RESOURCE / 2..]
                .to_vec(),
            expected: numbered_tags(state::MAX_TAGS_PER_RESOURCE),
        },
    ]
}

#[tokio::test]
async fn tag_resource_writes_the_set_list_tags_for_resource_reports() {
    let efs = Emulator::new();
    let host = efs.available_file_system("resource-tags-host").await;

    for kind in Kind::ALL {
        for (index, row) in write_table().iter().enumerate() {
            let resource = create_tagged(
                &efs,
                kind,
                &host,
                &format!("write-{kind:?}-{index}"),
                &row.initial,
            )
            .await;

            // The same write through each form the modelled `ResourceId` pattern admits:
            // both name the SAME resource, so the second call replaces the keys the first
            // one wrote instead of adding a second copy of them.
            for form in resource.forms() {
                let written = tag_resource(&efs, &form, &row.written).await;
                assert_eq!(
                    written.status,
                    200,
                    "case {:?} on {kind:?} as {form:?}: TagResource expected 200, got {} {:?}",
                    row.case,
                    written.status,
                    written.body
                );
                assert_eq!(
                    written.body_len, 0,
                    "case {:?} on {kind:?} as {form:?}: TagResource models no output, so the \
                     body is empty, got {:?}",
                    row.case, written.body
                );

                // Read back through both forms too: either names the one stored set.
                for read in resource.forms() {
                    let reply = list_tags(&efs, &read, "").await;
                    assert_eq!(
                        reply.status,
                        200,
                        "case {:?} on {kind:?} written as {form:?}, read as {read:?}: expected \
                         200, got {} {:?}",
                        row.case,
                        reply.status,
                        reply.body
                    );
                    assert_eq!(
                        as_set(&reported(&reply)),
                        as_set(&row.expected),
                        "case {:?} on {kind:?} written as {form:?}, read as {read:?}: unexpected \
                         tag set, got {:?}",
                        row.case,
                        reply.get("Tags")
                    );
                    assert!(
                        reply.get("NextToken").is_null(),
                        "case {:?} on {kind:?} read as {read:?}: {} tags fit the default page, so \
                         there is no NextToken, got {:?}",
                        row.case,
                        row.expected.len(),
                        reply.get("NextToken")
                    );
                }
            }

            // One resource's tags may not leak into another's: every row's resources live
            // in the SAME emulator, so the answer is keyed by resource.
            let other = create_tagged(
                &efs,
                kind,
                &host,
                &format!("neighbour-{kind:?}-{index}"),
                &owned(&[("neighbour", "untouched")]),
            )
            .await;
            let reply = list_tags(&efs, &other.id, "").await;
            assert_eq!(
                as_set(&reported(&reply)),
                owned(&[("neighbour", "untouched")]),
                "case {:?} on {kind:?}: a neighbouring resource keeps its own tag set, got {:?}",
                row.case,
                reply.get("Tags")
            );
        }
    }
}

/// The `Name` the resource's own description reports, read back through the implemented
/// describe for its kind.
async fn described_name(efs: &Emulator, kind: Kind, id: &str) -> serde_json::Value {
    let described = match kind {
        Kind::FileSystem => {
            let reply = efs.describe_file_systems(&format!("FileSystemId={id}")).await;
            reply
                .get("FileSystems")
                .as_array()
                .cloned()
                .unwrap_or_default()
        }
        Kind::AccessPoint => {
            let reply = efs
                .describe_access_points(&format!("AccessPointId={id}"))
                .await;
            reply
                .get("AccessPoints")
                .as_array()
                .cloned()
                .unwrap_or_default()
        }
    };
    described
        .first()
        .map(|description| description["Name"].clone())
        .unwrap_or(serde_json::Value::Null)
}

#[tokio::test]
async fn the_name_tag_a_write_leaves_is_what_the_description_reports() {
    // `FileSystemDescription$Name`: "If the file system has a Name tag, Amazon EFS returns
    // the value in this field." `AccessPointDescription$Name`: "The name of the access
    // point. This is the value of the Name tag."
    let efs = Emulator::new();
    let host = efs.available_file_system("name-tag-host").await;

    for kind in Kind::ALL {
        let resource = create_tagged(&efs, kind, &host, &format!("name-{kind:?}"), &[]).await;
        // Varying the value the write leaves, including a replacement of one already set.
        for name in ["first", "renamed", "", "naïve nom"] {
            let written = tag_resource(
                &efs,
                &resource.id,
                &owned(&[(state::NAME_TAG_KEY, name), ("env", "prod")]),
            )
            .await;
            assert_eq!(
                written.status,
                200,
                "case Name={name:?} on {kind:?}: TagResource expected 200, got {} {:?}",
                written.status,
                written.body
            );

            let listed = list_tags(&efs, &resource.id, "").await;
            assert!(
                reported(&listed).contains(&(state::NAME_TAG_KEY.to_string(), name.to_string())),
                "case Name={name:?} on {kind:?}: the Name tag must be in the reported set, got \
                 {:?}",
                listed.get("Tags")
            );

            let reported_name = described_name(&efs, kind, &resource.id).await;
            assert_eq!(
                reported_name,
                json!(name),
                "case Name={name:?} on {kind:?}: the description reports the Name tag the write \
                 left, got {reported_name:?}"
            );
        }

        // Removing the Name tag leaves the resource without one, so the description
        // reports no Name either.
        let removed = untag_resource(&efs, &resource.id, &[state::NAME_TAG_KEY]).await;
        assert_eq!(
            removed.status,
            200,
            "{kind:?}: UntagResource expected 200, got {} {:?}",
            removed.status,
            removed.body
        );
        let reported_name = described_name(&efs, kind, &resource.id).await;
        assert!(
            reported_name.is_null(),
            "{kind:?}: the resource carries no Name tag after the removal, so the description \
             reports no Name, got {reported_name:?}"
        );
    }
}

/// The set every removal row starts from: the `Name` tag, plain keys, and the key and
/// value forms the modelled `TagKey`/`TagValue` patterns allow.
fn removal_initial() -> Vec<(String, String)> {
    owned(&[
        ("Name", "analytics"),
        ("env", "prod"),
        ("cost:center/eu", "team=platform+core"),
        ("Équipe.de_test", "valeur naïve"),
        ("empty-value", ""),
    ])
}

/// One row of the removal table: the keys one `UntagResource` call names and the set the
/// resource must then report, written out per row rather than computed.
struct RemovalCase {
    case: &'static str,
    removed: Vec<&'static str>,
    expected: Vec<(String, String)>,
}

/// The removals the rule is asserted over: the count varies (one key, several, every
/// one), as does whether a named key is held at all and the form of the key.
fn removal_table() -> Vec<RemovalCase> {
    vec![
        RemovalCase {
            case: "one held key",
            removed: vec!["env"],
            expected: owned(&[
                ("Name", "analytics"),
                ("cost:center/eu", "team=platform+core"),
                ("Équipe.de_test", "valeur naïve"),
                ("empty-value", ""),
            ]),
        },
        RemovalCase {
            case: "several held keys",
            removed: vec!["Name", "empty-value"],
            expected: owned(&[
                ("env", "prod"),
                ("cost:center/eu", "team=platform+core"),
                ("Équipe.de_test", "valeur naïve"),
            ]),
        },
        RemovalCase {
            case: "a key the resource does not carry",
            removed: vec!["absent"],
            expected: removal_initial(),
        },
        RemovalCase {
            case: "a held and an absent key in one call",
            removed: vec!["env", "absent"],
            expected: owned(&[
                ("Name", "analytics"),
                ("cost:center/eu", "team=platform+core"),
                ("Équipe.de_test", "valeur naïve"),
                ("empty-value", ""),
            ]),
        },
        RemovalCase {
            case: "keys whose forms the modelled pattern allows",
            removed: vec!["cost:center/eu", "Équipe.de_test"],
            expected: owned(&[
                ("Name", "analytics"),
                ("env", "prod"),
                ("empty-value", ""),
            ]),
        },
        RemovalCase {
            case: "every key the resource holds",
            removed: vec![
                "Name",
                "env",
                "cost:center/eu",
                "Équipe.de_test",
                "empty-value",
            ],
            expected: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn untag_resource_removes_the_named_keys_and_nothing_else() {
    let efs = Emulator::new();
    let host = efs.available_file_system("untag-host").await;

    for kind in Kind::ALL {
        for (index, row) in removal_table().iter().enumerate() {
            let resource = create_tagged(
                &efs,
                kind,
                &host,
                &format!("untag-{kind:?}-{index}"),
                &removal_initial(),
            )
            .await;

            // Twice, through each form the modelled `ResourceId` pattern admits: a key
            // already gone leaves the set as it is, so the second call changes nothing.
            for form in resource.forms() {
                let removed = untag_resource(&efs, &form, &row.removed).await;
                assert_eq!(
                    removed.status,
                    200,
                    "case {:?} on {kind:?} as {form:?}: UntagResource expected 200, got {} {:?}",
                    row.case,
                    removed.status,
                    removed.body
                );
                assert_eq!(
                    removed.body_len, 0,
                    "case {:?} on {kind:?} as {form:?}: UntagResource models no output, so the \
                     body is empty, got {:?}",
                    row.case, removed.body
                );

                let reply = list_tags(&efs, &resource.id, "").await;
                assert_eq!(
                    reply.status,
                    200,
                    "case {:?} on {kind:?} as {form:?}: ListTagsForResource expected 200, got {} \
                     {:?}",
                    row.case,
                    reply.status,
                    reply.body
                );
                assert_eq!(
                    as_set(&reported(&reply)),
                    as_set(&row.expected),
                    "case {:?} on {kind:?} as {form:?}: only the named keys are removed, got {:?}",
                    row.case,
                    reply.get("Tags")
                );
            }

            // A removed key can be written again, so the removal took the key out of the
            // stored set rather than hiding it from the report.
            let rewritten = tag_resource(&efs, &resource.id, &removal_initial()).await;
            assert_eq!(
                rewritten.status,
                200,
                "case {:?} on {kind:?}: TagResource expected 200, got {} {:?}",
                row.case,
                rewritten.status,
                rewritten.body
            );
            let reply = list_tags(&efs, &resource.id, "").await;
            assert_eq!(
                as_set(&reported(&reply)),
                as_set(&removal_initial()),
                "case {:?} on {kind:?}: a re-written key comes back, got {:?}",
                row.case,
                reply.get("Tags")
            );
        }
    }
}

#[tokio::test]
async fn max_results_pages_the_tag_set_and_next_token_continues_it() {
    let efs = Emulator::new();
    let host = efs.available_file_system("paging-host").await;

    for kind in Kind::ALL {
        let tags = numbered_tags(state::MAX_TAGS_PER_RESOURCE);
        let resource = create_tagged(&efs, kind, &host, &format!("paging-{kind:?}"), &tags).await;

        // The order is taken from the service's OWN unpaginated answer, so no assertion
        // depends on an order the API does not specify.
        let whole = reported(&list_tags(&efs, &resource.id, "").await);
        assert_eq!(
            whole.len(),
            tags.len(),
            "{kind:?}: the default page holds the whole {}-tag set, got {whole:?}",
            tags.len()
        );

        // 1 is the modelled lower bound (`range {min: 1}`), 100 the documented default,
        // and the rows around them straddle both that bound and the set's own size.
        for max_results in [1usize, 2, 7, 49, 50, 51, 100, 1000] {
            let mut collected: Vec<(String, String)> = Vec::new();
            let mut query = format!("MaxResults={max_results}");
            let mut pages = 0usize;
            loop {
                let reply = list_tags(&efs, &resource.id, &query).await;
                assert_eq!(
                    reply.status,
                    200,
                    "case MaxResults={max_results} on {kind:?}: expected 200, got {} {:?}",
                    reply.status,
                    reply.body
                );
                let page = reported(&reply);
                assert!(
                    page.len() <= max_results.min(state::MAX_PAGE_SIZE),
                    "case MaxResults={max_results} on {kind:?}: a page may not exceed the \
                     requested maximum, got {} entries",
                    page.len()
                );
                collected.extend(page.iter().cloned());
                pages += 1;
                assert!(
                    pages <= whole.len() + 1,
                    "case MaxResults={max_results} on {kind:?}: paging did not terminate, got \
                     {pages} pages"
                );
                match reply.get("NextToken").as_str() {
                    Some(token) => {
                        assert!(
                            !page.is_empty(),
                            "case MaxResults={max_results} on {kind:?}: a NextToken was returned \
                             for an empty page, got {token:?}"
                        );
                        query = format!("MaxResults={max_results}&NextToken={token}");
                    }
                    None => break,
                }
            }
            assert_eq!(
                collected,
                whole,
                "case MaxResults={max_results} on {kind:?}: walking the pages must report every \
                 tag exactly once, in the order one unpaginated call reports them, got \
                 {collected:?}"
            );
            let expected_pages = whole
                .len()
                .div_ceil(max_results.min(state::MAX_PAGE_SIZE))
                .max(1);
            assert_eq!(
                pages, expected_pages,
                "case MaxResults={max_results} on {kind:?}: expected {expected_pages} pages, got \
                 {pages}"
            );
        }
    }
}

#[tokio::test]
async fn an_unknown_resource_or_next_token_is_refused_rather_than_answered_empty() {
    let efs = Emulator::new();
    let host = efs.available_file_system("refusal-host").await;

    for kind in Kind::ALL {
        let resource =
            create_tagged(&efs, kind, &host, &format!("refusal-{kind:?}"), &numbered_tags(3)).await;

        // An id of this kind that names nothing: the modelled 404 for that kind, from
        // both the read and the write, never an empty list or a silent write.
        for unknown in kind.unknown_ids() {
            for (operation, reply) in [
                ("ListTagsForResource", list_tags(&efs, &unknown, "").await),
                (
                    "TagResource",
                    tag_resource(&efs, &unknown, &owned(&[("env", "prod")])).await,
                ),
                (
                    "UntagResource",
                    untag_resource(&efs, &unknown, &["env"]).await,
                ),
            ] {
                assert_eq!(
                    reply.status,
                    404,
                    "case {operation} {unknown:?}: expected 404, got {} {:?}",
                    reply.status,
                    reply.body
                );
                assert_eq!(
                    reply.error_type.as_deref(),
                    Some(kind.not_found()),
                    "case {operation} {unknown:?}: unexpected error type, got {:?}",
                    reply.error_type
                );
            }
        }

        // "You can use NextToken in a subsequent request to fetch the next page … if the
        // response payload was paginated", so a token that names no entry of this set
        // cannot have come from one.
        for token in ["not-a-tag-key", "key-99", "0"] {
            let reply = list_tags(&efs, &resource.id, &format!("NextToken={token}")).await;
            assert_eq!(
                reply.status,
                400,
                "case NextToken={token:?} on {kind:?}: expected 400, got {} {:?}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some("BadRequest"),
                "case NextToken={token:?} on {kind:?}: unexpected error type, got {:?}",
                reply.error_type
            );
        }
    }
}

#[tokio::test]
async fn a_write_past_the_quota_or_with_a_repeated_key_is_refused_and_stores_nothing() {
    let efs = Emulator::new();
    let host = efs.available_file_system("quota-host").await;

    for kind in Kind::ALL {
        let held = numbered_tags(state::MAX_TAGS_PER_RESOURCE - 1);
        let resource = create_tagged(&efs, kind, &host, &format!("quota-{kind:?}"), &held).await;

        // Rows either side of the published quota: the write that fills it is accepted,
        // every write past it is refused. A key the resource already holds replaces a
        // value, so it never counts towards the quota however many times it is written.
        let last = numbered_tags(state::MAX_TAGS_PER_RESOURCE)
            [state::MAX_TAGS_PER_RESOURCE - 1..]
            .to_vec();
        let accepted = tag_resource(&efs, &resource.id, &last).await;
        assert_eq!(
            accepted.status,
            200,
            "{kind:?}: the write that fills the {}-tag quota is accepted, got {} {:?}",
            state::MAX_TAGS_PER_RESOURCE,
            accepted.status,
            accepted.body
        );
        let full = as_set(&reported(&list_tags(&efs, &resource.id, "").await));
        assert_eq!(
            full.len(),
            state::MAX_TAGS_PER_RESOURCE,
            "{kind:?}: the resource now holds the whole quota, got {full:?}"
        );

        let refused: Vec<(&str, Vec<(String, String)>)> = vec![
            ("one key past the quota", owned(&[("one-too-many", "x")])),
            (
                "several keys past the quota",
                numbered_tags(state::MAX_TAGS_PER_RESOURCE + 5)
                    [state::MAX_TAGS_PER_RESOURCE..]
                    .to_vec(),
            ),
            (
                "a request list longer than the quota on its own",
                numbered_tags(state::MAX_TAGS_PER_RESOURCE + 1)
                    .iter()
                    .map(|(key, value)| (format!("fresh-{key}"), value.clone()))
                    .collect(),
            ),
            (
                "a request repeating one key",
                owned(&[("dup", "first"), ("dup", "second")]),
            ),
        ];
        for (case, tags) in refused {
            let reply = tag_resource(&efs, &resource.id, &tags).await;
            assert_eq!(
                reply.status,
                400,
                "case {case:?} on {kind:?}: expected 400, got {} {:?}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some("BadRequest"),
                "case {case:?} on {kind:?}: unexpected error type, got {:?}",
                reply.error_type
            );
            assert_eq!(
                as_set(&reported(&list_tags(&efs, &resource.id, "").await)),
                full,
                "case {case:?} on {kind:?}: a refused write stores nothing, got a changed set"
            );
        }

        // A key the resource already holds is a replacement, so re-writing the whole
        // stored set at the quota is still accepted.
        let replayed = tag_resource(
            &efs,
            &resource.id,
            &numbered_tags(state::MAX_TAGS_PER_RESOURCE),
        )
        .await;
        assert_eq!(
            replayed.status,
            200,
            "{kind:?}: re-writing the held keys at the quota is a replacement, got {} {:?}",
            replayed.status,
            replayed.body
        );
    }
}

#[tokio::test]
async fn listing_tags_does_not_change_what_a_later_caller_sees() {
    let efs = Emulator::new();
    let host = efs.available_file_system("pure-read-host").await;

    for kind in Kind::ALL {
        let resource = create_tagged(
            &efs,
            kind,
            &host,
            &format!("pure-read-{kind:?}"),
            &numbered_tags(4),
        )
        .await;
        let first = reported(&list_tags(&efs, &resource.id, "").await);

        // Every read form, including the paginating ones, between the two reads.
        for query in ["MaxResults=1", "MaxResults=2&NextToken=key-01", "MaxResults=100"] {
            let reply = list_tags(&efs, &resource.id, query).await;
            assert_eq!(
                reply.status,
                200,
                "case {query:?} on {kind:?}: expected 200, got {} {:?}",
                reply.status,
                reply.body
            );
        }
        let again = reported(&list_tags(&efs, &resource.id, "").await);
        assert_eq!(
            again, first,
            "{kind:?}: a ListTagsForResource call must not change the tag set a later caller \
             reads, got {again:?}"
        );
    }
}
