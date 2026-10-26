// SPDX-License-Identifier: CC0-1.0

//! The tagging surface over the wire: `TagResource`, `UntagResource` and the
//! `ListTagsForResource` report they change.
//!
//! The rule under test: a resource's tags are a MAP keyed by tag key, stored against the
//! resource itself, so `TagResource` MERGES the pairs it is given into what the resource
//! already carries (a key the resource has takes the new value in place, a key it does
//! not is appended, every other tag is left alone) and `UntagResource` removes exactly
//! the keys it names and leaves the rest; both are addressed by resource ARN, so an ARN
//! resolving to no resource this account holds is refused with the modeled
//! `ResourceNotFoundException` (404), and `TagResource` is refused with the modeled
//! `LimitExceededException` (429) when the tags it would LEAVE on the resource exceed the
//! per-resource limit, with nothing written.
//!
//! The rule holds for every resource kind this service persists and reports tags for — a
//! connection, a host and a repository link — so every table below is driven across all
//! three, and the expected tag set of each row is computed as a map in the test rather
//! than read off the implementation.
//!
//! Every request goes through the real `MultiServiceRouter` dispatch, reusing the
//! `connection_lifecycle` suite's helpers, so these tests exercise the same path a signed
//! SDK request takes.

use std::collections::BTreeMap;
use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codeconnections_codeconnections_20231201::state::MAX_TAGS_PER_RESOURCE;

use super::connection_lifecycle::{REGION, call, router};
use super::git_sync::error_code;

/// The published bounds of the modeled `Tag` members, from
/// `smithy_query(query="shape", shape="Tag")`: `Key` is a `TagKey` (`length 1..128`),
/// `Value` a `TagValue` (`length 0..256`).
const TAG_KEY_MAX_LEN: usize = 128;
const TAG_VALUE_MAX_LEN: usize = 256;

/// A kind of resource this service persists, mints an ARN for and reports tags for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `connection/<id>` — created by `CreateConnection`.
    Connection,
    /// `host/<id>` — created by `CreateHost`.
    Host,
    /// `repository-link/<id>` — created by `CreateRepositoryLink`.
    RepositoryLink,
}

/// Every kind the tagging surface resolves an ARN to, so a rule asserted here is
/// asserted for all of them rather than for connections alone.
const KINDS: &[Kind] = &[Kind::Connection, Kind::Host, Kind::RepositoryLink];

/// A tag pair as a test row carries it.
type Pair = (String, String);

/// Build a tag pair from borrowed text.
fn pair(key: &str, value: &str) -> Pair {
    (key.to_string(), value.to_string())
}

/// Render tag pairs as the wire `TagList`.
fn tag_list(tags: &[Pair]) -> serde_json::Value {
    serde_json::Value::Array(
        tags.iter()
            .map(|(key, value)| serde_json::json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// The tag pairs as a MAP, applied left to right — the independent statement of the
/// rule a row's expectation is computed with: one key holds one value, and the last
/// entry naming a key wins.
fn as_map(tags: &[Pair]) -> BTreeMap<String, String> {
    tags.iter().cloned().collect()
}

/// Create one connection carrying `tags`, and return its `ConnectionArn`.
///
/// `slug` distinguishes the resources of one test from each other; a `ConnectionName` is
/// `length 1..32`, so it stays short.
async fn create_connection(
    router: &MultiServiceRouter,
    label: &str,
    slug: &str,
    tags: &[Pair],
) -> String {
    let (status, body) = call(
        router,
        "CreateConnection",
        serde_json::json!({
            "ConnectionName": format!("c-{slug}"),
            "ProviderType": "GitHub",
            "Tags": tag_list(tags),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected 200 from CreateConnection, got {status} {body}"
    );
    body["ConnectionArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {label:?}: CreateConnection returned no ConnectionArn: {body}")
        })
        .to_string()
}

/// Create one resource of `kind` carrying `tags`, and return its ARN.
async fn create_resource(
    router: &MultiServiceRouter,
    kind: Kind,
    label: &str,
    slug: &str,
    tags: &[Pair],
) -> String {
    match kind {
        Kind::Connection => create_connection(router, label, slug, tags).await,
        Kind::Host => {
            let (status, body) = call(
                router,
                "CreateHost",
                serde_json::json!({
                    "Name": format!("h-{slug}"),
                    "ProviderType": "GitHubEnterpriseServer",
                    "ProviderEndpoint": format!("https://h-{slug}.example.internal"),
                    "Tags": tag_list(tags),
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200 from CreateHost, got {status} {body}"
            );
            body["HostArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: CreateHost returned no HostArn: {body}"))
                .to_string()
        }
        Kind::RepositoryLink => {
            // A repository link is an association with a connection, so one is minted
            // first; its own tags stay empty, the link carries the row's.
            let connection_arn =
                create_connection(router, label, &format!("l{slug}"), &[]).await;
            let (status, body) = call(
                router,
                "CreateRepositoryLink",
                serde_json::json!({
                    "ConnectionArn": connection_arn,
                    "OwnerId": "octo-org",
                    "RepositoryName": format!("repo-{slug}"),
                    "Tags": tag_list(tags),
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200 from CreateRepositoryLink, got {status} {body}"
            );
            body["RepositoryLinkInfo"]["RepositoryLinkArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {label:?}: CreateRepositoryLink returned no RepositoryLinkArn: {body}"
                    )
                })
                .to_string()
        }
    }
}

/// The tags `ListTagsForResource` reports for `arn`, in the order it reports them.
async fn stored_tags(router: &MultiServiceRouter, label: &str, arn: &str) -> Vec<Pair> {
    let (status, body) = call(
        router,
        "ListTagsForResource",
        serde_json::json!({ "ResourceArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected 200 from ListTagsForResource, got {status} {body}"
    );
    body["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: ListTagsForResource reported no Tags: {body}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// One `TagResource` row: the tags the resource is created with, and the batch one call
/// applies to it.
struct MergeCase {
    /// What a failing assertion names this row by.
    label: &'static str,
    /// The tags the resource already carries when the call is made.
    initial: Vec<Pair>,
    /// The `Tags` the `TagResource` call supplies.
    apply: Vec<Pair>,
}

/// The table: batches differing in size (none, one, several), in whether they overwrite
/// a key the resource carries or add one it does not, in value length at the published
/// bounds, and in whether one batch names a key twice.
fn merge_cases() -> Vec<MergeCase> {
    vec![
        MergeCase {
            label: "one key added to an untagged resource",
            initial: Vec::new(),
            apply: vec![pair("team", "platform")],
        },
        MergeCase {
            label: "several keys added beside a key the resource carries",
            initial: vec![pair("env", "prod")],
            apply: vec![pair("a", "1"), pair("b", "2"), pair("c", "3")],
        },
        MergeCase {
            label: "the value of a carried key overwritten",
            initial: vec![pair("env", "prod"), pair("owner", "payments")],
            apply: vec![pair("env", "staging")],
        },
        MergeCase {
            label: "a batch that both overwrites and adds",
            initial: vec![pair("a", "1"), pair("b", "2")],
            apply: vec![pair("b", "20"), pair("c", "3")],
        },
        MergeCase {
            label: "an empty batch changes nothing",
            initial: vec![pair("env", "prod")],
            apply: Vec::new(),
        },
        MergeCase {
            label: "the pair the resource already carries is idempotent",
            initial: vec![pair("env", "prod")],
            apply: vec![pair("env", "prod")],
        },
        MergeCase {
            label: "a value at its published minimum length",
            initial: vec![pair("env", "prod")],
            apply: vec![pair("empty", "")],
        },
        MergeCase {
            label: "a key and a value at their published maximum lengths",
            initial: Vec::new(),
            apply: vec![pair(
                &"k".repeat(TAG_KEY_MAX_LEN),
                &"v".repeat(TAG_VALUE_MAX_LEN),
            )],
        },
        MergeCase {
            label: "one batch naming a key twice leaves its last value",
            initial: Vec::new(),
            apply: vec![pair("dup", "first"), pair("dup", "last")],
        },
    ]
}

/// `TagResource` merges its batch into the resource's stored tags, for every resource
/// kind: the merged map is what `ListTagsForResource` reports, and a key the resource
/// already carried keeps its position.
#[tokio::test]
async fn tag_resource_merges_its_batch_into_the_stored_tags() {
    let router = router();

    for (index, case) in merge_cases().iter().enumerate() {
        for kind in KINDS {
            let label = format!("{} ({kind:?})", case.label);
            let arn = create_resource(
                &router,
                *kind,
                &label,
                &format!("m{index}{}", kind_slug(*kind)),
                &case.initial,
            )
            .await;

            let before = stored_tags(&router, &label, &arn).await;
            let (status, body) = call(
                &router,
                "TagResource",
                serde_json::json!({ "ResourceArn": arn, "Tags": tag_list(&case.apply) }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200 from TagResource, got {status} {body}"
            );

            let after = stored_tags(&router, &label, &arn).await;
            let mut expected = as_map(&case.initial);
            expected.extend(as_map(&case.apply));
            assert_eq!(
                as_map(&after),
                expected,
                "case {label:?}: expected the merged tag map, got {after:?}"
            );
            assert_eq!(
                after.len(),
                expected.len(),
                "case {label:?}: expected one entry per key, got {after:?}"
            );
            // A key the resource already carried is updated IN PLACE, so the keys it
            // held keep their order and any new key follows them.
            let kept: Vec<&String> = after
                .iter()
                .take(before.len())
                .map(|(key, _)| key)
                .collect();
            let held: Vec<&String> = before.iter().map(|(key, _)| key).collect();
            assert_eq!(
                kept, held,
                "case {label:?}: expected the carried keys to keep their order, got {after:?}"
            );
        }
    }
}

/// One `UntagResource` row: the tags the resource carries, and the keys one call removes.
struct RemovalCase {
    /// What a failing assertion names this row by.
    label: &'static str,
    /// The tags the resource carries when the call is made.
    initial: Vec<Pair>,
    /// The `TagKeys` the `UntagResource` call supplies.
    remove: Vec<String>,
}

/// The table: key lists differing in size (none, one, several, every key the resource
/// carries) and in whether they name keys the resource does not carry.
fn removal_cases() -> Vec<RemovalCase> {
    vec![
        RemovalCase {
            label: "one of several keys removed",
            initial: vec![pair("env", "prod"), pair("team", "platform")],
            remove: vec!["env".to_string()],
        },
        RemovalCase {
            label: "every key removed",
            initial: vec![pair("a", "1"), pair("b", "2"), pair("c", "3")],
            remove: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        },
        RemovalCase {
            label: "a key the resource does not carry is nothing to do",
            initial: vec![pair("env", "prod")],
            remove: vec!["absent".to_string()],
        },
        RemovalCase {
            label: "carried and uncarried keys in one call",
            initial: vec![pair("env", "prod"), pair("team", "platform")],
            remove: vec!["team".to_string(), "absent".to_string()],
        },
        RemovalCase {
            label: "an empty key list changes nothing",
            initial: vec![pair("env", "prod")],
            remove: Vec::new(),
        },
        RemovalCase {
            label: "a key removed from an untagged resource",
            initial: Vec::new(),
            remove: vec!["env".to_string()],
        },
        RemovalCase {
            label: "a key at its published maximum length",
            initial: vec![pair(&"k".repeat(TAG_KEY_MAX_LEN), "held")],
            remove: vec!["k".repeat(TAG_KEY_MAX_LEN)],
        },
    ]
}

/// `UntagResource` removes exactly the keys it names, for every resource kind, and
/// leaves every other tag — a key the resource does not carry is answered 200 with
/// nothing changed, and repeating the call changes nothing further.
#[tokio::test]
async fn untag_resource_removes_exactly_the_keys_it_names() {
    let router = router();

    for (index, case) in removal_cases().iter().enumerate() {
        for kind in KINDS {
            let label = format!("{} ({kind:?})", case.label);
            let arn = create_resource(
                &router,
                *kind,
                &label,
                &format!("u{index}{}", kind_slug(*kind)),
                &case.initial,
            )
            .await;

            let (status, body) = call(
                &router,
                "UntagResource",
                serde_json::json!({ "ResourceArn": arn, "TagKeys": case.remove }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200 from UntagResource, got {status} {body}"
            );

            let mut expected = as_map(&case.initial);
            for key in &case.remove {
                expected.remove(key);
            }
            let after = stored_tags(&router, &label, &arn).await;
            assert_eq!(
                as_map(&after),
                expected,
                "case {label:?}: expected the keys it named to be gone and the rest kept, got {after:?}"
            );
            assert_eq!(
                after.len(),
                expected.len(),
                "case {label:?}: expected one entry per remaining key, got {after:?}"
            );

            // Removal states the POST-STATE of those keys, so a second identical call
            // is admissible and changes nothing.
            let (status, body) = call(
                &router,
                "UntagResource",
                serde_json::json!({ "ResourceArn": arn, "TagKeys": case.remove }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200 from a repeated UntagResource, got {status} {body}"
            );
            let repeated = stored_tags(&router, &label, &arn).await;
            assert_eq!(
                repeated, after,
                "case {label:?}: a repeated removal changes nothing, got {repeated:?}"
            );
        }
    }
}

/// A short, kind-specific slug so the resources of one row do not collide by name.
fn kind_slug(kind: Kind) -> &'static str {
    match kind {
        Kind::Connection => "c",
        Kind::Host => "h",
        Kind::RepositoryLink => "r",
    }
}

/// `TagResource` is held to the per-resource tag limit, judged on what the call would
/// LEAVE on the resource: a resource at the limit still accepts new values for the keys
/// it carries, a call that would push it past the limit is refused 429
/// `LimitExceededException`, and the refused call writes nothing.
#[tokio::test]
async fn tag_resource_refuses_what_would_exceed_the_per_resource_tag_limit() {
    let router = router();
    let at_limit: Vec<Pair> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| pair(&format!("key-{index}"), &format!("value-{index}")))
        .collect();

    for kind in KINDS {
        let label = format!("tag limit ({kind:?})");
        let arn = create_resource(&router, *kind, &label, kind_slug(*kind), &[]).await;

        // A single batch filling the resource to the limit is admissible.
        let (status, body) = call(
            &router,
            "TagResource",
            serde_json::json!({ "ResourceArn": arn, "Tags": tag_list(&at_limit) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 tagging up to the limit, got {status} {body}"
        );
        let after = stored_tags(&router, &label, &arn).await;
        assert_eq!(
            after.len(),
            MAX_TAGS_PER_RESOURCE,
            "case {label:?}: expected the resource to carry the whole batch, got {} entries",
            after.len()
        );

        // At the limit, re-applying the SAME keys with new values leaves the same number
        // of tags, so it is admissible.
        let updated: Vec<Pair> = at_limit
            .iter()
            .map(|(key, _)| pair(key, "updated"))
            .collect();
        let (status, body) = call(
            &router,
            "TagResource",
            serde_json::json!({ "ResourceArn": arn, "Tags": tag_list(&updated) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 re-applying the carried keys, got {status} {body}"
        );
        let after = stored_tags(&router, &label, &arn).await;
        assert_eq!(
            as_map(&after),
            as_map(&updated),
            "case {label:?}: expected every carried key updated, got {after:?}"
        );

        // One key past the limit is refused, and the refusal writes nothing.
        let (status, body) = call(
            &router,
            "TagResource",
            serde_json::json!({
                "ResourceArn": arn,
                "Tags": tag_list(&[pair("one-too-many", "x")]),
            }),
        )
        .await;
        assert_eq!(
            status, 429,
            "case {label:?}: expected 429 past the tag limit, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "LimitExceededException",
            "case {label:?}: expected LimitExceededException, got {body}"
        );
        let message = body["Message"]
            .as_str()
            .or_else(|| body["message"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains(&MAX_TAGS_PER_RESOURCE.to_string()),
            "case {label:?}: the refusal states the limit, got {message:?}"
        );
        let unchanged = stored_tags(&router, &label, &arn).await;
        assert_eq!(
            as_map(&unchanged),
            as_map(&updated),
            "case {label:?}: a refused TagResource writes nothing, got {unchanged:?}"
        );
    }

    // The same bound holds for a single batch applied to an untagged resource.
    let arn = create_resource(&router, Kind::Connection, "one batch past the limit", "b", &[]).await;
    let past_limit: Vec<Pair> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|index| pair(&format!("key-{index}"), "x"))
        .collect();
    let (status, body) = call(
        &router,
        "TagResource",
        serde_json::json!({ "ResourceArn": arn, "Tags": tag_list(&past_limit) }),
    )
    .await;
    assert_eq!(
        status, 429,
        "case one batch past the limit: expected 429, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceededException",
        "case one batch past the limit: expected LimitExceededException, got {body}"
    );
    let unchanged = stored_tags(&router, "one batch past the limit", &arn).await;
    assert!(
        unchanged.is_empty(),
        "case one batch past the limit: a refused TagResource writes nothing, got {unchanged:?}"
    );
}

/// An ARN that resolves to no resource this store holds, and the reader that reports
/// that resource kind's absence.
struct AbsentArn {
    /// What a failing assertion names this row by.
    label: &'static str,
    /// The ARN the tagging surface is called with.
    arn: String,
    /// The operation that addresses this kind by ARN and the member it takes it in —
    /// `None` for a repository link, which no operation addresses by ARN.
    reader: Option<(&'static str, &'static str)>,
}

/// An ARN of each kind that resolves to no resource this store holds, paired with the
/// reader that reports that kind's absence.
fn unresolvable_arns() -> Vec<AbsentArn> {
    let account = engine::emulator_account_id();
    vec![
        AbsentArn {
            label: "a connection ARN this store never minted",
            arn: format!(
                "arn:aws:codeconnections:{REGION}:{account}:connection/11111111-2222-3333-4444-555555555555"
            ),
            reader: Some(("GetConnection", "ConnectionArn")),
        },
        AbsentArn {
            label: "a host ARN this store never minted",
            arn: format!(
                "arn:aws:codeconnections:{REGION}:{account}:host/66666666-7777-8888-9999-aaaaaaaaaaaa"
            ),
            reader: Some(("GetHost", "HostArn")),
        },
        AbsentArn {
            label: "a repository-link ARN this store never minted",
            arn: format!(
                "arn:aws:codeconnections:{REGION}:{account}:repository-link/bbbbbbbb-cccc-dddd-eeee-ffffffffffff"
            ),
            reader: None,
        },
    ]
}

/// Both writers of the tagging surface refuse an ARN that resolves to no resource with
/// the modeled `ResourceNotFoundException` (404) — in the wording the reader of that
/// resource kind answers, where one addresses the kind by ARN.
#[tokio::test]
async fn the_tagging_writers_refuse_an_arn_naming_no_resource() {
    let router = router();

    for AbsentArn { label, arn, reader } in unresolvable_arns() {
        let requests = [
            (
                "TagResource",
                serde_json::json!({
                    "ResourceArn": arn,
                    "Tags": tag_list(&[pair("env", "prod")]),
                }),
            ),
            (
                "UntagResource",
                serde_json::json!({ "ResourceArn": arn, "TagKeys": ["env"] }),
            ),
        ];

        for (operation, body) in requests {
            let (status, got) = call(&router, operation, body).await;
            assert_eq!(
                status, 404,
                "case {label:?}/{operation}: expected 404, got {status} {got}"
            );
            assert_eq!(
                error_code(&got),
                "ResourceNotFoundException",
                "case {label:?}/{operation}: expected ResourceNotFoundException, got {got}"
            );
            let message = got["Message"]
                .as_str()
                .or_else(|| got["message"].as_str())
                .unwrap_or_default()
                .to_string();

            if let Some((reader_operation, member)) = reader {
                let (reader_status, reader_body) = call(
                    &router,
                    reader_operation,
                    serde_json::json!({ member: arn }),
                )
                .await;
                assert_eq!(
                    reader_status, 404,
                    "case {label:?}: expected 404 from {reader_operation}, got {reader_status} {reader_body}"
                );
                let reader_message = reader_body["Message"]
                    .as_str()
                    .or_else(|| reader_body["message"].as_str())
                    .unwrap_or_default();
                assert_eq!(
                    message, reader_message,
                    "case {label:?}/{operation}: expected the wording {reader_operation} answers, got {message:?}"
                );
            } else {
                assert!(
                    message.contains(&arn),
                    "case {label:?}/{operation}: the refusal names the ARN that resolved to nothing, got {message:?}"
                );
            }
        }
    }

    // A resource that IS there is tagged, so the 404s above are about resolution and not
    // about the surface refusing every ARN.
    let arn = create_resource(&router, Kind::Connection, "a live connection", "live", &[]).await;
    let (status, body) = call(
        &router,
        "TagResource",
        serde_json::json!({ "ResourceArn": arn, "Tags": tag_list(&[pair("env", "prod")]) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a live connection: expected 200 from TagResource, got {status} {body}"
    );
}

/// How many writers tag one resource at once, and how many keys each of them assigns.
const CONCURRENT_WRITERS: usize = 16;

/// Concurrent `TagResource` calls against one resource lose no write: tagging is a
/// read-modify-write of the resource's one tag map, so a writer that read the map before
/// another's write must not drop that write — every writer's own key is there afterwards,
/// beside the tag the create stored, and the one key they all assign holds some writer's
/// value.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_tag_requests_lose_no_write() {
    let router = router();

    for kind in KINDS {
        let label = format!("concurrent tagging ({kind:?})");
        let arn = create_resource(
            &router,
            *kind,
            &label,
            &format!("x{}", kind_slug(*kind)),
            &[pair("env", "prod")],
        )
        .await;

        let shared = Arc::new(router.clone());
        let mut writers = Vec::with_capacity(CONCURRENT_WRITERS);
        for index in 0..CONCURRENT_WRITERS {
            let router = Arc::clone(&shared);
            let resource_arn = arn.clone();
            writers.push(tokio::spawn(async move {
                let batch = vec![
                    pair(&format!("writer-{index:02}"), &format!("value-{index:02}")),
                    pair("shared", &format!("writer-{index:02}")),
                ];
                let (status, body) = call(
                    &router,
                    "TagResource",
                    serde_json::json!({ "ResourceArn": resource_arn, "Tags": tag_list(&batch) }),
                )
                .await;
                (index, status, body)
            }));
        }
        for writer in writers {
            let (index, status, body) = writer.await.expect("a tag writer task");
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200 from writer {index}, got {status} {body}"
            );
        }

        let after = stored_tags(&router, &label, &arn).await;
        let reported = as_map(&after);
        let mut expected = as_map(&[pair("env", "prod")]);
        for index in 0..CONCURRENT_WRITERS {
            expected.insert(format!("writer-{index:02}"), format!("value-{index:02}"));
        }
        // The contended key's surviving value is whichever writer wrote it last, so it is
        // checked against the set of values a writer could have left and the rest of the
        // map is compared exactly.
        let contended = reported.get("shared").cloned();
        assert!(
            contended
                .as_deref()
                .is_some_and(|value| (0..CONCURRENT_WRITERS)
                    .any(|index| value == format!("writer-{index:02}"))),
            "case {label:?}: expected the contended key to hold some writer's value, got {contended:?}"
        );
        let mut without_contended = reported.clone();
        without_contended.remove("shared");
        assert_eq!(
            without_contended, expected,
            "case {label:?}: expected every concurrent write to be reflected, got {after:?}"
        );
    }
}
