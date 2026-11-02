// SPDX-License-Identifier: CC0-1.0

//! `CopyProduct` and `DescribeCopyProductStatus` over the wire, driven as awsJson1_1
//! requests through the same `MultiServiceRouter` the production emulator mounts.
//!
//! The rule under test: a copy resolves its `SourceProductArn` against the catalog this
//! account holds — an ARN naming no product it can read is the modelled 400
//! `ResourceNotFoundException` — and then copies the source versions the request selects
//! (every one of them by default), plus the source's tags only when `CopyOptions` names
//! `CopyTags`, into the product `TargetProductId` names or into a new product it mints,
//! answering a `CopyProductToken` that `DescribeCopyProductStatus` reports that copy's
//! status and target product by. The copy is asynchronous, so its status is the
//! non-terminal one until the clock owes its completion, and a product it is still
//! filling in is not yet ready for use. The product a copy mints, like any product no
//! portfolio carries, is out of the END-USER view until a portfolio carries it: that view
//! reaches a product only through a launch path.
//!
//! Every assertion here is about STORED state: a response is checked against what an
//! earlier create, update or copy persisted, read back through `DescribeProductAsAdmin`
//! and `ListProvisioningArtifacts`, never against a value this file hands the service and
//! immediately reads back out of the same response.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    associate_product_with_portfolio, call, create_portfolio, create_product, emulator, error_code,
    is_catalog_id, tag_list, tag_map, ACCOUNT_ID, DEFAULT_REGION,
};

/// How long after the call that started it a copy stays non-terminal, from the service's
/// own constant, plus the slack a test needs to be sure the instant has passed.
fn past_the_due_instant() -> std::time::Duration {
    service_com_amazonaws_servicecatalog_aws242servicecatalogservice::state::PRODUCT_COPY_SUCCEEDS_AFTER
        + std::time::Duration::from_millis(400)
}

/// The most versions one product can carry, from the service's own constant — the bound
/// a copy refuses rather than growing a product past.
const MAX_VERSIONS_PER_PRODUCT: usize =
    service_com_amazonaws_servicecatalog_aws242servicecatalogservice::state::MAX_PROVISIONING_ARTIFACTS_PER_PRODUCT;

/// The string at `pointer` in a response body.
fn string_at(case: &str, body: &Value, pointer: &str) -> String {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no string at {pointer} in {body}"))
        .to_string()
}

/// The stored ARN of `product_id`, read back through `DescribeProductAsAdmin` — the one
/// identifier a copy addresses its source by.
async fn stored_arn(router: &MultiServiceRouter, product_id: &str) -> String {
    let (status, described) = call(router, "DescribeProductAsAdmin", &json!({"Id": product_id}))
        .await;
    assert_eq!(
        status, 200,
        "DescribeProductAsAdmin({product_id}): expected 200, got {status} with {described}"
    );
    string_at(product_id, &described, "/ProductViewDetail/ProductARN")
}

/// The stored `Status` of `product_id`.
async fn stored_status(router: &MultiServiceRouter, product_id: &str) -> String {
    let (status, described) = call(router, "DescribeProductAsAdmin", &json!({"Id": product_id}))
        .await;
    assert_eq!(
        status, 200,
        "DescribeProductAsAdmin({product_id}): expected 200, got {status} with {described}"
    );
    string_at(product_id, &described, "/ProductViewDetail/Status")
}

/// The stored name of `product_id`.
async fn stored_name(router: &MultiServiceRouter, product_id: &str) -> String {
    let (status, described) = call(router, "DescribeProductAsAdmin", &json!({"Id": product_id}))
        .await;
    assert_eq!(
        status, 200,
        "DescribeProductAsAdmin({product_id}): expected 200, got {status} with {described}"
    );
    string_at(
        product_id,
        &described,
        "/ProductViewDetail/ProductViewSummary/Name",
    )
}

/// The stored tags of `product_id`, as key -> value.
async fn stored_tags(router: &MultiServiceRouter, product_id: &str) -> BTreeMap<String, String> {
    let (status, described) = call(router, "DescribeProductAsAdmin", &json!({"Id": product_id}))
        .await;
    assert_eq!(
        status, 200,
        "DescribeProductAsAdmin({product_id}): expected 200, got {status} with {described}"
    );
    tag_map(product_id, &described["Tags"])
}

/// The stored versions of `product_id`, as artifact id -> version name.
async fn stored_versions(
    router: &MultiServiceRouter,
    product_id: &str,
) -> BTreeMap<String, String> {
    let (status, listed) = call(
        router,
        "ListProvisioningArtifacts",
        &json!({"ProductId": product_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListProvisioningArtifacts({product_id}): expected 200, got {status} with {listed}"
    );
    listed["ProvisioningArtifactDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("ListProvisioningArtifacts({product_id}): no list in {listed}"))
        .iter()
        .map(|detail| {
            (
                string_at(product_id, detail, "/Id"),
                string_at(product_id, detail, "/Name"),
            )
        })
        .collect()
}

/// Put `tags` on `product_id` through `UpdateProduct`, so the tags a copy may carry are
/// STORED state rather than something this file hands `CopyProduct`.
async fn tag_product(router: &MultiServiceRouter, product_id: &str, tags: &[(&str, &str)]) {
    let (status, updated) = call(
        router,
        "UpdateProduct",
        &json!({"Id": product_id, "AddTags": tag_list(tags)}),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateProduct({product_id}): expected 200, got {status} with {updated}"
    );
}

/// What one copy's status report says.
async fn copy_status(router: &MultiServiceRouter, case: &str, token: &str) -> (String, String) {
    let (status, reported) = call(
        router,
        "DescribeCopyProductStatus",
        &json!({"CopyProductToken": token}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeCopyProductStatus expected 200, got {status} with {reported}"
    );
    (
        string_at(case, &reported, "/CopyProductStatus"),
        string_at(case, &reported, "/TargetProductId"),
    )
}

/// The tags the source product of every case carries.
const SOURCE_TAGS: [(&str, &str); 2] = [("team", "ops"), ("tier", "gold")];

/// One copy into a new product: what the request selects, and what the stored target
/// must therefore read as.
struct NewProductCase {
    case: &'static str,
    /// `CopyOptions` as the request carries it: `None` leaves the member out.
    copy_options: Option<&'static [&'static str]>,
    /// Whether the request names the source's versions instead of defaulting to all.
    name_versions: bool,
    /// `TargetProductName`, or `None` to default to the source product's name.
    target_name: Option<&'static str>,
    /// Whether the source's tags must end up on the target.
    expect_tags: bool,
}

/// A copy into a new product mints a product carrying the source's versions under the
/// target's own version ids, the name the request asked for, and the source's tags only
/// when `CopyOptions` names `CopyTags`. Rows differ along the copy options (absent, empty,
/// `CopyTags`), whether the versions are named or defaulted, and whether the target is
/// renamed.
#[tokio::test]
async fn a_copy_into_a_new_product_carries_the_selected_versions_and_tags() {
    let router = emulator();
    let source_id = create_product(&router, "copy-source", "tok-copy-source").await;
    tag_product(&router, &source_id, &SOURCE_TAGS).await;
    let source_arn = stored_arn(&router, &source_id).await;
    let source_versions = stored_versions(&router, &source_id).await;
    let source_name = stored_name(&router, &source_id).await;
    let source_tags = stored_tags(&router, &source_id).await;

    let cases: [NewProductCase; 5] = [
        NewProductCase {
            case: "defaults: every version, no tags",
            copy_options: None,
            name_versions: false,
            target_name: None,
            expect_tags: false,
        },
        NewProductCase {
            case: "CopyTags",
            copy_options: Some(&["CopyTags"]),
            name_versions: false,
            target_name: None,
            expect_tags: true,
        },
        NewProductCase {
            case: "an empty copy-options list copies no tags",
            copy_options: Some(&[]),
            name_versions: false,
            target_name: None,
            expect_tags: false,
        },
        NewProductCase {
            case: "named versions, renamed target, CopyTags",
            copy_options: Some(&["CopyTags"]),
            name_versions: true,
            target_name: Some("copy-target-named"),
            expect_tags: true,
        },
        NewProductCase {
            case: "named versions, renamed target, no options",
            copy_options: None,
            name_versions: true,
            target_name: Some("copy-target-plain"),
            expect_tags: false,
        },
    ];

    // Every copy is started first, so the one wait below covers all of them.
    let mut started: Vec<(&str, String, String)> = Vec::new();
    for case in &cases {
        let mut request = json!({
            "SourceProductArn": source_arn,
            "IdempotencyToken": format!("tok-{}", started.len()),
        });
        if let Some(options) = case.copy_options {
            request["CopyOptions"] = json!(options);
        }
        if case.name_versions {
            request["SourceProvisioningArtifactIdentifiers"] = Value::Array(
                source_versions
                    .keys()
                    .map(|id| json!({"Id": id}))
                    .collect(),
            );
        }
        if let Some(name) = case.target_name {
            request["TargetProductName"] = json!(name);
        }

        let label = case.case;
        let (status, copied) = call(&router, "CopyProduct", &request).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} with {copied}"
        );
        let token = string_at(label, &copied, "/CopyProductToken");
        assert!(
            is_catalog_id("copy-product", &token),
            "case {label:?}: expected a minted copy token, got {token:?}"
        );

        // Straight after the call the copy is still owed, and so is the product it is
        // filling in: the transient status is what every reader sees.
        let (status_now, target_id) = copy_status(&router, label, &token).await;
        assert_eq!(
            status_now, "IN_PROGRESS",
            "case {label:?}: the copy is not due yet, got {status_now:?}"
        );
        assert_ne!(
            target_id, source_id,
            "case {label:?}: a copy with no TargetProductId mints its own product, got \
             {target_id:?}"
        );
        let product_status = stored_status(&router, &target_id).await;
        assert_eq!(
            product_status, "CREATING",
            "case {label:?}: the copy is still filling the product in, got {product_status:?}"
        );

        // The stored target carries the source's versions under its OWN version ids,
        // with the names the source's versions have.
        let copied_versions = stored_versions(&router, &target_id).await;
        let mut copied_names: Vec<&String> = copied_versions.values().collect();
        copied_names.sort();
        let mut source_names: Vec<&String> = source_versions.values().collect();
        source_names.sort();
        assert_eq!(
            copied_names, source_names,
            "case {label:?}: the copy carries the source's versions, got {copied_versions:?}"
        );
        for id in copied_versions.keys() {
            assert!(
                is_catalog_id("pa", id) && !source_versions.contains_key(id),
                "case {label:?}: a copied version gets the target's own id, got {id:?}"
            );
        }

        let name = stored_name(&router, &target_id).await;
        assert_eq!(
            name,
            case.target_name.unwrap_or(&source_name),
            "case {label:?}: the target's name defaults to the source's"
        );
        let tags = stored_tags(&router, &target_id).await;
        let expected_tags = if case.expect_tags {
            source_tags.clone()
        } else {
            BTreeMap::new()
        };
        assert_eq!(
            tags, expected_tags,
            "case {label:?}: the source's tags are copied only when CopyOptions names \
             CopyTags"
        );

        started.push((label, token, target_id));
    }

    // Once the clock owes the completion, every copy reads as done and every product it
    // minted is ready — nothing had to poll for it.
    tokio::time::sleep(past_the_due_instant()).await;
    for (label, token, target_id) in &started {
        let (status_later, reported_target) = copy_status(&router, label, token).await;
        assert_eq!(
            status_later, "SUCCEEDED",
            "case {label:?}: the clock owed the completion, got {status_later:?}"
        );
        assert_eq!(
            &reported_target, target_id,
            "case {label:?}: the copy reports the same target product throughout"
        );
        let product_status = stored_status(&router, target_id).await;
        assert_eq!(
            product_status, "CREATED",
            "case {label:?}: the copied product is ready once the copy fell due, got \
             {product_status:?}"
        );
    }
}

/// A copy into a product that already exists adds the selected source versions to THAT
/// product and merges the source's tags into it when asked, leaves the source untouched,
/// and leaves a product that was already ready to use ready.
#[tokio::test]
async fn a_copy_into_an_existing_product_adds_its_versions_to_that_product() {
    let router = emulator();
    let source_id = create_product(&router, "merge-source", "tok-merge-source").await;
    tag_product(&router, &source_id, &SOURCE_TAGS).await;
    let source_arn = stored_arn(&router, &source_id).await;
    let source_versions = stored_versions(&router, &source_id).await;
    let source_tags = stored_tags(&router, &source_id).await;

    let target_id = create_product(&router, "merge-target", "tok-merge-target").await;
    tag_product(&router, &target_id, &[("owner", "platform")]).await;
    let target_versions_before = stored_versions(&router, &target_id).await;
    let target_tags_before = stored_tags(&router, &target_id).await;

    let case = "copy into an existing product";
    let (status, copied) = call(
        &router,
        "CopyProduct",
        &json!({
            "SourceProductArn": source_arn,
            "TargetProductId": target_id,
            "CopyOptions": ["CopyTags"],
            "IdempotencyToken": "tok-merge",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {copied}"
    );
    let token = string_at(case, &copied, "/CopyProductToken");

    let (status_now, reported_target) = copy_status(&router, case, &token).await;
    assert_eq!(
        status_now, "IN_PROGRESS",
        "case {case:?}: the copy is not due yet, got {status_now:?}"
    );
    assert_eq!(
        reported_target, target_id,
        "case {case:?}: the copy reports the product the request named"
    );

    // The target keeps its own version and gains one copy of each selected source
    // version, under an id of its own.
    let target_versions = stored_versions(&router, &target_id).await;
    assert_eq!(
        target_versions.len(),
        target_versions_before.len() + source_versions.len(),
        "case {case:?}: the copy adds the source's versions, got {target_versions:?}"
    );
    for (id, name) in &target_versions_before {
        assert_eq!(
            target_versions.get(id),
            Some(name),
            "case {case:?}: the target keeps the versions it had, got {target_versions:?}"
        );
    }
    for id in target_versions.keys() {
        assert!(
            !source_versions.contains_key(id),
            "case {case:?}: a copied version gets the target's own id, got {id:?}"
        );
    }

    let mut expected_tags = target_tags_before.clone();
    expected_tags.extend(source_tags.clone());
    assert_eq!(
        stored_tags(&router, &target_id).await,
        expected_tags,
        "case {case:?}: CopyTags merges the source's tags into the target's"
    );
    let product_status = stored_status(&router, &target_id).await;
    assert_eq!(
        product_status, "CREATED",
        "case {case:?}: a product that was already ready stays ready, got {product_status:?}"
    );
    assert_eq!(
        stored_name(&router, &target_id).await,
        "merge-target",
        "case {case:?}: an existing target keeps its own name"
    );

    // The source is the thing being read FROM: the copy must not change it.
    assert_eq!(
        stored_versions(&router, &source_id).await,
        source_versions,
        "case {case:?}: the source's versions must not change"
    );
    assert_eq!(
        stored_tags(&router, &source_id).await,
        source_tags,
        "case {case:?}: the source's tags must not change"
    );

    tokio::time::sleep(past_the_due_instant()).await;
    let (status_later, _) = copy_status(&router, case, &token).await;
    assert_eq!(
        status_later, "SUCCEEDED",
        "case {case:?}: the clock owed the completion, got {status_later:?}"
    );
}

/// A copy may not grow one product past the versions a product can carry, and an
/// identifier names a version once however many times the request repeats it: the
/// versions a copy adds are bounded by the source's, whatever the request lists.
#[tokio::test]
async fn a_copy_cannot_grow_a_product_past_the_versions_it_may_carry() {
    let router = emulator();
    let source_id = create_product(&router, "bound-source", "tok-bound-source").await;
    let source_arn = stored_arn(&router, &source_id).await;
    let source_versions = stored_versions(&router, &source_id).await;
    let target_id = create_product(&router, "bound-target", "tok-bound-target").await;

    // A request naming the one source version three times copies it ONCE: an entry
    // identifies a version, so a repeated entry is the same version.
    let repeated: Vec<Value> = source_versions
        .keys()
        .flat_map(|id| std::iter::repeat_n(json!({"Id": id}), 3))
        .collect();
    let case = "a repeated identifier";
    let (status, copied) = call(
        &router,
        "CopyProduct",
        &json!({
            "SourceProductArn": source_arn,
            "TargetProductId": target_id,
            "SourceProvisioningArtifactIdentifiers": Value::Array(repeated),
            "IdempotencyToken": "tok-bound-repeat",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {copied}"
    );
    let after_repeat = stored_versions(&router, &target_id).await;
    assert_eq!(
        after_repeat.len(),
        1 + source_versions.len(),
        "case {case:?}: a repeated identifier copies its version once, got {after_repeat:?}"
    );

    // Copy until the target holds every version it may, then once more: the bound is
    // what refuses the copy, and the refusal stores nothing.
    let mut copies = 1;
    while stored_versions(&router, &target_id).await.len() < MAX_VERSIONS_PER_PRODUCT {
        copies += 1;
        let (status, body) = call(
            &router,
            "CopyProduct",
            &json!({
                "SourceProductArn": source_arn,
                "TargetProductId": target_id,
                "IdempotencyToken": format!("tok-bound-{copies}"),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"copy {copies}\": expected 200 up to the bound, got {status} with {body}"
        );
    }
    let at_the_bound = stored_versions(&router, &target_id).await;
    assert_eq!(
        at_the_bound.len(),
        MAX_VERSIONS_PER_PRODUCT,
        "case \"at the bound\": the target holds every version it may, got {}",
        at_the_bound.len()
    );

    let case = "one copy past the bound";
    let (status, refused) = call(
        &router,
        "CopyProduct",
        &json!({
            "SourceProductArn": source_arn,
            "TargetProductId": target_id,
            "IdempotencyToken": "tok-bound-past",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidParametersException",
        "case {case:?}: unexpected error in {refused}"
    );
    assert_eq!(
        stored_versions(&router, &target_id).await.len(),
        MAX_VERSIONS_PER_PRODUCT,
        "case {case:?}: the refused copy must store nothing"
    );
}

/// What one refused request carries beyond its source ARN.
enum Refusal {
    /// A `SourceProductArn` naming no product this account can read.
    Source(String),
    /// A `TargetProductId` naming no stored product, or the source product itself.
    Target(String),
    /// `SourceProvisioningArtifactIdentifiers` entries this source cannot resolve.
    Versions(Value),
}

/// A copy refuses what it cannot resolve, with the modelled client error for the kind of
/// thing that did not resolve: a source or target the account cannot read is
/// `ResourceNotFoundException`, a version identifier the source does not carry is
/// `InvalidParametersException` — and a refused copy stores nothing.
#[tokio::test]
async fn copy_product_refuses_a_source_target_or_version_it_cannot_resolve() {
    let router = emulator();
    let source_id = create_product(&router, "refusal-source", "tok-refusal-source").await;
    let source_arn = stored_arn(&router, &source_id).await;
    let source_versions = stored_versions(&router, &source_id).await;
    let unknown_version = format!("pa-{}", "0".repeat(12));
    // One entry per version a product may carry, plus one: more versions than any copy
    // can store. Every entry names a version the source really has, so it is the COUNT
    // that is refused.
    let one_too_many: Vec<Value> = source_versions
        .keys()
        .cycle()
        .take(MAX_VERSIONS_PER_PRODUCT + 1)
        .map(|id| json!({"Id": id}))
        .collect();

    // Rows differ along WHICH part of the request names nothing: the account in the
    // source ARN (a product another account holds is readable only once it has been
    // shared into a portfolio), its region, its product, the service it names at all,
    // the target product, and the source versions.
    let cases: [(&str, Refusal, &str); 9] = [
        (
            "a product another account holds",
            Refusal::Source(format!(
                "arn:aws:catalog:{DEFAULT_REGION}:111122223333:product/{source_id}"
            )),
            "ResourceNotFoundException",
        ),
        (
            "the same product id in another region",
            Refusal::Source(format!(
                "arn:aws:catalog:eu-west-1:{ACCOUNT_ID}:product/{source_id}"
            )),
            "ResourceNotFoundException",
        ),
        (
            "a product that was never created",
            Refusal::Source(format!(
                "arn:aws:catalog:{DEFAULT_REGION}:{ACCOUNT_ID}:product/prod-000000000000"
            )),
            "ResourceNotFoundException",
        ),
        (
            "an ARN of another service",
            Refusal::Source(format!("arn:aws:s3:::copy-product-{source_id}")),
            "ResourceNotFoundException",
        ),
        (
            "a target product that does not exist",
            Refusal::Target("prod-000000000000".to_string()),
            "ResourceNotFoundException",
        ),
        (
            "the source product as its own target",
            Refusal::Target(source_id.clone()),
            "InvalidParametersException",
        ),
        (
            "more versions than a product can carry",
            Refusal::Versions(Value::Array(one_too_many)),
            "InvalidParametersException",
        ),
        (
            "a version the source does not carry",
            Refusal::Versions(json!([{"Id": unknown_version}])),
            "InvalidParametersException",
        ),
        (
            "an identifier that names no version",
            Refusal::Versions(json!([{}])),
            "InvalidParametersException",
        ),
    ];

    for (index, (case, refusal, expected)) in cases.iter().enumerate() {
        // The name the copy WOULD have given the product it created, so a refusal that
        // stored something can be caught by reading that name back.
        let target_name = format!("refused-{index}");
        let mut request = json!({
            "SourceProductArn": source_arn,
            "TargetProductName": target_name,
            "IdempotencyToken": format!("tok-refused-{index}"),
        });
        match refusal {
            Refusal::Source(arn) => request["SourceProductArn"] = json!(arn),
            Refusal::Target(id) => request["TargetProductId"] = json!(id),
            Refusal::Versions(identifiers) => {
                request["SourceProvisioningArtifactIdentifiers"] = identifiers.clone();
            }
        }

        let (status, refused) = call(&router, "CopyProduct", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            &error_code(&refused),
            expected,
            "case {case:?}: unexpected error in {refused}"
        );

        // The refusal stored nothing: the product the copy would have minted is unknown.
        let (status, described) = call(
            &router,
            "DescribeProductAsAdmin",
            &json!({"Name": target_name}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused copy must store nothing, got {status} with {described}"
        );
        assert_eq!(
            error_code(&described),
            "ResourceNotFoundException",
            "case {case:?}: a refused copy must store nothing, got {described}"
        );
    }
}

/// The end-user view of a product resolves it only through a LAUNCH PATH — a portfolio
/// the product is associated with — so the product a copy mints, like any product no
/// portfolio carries, is refused by `DescribeProduct` until a portfolio carries it, while
/// the admin view reads it all along. The refusal does not tell "no access" from "no such
/// product": both are the modelled `ResourceNotFoundException`.
#[tokio::test]
async fn the_end_user_view_reaches_a_product_only_through_a_portfolio() {
    let router = emulator();
    let source_id = create_product(&router, "access-source", "tok-access-source").await;
    let source_arn = stored_arn(&router, &source_id).await;
    let portfolio_id = create_portfolio(
        &router,
        "access-portfolio",
        "acme",
        &[],
        "tok-access-portfolio",
    )
    .await;

    let (status, copied) = call(
        &router,
        "CopyProduct",
        &json!({
            "SourceProductArn": source_arn,
            "TargetProductName": "access-copy",
            "IdempotencyToken": "tok-access-copy",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"copy\": expected 200, got {status} with {copied}"
    );
    let token = string_at("copy", &copied, "/CopyProductToken");
    let (_, target_id) = copy_status(&router, "copy", &token).await;

    // Rows differ along which product is asked for and how: neither the source nor the
    // copy sits in a portfolio yet, and a name resolves no more than an id does.
    for (case, body) in [
        ("the copy by id", json!({"Id": target_id.clone()})),
        ("the copy by name", json!({"Name": "access-copy"})),
        ("the source by id", json!({"Id": source_id.clone()})),
        ("the source by name", json!({"Name": "access-source"})),
    ] {
        let (status, refused) = call(&router, "DescribeProduct", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a product no portfolio carries is out of the end-user view, got \
             {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error in {refused}"
        );
    }

    // The product is nonetheless STORED, which the admin view reports all along.
    assert_eq!(
        stored_name(&router, &target_id).await,
        "access-copy",
        "case \"admin view\": the copy is stored even while the end-user view refuses it"
    );

    // One portfolio carries the copy: that is its launch path, and the end-user view
    // reaches it through that path — and through it alone, so the source stays out of
    // view.
    associate_product_with_portfolio(&router, &target_id, &portfolio_id).await;
    for (case, body) in [
        ("the carried copy by id", json!({"Id": target_id.clone()})),
        ("the carried copy by name", json!({"Name": "access-copy"})),
    ] {
        let (status, described) = call(&router, "DescribeProduct", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {described}"
        );
        assert_eq!(
            string_at(case, &described, "/ProductViewSummary/ProductId"),
            target_id,
            "case {case:?}: it is the copied product, got {described}"
        );
        assert_eq!(
            string_at(case, &described, "/LaunchPaths/0/Id"),
            portfolio_id,
            "case {case:?}: the launch path is the carrying portfolio, got {described}"
        );
    }
    let (status, refused) = call(&router, "DescribeProduct", &json!({"Id": source_id})).await;
    assert_eq!(
        status, 400,
        "case \"the source after the copy was carried\": only the carried product came into \
         view, got {status} with {refused}"
    );
}

/// `IdempotencyToken` is what a repeat is recognised by: a request differing only by that
/// token starts a second copy, and one carrying a token already seen answers the token of
/// the copy it already started, against the same target product. A token
/// `DescribeCopyProductStatus` is asked about that no copy ever answered resolves nothing.
#[tokio::test]
async fn copy_product_replays_the_copy_an_idempotency_token_already_started() {
    let router = emulator();
    let source_id = create_product(&router, "replay-source", "tok-replay-source").await;
    let source_arn = stored_arn(&router, &source_id).await;

    let request = |token: &str| {
        json!({
            "SourceProductArn": source_arn,
            "IdempotencyToken": token,
        })
    };

    let case = "first copy";
    let (status, first) = call(&router, "CopyProduct", &request("tok-replay")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {first}"
    );
    let first_token = string_at(case, &first, "/CopyProductToken");
    let (_, first_target) = copy_status(&router, case, &first_token).await;

    let case = "the same idempotency token";
    let (status, replayed) = call(&router, "CopyProduct", &request("tok-replay")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {replayed}"
    );
    assert_eq!(
        string_at(case, &replayed, "/CopyProductToken"),
        first_token,
        "case {case:?}: a repeated request answers the copy it already started"
    );
    let (_, replayed_target) = copy_status(&router, case, &first_token).await;
    assert_eq!(
        replayed_target, first_target,
        "case {case:?}: the replay must not mint a second product"
    );

    let case = "another idempotency token";
    let (status, second) = call(&router, "CopyProduct", &request("tok-replay-other")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {second}"
    );
    let second_token = string_at(case, &second, "/CopyProductToken");
    assert_ne!(
        second_token, first_token,
        "case {case:?}: a request differing only by its token starts a second copy"
    );
    let (_, second_target) = copy_status(&router, case, &second_token).await;
    assert_ne!(
        second_target, first_target,
        "case {case:?}: the second copy mints its own product"
    );

    let case = "a token no copy answered";
    let (status, unknown) = call(
        &router,
        "DescribeCopyProductStatus",
        &json!({"CopyProductToken": "copy-product-000000000000"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {unknown}"
    );
    assert_eq!(
        error_code(&unknown),
        "ResourceNotFoundException",
        "case {case:?}: unexpected error in {unknown}"
    );
}
