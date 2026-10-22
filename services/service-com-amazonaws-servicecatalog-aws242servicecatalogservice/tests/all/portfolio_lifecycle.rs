// SPDX-License-Identifier: CC0-1.0

//! The portfolio lifecycle over the wire: CreatePortfolio, DescribePortfolio,
//! ListPortfolios, UpdatePortfolio and DeletePortfolio, driven as awsJson1_1
//! requests through the same `MultiServiceRouter` the production emulator mounts.
//!
//! Every assertion here is about STORED state: a response is checked against what an
//! earlier call persisted, never against a value this file hands the service and
//! immediately reads back out of the same response.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire transport and the response readers every suite in this crate shares.
use crate::harness::{
    call, call_in, emulator, error_code, is_catalog_id, tag_list, tag_map, ACCOUNT_ID,
    DEFAULT_REGION,
};

/// AWS mints a portfolio id as `port-` followed by 12 lowercase base-36
/// characters, which the modelled `Id` pattern `^[a-zA-Z0-9_\-]*$` admits.
fn is_portfolio_id(id: &str) -> bool {
    is_catalog_id("port", id)
}

/// Create a portfolio and return the id the service minted for it.
async fn create(router: &MultiServiceRouter, display_name: &str, token: &str) -> String {
    let (status, body) = call(
        router,
        "CreatePortfolio",
        &json!({
            "DisplayName": display_name,
            "ProviderName": "acme",
            "IdempotencyToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePortfolio({display_name:?}): expected 200, got {status} with {body}"
    );
    body["PortfolioDetail"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreatePortfolio({display_name:?}): no Id in {body}"))
        .to_string()
}

/// The issue's end-to-end script: Create -> Describe -> List -> Update -> Describe ->
/// Delete -> Describe, asserting the BODIES at every step.
#[tokio::test]
async fn the_portfolio_lifecycle_round_trips_through_stored_state() {
    let router = emulator();

    let (status, created) = call(
        &router,
        "CreatePortfolio",
        &json!({
            "DisplayName": "demo",
            "ProviderName": "acme",
            "Description": "the first portfolio",
            "IdempotencyToken": "tok-1",
            "Tags": tag_list(&[("env", "dev")]),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePortfolio: expected 200, got {status} with {created}"
    );

    let detail = &created["PortfolioDetail"];
    let id = detail["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreatePortfolio: no Id in {created}"))
        .to_string();
    assert!(
        is_portfolio_id(&id),
        "CreatePortfolio: expected an id of the form port-<12 chars>, got {id:?}"
    );
    assert_eq!(
        detail["ARN"].as_str(),
        Some(format!("arn:aws:catalog:{DEFAULT_REGION}:{ACCOUNT_ID}:portfolio/{id}").as_str()),
        "CreatePortfolio: unexpected ARN in {created}"
    );
    let created_time = detail["CreatedTime"].clone();
    assert!(
        created_time.as_f64().is_some_and(|t| t > 0.0),
        "CreatePortfolio: expected a CreatedTime timestamp, got {created_time}"
    );

    // Describe reports exactly what create stored.
    let (status, described) = call(&router, "DescribePortfolio", &json!({"Id": id})).await;
    assert_eq!(
        status, 200,
        "DescribePortfolio: expected 200, got {status} with {described}"
    );
    assert_eq!(
        described["PortfolioDetail"], *detail,
        "DescribePortfolio: expected the stored PortfolioDetail {detail}, got {}",
        described["PortfolioDetail"]
    );
    assert_eq!(
        tag_map("describe-after-create", &described["Tags"]),
        BTreeMap::from([("env".to_string(), "dev".to_string())]),
        "DescribePortfolio: expected the stored tags, got {}",
        described["Tags"]
    );
    assert_eq!(
        described["TagOptions"],
        json!([]),
        "DescribePortfolio: no TagOption can be associated yet, so TagOptions must be empty: {described}"
    );
    assert_eq!(
        described["Budgets"],
        json!([]),
        "DescribePortfolio: no budget can be associated yet, so Budgets must be empty: {described}"
    );

    // List reports the same record.
    let (status, listed) = call(&router, "ListPortfolios", &json!({"PageSize": 10})).await;
    assert_eq!(
        status, 200,
        "ListPortfolios: expected 200, got {status} with {listed}"
    );
    assert_eq!(
        listed["PortfolioDetails"],
        json!([detail]),
        "ListPortfolios: expected exactly the stored portfolio, got {listed}"
    );
    assert!(
        listed.get("NextPageToken").is_none(),
        "ListPortfolios: nothing remains after the only page, so NextPageToken must be absent: {listed}"
    );

    // Update mutates the stored record in place.
    let (status, updated) = call(
        &router,
        "UpdatePortfolio",
        &json!({
            "Id": id,
            "Description": "updated",
            "AddTags": tag_list(&[("team", "platform")]),
            "RemoveTags": ["env"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdatePortfolio: expected 200, got {status} with {updated}"
    );
    let updated_detail = &updated["PortfolioDetail"];
    assert_eq!(
        updated_detail["Description"].as_str(),
        Some("updated"),
        "UpdatePortfolio: expected the new description, got {updated_detail}"
    );
    assert_eq!(
        updated_detail["DisplayName"].as_str(),
        Some("demo"),
        "UpdatePortfolio: an absent member keeps its stored value, got {updated_detail}"
    );
    assert_eq!(
        updated_detail["CreatedTime"], created_time,
        "UpdatePortfolio: CreatedTime is minted once and read back verbatim, got {updated_detail}"
    );
    assert_eq!(
        tag_map("update", &updated["Tags"]),
        BTreeMap::from([("team".to_string(), "platform".to_string())]),
        "UpdatePortfolio: expected env removed and team added, got {}",
        updated["Tags"]
    );

    // The update is observable through a later read.
    let (_, described) = call(&router, "DescribePortfolio", &json!({"Id": id})).await;
    assert_eq!(
        described["PortfolioDetail"], *updated_detail,
        "DescribePortfolio after update: expected {updated_detail}, got {}",
        described["PortfolioDetail"]
    );

    // Delete removes the record.
    let (status, deleted) = call(&router, "DeletePortfolio", &json!({"Id": id})).await;
    assert_eq!(
        status, 200,
        "DeletePortfolio: expected 200, got {status} with {deleted}"
    );

    let (status, gone) = call(&router, "DescribePortfolio", &json!({"Id": id})).await;
    assert_eq!(
        status, 400,
        "DescribePortfolio after delete: expected 400, got {status} with {gone}"
    );
    assert_eq!(
        error_code(&gone),
        "ResourceNotFoundException",
        "DescribePortfolio after delete: unexpected error in {gone}"
    );

    let (status, listed) = call(&router, "ListPortfolios", &json!({"PageSize": 10})).await;
    assert_eq!(
        status, 200,
        "ListPortfolios after delete: expected 200, got {status} with {listed}"
    );
    assert_eq!(
        listed["PortfolioDetails"],
        json!([]),
        "ListPortfolios after delete: the deleted portfolio must not be listed: {listed}"
    );
}

/// One row of the create table: the inputs, and the values a read must report back.
struct CreateCase {
    name: &'static str,
    display_name: String,
    provider_name: String,
    description: Option<String>,
    tags: Vec<(String, String)>,
}

/// A created portfolio is readable back member-for-member, over a table spanning
/// the input class: name lengths at both modelled bounds, a description absent /
/// empty / at its 2000-character bound, and 0, 1 and 20 tags (the `AddTags` bound).
#[tokio::test]
async fn every_created_portfolio_reads_back_exactly_as_stored() {
    let owned = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    };
    let twenty_tags: Vec<(String, String)> = (0..20)
        .map(|i| (format!("key{i:02}"), format!("value{i:02}")))
        .collect();

    // Rows differ along what the rule generalises over: value, size and count.
    // The bounds come from the model (`smithy_query query="operation"
    // operation="CreatePortfolio"`): DisplayName 1..100, ProviderName 1..50,
    // Description 0..2000, Tags 0..20.
    let cases: Vec<CreateCase> = vec![
        CreateCase {
            name: "shortest names",
            display_name: "a".to_string(),
            provider_name: "p".to_string(),
            description: None,
            tags: vec![],
        },
        CreateCase {
            name: "longest names",
            display_name: "d".repeat(100),
            provider_name: "p".repeat(50),
            description: Some(String::new()),
            tags: owned(&[("only", "one")]),
        },
        CreateCase {
            name: "longest description",
            display_name: "described".to_string(),
            provider_name: "acme".to_string(),
            description: Some("x".repeat(2000)),
            tags: owned(&[("env", "prod"), ("owner", "platform")]),
        },
        CreateCase {
            name: "tag bound",
            display_name: "twenty tags".to_string(),
            provider_name: "acme".to_string(),
            description: Some("many tags".to_string()),
            tags: twenty_tags,
        },
        CreateCase {
            name: "unicode and punctuation",
            display_name: "pôrtfolio d'été".to_string(),
            provider_name: "açme".to_string(),
            description: Some("décrit".to_string()),
            tags: owned(&[("clé", "valeur")]),
        },
    ];

    let router = emulator();
    let mut minted = BTreeSet::new();

    for (index, row) in cases.iter().enumerate() {
        let CreateCase {
            name: case,
            display_name,
            provider_name,
            description,
            tags,
        } = row;
        let tag_pairs: Vec<(&str, &str)> = tags
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let mut request = json!({
            "DisplayName": display_name,
            "ProviderName": provider_name,
            "IdempotencyToken": format!("tok-{index}"),
            "Tags": tag_list(&tag_pairs),
        });
        if let Some(description) = description {
            request["Description"] = Value::String(description.clone());
        }

        let (status, created) = call(&router, "CreatePortfolio", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreatePortfolio, got {status} with {created}"
        );
        let id = created["PortfolioDetail"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {created}"))
            .to_string();
        assert!(
            is_portfolio_id(&id),
            "case {case:?}: expected an id of the form port-<12 chars>, got {id:?}"
        );
        assert!(
            minted.insert(id.clone()),
            "case {case:?}: id {id:?} was already minted for an earlier portfolio"
        );

        let (status, described) = call(&router, "DescribePortfolio", &json!({"Id": id})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DescribePortfolio, got {status} with {described}"
        );
        let detail = &described["PortfolioDetail"];
        assert_eq!(
            detail["DisplayName"].as_str(),
            Some(display_name.as_str()),
            "case {case:?}: expected DisplayName {display_name:?}, got {detail}"
        );
        assert_eq!(
            detail["ProviderName"].as_str(),
            Some(provider_name.as_str()),
            "case {case:?}: expected ProviderName {provider_name:?}, got {detail}"
        );
        assert_eq!(
            detail["Description"].as_str(),
            description.as_deref(),
            "case {case:?}: expected Description {description:?}, got {detail}"
        );
        assert_eq!(
            detail["ARN"].as_str(),
            Some(format!("arn:aws:catalog:{DEFAULT_REGION}:{ACCOUNT_ID}:portfolio/{id}").as_str()),
            "case {case:?}: unexpected ARN in {detail}"
        );
        let expected_tags: BTreeMap<String, String> = tags.iter().cloned().collect();
        assert_eq!(
            tag_map(case, &described["Tags"]),
            expected_tags,
            "case {case:?}: expected the stored tags {expected_tags:?}, got {}",
            described["Tags"]
        );
    }

    // Every row is listed, exactly once each.
    let (_, listed) = call(&router, "ListPortfolios", &json!({"PageSize": 100})).await;
    let listed_ids: BTreeSet<String> = listed["PortfolioDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("ListPortfolios: expected a list, got {listed}"))
        .iter()
        .map(|detail| detail["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed_ids, minted,
        "ListPortfolios: expected every created portfolio {minted:?}, got {listed_ids:?}"
    );
}

/// The ARN a create mints carries the region the request was signed for.
#[tokio::test]
async fn a_minted_arn_carries_the_requests_region() {
    // One row per region shape: classic, EU, Asia-Pacific.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let router = emulator();
        let (status, created) = call_in(
            &router,
            region,
            "CreatePortfolio",
            &json!({
                "DisplayName": "regional",
                "ProviderName": "acme",
                "IdempotencyToken": "tok-region",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status} with {created}"
        );
        let detail = &created["PortfolioDetail"];
        let id = detail["Id"].as_str().unwrap_or_default();
        assert_eq!(
            detail["ARN"].as_str(),
            Some(format!("arn:aws:catalog:{region}:{ACCOUNT_ID}:portfolio/{id}").as_str()),
            "case {region:?}: unexpected ARN in {detail}"
        );
    }
}

/// `IdempotencyToken` is `@idempotencyToken`: repeating a create with a token
/// already used replays the portfolio it created, whatever the token's shape, and a
/// fresh token creates a new one.
#[tokio::test]
async fn a_repeated_idempotency_token_replays_the_same_portfolio() {
    // Rows differ along the token's form, within the modelled pattern
    // `^[a-zA-Z0-9][a-zA-Z0-9_-]*$` and 1..128 length.
    let tokens = [
        ("single character", "t".to_string()),
        ("digits", "01234567890".to_string()),
        ("dashes and underscores", "tok_A-b_C-1".to_string()),
        ("at the length bound", format!("t{}", "k".repeat(127))),
    ];

    let router = emulator();
    let mut replayed = Vec::new();

    for (case, token) in &tokens {
        let request = json!({
            "DisplayName": format!("portfolio for {case}"),
            "ProviderName": "acme",
            "IdempotencyToken": token,
            "Tags": tag_list(&[("env", "dev")]),
        });

        let (_, first) = call(&router, "CreatePortfolio", &request).await;
        let (status, second) = call(&router, "CreatePortfolio", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the repeated create, got {status} with {second}"
        );
        assert_eq!(
            second, first,
            "case {case:?}: the repeated create must replay {first}, got {second}"
        );
        replayed.push(
            first["PortfolioDetail"]["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no Id in {first}"))
                .to_string(),
        );
    }

    // One portfolio per distinct token, and no more: the repeats created nothing.
    let (_, listed) = call(&router, "ListPortfolios", &json!({"PageSize": 100})).await;
    let listed_ids: BTreeSet<String> = listed["PortfolioDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("ListPortfolios: expected a list, got {listed}"))
        .iter()
        .map(|detail| detail["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    let expected: BTreeSet<String> = replayed.iter().cloned().collect();
    assert_eq!(
        expected.len(),
        tokens.len(),
        "each distinct token must create its own portfolio, got {expected:?}"
    );
    assert_eq!(
        listed_ids, expected,
        "ListPortfolios: expected one portfolio per token {expected:?}, got {listed_ids:?}"
    );
}

/// `PageSize` is a real bound and `NextPageToken` resumes where the previous page
/// stopped: for any portfolio count and any page size, walking the pages yields
/// every persisted portfolio exactly once.
#[tokio::test]
async fn paging_walks_every_persisted_portfolio_exactly_once() {
    // Rows span count x page size, including a size above the count, a size equal
    // to it, the smallest size, and the modelled maximum (100).
    let cases = [
        ("empty catalog", 0_usize, 1_i64),
        ("one per page", 3, 1),
        ("two per page", 5, 2),
        ("page larger than the catalog", 3, 10),
        ("page size equals the count", 4, 4),
        ("modelled maximum page size", 7, 100),
    ];

    for (case, count, page_size) in cases {
        let router = emulator();
        let mut created = BTreeSet::new();
        for i in 0..count {
            created.insert(create(&router, &format!("p{i}"), &format!("tok-{i}")).await);
        }

        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"PageSize": page_size});
            if let Some(token) = &token {
                request["PageToken"] = Value::String(token.clone());
            }
            let (status, page) = call(&router, "ListPortfolios", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from ListPortfolios, got {status} with {page}"
            );
            let details = page["PortfolioDetails"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {page}"));
            assert!(
                details.len() <= page_size as usize,
                "case {case:?}: PageSize {page_size} is a bound, got {} entries in {page}",
                details.len()
            );
            for detail in details {
                seen.push(detail["Id"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= count + 2,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            match page.get("NextPageToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case:?}: a portfolio was listed twice across pages: {seen:?}"
        );
        assert_eq!(
            unique, created,
            "case {case:?}: expected every created portfolio {created:?}, got {unique:?}"
        );
        // A token is handed back only while more remain, so the number of pages is
        // exactly the number the bound requires.
        let expected_pages = if count == 0 {
            1
        } else {
            count.div_ceil(page_size as usize)
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s) for {count} portfolios at PageSize \
             {page_size}, got {pages}"
        );
    }
}

/// `PageSize` 0 — the member's modelled default — asks for no caller-imposed
/// bound, so one page carries every portfolio the account holds.
#[tokio::test]
async fn the_default_page_size_bounds_nothing() {
    for (case, request) in [
        ("explicit zero", json!({"PageSize": 0})),
        ("member omitted", json!({})),
    ] {
        let router = emulator();
        let mut created = BTreeSet::new();
        for i in 0..7 {
            created.insert(create(&router, &format!("p{i}"), &format!("tok-{i}")).await);
        }

        let (status, listed) = call(&router, "ListPortfolios", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {listed}"
        );
        let listed_ids: BTreeSet<String> = listed["PortfolioDetails"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {listed}"))
            .iter()
            .map(|detail| detail["Id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            listed_ids, created,
            "case {case:?}: expected every portfolio on one page {created:?}, got {listed_ids:?}"
        );
        assert!(
            listed.get("NextPageToken").is_none(),
            "case {case:?}: nothing remains, so NextPageToken must be absent: {listed}"
        );
    }
}

/// A page token this service did not mint is a bad parameter; an empty token is the
/// documented way to ask for the first page.
#[tokio::test]
async fn an_unknown_page_token_is_refused_as_an_invalid_parameter() {
    // Rows differ along HOW the token is malformed: not hex at all, odd-length hex,
    // hex that is not UTF-8, and hex of a value no id could be.
    let refused = [
        ("not a token at all", "bogus"),
        ("odd-length hex", "abc"),
        ("hex of invalid utf-8", "ff"),
        ("hex of a value outside the Id pattern", "7b7d20"),
        ("hex of the empty string", ""),
    ];

    let router = emulator();
    let id = create(&router, "only", "tok-1").await;

    for (case, token) in refused {
        // The empty string is NOT a token: it means "the first page".
        let request = json!({"PageSize": 10, "PageToken": token});
        let (status, body) = call(&router, "ListPortfolios", &request).await;
        if token.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: an empty PageToken asks for the first page, got {status} with {body}"
            );
            assert_eq!(
                body["PortfolioDetails"]
                    .as_array()
                    .map(|details| details.len()),
                Some(1),
                "case {case:?}: expected the first page to list portfolio {id}, got {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for token {token:?}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParametersException",
            "case {case:?}: unexpected error for token {token:?} in {body}"
        );
    }
}

/// One row of the update table: the patch, and the stored values it must leave.
struct UpdateCase {
    name: &'static str,
    patch: Value,
    display_name: &'static str,
    provider_name: &'static str,
    description: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
}

/// An update changes only the members it supplies, and the stored record is what a
/// later read reports.
#[tokio::test]
async fn an_update_changes_only_the_members_it_supplies() {
    // Rows differ along WHICH members the update supplies.
    let cases: Vec<UpdateCase> = vec![
        UpdateCase {
            name: "nothing supplied",
            patch: json!({}),
            display_name: "start",
            provider_name: "acme",
            description: Some("original"),
            tags: vec![("env", "dev"), ("owner", "ops")],
        },
        UpdateCase {
            name: "display name only",
            patch: json!({"DisplayName": "renamed"}),
            display_name: "renamed",
            provider_name: "acme",
            description: Some("original"),
            tags: vec![("env", "dev"), ("owner", "ops")],
        },
        UpdateCase {
            name: "provider name only",
            patch: json!({"ProviderName": "newprovider"}),
            display_name: "start",
            provider_name: "newprovider",
            description: Some("original"),
            tags: vec![("env", "dev"), ("owner", "ops")],
        },
        UpdateCase {
            name: "description only",
            patch: json!({"Description": "replaced"}),
            display_name: "start",
            provider_name: "acme",
            description: Some("replaced"),
            tags: vec![("env", "dev"), ("owner", "ops")],
        },
        UpdateCase {
            name: "add overwrites an existing key",
            patch: json!({"AddTags": tag_list(&[("env", "prod")])}),
            display_name: "start",
            provider_name: "acme",
            description: Some("original"),
            tags: vec![("env", "prod"), ("owner", "ops")],
        },
        UpdateCase {
            name: "remove by key",
            patch: json!({"RemoveTags": ["env"]}),
            display_name: "start",
            provider_name: "acme",
            description: Some("original"),
            tags: vec![("owner", "ops")],
        },
        UpdateCase {
            name: "remove every tag",
            patch: json!({"RemoveTags": ["env", "owner"]}),
            display_name: "start",
            provider_name: "acme",
            description: Some("original"),
            tags: vec![],
        },
        UpdateCase {
            name: "add and remove together",
            patch: json!({
                "AddTags": tag_list(&[("team", "platform")]),
                "RemoveTags": ["owner"],
            }),
            display_name: "start",
            provider_name: "acme",
            description: Some("original"),
            tags: vec![("env", "dev"), ("team", "platform")],
        },
        UpdateCase {
            name: "every member at once",
            patch: json!({
                "DisplayName": "all",
                "ProviderName": "allprovider",
                "Description": "all",
                "AddTags": tag_list(&[("a", "1")]),
                "RemoveTags": ["env", "owner"],
            }),
            display_name: "all",
            provider_name: "allprovider",
            description: Some("all"),
            tags: vec![("a", "1")],
        },
    ];

    for row in cases {
        let UpdateCase {
            name: case,
            patch,
            display_name,
            provider_name,
            description,
            tags: expected_tags,
        } = row;
        let router = emulator();
        let (_, created) = call(
            &router,
            "CreatePortfolio",
            &json!({
                "DisplayName": "start",
                "ProviderName": "acme",
                "Description": "original",
                "IdempotencyToken": "tok-1",
                "Tags": tag_list(&[("env", "dev"), ("owner", "ops")]),
            }),
        )
        .await;
        let id = created["PortfolioDetail"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {created}"))
            .to_string();

        let mut request = patch;
        request["Id"] = Value::String(id.clone());
        let (status, updated) = call(&router, "UpdatePortfolio", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdatePortfolio, got {status} with {updated}"
        );

        // The response is the post-update stored state, and so is the later read.
        let (_, described) = call(&router, "DescribePortfolio", &json!({"Id": id})).await;
        assert_eq!(
            described["PortfolioDetail"], updated["PortfolioDetail"],
            "case {case:?}: DescribePortfolio must report the updated record {}, got {}",
            updated["PortfolioDetail"], described["PortfolioDetail"]
        );

        let detail = &described["PortfolioDetail"];
        assert_eq!(
            detail["DisplayName"].as_str(),
            Some(display_name),
            "case {case:?}: expected DisplayName {display_name:?}, got {detail}"
        );
        assert_eq!(
            detail["ProviderName"].as_str(),
            Some(provider_name),
            "case {case:?}: expected ProviderName {provider_name:?}, got {detail}"
        );
        assert_eq!(
            detail["Description"].as_str(),
            description,
            "case {case:?}: expected Description {description:?}, got {detail}"
        );
        assert_eq!(
            detail["Id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: the id is immutable, got {detail}"
        );
        let expected: BTreeMap<String, String> = expected_tags
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        assert_eq!(
            tag_map(case, &described["Tags"]),
            expected,
            "case {case:?}: expected tags {expected:?}, got {}",
            described["Tags"]
        );
        assert_eq!(
            tag_map(case, &updated["Tags"]),
            expected,
            "case {case:?}: the update response must carry the stored tags {expected:?}, got {}",
            updated["Tags"]
        );
    }
}

/// A portfolio cannot hold more tags than the modelled `Tags` list admits (50), and
/// the update that would exceed it stores nothing.
#[tokio::test]
async fn an_update_past_the_tag_limit_is_refused_and_stores_nothing() {
    let router = emulator();
    let batch = |from: usize| -> Value {
        Value::Array(
            (from..from + 20)
                .map(|i| json!({"Key": format!("key{i:02}"), "Value": format!("v{i:02}")}))
                .collect(),
        )
    };

    let (_, created) = call(
        &router,
        "CreatePortfolio",
        &json!({
            "DisplayName": "tagged",
            "ProviderName": "acme",
            "IdempotencyToken": "tok-1",
            "Tags": batch(0),
        }),
    )
    .await;
    let id = created["PortfolioDetail"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("no Id in {created}"))
        .to_string();

    // 20 + 20 = 40 tags: still inside the limit.
    let (status, updated) = call(
        &router,
        "UpdatePortfolio",
        &json!({"Id": id, "AddTags": batch(20)}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"40 tags\": expected 200, got {status} with {updated}"
    );
    assert_eq!(
        tag_map("40 tags", &updated["Tags"]).len(),
        40,
        "case \"40 tags\": expected 40 stored tags, got {}",
        updated["Tags"]
    );

    // 40 + 20 = 60 tags: past the limit.
    let (status, refused) = call(
        &router,
        "UpdatePortfolio",
        &json!({"Id": id, "AddTags": batch(40)}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"60 tags\": expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "case \"60 tags\": unexpected error in {refused}"
    );

    let (_, described) = call(&router, "DescribePortfolio", &json!({"Id": id})).await;
    assert_eq!(
        tag_map("after refusal", &described["Tags"]).len(),
        40,
        "case \"60 tags\": the refused update must store nothing, got {}",
        described["Tags"]
    );
}

/// Every portfolio operation that names a portfolio refuses an id no portfolio has,
/// whatever the id's shape — never a hollow 200.
#[tokio::test]
async fn an_unknown_portfolio_id_is_resource_not_found() {
    let router = emulator();
    let live = create(&router, "live", "tok-1").await;

    // Rows differ along the shape of the absent id, and along the operation.
    let unknown_ids = [
        ("doesnotexist", "port-doesnotexist"),
        ("well-formed but unused", "port-000000000000"),
        ("not a portfolio id", "lorem"),
        ("at the Id length bound", "p"),
    ];

    for (case, unknown) in unknown_ids {
        for (operation, body) in [
            ("DescribePortfolio", json!({"Id": unknown})),
            ("UpdatePortfolio", json!({"Id": unknown, "Description": "x"})),
            ("DeletePortfolio", json!({"Id": unknown})),
        ] {
            let (status, refused) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}/{operation}: expected 400 for id {unknown:?}, got {status} with \
                 {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {case:?}/{operation}: unexpected error for id {unknown:?} in {refused}"
            );
        }
    }

    // The live portfolio is untouched by all of that.
    let (status, described) = call(&router, "DescribePortfolio", &json!({"Id": live})).await;
    assert_eq!(
        status, 200,
        "the live portfolio must still be readable, got {status} with {described}"
    );
}

/// A delete frees its idempotency tokens: the same token afterwards creates a new
/// portfolio rather than replaying the deleted one.
#[tokio::test]
async fn a_token_whose_portfolio_was_deleted_creates_a_new_one() {
    let router = emulator();
    let request = json!({
        "DisplayName": "recreated",
        "ProviderName": "acme",
        "IdempotencyToken": "tok-reuse",
    });

    let (_, first) = call(&router, "CreatePortfolio", &request).await;
    let first_id = first["PortfolioDetail"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("no Id in {first}"))
        .to_string();
    let (status, deleted) = call(&router, "DeletePortfolio", &json!({"Id": first_id})).await;
    assert_eq!(
        status, 200,
        "DeletePortfolio: expected 200, got {status} with {deleted}"
    );

    let (status, second) = call(&router, "CreatePortfolio", &request).await;
    assert_eq!(
        status, 200,
        "CreatePortfolio after delete: expected 200, got {status} with {second}"
    );
    let second_id = second["PortfolioDetail"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("no Id in {second}"))
        .to_string();
    assert_ne!(
        second_id, first_id,
        "the deleted portfolio cannot be replayed, so the new id must differ: got {second_id:?}"
    );

    let (_, listed) = call(&router, "ListPortfolios", &json!({"PageSize": 100})).await;
    assert_eq!(
        listed["PortfolioDetails"],
        json!([second["PortfolioDetail"]]),
        "ListPortfolios: expected only the re-created portfolio, got {listed}"
    );
}

/// Reads do not change what a later caller sees: describing and listing a portfolio
/// repeatedly reports byte-identical state.
#[tokio::test]
async fn reads_do_not_change_stored_state() {
    let router = emulator();
    let id = create(&router, "stable", "tok-1").await;

    let (_, first_describe) = call(&router, "DescribePortfolio", &json!({"Id": id})).await;
    let (_, first_list) = call(&router, "ListPortfolios", &json!({"PageSize": 100})).await;
    for round in 0..3 {
        let (_, describe) = call(&router, "DescribePortfolio", &json!({"Id": id})).await;
        assert_eq!(
            describe, first_describe,
            "case {round}: DescribePortfolio must keep reporting {first_describe}, got {describe}"
        );
        let (_, list) = call(&router, "ListPortfolios", &json!({"PageSize": 100})).await;
        assert_eq!(
            list, first_list,
            "case {round}: ListPortfolios must keep reporting {first_list}, got {list}"
        );
    }
}

/// The account's portfolio quota is a real bound: the create past it is refused with
/// `LimitExceededException`, stores nothing, and a delete frees room again.
///
/// The bound is AWS's documented default Service Catalog quota of 100 portfolios per
/// account per Region (`crate::state::MAX_PORTFOLIOS`), and `LimitExceededException`
/// is the error `CreatePortfolio` models for it.
#[tokio::test]
async fn creates_past_the_portfolio_quota_are_refused() {
    const QUOTA: usize = 100;

    let router = emulator();
    let mut created = BTreeSet::new();
    for i in 0..QUOTA {
        created.insert(create(&router, &format!("p{i:03}"), &format!("tok-{i:03}")).await);
    }
    assert_eq!(
        created.len(),
        QUOTA,
        "expected {QUOTA} distinct portfolios up to the quota, got {}",
        created.len()
    );

    // Two rows past the bound: a fresh token, and one already used (whose portfolio
    // still exists, so it replays instead of being refused).
    let (status, refused) = call(
        &router,
        "CreatePortfolio",
        &json!({
            "DisplayName": "one too many",
            "ProviderName": "acme",
            "IdempotencyToken": "tok-over",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one past the quota\": expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "case \"one past the quota\": unexpected error in {refused}"
    );

    let (status, replayed) = call(
        &router,
        "CreatePortfolio",
        &json!({
            "DisplayName": "p000",
            "ProviderName": "acme",
            "IdempotencyToken": "tok-000",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"replay at the quota\": a replay creates nothing, so it is not refused: \
         got {status} with {replayed}"
    );

    // Nothing was stored by the refusal.
    let (_, listed) = call(&router, "ListPortfolios", &json!({"PageSize": 100})).await;
    let listed_ids: BTreeSet<String> = listed["PortfolioDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("ListPortfolios: expected a list, got {listed}"))
        .iter()
        .map(|detail| detail["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed_ids, created,
        "case \"one past the quota\": the refused create must store nothing, got {} entries",
        listed_ids.len()
    );

    // A delete frees room for exactly one more.
    let victim = created
        .iter()
        .next()
        .expect("the quota set is not empty")
        .clone();
    let (status, deleted) = call(&router, "DeletePortfolio", &json!({"Id": victim})).await;
    assert_eq!(
        status, 200,
        "DeletePortfolio: expected 200, got {status} with {deleted}"
    );
    let (status, allowed) = call(
        &router,
        "CreatePortfolio",
        &json!({
            "DisplayName": "back under the quota",
            "ProviderName": "acme",
            "IdempotencyToken": "tok-again",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"back under the quota\": expected 200, got {status} with {allowed}"
    );
}
