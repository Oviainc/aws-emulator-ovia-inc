// SPDX-License-Identifier: CC0-1.0

//! `AssociateTagOptionWithResource` over the wire, driven as awsJson1_1 requests through
//! the same `MultiServiceRouter` the production emulator mounts.
//!
//! The rule under test: associating a TagOption with a resource stores the (resource,
//! TagOption) pair, so every later read of that resource reports exactly the stored
//! TagOptions — the same way for a portfolio as for a product — a pair already stored is
//! the modelled `DuplicateResourceException`, a TagOption or a resource this account does
//! not hold is `ResourceNotFoundException`, a resource past the per-resource quota is
//! `LimitExceededException`, and a TagOption a resource still carries cannot be deleted.
//!
//! Each row varies what the rule generalises over: the KIND of resource, how many
//! TagOptions one resource carries, how many resources one TagOption reaches, whether two
//! associated values share a key, and the TagOption's active state. Every assertion is
//! about STORED state: a reported TagOption is compared against the detail
//! `DescribeTagOption` reports for it, never against the body of the request that made
//! the association.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_servicecatalog_aws242servicecatalogservice::state::MAX_TAG_OPTIONS_PER_RESOURCE;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_portfolio, create_product, emulator, error_code};

/// `case` as an `IdempotencyToken`: the modelled pattern is
/// `^[a-zA-Z0-9][a-zA-Z0-9_-]*$`, which a prose case name is not.
fn token(case: &str) -> String {
    case.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The kind of resource a row associates its TagOptions with. `ResourceId` is "the
/// resource identifier" of a portfolio OR a product, so both kinds are rows of the same
/// table, each read back through its own admin reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceKind {
    Portfolio,
    Product,
}

impl ResourceKind {
    /// The reader that reports this kind of resource's `TagOptions`
    /// (`smithy_query query="operation" operation="DescribePortfolio,DescribeProductAsAdmin"`:
    /// both carry a `TagOptions` member targeting `TagOptionDetails`).
    fn reader(self) -> &'static str {
        match self {
            ResourceKind::Portfolio => "DescribePortfolio",
            ResourceKind::Product => "DescribeProductAsAdmin",
        }
    }
}

/// One TagOption every case seeds: its key, its value, and whether a follow-up
/// `UpdateTagOption` deactivates it before it is associated.
struct TagOptionSeed {
    key: &'static str,
    value: &'static str,
    active: bool,
}

/// The seeded TagOptions. Two share a key with different values (a key may carry several
/// values), one carries a value the modelled `TagOptionValue` pattern
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` admits beyond plain words, and one is deactivated, so
/// the rows span the active state as well as the key/value shape.
const TAG_OPTIONS: [TagOptionSeed; 5] = [
    TagOptionSeed {
        key: "env",
        value: "dev",
        active: true,
    },
    TagOptionSeed {
        key: "env",
        value: "prod",
        active: true,
    },
    TagOptionSeed {
        key: "cost.center",
        value: "ou=platform/team=core",
        active: true,
    },
    TagOptionSeed {
        key: "owner",
        value: "platform@example.com",
        active: true,
    },
    TagOptionSeed {
        key: "retired",
        value: "yes",
        active: false,
    },
];

/// One association row: the kind of resource, and which seeded TagOptions it carries by
/// index into [`TAG_OPTIONS`].
struct AssociationCase {
    case: &'static str,
    kind: ResourceKind,
    options: &'static [usize],
}

/// The rows: both resource kinds, from no association through a single one to several,
/// including two values of one key and a deactivated TagOption, and one TagOption shared
/// by two resources (index 0 appears in several rows).
fn association_cases() -> Vec<AssociationCase> {
    vec![
        AssociationCase {
            case: "portfolio with no TagOption",
            kind: ResourceKind::Portfolio,
            options: &[],
        },
        AssociationCase {
            case: "portfolio with one TagOption",
            kind: ResourceKind::Portfolio,
            options: &[0],
        },
        AssociationCase {
            case: "portfolio with two values of one key",
            kind: ResourceKind::Portfolio,
            options: &[0, 1],
        },
        AssociationCase {
            case: "portfolio with a deactivated TagOption among several",
            kind: ResourceKind::Portfolio,
            options: &[2, 3, 4],
        },
        AssociationCase {
            case: "product with no TagOption",
            kind: ResourceKind::Product,
            options: &[],
        },
        AssociationCase {
            case: "product with one TagOption, shared with a portfolio",
            kind: ResourceKind::Product,
            options: &[0],
        },
        AssociationCase {
            case: "product with every seeded TagOption",
            kind: ResourceKind::Product,
            options: &[0, 1, 2, 3, 4],
        },
    ]
}

/// Create one TagOption and answer the id the service minted.
async fn create_tag_option(router: &MultiServiceRouter, key: &str, value: &str) -> String {
    let (status, created) =
        call(router, "CreateTagOption", &json!({"Key": key, "Value": value})).await;
    assert_eq!(
        status, 200,
        "seed CreateTagOption({key:?}, {value:?}): expected 200, got {status} with {created}"
    );
    created["TagOptionDetail"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("seed CreateTagOption({key:?}): no Id in {created}"))
        .to_string()
}

/// Seed every TagOption in [`TAG_OPTIONS`], deactivating the ones the seed marks
/// inactive, and answer their ids in table order.
async fn seed_tag_options(router: &MultiServiceRouter) -> Vec<String> {
    let mut ids = Vec::new();
    for seed in &TAG_OPTIONS {
        let id = create_tag_option(router, seed.key, seed.value).await;
        if !seed.active {
            let (status, updated) = call(
                router,
                "UpdateTagOption",
                &json!({"Id": id, "Active": false}),
            )
            .await;
            assert_eq!(
                status, 200,
                "seed UpdateTagOption({id}): expected 200, got {status} with {updated}"
            );
        }
        ids.push(id);
    }
    ids
}

/// Seed one resource of `kind` and answer its id.
async fn seed_resource(router: &MultiServiceRouter, kind: ResourceKind, name: &str) -> String {
    match kind {
        ResourceKind::Portfolio => {
            create_portfolio(router, name, "acme", &[], &token(name)).await
        }
        ResourceKind::Product => create_product(router, name, &token(name)).await,
    }
}

/// Associate one TagOption with one resource, expecting the call to be served.
async fn associate(router: &MultiServiceRouter, case: &str, resource_id: &str, option_id: &str) {
    let (status, body) = call(
        router,
        "AssociateTagOptionWithResource",
        &json!({"ResourceId": resource_id, "TagOptionId": option_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: AssociateTagOptionWithResource({resource_id}, {option_id}): expected 200, \
         got {status} with {body}"
    );
}

/// The `TagOptions` the reader for `kind` reports for `resource_id`.
async fn reported_tag_options(
    router: &MultiServiceRouter,
    case: &str,
    kind: ResourceKind,
    resource_id: &str,
) -> Vec<Value> {
    let reader = kind.reader();
    let (status, body) = call(router, reader, &json!({"Id": resource_id})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {reader}({resource_id}), got {status} with {body}"
    );
    body["TagOptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: {reader} reported no TagOptions list: {body}"))
        .clone()
}

/// The ids in a reported `TagOptions` list.
fn reported_ids(case: &str, reported: &[Value]) -> BTreeSet<String> {
    reported
        .iter()
        .map(|detail| {
            detail["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a reported TagOption has no Id: {detail}"))
                .to_string()
        })
        .collect()
}

/// The detail `DescribeTagOption` reports for `option_id` — the stored record every
/// reader of an association must agree with.
async fn described_detail(router: &MultiServiceRouter, case: &str, option_id: &str) -> Value {
    let (status, body) = call(router, "DescribeTagOption", &json!({"Id": option_id})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from DescribeTagOption({option_id}), got {status} with {body}"
    );
    body["TagOptionDetail"].clone()
}

#[tokio::test]
async fn a_resources_reported_tag_options_are_its_stored_associations() {
    let router = emulator();
    let option_ids = seed_tag_options(&router).await;

    for case in association_cases() {
        let resource_id = seed_resource(&router, case.kind, case.case).await;

        // Before any association, the resource reports no TagOption at all: a reader
        // that answered one here would not be reading stored state.
        let before = reported_tag_options(&router, case.case, case.kind, &resource_id).await;
        assert!(
            before.is_empty(),
            "case {:?}: a resource with no association must report no TagOption, got {before:?}",
            case.case
        );

        for index in case.options {
            associate(&router, case.case, &resource_id, &option_ids[*index]).await;
        }

        let expected: BTreeSet<String> = case
            .options
            .iter()
            .map(|index| option_ids[*index].clone())
            .collect();
        let reported = reported_tag_options(&router, case.case, case.kind, &resource_id).await;
        assert_eq!(
            reported_ids(case.case, &reported),
            expected,
            "case {:?}: expected exactly the associated TagOptions {expected:?}, got {reported:?}",
            case.case
        );

        // Each reported entry is the TagOption's own stored detail, deactivated ones
        // included: an association reports the record, not a copy of the request.
        for detail in &reported {
            let id = detail["Id"].as_str().unwrap_or_default();
            let stored = described_detail(&router, case.case, id).await;
            assert_eq!(
                *detail, stored,
                "case {:?}: the reported TagOption must be the stored one {stored}, got {detail}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn one_tag_option_reaches_every_resource_it_is_associated_with() {
    let router = emulator();
    let option_id = create_tag_option(&router, "env", "dev").await;
    let portfolio_id = create_portfolio(&router, "shared-portfolio", "acme", &[], "shared-p").await;
    let product_id = create_product(&router, "shared-product", "shared-r").await;

    // A TagOption is not consumed by an association: associating one with a second
    // resource leaves the first association standing.
    associate(&router, "shared: portfolio", &portfolio_id, &option_id).await;
    associate(&router, "shared: product", &product_id, &option_id).await;

    for (case, kind, resource_id) in [
        ("shared: portfolio", ResourceKind::Portfolio, &portfolio_id),
        ("shared: product", ResourceKind::Product, &product_id),
    ] {
        let reported = reported_tag_options(&router, case, kind, resource_id).await;
        assert_eq!(
            reported_ids(case, &reported),
            BTreeSet::from([option_id.clone()]),
            "case {case:?}: expected the shared TagOption {option_id}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn associating_a_stored_pair_again_is_the_duplicate() {
    let router = emulator();
    let option_id = create_tag_option(&router, "env", "dev").await;
    let other_id = create_tag_option(&router, "env", "prod").await;
    let portfolio_id = create_portfolio(&router, "dup-portfolio", "acme", &[], "dup-p").await;
    let product_id = create_product(&router, "dup-product", "dup-r").await;

    for (case, kind, resource_id) in [
        ("duplicate on a portfolio", ResourceKind::Portfolio, &portfolio_id),
        ("duplicate on a product", ResourceKind::Product, &product_id),
    ] {
        associate(&router, case, resource_id, &option_id).await;
        let (status, refused) = call(
            &router,
            "AssociateTagOptionWithResource",
            &json!({"ResourceId": resource_id, "TagOptionId": option_id}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a pair already stored must be refused, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "DuplicateResourceException",
            "case {case:?}: expected DuplicateResourceException, got {refused}"
        );

        // The refusal changed nothing, and another TagOption still associates.
        associate(&router, case, resource_id, &other_id).await;
        let reported = reported_tag_options(&router, case, kind, resource_id).await;
        assert_eq!(
            reported_ids(case, &reported),
            BTreeSet::from([option_id.clone(), other_id.clone()]),
            "case {case:?}: expected both TagOptions once each, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_tag_option_or_resource_is_not_found() {
    let router = emulator();
    let option_id = create_tag_option(&router, "env", "dev").await;
    let portfolio_id = create_portfolio(&router, "nf-portfolio", "acme", &[], "nf-p").await;
    let product_id = create_product(&router, "nf-product", "nf-r").await;

    // Rows: a TagOption id nothing minted; an id of each resource prefix nothing
    // minted; a well-formed id of the WRONG kind (a TagOption id is no resource, a
    // portfolio id is no TagOption), so the lookups cannot be reading one map for the
    // other.
    let cases: [(&str, &str, &str); 6] = [
        ("unknown TagOption", portfolio_id.as_str(), "tag-zzzzzzzzzzzz"),
        ("unknown portfolio", "port-zzzzzzzzzzzz", option_id.as_str()),
        ("unknown product", "prod-zzzzzzzzzzzz", option_id.as_str()),
        ("a TagOption id as the resource", option_id.as_str(), option_id.as_str()),
        ("a portfolio id as the TagOption", portfolio_id.as_str(), portfolio_id.as_str()),
        ("a product id as the TagOption", product_id.as_str(), product_id.as_str()),
    ];

    for (case, resource_id, option) in cases {
        let (status, refused) = call(
            &router,
            "AssociateTagOptionWithResource",
            &json!({"ResourceId": resource_id, "TagOptionId": option}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {refused}"
        );
    }

    // Nothing above was stored: both seeded resources still report no TagOption.
    for (case, kind, resource_id) in [
        ("nothing stored on the portfolio", ResourceKind::Portfolio, &portfolio_id),
        ("nothing stored on the product", ResourceKind::Product, &product_id),
    ] {
        let reported = reported_tag_options(&router, case, kind, resource_id).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: a refused association must store nothing, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_resource_takes_tag_options_up_to_the_quota_and_no_more() {
    let router = emulator();
    let portfolio_id = create_portfolio(&router, "quota-portfolio", "acme", &[], "quota-p").await;

    // Every association up to the quota the emulator enforces is stored, and the first
    // one past it is refused: the bound itself and the first value outside it.
    let mut associated = BTreeSet::new();
    for index in 0..MAX_TAG_OPTIONS_PER_RESOURCE {
        let option_id = create_tag_option(&router, "env", &format!("value-{index}")).await;
        associate(&router, "within the quota", &portfolio_id, &option_id).await;
        associated.insert(option_id);
    }

    let one_too_many = create_tag_option(&router, "env", "one-too-many").await;
    let (status, refused) = call(
        &router,
        "AssociateTagOptionWithResource",
        &json!({"ResourceId": portfolio_id, "TagOptionId": one_too_many}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"past the quota\": expected a 400 refusal, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "case \"past the quota\": expected LimitExceededException, got {refused}"
    );

    let reported = reported_tag_options(
        &router,
        "past the quota",
        ResourceKind::Portfolio,
        &portfolio_id,
    )
    .await;
    assert_eq!(
        reported_ids("past the quota", &reported),
        associated,
        "case \"past the quota\": the refused association must store nothing, got {reported:?}"
    );
}

#[tokio::test]
async fn a_tag_option_a_resource_carries_cannot_be_deleted() {
    let router = emulator();
    let held_id = create_tag_option(&router, "env", "dev").await;
    let free_id = create_tag_option(&router, "env", "prod").await;
    let product_id = create_product(&router, "in-use-product", "in-use-r").await;
    associate(&router, "in use", &product_id, &held_id).await;

    let (status, refused) = call(&router, "DeleteTagOption", &json!({"Id": held_id})).await;
    assert_eq!(
        status, 400,
        "case \"in use\": a TagOption a resource carries must not be deleted, got {status} with \
         {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ResourceInUseException",
        "case \"in use\": expected ResourceInUseException, got {refused}"
    );

    // The refused delete left both the TagOption and its association in place.
    let reported = reported_tag_options(&router, "in use", ResourceKind::Product, &product_id).await;
    assert_eq!(
        reported_ids("in use", &reported),
        BTreeSet::from([held_id.clone()]),
        "case \"in use\": the association must survive a refused delete, got {reported:?}"
    );

    // A TagOption no resource carries is deleted as before.
    let (status, deleted) = call(&router, "DeleteTagOption", &json!({"Id": free_id})).await;
    assert_eq!(
        status, 200,
        "case \"not in use\": expected 200 from DeleteTagOption({free_id}), got {status} with \
         {deleted}"
    );
}

#[tokio::test]
async fn reading_a_resource_does_not_change_its_associations() {
    let router = emulator();
    let option_id = create_tag_option(&router, "env", "dev").await;
    let portfolio_id = create_portfolio(&router, "read-portfolio", "acme", &[], "read-p").await;
    associate(&router, "repeated read", &portfolio_id, &option_id).await;

    let first =
        reported_tag_options(&router, "repeated read", ResourceKind::Portfolio, &portfolio_id).await;
    let second =
        reported_tag_options(&router, "repeated read", ResourceKind::Portfolio, &portfolio_id).await;
    assert_eq!(
        first, second,
        "case \"repeated read\": a read must not change what the next caller sees: {first:?} then \
         {second:?}"
    );
}
