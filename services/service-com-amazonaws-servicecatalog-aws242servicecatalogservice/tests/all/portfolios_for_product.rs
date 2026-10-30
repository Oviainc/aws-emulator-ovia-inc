// SPDX-License-Identifier: CC0-1.0

//! `ListPortfoliosForProduct` over the wire, driven as awsJson1_1 requests through the
//! same `MultiServiceRouter` the production emulator mounts.
//!
//! The rule under test: the portfolios reported for a product are exactly the stored
//! product-portfolio associations that product holds — each reported as the very
//! `PortfolioDetail` the portfolio's own describe reports, in one stable order, paged by
//! `PageSize`/`NextPageToken` — a product that holds none is served an empty list, and a
//! product id nothing stored carries is the modelled 400 `ResourceNotFoundException`.
//!
//! Every assertion here is about STORED state: a response is checked against what an
//! earlier create, associate or update persisted, never against a value this file hands
//! the service and immediately reads back out of the same response.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    associate_product_with_portfolio, call, create_constraint, create_portfolio, create_product,
    emulator, error_code,
};

/// One portfolio every case seeds. A case associates a SUBSET of them with the product
/// under test, so a response that reported a portfolio the product does not sit in — or
/// one another product sits in — would fail.
struct PortfolioSeed {
    display_name: &'static str,
    provider: &'static str,
    tags: &'static [(&'static str, &'static str)],
    /// Set on the stored portfolio by an `UpdatePortfolio` after the create, so the
    /// reported detail cannot be a copy of the create request this file sent.
    description: Option<&'static str>,
}

/// The seeded portfolios: differing in provider, in whether they carry tags, and in
/// whether they carry a description, so the reported `PortfolioDetail` is compared
/// against several shapes of stored record.
const PORTFOLIOS: [PortfolioSeed; 4] = [
    PortfolioSeed {
        display_name: "alpha-portfolio",
        provider: "acme",
        tags: &[("team", "ops")],
        description: Some("the alpha catalog"),
    },
    PortfolioSeed {
        display_name: "beta-portfolio",
        provider: "globex",
        tags: &[],
        description: None,
    },
    PortfolioSeed {
        display_name: "gamma-portfolio",
        provider: "initech",
        tags: &[("team", "platform"), ("tier", "gold")],
        description: None,
    },
    PortfolioSeed {
        display_name: "delta-portfolio",
        provider: "umbrella",
        tags: &[],
        description: Some("the delta catalog"),
    },
];

/// One `ListPortfoliosForProduct` row: which seeded portfolios carry the product, by
/// index into [`PORTFOLIOS`].
struct ListCase {
    case: &'static str,
    portfolios: &'static [usize],
}

/// The rows: none, one, a non-adjacent pair, and every seeded portfolio — the counts and
/// subsets the rule generalises over.
fn list_cases() -> Vec<ListCase> {
    vec![
        ListCase {
            case: "no portfolio carries the product",
            portfolios: &[],
        },
        ListCase {
            case: "one portfolio, no tags and no description",
            portfolios: &[1],
        },
        ListCase {
            case: "one portfolio with tags and a description",
            portfolios: &[0],
        },
        ListCase {
            case: "a non-adjacent pair of portfolios",
            portfolios: &[1, 3],
        },
        ListCase {
            case: "every seeded portfolio",
            portfolios: &[0, 1, 2, 3],
        },
    ]
}

/// Seed every [`PORTFOLIOS`] entry and answer the minted ids, in seed order.
async fn seed_portfolios(router: &MultiServiceRouter) -> Vec<String> {
    let mut ids = Vec::new();
    for (index, seed) in PORTFOLIOS.iter().enumerate() {
        let id = create_portfolio(
            router,
            seed.display_name,
            seed.provider,
            seed.tags,
            &format!("portfolio-{index}"),
        )
        .await;
        if let Some(description) = seed.description {
            let (status, updated) = call(
                router,
                "UpdatePortfolio",
                &json!({"Id": id, "Description": description}),
            )
            .await;
            assert_eq!(
                status, 200,
                "UpdatePortfolio {id}: expected 200, got {status} with {updated}"
            );
        }
        ids.push(id);
    }
    ids
}

/// The `PortfolioDetails` of a response, keyed by portfolio id — the whole modelled
/// record, so a member that drifted from stored state fails the comparison.
fn details_by_id(case: &str, listed: &Value) -> BTreeMap<String, Value> {
    let details = listed["PortfolioDetails"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: expected a PortfolioDetails list, got {listed}")
    });
    details
        .iter()
        .map(|detail| {
            let id = detail["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a portfolio has no Id: {detail}"))
                .to_string();
            (id, detail.clone())
        })
        .collect()
}

/// The `PortfolioDetail` the portfolio's own describe reports — the record every other
/// response that carries that portfolio has to agree with.
async fn described_detail(router: &MultiServiceRouter, case: &str, portfolio_id: &str) -> Value {
    let (status, described) = call(router, "DescribePortfolio", &json!({"Id": portfolio_id})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribePortfolio({portfolio_id}) must be served, got {status} with \
         {described}"
    );
    described["PortfolioDetail"].clone()
}

/// Call `ListPortfoliosForProduct` for a product and answer its details by id, asserting
/// the call is served.
async fn list_for(
    router: &MultiServiceRouter,
    case: &str,
    product_id: &str,
) -> BTreeMap<String, Value> {
    let (status, listed) = call(
        router,
        "ListPortfoliosForProduct",
        &json!({"ProductId": product_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {listed}"
    );
    details_by_id(case, &listed)
}

/// A product's portfolios are exactly the associations stored for it, each reported as
/// the detail its own describe reports — and no other product's associations leak in.
#[tokio::test]
async fn the_listed_portfolios_are_the_products_stored_associations() {
    for row in list_cases() {
        let case = row.case;
        let router = emulator();
        let portfolio_ids = seed_portfolios(&router).await;
        let product_id = create_product(&router, "listed-product", "product-token").await;
        // A second product sits in EVERY seeded portfolio, so a handler that reported
        // the account's portfolios, or another product's, instead of this product's
        // associations would fail every row below.
        let other_product_id = create_product(&router, "other-product", "other-token").await;
        for portfolio_id in &portfolio_ids {
            associate_product_with_portfolio(&router, &other_product_id, portfolio_id).await;
        }
        for index in row.portfolios {
            associate_product_with_portfolio(&router, &product_id, &portfolio_ids[*index]).await;
        }

        let served = list_for(&router, case, &product_id).await;
        let expected: BTreeSet<String> = row
            .portfolios
            .iter()
            .map(|index| portfolio_ids[*index].clone())
            .collect();
        let served_ids: BTreeSet<String> = served.keys().cloned().collect();
        assert_eq!(
            served_ids, expected,
            "case {case:?}: expected exactly the associated portfolios {expected:?}, got \
             {served_ids:?}"
        );

        // Every reported record is the stored one: identical to the portfolio's own
        // describe, which also fixes the seeded display name, provider and description.
        for index in row.portfolios {
            let portfolio_id = &portfolio_ids[*index];
            let seed = &PORTFOLIOS[*index];
            let detail = &served[portfolio_id];
            assert_eq!(
                detail,
                &described_detail(&router, case, portfolio_id).await,
                "case {case:?}: portfolio {portfolio_id} must read exactly as its describe \
                 reports it, got {detail}"
            );
            assert_eq!(
                detail["DisplayName"].as_str(),
                Some(seed.display_name),
                "case {case:?}: portfolio {portfolio_id} must carry the stored DisplayName, got \
                 {detail}"
            );
            assert_eq!(
                detail["ProviderName"].as_str(),
                Some(seed.provider),
                "case {case:?}: portfolio {portfolio_id} must carry the stored ProviderName, got \
                 {detail}"
            );
            assert_eq!(
                detail["Description"].as_str(),
                seed.description,
                "case {case:?}: portfolio {portfolio_id} must carry the stored Description, got \
                 {detail}"
            );
            assert!(
                detail["ARN"]
                    .as_str()
                    .is_some_and(|arn| arn.ends_with(portfolio_id)),
                "case {case:?}: portfolio {portfolio_id} must carry its stored ARN, got {detail}"
            );
            assert!(
                detail.get("CreatedTime").is_some(),
                "case {case:?}: portfolio {portfolio_id} must carry its stored CreatedTime, got \
                 {detail}"
            );
        }

        // The other product's list is untouched by this product's associations: it still
        // reports every seeded portfolio.
        let others: BTreeSet<String> = list_for(&router, case, &other_product_id)
            .await
            .into_keys()
            .collect();
        let all: BTreeSet<String> = portfolio_ids.iter().cloned().collect();
        assert_eq!(
            others, all,
            "case {case:?}: the other product's portfolios must stay {all:?}, got {others:?}"
        );
    }
}

/// A portfolio is reported exactly while the association exists: disassociating removes
/// it from the list and re-associating brings it back, and a product with no association
/// is served an EMPTY list rather than a refusal.
#[tokio::test]
async fn a_listed_portfolio_lives_exactly_as_long_as_its_association() {
    let router = emulator();
    let portfolio_ids = seed_portfolios(&router).await;
    let product_id = create_product(&router, "listed-product", "product-token").await;

    // Rows: the associations held before the call, and the ids the call must report.
    let steps: [(&str, &[usize]); 6] = [
        ("before any association", &[]),
        ("one association", &[0]),
        ("two associations", &[0, 2]),
        ("the first disassociated", &[2]),
        ("the first re-associated", &[0, 2]),
        ("all disassociated", &[]),
    ];
    let mut held: BTreeSet<usize> = BTreeSet::new();

    for (case, want_held) in steps {
        let want: BTreeSet<usize> = want_held.iter().copied().collect();
        for index in want.difference(&held).copied().collect::<Vec<_>>() {
            associate_product_with_portfolio(&router, &product_id, &portfolio_ids[index]).await;
        }
        for index in held.difference(&want).copied().collect::<Vec<_>>() {
            let (status, body) = call(
                &router,
                "DisassociateProductFromPortfolio",
                &json!({"ProductId": product_id, "PortfolioId": portfolio_ids[index]}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the disassociation to be served, got {status} with {body}"
            );
        }
        held = want.clone();

        let served: BTreeSet<String> = list_for(&router, case, &product_id).await.into_keys().collect();
        let expected: BTreeSet<String> = want
            .iter()
            .map(|index| portfolio_ids[*index].clone())
            .collect();
        assert_eq!(
            served, expected,
            "case {case:?}: expected the associated portfolios {expected:?}, got {served:?}"
        );
    }
}

/// `PageSize` is a real bound and `NextPageToken` resumes where the previous page
/// stopped: for any number of associations and any page size, walking the pages yields
/// every associated portfolio exactly once.
#[tokio::test]
async fn paging_walks_every_associated_portfolio_exactly_once() {
    // Rows span association count x page size, including the smallest size, a size equal
    // to the count, a size above it, and the modelled maximum (`PageSize` is
    // `PageSizeMax100`, range 0..100).
    let cases: [(&str, usize, i64); 6] = [
        ("one portfolio, one per page", 1, 1),
        ("five portfolios, one per page", 5, 1),
        ("six portfolios, two per page", 6, 2),
        ("page size equals the count", 3, 3),
        ("page larger than the association set", 2, 10),
        ("modelled maximum page size", 4, 100),
    ];

    for (case, count, page_size) in cases {
        let router = emulator();
        let product_id = create_product(&router, "listed-product", "product-token").await;
        let mut created = BTreeSet::new();
        for i in 0..count {
            let portfolio_id = create_portfolio(
                &router,
                &format!("portfolio-{i}"),
                "acme",
                &[],
                &format!("portfolio-{i}"),
            )
            .await;
            associate_product_with_portfolio(&router, &product_id, &portfolio_id).await;
            created.insert(portfolio_id);
        }

        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"ProductId": product_id, "PageSize": page_size});
            if let Some(token) = &token {
                request["PageToken"] = Value::String(token.clone());
            }
            let (status, page) = call(&router, "ListPortfoliosForProduct", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {page}"
            );
            let details = details_by_id(case, &page);
            assert!(
                details.len() <= page_size as usize,
                "case {case:?}: PageSize {page_size} is a bound, got {} entries in {page}",
                details.len()
            );
            seen.extend(details.into_keys());
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
            "case {case:?}: expected every associated portfolio {created:?}, got {unique:?}"
        );
        // A token is handed back only while more remain, so the number of pages is
        // exactly the number the bound requires.
        let expected_pages = count.div_ceil(page_size as usize);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s) for {count} associations at \
             PageSize {page_size}, got {pages}"
        );
    }
}

/// `PageSize` 0 — the member's modelled default — asks for no caller-imposed bound, so
/// one page carries every portfolio the product is associated with.
#[tokio::test]
async fn the_default_page_size_bounds_nothing() {
    for (case, extra) in [
        ("explicit zero", json!({"PageSize": 0})),
        ("member omitted", json!({})),
    ] {
        let router = emulator();
        let product_id = create_product(&router, "listed-product", "product-token").await;
        let mut created = BTreeSet::new();
        for i in 0..7 {
            let portfolio_id = create_portfolio(
                &router,
                &format!("portfolio-{i}"),
                "acme",
                &[],
                &format!("portfolio-{i}"),
            )
            .await;
            associate_product_with_portfolio(&router, &product_id, &portfolio_id).await;
            created.insert(portfolio_id);
        }

        let mut request = json!({"ProductId": product_id});
        if let Some(members) = extra.as_object() {
            for (key, value) in members {
                request[key] = value.clone();
            }
        }
        let (status, listed) = call(&router, "ListPortfoliosForProduct", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {listed}"
        );
        let served: BTreeSet<String> = details_by_id(case, &listed).into_keys().collect();
        assert_eq!(
            served, created,
            "case {case:?}: expected every association on one page {created:?}, got {served:?}"
        );
        assert!(
            listed.get("NextPageToken").is_none(),
            "case {case:?}: nothing remains, so NextPageToken must be absent: {listed}"
        );
    }
}

/// A page token this service did not mint is a bad parameter; an empty token is the
/// documented way to ask for the first page. A product id no product carries is the
/// missing resource, whatever the id's shape.
#[tokio::test]
async fn a_bad_token_and_an_unknown_product_take_their_own_refusals() {
    let router = emulator();
    let portfolio_id = create_portfolio(&router, "alpha-portfolio", "acme", &[], "portfolio").await;
    let product_id = create_product(&router, "listed-product", "product-token").await;
    associate_product_with_portfolio(&router, &product_id, &portfolio_id).await;

    // Rows differ along HOW the token is malformed: not hex at all, odd-length hex, hex
    // that is not UTF-8, and hex of a value no id form admits.
    for (case, token) in [
        ("not a token at all", "bogus"),
        ("odd-length hex", "abc"),
        ("hex of invalid utf-8", "ff"),
        ("hex of a value outside the Id pattern", "7b7d20"),
        ("the empty token, which asks for the first page", ""),
    ] {
        let request = json!({"ProductId": product_id, "PageSize": 10, "PageToken": token});
        let (status, body) = call(&router, "ListPortfoliosForProduct", &request).await;
        if token.is_empty() {
            // The empty string is NOT a token: it means "the first page".
            assert_eq!(
                status, 200,
                "case {case:?}: an empty PageToken asks for the first page, got {status} with \
                 {body}"
            );
            let served: BTreeSet<String> = details_by_id(case, &body).into_keys().collect();
            assert_eq!(
                served,
                BTreeSet::from([portfolio_id.clone()]),
                "case {case:?}: expected the stored association, got {served:?} in {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for PageToken {token:?}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParametersException",
            "case {case:?}: unexpected error in {body}"
        );
    }

    // Rows differ along the shape of the absent product id.
    for (case, unknown) in [
        ("well-formed but unused", "prod-000000000000"),
        ("id of another resource kind", &portfolio_id as &str),
        ("not an id of any kind", "lorem"),
        ("at the Id length bound", "x"),
    ] {
        let (status, refused) = call(
            &router,
            "ListPortfoliosForProduct",
            &json!({"ProductId": unknown}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for ProductId {unknown:?}, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error in {refused}"
        );
        assert_eq!(
            refused["Message"].as_str().or(refused["message"].as_str()),
            Some(format!("There are no local or default products with id {unknown}").as_str()),
            "case {case:?}: unexpected refusal wording in {refused}"
        );
    }

    // None of those refusals disturbed the stored association.
    let served: BTreeSet<String> = list_for(&router, "after the refusals", &product_id)
        .await
        .into_keys()
        .collect();
    assert_eq!(
        served,
        BTreeSet::from([portfolio_id.clone()]),
        "case \"after the refusals\": the stored association must still be listed, got {served:?}"
    );
}

/// Listing the portfolios reports them and changes nothing: the same request repeated —
/// and a read of the portfolio and the product in between — answers the same document,
/// and the association itself survives.
#[tokio::test]
async fn listing_the_portfolios_changes_nothing() {
    let router = emulator();
    let portfolio_ids = seed_portfolios(&router).await;
    let product_id = create_product(&router, "listed-product", "product-token").await;
    for portfolio_id in &portfolio_ids[..2] {
        associate_product_with_portfolio(&router, &product_id, portfolio_id).await;
    }

    let request = json!({"ProductId": product_id});
    let (status, first) = call(&router, "ListPortfoliosForProduct", &request).await;
    assert_eq!(
        status, 200,
        "the first list must be served, got {status} with {first}"
    );
    let (status, described) = call(
        &router,
        "DescribeProductAsAdmin",
        &json!({"Id": product_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeProductAsAdmin must be served, got {status} with {described}"
    );
    let (status, second) = call(&router, "ListPortfoliosForProduct", &request).await;
    assert_eq!(
        status, 200,
        "the second list must be served, got {status} with {second}"
    );
    assert_eq!(
        details_by_id("repeated read", &second),
        details_by_id("repeated read", &first),
        "case \"repeated read\": a read changed what a later caller sees: {first} then {second}"
    );
}

/// The very associations this list reports are what `SourcePortfolioId` narrows a
/// product describe to: a portfolio that holds the product resolves it, a portfolio that
/// does not resolves nothing, and the refusal is AWS's product-lookup wording — the
/// catalog holds no local or default product under the requested id.
#[tokio::test]
async fn a_source_portfolio_narrows_a_describe_to_the_stored_association() {
    let router = emulator();
    let portfolio_ids = seed_portfolios(&router).await;
    let product_id = create_product(&router, "listed-product", "product-token").await;
    for index in [0usize, 2] {
        associate_product_with_portfolio(&router, &product_id, &portfolio_ids[index]).await;
    }

    // Rows: no source portfolio, each portfolio that holds the product, and portfolios
    // that do not — one seeded, one no portfolio at all.
    let cases: [(&str, Option<&str>, bool); 5] = [
        ("no source portfolio", None, true),
        ("the first holding portfolio", Some(&portfolio_ids[0]), true),
        ("the second holding portfolio", Some(&portfolio_ids[2]), true),
        (
            "a seeded portfolio that does not hold it",
            Some(&portfolio_ids[1]),
            false,
        ),
        (
            "a portfolio id nothing stored carries",
            Some("port-000000000000"),
            false,
        ),
    ];

    for (case, source, resolves) in cases {
        let mut request = json!({"Id": product_id});
        if let Some(source) = source {
            request["SourcePortfolioId"] = Value::String(source.to_string());
        }
        let (status, body) = call(&router, "DescribeProductAsAdmin", &request).await;
        if resolves {
            assert_eq!(
                status, 200,
                "case {case:?}: the association resolves the product, got {status} with {body}"
            );
            assert_eq!(
                body["ProductViewDetail"]["ProductViewSummary"]["ProductId"].as_str(),
                Some(product_id.as_str()),
                "case {case:?}: expected the stored product, got {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: no association resolves the product, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error in {body}"
        );
        let portfolio = source.unwrap_or_default();
        assert_eq!(
            body["Message"].as_str().or(body["message"].as_str()),
            Some(
                format!(
                    "There are no local or default products with id {product_id} associated with \
                     portfolio {portfolio}"
                )
                .as_str()
            ),
            "case {case:?}: unexpected refusal wording in {body}"
        );
    }

    // A product id nothing stored carries takes the same lookup refusal, without the
    // portfolio clause.
    let (status, refused) = call(
        &router,
        "DescribeProductAsAdmin",
        &json!({"Id": "prod-000000000000"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"an unknown product id\": expected 400, got {status} with {refused}"
    );
    assert_eq!(
        refused["Message"].as_str().or(refused["message"].as_str()),
        Some("There are no local or default products with id prod-000000000000"),
        "case \"an unknown product id\": unexpected refusal wording in {refused}"
    );
}

/// The very associations this list reports are what hold the product: while any of them
/// is stored the delete is refused with the modelled `ResourceInUseException`, in AWS's
/// one wording for that refusal — which names the product and the kinds of association
/// that can hold it, not the one that did — and once none is stored the delete goes
/// through.
#[tokio::test]
async fn a_held_product_cannot_be_deleted_and_takes_the_same_refusal_whichever_holds_it() {
    /// One row: what holds the product — the portfolios it is associated with, and the
    /// (portfolio, `ConstraintType`) pairs a constraint is created on.
    struct HeldCase {
        case: &'static str,
        portfolios: &'static [usize],
        constraints: &'static [(usize, &'static str)],
    }

    // Rows differ along WHAT holds the product: nothing, one portfolio, two portfolios,
    // and a portfolio plus a constraint on that pair.
    let cases = [
        HeldCase {
            case: "nothing holds the product",
            portfolios: &[],
            constraints: &[],
        },
        HeldCase {
            case: "one portfolio holds it",
            portfolios: &[0],
            constraints: &[],
        },
        HeldCase {
            case: "two portfolios hold it",
            portfolios: &[0, 2],
            constraints: &[],
        },
        HeldCase {
            case: "a portfolio and a constraint hold it",
            portfolios: &[1],
            constraints: &[(1, "LAUNCH")],
        },
    ];

    for HeldCase {
        case,
        portfolios,
        constraints,
    } in cases
    {
        let router = emulator();
        let portfolio_ids = seed_portfolios(&router).await;
        let product_id = create_product(&router, "held-product", "product-token").await;
        for index in portfolios {
            associate_product_with_portfolio(&router, &product_id, &portfolio_ids[*index]).await;
        }
        for (position, (index, constraint_type)) in constraints.iter().enumerate() {
            create_constraint(
                &router,
                &portfolio_ids[*index],
                &product_id,
                constraint_type,
                "a constraint that holds the product",
                &format!("constraint-{position}"),
            )
            .await;
        }

        let (status, body) = call(&router, "DeleteProduct", &json!({"Id": product_id})).await;
        if portfolios.is_empty() && constraints.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: an unheld product deletes, got {status} with {body}"
            );
            let (status, listed) = call(
                &router,
                "ListPortfoliosForProduct",
                &json!({"ProductId": product_id}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: a deleted product has no portfolios to list, got {status} with \
                 {listed}"
            );
            assert_eq!(
                error_code(&listed),
                "ResourceNotFoundException",
                "case {case:?}: unexpected error in {listed}"
            );
            assert_eq!(
                listed["Message"].as_str().or(listed["message"].as_str()),
                Some(
                    format!("There are no local or default products with id {product_id}").as_str()
                ),
                "case {case:?}: unexpected refusal wording in {listed}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: a held product cannot be deleted, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceInUseException",
            "case {case:?}: unexpected error in {body}"
        );
        assert_eq!(
            body["Message"].as_str().or(body["message"].as_str()),
            Some(
                format!(
                    "Product {product_id} still assigned to Portfolio(s), has Users, or has \
                     Constraints."
                )
                .as_str()
            ),
            "case {case:?}: unexpected refusal wording in {body}"
        );

        // The refused delete left every association where it was.
        let served: BTreeSet<String> = list_for(&router, case, &product_id).await.into_keys().collect();
        let expected: BTreeSet<String> = portfolios
            .iter()
            .map(|index| portfolio_ids[*index].clone())
            .collect();
        assert_eq!(
            served, expected,
            "case {case:?}: the refused delete must leave the associations {expected:?}, got \
             {served:?}"
        );
    }
}
