// SPDX-License-Identifier: CC0-1.0

//! `ListResourcesForTagOption` over the wire, driven as awsJson1_1 requests through the
//! same `MultiServiceRouter` the production emulator mounts.
//!
//! The rule under test: listing the resources of a TagOption reports, from stored state,
//! exactly the portfolios and products the (resource, TagOption) pairs hold for it —
//! narrowed to one kind when `ResourceType` names one — each as the same stored record
//! that resource's own reader reports, paged by `PageSize` with an opaque `PageToken`
//! cursor, and changing nothing.
//!
//! Each row varies what the rule generalises over: the KIND of resource, how many
//! resources one TagOption reaches (none, one, several of each kind), which kind the
//! request narrows to, and the page size the walk is cut into. A reported record is
//! compared against what `DescribePortfolio` / `DescribeProductAsAdmin` report for that
//! resource, never against the body of the request that created it.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_portfolio, create_product, emulator, error_code};

/// The kinds `ResourceType` names. The model documents exactly these two values for the
/// member (`smithy_query query="doc" shape="ListResourcesForTagOptionInput"
/// member="ResourceType"`), and they are the two kinds a `ResourceId` can identify.
const RESOURCE_TYPE_PORTFOLIO: &str = "Portfolio";
const RESOURCE_TYPE_PRODUCT: &str = "Product";

/// The kind of resource a seeded row is, which fixes both the `ResourceType` that selects
/// it and the admin reader that reports its stored record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceKind {
    Portfolio,
    Product,
}

impl ResourceKind {
    fn resource_type(self) -> &'static str {
        match self {
            ResourceKind::Portfolio => RESOURCE_TYPE_PORTFOLIO,
            ResourceKind::Product => RESOURCE_TYPE_PRODUCT,
        }
    }

    /// The reader that reports this kind's stored record and its `TagOptions`.
    fn reader(self) -> &'static str {
        match self {
            ResourceKind::Portfolio => "DescribePortfolio",
            ResourceKind::Product => "DescribeProductAsAdmin",
        }
    }
}

/// One seeded resource: its kind, the name it is created under, and the description a
/// follow-up update gives it (`None` leaves it without one, so the rows span a
/// `ResourceDetail` whose `Description` is absent as well as one that carries it).
struct ResourceSeed {
    kind: ResourceKind,
    name: &'static str,
    description: Option<&'static str>,
}

/// The seeded resources: three of each kind, with and without a description.
const RESOURCES: [ResourceSeed; 6] = [
    ResourceSeed {
        kind: ResourceKind::Portfolio,
        name: "alpha-portfolio",
        description: Some("the first portfolio"),
    },
    ResourceSeed {
        kind: ResourceKind::Portfolio,
        name: "beta-portfolio",
        description: None,
    },
    ResourceSeed {
        kind: ResourceKind::Portfolio,
        name: "gamma-portfolio",
        description: Some("ou=platform/team=core"),
    },
    ResourceSeed {
        kind: ResourceKind::Product,
        name: "alpha-product",
        description: Some("the first product"),
    },
    ResourceSeed {
        kind: ResourceKind::Product,
        name: "beta-product",
        description: None,
    },
    ResourceSeed {
        kind: ResourceKind::Product,
        name: "gamma-product",
        description: Some("a product with a long-ish description"),
    },
];

/// The seeded TagOptions, each with the resources (by index into [`RESOURCES`]) it is
/// associated with: one reaching both kinds, one reaching only portfolios, one reaching
/// only products, and one reaching nothing at all.
struct TagOptionSeed {
    key: &'static str,
    value: &'static str,
    resources: &'static [usize],
}

const TAG_OPTIONS: [TagOptionSeed; 4] = [
    TagOptionSeed {
        key: "env",
        value: "dev",
        resources: &[0, 1, 3, 4, 5],
    },
    TagOptionSeed {
        key: "env",
        value: "prod",
        resources: &[2],
    },
    TagOptionSeed {
        key: "cost.center",
        value: "core",
        resources: &[5],
    },
    TagOptionSeed {
        key: "retired",
        value: "yes",
        resources: &[],
    },
];

/// `name` as an `IdempotencyToken`: the modelled pattern is
/// `^[a-zA-Z0-9][a-zA-Z0-9_-]*$`, which a prose name is not.
fn token(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Seed every resource in [`RESOURCES`] and answer their ids in table order.
async fn seed_resources(router: &MultiServiceRouter) -> Vec<String> {
    let mut ids = Vec::new();
    for seed in &RESOURCES {
        let id = match seed.kind {
            ResourceKind::Portfolio => {
                create_portfolio(router, seed.name, "acme", &[], &token(seed.name)).await
            }
            ResourceKind::Product => create_product(router, seed.name, &token(seed.name)).await,
        };
        if let Some(description) = seed.description {
            let updater = match seed.kind {
                ResourceKind::Portfolio => "UpdatePortfolio",
                ResourceKind::Product => "UpdateProduct",
            };
            let (status, updated) =
                call(router, updater, &json!({"Id": id, "Description": description})).await;
            assert_eq!(
                status, 200,
                "seed {updater}({id}): expected 200, got {status} with {updated}"
            );
        }
        ids.push(id);
    }
    ids
}

/// Seed every TagOption in [`TAG_OPTIONS`], associate it with the resources its row
/// names, and answer their ids in table order.
async fn seed_tag_options(router: &MultiServiceRouter, resource_ids: &[String]) -> Vec<String> {
    let mut ids = Vec::new();
    for seed in &TAG_OPTIONS {
        let (status, created) = call(
            router,
            "CreateTagOption",
            &json!({"Key": seed.key, "Value": seed.value}),
        )
        .await;
        assert_eq!(
            status, 200,
            "seed CreateTagOption({:?}): expected 200, got {status} with {created}",
            seed.key
        );
        let option_id = created["TagOptionDetail"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("seed CreateTagOption: no Id in {created}"))
            .to_string();
        for index in seed.resources {
            let resource_id = &resource_ids[*index];
            let (status, body) = call(
                router,
                "AssociateTagOptionWithResource",
                &json!({"ResourceId": resource_id, "TagOptionId": option_id}),
            )
            .await;
            assert_eq!(
                status, 200,
                "seed AssociateTagOptionWithResource({resource_id}, {option_id}): expected 200, \
                 got {status} with {body}"
            );
        }
        ids.push(option_id);
    }
    ids
}

/// One `ListResourcesForTagOption` call, expected to be served.
async fn list(
    router: &MultiServiceRouter,
    case: &str,
    tag_option_id: &str,
    resource_type: Option<&str>,
    page_size: Option<i64>,
    page_token: Option<&str>,
) -> Value {
    let mut request = json!({"TagOptionId": tag_option_id});
    if let Some(kind) = resource_type {
        request["ResourceType"] = json!(kind);
    }
    if let Some(size) = page_size {
        request["PageSize"] = json!(size);
    }
    if let Some(cursor) = page_token {
        request["PageToken"] = json!(cursor);
    }
    let (status, body) = call(router, "ListResourcesForTagOption", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListResourcesForTagOption({request}), got {status} with \
         {body}"
    );
    body
}

/// The `ResourceDetails` list of a served response.
fn resource_details(case: &str, body: &Value) -> Vec<Value> {
    body["ResourceDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no ResourceDetails list in {body}"))
        .clone()
}

/// The ids of a reported `ResourceDetails` list, in the order they were reported.
fn reported_ids(case: &str, details: &[Value]) -> Vec<String> {
    details
        .iter()
        .map(|detail| {
            detail["Id"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a reported resource has no Id: {detail}")
                })
                .to_string()
        })
        .collect()
}

/// The five `ResourceDetail` members, so a reported record and a stored one are compared
/// member by member rather than by the shape they arrived in.
fn detail_members(detail: &Value) -> Value {
    json!({
        "Id": detail["Id"],
        "ARN": detail["ARN"],
        "Name": detail["Name"],
        "Description": detail["Description"],
        "CreatedTime": detail["CreatedTime"],
    })
}

/// The resource's own stored record, rendered as the same five members: a portfolio's
/// `Name` is its `DisplayName` and a product's `Description` is its `ShortDescription`,
/// which is how each kind reports itself through its admin reader.
async fn stored_detail_members(
    router: &MultiServiceRouter,
    case: &str,
    kind: ResourceKind,
    resource_id: &str,
) -> Value {
    let reader = kind.reader();
    let (status, body) = call(router, reader, &json!({"Id": resource_id})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {reader}({resource_id}), got {status} with {body}"
    );
    match kind {
        ResourceKind::Portfolio => {
            let detail = &body["PortfolioDetail"];
            json!({
                "Id": detail["Id"],
                "ARN": detail["ARN"],
                "Name": detail["DisplayName"],
                "Description": detail["Description"],
                "CreatedTime": detail["CreatedTime"],
            })
        }
        ResourceKind::Product => {
            let detail = &body["ProductViewDetail"];
            let summary = &detail["ProductViewSummary"];
            json!({
                "Id": summary["ProductId"],
                "ARN": detail["ProductARN"],
                "Name": summary["Name"],
                "Description": summary["ShortDescription"],
                "CreatedTime": detail["CreatedTime"],
            })
        }
    }
}

/// The ids of the `TagOptions` a resource's own reader reports.
async fn tag_options_of_resource(
    router: &MultiServiceRouter,
    case: &str,
    kind: ResourceKind,
    resource_id: &str,
) -> BTreeSet<String> {
    let reader = kind.reader();
    let (status, body) = call(router, reader, &json!({"Id": resource_id})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {reader}({resource_id}), got {status} with {body}"
    );
    body["TagOptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: {reader} reported no TagOptions: {body}"))
        .iter()
        .map(|detail| {
            detail["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a TagOption has no Id: {detail}"))
                .to_string()
        })
        .collect()
}

/// The ids the rule expects for one (TagOption, narrowing) pair, in the reported order:
/// the associated resources of the requested kind, in resource-id order.
fn expected_ids(option: &TagOptionSeed, resource_ids: &[String], narrow: Option<&str>) -> Vec<String> {
    let mut ids: Vec<String> = option
        .resources
        .iter()
        .filter(|index| narrow.is_none_or(|kind| RESOURCES[**index].kind.resource_type() == kind))
        .map(|index| resource_ids[*index].clone())
        .collect();
    ids.sort();
    ids
}

/// Walk every page of one list with `page_size`, asserting no page is larger than the
/// bound, and answer the ids in the order they were reported across the whole walk.
async fn walk_pages(
    router: &MultiServiceRouter,
    case: &str,
    tag_option_id: &str,
    resource_type: Option<&str>,
    page_size: i64,
) -> Vec<String> {
    let mut walked = Vec::new();
    let mut cursor: Option<String> = None;
    // The seeded set is small, so a walk needing more pages than this is a cursor that
    // does not advance, which must fail rather than loop.
    for _ in 0..(RESOURCES.len() + 2) {
        let body = list(
            router,
            case,
            tag_option_id,
            resource_type,
            Some(page_size),
            cursor.as_deref(),
        )
        .await;
        let details = resource_details(case, &body);
        assert!(
            page_size == 0 || details.len() <= page_size as usize,
            "case {case:?}: a page must hold at most PageSize={page_size} resources, got {}",
            details.len()
        );
        walked.extend(reported_ids(case, &details));
        match body["PageToken"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => return walked,
        }
    }
    panic!("case {case:?}: the walk never ran out of pages; walked {walked:?}");
}

#[tokio::test]
async fn a_tag_options_resources_are_its_stored_associations() {
    let router = emulator();
    let resource_ids = seed_resources(&router).await;
    let option_ids = seed_tag_options(&router, &resource_ids).await;

    // Rows: every seeded TagOption (reaching both kinds, one kind, the other kind, and
    // nothing) crossed with every narrowing (none, each documented kind).
    for (index, option) in TAG_OPTIONS.iter().enumerate() {
        for narrow in [None, Some(RESOURCE_TYPE_PORTFOLIO), Some(RESOURCE_TYPE_PRODUCT)] {
            let case = format!(
                "TagOption {}={} narrowed to {narrow:?}",
                option.key, option.value
            );
            let body = list(&router, &case, &option_ids[index], narrow, None, None).await;
            let details = resource_details(&case, &body);
            let expected = expected_ids(option, &resource_ids, narrow);
            assert_eq!(
                reported_ids(&case, &details),
                expected,
                "case {case:?}: expected exactly {expected:?} in resource-id order, got {details:?}"
            );
            assert!(
                body["PageToken"].is_null(),
                "case {case:?}: a complete page must carry no PageToken, got {body}"
            );

            // Every reported record is the resource's own stored record.
            for detail in &details {
                let id = detail["Id"].as_str().unwrap_or_default();
                let position = resource_ids
                    .iter()
                    .position(|seeded| seeded == id)
                    .unwrap_or_else(|| panic!("case {case:?}: {id} is not a seeded resource"));
                let kind = RESOURCES[position].kind;
                let stored = stored_detail_members(&router, &case, kind, id).await;
                assert_eq!(
                    detail_members(detail),
                    stored,
                    "case {case:?}: the reported resource must be the stored one {stored}, got \
                     {detail}"
                );
            }
        }
    }

    // The inverse holds over the same stored pairs: a resource reports a TagOption
    // exactly when that TagOption reports the resource.
    for (position, seed) in RESOURCES.iter().enumerate() {
        let case = format!("inverse for {}", seed.name);
        let carried =
            tag_options_of_resource(&router, &case, seed.kind, &resource_ids[position]).await;
        let expected: BTreeSet<String> = TAG_OPTIONS
            .iter()
            .enumerate()
            .filter(|(_, option)| option.resources.contains(&position))
            .map(|(index, _)| option_ids[index].clone())
            .collect();
        assert_eq!(
            carried, expected,
            "case {case:?}: expected the resource to carry {expected:?}, got {carried:?}"
        );
    }
}

#[tokio::test]
async fn a_paged_walk_reports_every_resource_exactly_once() {
    let router = emulator();
    let resource_ids = seed_resources(&router).await;
    let option_ids = seed_tag_options(&router, &resource_ids).await;

    // The TagOption reaching the most resources, walked at every page size from one
    // resource per page up to the modelled `PageSize` maximum of 20, plus the default 0
    // which asks for no caller-imposed bound.
    let option = &TAG_OPTIONS[0];
    let expected = expected_ids(option, &resource_ids, None);
    for page_size in [0, 1, 2, 3, 5, 20] {
        let case = format!("walk at PageSize={page_size}");
        let walked = walk_pages(&router, &case, &option_ids[0], None, page_size).await;
        assert_eq!(
            walked, expected,
            "case {case:?}: a paged walk must report every resource exactly once, in one order: \
             expected {expected:?}, got {walked:?}"
        );
    }

    // The same walk narrowed to one kind pages over that kind only.
    for narrow in [RESOURCE_TYPE_PORTFOLIO, RESOURCE_TYPE_PRODUCT] {
        let case = format!("walk at PageSize=1 narrowed to {narrow}");
        let walked = walk_pages(&router, &case, &option_ids[0], Some(narrow), 1).await;
        let expected = expected_ids(option, &resource_ids, Some(narrow));
        assert_eq!(
            walked, expected,
            "case {case:?}: expected {expected:?}, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_bad_request_is_refused_and_a_known_tag_option_is_not() {
    let router = emulator();
    let resource_ids = seed_resources(&router).await;
    let option_ids = seed_tag_options(&router, &resource_ids).await;
    let known = option_ids[0].clone();

    // Rows: a TagOption id nothing minted, and well-formed ids of the WRONG kind (a
    // portfolio id and a product id are no TagOption), so the lookup cannot be reading
    // one map for another.
    for (case, tag_option_id) in [
        ("unminted TagOption id", "tag-zzzzzzzzzzzz"),
        ("a portfolio id as the TagOption", resource_ids[0].as_str()),
        ("a product id as the TagOption", resource_ids[3].as_str()),
    ] {
        let (status, refused) = call(
            &router,
            "ListResourcesForTagOption",
            &json!({"TagOptionId": tag_option_id}),
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

    // A `ResourceType` naming neither documented kind is a bad parameter: the wrong case
    // of each kind, a kind that is no resource a TagOption can reach, and a value with
    // trailing space.
    for (case, resource_type) in [
        ("lowercase portfolio", "portfolio"),
        ("uppercase PRODUCT", "PRODUCT"),
        ("a kind that is no TagOption resource", "ProvisionedProduct"),
        ("a kind with trailing space", "Product "),
    ] {
        let (status, refused) = call(
            &router,
            "ListResourcesForTagOption",
            &json!({"TagOptionId": known, "ResourceType": resource_type}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidParametersException",
            "case {case:?}: expected InvalidParametersException, got {refused}"
        );
    }

    // A `PageToken` this service did not mint is a bad parameter too: a token it cannot
    // decode at all, and one whose decoded bytes are not an id.
    for (case, page_token) in [
        ("a token that is not a cursor", "not-a-page-token"),
        ("a token decoding to a non-id", &hex::encode("port /x")),
    ] {
        let (status, refused) = call(
            &router,
            "ListResourcesForTagOption",
            &json!({"TagOptionId": known, "PageToken": page_token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidParametersException",
            "case {case:?}: expected InvalidParametersException, got {refused}"
        );
    }

    // An empty `ResourceType` names no kind, so it narrows nothing: the same answer the
    // request without the member gets.
    let narrowed = list(&router, "empty ResourceType", &known, Some(""), None, None).await;
    let unnarrowed = list(&router, "absent ResourceType", &known, None, None, None).await;
    assert_eq!(
        narrowed, unnarrowed,
        "case \"empty ResourceType\": expected the unnarrowed answer {unnarrowed}, got {narrowed}"
    );
}

#[tokio::test]
async fn listing_a_tag_options_resources_changes_nothing() {
    let router = emulator();
    let resource_ids = seed_resources(&router).await;
    let option_ids = seed_tag_options(&router, &resource_ids).await;
    let case = "repeated read";

    let first = list(&router, case, &option_ids[0], None, None, None).await;
    let second = list(&router, case, &option_ids[0], None, None, None).await;
    assert_eq!(
        first, second,
        "case {case:?}: a read must not change what the next caller sees: {first} then {second}"
    );

    // Nor does it change the associations the resources themselves report, or let the
    // TagOption be deleted while a resource still carries it.
    let carried = tag_options_of_resource(&router, case, RESOURCES[0].kind, &resource_ids[0]).await;
    assert!(
        carried.contains(&option_ids[0]),
        "case {case:?}: the association must survive a read, got {carried:?}"
    );
    let (status, refused) = call(&router, "DeleteTagOption", &json!({"Id": option_ids[0]})).await;
    assert_eq!(
        status, 400,
        "case {case:?}: a TagOption a resource carries must not be deleted, got {status} with \
         {refused}"
    );

    // A TagOption no resource is associated with reports an empty list, not a refusal.
    let empty = list(&router, "no associations", &option_ids[3], None, None, None).await;
    let details = resource_details("no associations", &empty);
    assert!(
        details.is_empty(),
        "case \"no associations\": expected no resource, got {details:?}"
    );
}
