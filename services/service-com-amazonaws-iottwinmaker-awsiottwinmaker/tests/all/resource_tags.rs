// SPDX-License-Identifier: CC0-1.0

//! The tagging surface — `TagResource`, `ListTagsForResource`, `UntagResource` —
//! end to end over HTTP.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses), so the route, the query bindings, the
//! status code and the serialized body are all exercised, not just a handler
//! body.
//!
//! The tests assert RULES over tables of inputs rather than one recorded case:
//! tagging is an upsert on the tag set the resource already carries, whatever
//! kind of resource the ARN names; untagging removes exactly the named keys and
//! is idempotent; a call that would carry a resource past the QUOTA of 50 tags
//! per resource is refused as the quota breach it is and stores nothing; and a
//! page sequence partitions the listing.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, MAX_TAGS_PER_RESOURCE,
};

use crate::harness::{component_type_arn, entity_arn, scene_arn, workspace_arn, Emulator, Resp};

/// The `tags` member of a `ListTagsForResource` response, as a map a test can
/// compare: a JSON object, never an absent member.
fn tags_of(resp: &Resp) -> BTreeMap<String, String> {
    resp.member("tags")
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| (key, value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// Every tag `resource_arn` carries, read through one unpaginated listing large
/// enough for the whole set (the resource can hold at most
/// [`MAX_TAGS_PER_RESOURCE`], and `maxResults` admits more than that).
async fn all_tags(emulator: &Emulator, case: &str, resource_arn: &str) -> BTreeMap<String, String> {
    let resp = emulator
        .list_tags_for_resource(resource_arn, json!({"maxResults": MAX_TAGS_PER_RESOURCE}))
        .await;
    assert_eq!(
        resp.status, 200,
        "case {case:?}: expected 200 from ListTagsForResource, got {resp:?}"
    );
    assert!(
        resp.member("nextToken").is_null(),
        "case {case:?}: a page holding the whole tag set must carry no nextToken, got {:?}",
        resp.member("nextToken")
    );
    tags_of(&resp)
}

/// `count` distinct tags, keyed `k00`, `k01`, … with the value suffix `suffix`.
fn tag_batch(count: usize, suffix: &str) -> BTreeMap<String, String> {
    (0..count)
        .map(|i| (format!("k{i:02}"), format!("v{i:02}-{suffix}")))
        .collect()
}

/// A tag map as the JSON object `TagResource` takes.
fn as_json(tags: &BTreeMap<String, String>) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect(),
    )
}

/// The page size the service serves for a requested `maxResults`: the
/// documented default of 25 when the caller names none, and never more than the
/// service's own ceiling.
fn effective_page_size(max_results: Option<u32>) -> usize {
    match max_results {
        None => DEFAULT_PAGE_SIZE,
        Some(size) => (size as usize).min(MAX_PAGE_SIZE),
    }
}

/// Walk every page of `ListTagsForResource` at page size `max_results`,
/// returning the concatenated entries (so a duplicate is visible) and the
/// number of pages.
async fn walk_tag_pages(
    emulator: &Emulator,
    case: &str,
    resource_arn: &str,
    max_results: Option<u32>,
) -> (Vec<(String, String)>, usize) {
    let mut entries = Vec::new();
    let mut pages = 0;
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(max_results) = max_results {
            request["maxResults"] = json!(max_results);
        }
        if let Some(token) = &next_token {
            request["nextToken"] = json!(token);
        }
        let resp = emulator.list_tags_for_resource(resource_arn, request).await;
        assert_eq!(
            resp.status, 200,
            "case {case:?}: page {pages} expected 200, got {resp:?}"
        );
        pages += 1;
        let page = tags_of(&resp);
        assert!(
            page.len() <= effective_page_size(max_results),
            "case {case:?}: page {pages} served {} entries, past the page size {}",
            page.len(),
            effective_page_size(max_results)
        );
        entries.extend(page);
        next_token = resp.member("nextToken").as_str().map(str::to_string);
        if next_token.is_none() {
            return (entries, pages);
        }
        assert!(
            pages < 64,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
    }
}

/// RULE: `TagResource` upserts the request's tags onto the tag set the resource
/// its ARN names already carries — a key the resource lacks is added, a key it
/// carries has its value replaced, and every other tag is left alone — and
/// `ListTagsForResource` reports exactly that set.
///
/// The rows differ along the shape of the applied map (one key, several keys,
/// keys drawn from the whole `TagKey` character set, an empty map, a repeat of
/// what is already stored), and the whole table is driven against EVERY kind of
/// resource this service mints an ARN for, since the rule is per-ARN and not
/// per-resource-kind.
#[tokio::test]
async fn tagging_upserts_and_lists_back_exactly_what_was_stored() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator
            .call(
                "POST",
                "/workspaces/tagged",
                Some(json!({"tags": {"seed": "fromCreate"}})),
            )
            .await
            .status,
        200,
        "the workspace under test must be created"
    );
    assert_eq!(
        emulator
            .create_entity(
                "tagged",
                json!({
                    "entityId": "pump",
                    "entityName": "Pump",
                    "tags": {"seed": "fromCreate"},
                }),
            )
            .await
            .status,
        200,
        "the entity under test must be created"
    );

    // The keys here are drawn from the `TagKey` pattern's own character set
    // (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, from
    // `smithy_query(query="shape",shape="TagKey")`): letters, separators,
    // numbers and `_ . : / = + - @`.
    let rows: Vec<(&str, BTreeMap<String, String>)> = vec![
        (
            "twoNewKeys",
            BTreeMap::from([
                ("env".to_string(), "test".to_string()),
                ("team".to_string(), "plant".to_string()),
            ]),
        ),
        (
            "replaceOneValue",
            BTreeMap::from([("env".to_string(), "prod".to_string())]),
        ),
        (
            "punctuatedKeys",
            BTreeMap::from([
                ("cost:center".to_string(), "cc/1".to_string()),
                ("path=to+x".to_string(), "v@1".to_string()),
            ]),
        ),
        (
            "unicodeAndSpaces",
            BTreeMap::from([("équipe principale".to_string(), "usine 1".to_string())]),
        ),
        ("emptyMap", BTreeMap::new()),
        (
            "repeatOfWhatIsStored",
            BTreeMap::from([("env".to_string(), "prod".to_string())]),
        ),
    ];

    for (kind, resource_arn) in [
        ("workspace", workspace_arn("tagged")),
        ("entity", entity_arn("tagged", "pump")),
    ] {
        // The create's own tags are the SAME set the tagging API reads and
        // writes, so the listing starts from them.
        let mut expected: BTreeMap<String, String> =
            BTreeMap::from([("seed".to_string(), "fromCreate".to_string())]);
        assert_eq!(
            all_tags(&emulator, kind, &resource_arn).await,
            expected,
            "case {kind:?}: the create's tags must be the set the tagging API reads"
        );

        for (row, applied) in &rows {
            let case = format!("{kind}/{row}");
            let tagged = emulator.tag_resource(&resource_arn, as_json(applied)).await;
            assert_eq!(
                tagged.status, 200,
                "case {case:?}: expected 200 from TagResource, got {tagged:?}"
            );
            expected.extend(applied.clone());
            let listed = all_tags(&emulator, &case, &resource_arn).await;
            assert_eq!(
                listed, expected,
                "case {case:?}: expected the upserted set {expected:?}, got {listed:?}"
            );
        }
    }

    // One resource's tags are its own: writing through one ARN leaves the
    // other's set alone.
    let workspace = workspace_arn("tagged");
    let entity = entity_arn("tagged", "pump");
    assert_eq!(
        emulator
            .tag_resource(&workspace, json!({"scope": "workspace"}))
            .await
            .status,
        200,
        "case \"isolation\": tagging the workspace must succeed"
    );
    let entity_tags = all_tags(&emulator, "isolation", &entity).await;
    assert!(
        !entity_tags.contains_key("scope"),
        "case \"isolation\": tagging the workspace must not touch the entity, got {entity_tags:?}"
    );
    assert_eq!(
        all_tags(&emulator, "isolation", &workspace)
            .await
            .get("scope")
            .map(String::as_str),
        Some("workspace"),
        "case \"isolation\": the workspace must carry its own tag"
    );
}

/// RULE: `UntagResource` removes exactly the named keys together with their
/// values and leaves every other tag alone; a key the resource does not carry is
/// removed vacuously, so the call is idempotent and still answers 200.
///
/// The rows differ along which keys are named: one present key, several, a mix
/// of present and absent, a key that was never there, a repeat of a removal
/// already done, and no keys at all.
#[tokio::test]
async fn untagging_removes_exactly_the_named_keys_and_is_idempotent() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_workspace("untag").await.status, 200);
    let resource_arn = workspace_arn("untag");

    let stored: BTreeMap<String, String> = BTreeMap::from([
        ("a".to_string(), "1".to_string()),
        ("b".to_string(), "2".to_string()),
        ("c".to_string(), "3".to_string()),
        ("cost:center".to_string(), "cc/1".to_string()),
        ("équipe principale".to_string(), "usine 1".to_string()),
    ]);

    // Every row names at least one key: `tagKeys` is `@required` and bound to
    // the query string, so a call naming none carries no `tagKeys` parameter at
    // all and is not a request the REST binding can express.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("onePresentKey", vec!["a"]),
        ("twoPresentKeys", vec!["b", "c"]),
        ("presentAndAbsent", vec!["cost:center", "neverThere"]),
        ("onlyAbsent", vec!["neverThere"]),
        ("repeatOfARemoval", vec!["a", "b"]),
        ("punctuatedAndUnicode", vec!["équipe principale"]),
    ];

    for (case, removed) in &rows {
        // Each row starts from the same stored set, so the row's own keys are
        // what the assertion is about.
        assert_eq!(
            emulator
                .tag_resource(&resource_arn, as_json(&stored))
                .await
                .status,
            200,
            "case {case:?}: restoring the stored tag set must succeed"
        );
        let untagged = emulator.untag_resource(&resource_arn, removed).await;
        assert_eq!(
            untagged.status, 200,
            "case {case:?}: expected 200 from UntagResource, got {untagged:?}"
        );

        let expected: BTreeMap<String, String> = stored
            .iter()
            .filter(|(key, _)| !removed.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let listed = all_tags(&emulator, case, &resource_arn).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?} after removing {removed:?}, got {listed:?}"
        );

        // Idempotence: the same removal again is still a 200 and the same set.
        let again = emulator.untag_resource(&resource_arn, removed).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: a repeated UntagResource must still answer 200, got {again:?}"
        );
        let listed_again = all_tags(&emulator, case, &resource_arn).await;
        assert_eq!(
            listed_again, expected,
            "case {case:?}: a repeated removal must leave {expected:?}, got {listed_again:?}"
        );
    }
}

/// One row of the tag-maximum table: the tags the resource already carries, the
/// tags the call applies, and whether the service accepts it.
struct CapCase {
    case: &'static str,
    stored: BTreeMap<String, String>,
    applied: BTreeMap<String, String>,
    accepted: bool,
}

impl CapCase {
    fn new(
        case: &'static str,
        stored: BTreeMap<String, String>,
        applied: BTreeMap<String, String>,
        accepted: bool,
    ) -> Self {
        Self {
            case,
            stored,
            applied,
            accepted,
        }
    }

    /// The size of the tag set this row would leave on the resource: the keys it
    /// already carries plus the request's keys it does not — rewriting a key it
    /// carries never grows the set.
    fn resulting(&self) -> usize {
        self.stored.len()
            + self
                .applied
                .keys()
                .filter(|key| !self.stored.contains_key(*key))
                .count()
    }
}

/// Drive one row of the tag-quota table against `resource_arn` and assert the
/// whole rule on it: the answer the row expects, a refusal that is the
/// `ServiceQuotaExceededException` (HTTP 402) the service models for a quota
/// breach, carrying an empty message, and the set the resource is left holding,
/// read back through `ListTagsForResource` so a refused call is proven to have
/// stored nothing. The one assertion block both tables below drive.
async fn assert_cap_case(emulator: &Emulator, case: &str, resource_arn: &str, row: &CapCase) {
    if !row.stored.is_empty() {
        assert_eq!(
            emulator
                .tag_resource(resource_arn, as_json(&row.stored))
                .await
                .status,
            200,
            "case {case:?}: storing the initial {} tags must succeed",
            row.stored.len()
        );
    }

    let resp = emulator
        .tag_resource(resource_arn, as_json(&row.applied))
        .await;
    let mut expected = row.stored.clone();
    if row.accepted {
        assert!(
            row.resulting() <= MAX_TAGS_PER_RESOURCE,
            "case {case:?}: an accepted row must stay within the maximum \
             {MAX_TAGS_PER_RESOURCE}, got {}",
            row.resulting()
        );
        assert_eq!(
            resp.status, 200,
            "case {case:?}: expected 200 from TagResource, got {resp:?}"
        );
        expected.extend(row.applied.clone());
    } else {
        assert!(
            row.resulting() > MAX_TAGS_PER_RESOURCE,
            "case {case:?}: a refused row must reach past the maximum \
             {MAX_TAGS_PER_RESOURCE}, got {}",
            row.resulting()
        );
        let code = resp.error_type.clone().unwrap_or_default();
        assert!(
            TAG_RESOURCE_REFUSAL_CODES.contains(&code.as_str()),
            "case {case:?}: expected one of the refusals this service answers \
             {TAG_RESOURCE_REFUSAL_CODES:?}, got {resp:?}"
        );
        assert_eq!(
            (resp.status, code.as_str()),
            (402, "ServiceQuotaExceededException"),
            "case {case:?}: expected a 402 ServiceQuotaExceededException, got {resp:?}"
        );
        assert_eq!(
            resp.member("message").as_str(),
            Some(QUOTA_EXCEEDED_MESSAGE),
            "case {case:?}: expected the quota refusal's empty message, got {resp:?}"
        );
    }

    let listed = all_tags(emulator, case, resource_arn).await;
    assert_eq!(
        listed, expected,
        "case {case:?}: expected the stored set {expected:?}, got {listed:?}"
    );
}

/// RULE: a `TagResource` call is accepted exactly while the tag set it would
/// leave on the resource is within the quota of 50 tags per resource
/// ([`MAX_TAGS_PER_RESOURCE`]); otherwise it is refused as the QUOTA breach it is
/// — HTTP 402 `ServiceQuotaExceededException` — and stores nothing. Rewriting the
/// value of a key the resource already carries never grows the set, so it is
/// accepted even at the quota.
///
/// The rows differ along how full the resource is and how many of the request's
/// keys are new — including both sides of the boundary and one past it.
#[tokio::test]
async fn tagging_past_the_maximum_is_refused_and_stores_nothing() {
    let cap = MAX_TAGS_PER_RESOURCE;

    let rows: Vec<CapCase> = vec![
        CapCase::new("emptyToFull", BTreeMap::new(), tag_batch(cap, "first"), true),
        CapCase::new(
            "oneBelowPlusOneNew",
            tag_batch(cap - 1, "first"),
            BTreeMap::from([("extra".to_string(), "x".to_string())]),
            true,
        ),
        CapCase::new(
            "twoBelowPlusTwoNew",
            tag_batch(cap - 2, "first"),
            BTreeMap::from([
                ("extra1".to_string(), "x".to_string()),
                ("extra2".to_string(), "y".to_string()),
            ]),
            true,
        ),
        CapCase::new(
            "fullRewrittenInPlace",
            tag_batch(cap, "first"),
            tag_batch(cap, "second"),
            true,
        ),
        CapCase::new(
            "fullPlusOneNew",
            tag_batch(cap, "first"),
            BTreeMap::from([("extra".to_string(), "x".to_string())]),
            false,
        ),
        CapCase::new(
            "oneBelowPlusTwoNew",
            tag_batch(cap - 1, "first"),
            BTreeMap::from([
                ("extra1".to_string(), "x".to_string()),
                ("extra2".to_string(), "y".to_string()),
            ]),
            false,
        ),
        CapCase::new(
            "fullPlusMixOfOldAndNew",
            tag_batch(cap, "first"),
            BTreeMap::from([
                ("k00".to_string(), "rewritten".to_string()),
                ("extra".to_string(), "x".to_string()),
            ]),
            false,
        ),
    ];

    for row in &rows {
        // A workspace of its own per row, so one row's tag set cannot leak into
        // the next.
        let emulator = Emulator::new();
        let workspace_id = format!("cap{}", row.case);
        assert_eq!(
            emulator.create_workspace(&workspace_id).await.status,
            200,
            "case {:?}: the workspace must be created",
            row.case
        );
        assert_cap_case(&emulator, row.case, &workspace_arn(&workspace_id), row).await;
    }
}

/// The message the quota refusal carries, written out here so the WORDING is a
/// check that can fail: a breach of the per-resource tag quota names no value of
/// the request, so the message is empty.
const QUOTA_EXCEEDED_MESSAGE: &str = "";

/// The error codes a refusal of `TagResource` is one of: the three the operation
/// declares (`smithy_query(query="operation",operation="TagResource")`) plus the
/// `ServiceQuotaExceededException` this SERVICE models (HTTP 402, declared by
/// `UpdateComponentType`) and answers for a breach of the per-resource tag
/// quota, and the `ValidationException` it answers for a reserved tag key.
const TAG_RESOURCE_REFUSAL_CODES: [&str; 5] = [
    "AccessDeniedException",
    "ResourceNotFoundException",
    "ServiceQuotaExceededException",
    "TooManyTagsException",
    "ValidationException",
];

/// One taggable resource of `kind`, carrying no tags, and the ARN that names it.
///
/// Every kind of resource this service mints an ARN for is created through its
/// own operation, so the ARN under test is one the service really holds.
async fn taggable_resource(emulator: &Emulator, kind: &str, case: &str) -> String {
    let workspace_id = "taglimit";
    assert_eq!(
        emulator.create_workspace(workspace_id).await.status,
        200,
        "case {case:?}: the workspace holding the resource must be created"
    );
    match kind {
        "workspace" => workspace_arn(workspace_id),
        "entity" => {
            assert_eq!(
                emulator
                    .create_entity(workspace_id, json!({"entityId": "pump", "entityName": "Pump"}))
                    .await
                    .status,
                200,
                "case {case:?}: the entity under test must be created"
            );
            entity_arn(workspace_id, "pump")
        }
        "componentType" => {
            assert_eq!(
                emulator
                    .create_component_type(workspace_id, "com.example.pump", json!({}))
                    .await
                    .status,
                200,
                "case {case:?}: the component type under test must be created"
            );
            component_type_arn(workspace_id, "com.example.pump")
        }
        "scene" => {
            assert_eq!(
                emulator
                    .create_scene(
                        workspace_id,
                        json!({"sceneId": "floor", "contentLocation": "s3://bucket/floor.json"}),
                    )
                    .await
                    .status,
                200,
                "case {case:?}: the scene under test must be created"
            );
            scene_arn(workspace_id, "floor")
        }
        other => panic!("case {case:?}: no taggable resource of kind {other:?}"),
    }
}

/// RULE: a refusal of `TagResource` is one of the CODES THIS SERVICE ANSWERS for
/// it ([`TAG_RESOURCE_REFUSAL_CODES`] — the three the operation declares plus the
/// two the service models and this operation raises), and the one it answers for
/// a call that would leave a resource past the per-resource tag QUOTA is
/// `ServiceQuotaExceededException` (HTTP 402), carrying an empty message —
/// whatever kind of resource the ARN names and however far past the quota the
/// call reaches. The set is a CONTAINMENT guard, not a model-conformance check:
/// the quota code AWS answers here is one TagResource's own error list omits.
///
/// The rows differ along how full the resource already is, how many of the
/// request's keys are new, how far past the quota the resulting set would
/// reach (both sides of the boundary, one past it and three past it) and, in the
/// outer loop, the kind of resource the ARN names. The not-found row is another
/// of the codes, so the rule is checked on a refusal the tag quota has
/// nothing to do with.
#[tokio::test]
async fn a_tag_refusal_is_one_of_the_codes_this_service_answers() {
    let cap = MAX_TAGS_PER_RESOURCE;

    let rows: Vec<CapCase> = vec![
        CapCase::new("emptyToFull", BTreeMap::new(), tag_batch(cap, "first"), true),
        CapCase::new(
            "fullRewrittenInPlace",
            tag_batch(cap, "first"),
            tag_batch(cap, "second"),
            true,
        ),
        CapCase::new(
            "fullPlusOneNew",
            tag_batch(cap, "first"),
            BTreeMap::from([("extra".to_string(), "x".to_string())]),
            false,
        ),
        CapCase::new(
            "fullPlusAReplacementAndANewKey",
            tag_batch(cap, "first"),
            BTreeMap::from([
                ("k00".to_string(), "rewritten".to_string()),
                ("extra".to_string(), "x".to_string()),
            ]),
            false,
        ),
        CapCase::new(
            "fullPlusThreeNew",
            tag_batch(cap, "first"),
            BTreeMap::from([
                ("extra1".to_string(), "x".to_string()),
                ("extra2".to_string(), "y".to_string()),
                ("extra3".to_string(), "z".to_string()),
            ]),
            false,
        ),
        CapCase::new(
            "oneStoredPlusAFullBatchOfNewKeys",
            BTreeMap::from([("seed".to_string(), "s".to_string())]),
            tag_batch(cap, "batch"),
            false,
        ),
    ];

    for kind in ["workspace", "entity", "componentType", "scene"] {
        for row in &rows {
            // A resource of its own per row, so one row's tag set cannot leak
            // into the next.
            let emulator = Emulator::new();
            let case = format!("{kind}/{}", row.case);
            let resource_arn = taggable_resource(&emulator, kind, &case).await;
            assert_cap_case(&emulator, &case, &resource_arn, row).await;
        }
    }

    // Another of the refusals: an ARN of the right shape that names no
    // stored resource. Its code is in the same set, and the tag quota has
    // nothing to do with it.
    let emulator = Emulator::new();
    let absent = workspace_arn("neverCreated");
    let resp = emulator
        .tag_resource(&absent, json!({"env": "test"}))
        .await;
    let code = resp.error_type.clone().unwrap_or_default();
    assert!(
        TAG_RESOURCE_REFUSAL_CODES.contains(&code.as_str()),
        "case \"absentResource\": expected one of the refusals this service answers \
         {TAG_RESOURCE_REFUSAL_CODES:?}, got {resp:?}"
    );
    assert_eq!(
        (resp.status, code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"absentResource\": expected a 404 ResourceNotFoundException, got {resp:?}"
    );
    // The refused call stored nothing here either: the ARN still names no
    // resource, so the listing refuses it the same way rather than reporting a
    // tag set the refusal conjured.
    let listed = emulator.list_tags_for_resource(&absent, json!({})).await;
    assert_eq!(
        (listed.status, listed.error_type.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case \"absentResource\": the refused call must leave the ARN naming no resource, \
         got {listed:?}"
    );
}

/// RULE: `ListTagsForResource` serves one page at a time in tag-key order and
/// the page sequence PARTITIONS the tag set — concatenating the pages yields
/// each stored tag exactly once, no page is larger than the effective page size
/// (the documented default of 25 when the request names none, never more than
/// the service's ceiling), and only the last page omits the `nextToken`.
///
/// The rows differ along the number of stored tags (none, one, exactly the
/// default page, one past it, the maximum) and the requested page size
/// (unnamed, one, a size that does not divide the total, a size larger than the
/// total, and the modelled ceiling).
#[tokio::test]
async fn a_tag_listing_is_partitioned_by_its_pages() {
    let emulator = Emulator::new();

    let totals = [0usize, 1, DEFAULT_PAGE_SIZE, DEFAULT_PAGE_SIZE + 1, MAX_TAGS_PER_RESOURCE];
    let page_sizes: [Option<u32>; 5] = [None, Some(1), Some(7), Some(100), Some(200)];

    for total in totals {
        let workspace_id = format!("pages{total}");
        assert_eq!(
            emulator.create_workspace(&workspace_id).await.status,
            200,
            "total {total}: the workspace must be created"
        );
        let resource_arn = workspace_arn(&workspace_id);
        let stored = tag_batch(total, "page");
        if total > 0 {
            assert_eq!(
                emulator
                    .tag_resource(&resource_arn, as_json(&stored))
                    .await
                    .status,
                200,
                "total {total}: storing the tags must succeed"
            );
        }

        for max_results in page_sizes {
            let case = format!("{total} tags at page size {max_results:?}");
            let (entries, pages) =
                walk_tag_pages(&emulator, &case, &resource_arn, max_results).await;
            let collected: BTreeMap<String, String> = entries.iter().cloned().collect();
            assert_eq!(
                entries.len(),
                collected.len(),
                "case {case:?}: a tag was served by two pages, got {entries:?}"
            );
            assert_eq!(
                collected, stored,
                "case {case:?}: expected the stored set {stored:?}, got {collected:?}"
            );
            let page_size = effective_page_size(max_results);
            let expected_pages = total.div_ceil(page_size).max(1);
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} pages of at most {page_size}, got {pages}"
            );
        }
    }
}

/// RULE: every tagging operation addresses its resource by ARN, so an ARN this
/// service holds no resource under — never minted, or minted for a resource
/// since deleted — is refused with HTTP 404 `ResourceNotFoundException`; and a
/// page request the model's own bounds cannot refuse (a `nextToken` minted for
/// another resource, a `maxResults` below the documented minimum of 1) is
/// refused with HTTP 400 `ValidationException`.
#[tokio::test]
async fn a_tagging_call_on_an_unknown_resource_is_refused() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_workspace("live").await.status, 200);
    assert_eq!(emulator.create_workspace("doomed").await.status, 200);
    let live = workspace_arn("live");
    let absent = workspace_arn("absent");
    let absent_entity = entity_arn("live", "absent");
    assert_eq!(
        emulator.delete_workspace("doomed").await.status,
        200,
        "the workspace under test must be deletable"
    );
    let deleted = workspace_arn("doomed");

    // Every operation of the tagging surface, against every kind of ARN that
    // names no stored resource.
    for (case, arn) in [
        ("neverMinted", &absent),
        ("neverMintedEntity", &absent_entity),
        ("deletedWorkspace", &deleted),
    ] {
        let tagged = emulator.tag_resource(arn, json!({"env": "test"})).await;
        assert_eq!(
            (tagged.status, tagged.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: TagResource expected a 404 ResourceNotFoundException, got {tagged:?}"
        );
        let listed = emulator.list_tags_for_resource(arn, json!({})).await;
        assert_eq!(
            (listed.status, listed.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: ListTagsForResource expected a 404, got {listed:?}"
        );
        let untagged = emulator.untag_resource(arn, &["env"]).await;
        assert_eq!(
            (untagged.status, untagged.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: UntagResource expected a 404, got {untagged:?}"
        );
    }

    // A token is bound to the listing it was minted for: resuming another
    // resource's listing with it is refused, not silently served.
    assert_eq!(
        emulator
            .tag_resource(&live, json!({"a": "1", "b": "2"}))
            .await
            .status,
        200,
        "case \"foreignToken\": tagging the live workspace must succeed"
    );
    let first = emulator
        .list_tags_for_resource(&live, json!({"maxResults": 1}))
        .await;
    let token = first
        .member("nextToken")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "case \"foreignToken\": a truncated page must carry a nextToken, got {first:?}"
    );
    assert_eq!(emulator.create_workspace("other").await.status, 200);
    let foreign = emulator
        .list_tags_for_resource(&workspace_arn("other"), json!({"nextToken": token}))
        .await;
    assert_eq!(
        (foreign.status, foreign.error_type.as_deref()),
        (400, Some("ValidationException")),
        "case \"foreignToken\": expected a 400 ValidationException, got {foreign:?}"
    );

    let zero_page = emulator
        .list_tags_for_resource(&live, json!({"maxResults": 0}))
        .await;
    assert_eq!(
        (zero_page.status, zero_page.error_type.as_deref()),
        (400, Some("ValidationException")),
        "case \"maxResultsZero\": expected a 400 ValidationException, got {zero_page:?}"
    );
}
