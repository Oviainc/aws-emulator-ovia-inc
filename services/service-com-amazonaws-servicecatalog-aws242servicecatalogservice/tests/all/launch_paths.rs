// SPDX-License-Identifier: CC0-1.0

//! `ListLaunchPaths` over the wire, driven as awsJson1_1 requests through the same
//! `MultiServiceRouter` the production emulator mounts.
//!
//! The rule under test: a product's launch paths ARE the portfolio associations the
//! service holds for it — one path per carrying portfolio, identified and named by that
//! portfolio, carrying the constraints stored for that portfolio-product pair and the
//! portfolio's tags — and a product no portfolio carries resolves no path at all, which
//! is the modelled 400 `ResourceNotFoundException` and never an empty page.
//!
//! Every assertion here is about STORED state: a response is checked against what an
//! earlier create or associate persisted, never against a value this file hands the
//! service and immediately reads back out of the same response.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator, error_code, tag_list, tag_map};

/// One portfolio a case seeds.
struct PortfolioSeed {
    display_name: &'static str,
    provider: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

/// The portfolios every case seeds. A case associates a SUBSET of them with its
/// product, so a response that reported a portfolio the product does not sit in would
/// fail.
const PORTFOLIOS: [PortfolioSeed; 3] = [
    PortfolioSeed {
        display_name: "alpha-portfolio",
        provider: "acme",
        tags: &[("team", "ops")],
    },
    PortfolioSeed {
        display_name: "beta-portfolio",
        provider: "globex",
        tags: &[],
    },
    PortfolioSeed {
        display_name: "gamma-portfolio",
        provider: "initech",
        tags: &[("team", "platform"), ("tier", "gold")],
    },
];

/// One launch path, as a response reports it and as the stored state says it must read:
/// the portfolio's name, the portfolio's tags, and the constraints on the
/// portfolio-product pair as `ConstraintType` -> description.
#[derive(Debug, PartialEq, Eq)]
struct ListedPath {
    name: String,
    tags: BTreeMap<String, String>,
    constraints: BTreeMap<String, String>,
}

/// The launch paths of one response, keyed by path id.
type ListedPaths = BTreeMap<String, ListedPath>;

/// One `ListLaunchPaths` row.
struct PathCase {
    case: &'static str,
    /// Which seeded portfolios carry the product, by index into [`PORTFOLIOS`].
    portfolios: &'static [usize],
    /// The constraints created on a (portfolio index, `ConstraintType`, description)
    /// triple before the product's paths are listed.
    constraints: &'static [(usize, &'static str, &'static str)],
}

/// The rows: zero, one, two and three carrying portfolios, with no constraint, one
/// constraint, a constraint on only one of two paths, and two constraints of different
/// types on one path.
///
/// The constraint types are the model's own
/// (`smithy_query query="shape" shape="ConstraintType"` documents LAUNCH,
/// NOTIFICATION, STACKSET and TEMPLATE), so the rows cover a type the stub census issue
/// named none of.
fn path_cases() -> Vec<PathCase> {
    vec![
        PathCase {
            case: "no portfolio carries the product",
            portfolios: &[],
            constraints: &[],
        },
        PathCase {
            case: "one portfolio, no constraint and no portfolio tag",
            portfolios: &[1],
            constraints: &[],
        },
        PathCase {
            case: "one portfolio with one NOTIFICATION constraint",
            portfolios: &[0],
            constraints: &[(0, "NOTIFICATION", "tell the operators")],
        },
        PathCase {
            case: "two portfolios, only the second constrained",
            portfolios: &[0, 2],
            constraints: &[(2, "TEMPLATE", "the template rules")],
        },
        PathCase {
            case: "three portfolios, two constraints on one path",
            portfolios: &[0, 1, 2],
            constraints: &[
                (0, "LAUNCH", "the launch role"),
                (0, "STACKSET", "the stack set targets"),
            ],
        },
    ]
}

/// A `Parameters` document of the shape each constraint type documents
/// (`smithy_query query="doc" shape="CreateConstraintInput" member="Parameters"`): the
/// emulator stores it verbatim, so the only thing that matters here is that each row
/// sends the document its type calls for.
fn constraint_parameters(constraint_type: &str) -> String {
    match constraint_type {
        "LAUNCH" => r#"{"RoleArn":"arn:aws:iam::000000000000:role/SCBasicLaunchRole"}"#.to_string(),
        "NOTIFICATION" => {
            r#"{"NotificationArns":["arn:aws:sns:us-east-1:000000000000:topic"]}"#.to_string()
        }
        "STACKSET" => r#"{"AccountList":["000000000000"],"RegionList":["us-east-1"],"AdminRole":"arn:aws:iam::000000000000:role/AWSCloudFormationStackSetAdministrationRole","ExecutionRole":"AWSCloudFormationStackSetExecutionRole"}"#
            .to_string(),
        _ => r#"{"Rules":{"Required":{"Assertions":[]}}}"#.to_string(),
    }
}

/// Create one portfolio and answer the id the service minted.
async fn create_portfolio(
    router: &MultiServiceRouter,
    display_name: &str,
    provider: &str,
    tags: &[(&str, &str)],
    token: &str,
) -> String {
    let mut request = json!({
        "DisplayName": display_name,
        "ProviderName": provider,
        "IdempotencyToken": token,
    });
    if !tags.is_empty() {
        request["Tags"] = tag_list(tags);
    }
    let (status, created) = call(router, "CreatePortfolio", &request).await;
    assert_eq!(
        status, 200,
        "CreatePortfolio {display_name:?}: expected 200, got {status} with {created}"
    );
    created["PortfolioDetail"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreatePortfolio {display_name:?}: no Id in {created}"))
        .to_string()
}

/// Create one CloudFormation product and answer the id the service minted.
async fn create_product(router: &MultiServiceRouter, name: &str, token: &str) -> String {
    let (status, created) = call(
        router,
        "CreateProduct",
        &json!({
            "Name": name,
            "Owner": "acme",
            "ProductType": "CLOUD_FORMATION_TEMPLATE",
            "ProvisioningArtifactParameters": {
                "Name": "v1",
                "Type": "CLOUD_FORMATION_TEMPLATE",
                // The accepted `Info` keys are the model's own:
                // `ProvisioningArtifactProperties$Info` documents
                // "Keys accepted: [ LoadTemplateFromURL, ImportFromPhysicalId ]".
                "Info": {"LoadTemplateFromURL": "https://example.invalid/template.json"},
            },
            "IdempotencyToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateProduct {name:?}: expected 200, got {status} with {created}"
    );
    created["ProductViewDetail"]["ProductViewSummary"]["ProductId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateProduct {name:?}: no ProductId in {created}"))
        .to_string()
}

/// Associate a product with a portfolio, which is what creates a launch path.
async fn associate(router: &MultiServiceRouter, product_id: &str, portfolio_id: &str) {
    let (status, body) = call(
        router,
        "AssociateProductWithPortfolio",
        &json!({"ProductId": product_id, "PortfolioId": portfolio_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "AssociateProductWithPortfolio({product_id}, {portfolio_id}): expected 200, got {status} \
         with {body}"
    );
}

/// Create one constraint on a portfolio-product pair.
async fn create_constraint(
    router: &MultiServiceRouter,
    portfolio_id: &str,
    product_id: &str,
    constraint_type: &str,
    description: &str,
    token: &str,
) {
    let (status, created) = call(
        router,
        "CreateConstraint",
        &json!({
            "PortfolioId": portfolio_id,
            "ProductId": product_id,
            "Type": constraint_type,
            "Description": description,
            "Parameters": constraint_parameters(constraint_type),
            "IdempotencyToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateConstraint {constraint_type} on ({portfolio_id}, {product_id}): expected 200, got \
         {status} with {created}"
    );
}

/// The launch paths of a response, keyed by path id.
fn paths_by_id(case: &str, listed: &Value) -> ListedPaths {
    let summaries = listed["LaunchPathSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a LaunchPathSummaries list in {listed}"));
    summaries
        .iter()
        .map(|summary| {
            let id = summary["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a path has no Id: {summary}"))
                .to_string();
            let name = summary["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: path {id} has no Name: {summary}"))
                .to_string();
            let tags = tag_map(case, &summary["Tags"]);
            let constraints = summary["ConstraintSummaries"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: path {id} has no ConstraintSummaries: {summary}")
                })
                .iter()
                .map(|constraint| {
                    (
                        constraint["Type"].as_str().unwrap_or_default().to_string(),
                        constraint["Description"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect();
            (
                id,
                ListedPath {
                    name,
                    tags,
                    constraints,
                },
            )
        })
        .collect()
}

/// A product's launch paths are the portfolios that carry it: each path is identified
/// and named by its portfolio, carries that portfolio's tags and the constraints stored
/// for the portfolio-product pair, and a product no portfolio carries has no path.
#[tokio::test]
async fn launch_paths_are_the_portfolios_that_carry_the_product() {
    for row in path_cases() {
        let case = row.case;
        let router = emulator();

        // Every case seeds every portfolio; only the row's subset carries the product.
        let mut portfolio_ids = Vec::new();
        for (index, seed) in PORTFOLIOS.iter().enumerate() {
            portfolio_ids.push(
                create_portfolio(
                    &router,
                    seed.display_name,
                    seed.provider,
                    seed.tags,
                    &format!("portfolio-{index}"),
                )
                .await,
            );
        }
        let product_id = create_product(&router, "listed-product", "product-token").await;
        // A second product in every portfolio, so a path reported for the product under
        // test cannot come from another product's associations or constraints.
        let other_product_id = create_product(&router, "other-product", "other-token").await;
        for portfolio_id in &portfolio_ids {
            associate(&router, &other_product_id, portfolio_id).await;
        }
        create_constraint(
            &router,
            &portfolio_ids[0],
            &other_product_id,
            "NOTIFICATION",
            "another product's constraint",
            "other-constraint",
        )
        .await;

        for index in row.portfolios {
            associate(&router, &product_id, &portfolio_ids[*index]).await;
        }
        for (position, (index, constraint_type, description)) in row.constraints.iter().enumerate() {
            create_constraint(
                &router,
                &portfolio_ids[*index],
                &product_id,
                constraint_type,
                description,
                &format!("constraint-{position}"),
            )
            .await;
        }

        let (status, listed) = call(&router, "ListLaunchPaths", &json!({"ProductId": product_id}))
            .await;

        if row.portfolios.is_empty() {
            assert_eq!(
                status, 400,
                "case {case:?}: a product in no portfolio has no launch path, got {status} with \
                 {listed}"
            );
            assert_eq!(
                error_code(&listed),
                "ResourceNotFoundException",
                "case {case:?}: unexpected error in {listed}"
            );
            assert_eq!(
                listed["message"].as_str().or(listed["Message"].as_str()),
                Some(format!("No launch paths found for resource: {product_id}").as_str()),
                "case {case:?}: unexpected message in {listed}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {listed}"
        );
        assert!(
            listed.get("NextPageToken").is_none(),
            "case {case:?}: one page carries every path, so NextPageToken must be absent: {listed}"
        );

        // What the earlier calls persisted: one path per carrying portfolio, named by
        // that portfolio, with its tags and the constraints on the pair.
        let expected: ListedPaths = row
            .portfolios
            .iter()
            .map(|index| {
                let seed = &PORTFOLIOS[*index];
                (
                    portfolio_ids[*index].clone(),
                    ListedPath {
                        name: seed.display_name.to_string(),
                        tags: seed
                            .tags
                            .iter()
                            .map(|(key, value)| (key.to_string(), value.to_string()))
                            .collect(),
                        constraints: row
                            .constraints
                            .iter()
                            .filter(|(on, _, _)| on == index)
                            .map(|(_, constraint_type, description)| {
                                (constraint_type.to_string(), description.to_string())
                            })
                            .collect(),
                    },
                )
            })
            .collect();
        let served = paths_by_id(case, &listed);
        assert_eq!(
            served, expected,
            "case {case:?}: expected the stored paths {expected:?}, got {served:?} in {listed}"
        );

        // The same stored set DescribeProduct reports as LaunchPaths, so the two
        // readers of one association cannot disagree.
        let (status, described) = call(&router, "DescribeProduct", &json!({"Id": product_id})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeProduct to be served, got {status} with {described}"
        );
        let described_paths: BTreeMap<String, String> = described["LaunchPaths"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected LaunchPaths in {described}"))
            .iter()
            .map(|path| {
                (
                    path["Id"].as_str().unwrap_or_default().to_string(),
                    path["Name"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let listed_paths: BTreeMap<String, String> = served
            .iter()
            .map(|(id, path)| (id.clone(), path.name.clone()))
            .collect();
        assert_eq!(
            listed_paths, described_paths,
            "case {case:?}: ListLaunchPaths reported {listed_paths:?} where DescribeProduct \
             reports {described_paths:?}"
        );
    }
}

/// A path exists exactly while the association does: disassociating the last portfolio
/// leaves the product with no launch path, and re-associating it brings the path back.
#[tokio::test]
async fn a_path_lives_exactly_as_long_as_its_association() {
    let router = emulator();
    let portfolio_id = create_portfolio(&router, "alpha-portfolio", "acme", &[], "portfolio").await;
    let second_id = create_portfolio(&router, "beta-portfolio", "globex", &[], "portfolio-2").await;
    let product_id = create_product(&router, "listed-product", "product-token").await;

    // Rows: the associations held before the call, and the ids the call must report —
    // nothing at all for the row that holds none.
    let steps: [(&str, &[usize], &[usize]); 5] = [
        ("before any association", &[], &[]),
        ("one association", &[0], &[0]),
        ("both associations", &[0, 1], &[0, 1]),
        ("the first disassociated", &[1], &[1]),
        ("both disassociated", &[], &[]),
    ];
    let ids = [portfolio_id.clone(), second_id.clone()];
    let mut held: BTreeSet<usize> = BTreeSet::new();

    for (case, want_held, expected) in steps {
        let want: BTreeSet<usize> = want_held.iter().copied().collect();
        for index in want.difference(&held).copied().collect::<Vec<_>>() {
            associate(&router, &product_id, &ids[index]).await;
        }
        for index in held.difference(&want).copied().collect::<Vec<_>>() {
            let (status, body) = call(
                &router,
                "DisassociateProductFromPortfolio",
                &json!({"ProductId": product_id, "PortfolioId": ids[index]}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the disassociation to be served, got {status} with {body}"
            );
        }
        held = want;

        let (status, listed) =
            call(&router, "ListLaunchPaths", &json!({"ProductId": product_id})).await;
        if expected.is_empty() {
            assert_eq!(
                status, 400,
                "case {case:?}: no association means no launch path, got {status} with {listed}"
            );
            assert_eq!(
                error_code(&listed),
                "ResourceNotFoundException",
                "case {case:?}: unexpected error in {listed}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {listed}"
        );
        let served: BTreeSet<String> = paths_by_id(case, &listed).into_keys().collect();
        let want_ids: BTreeSet<String> = expected.iter().map(|index| ids[*index].clone()).collect();
        assert_eq!(
            served, want_ids,
            "case {case:?}: expected the paths {want_ids:?}, got {served:?} in {listed}"
        );
    }
}

/// `PageSize` is a real bound and `NextPageToken` resumes where the previous page
/// stopped: for any number of paths and any page size, walking the pages yields every
/// path exactly once.
#[tokio::test]
async fn paging_walks_every_launch_path_exactly_once() {
    // Rows span path count x page size, including the smallest size, a size equal to
    // the count, a size above it, and the modelled maximum (`PageSize` is range 0..20).
    let cases: [(&str, usize, i64); 6] = [
        ("one path, one per page", 1, 1),
        ("three paths, one per page", 3, 1),
        ("four paths, two per page", 4, 2),
        ("page size equals the count", 3, 3),
        ("page larger than the path set", 2, 10),
        ("modelled maximum page size", 5, 20),
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
            associate(&router, &product_id, &portfolio_id).await;
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
            let (status, page) = call(&router, "ListLaunchPaths", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {page}"
            );
            let summaries = page["LaunchPathSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {page}"));
            assert!(
                summaries.len() <= page_size as usize,
                "case {case:?}: PageSize {page_size} is a bound, got {} entries in {page}",
                summaries.len()
            );
            for summary in summaries {
                seen.push(summary["Id"].as_str().unwrap_or_default().to_string());
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
            "case {case:?}: a path was listed twice across pages: {seen:?}"
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
            "case {case:?}: expected {expected_pages} page(s) for {count} paths at PageSize \
             {page_size}, got {pages}"
        );
    }
}

/// `PageSize` 0 — the member's modelled default — asks for no caller-imposed bound, so
/// one page carries every path the product has.
#[tokio::test]
async fn the_default_page_size_bounds_nothing() {
    for (case, extra) in [
        ("explicit zero", json!({"PageSize": 0})),
        ("member omitted", json!({})),
    ] {
        let router = emulator();
        let product_id = create_product(&router, "listed-product", "product-token").await;
        let mut created = BTreeSet::new();
        for i in 0..6 {
            let portfolio_id = create_portfolio(
                &router,
                &format!("portfolio-{i}"),
                "acme",
                &[],
                &format!("portfolio-{i}"),
            )
            .await;
            associate(&router, &product_id, &portfolio_id).await;
            created.insert(portfolio_id);
        }

        let mut request = json!({"ProductId": product_id});
        if let Some(members) = extra.as_object() {
            for (key, value) in members {
                request[key] = value.clone();
            }
        }
        let (status, listed) = call(&router, "ListLaunchPaths", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {listed}"
        );
        let served: BTreeSet<String> = paths_by_id(case, &listed).into_keys().collect();
        assert_eq!(
            served, created,
            "case {case:?}: expected every path on one page {created:?}, got {served:?}"
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
    associate(&router, &product_id, &portfolio_id).await;

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
        let (status, body) = call(&router, "ListLaunchPaths", &request).await;
        if token.is_empty() {
            // The empty string is NOT a token: it means "the first page".
            assert_eq!(
                status, 200,
                "case {case:?}: an empty PageToken asks for the first page, got {status} with \
                 {body}"
            );
            let served: BTreeSet<String> = paths_by_id(case, &body).into_keys().collect();
            assert_eq!(
                served,
                BTreeSet::from([portfolio_id.clone()]),
                "case {case:?}: expected the stored path, got {served:?} in {body}"
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
        ("id of another resource kind", "port-zzzzzzzzzzzz"),
        ("not an id of any kind", "lorem"),
        ("at the Id length bound", "x"),
    ] {
        let (status, refused) = call(&router, "ListLaunchPaths", &json!({"ProductId": unknown})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for ProductId {unknown:?}, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error in {refused}"
        );
    }

    // None of those refusals disturbed the stored association.
    let (status, listed) = call(&router, "ListLaunchPaths", &json!({"ProductId": product_id})).await;
    assert_eq!(
        status, 200,
        "the stored path must still be listed, got {status} with {listed}"
    );
}

/// Listing the paths reports them and changes nothing: the same request repeated — and a
/// read of the portfolio and the product in between — answers the same document.
#[tokio::test]
async fn listing_the_paths_changes_nothing() {
    let router = emulator();
    let portfolio_id = create_portfolio(
        &router,
        "alpha-portfolio",
        "acme",
        &[("team", "ops")],
        "portfolio",
    )
    .await;
    let product_id = create_product(&router, "listed-product", "product-token").await;
    associate(&router, &product_id, &portfolio_id).await;
    create_constraint(
        &router,
        &portfolio_id,
        &product_id,
        "TEMPLATE",
        "the template rules",
        "constraint",
    )
    .await;

    let request = json!({"ProductId": product_id});
    let (status, first) = call(&router, "ListLaunchPaths", &request).await;
    assert_eq!(
        status, 200,
        "the first list must be served, got {status} with {first}"
    );
    let (status, described) = call(&router, "DescribePortfolio", &json!({"Id": portfolio_id})).await;
    assert_eq!(
        status, 200,
        "DescribePortfolio must be served, got {status} with {described}"
    );
    let (status, second) = call(&router, "ListLaunchPaths", &request).await;
    assert_eq!(
        status, 200,
        "the second list must be served, got {status} with {second}"
    );
    assert_eq!(
        paths_by_id("repeated read", &second),
        paths_by_id("repeated read", &first),
        "case \"repeated read\": a read changed what a later caller sees: {first} then {second}"
    );
}
