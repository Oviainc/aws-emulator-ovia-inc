// SPDX-License-Identifier: CC0-1.0

//! `ProvisionProduct`, `DescribeProvisionedProduct` and `SearchProvisionedProducts` over
//! the wire, driven as awsJson1_1 requests through the same `MultiServiceRouter` the
//! production emulator mounts.
//!
//! The rules under test:
//!
//! * provisioning resolves the product's launch paths — its stored portfolio
//!   associations — before it provisions anything, so a product that resolves no path
//!   is refused with the modelled 400 `ResourceNotFoundException` and the refusal
//!   persists nothing;
//! * a resolved request persists a provisioned product and the record of the
//!   provisioning, and every later read of either reports those stored values;
//! * a search reports the stored provisioned products the caller's `SearchQuery`
//!   terms select, over the fields the model documents as searchable.
//!
//! Every assertion is about STORED state: a response is checked against what an earlier
//! create, associate or provision persisted, or against a value derived from it, never
//! against a value this file hands the service and immediately reads back out of the
//! same response.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::harness::{
    associate_product_with_portfolio, call, create_portfolio, create_product,
    create_product_of_type, emulator, error_code, first_provisioning_artifact_id, is_catalog_id,
    tag_list, tag_map, ACCOUNT_ID, DEFAULT_REGION,
};

/// The error message of a failure body, whichever case the wire member carries.
fn message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or(body["Message"].as_str())
        .unwrap_or_else(|| panic!("no message in {body}"))
        .to_string()
}

/// How a request addresses a resource that takes either an id or a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddressBy {
    Id,
    Name,
}

/// The names of the provisioned products one search reported, with the
/// `TotalResultsCount` it answered.
async fn search(
    router: &smithy_server_runtime::MultiServiceRouter,
    request: &Value,
) -> (Vec<String>, i64) {
    let (status, found) = call(router, "SearchProvisionedProducts", request).await;
    assert_eq!(
        status, 200,
        "SearchProvisionedProducts {request}: expected 200, got {status} with {found}"
    );
    let names = found["ProvisionedProducts"]
        .as_array()
        .unwrap_or_else(|| panic!("no ProvisionedProducts list in {found}"))
        .iter()
        .map(|attribute| {
            attribute["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("a provisioned product has no Name: {attribute}"))
                .to_string()
        })
        .collect();
    let total = found["TotalResultsCount"]
        .as_i64()
        .unwrap_or_else(|| panic!("no TotalResultsCount in {found}"));
    (names, total)
}

/// One `SearchQuery` search for the provisioned product called `name`.
fn by_name(name: &str) -> Value {
    json!({"Filters": {"SearchQuery": [format!("name:{name}")]}})
}

/// One row of the no-launch-path table: how the product under test is stored, and how
/// the provisioning request addresses it.
struct NoPathCase {
    case: &'static str,
    /// `Some(..)`: a product this case creates, addressed by id or by name, that no
    /// portfolio carries. `None`: the identifier below is not a stored product at all.
    stored: Option<AddressBy>,
    /// For a stored product, whether its one association is created and then removed
    /// before the provisioning request — a product whose path was taken away has no
    /// more of a path than one that never had one.
    disassociate: bool,
    /// The identifier a case with no stored product names.
    unknown: &'static str,
}

/// A product that resolves no launch path cannot be provisioned, the refusal names the
/// product the request addressed, and nothing is provisioned by it. Once a portfolio
/// carries the product the byte-identical request succeeds.
#[tokio::test]
async fn provisioning_a_product_with_no_launch_path_is_refused_and_has_no_effect() {
    // Rows differ along how the product is addressed (id, name), how it came to have no
    // path (never associated, association removed) and, for an identifier nothing
    // stored carries, along the SHAPE of that identifier.
    let cases = [
        NoPathCase {
            case: "stored product in no portfolio, addressed by id",
            stored: Some(AddressBy::Id),
            disassociate: false,
            unknown: "",
        },
        NoPathCase {
            case: "stored product in no portfolio, addressed by name",
            stored: Some(AddressBy::Name),
            disassociate: false,
            unknown: "",
        },
        NoPathCase {
            case: "the product's only association was removed",
            stored: Some(AddressBy::Id),
            disassociate: true,
            unknown: "",
        },
        NoPathCase {
            case: "a well-formed product id nothing stored carries",
            stored: None,
            disassociate: false,
            unknown: "prod-000000000000",
        },
        NoPathCase {
            case: "an id of another resource kind",
            stored: None,
            disassociate: false,
            unknown: "port-zzzzzzzzzzzz",
        },
        NoPathCase {
            case: "not an id of any kind",
            stored: None,
            disassociate: false,
            unknown: "lorem",
        },
    ];

    for row in cases {
        let case = row.case;
        let router = emulator();
        let portfolio_id =
            create_portfolio(&router, "alpha-portfolio", "acme", &[], "portfolio").await;
        let product_name = "provisioned-source-product";
        let product_id = create_product(&router, product_name, "product-token").await;
        let artifact_id = first_provisioning_artifact_id(&router, &product_id).await;
        // A second product that IS in the portfolio, so a path reported for the product
        // under test cannot come from another product's association.
        let other_product_id = create_product(&router, "other-product", "other-token").await;
        associate_product_with_portfolio(&router, &other_product_id, &portfolio_id).await;

        if row.disassociate {
            associate_product_with_portfolio(&router, &product_id, &portfolio_id).await;
            let (status, body) = call(
                &router,
                "DisassociateProductFromPortfolio",
                &json!({"ProductId": product_id, "PortfolioId": portfolio_id}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the disassociation to be served, got {status} with {body}"
            );
        }

        let provisioned_name = "pp-under-test";
        let mut request = json!({
            "ProvisionedProductName": provisioned_name,
            "ProvisioningArtifactId": artifact_id,
            "ProvisionToken": "provision-token",
            "ProvisioningParameters": [{"Key": "Env", "Value": "prod"}],
        });
        let addressed = match row.stored {
            Some(AddressBy::Id) => {
                request["ProductId"] = Value::String(product_id.clone());
                product_id.clone()
            }
            Some(AddressBy::Name) => {
                request["ProductName"] = Value::String(product_name.to_string());
                product_name.to_string()
            }
            None => {
                request["ProductId"] = Value::String(row.unknown.to_string());
                row.unknown.to_string()
            }
        };

        let (status, refused) = call(&router, "ProvisionProduct", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a product with no launch path cannot be provisioned, got {status} \
             with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error in {refused}"
        );
        assert_eq!(
            message(&refused),
            format!("No launch paths found for resource: {addressed}"),
            "case {case:?}: unexpected message in {refused}"
        );

        // The refusal had no effect: no provisioned product by that name exists, for
        // the search and for the describe alike.
        let (names, total) = search(&router, &by_name(provisioned_name)).await;
        assert!(
            names.is_empty() && total == 0,
            "case {case:?}: the refused call provisioned something: {names:?} (total {total})"
        );
        let (status, described) = call(
            &router,
            "DescribeProvisionedProduct",
            &json!({"Name": provisioned_name}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: nothing was provisioned, so the describe must be a not-found, got \
             {status} with {described}"
        );
        assert_eq!(
            error_code(&described),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error in {described}"
        );

        // The positive control: once a portfolio carries the product, the SAME request
        // resolves a path and is served.
        if row.stored.is_some() {
            associate_product_with_portfolio(&router, &product_id, &portfolio_id).await;
            let (status, provisioned) = call(&router, "ProvisionProduct", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the identical request must succeed once the product is in a \
                 portfolio, got {status} with {provisioned}"
            );
            assert_eq!(
                provisioned["RecordDetail"]["PathId"].as_str(),
                Some(portfolio_id.as_str()),
                "case {case:?}: the record must name the path the product resolved: {provisioned}"
            );
            let (names, total) = search(&router, &by_name(provisioned_name)).await;
            assert_eq!(
                (names.as_slice(), total),
                (&[provisioned_name.to_string()][..], 1),
                "case {case:?}: the served call must have provisioned exactly one product"
            );
        }
    }
}

/// How a row addresses the launch path to provision through.
#[derive(Debug, Clone, Copy)]
enum PathArg {
    /// The product has one path, so the request names none.
    Omitted,
    ById,
    ByName,
}

/// One row of the provisioning table.
struct ProvisionCase {
    case: &'static str,
    /// A `ProductType` value the product is created with.
    product_type: &'static str,
    /// The `ProvisionedProductType` provisioning that product must report.
    provisioned_type: &'static str,
    tags: &'static [(&'static str, &'static str)],
    path: PathArg,
    product: AddressBy,
}

/// Provisioning a product through its launch path persists the provisioned product and
/// the record of the request, and every later read reports those stored values.
#[tokio::test]
async fn provisioning_persists_the_provisioned_product_and_its_record() {
    // Rows differ along the product type (and so the provisioned-product type it maps
    // to), the number of tags, how the path is addressed and how the product is
    // addressed. The product types are the model's own
    // (`smithy_query query="shape" shape="ProductType"`), including MARKETPLACE, which
    // the issue cited none of.
    let cases = [
        ProvisionCase {
            case: "CloudFormation product, no tag, default path, by product id",
            product_type: "CLOUD_FORMATION_TEMPLATE",
            provisioned_type: "CFN_STACK",
            tags: &[],
            path: PathArg::Omitted,
            product: AddressBy::Id,
        },
        ProvisionCase {
            case: "CloudFormation product, two tags, path by id, by product id",
            product_type: "CLOUD_FORMATION_TEMPLATE",
            provisioned_type: "CFN_STACK",
            tags: &[("env", "prod"), ("team", "platform")],
            path: PathArg::ById,
            product: AddressBy::Id,
        },
        ProvisionCase {
            case: "Terraform product, one tag, path by name, by product name",
            product_type: "TERRAFORM_OPEN_SOURCE",
            provisioned_type: "TERRAFORM_OPEN_SOURCE",
            tags: &[("env", "dev")],
            path: PathArg::ByName,
            product: AddressBy::Name,
        },
        ProvisionCase {
            case: "external product, no tag, path by id, by product name",
            product_type: "EXTERNAL",
            provisioned_type: "EXTERNAL",
            tags: &[],
            path: PathArg::ById,
            product: AddressBy::Name,
        },
        ProvisionCase {
            case: "Marketplace product, one tag, default path",
            product_type: "MARKETPLACE",
            provisioned_type: "CFN_STACK",
            tags: &[("tier", "gold")],
            path: PathArg::Omitted,
            product: AddressBy::Id,
        },
    ];

    for row in cases {
        let case = row.case;
        let router = emulator();
        let portfolio_name = "alpha-portfolio";
        let portfolio_id = create_portfolio(&router, portfolio_name, "acme", &[], "portfolio").await;
        let product_name = "provisioned-source-product";
        let product_id =
            create_product_of_type(&router, product_name, row.product_type, "product-token").await;
        let artifact_id = first_provisioning_artifact_id(&router, &product_id).await;
        associate_product_with_portfolio(&router, &product_id, &portfolio_id).await;

        let provisioned_name = "pp-under-test";
        let token = "provision-token";
        let mut request = json!({
            "ProvisionedProductName": provisioned_name,
            "ProvisioningArtifactId": artifact_id,
            "ProvisionToken": token,
        });
        match row.product {
            AddressBy::Id => request["ProductId"] = Value::String(product_id.clone()),
            AddressBy::Name => request["ProductName"] = Value::String(product_name.to_string()),
        }
        match row.path {
            PathArg::Omitted => {}
            PathArg::ById => request["PathId"] = Value::String(portfolio_id.clone()),
            PathArg::ByName => request["PathName"] = Value::String(portfolio_name.to_string()),
        }
        if !row.tags.is_empty() {
            request["Tags"] = tag_list(row.tags);
        }

        let (status, provisioned) = call(&router, "ProvisionProduct", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {provisioned}"
        );
        let record = &provisioned["RecordDetail"];
        let record_id = record["RecordId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no RecordId in {provisioned}"))
            .to_string();
        assert!(
            is_catalog_id("rec", &record_id),
            "case {case:?}: expected a minted record id, got {record_id:?}"
        );
        let provisioned_product_id = record["ProvisionedProductId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ProvisionedProductId in {provisioned}"))
            .to_string();
        assert!(
            is_catalog_id("pp", &provisioned_product_id),
            "case {case:?}: expected a minted provisioned-product id, got \
             {provisioned_product_id:?}"
        );
        let expected_record = json!({
            "ProvisionedProductName": provisioned_name,
            // "CREATED - the request was created but the operation has not started":
            // the provisioning is still in flight when the request is answered.
            "Status": "CREATED",
            "RecordType": "PROVISION_PRODUCT",
            "ProvisionedProductType": row.provisioned_type,
            "ProductId": product_id,
            "ProvisioningArtifactId": artifact_id,
            "PathId": portfolio_id,
        });
        for (member, want) in expected_record.as_object().into_iter().flatten() {
            assert_eq!(
                record[member].as_str(),
                want.as_str(),
                "case {case:?}: RecordDetail.{member} expected {want}, got {}",
                record[member]
            );
        }
        assert_eq!(
            record["RecordErrors"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a served provisioning carries no record error: {record}"
        );
        let record_tags: BTreeMap<String, String> = tag_map(case, &record["RecordTags"]);
        let expected_tags: BTreeMap<String, String> = row
            .tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            record_tags, expected_tags,
            "case {case:?}: expected the requested tags {expected_tags:?} on the record, got \
             {record_tags:?}"
        );

        // Read the provisioned product back, by id and by name: both address the one
        // stored record, so both must report the same stored values.
        for (reader, key) in [
            ("by id", json!({"Id": provisioned_product_id})),
            ("by name", json!({"Name": provisioned_name})),
        ] {
            let (status, described) = call(&router, "DescribeProvisionedProduct", &key).await;
            assert_eq!(
                status, 200,
                "case {case:?} {reader}: expected 200, got {status} with {described}"
            );
            let detail = &described["ProvisionedProductDetail"];
            let expected_detail = json!({
                "Name": provisioned_name,
                "Id": provisioned_product_id,
                "Arn": format!(
                    "arn:aws:servicecatalog:{DEFAULT_REGION}:{ACCOUNT_ID}:stack/\
                     {provisioned_name}/{provisioned_product_id}"
                ),
                "Type": row.provisioned_type,
                // The provisioning the record above is still CREATED for leaves the
                // provisioned product in the transitive status.
                "Status": "UNDER_CHANGE",
                "IdempotencyToken": token,
                "LastRecordId": record_id,
                "LastProvisioningRecordId": record_id,
                "ProductId": product_id,
                "ProvisioningArtifactId": artifact_id,
            });
            for (member, want) in expected_detail.as_object().into_iter().flatten() {
                assert_eq!(
                    detail[member].as_str(),
                    want.as_str(),
                    "case {case:?} {reader}: ProvisionedProductDetail.{member} expected {want}, \
                     got {}",
                    detail[member]
                );
            }
            assert!(
                detail.get("LastSuccessfulProvisioningRecordId").is_none(),
                "case {case:?} {reader}: no provisioning has succeeded yet, so the member must \
                 be absent: {detail}"
            );
        }

        // The search reports the same stored provisioned product, with the product and
        // version NAMES read from the product it was provisioned from.
        let (status, found) =
            call(&router, "SearchProvisionedProducts", &by_name(provisioned_name)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the search to be served, got {status} with {found}"
        );
        let attributes = found["ProvisionedProducts"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no ProvisionedProducts in {found}"));
        assert_eq!(
            attributes.len(),
            1,
            "case {case:?}: expected exactly the provisioned product, got {found}"
        );
        let attribute = &attributes[0];
        let expected_attribute = json!({
            "Id": provisioned_product_id,
            "Name": provisioned_name,
            "Type": row.provisioned_type,
            "Status": "UNDER_CHANGE",
            "ProductId": product_id,
            "ProductName": product_name,
            "ProvisioningArtifactId": artifact_id,
            // The one version `create_product_of_type` gave the product.
            "ProvisioningArtifactName": "v1",
            "LastRecordId": record_id,
        });
        for (member, want) in expected_attribute.as_object().into_iter().flatten() {
            assert_eq!(
                attribute[member].as_str(),
                want.as_str(),
                "case {case:?}: ProvisionedProductAttribute.{member} expected {want}, got {}",
                attribute[member]
            );
        }
        assert_eq!(
            tag_map(case, &attribute["Tags"]),
            expected_tags,
            "case {case:?}: the search must report the stored tags: {attribute}"
        );

        // `ProvisionToken` is an idempotency token: the same request repeated replays
        // the record it minted and provisions nothing further.
        let (status, replayed) = call(&router, "ProvisionProduct", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeated request must be served, got {status} with {replayed}"
        );
        assert_eq!(
            replayed["RecordDetail"]["RecordId"].as_str(),
            Some(record_id.as_str()),
            "case {case:?}: the repeat must replay the first record: {replayed}"
        );
        let (names, total) = search(&router, &by_name(provisioned_name)).await;
        assert_eq!(
            (names.len(), total),
            (1, 1),
            "case {case:?}: the repeat must not provision a second product: {names:?}"
        );

        // The name is unique for the account: the same name with a fresh token is a
        // duplicate, and leaves the stored provisioned product alone.
        let mut duplicate = request.clone();
        duplicate["ProvisionToken"] = Value::String("second-token".to_string());
        let (status, refused) = call(&router, "ProvisionProduct", &duplicate).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a repeated ProvisionedProductName is a duplicate, got {status} with \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "DuplicateResourceException",
            "case {case:?}: unexpected error in {refused}"
        );
        assert_eq!(
            message(&refused),
            format!("A provisioned product with the name {provisioned_name} already exists."),
            "case {case:?}: unexpected message in {refused}"
        );
        let (names, total) = search(&router, &by_name(provisioned_name)).await;
        assert_eq!(
            (names.len(), total),
            (1, 1),
            "case {case:?}: the refused duplicate must have changed nothing: {names:?}"
        );
    }
}

/// A request that addresses a resource by neither or both of the two members that
/// address it is a parameter error; one that addresses a resource nothing stored
/// carries is a not-found. Neither provisions anything.
#[tokio::test]
async fn ambiguous_or_unresolvable_references_are_refused() {
    let router = emulator();
    let portfolio_name = "alpha-portfolio";
    let portfolio_id = create_portfolio(&router, portfolio_name, "acme", &[], "portfolio").await;
    let other_portfolio_id =
        create_portfolio(&router, "beta-portfolio", "globex", &[], "portfolio-2").await;
    let product_name = "provisioned-source-product";
    let product_id = create_product(&router, product_name, "product-token").await;
    let artifact_id = first_provisioning_artifact_id(&router, &product_id).await;
    associate_product_with_portfolio(&router, &product_id, &portfolio_id).await;
    // Another product, in another portfolio, with its own version: its path and its
    // version belong to it and must not resolve for the product under test.
    let other_product_id = create_product(&router, "other-product", "other-token").await;
    let other_artifact_id = first_provisioning_artifact_id(&router, &other_product_id).await;
    associate_product_with_portfolio(&router, &other_product_id, &other_portfolio_id).await;

    // Rows differ along WHICH pair of members is ambiguous or unresolvable, and along
    // whether the refusal is about the request (parameter) or about the store
    // (not-found).
    let cases: [(&str, Value, &str); 9] = [
        (
            "both ProductId and ProductName",
            json!({"ProductId": product_id, "ProductName": product_name}),
            "InvalidParametersException",
        ),
        (
            "neither ProductId nor ProductName",
            json!({}),
            "InvalidParametersException",
        ),
        (
            "both PathId and PathName",
            json!({"ProductId": product_id, "PathId": portfolio_id, "PathName": portfolio_name}),
            "InvalidParametersException",
        ),
        (
            "a PathId that is a portfolio the product is not in",
            json!({"ProductId": product_id, "PathId": other_portfolio_id}),
            "ResourceNotFoundException",
        ),
        (
            "a PathName that names no path of the product",
            json!({"ProductId": product_id, "PathName": "beta-portfolio"}),
            "ResourceNotFoundException",
        ),
        (
            "both ProvisioningArtifactId and ProvisioningArtifactName",
            json!({"ProductId": product_id, "ProvisioningArtifactName": "v1"}),
            "InvalidParametersException",
        ),
        (
            "an unknown ProvisioningArtifactId",
            json!({"ProductId": product_id, "ProvisioningArtifactId": "pa-000000000000"}),
            "ResourceNotFoundException",
        ),
        (
            "a ProvisioningArtifactId of another product",
            json!({"ProductId": product_id, "ProvisioningArtifactId": other_artifact_id}),
            "ResourceNotFoundException",
        ),
        (
            "a ProvisioningArtifactName no version of the product carries",
            json!({"ProductId": product_id, "ProvisioningArtifactName": "v9"}),
            "ResourceNotFoundException",
        ),
    ];

    for (index, (case, members, expected)) in cases.into_iter().enumerate() {
        let provisioned_name = format!("pp-refused-{index}");
        let mut request = json!({
            "ProvisionedProductName": provisioned_name,
            "ProvisionToken": format!("token-{index}"),
        });
        // Every row that is not about the version itself names the stored version, so
        // the refusal under test is the only thing wrong with the request.
        if members.get("ProvisioningArtifactId").is_none()
            && members.get("ProvisioningArtifactName").is_none()
        {
            request["ProvisioningArtifactId"] = Value::String(artifact_id.clone());
        }
        if case == "both ProvisioningArtifactId and ProvisioningArtifactName" {
            request["ProvisioningArtifactId"] = Value::String(artifact_id.clone());
        }
        for (member, value) in members.as_object().into_iter().flatten() {
            request[member] = value.clone();
        }

        let (status, refused) = call(&router, "ProvisionProduct", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            expected,
            "case {case:?}: unexpected error in {refused}"
        );

        let (names, total) = search(&router, &by_name(&provisioned_name)).await;
        assert!(
            names.is_empty() && total == 0,
            "case {case:?}: the refused call provisioned something: {names:?}"
        );
    }

    // A request with no version at all is a parameter error of its own.
    let (status, refused) = call(
        &router,
        "ProvisionProduct",
        &json!({
            "ProductId": product_id,
            "ProvisionedProductName": "pp-no-version",
            "ProvisionToken": "token-no-version",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a request naming no product version must be refused, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidParametersException",
        "unexpected error in {refused}"
    );

    // Nothing above provisioned anything at all.
    let (names, total) = search(&router, &json!({})).await;
    assert!(
        names.is_empty() && total == 0,
        "the refusals must have provisioned nothing, got {names:?} (total {total})"
    );
}

/// A path is required exactly when the product has more than one, and naming either
/// half of a path — its id or its portfolio's name — selects it.
#[tokio::test]
async fn a_product_with_several_paths_needs_the_path_named() {
    let router = emulator();
    let first_name = "alpha-portfolio";
    let second_name = "beta-portfolio";
    let first_id = create_portfolio(&router, first_name, "acme", &[], "portfolio").await;
    let second_id = create_portfolio(&router, second_name, "globex", &[], "portfolio-2").await;
    let product_id = create_product(&router, "provisioned-source-product", "product-token").await;
    let artifact_id = first_provisioning_artifact_id(&router, &product_id).await;
    associate_product_with_portfolio(&router, &product_id, &first_id).await;
    associate_product_with_portfolio(&router, &product_id, &second_id).await;

    // Rows: no path named (ambiguous), each path named by id, and each path named by
    // its portfolio's name.
    let cases: [(&str, Value, Option<&String>); 5] = [
        ("no path named, two paths exist", json!({}), None),
        (
            "the first path by id",
            json!({"PathId": first_id}),
            Some(&first_id),
        ),
        (
            "the second path by id",
            json!({"PathId": second_id}),
            Some(&second_id),
        ),
        (
            "the first path by name",
            json!({"PathName": first_name}),
            Some(&first_id),
        ),
        (
            "the second path by name",
            json!({"PathName": second_name}),
            Some(&second_id),
        ),
    ];

    for (index, (case, members, expected_path)) in cases.into_iter().enumerate() {
        let provisioned_name = format!("pp-path-{index}");
        let mut request = json!({
            "ProductId": product_id,
            "ProvisioningArtifactId": artifact_id,
            "ProvisionedProductName": provisioned_name,
            "ProvisionToken": format!("token-{index}"),
        });
        for (member, value) in members.as_object().into_iter().flatten() {
            request[member] = value.clone();
        }

        let (status, body) = call(&router, "ProvisionProduct", &request).await;
        match expected_path {
            None => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: an ambiguous path must be refused, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidParametersException",
                    "case {case:?}: unexpected error in {body}"
                );
                let (names, total) = search(&router, &by_name(&provisioned_name)).await;
                assert!(
                    names.is_empty() && total == 0,
                    "case {case:?}: the refused call provisioned something: {names:?}"
                );
            }
            Some(path_id) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} with {body}"
                );
                assert_eq!(
                    body["RecordDetail"]["PathId"].as_str(),
                    Some(path_id.as_str()),
                    "case {case:?}: the record must name the path the request selected: {body}"
                );
            }
        }
    }
}

/// The provisioning the request starts is not instantaneous: the record is answered in
/// flight and the clock owes its completion, which every later read derives.
#[tokio::test]
async fn the_clock_completes_the_provisioning() {
    let router = emulator();
    let portfolio_id = create_portfolio(&router, "alpha-portfolio", "acme", &[], "portfolio").await;
    let product_id = create_product(&router, "provisioned-source-product", "product-token").await;
    let artifact_id = first_provisioning_artifact_id(&router, &product_id).await;
    associate_product_with_portfolio(&router, &product_id, &portfolio_id).await;

    let provisioned_name = "pp-under-change";
    let (status, provisioned) = call(
        &router,
        "ProvisionProduct",
        &json!({
            "ProductId": product_id,
            "ProvisioningArtifactId": artifact_id,
            "ProvisionedProductName": provisioned_name,
            "ProvisionToken": "provision-token",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the provisioning to be served, got {status} with {provisioned}"
    );
    let record_id = provisioned["RecordDetail"]["RecordId"]
        .as_str()
        .unwrap_or_else(|| panic!("no RecordId in {provisioned}"))
        .to_string();

    let describe = json!({"Name": provisioned_name});
    let (status, in_flight) = call(&router, "DescribeProvisionedProduct", &describe).await;
    assert_eq!(
        status, 200,
        "expected the describe to be served, got {status} with {in_flight}"
    );
    assert_eq!(
        in_flight["ProvisionedProductDetail"]["Status"].as_str(),
        Some("UNDER_CHANGE"),
        "a caller acting straight after the request still sees the transitive status: {in_flight}"
    );

    // The transition is owed by the clock, not by this read: wait the span out and the
    // SAME read reports the stable status and the record that succeeded.
    std::thread::sleep(std::time::Duration::from_millis(3_500));

    let (status, settled) = call(&router, "DescribeProvisionedProduct", &describe).await;
    assert_eq!(
        status, 200,
        "expected the second describe to be served, got {status} with {settled}"
    );
    let detail = &settled["ProvisionedProductDetail"];
    assert_eq!(
        detail["Status"].as_str(),
        Some("AVAILABLE"),
        "the clock owes the completion, so the provisioned product must be stable: {settled}"
    );
    assert_eq!(
        detail["LastSuccessfulProvisioningRecordId"].as_str(),
        Some(record_id.as_str()),
        "the completed provisioning is the last successful one: {settled}"
    );

    // And the search reports exactly the same stored status.
    let (status, found) = call(&router, "SearchProvisionedProducts", &by_name(provisioned_name)).await;
    assert_eq!(
        status, 200,
        "expected the search to be served, got {status} with {found}"
    );
    assert_eq!(
        found["ProvisionedProducts"][0]["Status"].as_str(),
        Some("AVAILABLE"),
        "two readers of one provisioned product cannot disagree: {found}"
    );
}

/// One provisioned product a search case seeds.
struct Seed {
    provisioned_name: &'static str,
    product: &'static str,
    product_type: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

/// The set a search case searches over: three provisioned products across two products,
/// of two provisioned-product types, with different tags.
const SEEDS: [Seed; 3] = [
    Seed {
        provisioned_name: "alpha-stack",
        product: "cfn-product",
        product_type: "CLOUD_FORMATION_TEMPLATE",
        tags: &[("env", "prod")],
    },
    Seed {
        provisioned_name: "beta-stack",
        product: "cfn-product",
        product_type: "CLOUD_FORMATION_TEMPLATE",
        tags: &[("env", "dev")],
    },
    Seed {
        provisioned_name: "gamma-workspace",
        product: "terraform-product",
        product_type: "TERRAFORM_OPEN_SOURCE",
        tags: &[],
    },
];

/// Seed [`SEEDS`] on a fresh emulator and answer the product id of each seeded product
/// by product name.
async fn seed_provisioned_products(
    router: &smithy_server_runtime::MultiServiceRouter,
) -> BTreeMap<String, String> {
    let portfolio_id = create_portfolio(router, "alpha-portfolio", "acme", &[], "portfolio").await;
    let mut product_ids: BTreeMap<String, String> = BTreeMap::new();
    for (index, seed) in SEEDS.iter().enumerate() {
        if !product_ids.contains_key(seed.product) {
            let product_id = create_product_of_type(
                router,
                seed.product,
                seed.product_type,
                &format!("product-{index}"),
            )
            .await;
            associate_product_with_portfolio(router, &product_id, &portfolio_id).await;
            product_ids.insert(seed.product.to_string(), product_id);
        }
        let product_id = &product_ids[seed.product];
        let artifact_id = first_provisioning_artifact_id(router, product_id).await;
        let mut request = json!({
            "ProductId": product_id,
            "ProvisioningArtifactId": artifact_id,
            "ProvisionedProductName": seed.provisioned_name,
            "ProvisionToken": format!("provision-{index}"),
        });
        if !seed.tags.is_empty() {
            request["Tags"] = tag_list(seed.tags);
        }
        let (status, provisioned) = call(router, "ProvisionProduct", &request).await;
        assert_eq!(
            status, 200,
            "seeding {}: expected 200, got {status} with {provisioned}",
            seed.provisioned_name
        );
    }
    product_ids
}

/// A `SearchQuery` term selects the provisioned products whose searchable field holds
/// it, every term must match, and a search with no filter reports the whole set.
#[tokio::test]
async fn a_search_query_selects_over_the_searchable_fields() {
    let router = emulator();
    let product_ids = seed_provisioned_products(&router).await;
    let cfn_product = &product_ids["cfn-product"];

    // Rows differ along WHICH searchable field a term names (name, status, type,
    // productId, productName, tags, provisioningArtifactName), along a bare term
    // matched against every field, along a term that selects nothing, and along the
    // number of terms (one, two — which must narrow rather than widen).
    let cases: Vec<(&str, Vec<String>, Vec<&str>)> = vec![
        ("no filter at all", vec![], vec!["alpha-stack", "beta-stack", "gamma-workspace"]),
        ("an exact name", vec!["name:alpha-stack".to_string()], vec!["alpha-stack"]),
        (
            "a name fragment several share",
            vec!["name:-stack".to_string()],
            vec!["alpha-stack", "beta-stack"],
        ),
        (
            "the transitive status every seeded product is in",
            vec!["status:UNDER_CHANGE".to_string()],
            vec!["alpha-stack", "beta-stack", "gamma-workspace"],
        ),
        (
            "a provisioned-product type",
            vec!["type:CFN_STACK".to_string()],
            vec!["alpha-stack", "beta-stack"],
        ),
        (
            "the other provisioned-product type",
            vec!["type:TERRAFORM_OPEN_SOURCE".to_string()],
            vec!["gamma-workspace"],
        ),
        (
            "the product id",
            vec![format!("productId:{cfn_product}")],
            vec!["alpha-stack", "beta-stack"],
        ),
        (
            "the product name",
            vec!["productName:terraform-product".to_string()],
            vec!["gamma-workspace"],
        ),
        (
            "a tag key",
            vec!["tags:env".to_string()],
            vec!["alpha-stack", "beta-stack"],
        ),
        (
            "a tag value",
            vec!["tags:prod".to_string()],
            vec!["alpha-stack"],
        ),
        (
            "the version name every product carries",
            vec!["provisioningArtifactName:v1".to_string()],
            vec!["alpha-stack", "beta-stack", "gamma-workspace"],
        ),
        (
            "a bare term matched against every field",
            vec!["gamma".to_string()],
            vec!["gamma-workspace"],
        ),
        (
            "a field this service reports for nothing",
            vec!["physicalId:i-0123456789".to_string()],
            vec![],
        ),
        (
            "a term nothing holds",
            vec!["name:delta".to_string()],
            vec![],
        ),
        (
            "two terms narrow rather than widen",
            vec!["type:CFN_STACK".to_string(), "tags:dev".to_string()],
            vec!["beta-stack"],
        ),
        (
            "two terms that no one product satisfies",
            vec!["type:CFN_STACK".to_string(), "name:gamma".to_string()],
            vec![],
        ),
    ];

    for (case, terms, expected) in cases {
        let mut request = json!({});
        if !terms.is_empty() {
            request["Filters"] = json!({"SearchQuery": terms});
        }
        let (names, total) = search(&router, &request).await;
        let found: BTreeSet<String> = names.iter().cloned().collect();
        let want: BTreeSet<String> = expected.iter().map(|name| name.to_string()).collect();
        assert_eq!(
            found, want,
            "case {case:?}: expected {want:?}, got {found:?}"
        );
        assert_eq!(
            total,
            want.len() as i64,
            "case {case:?}: TotalResultsCount must count the whole result, got {total} for \
             {found:?}"
        );
    }
}

/// `SortBy` orders the result by the field it names, `SortOrder` reverses it, and an
/// unsupported sort field or access level is a parameter error.
#[tokio::test]
async fn sorting_paging_and_the_access_level_follow_the_model() {
    let router = emulator();
    seed_provisioned_products(&router).await;
    let ascending = vec![
        "alpha-stack".to_string(),
        "beta-stack".to_string(),
        "gamma-workspace".to_string(),
    ];
    let mut descending = ascending.clone();
    descending.reverse();

    // Sorting: by the one sort field whose order the seeded names fix, in both
    // directions, and with the order member omitted.
    for (case, request, expected) in [
        (
            "by name, ascending",
            json!({"SortBy": "name", "SortOrder": "ASCENDING"}),
            ascending.clone(),
        ),
        (
            "by name, descending",
            json!({"SortBy": "name", "SortOrder": "DESCENDING"}),
            descending.clone(),
        ),
        (
            "by name, no order named",
            json!({"SortBy": "name"}),
            ascending.clone(),
        ),
    ] {
        let (names, total) = search(&router, &request).await;
        assert_eq!(names, expected, "case {case:?}: expected {expected:?}");
        assert_eq!(
            total, 3,
            "case {case:?}: every seeded product is in the result"
        );
    }

    // The access level: the model's own keys with its only supported value select this
    // account's set; any other value is refused.
    for key in ["Account", "Role", "User"] {
        let (names, total) = search(
            &router,
            &json!({"AccessLevelFilter": {"Key": key, "Value": "self"}, "SortBy": "name"}),
        )
        .await;
        assert_eq!(
            (names.as_slice(), total),
            (ascending.as_slice(), 3),
            "case {key:?}: the account's own set must be reported"
        );
    }

    // Rows differ along WHICH member is unsupported: the sort field, the access-level
    // value, the filter key, a page token this service did not mint, and more search
    // terms than one search may carry.
    let too_many_terms: Vec<String> = (0..21).map(|i| format!("name:term-{i}")).collect();
    for (case, request) in [
        ("a sort field the model does not list", json!({"SortBy": "createdTime"})),
        (
            "an access level value other than self",
            json!({"AccessLevelFilter": {"Key": "Account", "Value": "everyone"}}),
        ),
        (
            "a filter key other than SearchQuery",
            json!({"Filters": {"Name": ["alpha-stack"]}}),
        ),
        ("a page token we did not mint", json!({"PageToken": "bogus"})),
        (
            "more search terms than a search may carry",
            json!({"Filters": {"SearchQuery": too_many_terms}}),
        ),
    ] {
        let (status, refused) = call(&router, "SearchProvisionedProducts", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidParametersException",
            "case {case:?}: unexpected error in {refused}"
        );
    }

    // Paging: for any page size, walking the pages yields every provisioned product
    // exactly once, and a token comes back only while more remain.
    for (case, page_size) in [
        ("one per page", 1),
        ("two per page", 2),
        ("page size equals the count", 3),
        ("page larger than the set", 10),
        ("the modelled maximum", 100),
    ] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"SortBy": "name", "PageSize": page_size});
            if let Some(token) = &token {
                request["PageToken"] = Value::String(token.clone());
            }
            let (status, page) = call(&router, "SearchProvisionedProducts", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {page}"
            );
            let listed = page["ProvisionedProducts"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no list in {page}"));
            assert!(
                listed.len() <= page_size as usize,
                "case {case:?}: PageSize {page_size} is a bound, got {} entries",
                listed.len()
            );
            assert_eq!(
                page["TotalResultsCount"].as_i64(),
                Some(3),
                "case {case:?}: every page reports the whole result count: {page}"
            );
            for attribute in listed {
                seen.push(attribute["Name"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= 5,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            match page.get("NextPageToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, ascending,
            "case {case:?}: the pages must carry every provisioned product exactly once, in the \
             sorted order"
        );
        let expected_pages = 3usize.div_ceil(page_size as usize);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s) at PageSize {page_size}, got {pages}"
        );
    }
}

/// `DescribeProvisionedProduct` addresses one provisioned product by id or by name —
/// "If you do not provide a name or ID, or you provide both name and ID, an
/// `InvalidParametersException` will occur" — and reading it changes nothing.
#[tokio::test]
async fn describing_a_provisioned_product_takes_one_identifier_and_changes_nothing() {
    let router = emulator();
    seed_provisioned_products(&router).await;
    let (status, described) = call(
        &router,
        "DescribeProvisionedProduct",
        &json!({"Name": "alpha-stack"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the describe to be served, got {status} with {described}"
    );
    let id = described["ProvisionedProductDetail"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("no Id in {described}"))
        .to_string();

    // Rows differ along which identifiers the request carries, and along the shape of
    // an identifier nothing stored carries.
    for (case, request, expected) in [
        ("neither Id nor Name", json!({}), "InvalidParametersException"),
        (
            "both Id and Name",
            json!({"Id": id, "Name": "alpha-stack"}),
            "InvalidParametersException",
        ),
        (
            "a well-formed id nothing stored carries",
            json!({"Id": "pp-000000000000"}),
            "ResourceNotFoundException",
        ),
        (
            "a name nothing stored carries",
            json!({"Name": "delta-stack"}),
            "ResourceNotFoundException",
        ),
        (
            "an id of another resource kind",
            json!({"Id": "prod-zzzzzzzzzzzz"}),
            "ResourceNotFoundException",
        ),
    ] {
        let (status, refused) = call(&router, "DescribeProvisionedProduct", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            expected,
            "case {case:?}: unexpected error in {refused}"
        );
    }

    // The reads above changed nothing a later caller sees.
    let (status, again) = call(
        &router,
        "DescribeProvisionedProduct",
        &json!({"Name": "alpha-stack"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the repeated describe to be served, got {status} with {again}"
    );
    assert_eq!(
        again["ProvisionedProductDetail"], described["ProvisionedProductDetail"],
        "a read changed what a later caller sees: {described} then {again}"
    );
}
