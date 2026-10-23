// SPDX-License-Identifier: CC0-1.0

//! The tags of an S3 Files resource, end to end over HTTP: `TagResource`,
//! `ListTagsForResource` and `UntagResource`, each asserted against what the service
//! PERSISTED rather than against what one request carried.
//!
//! The suite reuses `file_system_lifecycle`'s harness, so every case goes through the
//! same `MultiServiceRouter` the emulator boots: routing, the restJson1 wire shapes
//! (`resourceId` as an `@httpLabel`, `tagKeys` and `maxResults` as `@httpQuery`), the
//! modelled response statuses and the modelled error envelopes are all exercised.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. A tag write MERGES into the tag set the named resource already carries: a key it
//!    carries takes the new value in place, keeping its position; a key it does not
//!    carry is appended. Every later read — `ListTagsForResource` and the resource's own
//!    Get — reports that stored set, and reading it does not change it.
//! 2. `resourceId` covers BOTH taggable resources in either form the model allows — the
//!    bare id or the resource's ARN — and every form reaches the same tag set; the tags
//!    of one resource are never the tags of another.
//! 3. An untag removes exactly the keys it names and leaves the rest in order; a key the
//!    resource does not carry is removed by doing nothing, so the operation is
//!    `@idempotent`.
//! 4. A tag write that breaks a rule the model documents — the reserved `aws:` key
//!    namespace, or `TagList`'s maximum of 50 tags on one resource — is refused with the
//!    modelled `ValidationException` (400) and leaves the stored tags untouched.
//! 5. A tagging call naming a resource this service does not hold is the modelled
//!    `ResourceNotFoundException` (404); a `resourceId` no form of the model's pattern
//!    accepts is refused with 400 before any state is touched.
//! 6. `tags` is a `TagList` (`@length(1, 50)`), so a resource carrying no tags reports
//!    the member ABSENT rather than as an empty list.
//! 7. A listing cuts the tag set into pages of at most `maxResults`, whose `nextToken`
//!    resumes exactly where the previous page stopped, so the pages together are the
//!    stored set once each and in order.
//! 8. The resource's `name` is derived from its `Name` tag, so a tag write that sets,
//!    changes or removes that tag moves the name with it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::file_system_lifecycle::{
    call, create_access_point, created_file_system_id, encoded, file_system_arn_prefix, new_router,
    Answer,
};

/// The route all three tagging operations are bound to:
/// `POST|GET|DELETE /resource-tags/{resourceId}`.
fn tags_route(label: &str) -> String {
    format!("/resource-tags/{label}")
}

/// `TagResource`: `tags` rides in the JSON body, `resourceId` in the path.
async fn tag_resource(router: &MultiServiceRouter, label: &str, tags: &[(&str, &str)]) -> Answer {
    let body: Vec<Value> = tags
        .iter()
        .map(|(key, value)| json!({ "key": key, "value": value }))
        .collect();
    call(
        router,
        "POST",
        &tags_route(label),
        Some(json!({ "tags": body })),
    )
    .await
}

/// `ListTagsForResource`. `maxResults` and `nextToken` ride as the `@httpQuery`
/// parameters the model names them with (`MaxResults` / `NextToken`).
async fn list_tags(
    router: &MultiServiceRouter,
    label: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Answer {
    let mut query: Vec<String> = Vec::new();
    if let Some(size) = max_results {
        query.push(format!("MaxResults={size}"));
    }
    if let Some(token) = next_token {
        query.push(format!("NextToken={}", encoded(token)));
    }
    let path = if query.is_empty() {
        tags_route(label)
    } else {
        format!("{}?{}", tags_route(label), query.join("&"))
    };
    call(router, "GET", &path, None).await
}

/// `UntagResource`: `tagKeys` is a list `@httpQuery` parameter, so each key is one
/// repetition of `tagKeys=`.
async fn untag_resource(router: &MultiServiceRouter, label: &str, keys: &[&str]) -> Answer {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", query_encoded(key)))
        .collect();
    let path = if query.is_empty() {
        tags_route(label)
    } else {
        format!("{}?{}", tags_route(label), query.join("&"))
    };
    call(router, "DELETE", &path, None).await
}

/// Percent-encode every byte outside RFC 3986's unreserved set, which is how an SDK puts
/// a tag key into a query parameter: `TagKey`'s pattern
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$` admits spaces, `/`, `:`, `=`, `+` and non-ASCII
/// letters, none of which may ride raw in a URI (smithy_query: shape TagKey).
fn query_encoded(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(*byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The tags an answer reports, in the order it reported them. An absent `tags` member is
/// the empty set — rule 6 — so this reads both the same way.
fn reported_tags(case: &str, answer: &Answer) -> Vec<(String, String)> {
    match answer.field("tags") {
        Value::Null => Vec::new(),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                let key = item["key"].as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: key is @required on Tag, got {item:?}")
                });
                let value = item["value"].as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: value is @required on Tag, got {item:?}")
                });
                (key.to_string(), value.to_string())
            })
            .collect(),
        other => panic!("case {case:?}: expected tags to be a list, got {other:?}"),
    }
}

/// The tag set `label` carries, read through `ListTagsForResource` and asserted to have
/// answered the modelled 200.
async fn stored_tags(
    router: &MultiServiceRouter,
    case: &str,
    label: &str,
) -> Vec<(String, String)> {
    let listed = list_tags(router, label, None, None).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from ListTagsForResource, got {} / {:?}",
        listed.status, listed.body
    );
    reported_tags(case, &listed)
}

/// Apply rule 1's merge to `current` on the test's side, so what the service answered is
/// compared against the rule rather than against a copy of the service's own output.
fn merged(current: &[(String, String)], incoming: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut merged: Vec<(String, String)> = current.to_vec();
    for (key, value) in incoming {
        match merged.iter_mut().find(|(held, _)| held == key) {
            Some(held) => held.1 = (*value).to_string(),
            None => merged.push(((*key).to_string(), (*value).to_string())),
        }
    }
    merged
}

/// A well-formed `fs-` id (`^fs-[0-9a-f]{17,40}$`) that no create ever minted, so it
/// passes the boundary validator and reaches the handler's lookup.
const UNKNOWN_FILE_SYSTEM_ID: &str = "fs-0123456789abcdef0";

/// The same for an access point (`^fsap-[0-9a-f]{17,40}$`).
const UNKNOWN_ACCESS_POINT_ID: &str = "fsap-0123456789abcdef0";

/// One taggable resource: how a case names it, its bare id and its ARN.
struct Resource {
    kind: &'static str,
    id: String,
    arn: String,
}

impl Resource {
    /// The two `resourceId` forms the model accepts for this resource.
    fn labels(&self) -> [(&'static str, String); 2] {
        [
            ("the bare id", self.id.clone()),
            ("the ARN", encoded(&self.arn)),
        ]
    }

    /// The route the resource's own Get is bound to, so a tag write can be proved
    /// through a second reader.
    fn get_route(&self) -> String {
        match self.kind {
            "a file system" => format!("/file-systems/{}", self.id),
            _ => format!("/access-points/{}", self.id),
        }
    }
}

/// A fresh file system and a fresh access point under it, so every rule is exercised on
/// BOTH resources `ResourceId` covers.
async fn resources(router: &MultiServiceRouter, bucket: &str, token: &str) -> Vec<Resource> {
    let file_system_id = created_file_system_id(
        router,
        &format!("arn:aws:s3:::{bucket}"),
        Some(&format!("{token}-fs")),
    )
    .await;
    let file_system_arn = format!("{}{file_system_id}", file_system_arn_prefix());

    let created = create_access_point(
        router,
        &file_system_id,
        json!({ "clientToken": format!("{token}-ap") }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateAccessPoint, got {} / {:?}",
        created.status, created.body
    );
    let access_point_id = created.text("accessPointId");

    vec![
        Resource {
            kind: "a file system",
            id: file_system_id,
            arn: file_system_arn.clone(),
        },
        Resource {
            kind: "an access point",
            arn: format!("{file_system_arn}/access-point/{access_point_id}"),
            id: access_point_id,
        },
    ]
}

/// The tag writes rule 1 is asserted over, applied in order to the same resource. The
/// rows differ along everything the rule generalises over: how many tags one write
/// carries, whether each key is new or already held, the value's emptiness, the
/// characters the key and value carry, and the longest key and value the model admits.
///
/// `TagKey` is `@length(1, 128)` with pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$` and
/// `TagValue` is `@length(0, 256)` with the same pattern made optional, so each row below
/// is inside those constraints (smithy_query: shape TagKey / TagValue).
fn tag_write_table() -> Vec<(&'static str, Vec<(String, String)>)> {
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    let punctuation = vec![
        ("cost:center", "storage"),
        ("path/of/data", "team/one"),
        ("a=b", "c=d"),
        ("plus+one", "plus+two"),
        ("at@sign", "user@example"),
        ("dot.name", "a.b.c"),
        ("under_score", "with_underscore"),
        ("dash-ed", "with-dash"),
        ("a key with spaces", "a value with spaces"),
    ];
    let rows: Vec<(&'static str, Vec<(&str, &str)>)> = vec![
        ("one new tag", vec![("env", "test")]),
        (
            "three new tags in one write",
            vec![("team", "storage"), ("tier", "gold"), ("owner", "ops")],
        ),
        (
            "a key already held, with a new value",
            vec![("tier", "silver")],
        ),
        ("a tag whose value is empty", vec![("retired", "")]),
        (
            "a key already held whose new value is empty",
            vec![("owner", "")],
        ),
        (
            "non-ASCII letters in the key and the value",
            vec![("Abteilung", "Lagerung"), ("équipe", "stockage")],
        ),
        ("every punctuation character the pattern admits", punctuation),
        (
            "the longest key and value the model admits",
            vec![(longest_key.as_str(), longest_value.as_str())],
        ),
        (
            "a write repeating a key it also adds (the last entry lands)",
            vec![("tier", "bronze"), ("tier", "platinum")],
        ),
    ];
    rows.into_iter()
        .map(|(case, tags)| {
            (
                case,
                tags.into_iter()
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect(),
            )
        })
        .collect()
}

/// Rule 1 and rule 2: a write merges into what the resource already carries, both reads
/// report the stored set, and a read changes nothing — on both resources, addressed by
/// both id forms in turn.
#[tokio::test]
async fn a_tag_write_merges_into_the_stored_tag_set() {
    let router = new_router();
    for resource in resources(&router, "s3files-tag-merge", "tag-merge").await {
        let mut expected: Vec<(String, String)> = Vec::new();
        for (index, (row, tags)) in tag_write_table().into_iter().enumerate() {
            // Alternate the `resourceId` form, so neither form is the only one a write
            // is ever addressed by.
            let (form, label) = resource.labels()[index % 2].clone();
            let case = format!("{} tagged by {form}: {row}", resource.kind);
            let borrowed: Vec<(&str, &str)> = tags
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect();

            let written = tag_resource(&router, &label, &borrowed).await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected the modelled 200 from TagResource, got {} / {:?}",
                written.status, written.body
            );

            expected = merged(&expected, &borrowed);
            let listed = stored_tags(&router, &case, &label).await;
            assert_eq!(
                listed, expected,
                "case {case:?}: expected the merged tag set {expected:?}, got {listed:?}"
            );

            // The resource's own Get reads the same record, so it reports the same set.
            let read = call(&router, "GET", &resource.get_route(), None).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected 200 from the resource's Get, got {} / {:?}",
                read.status, read.body
            );
            assert_eq!(
                reported_tags(&case, &read),
                expected,
                "case {case:?}: the resource's own Get must report the stored tags"
            );

            // A read reports, it does not consume: the other id form answers the same.
            let (other_form, other_label) = resource.labels()[(index + 1) % 2].clone();
            let again = stored_tags(&router, &case, &other_label).await;
            assert_eq!(
                again, expected,
                "case {case:?}: reading the tags by {other_form} must answer the same set, \
                 got {again:?}"
            );
        }
    }
}

/// Rule 2's second half: a file system and its access point hold tag sets of their own,
/// so a write to one is invisible to the other.
#[tokio::test]
async fn each_resource_holds_its_own_tag_set() {
    let router = new_router();
    let resources = resources(&router, "s3files-tag-isolation", "tag-iso").await;

    for (index, resource) in resources.iter().enumerate() {
        let case = format!("tag only {}", resource.kind);
        let tagged = tag_resource(
            &router,
            &resource.id,
            &[("owner", resource.kind), ("index", "one")],
        )
        .await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: expected 200 from TagResource, got {} / {:?}",
            tagged.status, tagged.body
        );

        let other = &resources[(index + 1) % resources.len()];
        let other_tags = stored_tags(&router, &case, &other.id).await;
        let expected: Vec<(String, String)> = if index == 0 {
            // The access point has not been tagged yet.
            Vec::new()
        } else {
            vec![
                ("owner".to_string(), resources[0].kind.to_string()),
                ("index".to_string(), "one".to_string()),
            ]
        };
        assert_eq!(
            other_tags, expected,
            "case {case:?}: tagging one resource must not change what {} carries, got {other_tags:?}",
            other.kind
        );
    }

    // And an untag on one leaves the other's identical key alone.
    let dropped = untag_resource(&router, &resources[0].id, &["owner"]).await;
    assert_eq!(
        dropped.status, 200,
        "case \"untag only the file system\": expected 200, got {} / {:?}",
        dropped.status, dropped.body
    );
    let access_point_tags = stored_tags(&router, "untag only the file system", &resources[1].id).await;
    assert_eq!(
        access_point_tags,
        vec![
            ("owner".to_string(), resources[1].kind.to_string()),
            ("index".to_string(), "one".to_string()),
        ],
        "case \"untag only the file system\": the access point keeps its own `owner` tag, \
         got {access_point_tags:?}"
    );
}

/// Rule 3: an untag removes exactly the keys it names, leaves the rest in order, and
/// repeating it is the same call. The rows differ along how many keys are named, whether
/// each is held, and where in the order it sits.
#[tokio::test]
async fn an_untag_removes_exactly_the_named_keys_and_repeats_cleanly() {
    let router = new_router();
    let baseline: Vec<(&str, &str)> = vec![
        ("env", "test"),
        ("team", "storage"),
        ("tier", "gold"),
        ("cost:center", "4711"),
        ("Name", "tagged"),
    ];
    let table: Vec<(&str, Vec<&str>)> = vec![
        ("one key, the first held", vec!["env"]),
        ("one key, the last held", vec!["Name"]),
        ("a key in the middle", vec!["team"]),
        ("two held keys at once", vec!["tier", "cost:center"]),
        ("a key the resource does not carry", vec!["absent"]),
        (
            "a mix of held and absent keys",
            vec!["env", "absent", "tier"],
        ),
        (
            "every key it carries, and one it does not",
            vec!["env", "team", "tier", "cost:center", "Name", "absent"],
        ),
    ];

    for resource in resources(&router, "s3files-tag-untag", "tag-untag").await {
        for (index, (row, keys)) in table.iter().enumerate() {
            let (form, label) = resource.labels()[index % 2].clone();
            let case = format!("{} untagged by {form}: {row}", resource.kind);

            // Re-establish the baseline so each row starts from the same tag set.
            let restored = tag_resource(&router, &label, &baseline).await;
            assert_eq!(
                restored.status, 200,
                "case {case:?}: expected 200 restoring the baseline, got {} / {:?}",
                restored.status, restored.body
            );
            let before = stored_tags(&router, &case, &label).await;
            let expected: Vec<(String, String)> = before
                .iter()
                .filter(|(key, _)| !keys.contains(&key.as_str()))
                .cloned()
                .collect();

            // `@idempotent`: the same call three times leaves the same tag set.
            for attempt in 1..=3 {
                let answer = untag_resource(&router, &label, keys).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} attempt {attempt}: expected the modelled 200 from \
                     UntagResource, got {} / {:?}",
                    answer.status, answer.body
                );
                let after = stored_tags(&router, &case, &label).await;
                assert_eq!(
                    after, expected,
                    "case {case:?} attempt {attempt}: expected the remaining tags \
                     {expected:?}, got {after:?}"
                );
            }
        }
    }
}

/// Rule 4: a write breaking a documented rule is refused and changes nothing. The rows
/// are the reserved key namespace (on its own and mixed into a valid write) and
/// `TagList`'s own maximum of 50 tags on one resource, tested AT the bound and one past
/// it.
#[tokio::test]
async fn a_tag_write_breaking_a_documented_rule_is_refused() {
    let router = new_router();
    let max_tags = service_com_amazonaws_s3files_s3files::state::MAX_TAGS_PER_RESOURCE;

    for resource in resources(&router, "s3files-tag-rules", "tag-rules").await {
        let label = resource.id.clone();
        // A set that IS acceptable, so every refusal below can be shown to have changed
        // nothing.
        let accepted: Vec<(&str, &str)> = vec![("env", "test"), ("team", "storage")];
        let written = tag_resource(&router, &label, &accepted).await;
        assert_eq!(
            written.status, 200,
            "{}: expected the baseline write to be accepted, got {} / {:?}",
            resource.kind, written.status, written.body
        );
        let before = stored_tags(&router, "baseline", &label).await;

        // The `aws:` namespace is reserved (model doc on Tag$key: "The key can't start
        // with `aws:`"), whichever tag in the write carries it.
        let reserved: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("the reserved prefix alone", vec![("aws:", "x")]),
            (
                "a reserved service key",
                vec![("aws:cloudformation:stack-name", "stack")],
            ),
            (
                "a reserved key mixed into an otherwise valid write",
                vec![("valid", "yes"), ("aws:createdBy", "someone")],
            ),
        ];
        for (row, tags) in reserved {
            let case = format!("{}: {row}", resource.kind);
            let refused = tag_resource(&router, &label, &tags).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected 400, got {} / {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type, "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {:?} / {:?}",
                refused.error_type, refused.body
            );
            let after = stored_tags(&router, &case, &label).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused write must leave the stored tags as they were, \
                 got {after:?}"
            );
        }

        // Fill the resource to exactly the bound: the write that reaches 50 tags is
        // accepted.
        let keys: Vec<String> = (before.len()..max_tags)
            .map(|index| format!("key{index:02}"))
            .collect();
        let filling: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "bulk")).collect();
        let filled = tag_resource(&router, &label, &filling).await;
        assert_eq!(
            filled.status, 200,
            "case \"{} filled to exactly {max_tags} tags\": expected 200, got {} / {:?}",
            resource.kind, filled.status, filled.body
        );
        let at_bound = stored_tags(&router, "at the bound", &label).await;
        assert_eq!(
            at_bound.len(),
            max_tags,
            "case \"{} filled to exactly {max_tags} tags\": expected {max_tags} stored tags, \
             got {}",
            resource.kind,
            at_bound.len()
        );

        // At the bound, a write that only REPLACES a held key's value still fits.
        let replaced = tag_resource(&router, &label, &[(keys[0].as_str(), "replaced")]).await;
        assert_eq!(
            replaced.status, 200,
            "case \"{}: replacing a value at the bound\": expected 200, got {} / {:?}",
            resource.kind, replaced.status, replaced.body
        );

        // One past the bound: a write adding a 51st key is refused and changes nothing.
        let at_bound = stored_tags(&router, "at the bound", &label).await;
        let past = tag_resource(&router, &label, &[("one-too-many", "x")]).await;
        assert_eq!(
            past.status, 400,
            "case \"{}: one tag past the bound\": expected 400, got {} / {:?}",
            resource.kind, past.status, past.body
        );
        assert_eq!(
            past.error_type, "ValidationException",
            "case \"{}: one tag past the bound\": expected the modelled ValidationException, \
             got {:?} / {:?}",
            resource.kind, past.error_type, past.body
        );
        let after = stored_tags(&router, "one past the bound", &label).await;
        assert_eq!(
            after, at_bound,
            "case \"{}: one tag past the bound\": a refused write changes nothing, got {} tags",
            resource.kind,
            after.len()
        );
    }
}

/// Rule 5: a resource this service does not hold is a 404 on every tagging operation, and
/// a `resourceId` outside the model's pattern never reaches the store at all.
#[tokio::test]
async fn an_unknown_or_unacceptable_resource_id_is_refused() {
    let router = new_router();
    // Exists, so the refusals below cannot be explained by an empty store.
    let held = resources(&router, "s3files-tag-missing", "tag-missing").await;
    let unknown_arn = format!("{}{UNKNOWN_FILE_SYSTEM_ID}", file_system_arn_prefix());
    let unknown_access_point_arn = format!("{unknown_arn}/access-point/{UNKNOWN_ACCESS_POINT_ID}");

    let unknown: Vec<(&str, String)> = vec![
        ("an unknown file system id", UNKNOWN_FILE_SYSTEM_ID.to_string()),
        ("an unknown file system ARN", encoded(&unknown_arn)),
        (
            "an unknown access point id",
            UNKNOWN_ACCESS_POINT_ID.to_string(),
        ),
        (
            "an unknown access point ARN",
            encoded(&unknown_access_point_arn),
        ),
        (
            "an unknown access point under a held file system",
            encoded(&format!(
                "{}/access-point/{UNKNOWN_ACCESS_POINT_ID}",
                held[0].arn
            )),
        ),
    ];
    for (case, label) in unknown {
        for (operation, answer) in [
            ("TagResource", tag_resource(&router, &label, &[("env", "test")]).await),
            ("ListTagsForResource", list_tags(&router, &label, None, None).await),
            ("UntagResource", untag_resource(&router, &label, &["env"]).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?} on {operation}: expected 404, got {} / {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, "ResourceNotFoundException",
                "case {case:?} on {operation}: expected the modelled \
                 ResourceNotFoundException, got {:?} / {:?}",
                answer.error_type, answer.body
            );
        }
    }

    // Values no `ResourceId` form accepts: refused with 400, having touched nothing.
    let unacceptable: Vec<(&str, &str)> = vec![
        ("an id that is too short", "fs-0a1b2c3d"),
        ("an id that is not hex", "fsap-zzzzzzzzzzzzzzzzz"),
        ("an id of another service", "vol-0123456789abcdef0"),
        ("a bare word", "resource"),
    ];
    for (case, label) in unacceptable {
        let refused = tag_resource(&router, label, &[("env", "test")]).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} / {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {:?} / {:?}",
            refused.error_type, refused.body
        );
    }

    // A tag write still has to name at least one tag: `TagList` is `@length(1, 50)`, so
    // the boundary validator refuses a request carrying none.
    let no_tags = call(
        &router,
        "POST",
        &tags_route(&held[0].id),
        Some(json!({ "tags": [] })),
    )
    .await;
    assert_eq!(
        no_tags.status, 400,
        "case \"a write carrying no tags\": expected 400, got {} / {:?}",
        no_tags.status, no_tags.body
    );
}

/// Rule 6: a resource carrying no tags reports `tags` ABSENT — before any write, and
/// again once every tag has been removed.
#[tokio::test]
async fn a_resource_with_no_tags_reports_no_tags_member() {
    let router = new_router();
    let untagged = created_file_system_id(
        &router,
        "arn:aws:s3:::s3files-tag-empty",
        Some("tag-empty-fs"),
    )
    .await;
    let created = create_access_point(
        &router,
        &untagged,
        json!({ "clientToken": "tag-empty-ap" }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateAccessPoint, got {} / {:?}",
        created.status, created.body
    );

    for (case, label) in [
        ("a file system no write has tagged", untagged.clone()),
        (
            "an access point no write has tagged",
            created.text("accessPointId"),
        ),
    ] {
        let listed = list_tags(&router, &label, None, None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200, got {} / {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            *listed.field("tags"),
            Value::Null,
            "case {case:?}: TagList is @length(1, 50), so no tags means an absent member, \
             got {:?}",
            listed.field("tags")
        );
        assert_eq!(
            *listed.field("nextToken"),
            Value::Null,
            "case {case:?}: a last page carries no nextToken, got {:?}",
            listed.field("nextToken")
        );

        // Tagged, then stripped: the member comes back and goes absent again.
        let tags: Vec<(&str, &str)> = vec![("env", "test"), ("team", "storage")];
        assert_eq!(
            tag_resource(&router, &label, &tags).await.status,
            200,
            "case {case:?}: expected the write to be accepted"
        );
        assert_eq!(
            stored_tags(&router, case, &label).await.len(),
            2,
            "case {case:?}: both tags must be stored"
        );
        assert_eq!(
            untag_resource(&router, &label, &["env", "team"])
                .await
                .status,
            200,
            "case {case:?}: expected the untag to be accepted"
        );
        let stripped = list_tags(&router, &label, None, None).await;
        assert_eq!(
            *stripped.field("tags"),
            Value::Null,
            "case {case:?}: a resource stripped of every tag reports an absent member, \
             got {:?}",
            stripped.field("tags")
        );
    }
}

/// Rule 7: the pages of a listing partition the stored tag set, in order, once each —
/// over every page size the model admits, including each published bound; and a
/// `maxResults` outside `@range(1, 50)` is refused.
#[tokio::test]
async fn tag_pages_partition_the_stored_tag_set() {
    let router = new_router();
    let resource = &resources(&router, "s3files-tag-pages", "tag-pages").await[0];
    let keys: Vec<String> = (0..23).map(|index| format!("key{index:02}")).collect();
    let tags: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "paged")).collect();
    assert_eq!(
        tag_resource(&router, &resource.id, &tags).await.status,
        200,
        "expected the 23-tag write to be accepted"
    );
    let whole = stored_tags(&router, "the whole set", &resource.id).await;
    assert_eq!(
        whole.len(),
        23,
        "expected all 23 tags to be stored, got {whole:?}"
    );

    // `ListTagsForResourceRequest$maxResults` is `@range(1, 50)`: both bounds, a size in
    // between, and no size at all.
    for size in [None, Some(1), Some(2), Some(7), Some(23), Some(50)] {
        let case = format!("maxResults={size:?}");
        let mut seen: Vec<(String, String)> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let listed = list_tags(&router, &resource.id, size, next_token.as_deref()).await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}: expected 200, got {} / {:?}",
                listed.status, listed.body
            );
            let page = reported_tags(&case, &listed);
            if let Some(limit) = size {
                assert!(
                    page.len() as i64 <= limit,
                    "case {case:?}: a page holds at most {limit} tags, got {}",
                    page.len()
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: the listing did not terminate after {pages} pages"
            );
            match listed.field("nextToken").as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, whole,
            "case {case:?}: the pages together must report the stored set once each and in \
             order, got {seen:?}"
        );
    }

    // One past each published bound, and a cursor this service never issued.
    for (case, size) in [("below the range", 0), ("above the range", 51)] {
        let refused = list_tags(&router, &resource.id, Some(size), None).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 for maxResults={size}, got {} / {:?}",
            refused.status, refused.body
        );
    }
    let forged = list_tags(&router, &resource.id, None, Some("not-a-token")).await;
    assert_eq!(
        forged.status, 400,
        "case \"a forged nextToken\": expected 400, got {} / {:?}",
        forged.status, forged.body
    );
    assert_eq!(
        forged.error_type, "ValidationException",
        "case \"a forged nextToken\": expected the modelled ValidationException, got {:?}",
        forged.error_type
    );
}

/// One row of rule 8's table: how the case is named, the tags the write carries, and
/// the name that must follow it (`None` for a resource with no name).
type NameRow = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Option<&'static str>,
);

/// Rule 8: the resource's `name` is derived from its `Name` tag, so a tag write moves it.
#[tokio::test]
async fn the_name_follows_the_name_tag() {
    let router = new_router();
    for resource in resources(&router, "s3files-tag-name", "tag-name").await {
        // Rows: what the write leaves the `Name` tag as, and the name that follows.
        let table: Vec<NameRow> = vec![
            ("no Name tag yet", vec![("env", "test")], None),
            (
                "a Name tag is added",
                vec![("Name", "first")],
                Some("first"),
            ),
            (
                "the Name tag is changed",
                vec![("Name", "second")],
                Some("second"),
            ),
            (
                "another tag is added beside it",
                vec![("team", "storage")],
                Some("second"),
            ),
            (
                "the Name tag is emptied",
                vec![("Name", "")],
                Some(""),
            ),
        ];
        for (row, tags, expected) in table {
            let case = format!("{}: {row}", resource.kind);
            let written = tag_resource(&router, &resource.id, &tags).await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected 200 from TagResource, got {} / {:?}",
                written.status, written.body
            );
            let read = call(&router, "GET", &resource.get_route(), None).await;
            let name = read.field("name");
            match expected {
                Some(value) => assert_eq!(
                    name.as_str(),
                    Some(value),
                    "case {case:?}: the name is derived from the Name tag, got {name:?}"
                ),
                None => assert_eq!(
                    *name,
                    Value::Null,
                    "case {case:?}: with no Name tag the resource has no name, got {name:?}"
                ),
            }
        }

        // Removing the tag removes the name it backed.
        let dropped = untag_resource(&router, &resource.id, &["Name"]).await;
        assert_eq!(
            dropped.status, 200,
            "case \"{}: the Name tag is removed\": expected 200, got {} / {:?}",
            resource.kind, dropped.status, dropped.body
        );
        let read = call(&router, "GET", &resource.get_route(), None).await;
        assert_eq!(
            *read.field("name"),
            Value::Null,
            "case \"{}: the Name tag is removed\": the name goes with it, got {:?}",
            resource.kind,
            read.field("name")
        );
    }
}
