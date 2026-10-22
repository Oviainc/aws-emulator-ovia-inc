// SPDX-License-Identifier: CC0-1.0

//! The source-view, tagging and resource-policy surface, end to end over the
//! awsJson1_0 wire.
//!
//! Each test pins the RULE the operations follow, over a table of inputs that spans
//! the class the rule generalises over (how many sources are associated, how deep
//! the source chain runs, how many tags and of what size, which billing-view type
//! owns the ARN, which page size a paginated read uses) — never one recorded
//! request:
//!
//! * `AssociateSourceViews` adds source views to the stored record of an existing
//!   CUSTOM view, and refuses any request that would put a view in its own source
//!   chain (400 `ValidationException`) without storing anything.
//! * `ListSourceViewsForBillingView` reports that record, so an ARN no view is
//!   stored under is 404 `ResourceNotFoundException` and an empty list means
//!   "exists, draws from nothing".
//! * `TagResource`/`ListTagsForResource` read and write the view's tag set, refusing
//!   a managed view with 400 `ValidationException` and an unknown ARN with 404
//!   `ResourceNotFoundException`.
//! * `GetResourcePolicy` reports the policy document stored on a CUSTOM view and
//!   answers 404 `ResourceNotFoundException` when there is none.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::billing_views::{call, emulator, emulator_with_engine, error_code, ACCOUNT_ID, PRIMARY_ARN, ABSENT_ARN};

/// Create one CUSTOM billing view drawing from `sources` and return its ARN.
async fn create_view(router: &MultiServiceRouter, name: &str, sources: &[String]) -> String {
    let (status, body) = call(
        router,
        "CreateBillingView",
        &json!({ "name": name, "sourceViews": sources }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected CreateBillingView to answer 200, got {status} {body:?}"
    );
    body["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateBillingView answered no arn: {body:?}"))
        .to_string()
}

/// The `sourceViewCount` `GetBillingView` derives for `arn`.
async fn source_view_count(router: &MultiServiceRouter, arn: &str) -> u64 {
    let (status, body) = call(router, "GetBillingView", &json!({ "arn": arn })).await;
    assert_eq!(
        status, 200,
        "GetBillingView({arn}) must answer 200, got {status} {body:?}"
    );
    body["billingView"]["sourceViewCount"]
        .as_u64()
        .unwrap_or_else(|| panic!("GetBillingView({arn}) reported no sourceViewCount: {body:?}"))
}

/// The `derivedViewCount` `GetBillingView` derives for `arn`.
async fn derived_view_count(router: &MultiServiceRouter, arn: &str) -> u64 {
    let (status, body) = call(router, "GetBillingView", &json!({ "arn": arn })).await;
    assert_eq!(
        status, 200,
        "GetBillingView({arn}) must answer 200, got {status} {body:?}"
    );
    body["billingView"]["derivedViewCount"]
        .as_u64()
        .unwrap_or_else(|| panic!("GetBillingView({arn}) reported no derivedViewCount: {body:?}"))
}

/// Every source view `ListSourceViewsForBillingView` reports for `arn`, walking
/// `nextToken` to the end with `max_results` per page (or one page when `None`),
/// sorted so an assertion is about the SET the view holds rather than the order
/// the operation reports it in.
async fn list_source_views(
    router: &MultiServiceRouter,
    arn: &str,
    max_results: Option<u32>,
) -> Vec<String> {
    let mut collected = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut body = json!({ "arn": arn });
        if let Some(max) = max_results {
            body["maxResults"] = json!(max);
        }
        if let Some(token) = &next_token {
            body["nextToken"] = json!(token);
        }
        let (status, page) = call(router, "ListSourceViewsForBillingView", &body).await;
        assert_eq!(
            status, 200,
            "ListSourceViewsForBillingView({arn}) must answer 200, got {status} {page:?}"
        );
        collected.extend(
            page["sourceViews"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("ListSourceViewsForBillingView({arn}) answered no sourceViews: {page:?}")
                })
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string()),
        );
        next_token = page["nextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            collected.sort();
            return collected;
        }
        assert!(
            collected.len() <= 64,
            "ListSourceViewsForBillingView({arn}) kept handing out tokens: {collected:?}"
        );
    }
}

/// The tags `ListTagsForResource` reports for `arn`, as a key -> value map.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let (status, body) = call(router, "ListTagsForResource", &json!({ "resourceArn": arn })).await;
    assert_eq!(
        status, 200,
        "ListTagsForResource({arn}) must answer 200, got {status} {body:?}"
    );
    body["resourceTags"]
        .as_array()
        .unwrap_or_else(|| panic!("ListTagsForResource({arn}) answered no resourceTags: {body:?}"))
        .iter()
        .map(|tag| {
            (
                tag["key"].as_str().unwrap_or_default().to_string(),
                tag["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// A pattern-valid billing-view ARN with resource id `id`.
fn view_arn(id: &str) -> String {
    format!("arn:aws:billing::{ACCOUNT_ID}:billingview/{id}")
}

/// One row of the tag round-trip table: the case name, the tags the create
/// stored, and the tags a later `TagResource` attaches.
type TagRow = (&'static str, Vec<(String, String)>, Vec<(String, String)>);

#[tokio::test]
async fn associating_sources_is_reported_by_both_reads_for_every_count() {
    let router = emulator();

    // The rows differ along how many sources one request associates — one, a
    // handful, and the modeled `BillingViewSourceViewsList` maximum of 10 minus
    // the source the create already stored — and along whether the request
    // repeats a source the view already draws from.
    for (case, added, repeat_previous) in [
        ("one source", 1_usize, false),
        ("three sources", 3, false),
        ("three sources, two already associated", 3, true),
        ("nine sources, filling the per-view maximum", 9, false),
    ] {
        let target = create_view(&router, "target", &[PRIMARY_ARN.to_string()]).await;
        let mut sources = Vec::new();
        for index in 0..added {
            sources.push(create_view(&router, &format!("src-{index}"), &[PRIMARY_ARN.to_string()]).await);
        }

        if repeat_previous {
            let first_two: Vec<String> = sources.iter().take(2).cloned().collect();
            let (status, body) = call(
                &router,
                "AssociateSourceViews",
                &json!({ "arn": target, "sourceViews": first_two }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the first associate must answer 200, got {status} {body:?}"
            );
        }

        let (status, body) = call(
            &router,
            "AssociateSourceViews",
            &json!({ "arn": target, "sourceViews": sources }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected AssociateSourceViews to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            body["arn"].as_str(),
            Some(target.as_str()),
            "case {case:?}: the response must name the view that was updated, got {:?}",
            body["arn"]
        );

        // The association is a set: the create's own source plus each associated
        // one, counted once however many requests carried it.
        let mut expected: Vec<String> = sources.clone();
        expected.push(PRIMARY_ARN.to_string());
        expected.sort();

        let listed = list_source_views(&router, &target, None).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: ListSourceViewsForBillingView must report exactly the stored sources, got {listed:?}"
        );
        let count = source_view_count(&router, &target).await;
        assert_eq!(
            count,
            expected.len() as u64,
            "case {case:?}: sourceViewCount must count the stored sources, got {count}"
        );
        for source in &sources {
            let derived = derived_view_count(&router, source).await;
            assert_eq!(
                derived, 1,
                "case {case:?}: {source} is now a source of one view, got derivedViewCount {derived}"
            );
        }

        // Replaying the same request stores nothing new.
        let (status, _) = call(
            &router,
            "AssociateSourceViews",
            &json!({ "arn": target, "sourceViews": sources }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a replayed associate must answer 200, got {status}"
        );
        let replayed = list_source_views(&router, &target, None).await;
        assert_eq!(
            replayed, expected,
            "case {case:?}: a replayed associate must store nothing new, got {replayed:?}"
        );
    }
}

#[tokio::test]
async fn a_view_can_never_end_up_in_its_own_source_chain() {
    let router = emulator();

    // A chain of views, each drawing from the one before it:
    // chain[0] <- chain[1] <- chain[2] <- chain[3].
    let mut chain = vec![create_view(&router, "chain-0", &[PRIMARY_ARN.to_string()]).await];
    for index in 1..4 {
        let previous = chain[index - 1].clone();
        chain.push(create_view(&router, &format!("chain-{index}"), &[previous]).await);
    }
    let unrelated = create_view(&router, "unrelated", &[PRIMARY_ARN.to_string()]).await;

    // The rows span the shapes the rule covers: a view named as its own source
    // (alone, and alongside legal sources), and a source one, two or three links
    // down the chain that already draws from the target.
    let cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "the primary view associated with itself",
            PRIMARY_ARN.to_string(),
            vec![PRIMARY_ARN.to_string()],
        ),
        (
            "a custom view associated with itself",
            chain[0].clone(),
            vec![chain[0].clone()],
        ),
        (
            "a custom view among other, legal sources",
            chain[0].clone(),
            vec![unrelated.clone(), chain[0].clone()],
        ),
        (
            "a direct derived view as a source",
            chain[0].clone(),
            vec![chain[1].clone()],
        ),
        (
            "a derived view two links down",
            chain[0].clone(),
            vec![chain[2].clone()],
        ),
        (
            "a derived view three links down, after a legal source",
            chain[0].clone(),
            vec![unrelated.clone(), chain[3].clone()],
        ),
    ];

    for (case, arn, source_views) in cases {
        let before = source_view_count(&router, &arn).await;
        let (status, body) = call(
            &router,
            "AssociateSourceViews",
            &json!({ "arn": arn, "sourceViews": source_views }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
        let after = source_view_count(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused associate must change no source: {before} -> {after}"
        );
    }
}

#[tokio::test]
async fn associate_refuses_an_unknown_view_and_a_managed_target() {
    let router = emulator();
    let custom = create_view(&router, "custom", &[PRIMARY_ARN.to_string()]).await;

    // An ARN naming no stored view is the modeled 404, whether it arrives as the
    // target or as one of the sources.
    for (case, arn, source_views) in [
        (
            "unknown target",
            ABSENT_ARN.to_string(),
            vec![custom.clone()],
        ),
        (
            "unknown target, unknown source",
            view_arn("nope-1"),
            vec![view_arn("nope-2")],
        ),
        (
            "known target, unknown source",
            custom.clone(),
            vec![view_arn("nope-3")],
        ),
        (
            "known target, one known and one unknown source",
            custom.clone(),
            vec![PRIMARY_ARN.to_string(), view_arn("nope-4")],
        ),
    ] {
        let (status, body) = call(
            &router,
            "AssociateSourceViews",
            &json!({ "arn": arn, "sourceViews": source_views }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }
    let sources = list_source_views(&router, &custom, None).await;
    assert_eq!(
        sources,
        vec![PRIMARY_ARN.to_string()],
        "a refused associate must store no source, got {sources:?}"
    );

    // The PRIMARY view is AWS-managed: its definition, and so the set of views it
    // aggregates, is not the customer's to change.
    let (status, body) = call(
        &router,
        "AssociateSourceViews",
        &json!({ "arn": PRIMARY_ARN, "sourceViews": [custom] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"managed target\": expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"managed target\": expected ValidationException, got {body:?}"
    );
    let primary_sources = list_source_views(&router, PRIMARY_ARN, None).await;
    assert!(
        primary_sources.is_empty(),
        "case \"managed target\": the managed view must keep drawing from nothing, got {primary_sources:?}"
    );
}

#[tokio::test]
async fn a_view_cannot_be_pushed_past_the_modeled_source_view_maximum() {
    let router = emulator();
    // `BillingViewSourceViewsList` is `@length` 1..10, so a view draws from at
    // most ten sources; the create already stored one.
    let target = create_view(&router, "target", &[PRIMARY_ARN.to_string()]).await;
    let mut sources = Vec::new();
    for index in 0..10 {
        sources.push(create_view(&router, &format!("src-{index}"), &[PRIMARY_ARN.to_string()]).await);
    }

    let fits: Vec<String> = sources.iter().take(9).cloned().collect();
    let (status, body) = call(
        &router,
        "AssociateSourceViews",
        &json!({ "arn": target, "sourceViews": fits }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"up to the maximum\": expected 200, got {status} {body:?}"
    );
    let count = source_view_count(&router, &target).await;
    assert_eq!(
        count, 10,
        "case \"up to the maximum\": expected 10 stored sources, got {count}"
    );

    let (status, body) = call(
        &router,
        "AssociateSourceViews",
        &json!({ "arn": target, "sourceViews": [sources[9].clone()] }),
    )
    .await;
    assert_eq!(
        status, 402,
        "case \"one past the maximum\": expected HTTP 402, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case \"one past the maximum\": expected ServiceQuotaExceededException, got {body:?}"
    );
    let count = source_view_count(&router, &target).await;
    assert_eq!(
        count, 10,
        "case \"one past the maximum\": the refusal must store nothing, got {count}"
    );
}

#[tokio::test]
async fn list_source_views_tells_a_missing_view_from_one_with_no_sources() {
    let router = emulator();
    let with_sources = create_view(&router, "with-sources", &[PRIMARY_ARN.to_string()]).await;

    // An ARN no view is stored under: the modeled 404, never a 200 with an empty
    // list. The rows differ in the resource id's shape, including the maximum the
    // `BillingViewArn` pattern allows.
    for (case, arn) in [
        ("the recorded absent arn", ABSENT_ARN.to_string()),
        ("a short absent id", view_arn("x")),
        ("an absent id with every allowed character", view_arn("a_Z9/:+=.-@b")),
        (
            "an absent id at the pattern's length maximum",
            view_arn(&format!("{}z", "a".repeat(75))),
        ),
        (
            "an absent id that looks like a minted one",
            view_arn("custom-999999"),
        ),
    ] {
        let (status, body) = call(
            &router,
            "ListSourceViewsForBillingView",
            &json!({ "arn": arn }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }

    // A view that exists and draws from nothing: a 200 with an empty list, which
    // is the answer the 404 above must stay distinguishable from.
    let (status, body) = call(
        &router,
        "ListSourceViewsForBillingView",
        &json!({ "arn": PRIMARY_ARN }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a view with no sources\": expected HTTP 200, got {status} {body:?}"
    );
    assert_eq!(
        body["sourceViews"],
        json!([]),
        "case \"a view with no sources\": expected an empty list, got {:?}",
        body["sourceViews"]
    );
    assert!(
        body["nextToken"].is_null(),
        "case \"a view with no sources\": a single page carries no nextToken, got {:?}",
        body["nextToken"]
    );

    let listed = list_source_views(&router, &with_sources, None).await;
    assert_eq!(
        listed,
        vec![PRIMARY_ARN.to_string()],
        "case \"a view with one source\": expected the stored source, got {listed:?}"
    );
}

#[tokio::test]
async fn source_view_pagination_returns_every_source_once_for_every_page_size() {
    let router = emulator();
    let target = create_view(&router, "target", &[PRIMARY_ARN.to_string()]).await;
    let mut sources = vec![PRIMARY_ARN.to_string()];
    for index in 0..5 {
        sources.push(create_view(&router, &format!("src-{index}"), &[PRIMARY_ARN.to_string()]).await);
    }
    let added: Vec<String> = sources.iter().skip(1).cloned().collect();
    let (status, body) = call(
        &router,
        "AssociateSourceViews",
        &json!({ "arn": target, "sourceViews": added }),
    )
    .await;
    assert_eq!(status, 200, "the associate must answer 200, got {status} {body:?}");

    let mut expected = sources.clone();
    expected.sort();

    // Every page size from one entry to one past the whole set walks the same
    // sources, once each, in the same order.
    for page_size in 1..=(expected.len() + 1) {
        let walked = list_source_views(&router, &target, Some(page_size as u32)).await;
        assert_eq!(
            walked, expected,
            "case \"maxResults {page_size}\": the walk must return every source exactly once, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn tagging_a_custom_view_round_trips_through_list_tags() {
    let router = emulator();
    let long_key = "k".repeat(128); // ResourceTagKey is @length 1..128.
    let long_value = "v".repeat(256); // ResourceTagValue is @length 0..256.

    // The rows differ along tag count (none, one, several), value shape (ordinary,
    // empty, at the modeled maximum), key length (ordinary, at the modeled
    // maximum) and whether the request replaces a key the view already carries.
    let cases: Vec<TagRow> = vec![
        ("no tags at create, one attached", vec![], vec![("team".to_string(), "finance".to_string())]),
        (
            "no tags attached at all",
            vec![],
            vec![],
        ),
        (
            "three tags attached at once",
            vec![],
            vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "2".to_string()),
                ("c".to_string(), "3".to_string()),
            ],
        ),
        (
            "an empty value and a value at the modeled maximum",
            vec![],
            vec![
                ("empty".to_string(), String::new()),
                ("long".to_string(), long_value.clone()),
            ],
        ),
        (
            "a key at the modeled maximum length",
            vec![],
            vec![(long_key.clone(), "ok".to_string())],
        ),
        (
            "a key the create already stored is replaced",
            vec![("env".to_string(), "dev".to_string())],
            vec![("env".to_string(), "prod".to_string())],
        ),
        (
            "one key replaced, one added",
            vec![
                ("env".to_string(), "dev".to_string()),
                ("owner".to_string(), "alice".to_string()),
            ],
            vec![
                ("env".to_string(), "prod".to_string()),
                ("cost-centre".to_string(), "42".to_string()),
            ],
        ),
    ];

    for (case, created_with, attached) in cases {
        let (status, created) = call(
            &router,
            "CreateBillingView",
            &json!({
                "name": "tagged",
                "sourceViews": [PRIMARY_ARN],
                "resourceTags": created_with
                    .iter()
                    .map(|(key, value)| json!({ "key": key, "value": value }))
                    .collect::<Vec<Value>>(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the create must answer 200, got {status} {created:?}"
        );
        let arn = created["arn"].as_str().unwrap_or_default().to_string();

        let tags = list_tags(&router, &arn).await;
        let expected: BTreeMap<String, String> = created_with.iter().cloned().collect();
        assert_eq!(
            tags, expected,
            "case {case:?}: ListTagsForResource must report the tags the create stored, got {tags:?}"
        );

        let (status, body) = call(
            &router,
            "TagResource",
            &json!({
                "resourceArn": arn,
                "resourceTags": attached
                    .iter()
                    .map(|(key, value)| json!({ "key": key, "value": value }))
                    .collect::<Vec<Value>>(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected TagResource to answer 200, got {status} {body:?}"
        );

        // A tag whose key is already on the view replaces that key's value; every
        // other key the view carries stays.
        let mut merged = expected.clone();
        merged.extend(attached.iter().cloned());
        let tags = list_tags(&router, &arn).await;
        assert_eq!(
            tags, merged,
            "case {case:?}: expected the merged tag set, got {tags:?}"
        );

        // Replaying the request is a no-op.
        let (status, _) = call(
            &router,
            "TagResource",
            &json!({
                "resourceArn": arn,
                "resourceTags": attached
                    .iter()
                    .map(|(key, value)| json!({ "key": key, "value": value }))
                    .collect::<Vec<Value>>(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a replayed TagResource must answer 200, got {status}"
        );
        let tags = list_tags(&router, &arn).await;
        assert_eq!(
            tags, merged,
            "case {case:?}: a replayed TagResource must change nothing, got {tags:?}"
        );
    }
}

#[tokio::test]
async fn the_tag_store_is_bounded_per_resource() {
    let router = emulator();
    let arn = create_view(&router, "bounded", &[PRIMARY_ARN.to_string()]).await;

    // `MAX_TAGS_PER_RESOURCE` is 200, the figure `ResourceTagList`'s `@length`
    // names; one request may carry at most that many, so the cap is reached in
    // one call and crossed by the next.
    let at_cap: Vec<Value> = (0..200)
        .map(|index| json!({ "key": format!("key-{index:03}"), "value": "v" }))
        .collect();
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "resourceArn": arn, "resourceTags": at_cap }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"at the cap\": expected 200, got {status} {body:?}"
    );
    let tags = list_tags(&router, &arn).await;
    assert_eq!(
        tags.len(),
        200,
        "case \"at the cap\": expected 200 stored tags, got {}",
        tags.len()
    );

    // A request that only replaces keys the resource already holds does not
    // grow the set, so it is still accepted at the cap.
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "resourceArn": arn, "resourceTags": [{ "key": "key-000", "value": "replaced" }] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"replacing a key at the cap\": expected 200, got {status} {body:?}"
    );
    let tags = list_tags(&router, &arn).await;
    assert_eq!(
        tags.get("key-000").map(String::as_str),
        Some("replaced"),
        "case \"replacing a key at the cap\": expected the new value, got {:?}",
        tags.get("key-000")
    );
    assert_eq!(
        tags.len(),
        200,
        "case \"replacing a key at the cap\": the set must not grow, got {}",
        tags.len()
    );

    // One fresh key past the cap is refused, and attaches nothing.
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "resourceArn": arn, "resourceTags": [{ "key": "one-too-many", "value": "v" }] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one past the cap\": expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"one past the cap\": expected ValidationException, got {body:?}"
    );
    let tags = list_tags(&router, &arn).await;
    assert_eq!(
        tags.len(),
        200,
        "case \"one past the cap\": the refusal must attach nothing, got {}",
        tags.len()
    );
    assert!(
        !tags.contains_key("one-too-many"),
        "case \"one past the cap\": the refused key must not be stored, got {tags:?}"
    );
}

#[tokio::test]
async fn tagging_refuses_a_managed_view_and_an_unknown_resource() {
    let (router, engine) = emulator_with_engine();
    let custom = create_view(&router, "custom", &[PRIMARY_ARN.to_string()]).await;

    // A managed view is not taggable, in either direction of the tag API: the
    // modeled 400, not a 404 and not an empty tag list.
    for operation in ["TagResource", "ListTagsForResource"] {
        let mut body = json!({ "resourceArn": PRIMARY_ARN });
        if operation == "TagResource" {
            body["resourceTags"] = json!([{ "key": "k", "value": "v" }]);
        }
        let (status, answer) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case \"managed view, {operation}\": expected HTTP 400, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case \"managed view, {operation}\": expected ValidationException, got {answer:?}"
        );
    }

    // An ARN naming no stored resource is the modeled 404, for both operations
    // and for every shape of absent id.
    for (case, arn) in [
        ("the recorded absent arn", ABSENT_ARN.to_string()),
        (
            "an absent id in the minted form",
            view_arn("custom-a1b2c3d4-e5f6-47a8-9b0c-d1e2f3a4b5c6"),
        ),
        ("a one-character absent id", view_arn("q")),
        ("an absent non-billingview resource", format!("arn:aws:billing::{ACCOUNT_ID}:budget/none")),
    ] {
        for operation in ["TagResource", "ListTagsForResource"] {
            let mut body = json!({ "resourceArn": arn });
            if operation == "TagResource" {
                body["resourceTags"] = json!([{ "key": "k", "value": "v" }]);
            }
            let (status, answer) = call(&router, operation, &body).await;
            assert_eq!(
                status, 404,
                "case {case:?} ({operation}): expected HTTP 404, got {status} {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {case:?} ({operation}): expected ResourceNotFoundException, got {answer:?}"
            );
        }
    }

    // One request may not carry the same key twice: the stored value would depend
    // on arrival order.
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({
            "resourceArn": custom,
            "resourceTags": [
                { "key": "env", "value": "dev" },
                { "key": "env", "value": "prod" },
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a duplicated tag key\": expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"a duplicated tag key\": expected ValidationException, got {body:?}"
    );

    // Every refusal above attached nothing — including the managed view, whose
    // tag set no read can report.
    let tags = list_tags(&router, &custom).await;
    assert!(
        tags.is_empty(),
        "a refused TagResource must attach no tag, got {tags:?}"
    );
    let state = service_com_amazonaws_billing_awsbilling::state::service_state(&*engine)
        .expect("the billing state is reachable from the shared engine");
    let state = state.lock();
    let primary = state
        .billing_views
        .get(PRIMARY_ARN)
        .expect("the account's PRIMARY view exists before any call");
    assert!(
        primary.tags.is_empty(),
        "a refused TagResource must attach no tag to the managed view, got {:?}",
        primary.tags
    );
}

#[tokio::test]
async fn get_resource_policy_reports_a_stored_policy_and_404s_without_one() {
    let (router, engine) = emulator_with_engine();
    let unshared = create_view(&router, "unshared", &[PRIMARY_ARN.to_string()]).await;

    // No policy document is stored: the modeled 404, for a managed view that
    // resolves through GetBillingView just as much as for an absent ARN.
    for (case, arn) in [
        ("the managed PRIMARY view", PRIMARY_ARN.to_string()),
        ("a custom view carrying no policy", unshared.clone()),
        ("an absent billing view", ABSENT_ARN.to_string()),
        (
            "an absent non-billingview resource",
            format!("arn:aws:billing::{ACCOUNT_ID}:budget/none"),
        ),
    ] {
        let (status, body) = call(&router, "GetResourcePolicy", &json!({ "resourceArn": arn })).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }
    // The same ARN still resolves through GetBillingView: the 404 above is about
    // the policy, not about the view.
    let (status, _) = call(&router, "GetBillingView", &json!({ "arn": PRIMARY_ARN })).await;
    assert_eq!(
        status, 200,
        "GetBillingView must still resolve the managed view, got {status}"
    );

    // A CUSTOM view that does carry a policy document reports exactly the stored
    // text. This service models no operation that attaches one (there is no
    // PutResourcePolicy), so the store is written directly, the way a sharing
    // mechanism outside this API would.
    let documents = [
        ("a minimal document", r#"{"Version":"2012-10-17","Statement":[]}"#),
        (
            "a statement naming a principal",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"000000000000"},"Action":"billing:GetBillingView","Resource":"*"}]}"#,
        ),
        ("a document with newlines", "{\n  \"Version\": \"2012-10-17\"\n}"),
    ];
    for (case, document) in documents {
        let target = create_view(&router, "shared", &[PRIMARY_ARN.to_string()]).await;
        {
            let state = service_com_amazonaws_billing_awsbilling::state::service_state(&*engine)
                .expect("the billing state is reachable from the shared engine");
            let mut state = state.lock();
            state
                .billing_views
                .get_mut(&target)
                .expect("the view just created is stored")
                .resource_policy = Some(document.to_string());
        }

        let (status, body) = call(
            &router,
            "GetResourcePolicy",
            &json!({ "resourceArn": target }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body:?}"
        );
        assert_eq!(
            body["resourceArn"].as_str(),
            Some(target.as_str()),
            "case {case:?}: the response must echo the resource it answered for, got {:?}",
            body["resourceArn"]
        );
        assert_eq!(
            body["policy"].as_str(),
            Some(document),
            "case {case:?}: expected the stored policy document, got {:?}",
            body["policy"]
        );
    }
}

#[tokio::test]
async fn the_new_reads_change_nothing_for_a_later_caller() {
    let (router, engine) = emulator_with_engine();
    let target = create_view(&router, "target", &[PRIMARY_ARN.to_string()]).await;
    let (status, _) = call(
        &router,
        "TagResource",
        &json!({ "resourceArn": target, "resourceTags": [{ "key": "env", "value": "prod" }] }),
    )
    .await;
    assert_eq!(status, 200, "the tag must be attached, got {status}");

    let snapshot = |engine: &Arc<engine::InMemoryEngine>| {
        let state = service_com_amazonaws_billing_awsbilling::state::service_state(&**engine)
            .expect("the billing state is reachable from the shared engine");
        let state = state.lock();
        format!("{:?}", state.billing_views)
    };
    let before = snapshot(&engine);

    // Each read, called twice, answers the same thing and leaves the store as it
    // was: these operations report, they do not mutate.
    for (case, operation, body) in [
        (
            "ListSourceViewsForBillingView",
            "ListSourceViewsForBillingView",
            json!({ "arn": target }),
        ),
        (
            "ListTagsForResource",
            "ListTagsForResource",
            json!({ "resourceArn": target }),
        ),
        (
            "GetResourcePolicy",
            "GetResourcePolicy",
            json!({ "resourceArn": target }),
        ),
    ] {
        let first = call(&router, operation, &body).await;
        let second = call(&router, operation, &body).await;
        assert_eq!(
            first, second,
            "case {case:?}: two identical reads must answer the same, got {first:?} then {second:?}"
        );
    }

    let after = snapshot(&engine);
    assert_eq!(
        before, after,
        "the reads must leave the store untouched:\nbefore {before}\nafter  {after}"
    );
}
