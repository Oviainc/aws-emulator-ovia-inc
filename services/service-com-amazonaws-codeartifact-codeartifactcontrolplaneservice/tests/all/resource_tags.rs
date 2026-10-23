// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for CodeArtifact's resource tagging (`TagResource` and the
//! reader it writes for, `ListTagsForResource`).
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees — status, error code and body.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one example,
//! and each on BOTH resource kinds this service holds (a domain and a repository):
//!
//! 1. `TagResource` adds or updates tags: a key the resource already holds takes the
//!    request's value, a key it does not hold is added, a key the request does not name
//!    keeps the value it had, and a resource holds one entry per distinct key.
//! 2. A tag key and value are opaque Unicode strings, stored and reported verbatim —
//!    never normalised, trimmed or re-encoded.
//! 3. A resource holds at most [`MAX_TAGS_PER_RESOURCE`] tags; a call that would leave it
//!    holding more is refused with `ServiceQuotaExceededException` (402) and writes
//!    nothing.
//! 4. A tag set belongs to the one resource its ARN names: an ARN outside this service's
//!    namespace is a bad request (400), a well-formed one naming no held resource is a
//!    404, and tagging one resource never touches another's set.
//! 5. A value at a modelled bound is accepted and one past it is refused (400).

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::state::MAX_TAGS_PER_RESOURCE;

// The emulator harness is the crate's one copy (`tests/all/harness.rs`), shared by every
// suite: the same router assembly, the same SigV4 credential scope (whose region every
// minted ARN carries), the same request/answer reduction and the same account id. Only
// the TAGGED creates below are this suite's own, because the harness's creators take no
// `tags`.
use crate::harness::{call, emulator, Answer, ACCOUNT, REGION};

/// Percent-encode one query value the way an SDK does: `percent_encoding`'s
/// `NON_ALPHANUMERIC` set, so an ARN's colons and slashes — and the spaces and non-ASCII
/// characters the refusal rows carry — survive the server's decode. The maintained
/// `percent-encoding` crate does this, as it does for the sibling service crates' suites.
pub(crate) fn encode_query(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// A modelled `TagList` body member from key/value pairs, in the order given.
fn tag_list(pairs: &[(String, String)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": value }))
            .collect(),
    )
}

/// `(key, value)` pairs, for a table row written from `&str` literals.
pub(crate) fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `n` distinct tags, for the quota rows.
fn generated(n: usize) -> Vec<(String, String)> {
    (0..n)
        .map(|index| (format!("k{index}"), format!("v{index}")))
        .collect()
}

/// `CreateDomain`, optionally carrying create-time tags. Answers the created domain's ARN.
async fn create_tagged_domain(router: &MultiServiceRouter, domain: &str, tags: &[(String, String)]) -> String {
    let created = call(
        router,
        "POST",
        &format!("/v1/domain?domain={domain}"),
        Some(json!({ "tags": tag_list(tags) })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreateDomain {domain} must succeed, got {created:?}"
    );
    created.body["domain"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateDomain {domain} must report an ARN, got {created:?}"))
        .to_string()
}

/// `CreateRepository` in `domain`, optionally carrying create-time tags. Answers its ARN.
async fn create_tagged_repository(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    tags: &[(String, String)],
) -> String {
    let created = call(
        router,
        "POST",
        &format!("/v1/repository?domain={domain}&repository={repository}"),
        Some(json!({ "tags": tag_list(tags) })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreateRepository {repository} must succeed, got {created:?}"
    );
    created.body["repository"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRepository must report an ARN, got {created:?}"))
        .to_string()
}

/// `CreatePackageGroup` in `domain`, optionally carrying create-time tags. Answers its ARN.
async fn create_tagged_package_group(
    router: &MultiServiceRouter,
    domain: &str,
    pattern: &str,
    tags: &[(String, String)],
) -> String {
    let created = call(
        router,
        "POST",
        &format!("/v1/package-group?domain={domain}"),
        Some(json!({ "packageGroup": pattern, "tags": tag_list(tags) })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreatePackageGroup {pattern} must succeed, got {created:?}"
    );
    created.body["packageGroup"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreatePackageGroup must report an ARN, got {created:?}"))
        .to_string()
}

/// `TagResource`: `POST /v1/tag?resourceArn=…` with the batch in the body.
pub(crate) async fn tag(
    router: &MultiServiceRouter,
    arn: &str,
    batch: &[(String, String)],
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/tag?resourceArn={}", encode_query(arn)),
        Some(json!({ "tags": tag_list(batch) })),
    )
    .await
}

/// `ListTagsForResource`: `POST /v1/tags?resourceArn=…`.
pub(crate) async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/tags?resourceArn={}", encode_query(arn)),
        Some(json!({})),
    )
    .await
}

/// The tags `arn` holds, read back through `ListTagsForResource` as a map.
///
/// Asserts on the way through that the reported list holds one entry per distinct KEY: a
/// tag's key is its identity, so a resource reporting the same key twice would make the
/// map below silently lossy.
async fn held_tags(router: &MultiServiceRouter, arn: &str, case: &str) -> BTreeMap<String, String> {
    let answer = list_tags(router, arn).await;
    assert_eq!(
        (answer.status, answer.code()),
        (200, String::new()),
        "case {case:?}: ListTagsForResource must succeed, got {answer:?}"
    );
    let items = answer.body["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags list, got {answer:?}"))
        .clone();
    let map: BTreeMap<String, String> = items
        .iter()
        .map(|tag| {
            (
                tag["key"].as_str().unwrap_or_default().to_string(),
                tag["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        map.len(),
        items.len(),
        "case {case:?}: a resource must report one entry per key, got {items:?}"
    );
    map
}

/// The expected map for a table row.
fn expected_map(rows: &[(String, String)]) -> BTreeMap<String, String> {
    rows.iter().cloned().collect()
}

/// One row of the merge table: what the resource was created with, the batches sent to
/// `TagResource` in order, and the tag set the resource must then hold.
struct MergeCase {
    label: &'static str,
    at_create: Vec<(String, String)>,
    batches: Vec<Vec<(String, String)>>,
    expected: Vec<(String, String)>,
}

/// RULE 1: `TagResource` "adds or updates tags for a resource", keyed by tag key.
///
/// Rows differ along what the rule generalises over: the number of tags already held
/// (0, 1, 3), the number of batches (1, 2), the size of a batch (0, 1, 3), whether a
/// batch's keys overlap the held ones, whether one batch repeats a key, and the form of
/// the values (ASCII, empty, multi-byte).
#[tokio::test]
async fn tag_resource_adds_or_updates_tags_keyed_by_tag_key() {
    let cases = vec![
        MergeCase {
            label: "no tags held, one batch of one",
            at_create: Vec::new(),
            batches: vec![pairs(&[("team", "platform")])],
            expected: pairs(&[("team", "platform")]),
        },
        MergeCase {
            label: "a batch of three added beside the create-time tags",
            at_create: pairs(&[("created", "yes")]),
            batches: vec![pairs(&[("a", "1"), ("b", "2"), ("c", "3")])],
            expected: pairs(&[("created", "yes"), ("a", "1"), ("b", "2"), ("c", "3")]),
        },
        MergeCase {
            label: "a key already held takes the new value, the others keep theirs",
            at_create: pairs(&[("env", "dev"), ("team", "platform"), ("cost", "42")]),
            batches: vec![pairs(&[("env", "prod")])],
            expected: pairs(&[("env", "prod"), ("team", "platform"), ("cost", "42")]),
        },
        MergeCase {
            label: "a batch that both updates and adds",
            at_create: pairs(&[("env", "dev")]),
            batches: vec![pairs(&[("env", "stage"), ("owner", "ops")])],
            expected: pairs(&[("env", "stage"), ("owner", "ops")]),
        },
        MergeCase {
            label: "one batch repeating a key: the last value in the batch wins",
            at_create: Vec::new(),
            batches: vec![pairs(&[("dup", "first"), ("dup", "second")])],
            expected: pairs(&[("dup", "second")]),
        },
        MergeCase {
            label: "an empty batch leaves the set unchanged",
            at_create: pairs(&[("env", "dev")]),
            batches: vec![Vec::new()],
            expected: pairs(&[("env", "dev")]),
        },
        MergeCase {
            label: "two batches, the second disjoint from the first",
            at_create: Vec::new(),
            batches: vec![pairs(&[("a", "1")]), pairs(&[("b", "2")])],
            expected: pairs(&[("a", "1"), ("b", "2")]),
        },
        MergeCase {
            label: "two batches, the second re-editing the first's key",
            at_create: pairs(&[("keep", "me")]),
            batches: vec![pairs(&[("a", "1")]), pairs(&[("a", "2")])],
            expected: pairs(&[("keep", "me"), ("a", "2")]),
        },
        MergeCase {
            label: "a value cleared to the empty string",
            at_create: pairs(&[("note", "something")]),
            batches: vec![pairs(&[("note", "")])],
            expected: pairs(&[("note", "")]),
        },
        MergeCase {
            label: "multi-byte keys and values",
            at_create: pairs(&[("キー", "値")]),
            batches: vec![pairs(&[("キー", "新しい値"), ("مفتاح", "قيمة")])],
            expected: pairs(&[("キー", "新しい値"), ("مفتاح", "قيمة")]),
        },
    ];

    let router = emulator();
    for (index, case) in cases.iter().enumerate() {
        // The rule is about a RESOURCE, so every row runs on both kinds this service
        // holds: a domain, and a repository inside it.
        let domain = format!("merge-{index}");
        let domain_arn = create_tagged_domain(&router, &domain, &case.at_create).await;
        let repository_arn =
            create_tagged_repository(&router, &domain, "repo", &case.at_create).await;

        for (kind, arn) in [("domain", &domain_arn), ("repository", &repository_arn)] {
            let case_name = format!("{} on a {kind}", case.label);
            for batch in &case.batches {
                let answer = tag(&router, arn, batch).await;
                assert_eq!(
                    (answer.status, answer.code()),
                    (200, String::new()),
                    "case {case_name:?}: TagResource must accept {batch:?}, got {answer:?}"
                );
            }
            let got = held_tags(&router, arn, &case_name).await;
            assert_eq!(
                got,
                expected_map(&case.expected),
                "case {case_name:?}: expected {:?}, got {got:?}",
                expected_map(&case.expected)
            );
        }

        // RULE 4, in passing: the two resources carry independent sets, so the batches
        // above landed on each one separately rather than on a shared map.
        let only_domain = tag(&router, &domain_arn, &pairs(&[("domain-only", "1")])).await;
        assert_eq!(
            only_domain.status, 200,
            "case {:?}: tagging the domain must succeed, got {only_domain:?}",
            case.label
        );
        let repository_set = held_tags(&router, &repository_arn, case.label).await;
        assert!(
            !repository_set.contains_key("domain-only"),
            "case {:?}: a domain's tag must not appear on its repository, got {repository_set:?}",
            case.label
        );
    }
}

/// RULE 2: a tag key and value are opaque Unicode strings.
///
/// The catalog of forms comes from the model (`smithy_query(query="shape",shape="Tag")`):
/// `TagKey` is len 1..128 pattern `^\P{C}+$` and `TagValue` is len 0..256 pattern
/// `^\P{C}*$` — every Unicode scalar that is not a control character, so the rows below
/// span the scripts, the composition forms and the grapheme clusters that admits.
#[tokio::test]
async fn tag_keys_and_values_round_trip_verbatim() {
    // (case, key, value)
    let rows: Vec<(&str, String, String)> = vec![
        ("plain ASCII", "team".to_string(), "platform".to_string()),
        ("the empty value", "cleared".to_string(), String::new()),
        (
            "an astral-plane emoji",
            "rocket".to_string(),
            "🚀🛰️".to_string(),
        ),
        (
            "an emoji with a skin-tone modifier",
            "thumbs".to_string(),
            "👍🏽".to_string(),
        ),
        (
            "CJK in both key and value",
            "コード".to_string(),
            "アーティファクト".to_string(),
        ),
        (
            "right-to-left script",
            "مفتاح".to_string(),
            "مرحبا بالعالم".to_string(),
        ),
        ("Greek", "κλειδί".to_string(), "τιμή".to_string()),
        (
            "a precomposed (NFC) value",
            "nfc".to_string(),
            "café".to_string(),
        ),
        (
            "the same text decomposed (NFD): a DIFFERENT value, never normalised",
            "nfd".to_string(),
            "cafe\u{301}".to_string(),
        ),
        (
            "interior spaces and punctuation",
            "sentence".to_string(),
            "a b, c; d  e".to_string(),
        ),
        (
            "a value whose bytes are not its characters (multi-byte, under the bound)",
            "bytes".to_string(),
            "ß".repeat(100),
        ),
        (
            "a key of one character: the modelled minimum length",
            "k".to_string(),
            "shortest-key".to_string(),
        ),
    ];

    let router = emulator();
    let domain_arn = create_tagged_domain(&router, "unicode-tags", &[]).await;
    let repository_arn = create_tagged_repository(&router, "unicode-tags", "repo", &[]).await;

    for (kind, arn) in [("domain", &domain_arn), ("repository", &repository_arn)] {
        let mut expected: BTreeMap<String, String> = BTreeMap::new();
        for (case, key, value) in &rows {
            let case_name = format!("{case} on a {kind}");
            let answer = tag(&router, arn, &[(key.clone(), value.clone())]).await;
            assert_eq!(
                (answer.status, answer.code()),
                (200, String::new()),
                "case {case_name:?}: TagResource must accept {key:?}={value:?}, got {answer:?}"
            );
            expected.insert(key.clone(), value.clone());

            let got = held_tags(&router, arn, &case_name).await;
            let stored = got.get(key);
            assert_eq!(
                stored,
                Some(value),
                "case {case_name:?}: expected the value {value:?} verbatim, got {stored:?}"
            );
            // Every value written so far is still exactly what was written: a later tag
            // never rewrites an earlier one.
            assert_eq!(
                got, expected,
                "case {case_name:?}: expected {expected:?}, got {got:?}"
            );
        }

        // The two composition forms of the same text stayed distinct values.
        let got = held_tags(&router, arn, "composition forms").await;
        assert_ne!(
            got.get("nfc"),
            got.get("nfd"),
            "NFC and NFD values must not be normalised together, got {got:?}"
        );

        // Reading does not change what a later caller sees.
        let again = held_tags(&router, arn, "a second read").await;
        assert_eq!(
            again, got,
            "ListTagsForResource must be repeatable, got {again:?} then {got:?}"
        );
    }
}

/// RULE 3: a resource holds at most [`MAX_TAGS_PER_RESOURCE`] tags — CodeArtifact's
/// published tag restriction ("Maximum number of tags per resource – 50"), which
/// `ServiceQuotaExceededException` (402) is the modelled refusal for.
///
/// The bound is tested at 0, 1 and both published edges (the bound itself and one past
/// it), in one batch and accumulated across batches.
#[tokio::test]
async fn a_resource_holds_at_most_the_published_number_of_tags() {
    let router = emulator();

    // (case, how many tags in one batch, whether the call is accepted)
    let one_batch: Vec<(String, usize, bool)> = vec![
        ("an empty batch".to_string(), 0, true),
        ("one tag".to_string(), 1, true),
        (
            format!("one short of the bound ({})", MAX_TAGS_PER_RESOURCE - 1),
            MAX_TAGS_PER_RESOURCE - 1,
            true,
        ),
        (
            format!("the bound itself ({MAX_TAGS_PER_RESOURCE})"),
            MAX_TAGS_PER_RESOURCE,
            true,
        ),
        (
            format!("one past the bound ({})", MAX_TAGS_PER_RESOURCE + 1),
            MAX_TAGS_PER_RESOURCE + 1,
            false,
        ),
    ];

    for (index, (case, count, accepted)) in one_batch.iter().enumerate() {
        let domain = format!("quota-{index}");
        let domain_arn = create_tagged_domain(&router, &domain, &[]).await;
        let repository_arn = create_tagged_repository(&router, &domain, "repo", &[]).await;

        for (kind, arn) in [("domain", &domain_arn), ("repository", &repository_arn)] {
            let case_name = format!("{case} on a {kind}");
            let answer = tag(&router, arn, &generated(*count)).await;
            if *accepted {
                assert_eq!(
                    (answer.status, answer.code()),
                    (200, String::new()),
                    "case {case_name:?}: a batch of {count} must be accepted, got {answer:?}"
                );
                let got = held_tags(&router, arn, &case_name).await;
                assert_eq!(
                    got,
                    expected_map(&generated(*count)),
                    "case {case_name:?}: expected {count} tags, got {got:?}"
                );
            } else {
                assert_eq!(
                    (answer.status, answer.code()),
                    (402, "ServiceQuotaExceededException".to_string()),
                    "case {case_name:?}: a batch of {count} must be refused, got {answer:?}"
                );
                let got = held_tags(&router, arn, &case_name).await;
                assert!(
                    got.is_empty(),
                    "case {case_name:?}: a refused batch must write nothing, got {got:?}"
                );
            }
        }
    }

    // Accumulated: the bound is on what the RESOURCE holds, not on one request.
    let arn = create_tagged_domain(&router, "quota-accumulated", &[]).await;
    let filled = tag(&router, &arn, &generated(MAX_TAGS_PER_RESOURCE)).await;
    assert_eq!(
        filled.status, 200,
        "filling to the bound must be accepted, got {filled:?}"
    );
    let at_bound = held_tags(&router, &arn, "at the bound").await;

    let over = tag(&router, &arn, &pairs(&[("one-more", "x")])).await;
    assert_eq!(
        (over.status, over.code()),
        (402, "ServiceQuotaExceededException".to_string()),
        "one new key past the bound must be refused, got {over:?}"
    );
    let after = held_tags(&router, &arn, "after the refusal").await;
    assert_eq!(
        after, at_bound,
        "a refused call must leave the tag set untouched, got {after:?}"
    );

    // Editing a key already held does not grow the set, so it is accepted at the bound.
    let edited = tag(&router, &arn, &pairs(&[("k0", "edited")])).await;
    assert_eq!(
        (edited.status, edited.code()),
        (200, String::new()),
        "editing a held key at the bound must be accepted, got {edited:?}"
    );
    let reread = held_tags(&router, &arn, "after the edit").await;
    assert_eq!(
        reread.len(),
        MAX_TAGS_PER_RESOURCE,
        "an edit must not grow the set, got {reread:?}"
    );
    assert_eq!(
        reread.get("k0").map(String::as_str),
        Some("edited"),
        "the edited key must carry the new value, got {reread:?}"
    );
}

/// RULES 1 and 3 at the OTHER write sites: `CreateDomain` and `CreateRepository` carry a
/// `tags` member, so the per-resource tag rules — one entry per distinct key, at most
/// [`MAX_TAGS_PER_RESOURCE`] of them — hold for the set they store too.
#[tokio::test]
async fn create_time_tags_follow_the_same_per_resource_rules() {
    let router = emulator();

    // One entry per distinct key: a create repeating a key stores one tag, the last value.
    let domain_arn = create_tagged_domain(
        &router,
        "create-tags",
        &pairs(&[("dup", "first"), ("env", "dev"), ("dup", "second")]),
    )
    .await;
    let got = held_tags(&router, &domain_arn, "a create repeating a key").await;
    assert_eq!(
        got,
        expected_map(&pairs(&[("dup", "second"), ("env", "dev")])),
        "a create repeating a key must store one entry per key, got {got:?}"
    );
    let repository_arn = create_tagged_repository(
        &router,
        "create-tags",
        "repo",
        &pairs(&[("dup", "first"), ("dup", "second")]),
    )
    .await;
    let got = held_tags(&router, &repository_arn, "a repository create repeating a key").await;
    assert_eq!(
        got,
        expected_map(&pairs(&[("dup", "second")])),
        "a repository create repeating a key must store one entry per key, got {got:?}"
    );

    // The bound itself is accepted at create time...
    let at_bound = create_tagged_domain(&router, "create-at-bound", &generated(MAX_TAGS_PER_RESOURCE)).await;
    let got = held_tags(&router, &at_bound, "a create at the bound").await;
    assert_eq!(
        got,
        expected_map(&generated(MAX_TAGS_PER_RESOURCE)),
        "a create carrying {MAX_TAGS_PER_RESOURCE} tags must store them all, got {got:?}"
    );

    // ...and one past it is refused, with nothing created.
    let over = call(
        &router,
        "POST",
        "/v1/domain?domain=create-over-bound",
        Some(json!({ "tags": tag_list(&generated(MAX_TAGS_PER_RESOURCE + 1)) })),
    )
    .await;
    assert_eq!(
        (over.status, over.code()),
        (402, "ServiceQuotaExceededException".to_string()),
        "a create carrying {} tags must be refused, got {over:?}",
        MAX_TAGS_PER_RESOURCE + 1
    );
    let absent = call(&router, "GET", "/v1/domain?domain=create-over-bound", None).await;
    assert_eq!(
        (absent.status, absent.code()),
        (404, "ResourceNotFoundException".to_string()),
        "a refused create must leave no domain behind, got {absent:?}"
    );

    let over_repository = call(
        &router,
        "POST",
        "/v1/repository?domain=create-tags&repository=over-bound",
        Some(json!({ "tags": tag_list(&generated(MAX_TAGS_PER_RESOURCE + 1)) })),
    )
    .await;
    assert_eq!(
        (over_repository.status, over_repository.code()),
        (402, "ServiceQuotaExceededException".to_string()),
        "a repository create carrying {} tags must be refused, got {over_repository:?}",
        MAX_TAGS_PER_RESOURCE + 1
    );
    let absent = call(
        &router,
        "GET",
        "/v1/repository?domain=create-tags&repository=over-bound",
        None,
    )
    .await;
    assert_eq!(
        (absent.status, absent.code()),
        (404, "ResourceNotFoundException".to_string()),
        "a refused create must leave no repository behind, got {absent:?}"
    );
}

/// RULES 1–4 on the THIRD taggable kind this store holds: a package group, whose
/// identifier is a PATH (`/*`, `/npm/scope/*`), so its ARN's resource field carries `/`
/// separators of its own.
#[tokio::test]
async fn a_package_group_is_tagged_by_the_same_rules() {
    let router = emulator();
    let domain_arn = create_tagged_domain(&router, "group-tags", &[]).await;

    // Every domain is created holding its root group, so its ARN is addressable without
    // a create of its own.
    let root_arn = format!(
        "arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/group-tags/*"
    );
    // A nested pattern, created with tags that repeat a key: rule 1 at the create site.
    let nested_arn = create_tagged_package_group(
        &router,
        "group-tags",
        "/npm/scope/*",
        &pairs(&[("dup", "first"), ("env", "dev"), ("dup", "second")]),
    )
    .await;
    assert_eq!(
        nested_arn,
        format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/group-tags/npm/scope/*"),
        "the group's ARN must carry its pattern as a path, got {nested_arn:?}"
    );
    let got = held_tags(&router, &nested_arn, "a group created with a repeated key").await;
    assert_eq!(
        got,
        expected_map(&pairs(&[("dup", "second"), ("env", "dev")])),
        "a create repeating a key must store one entry per key, got {got:?}"
    );

    // Rule 1 through TagResource, on both patterns.
    for (case, arn) in [("the root group", &root_arn), ("a nested group", &nested_arn)] {
        let answer = tag(&router, arn, &pairs(&[("env", "prod"), ("owner", "ops")])).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {case:?}: TagResource must accept a batch, got {answer:?}"
        );
        let got = held_tags(&router, arn, case).await;
        assert_eq!(
            got.get("env").map(String::as_str),
            Some("prod"),
            "case {case:?}: the updated key must carry the new value, got {got:?}"
        );
        assert_eq!(
            got.get("owner").map(String::as_str),
            Some("ops"),
            "case {case:?}: the added key must be stored, got {got:?}"
        );
    }

    // Rule 4: the two groups and the domain carry independent sets.
    let root_set = held_tags(&router, &root_arn, "the root group").await;
    assert!(
        !root_set.contains_key("dup"),
        "the root group must not see a nested group's tag, got {root_set:?}"
    );
    let domain_set = held_tags(&router, &domain_arn, "the domain").await;
    assert!(
        domain_set.is_empty(),
        "the domain must hold no tag of its package groups, got {domain_set:?}"
    );

    // Rule 3: the per-resource cap holds for a package group too.
    let over = tag(&router, &root_arn, &generated(MAX_TAGS_PER_RESOURCE + 1)).await;
    assert_eq!(
        (over.status, over.code()),
        (402, "ServiceQuotaExceededException".to_string()),
        "a batch of {} on a package group must be refused, got {over:?}",
        MAX_TAGS_PER_RESOURCE + 1
    );
    let after = held_tags(&router, &root_arn, "after the refusal").await;
    assert_eq!(
        after, root_set,
        "a refused batch must leave the group's tags untouched, got {after:?}"
    );

    // Rule 4: a pattern this domain does not hold is a not-found, both ways.
    let absent = format!(
        "arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/group-tags/maven/absent/*"
    );
    let tagged = tag(&router, &absent, &pairs(&[("probe", "1")])).await;
    assert_eq!(
        (tagged.status, tagged.code()),
        (404, "ResourceNotFoundException".to_string()),
        "tagging a pattern the domain does not hold must be a 404, got {tagged:?}"
    );
    let listed = list_tags(&router, &absent).await;
    assert_eq!(
        (listed.status, listed.code()),
        (404, "ResourceNotFoundException".to_string()),
        "listing a pattern the domain does not hold must be a 404, got {listed:?}"
    );
}

/// RULE 4: a `resourceArn` names one resource of THIS service.
///
/// An ARN that is not a well-formed resource ARN of the `codeartifact` namespace names
/// nothing this API can address, so it is a bad request; a well-formed one that names no
/// resource this store holds is a not-found. Both `TagResource` and
/// `ListTagsForResource` read the same ARN, so the table runs against both.
#[tokio::test]
async fn a_resource_arn_names_one_resource_of_this_service() {
    let router = emulator();
    create_tagged_domain(&router, "arn-rules", &[]).await;
    create_tagged_repository(&router, "arn-rules", "repo", &[]).await;

    // (case, resourceArn, expected status, expected error code)
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a domain of this account",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:domain/arn-rules"),
            200,
            "",
        ),
        (
            "a repository of this account",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:repository/arn-rules/repo"),
            200,
            "",
        ),
        (
            "a domain name this store does not hold",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:domain/absent"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a repository name the domain does not hold",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:repository/arn-rules/absent"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a repository in a domain this store does not hold",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:repository/absent/repo"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the right domain name pinned to another account",
            format!("arn:aws:codeartifact:{REGION}:999999999999:domain/arn-rules"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the root package group every domain holds",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/arn-rules/*"),
            200,
            "",
        ),
        (
            "a package-group pattern the domain does not hold",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/arn-rules/npm/*"),
            404,
            "ResourceNotFoundException",
        ),
        (
            // `ResourceType` models package, package-version and asset beside domain and
            // repository; this store holds no record of those kinds at all.
            "a kind this store holds no record of",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:package/arn-rules/repo/npm//pkg"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an ARN of another vendor",
            "arn:aws:s3:::assets-bucket".to_string(),
            400,
            "ValidationException",
        ),
        (
            "an ARN of another vendor that mimics the resource field",
            format!("arn:aws:codebuild:{REGION}:{ACCOUNT}:domain/arn-rules"),
            400,
            "ValidationException",
        ),
        (
            "a value that is not an ARN",
            "arn-rules".to_string(),
            400,
            "ValidationException",
        ),
        (
            "an ARN that stops before its resource field",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}"),
            400,
            "ValidationException",
        ),
        (
            "an ARN whose vendor differs only in case",
            format!("arn:aws:CodeArtifact:{REGION}:{ACCOUNT}:domain/arn-rules"),
            400,
            "ValidationException",
        ),
    ];

    for (case, arn, status, code) in &rows {
        let tagged = tag(&router, arn, &pairs(&[("probe", "1")])).await;
        assert_eq!(
            (tagged.status, tagged.code()),
            (*status, (*code).to_string()),
            "case {case:?}: TagResource on {arn:?} expected {status} {code}, got {tagged:?}"
        );

        let listed = list_tags(&router, arn).await;
        assert_eq!(
            (listed.status, listed.code()),
            (*status, (*code).to_string()),
            "case {case:?}: ListTagsForResource on {arn:?} expected {status} {code}, got {listed:?}"
        );
    }

    // A 404 reports the KIND the ARN resolved to, wherever `ResourceType` declares a
    // value for it: `domain` and `repository` are modelled values, a package group has
    // none, and an ARN that resolved to no kind at all has nothing to report.
    for (case, arn, expected_type) in [
        (
            "an absent domain",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:domain/absent"),
            Some("domain"),
        ),
        (
            "an absent repository",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:repository/arn-rules/absent"),
            Some("repository"),
        ),
        (
            "an absent package group",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/arn-rules/npm/*"),
            None,
        ),
        (
            "a kind this store holds no record of",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:package/arn-rules/repo/npm//pkg"),
            None,
        ),
    ] {
        let answer = tag(&router, &arn, &pairs(&[("probe", "1")])).await;
        let got = answer.body.get("resourceType").and_then(Value::as_str);
        assert_eq!(
            got, expected_type,
            "case {case:?}: expected resourceType {expected_type:?}, got {got:?} in {answer:?}"
        );
    }

    // Neither refused call left a tag anywhere: the two resources that do exist hold only
    // the probe the accepted rows wrote.
    for (kind, arn) in [
        (
            "domain",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:domain/arn-rules"),
        ),
        (
            "repository",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:repository/arn-rules/repo"),
        ),
    ] {
        let got = held_tags(&router, &arn, kind).await;
        assert_eq!(
            got,
            expected_map(&pairs(&[("probe", "1")])),
            "the {kind} must hold exactly the probe tag, got {got:?}"
        );
    }
}

/// RULE 5: the modelled bounds on a tag.
///
/// From `smithy_query(query="shape",shape="Tag,TagList")`: `TagKey` is len 1..128
/// pattern `^\P{C}+$`, `TagValue` len 0..256 pattern `^\P{C}*$` and `TagList` len
/// 0..200 — each tested AT the bound and one past it, and the pattern on a character of
/// Unicode category C (which it excludes). A refusal past a bound is the framework's
/// boundary refusal (HTTP 400), which runs before the handler, so none of these rows may
/// store anything.
#[tokio::test]
async fn the_modelled_bounds_of_a_tag_are_enforced() {
    let router = emulator();
    let arn = create_tagged_domain(&router, "tag-bounds", &[]).await;

    // (case, key, value, accepted)
    let rows: Vec<(String, String, String, bool)> = vec![
        (
            "a key of 1 character: the modelled minimum".to_string(),
            "a".to_string(),
            "v".to_string(),
            true,
        ),
        (
            "a key of 128 characters: the modelled maximum".to_string(),
            "k".repeat(128),
            "v".to_string(),
            true,
        ),
        (
            "a key of 129 characters: one past the maximum".to_string(),
            "k".repeat(129),
            "v".to_string(),
            false,
        ),
        (
            "a value of 0 characters: the modelled minimum".to_string(),
            "empty-value".to_string(),
            String::new(),
            true,
        ),
        (
            "a value of 256 characters: the modelled maximum".to_string(),
            "long-value".to_string(),
            "v".repeat(256),
            true,
        ),
        (
            "a value of 257 characters: one past the maximum".to_string(),
            "over-long-value".to_string(),
            "v".repeat(257),
            false,
        ),
        (
            // U+200D ZERO WIDTH JOINER is category Cf, inside `\p{C}`, so an emoji
            // sequence joined with it is NOT a legal tag value however common it is.
            "a value carrying a format character (U+200D)".to_string(),
            "joined".to_string(),
            "👨\u{200d}👩".to_string(),
            false,
        ),
        (
            "a value carrying a control character (U+000A)".to_string(),
            "newline".to_string(),
            "two\nlines".to_string(),
            false,
        ),
        (
            "a key carrying a control character (U+0009)".to_string(),
            "ta\tb".to_string(),
            "v".to_string(),
            false,
        ),
    ];

    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for (case, key, value, accepted) in &rows {
        let answer = tag(&router, &arn, &[(key.clone(), value.clone())]).await;
        if *accepted {
            assert_eq!(
                (answer.status, answer.code()),
                (200, String::new()),
                "case {case:?}: expected 200, got {answer:?}"
            );
            expected.insert(key.clone(), value.clone());
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected a 400 refusal, got {answer:?}"
            );
        }
        let got = held_tags(&router, &arn, case).await;
        assert_eq!(
            got, expected,
            "case {case:?}: expected the resource to hold {expected:?}, got {got:?}"
        );
    }

    // `TagList`'s own bound: 200 tags in one batch is the modelled maximum, and 201 is
    // refused by the boundary validator — before the per-resource quota is consulted, so
    // the answer is a 400 rather than the 402 a batch of 51 gets.
    let at_list_bound = tag(&router, &arn, &generated(200)).await;
    assert_eq!(
        (at_list_bound.status, at_list_bound.code()),
        (402, "ServiceQuotaExceededException".to_string()),
        "200 tags are within TagList but past the per-resource quota, got {at_list_bound:?}"
    );
    let past_list_bound = tag(&router, &arn, &generated(201)).await;
    assert_eq!(
        past_list_bound.status, 400,
        "201 tags are past TagList's modelled maximum, got {past_list_bound:?}"
    );
    let got = held_tags(&router, &arn, "after the TagList bounds").await;
    assert_eq!(
        got, expected,
        "neither oversized batch may store anything, got {got:?}"
    );
}

/// One row of the ordering table: the batch sent to a resource holding `alpha`, `beta`
/// and `gamma` (in that order), and the ordered tag list it must then report.
struct OrderCase {
    label: &'static str,
    batch: Vec<(String, String)>,
    expected: Vec<(String, String)>,
}

/// RULE 1, on ORDER: a key already held keeps the POSITION it had and takes the new
/// value, and a key the resource does not hold is APPENDED — the merge is keyed, so an
/// update never reorders the set. Asserted on the ordered `tags` array rather than on the
/// per-key map every other case reduces to, with the rows differing in WHICH held
/// position the request updates (first, middle, last) and whether the batch also adds.
#[tokio::test]
async fn an_update_keeps_a_held_keys_position_and_appends_a_new_key() {
    let held = pairs(&[("alpha", "1"), ("beta", "2"), ("gamma", "3")]);

    let rows = [
        OrderCase {
            label: "updating the FIRST held key",
            batch: pairs(&[("alpha", "updated")]),
            expected: pairs(&[("alpha", "updated"), ("beta", "2"), ("gamma", "3")]),
        },
        OrderCase {
            label: "updating the MIDDLE held key",
            batch: pairs(&[("beta", "updated")]),
            expected: pairs(&[("alpha", "1"), ("beta", "updated"), ("gamma", "3")]),
        },
        OrderCase {
            label: "updating the LAST held key",
            batch: pairs(&[("gamma", "updated")]),
            expected: pairs(&[("alpha", "1"), ("beta", "2"), ("gamma", "updated")]),
        },
        OrderCase {
            label: "updating a held key and adding a new one in one batch",
            batch: pairs(&[("beta", "updated"), ("delta", "4")]),
            expected: pairs(&[
                ("alpha", "1"),
                ("beta", "updated"),
                ("gamma", "3"),
                ("delta", "4"),
            ]),
        },
        OrderCase {
            label: "a batch given in the reverse of the held order",
            batch: pairs(&[("gamma", "z"), ("beta", "y"), ("alpha", "x")]),
            expected: pairs(&[("alpha", "x"), ("beta", "y"), ("gamma", "z")]),
        },
    ];

    let router = emulator();
    for (index, OrderCase { label: case, batch, expected }) in rows.iter().enumerate() {
        let domain = format!("order-{index}");
        let domain_arn = create_tagged_domain(&router, &domain, &held).await;
        let repository_arn = create_tagged_repository(&router, &domain, "repo", &held).await;

        for (kind, arn) in [("domain", &domain_arn), ("repository", &repository_arn)] {
            let case_name = format!("{case} on a {kind}");
            let answer = tag(&router, arn, batch).await;
            assert_eq!(
                (answer.status, answer.code()),
                (200, String::new()),
                "case {case_name:?}: TagResource must accept {batch:?}, got {answer:?}"
            );

            let listed = list_tags(&router, arn).await;
            let got: Vec<(String, String)> = listed.body["tags"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case_name:?}: expected a tags list, got {listed:?}"))
                .iter()
                .map(|entry| {
                    (
                        entry["key"].as_str().unwrap_or_default().to_string(),
                        entry["value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            assert_eq!(
                &got, expected,
                "case {case_name:?}: expected the ordered set {expected:?}, got {got:?}"
            );
        }
    }
}
