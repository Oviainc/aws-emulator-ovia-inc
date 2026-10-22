// SPDX-License-Identifier: CC0-1.0

//! Products, self-service actions and TagOptions over the wire: CreateProduct,
//! CreateServiceAction, CreateTagOption and the Describe readers that report what each
//! create persisted, driven as awsJson1_1 requests through the same `MultiServiceRouter`
//! the production emulator mounts.
//!
//! The rule under test: each of these creates mints a resource, persists it, and reports
//! the modelled 200 output computed from that stored record — so a Describe of the new
//! resource reports the same document the create answered, for every input in the class
//! (each `ProductType`, an artifact type given or inferred, any number of tags, any
//! `ServiceActionDefinitionMap`, any `TagOption` key/value the pattern admits).
//!
//! Every assertion here is about STORED state: a response is checked against what an
//! earlier call persisted, never against a value this file hands the service and
//! immediately reads back out of the same response.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::harness::{
    call, call_in, emulator, error_code, is_catalog_id, tag_list, tag_map, ACCOUNT_ID,
    DEFAULT_REGION,
};

/// One `CreateProduct` row.
struct ProductCase {
    case: &'static str,
    /// A `ProductType` value; the five rows below cover every one the model declares
    /// (`smithy_query query="shape" shape="ProductType"`).
    product_type: &'static str,
    /// `ProvisioningArtifactParameters.Type`, or `None` to let the service infer it.
    artifact_type: Option<&'static str>,
    artifact_name: &'static str,
    artifact_info: &'static [(&'static str, &'static str)],
    tags: &'static [(&'static str, &'static str)],
    description: Option<&'static str>,
    support_email: Option<&'static str>,
    /// The `ProvisioningArtifactDetail.Type` the answer must carry: the requested one
    /// when the row names it, otherwise the product type where the two enums share that
    /// name, and nothing at all for a product type that names no artifact type.
    expected_artifact_type: Option<&'static str>,
}

/// The rows: every `ProductType`, with the artifact type given and inferred, with and
/// without the optional members, and with 0, 1, 2 and 3 tags.
///
/// `MARKETPLACE` is the one product type that is NOT also a `ProvisioningArtifactType`
/// (the enums share CLOUD_FORMATION_TEMPLATE, TERRAFORM_OPEN_SOURCE, TERRAFORM_CLOUD and
/// EXTERNAL — `smithy_query query="shape" shape="ProductType,ProvisioningArtifactType"`),
/// so it is the row that must report no inferred artifact type and, given one of the
/// marketplace artifact types, must report that.
fn product_cases() -> Vec<ProductCase> {
    vec![
        ProductCase {
            case: "CloudFormation product, artifact type inferred",
            product_type: "CLOUD_FORMATION_TEMPLATE",
            artifact_type: None,
            artifact_name: "v1",
            artifact_info: &[("LoadTemplateFromURL", "https://example.test/one.json")],
            tags: &[],
            description: Some("the first product"),
            support_email: None,
            expected_artifact_type: Some("CLOUD_FORMATION_TEMPLATE"),
        },
        ProductCase {
            case: "marketplace product, no artifact type to infer",
            product_type: "MARKETPLACE",
            artifact_type: None,
            artifact_name: "ami-version",
            artifact_info: &[("ImportFromPhysicalId", "stack-1")],
            tags: &[("team", "ops")],
            description: None,
            support_email: None,
            expected_artifact_type: None,
        },
        ProductCase {
            case: "marketplace product with an AMI artifact type",
            product_type: "MARKETPLACE",
            artifact_type: Some("MARKETPLACE_AMI"),
            artifact_name: "v2beta",
            artifact_info: &[("ImportFromPhysicalId", "stack-2")],
            tags: &[("team", "ops"), ("tier", "gold")],
            description: Some("bought, not built"),
            support_email: Some("support@example.test"),
            expected_artifact_type: Some("MARKETPLACE_AMI"),
        },
        ProductCase {
            case: "terraform open source product, artifact type inferred",
            product_type: "TERRAFORM_OPEN_SOURCE",
            artifact_type: None,
            artifact_name: "v1.0.0",
            artifact_info: &[("LoadTemplateFromURL", "https://example.test/main.tf")],
            tags: &[("a", "1"), ("b", "2"), ("c", "3")],
            description: None,
            support_email: Some("tf@example.test"),
            expected_artifact_type: Some("TERRAFORM_OPEN_SOURCE"),
        },
        ProductCase {
            case: "terraform cloud product, artifact type given",
            product_type: "TERRAFORM_CLOUD",
            artifact_type: Some("TERRAFORM_CLOUD"),
            artifact_name: "cloud-v1",
            artifact_info: &[
                ("LoadTemplateFromURL", "https://example.test/cloud.tf"),
                ("ImportFromPhysicalId", "stack-3"),
            ],
            tags: &[("owner", "platform")],
            description: Some("terraform cloud"),
            support_email: None,
            expected_artifact_type: Some("TERRAFORM_CLOUD"),
        },
        ProductCase {
            case: "external product, artifact type inferred",
            product_type: "EXTERNAL",
            artifact_type: None,
            artifact_name: "external-1",
            artifact_info: &[("ImportFromPhysicalId", "stack-4")],
            tags: &[],
            description: Some("managed elsewhere"),
            support_email: Some("ext@example.test"),
            expected_artifact_type: Some("EXTERNAL"),
        },
        ProductCase {
            case: "the artifact's own type wins over the product's",
            product_type: "CLOUD_FORMATION_TEMPLATE",
            artifact_type: Some("EXTERNAL"),
            artifact_name: "mixed",
            artifact_info: &[("LoadTemplateFromURL", "https://example.test/two.json")],
            tags: &[("mixed", "yes")],
            description: None,
            support_email: None,
            expected_artifact_type: Some("EXTERNAL"),
        },
    ]
}

/// A `CreateProduct` request body for one row.
fn create_product_body(case: &ProductCase, token: &str) -> Value {
    let mut artifact = json!({
        "Name": case.artifact_name,
        "Info": case
            .artifact_info
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect::<serde_json::Map<String, Value>>(),
    });
    if let Some(artifact_type) = case.artifact_type {
        artifact["Type"] = json!(artifact_type);
    }

    let mut body = json!({
        "Name": format!("product-{token}"),
        "Owner": "catalog-admin",
        "ProductType": case.product_type,
        "IdempotencyToken": token,
        "ProvisioningArtifactParameters": artifact,
        "Tags": tag_list(case.tags),
    });
    if let Some(description) = case.description {
        body["Description"] = json!(description);
    }
    if let Some(support_email) = case.support_email {
        body["SupportEmail"] = json!(support_email);
    }
    body
}

/// `map` from a response body as a key -> value map of strings.
fn string_map(case: &str, value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a map, got {value}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: {key} is not a string: {value}"))
                    .to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn every_created_product_reads_back_exactly_as_stored() {
    let router = emulator();

    for (row, case) in product_cases().iter().enumerate() {
        let name = case.case;
        let token = format!("token{row}");
        let (status, created) =
            call(&router, "CreateProduct", &create_product_body(case, &token)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from CreateProduct, got {status}: {created}"
        );

        let detail = &created["ProductViewDetail"];
        let summary = &detail["ProductViewSummary"];
        let product_id = summary["ProductId"].as_str().unwrap_or_else(|| {
            panic!("case {name:?}: no ProductViewSummary.ProductId in {created}")
        });
        assert!(
            is_catalog_id("prod", product_id),
            "case {name:?}: expected a `prod-` product id, got {product_id:?}"
        );
        let view_id = summary["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no ProductViewSummary.Id in {created}"));
        assert!(
            is_catalog_id("prodview", view_id),
            "case {name:?}: expected a `prodview-` product view id, got {view_id:?}"
        );
        assert_eq!(
            detail["Status"].as_str(),
            Some("CREATED"),
            "case {name:?}: expected a created product's Status, got {detail}"
        );
        assert_eq!(
            detail["ProductARN"].as_str(),
            Some(
                format!("arn:aws:catalog:{DEFAULT_REGION}:{ACCOUNT_ID}:product/{product_id}")
                    .as_str()
            ),
            "case {name:?}: expected the catalog ARN of {product_id}, got {detail}"
        );
        assert!(
            detail["CreatedTime"].is_number(),
            "case {name:?}: expected an epoch-seconds CreatedTime, got {detail}"
        );
        assert_eq!(
            summary["Type"].as_str(),
            Some(case.product_type),
            "case {name:?}: expected the requested ProductType, got {summary}"
        );
        assert_eq!(
            summary["Owner"].as_str(),
            Some("catalog-admin"),
            "case {name:?}: expected the requested Owner, got {summary}"
        );
        assert_eq!(
            summary["Name"].as_str(),
            Some(format!("product-{token}").as_str()),
            "case {name:?}: expected the requested Name, got {summary}"
        );
        assert_eq!(
            summary["ShortDescription"].as_str(),
            case.description,
            "case {name:?}: expected the requested Description as ShortDescription, got {summary}"
        );
        assert_eq!(
            summary["SupportEmail"].as_str(),
            case.support_email,
            "case {name:?}: expected the requested SupportEmail, got {summary}"
        );
        assert_eq!(
            summary["HasDefaultPath"].as_bool(),
            Some(false),
            "case {name:?}: a product in no portfolio has no default path, got {summary}"
        );

        let artifact = &created["ProvisioningArtifactDetail"];
        let artifact_id = artifact.get("Id").and_then(Value::as_str).unwrap_or_else(|| {
            panic!("case {name:?}: no ProvisioningArtifactDetail.Id in {created}")
        });
        assert!(
            is_catalog_id("pa", artifact_id),
            "case {name:?}: expected a `pa-` artifact id, got {artifact_id:?}"
        );
        assert_eq!(
            artifact["Name"].as_str(),
            Some(case.artifact_name),
            "case {name:?}: expected the requested artifact Name, got {artifact}"
        );
        assert_eq!(
            artifact["Active"].as_bool(),
            Some(true),
            "case {name:?}: a created artifact is active, got {artifact}"
        );
        assert_eq!(
            artifact["Guidance"].as_str(),
            Some("DEFAULT"),
            "case {name:?}: a created artifact's guidance is DEFAULT, got {artifact}"
        );
        assert_eq!(
            artifact.get("Type").and_then(Value::as_str),
            case.expected_artifact_type,
            "case {name:?}: expected the artifact type {:?}, got {artifact}",
            case.expected_artifact_type
        );

        let expected_tags: BTreeMap<String, String> = case
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            tag_map(name, &created["Tags"]),
            expected_tags,
            "case {name:?}: expected the requested tags, got {created}"
        );

        // The read-back: DescribeProductAsAdmin reports the SAME detail document the
        // create answered, because both are computed from the one stored record.
        let (status, described) = call(
            &router,
            "DescribeProductAsAdmin",
            &json!({"Id": product_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from DescribeProductAsAdmin, got {status}: \
             {described}"
        );
        assert_eq!(
            described["ProductViewDetail"], created["ProductViewDetail"],
            "case {name:?}: the described product must be the created one"
        );
        assert_eq!(
            tag_map(name, &described["Tags"]),
            expected_tags,
            "case {name:?}: the described tags must be the stored ones, got {described}"
        );
        let summaries = described["ProvisioningArtifactSummaries"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {name:?}: no ProvisioningArtifactSummaries in {described}")
            });
        assert_eq!(
            summaries.len(),
            1,
            "case {name:?}: a created product has exactly its create's artifact, got \
             {described}"
        );
        assert_eq!(
            summaries[0]["Id"].as_str(),
            Some(artifact_id),
            "case {name:?}: the summarised artifact must be the created one, got {described}"
        );
        assert_eq!(
            summaries[0]["Name"].as_str(),
            Some(case.artifact_name),
            "case {name:?}: the summarised artifact name must be the stored one, got \
             {described}"
        );
        let expected_info: BTreeMap<String, String> = case
            .artifact_info
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            string_map(name, &summaries[0]["ProvisioningArtifactMetadata"]),
            expected_info,
            "case {name:?}: the artifact metadata must be the requested Info, got {described}"
        );

        // The same product by NAME, the model's other way of addressing one.
        let (status, by_name) = call(
            &router,
            "DescribeProductAsAdmin",
            &json!({"Name": format!("product-{token}")}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 describing by Name, got {status}: {by_name}"
        );
        assert_eq!(
            by_name["ProductViewDetail"], created["ProductViewDetail"],
            "case {name:?}: describing by Name must report the same product"
        );
    }
}

#[tokio::test]
async fn a_products_arn_carries_the_requests_region() {
    let router = emulator();
    // Two regions, so the ARN cannot be a constant: each product's ARN names the region
    // its create was signed for.
    for (row, region) in ["us-east-1", "eu-west-2", "ap-southeast-1"]
        .iter()
        .enumerate()
    {
        let token = format!("region{row}");
        let case = &product_cases()[0];
        let (status, created) = call_in(
            &router,
            region,
            "CreateProduct",
            &create_product_body(case, &token),
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region:?}: expected HTTP 200, got {status}: {created}"
        );
        let detail = &created["ProductViewDetail"];
        let product_id = detail["ProductViewSummary"]["ProductId"]
            .as_str()
            .unwrap_or_else(|| panic!("region {region:?}: no ProductId in {created}"));
        assert_eq!(
            detail["ProductARN"].as_str(),
            Some(format!("arn:aws:catalog:{region}:{ACCOUNT_ID}:product/{product_id}").as_str()),
            "region {region:?}: the ARN must carry the request's region, got {detail}"
        );
    }
}

#[tokio::test]
async fn a_repeated_idempotency_token_replays_the_same_product() {
    let router = emulator();
    let case = &product_cases()[0];
    let body = create_product_body(case, "repeat-token");

    let (first_status, first) = call(&router, "CreateProduct", &body).await;
    let (second_status, second) = call(&router, "CreateProduct", &body).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "both creates must answer 200, got {first_status} then {second_status}"
    );
    assert_eq!(
        first, second,
        "a repeated IdempotencyToken must replay the first product's response"
    );

    // And no second product was minted: the replayed id is still the only one a
    // differently-tokened create did not create.
    let (status, other) = call(
        &router,
        "CreateProduct",
        &create_product_body(case, "other-token"),
    )
    .await;
    assert_eq!(status, 200, "a fresh token must create a product: {other}");
    assert_ne!(
        other["ProductViewDetail"]["ProductViewSummary"]["ProductId"],
        first["ProductViewDetail"]["ProductViewSummary"]["ProductId"],
        "a different token must mint a different product"
    );
}

#[tokio::test]
async fn a_create_describing_no_version_is_refused_and_stores_nothing() {
    let router = emulator();
    // Neither member that can describe a first version: no
    // `ProvisioningArtifactParameters.Info` and no `SourceConnection`. Both rows are the
    // same refusal — the second carries artifact properties, but none that name a
    // template source.
    let rows = [
        (
            "nothing at all",
            json!({
                "Name": "no-artifact",
                "Owner": "catalog-admin",
                "ProductType": "CLOUD_FORMATION_TEMPLATE",
                "IdempotencyToken": "noartifact",
            }),
        ),
        (
            "artifact properties with no Info",
            json!({
                "Name": "no-info",
                "Owner": "catalog-admin",
                "ProductType": "CLOUD_FORMATION_TEMPLATE",
                "IdempotencyToken": "noinfo",
                "ProvisioningArtifactParameters": {"Name": "v1"},
            }),
        ),
    ];
    for (case, body) in &rows {
        let (status, refused) = call(&router, "CreateProduct", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a create that describes no version, got \
             {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidParametersException",
            "case {case:?}: expected the modelled parameter error, got {refused}"
        );
    }

    // Nothing was stored, so neither product can be described.
    for name in ["no-artifact", "no-info"] {
        let (status, described) =
            call(&router, "DescribeProductAsAdmin", &json!({"Name": name})).await;
        assert_eq!(
            (status, error_code(&described).as_str()),
            (400, "ResourceNotFoundException"),
            "a refused create must store nothing, got {status} for {name}: {described}"
        );
    }
}

#[tokio::test]
async fn a_repo_product_is_created_with_the_connection_and_no_synced_version() {
    let router = emulator();
    // A `SourceConnection` alone describes the first version: Service Catalog syncs it
    // from the repository. No sync runs in this emulator (it makes no outbound
    // connections), so the product and its connection are stored and reported, and the
    // response carries no artifact rather than a fabricated one.
    let body = json!({
        "Name": "repo-product",
        "Owner": "catalog-admin",
        "ProductType": "CLOUD_FORMATION_TEMPLATE",
        "IdempotencyToken": "repo-token",
        "SourceConnection": {
            "Type": "CODESTAR",
            "ConnectionParameters": {
                "CodeStar": {
                    "ConnectionArn": format!(
                        "arn:aws:codestar-connections:{DEFAULT_REGION}:{ACCOUNT_ID}:connection/abc"
                    ),
                    "Repository": "acme/templates",
                    "Branch": "main",
                    "ArtifactPath": "folder/file.json",
                }
            }
        },
    });
    let (status, created) = call(&router, "CreateProduct", &body).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 for a product with a source connection, got {status}: {created}"
    );
    assert_eq!(
        created["ProductViewDetail"]["SourceConnection"]["Type"].as_str(),
        Some("CODESTAR"),
        "the stored connection's type must be reported, got {created}"
    );
    assert_eq!(
        created["ProductViewDetail"]["SourceConnection"]["ConnectionParameters"]["CodeStar"]
            ["Repository"]
            .as_str(),
        Some("acme/templates"),
        "the stored connection's parameters must be reported, got {created}"
    );
    assert!(
        created["ProductViewDetail"]["SourceConnection"]
            .get("LastSync")
            .is_none(),
        "no sync has run, so the connection has no LastSync, got {created}"
    );
    assert!(
        created.get("ProvisioningArtifactDetail").is_none(),
        "no artifact was described or synced, so none may be reported, got {created}"
    );

    let product_id = created["ProductViewDetail"]["ProductViewSummary"]["ProductId"]
        .as_str()
        .expect("the created product has an id");
    let (status, described) = call(
        &router,
        "DescribeProductAsAdmin",
        &json!({"Id": product_id}),
    )
    .await;
    assert_eq!(status, 200, "expected HTTP 200 reading it back: {described}");
    assert_eq!(
        described["ProductViewDetail"], created["ProductViewDetail"],
        "the described product must be the created one"
    );
    assert_eq!(
        described["ProvisioningArtifactSummaries"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "a product with no synced version has no artifact summaries, got {described}"
    );
}

#[tokio::test]
async fn describing_a_product_without_an_identifier_is_refused() {
    let router = emulator();
    let (status, refused) = call(&router, "DescribeProductAsAdmin", &json!({})).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "InvalidParametersException"),
        "a describe naming neither Id nor Name must be refused, got {status}: {refused}"
    );

    let (status, missing) = call(
        &router,
        "DescribeProductAsAdmin",
        &json!({"Id": "prod-000000000000"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&missing).as_str()),
        (400, "ResourceNotFoundException"),
        "an unknown product id must be ResourceNotFound, got {status}: {missing}"
    );
}

/// One `CreateServiceAction` row.
struct ActionCase {
    case: &'static str,
    name: &'static str,
    description: Option<&'static str>,
    /// Keys are `ServiceActionDefinitionKey` values: Name, Version, AssumeRole,
    /// Parameters (`smithy_query query="shape" shape="ServiceActionDefinitionKey"`).
    definition: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn every_created_service_action_reads_back_exactly_as_stored() {
    let router = emulator();
    let cases = [
        ActionCase {
            case: "the smallest definition",
            name: "RestartInstance",
            description: None,
            definition: &[("Name", "AWS-RestartEC2Instance")],
        },
        ActionCase {
            case: "every definition key",
            name: "Restart.Instance_v2-1",
            description: Some("restarts the instance"),
            definition: &[
                ("Name", "AWS-RestartEC2Instance"),
                ("Version", "1"),
                ("AssumeRole", "arn:aws:iam::000000000000:role/SCRole"),
                ("Parameters", "[{\"Name\":\"InstanceId\",\"Type\":\"TARGET\"}]"),
            ],
        },
        ActionCase {
            case: "a name of digits and underscores",
            name: "action_0123456789",
            description: Some("named oddly, stored the same"),
            definition: &[("Name", "AWS-StopEC2Instance"), ("Version", "3")],
        },
    ];

    for (row, case) in cases.iter().enumerate() {
        let name = case.case;
        let definition: serde_json::Map<String, Value> = case
            .definition
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect();
        let mut body = json!({
            "Name": case.name,
            "DefinitionType": "SSM_AUTOMATION",
            "Definition": definition,
            "IdempotencyToken": format!("action{row}"),
        });
        if let Some(description) = case.description {
            body["Description"] = json!(description);
        }

        let (status, created) = call(&router, "CreateServiceAction", &body).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from CreateServiceAction, got {status}: {created}"
        );
        let detail = &created["ServiceActionDetail"];
        let summary = &detail["ServiceActionSummary"];
        let id = summary["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no ServiceActionSummary.Id in {created}"));
        assert!(
            is_catalog_id("act", id),
            "case {name:?}: expected an `act-` service-action id, got {id:?}"
        );
        assert_eq!(
            summary["Name"].as_str(),
            Some(case.name),
            "case {name:?}: expected the requested Name, got {summary}"
        );
        assert_eq!(
            summary["DefinitionType"].as_str(),
            Some("SSM_AUTOMATION"),
            "case {name:?}: expected the requested DefinitionType, got {summary}"
        );
        assert_eq!(
            summary.get("Description").and_then(Value::as_str),
            case.description,
            "case {name:?}: expected the requested Description, got {summary}"
        );
        let expected_definition: BTreeMap<String, String> = case
            .definition
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            string_map(name, &detail["Definition"]),
            expected_definition,
            "case {name:?}: expected the requested Definition, got {detail}"
        );

        let (status, described) = call(&router, "DescribeServiceAction", &json!({"Id": id})).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from DescribeServiceAction, got {status}: \
             {described}"
        );
        assert_eq!(
            described["ServiceActionDetail"], created["ServiceActionDetail"],
            "case {name:?}: the described action must be the created one"
        );
    }

    let (status, missing) = call(
        &router,
        "DescribeServiceAction",
        &json!({"Id": "act-000000000000"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&missing).as_str()),
        (400, "ResourceNotFoundException"),
        "an unknown service-action id must be ResourceNotFound, got {status}: {missing}"
    );
}

#[tokio::test]
async fn a_repeated_idempotency_token_replays_the_same_service_action() {
    let router = emulator();
    let body = json!({
        "Name": "ReplayedAction",
        "DefinitionType": "SSM_AUTOMATION",
        "Definition": {"Name": "AWS-RestartEC2Instance"},
        "IdempotencyToken": "replay-action",
    });
    let (first_status, first) = call(&router, "CreateServiceAction", &body).await;
    let (second_status, second) = call(&router, "CreateServiceAction", &body).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "both creates must answer 200, got {first_status} then {second_status}"
    );
    assert_eq!(
        first, second,
        "a repeated IdempotencyToken must replay the first action's response"
    );
}

#[tokio::test]
async fn every_created_tag_option_reads_back_exactly_as_stored() {
    let router = emulator();
    // The modelled `TagOptionKey`/`TagOptionValue` pattern is
    // `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` with lengths 1..128 and 1..256
    // (`smithy_query query="operation" operation="CreateTagOption"`), so the rows span
    // letters, marks of separation, digits and each punctuation character it admits —
    // and both published upper bounds.
    let long_key = "k".repeat(128);
    let long_value = "v".repeat(256);
    let cases: Vec<(&str, String, String)> = vec![
        (
            "a plain pair",
            "Environment".to_string(),
            "Production".to_string(),
        ),
        (
            "the same key, another value",
            "Environment".to_string(),
            "Staging".to_string(),
        ),
        (
            "punctuation the pattern admits",
            "cost:center-1_2.3".to_string(),
            "a/b=c+d-e@f".to_string(),
        ),
        (
            "letters outside ASCII and a separator",
            "Ünïcøde Schlüssel".to_string(),
            "ключ значение".to_string(),
        ),
        ("digits only", "12345".to_string(), "67890".to_string()),
        ("the longest key", long_key, "bounded".to_string()),
        ("the longest value", "bounded".to_string(), long_value),
    ];

    let mut seen: Vec<String> = Vec::new();
    for (name, key, value) in &cases {
        let (status, created) =
            call(&router, "CreateTagOption", &json!({"Key": key, "Value": value})).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from CreateTagOption, got {status}: {created}"
        );
        let detail = &created["TagOptionDetail"];
        let id = detail["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no TagOptionDetail.Id in {created}"));
        assert!(
            is_catalog_id("tag", id),
            "case {name:?}: expected a `tag-` TagOption id, got {id:?}"
        );
        assert!(
            !seen.contains(&id.to_string()),
            "case {name:?}: the id {id:?} was already minted for another TagOption"
        );
        seen.push(id.to_string());
        assert_eq!(
            detail["Key"].as_str(),
            Some(key.as_str()),
            "case {name:?}: expected the requested Key, got {detail}"
        );
        assert_eq!(
            detail["Value"].as_str(),
            Some(value.as_str()),
            "case {name:?}: expected the requested Value, got {detail}"
        );
        assert_eq!(
            detail["Active"].as_bool(),
            Some(true),
            "case {name:?}: a created TagOption is active, got {detail}"
        );
        assert_eq!(
            detail["Owner"].as_str(),
            Some(ACCOUNT_ID),
            "case {name:?}: expected the creating account as Owner, got {detail}"
        );

        let (status, described) = call(&router, "DescribeTagOption", &json!({"Id": id})).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from DescribeTagOption, got {status}: {described}"
        );
        assert_eq!(
            described["TagOptionDetail"], created["TagOptionDetail"],
            "case {name:?}: the described TagOption must be the created one"
        );
    }

    // A pair that already exists is the one refusal this operation models for a
    // well-formed request.
    let (status, duplicate) = call(
        &router,
        "CreateTagOption",
        &json!({"Key": "Environment", "Value": "Production"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&duplicate).as_str()),
        (400, "DuplicateResourceException"),
        "a repeated key/value pair must be refused as a duplicate, got {status}: {duplicate}"
    );

    let (status, missing) = call(
        &router,
        "DescribeTagOption",
        &json!({"Id": "tag-000000000000"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&missing).as_str()),
        (400, "ResourceNotFoundException"),
        "an unknown TagOption id must be ResourceNotFound, got {status}: {missing}"
    );
}

#[tokio::test]
async fn a_tag_option_past_a_modelled_bound_is_refused_and_stores_nothing() {
    let router = emulator();
    // One character past each published upper bound (`TagOptionKey` 1..128,
    // `TagOptionValue` 1..256), and the empty string one below each lower bound.
    let rows = [
        ("a key past 128 characters", "k".repeat(129), "v".to_string()),
        (
            "a value past 256 characters",
            "k".to_string(),
            "v".repeat(257),
        ),
        ("an empty key", String::new(), "v".to_string()),
        ("an empty value", "k".to_string(), String::new()),
    ];
    for (name, key, value) in &rows {
        let (status, refused) =
            call(&router, "CreateTagOption", &json!({"Key": key, "Value": value})).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400 outside the modelled bound, got {status}: \
             {refused}"
        );
        // Nothing was stored: the same request is refused the same way, never as a
        // duplicate of a record the first attempt left behind.
        let (again_status, again) =
            call(&router, "CreateTagOption", &json!({"Key": key, "Value": value})).await;
        assert_eq!(
            (again_status, error_code(&again)),
            (status, error_code(&refused)),
            "case {name:?}: a refused create must store nothing, got {again_status}: {again}"
        );
    }
}

#[tokio::test]
async fn an_update_changes_only_the_members_it_supplies() {
    let router = emulator();

    // --- a self-service action -------------------------------------------------------
    let (_, created) = call(
        &router,
        "CreateServiceAction",
        &json!({
            "Name": "UpdatedAction",
            "DefinitionType": "SSM_AUTOMATION",
            "Definition": {"Name": "AWS-RestartEC2Instance", "Version": "1"},
            "Description": "the original description",
            "IdempotencyToken": "update-action",
        }),
    )
    .await;
    let action_id = created["ServiceActionDetail"]["ServiceActionSummary"]["Id"]
        .as_str()
        .expect("the created action has an id")
        .to_string();

    // One row per supplied-member combination: each must change exactly what it names
    // and leave everything else at what the previous call stored.
    /// What an `UpdateServiceAction` row sends, and the whole record it must leave.
    struct ActionUpdate {
        case: &'static str,
        body: Value,
        expected_name: &'static str,
        expected_description: Option<&'static str>,
        expected_definition: &'static [(&'static str, &'static str)],
    }
    let action_rows = vec![
        ActionUpdate {
            case: "only the description",
            body: json!({"Id": action_id, "Description": "a new description"}),
            expected_name: "UpdatedAction",
            expected_description: Some("a new description"),
            expected_definition: &[("Name", "AWS-RestartEC2Instance"), ("Version", "1")],
        },
        ActionUpdate {
            case: "only the name",
            body: json!({"Id": action_id, "Name": "RenamedAction"}),
            expected_name: "RenamedAction",
            expected_description: Some("a new description"),
            expected_definition: &[("Name", "AWS-RestartEC2Instance"), ("Version", "1")],
        },
        ActionUpdate {
            case: "only the definition",
            body: json!({
                "Id": action_id,
                "Definition": {"Name": "AWS-StopEC2Instance", "Version": "2", "AssumeRole": "arn:aws:iam::000000000000:role/SCRole"},
            }),
            expected_name: "RenamedAction",
            expected_description: Some("a new description"),
            expected_definition: &[
                ("Name", "AWS-StopEC2Instance"),
                ("Version", "2"),
                ("AssumeRole", "arn:aws:iam::000000000000:role/SCRole"),
            ],
        },
        ActionUpdate {
            case: "nothing but the id",
            body: json!({"Id": action_id}),
            expected_name: "RenamedAction",
            expected_description: Some("a new description"),
            expected_definition: &[
                ("Name", "AWS-StopEC2Instance"),
                ("Version", "2"),
                ("AssumeRole", "arn:aws:iam::000000000000:role/SCRole"),
            ],
        },
    ];
    for row in action_rows {
        let case = row.case;
        let (status, updated) = call(&router, "UpdateServiceAction", &row.body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateServiceAction, got {status}: {updated}"
        );
        // The update's own answer AND a later read must both report the stored record.
        let (_, described) = call(
            &router,
            "DescribeServiceAction",
            &json!({"Id": action_id}),
        )
        .await;
        for (source, body) in [("the update", &updated), ("the read-back", &described)] {
            let summary = &body["ServiceActionDetail"]["ServiceActionSummary"];
            assert_eq!(
                summary["Name"].as_str(),
                Some(row.expected_name),
                "case {case:?}: {source} must report the stored Name, got {body}"
            );
            assert_eq!(
                summary.get("Description").and_then(Value::as_str),
                row.expected_description,
                "case {case:?}: {source} must report the stored Description, got {body}"
            );
            assert_eq!(
                summary["DefinitionType"].as_str(),
                Some("SSM_AUTOMATION"),
                "case {case:?}: {source} must keep the stored DefinitionType, got {body}"
            );
            assert_eq!(
                string_map(case, &body["ServiceActionDetail"]["Definition"]),
                row.expected_definition
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect::<BTreeMap<String, String>>(),
                "case {case:?}: {source} must report the stored Definition, got {body}"
            );
        }
    }

    // --- a TagOption -----------------------------------------------------------------
    let (_, created) = call(
        &router,
        "CreateTagOption",
        &json!({"Key": "Stage", "Value": "alpha"}),
    )
    .await;
    let option_id = created["TagOptionDetail"]["Id"]
        .as_str()
        .expect("the created TagOption has an id")
        .to_string();

    let option_rows: Vec<(&str, Value, &str, bool)> = vec![
        (
            "only the value",
            json!({"Id": option_id, "Value": "beta"}),
            "beta",
            true,
        ),
        (
            "only the active flag",
            json!({"Id": option_id, "Active": false}),
            "beta",
            false,
        ),
        (
            "both members",
            json!({"Id": option_id, "Value": "gamma", "Active": true}),
            "gamma",
            true,
        ),
        (
            "nothing but the id",
            json!({"Id": option_id}),
            "gamma",
            true,
        ),
    ];
    for (case, body, expected_value, expected_active) in option_rows {
        let (status, updated) = call(&router, "UpdateTagOption", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateTagOption, got {status}: {updated}"
        );
        let (_, described) = call(&router, "DescribeTagOption", &json!({"Id": option_id})).await;
        for (source, body) in [("the update", &updated), ("the read-back", &described)] {
            let detail = &body["TagOptionDetail"];
            assert_eq!(
                detail["Key"].as_str(),
                Some("Stage"),
                "case {case:?}: {source} must keep the stored Key, got {body}"
            );
            assert_eq!(
                detail["Value"].as_str(),
                Some(expected_value),
                "case {case:?}: {source} must report the stored Value, got {body}"
            );
            assert_eq!(
                detail["Active"].as_bool(),
                Some(expected_active),
                "case {case:?}: {source} must report the stored Active, got {body}"
            );
            assert_eq!(
                detail["Id"].as_str(),
                Some(option_id.as_str()),
                "case {case:?}: {source} must keep the stored Id, got {body}"
            );
        }
    }

    // A value another TagOption already carries under the same key is the duplicate this
    // operation models — and the refused update changes nothing.
    let (_, other) = call(
        &router,
        "CreateTagOption",
        &json!({"Key": "Stage", "Value": "delta"}),
    )
    .await;
    let other_id = other["TagOptionDetail"]["Id"]
        .as_str()
        .expect("the second TagOption has an id")
        .to_string();
    let (status, refused) = call(
        &router,
        "UpdateTagOption",
        &json!({"Id": other_id, "Value": "gamma"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "DuplicateResourceException"),
        "a value an existing TagOption already has under this key must be refused, got \
         {status}: {refused}"
    );
    let (_, described) = call(&router, "DescribeTagOption", &json!({"Id": other_id})).await;
    assert_eq!(
        described["TagOptionDetail"]["Value"].as_str(),
        Some("delta"),
        "a refused update must leave the stored value alone, got {described}"
    );

    // An unknown id is the modelled not-found on both updates.
    for (operation, body) in [
        ("UpdateServiceAction", json!({"Id": "act-000000000000"})),
        (
            "UpdateTagOption",
            json!({"Id": "tag-000000000000", "Active": false}),
        ),
    ] {
        let (status, missing) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&missing).as_str()),
            (400, "ResourceNotFoundException"),
            "{operation} on an unknown id must be ResourceNotFound, got {status}: {missing}"
        );
    }
}

#[tokio::test]
async fn a_delete_makes_the_resource_unreadable_and_frees_its_token() {
    let router = emulator();

    // A service action: create, delete, and the read that proves it is gone.
    let token = "delete-action";
    let (_, created) = call(
        &router,
        "CreateServiceAction",
        &json!({
            "Name": "DeletedAction",
            "DefinitionType": "SSM_AUTOMATION",
            "Definition": {"Name": "AWS-RestartEC2Instance"},
            "IdempotencyToken": token,
        }),
    )
    .await;
    let action_id = created["ServiceActionDetail"]["ServiceActionSummary"]["Id"]
        .as_str()
        .expect("the created action has an id")
        .to_string();

    let (status, deleted) = call(
        &router,
        "DeleteServiceAction",
        &json!({"Id": action_id, "IdempotencyToken": "delete-token"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from DeleteServiceAction, got {status}: {deleted}"
    );
    let (status, gone) = call(
        &router,
        "DescribeServiceAction",
        &json!({"Id": action_id}),
    )
    .await;
    assert_eq!(
        (status, error_code(&gone).as_str()),
        (400, "ResourceNotFoundException"),
        "a deleted action must be unreadable, got {status}: {gone}"
    );
    // A second delete of the same id is the modelled not-found, not a silent success.
    let (status, again) = call(&router, "DeleteServiceAction", &json!({"Id": action_id})).await;
    assert_eq!(
        (status, error_code(&again).as_str()),
        (400, "ResourceNotFoundException"),
        "deleting an already-deleted action must be ResourceNotFound, got {status}: {again}"
    );
    // The delete freed the create's idempotency token: repeating it mints a NEW action
    // rather than replaying the deleted one.
    let (status, recreated) = call(
        &router,
        "CreateServiceAction",
        &json!({
            "Name": "DeletedAction",
            "DefinitionType": "SSM_AUTOMATION",
            "Definition": {"Name": "AWS-RestartEC2Instance"},
            "IdempotencyToken": token,
        }),
    )
    .await;
    assert_eq!(status, 200, "the re-create must answer 200: {recreated}");
    let new_id = recreated["ServiceActionDetail"]["ServiceActionSummary"]["Id"]
        .as_str()
        .expect("the re-created action has an id");
    assert_ne!(
        new_id, action_id,
        "a token whose action was deleted must create a new action, not replay the old id"
    );

    // A TagOption: the same three assertions, plus that its key/value pair is free again.
    let (_, created) = call(
        &router,
        "CreateTagOption",
        &json!({"Key": "Removable", "Value": "yes"}),
    )
    .await;
    let option_id = created["TagOptionDetail"]["Id"]
        .as_str()
        .expect("the created TagOption has an id")
        .to_string();
    let (status, deleted) = call(&router, "DeleteTagOption", &json!({"Id": option_id})).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from DeleteTagOption, got {status}: {deleted}"
    );
    let (status, gone) = call(&router, "DescribeTagOption", &json!({"Id": option_id})).await;
    assert_eq!(
        (status, error_code(&gone).as_str()),
        (400, "ResourceNotFoundException"),
        "a deleted TagOption must be unreadable, got {status}: {gone}"
    );
    let (status, again) = call(&router, "DeleteTagOption", &json!({"Id": option_id})).await;
    assert_eq!(
        (status, error_code(&again).as_str()),
        (400, "ResourceNotFoundException"),
        "deleting an already-deleted TagOption must be ResourceNotFound, got {status}: {again}"
    );
    let (status, recreated) = call(
        &router,
        "CreateTagOption",
        &json!({"Key": "Removable", "Value": "yes"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the deleted pair is free again, so the re-create must answer 200: {recreated}"
    );
    assert_ne!(
        recreated["TagOptionDetail"]["Id"].as_str(),
        Some(option_id.as_str()),
        "the re-created TagOption must carry a newly minted id"
    );
}

#[tokio::test]
async fn creates_past_the_product_quota_are_refused_and_store_nothing() {
    // The store's bound, named by the crate so the test cannot drift from it.
    const QUOTA: usize =
        service_com_amazonaws_servicecatalog_aws242servicecatalogservice::state::MAX_PRODUCTS;
    let router = emulator();
    let case = &product_cases()[0];

    for row in 0..QUOTA {
        let (status, created) = call(
            &router,
            "CreateProduct",
            &create_product_body(case, &format!("quota{row}")),
        )
        .await;
        assert_eq!(
            status, 200,
            "product {row} of the quota must be created, got {status}: {created}"
        );
    }

    // One past the bound, with a fresh token: refused with the modelled quota error.
    let (status, refused) = call(
        &router,
        "CreateProduct",
        &create_product_body(case, "quota-over"),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "LimitExceededException"),
        "one past the quota must be refused, got {status}: {refused}"
    );
    // A token already used still replays: a replay creates nothing, so the bound does
    // not apply to it.
    let (status, replayed) = call(
        &router,
        "CreateProduct",
        &create_product_body(case, "quota0"),
    )
    .await;
    assert_eq!(
        status, 200,
        "a replay at the quota creates nothing, so it is not refused: {replayed}"
    );
    // And the refusal stored nothing: the next create is still refused, not accepted
    // because the store grew.
    let (status, again) = call(
        &router,
        "CreateProduct",
        &create_product_body(case, "quota-over-2"),
    )
    .await;
    assert_eq!(
        (status, error_code(&again).as_str()),
        (400, "LimitExceededException"),
        "a refused create must store nothing, got {status}: {again}"
    );
}

#[tokio::test]
async fn reads_do_not_change_stored_state() {
    let router = emulator();
    let case = &product_cases()[0];
    let (_, created) = call(
        &router,
        "CreateProduct",
        &create_product_body(case, "stable-token"),
    )
    .await;
    let product_id = created["ProductViewDetail"]["ProductViewSummary"]["ProductId"]
        .as_str()
        .expect("the created product has an id")
        .to_string();

    let (first_status, first) = call(
        &router,
        "DescribeProductAsAdmin",
        &json!({"Id": product_id}),
    )
    .await;
    let (second_status, second) = call(
        &router,
        "DescribeProductAsAdmin",
        &json!({"Id": product_id}),
    )
    .await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "both describes must answer 200"
    );
    assert_eq!(
        first, second,
        "a read must leave the stored product exactly as the previous read saw it"
    );
}
