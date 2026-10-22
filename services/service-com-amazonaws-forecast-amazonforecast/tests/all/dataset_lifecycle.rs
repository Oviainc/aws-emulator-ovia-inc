// SPDX-License-Identifier: CC0-1.0

//! The dataset / dataset-group lifecycle, driven end to end over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AmazonForecast.<Operation>`).
//!
//! Every assertion here is about a RULE, exercised over a table that spans the class the
//! rule generalises over — the enum catalogues the model declares, the documented
//! `DataFrequency` intervals, membership lists of different length and order, and page
//! sizes against collections of different size — rather than about one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every call is signed for by default; a request's SigV4 credential scope is
/// both how the router picks the service and where the ARN's region segment comes from.
/// The signature is never checked here: the suite turns request authentication off below.
const REGION: &str = "us-west-2";

/// `Domain`, every value the model declares
/// (`smithy_query(query="shape",shape="Domain")`).
const DOMAINS: &[&str] = &[
    "RETAIL",
    "CUSTOM",
    "INVENTORY_PLANNING",
    "EC2_CAPACITY",
    "WORK_FORCE",
    "WEB_TRAFFIC",
    "METRICS",
];

/// `DatasetType`, every value the model declares
/// (`smithy_query(query="shape",shape="DatasetType")`).
const DATASET_TYPES: &[&str] = &["TARGET_TIME_SERIES", "RELATED_TIME_SERIES", "ITEM_METADATA"];

/// Every interval `DataFrequency` admits, from the member's pattern
/// `^Y|M|W|D|H|30min|15min|10min|5min|1min$`
/// (`smithy_query(query="operation",operation="CreateDataset")`).
const DATA_FREQUENCIES: &[&str] = &[
    "Y", "M", "W", "D", "H", "30min", "15min", "10min", "5min", "1min",
];

/// A `RoleArn` for `EncryptionConfig`: the member's pattern is
/// `^arn:([a-z\d-]+):forecast:.*:.*:.+$`.
const ROLE_ARN: &str = "arn:aws:forecast:us-west-2:000000000000:role/forecast_access";

/// A `KMSKeyArn` for `EncryptionConfig`: the member's pattern is `^arn:aws:kms:.*:key/`.
const KMS_KEY_ARN: &str = "arn:aws:kms:us-west-2:000000000000:key/2f1c4a90";

/// One row of the dataset table.
#[derive(Debug, Clone)]
struct DatasetCase {
    name: &'static str,
    domain: &'static str,
    dataset_type: &'static str,
    data_frequency: Option<&'static str>,
    attributes: &'static [(&'static str, &'static str)],
    encrypted: bool,
    tags: &'static [(&'static str, &'static str)],
}

/// The target-time-series schema the Forecast `RETAIL` domain requires.
const RETAIL_TARGET: &[(&str, &str)] = &[
    ("item_id", "string"),
    ("timestamp", "timestamp"),
    ("demand", "float"),
];

/// A related-time-series schema: a different attribute count and types from
/// [`RETAIL_TARGET`], so the schema is not stored by shape.
const RELATED: &[(&str, &str)] = &[
    ("item_id", "string"),
    ("timestamp", "timestamp"),
    ("price", "float"),
    ("promotion", "integer"),
];

/// An item-metadata schema: no timestamp at all.
const ITEM_METADATA: &[(&str, &str)] = &[("item_id", "string"), ("category", "string")];

/// A geolocation-carrying schema, so every `AttributeType` the model lists appears
/// somewhere in the table (`smithy_query(query="shape",shape="SchemaAttribute")`).
const GEO_TARGET: &[(&str, &str)] = &[
    ("item_id", "string"),
    ("timestamp", "timestamp"),
    ("target_value", "float"),
    ("location", "geolocation"),
];

/// The table: all seven `Domain` values, all three `DatasetType` values, present and
/// absent `DataFrequency`, present and absent `EncryptionConfig`, four schema shapes.
/// The issue's own case (`RETAIL` / `TARGET_TIME_SERIES`) is just the first row.
const DATASET_TABLE: &[DatasetCase] = &[
    DatasetCase {
        name: "retail_target",
        domain: "RETAIL",
        dataset_type: "TARGET_TIME_SERIES",
        data_frequency: Some("D"),
        attributes: RETAIL_TARGET,
        encrypted: false,
        tags: &[],
    },
    DatasetCase {
        name: "custom_metadata",
        domain: "CUSTOM",
        dataset_type: "ITEM_METADATA",
        data_frequency: None,
        attributes: ITEM_METADATA,
        encrypted: false,
        tags: &[("owner", "planning")],
    },
    DatasetCase {
        name: "web_related",
        domain: "WEB_TRAFFIC",
        dataset_type: "RELATED_TIME_SERIES",
        data_frequency: Some("15min"),
        attributes: RELATED,
        encrypted: true,
        tags: &[],
    },
    DatasetCase {
        name: "metrics_target",
        domain: "METRICS",
        dataset_type: "TARGET_TIME_SERIES",
        data_frequency: Some("1min"),
        attributes: GEO_TARGET,
        encrypted: true,
        tags: &[("team", "sre"), ("tier", "gold")],
    },
    DatasetCase {
        name: "ec2_target",
        domain: "EC2_CAPACITY",
        dataset_type: "TARGET_TIME_SERIES",
        data_frequency: Some("H"),
        attributes: RETAIL_TARGET,
        encrypted: false,
        tags: &[],
    },
    DatasetCase {
        name: "workforce_target",
        domain: "WORK_FORCE",
        dataset_type: "TARGET_TIME_SERIES",
        data_frequency: Some("W"),
        attributes: RETAIL_TARGET,
        encrypted: false,
        tags: &[],
    },
    DatasetCase {
        name: "inventory_related",
        domain: "INVENTORY_PLANNING",
        dataset_type: "RELATED_TIME_SERIES",
        data_frequency: Some("Y"),
        attributes: RELATED,
        encrypted: false,
        tags: &[],
    },
];

/// A router carrying only this service, over a fresh engine: each test starts from an
/// empty store.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are verified by the emulator, not by the service; this suite is about
    // the service's own behaviour, so it signs nothing and checks nothing.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_forecast_amazonforecast::EmulatedService::new(engine),
    );
    router
}

/// Call one operation signed for [`REGION`], returning its HTTP status and parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call_signed_for(router, REGION, operation, body).await
}

/// Call one operation with a credential scope naming `region`, so a test can exercise the
/// region segment of the ARNs the service mints.
async fn call_signed_for(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/forecast/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=abc123"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonForecast.{operation}"))
        .header("authorization", authorization)
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("the request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code (`__type`) of an error response.
fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or("<no __type>")
}

fn schema_json(attributes: &[(&str, &str)]) -> Value {
    json!({
        "Attributes": attributes
            .iter()
            .map(|(name, kind)| json!({"AttributeName": name, "AttributeType": kind}))
            .collect::<Vec<_>>(),
    })
}

fn encryption_json() -> Value {
    json!({"RoleArn": ROLE_ARN, "KMSKeyArn": KMS_KEY_ARN})
}

fn create_dataset_body(case: &DatasetCase) -> Value {
    let mut body = json!({
        "DatasetName": case.name,
        "Domain": case.domain,
        "DatasetType": case.dataset_type,
        "Schema": schema_json(case.attributes),
    });
    if let Some(frequency) = case.data_frequency {
        body["DataFrequency"] = json!(frequency);
    }
    if case.encrypted {
        body["EncryptionConfig"] = encryption_json();
    }
    if !case.tags.is_empty() {
        body["Tags"] = json!(case
            .tags
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect::<Vec<_>>());
    }
    body
}

/// The ARN this emulator must mint for a dataset: the account comes from the framework's
/// single source of truth, the region from the request's credential scope.
fn expected_dataset_arn(name: &str) -> String {
    format!(
        "arn:aws:forecast:{REGION}:{}:dataset/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

fn expected_group_arn(name: &str) -> String {
    format!(
        "arn:aws:forecast:{REGION}:{}:dataset-group/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

async fn create_dataset(router: &MultiServiceRouter, case: &DatasetCase) -> String {
    let (status, body) = call(router, "CreateDataset", create_dataset_body(case)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateDataset 200, got {status} with {body}"
    );
    body["DatasetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a DatasetArn, got {body}"))
        .to_string()
}

/// A minimal dataset in `domain`, for the tests that only need membership.
fn plain_case(name: &'static str, domain: &'static str) -> DatasetCase {
    DatasetCase {
        name,
        domain,
        dataset_type: "TARGET_TIME_SERIES",
        data_frequency: Some("D"),
        attributes: RETAIL_TARGET,
        encrypted: false,
        tags: &[],
    }
}

async fn create_group(
    router: &MultiServiceRouter,
    name: &str,
    domain: &str,
    members: &[String],
) -> String {
    let (status, body) = call(
        router,
        "CreateDatasetGroup",
        json!({"DatasetGroupName": name, "Domain": domain, "DatasetArns": members}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case group {name} with {members:?}: expected CreateDatasetGroup 200, got {status} with {body}"
    );
    body["DatasetGroupArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case group {name}: expected a DatasetGroupArn, got {body}"))
        .to_string()
}

/// Walk every page of a listing, asserting the page-size bound and that only a
/// non-final page carries a token. Returns the ARNs in the order served.
async fn collect_listing(
    router: &MultiServiceRouter,
    operation: &str,
    items_member: &str,
    arn_member: &str,
    max_results: Option<i32>,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut body = json!({});
        if let Some(limit) = max_results {
            body["MaxResults"] = json!(limit);
        }
        if let Some(cursor) = &token {
            body["NextToken"] = json!(cursor);
        }
        let (status, response) = call(router, operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation} max_results={max_results:?} page {pages}: expected 200, got {status} with {response}"
        );
        let items = response[items_member]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!(
                "case {operation} max_results={max_results:?}: expected a {items_member} array, got {response}"
            ));
        if let Some(limit) = max_results {
            assert!(
                items.len() <= limit as usize,
                "case {operation} max_results={max_results:?} page {pages}: expected at most {limit} items, got {}",
                items.len()
            );
        }
        for item in &items {
            seen.push(
                item[arn_member]
                    .as_str()
                    .unwrap_or_else(|| panic!(
                        "case {operation} max_results={max_results:?}: expected {arn_member} on every summary, got {item}"
                    ))
                    .to_string(),
            );
        }
        pages += 1;
        match response["NextToken"].as_str() {
            Some(next) => {
                assert!(
                    !items.is_empty(),
                    "case {operation} max_results={max_results:?} page {pages}: a page carrying a NextToken must not be empty"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
        assert!(
            pages < 64,
            "case {operation} max_results={max_results:?}: pagination did not terminate, saw {seen:?}"
        );
    }
    seen
}

#[tokio::test]
async fn a_created_dataset_reads_back_exactly_as_it_was_written() {
    let router = router();

    for case in DATASET_TABLE {
        let arn = create_dataset(&router, case).await;
        assert_eq!(
            arn,
            expected_dataset_arn(case.name),
            "case {case:?}: expected the minted DatasetArn to name the dataset, got {arn}"
        );

        let (status, got) = call(&router, "DescribeDataset", json!({"DatasetArn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeDataset 200, got {status} with {got}"
        );
        assert_eq!(
            got["DatasetArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected DatasetArn {arn}, got {got}"
        );
        assert_eq!(
            got["DatasetName"].as_str(),
            Some(case.name),
            "case {case:?}: expected DatasetName {}, got {got}",
            case.name
        );
        assert_eq!(
            got["Domain"].as_str(),
            Some(case.domain),
            "case {case:?}: expected Domain {}, got {got}",
            case.domain
        );
        assert_eq!(
            got["DatasetType"].as_str(),
            Some(case.dataset_type),
            "case {case:?}: expected DatasetType {}, got {got}",
            case.dataset_type
        );
        assert_eq!(
            got["DataFrequency"].as_str(),
            case.data_frequency,
            "case {case:?}: expected DataFrequency {:?}, got {got}",
            case.data_frequency
        );
        assert_eq!(
            got["Schema"],
            schema_json(case.attributes),
            "case {case:?}: expected the submitted Schema back, got {got}"
        );
        if case.encrypted {
            assert_eq!(
                got["EncryptionConfig"],
                encryption_json(),
                "case {case:?}: expected the submitted EncryptionConfig back, got {got}"
            );
        } else {
            assert!(
                got.get("EncryptionConfig").is_none(),
                "case {case:?}: expected no EncryptionConfig, got {got}"
            );
        }
        assert_eq!(
            got["Status"].as_str(),
            Some("ACTIVE"),
            "case {case:?}: expected Status ACTIVE, got {got}"
        );
        let creation = got["CreationTime"].as_f64();
        let modified = got["LastModificationTime"].as_f64();
        assert!(
            creation.is_some(),
            "case {case:?}: expected a CreationTime, got {got}"
        );
        assert_eq!(
            creation, modified,
            "case {case:?}: an unmodified dataset must report LastModificationTime == CreationTime, got {got}"
        );
    }
}

#[tokio::test]
async fn a_describe_is_stable_across_reads() {
    let router = router();
    let case = &DATASET_TABLE[0];
    let arn = create_dataset(&router, case).await;

    let (_, first) = call(&router, "DescribeDataset", json!({"DatasetArn": &arn})).await;
    let (_, second) = call(&router, "DescribeDataset", json!({"DatasetArn": &arn})).await;
    assert_eq!(
        first, second,
        "case {case:?}: reading a dataset must not change what the next reader sees: {first} then {second}"
    );

    let group_arn = create_group(&router, "stable_group", case.domain, &[arn]).await;
    let (_, first) = call(
        &router,
        "DescribeDatasetGroup",
        json!({"DatasetGroupArn": &group_arn}),
    )
    .await;
    let (_, second) = call(
        &router,
        "DescribeDatasetGroup",
        json!({"DatasetGroupArn": &group_arn}),
    )
    .await;
    assert_eq!(
        first, second,
        "case group stable_group: reading a dataset group must not change what the next reader sees: {first} then {second}"
    );
}

#[tokio::test]
async fn every_documented_data_frequency_is_stored_verbatim() {
    let router = router();

    for (index, frequency) in DATA_FREQUENCIES.iter().enumerate() {
        let name = format!("frequency_{index}");
        let body = json!({
            "DatasetName": &name,
            "Domain": "CUSTOM",
            "DatasetType": "TARGET_TIME_SERIES",
            "DataFrequency": frequency,
            "Schema": schema_json(RETAIL_TARGET),
        });
        let (status, created) = call(&router, "CreateDataset", body).await;
        assert_eq!(
            status, 200,
            "case DataFrequency={frequency}: expected CreateDataset 200, got {status} with {created}"
        );
        let arn = created["DatasetArn"].as_str().unwrap_or_default().to_string();
        let (status, got) = call(&router, "DescribeDataset", json!({"DatasetArn": arn})).await;
        assert_eq!(
            status, 200,
            "case DataFrequency={frequency}: expected DescribeDataset 200, got {status} with {got}"
        );
        assert_eq!(
            got["DataFrequency"].as_str(),
            Some(*frequency),
            "case DataFrequency={frequency}: expected it back unchanged, got {got}"
        );
    }
}

#[tokio::test]
async fn a_related_time_series_dataset_without_a_data_frequency_is_refused() {
    let router = router();

    for (index, domain) in DOMAINS.iter().enumerate() {
        for dataset_type in DATASET_TYPES {
            let name = format!("freq_rule_{index}_{dataset_type}");
            let body = json!({
                "DatasetName": &name,
                "Domain": domain,
                "DatasetType": dataset_type,
                "Schema": schema_json(RETAIL_TARGET),
            });
            let (status, got) = call(&router, "CreateDataset", body).await;
            if *dataset_type == "RELATED_TIME_SERIES" {
                assert_eq!(
                    status, 400,
                    "case {domain}/{dataset_type} without DataFrequency: expected 400, got {status} with {got}"
                );
                assert_eq!(
                    error_code(&got), "InvalidInputException",
                    "case {domain}/{dataset_type} without DataFrequency: expected InvalidInputException, got {got}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {domain}/{dataset_type} without DataFrequency: expected 200, got {status} with {got}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_second_create_with_a_taken_name_is_refused_and_changes_nothing() {
    let router = router();

    for case in DATASET_TABLE {
        let arn = create_dataset(&router, case).await;

        // Same name, deliberately different everything else: the refusal is about the
        // name, and the stored record must survive it untouched.
        let clashing = DatasetCase {
            domain: if case.domain == "CUSTOM" { "RETAIL" } else { "CUSTOM" },
            dataset_type: "ITEM_METADATA",
            data_frequency: None,
            attributes: ITEM_METADATA,
            encrypted: false,
            tags: &[],
            ..case.clone()
        };
        let (status, got) = call(&router, "CreateDataset", create_dataset_body(&clashing)).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected a repeated CreateDataset to answer 403, got {status} with {got}"
        );
        assert_eq!(
            error_code(&got),
            "ResourceAlreadyExistsException",
            "case {case:?}: expected ResourceAlreadyExistsException, got {got}"
        );

        let (_, described) = call(&router, "DescribeDataset", json!({"DatasetArn": arn})).await;
        assert_eq!(
            described["Domain"].as_str(),
            Some(case.domain),
            "case {case:?}: a refused create must leave the stored record alone, got {described}"
        );
    }

    // The same rule on dataset groups.
    let member = create_dataset(&router, &plain_case("group_rule_ds", "RETAIL")).await;
    create_group(&router, "clash_group", "RETAIL", &[member]).await;
    let (status, got) = call(
        &router,
        "CreateDatasetGroup",
        json!({"DatasetGroupName": "clash_group", "Domain": "CUSTOM"}),
    )
    .await;
    assert_eq!(
        status, 403,
        "case group clash_group: expected a repeated CreateDatasetGroup to answer 403, got {status} with {got}"
    );
    assert_eq!(
        error_code(&got),
        "ResourceAlreadyExistsException",
        "case group clash_group: expected ResourceAlreadyExistsException, got {got}"
    );
}

#[tokio::test]
async fn an_arn_with_no_record_is_resource_not_found() {
    let router = router();

    // Well-formed ARNs (the model's pattern is satisfied) that name nothing in state.
    let dataset_arns = [
        expected_dataset_arn("never_created"),
        format!(
            "arn:aws:forecast:eu-west-1:{}:dataset/other_region",
            engine::EMULATOR_ACCOUNT_ID
        ),
        "arn:aws-cn:forecast:cn-north-1:000000000000:dataset/other_partition".to_string(),
    ];
    for arn in &dataset_arns {
        for operation in ["DescribeDataset", "DeleteDataset"] {
            let (status, got) = call(&router, operation, json!({"DatasetArn": arn})).await;
            assert_eq!(
                status, 404,
                "case {operation}({arn}): expected 404, got {status} with {got}"
            );
            assert_eq!(
                error_code(&got),
                "ResourceNotFoundException",
                "case {operation}({arn}): expected ResourceNotFoundException, got {got}"
            );
        }
    }

    let group_arns = [
        expected_group_arn("never_created"),
        format!(
            "arn:aws:forecast:eu-west-1:{}:dataset-group/other_region",
            engine::EMULATOR_ACCOUNT_ID
        ),
    ];
    for arn in &group_arns {
        for operation in ["DescribeDatasetGroup", "DeleteDatasetGroup"] {
            let (status, got) = call(&router, operation, json!({"DatasetGroupArn": arn})).await;
            assert_eq!(
                status, 404,
                "case {operation}({arn}): expected 404, got {status} with {got}"
            );
            assert_eq!(
                error_code(&got),
                "ResourceNotFoundException",
                "case {operation}({arn}): expected ResourceNotFoundException, got {got}"
            );
        }
        let (status, got) = call(
            &router,
            "UpdateDatasetGroup",
            json!({"DatasetGroupArn": arn, "DatasetArns": []}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case UpdateDatasetGroup({arn}): expected 404, got {status} with {got}"
        );
        assert_eq!(
            error_code(&got),
            "ResourceNotFoundException",
            "case UpdateDatasetGroup({arn}): expected ResourceNotFoundException, got {got}"
        );
    }
}

#[tokio::test]
async fn update_replaces_the_whole_membership_and_keeps_creation_time() {
    let router = router();
    let first = create_dataset(&router, &plain_case("member_one", "RETAIL")).await;
    let second = create_dataset(&router, &plain_case("member_two", "RETAIL")).await;
    let third = create_dataset(&router, &plain_case("member_three", "RETAIL")).await;

    let group = create_group(&router, "membership", "RETAIL", std::slice::from_ref(&first)).await;
    assert_eq!(
        group,
        expected_group_arn("membership"),
        "case group membership: expected the minted DatasetGroupArn to name the group, got {group}"
    );

    let (_, initial) = call(
        &router,
        "DescribeDatasetGroup",
        json!({"DatasetGroupArn": &group}),
    )
    .await;
    let created_at = initial["CreationTime"].as_f64();
    assert_eq!(
        initial["DatasetArns"],
        json!([first.clone()]),
        "case group membership: expected the created membership back, got {initial}"
    );
    assert_eq!(
        initial["Domain"].as_str(),
        Some("RETAIL"),
        "case group membership: expected Domain RETAIL, got {initial}"
    );
    assert_eq!(
        initial["Status"].as_str(),
        Some("ACTIVE"),
        "case group membership: expected Status ACTIVE, got {initial}"
    );

    // Each row REPLACES the membership: a longer list, a reordered one, a shorter one,
    // and the empty one.
    let memberships: &[Vec<String>] = &[
        vec![second.clone(), third.clone()],
        vec![third.clone(), second.clone(), first.clone()],
        vec![],
        vec![first.clone()],
    ];
    for members in memberships {
        let (status, got) = call(
            &router,
            "UpdateDatasetGroup",
            json!({"DatasetGroupArn": &group, "DatasetArns": members}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case membership {members:?}: expected UpdateDatasetGroup 200, got {status} with {got}"
        );

        let (status, described) = call(
            &router,
            "DescribeDatasetGroup",
            json!({"DatasetGroupArn": &group}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case membership {members:?}: expected DescribeDatasetGroup 200, got {status} with {described}"
        );
        assert_eq!(
            described["DatasetArns"],
            json!(members),
            "case membership {members:?}: expected exactly the membership last written, got {described}"
        );
        assert_eq!(
            described["CreationTime"].as_f64(),
            created_at,
            "case membership {members:?}: an update must not move CreationTime, got {described}"
        );
        let modified = described["LastModificationTime"].as_f64();
        assert!(
            modified >= created_at,
            "case membership {members:?}: expected LastModificationTime at or after CreationTime {created_at:?}, got {modified:?}"
        );
    }
}

#[tokio::test]
async fn a_group_member_must_exist_and_share_the_group_domain() {
    let router = router();
    let retail = create_dataset(&router, &plain_case("retail_member", "RETAIL")).await;
    let custom = create_dataset(&router, &plain_case("custom_member", "CUSTOM")).await;
    let missing = expected_dataset_arn("never_created");

    // CreateDatasetGroup refuses an absent member…
    let (status, got) = call(
        &router,
        "CreateDatasetGroup",
        json!({"DatasetGroupName": "bad_members", "Domain": "RETAIL", "DatasetArns": [retail.clone(), missing.clone()]}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case create group with an absent member: expected 404, got {status} with {got}"
    );
    assert_eq!(
        error_code(&got),
        "ResourceNotFoundException",
        "case create group with an absent member: expected ResourceNotFoundException, got {got}"
    );

    // …and a member whose Domain differs from the group's.
    let (status, got) = call(
        &router,
        "CreateDatasetGroup",
        json!({"DatasetGroupName": "bad_domain", "Domain": "RETAIL", "DatasetArns": [custom.clone()]}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case create group with a mismatched domain: expected 400, got {status} with {got}"
    );
    assert_eq!(
        error_code(&got),
        "InvalidInputException",
        "case create group with a mismatched domain: expected InvalidInputException, got {got}"
    );

    // Neither refusal left a group behind.
    let groups = collect_listing(
        &router,
        "ListDatasetGroups",
        "DatasetGroups",
        "DatasetGroupArn",
        None,
    )
    .await;
    assert!(
        groups.is_empty(),
        "case refused creates: expected no dataset group in state, got {groups:?}"
    );

    // UpdateDatasetGroup applies the same two rules, and a refused update is atomic.
    let group = create_group(&router, "good_group", "RETAIL", std::slice::from_ref(&retail)).await;
    for (label, members, expected_status, expected_code) in [
        ("absent member", vec![missing.clone()], 404, "ResourceNotFoundException"),
        ("mismatched domain", vec![custom.clone()], 400, "InvalidInputException"),
        (
            "one good and one absent",
            vec![retail.clone(), missing.clone()],
            404,
            "ResourceNotFoundException",
        ),
    ] {
        let (status, got) = call(
            &router,
            "UpdateDatasetGroup",
            json!({"DatasetGroupArn": &group, "DatasetArns": members}),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case update with {label}: expected {expected_status}, got {status} with {got}"
        );
        assert_eq!(
            error_code(&got), expected_code,
            "case update with {label}: expected {expected_code}, got {got}"
        );
        let (_, described) = call(
            &router,
            "DescribeDatasetGroup",
            json!({"DatasetGroupArn": &group}),
        )
        .await;
        assert_eq!(
            described["DatasetArns"],
            json!([retail.clone()]),
            "case update with {label}: a refused update must leave the membership alone, got {described}"
        );
    }
}

#[tokio::test]
async fn a_listing_enumerates_state_and_pages_by_max_results() {
    for total in [0usize, 1, 2, 5, 7] {
        for max_results in [None, Some(1), Some(2), Some(3), Some(100)] {
            let router = router();
            let mut created = Vec::new();
            for index in 0..total {
                let name = format!("paged_{index}");
                let body = json!({
                    "DatasetName": &name,
                    "Domain": DOMAINS[index % DOMAINS.len()],
                    "DatasetType": "TARGET_TIME_SERIES",
                    "DataFrequency": DATA_FREQUENCIES[index % DATA_FREQUENCIES.len()],
                    "Schema": schema_json(RETAIL_TARGET),
                });
                let (status, got) = call(&router, "CreateDataset", body).await;
                assert_eq!(
                    status, 200,
                    "case total={total} max_results={max_results:?}: expected CreateDataset 200 for {name}, got {status} with {got}"
                );
                created.push(got["DatasetArn"].as_str().unwrap_or_default().to_string());
            }
            created.sort();

            let seen = collect_listing(
                &router,
                "ListDatasets",
                "Datasets",
                "DatasetArn",
                max_results,
            )
            .await;
            assert_eq!(
                seen, created,
                "case total={total} max_results={max_results:?}: expected the listing to enumerate exactly what is in state"
            );

            // The same rule on dataset groups, over the same page sizes.
            let mut groups = Vec::new();
            for index in 0..total {
                let name = format!("paged_group_{index}");
                let (status, got) = call(
                    &router,
                    "CreateDatasetGroup",
                    json!({"DatasetGroupName": &name, "Domain": DOMAINS[index % DOMAINS.len()]}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case total={total} max_results={max_results:?}: expected CreateDatasetGroup 200 for {name}, got {status} with {got}"
                );
                groups.push(
                    got["DatasetGroupArn"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            groups.sort();

            let seen = collect_listing(
                &router,
                "ListDatasetGroups",
                "DatasetGroups",
                "DatasetGroupArn",
                max_results,
            )
            .await;
            assert_eq!(
                seen, groups,
                "case total={total} max_results={max_results:?}: expected the group listing to enumerate exactly what is in state"
            );
        }
    }
}

#[tokio::test]
async fn a_listing_summary_reports_the_stored_record() {
    let router = router();
    for case in DATASET_TABLE {
        create_dataset(&router, case).await;
    }

    let (status, listed) = call(&router, "ListDatasets", json!({})).await;
    assert_eq!(
        status, 200,
        "case summaries: expected ListDatasets 200, got {status} with {listed}"
    );
    let summaries = listed["Datasets"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        summaries.len(),
        DATASET_TABLE.len(),
        "case summaries: expected one summary per stored dataset, got {listed}"
    );

    for case in DATASET_TABLE {
        let arn = expected_dataset_arn(case.name);
        let summary = summaries
            .iter()
            .find(|entry| entry["DatasetArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: expected a summary for {arn}, got {listed}"));
        assert_eq!(
            summary["DatasetName"].as_str(),
            Some(case.name),
            "case {case:?}: expected DatasetName {}, got {summary}",
            case.name
        );
        assert_eq!(
            summary["Domain"].as_str(),
            Some(case.domain),
            "case {case:?}: expected Domain {}, got {summary}",
            case.domain
        );
        assert_eq!(
            summary["DatasetType"].as_str(),
            Some(case.dataset_type),
            "case {case:?}: expected DatasetType {}, got {summary}",
            case.dataset_type
        );
        let (_, described) = call(&router, "DescribeDataset", json!({"DatasetArn": &arn})).await;
        assert_eq!(
            summary["CreationTime"].as_f64(),
            described["CreationTime"].as_f64(),
            "case {case:?}: the summary and the describe must report one stored CreationTime, got {summary} and {described}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_listing_did_not_mint_is_refused() {
    let router = router();
    for index in 0..3 {
        let name = format!("token_ds_{index}");
        let (status, _) = call(
            &router,
            "CreateDataset",
            json!({
                "DatasetName": &name,
                "Domain": "CUSTOM",
                "DatasetType": "TARGET_TIME_SERIES",
                "DataFrequency": "D",
                "Schema": schema_json(RETAIL_TARGET),
            }),
        )
        .await;
        assert_eq!(status, 200, "case token setup: expected 200 for {name}");
        let (status, _) = call(
            &router,
            "CreateDatasetGroup",
            json!({"DatasetGroupName": format!("token_group_{index}"), "Domain": "CUSTOM"}),
        )
        .await;
        assert_eq!(status, 200, "case token setup: expected 200 for group {index}");
    }

    // A real token from each listing, so the cross-listing cases below use tokens that
    // ARE valid — for the other listing.
    let (_, datasets) = call(&router, "ListDatasets", json!({"MaxResults": 1})).await;
    let dataset_token = datasets["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a NextToken on a truncated page, got {datasets}"))
        .to_string();
    let (_, groups) = call(&router, "ListDatasetGroups", json!({"MaxResults": 1})).await;
    let group_token = groups["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a NextToken on a truncated page, got {groups}"))
        .to_string();

    let bad_tokens: &[(&str, String)] = &[
        ("plain garbage", "not-a-token".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex of foreign text", hex::encode("some other cursor")),
        ("a raw state key", expected_dataset_arn("token_ds_0")),
        ("the other listing's token", group_token.clone()),
    ];
    for (label, token) in bad_tokens {
        let (status, got) = call(&router, "ListDatasets", json!({"NextToken": token})).await;
        assert_eq!(
            status, 400,
            "case ListDatasets with {label}: expected 400, got {status} with {got}"
        );
        assert_eq!(
            error_code(&got),
            "InvalidNextTokenException",
            "case ListDatasets with {label}: expected InvalidNextTokenException, got {got}"
        );
    }

    let bad_group_tokens: &[(&str, String)] = &[
        ("plain garbage", "not-a-token".to_string()),
        ("the other listing's token", dataset_token.clone()),
    ];
    for (label, token) in bad_group_tokens {
        let (status, got) = call(&router, "ListDatasetGroups", json!({"NextToken": token})).await;
        assert_eq!(
            status, 400,
            "case ListDatasetGroups with {label}: expected 400, got {status} with {got}"
        );
        assert_eq!(
            error_code(&got),
            "InvalidNextTokenException",
            "case ListDatasetGroups with {label}: expected InvalidNextTokenException, got {got}"
        );
    }

    // And each listing's OWN token still resumes.
    let (status, resumed) = call(
        &router,
        "ListDatasets",
        json!({"MaxResults": 1, "NextToken": dataset_token}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case ListDatasets with its own token: expected 200, got {status} with {resumed}"
    );
    assert_eq!(
        resumed["Datasets"][0]["DatasetArn"].as_str(),
        Some(expected_dataset_arn("token_ds_1").as_str()),
        "case ListDatasets with its own token: expected the page to resume after the first dataset, got {resumed}"
    );
}

#[tokio::test]
async fn deleting_a_dataset_a_group_lists_leaves_the_membership_stale() {
    let router = router();

    // Several memberships, so the rule is not read off one group holding one dataset.
    let first = create_dataset(&router, &plain_case("held_one", "RETAIL")).await;
    let second = create_dataset(&router, &plain_case("held_two", "RETAIL")).await;
    let free = create_dataset(&router, &plain_case("held_none", "RETAIL")).await;
    let group = create_group(&router, "holder", "RETAIL", &[first.clone(), second.clone()]).await;

    // "Forecast does not automatically update any dataset groups that contain the deleted
    // dataset. In order to update the dataset group, use the UpdateDatasetGroup operation,
    // omitting the deleted dataset's ARN." (DeleteDataset's modelled documentation.) So a
    // dataset a group still lists IS deleted, and the group keeps reporting the stale ARN.
    for held in [&first, &second] {
        let (status, got) = call(&router, "DeleteDataset", json!({"DatasetArn": held})).await;
        assert_eq!(
            status, 200,
            "case delete held {held}: expected 200, got {status} with {got}"
        );
        let (status, _) = call(&router, "DescribeDataset", json!({"DatasetArn": held})).await;
        assert_eq!(
            status, 404,
            "case delete held {held}: expected a later DescribeDataset to answer 404, got {status}"
        );
        let (_, described) = call(
            &router,
            "DescribeDatasetGroup",
            json!({"DatasetGroupArn": &group}),
        )
        .await;
        assert_eq!(
            described["DatasetArns"],
            json!([first.clone(), second.clone()]),
            "case delete held {held}: the group must still report the membership it was last written with, got {described}"
        );
    }

    // A dataset no group ever listed deletes the same way, and the listing drops every
    // deleted dataset.
    let (status, got) = call(&router, "DeleteDataset", json!({"DatasetArn": &free})).await;
    assert_eq!(
        status, 200,
        "case delete unheld {free}: expected 200, got {status} with {got}"
    );
    let listed = collect_listing(&router, "ListDatasets", "Datasets", "DatasetArn", None).await;
    assert!(
        listed.is_empty(),
        "case delete all: expected the listing to drop every deleted dataset, got {listed:?}"
    );

    // The documented remedy: an update that still names a deleted ARN is refused, and one
    // that omits it succeeds and clears the stale membership.
    let (status, got) = call(
        &router,
        "UpdateDatasetGroup",
        json!({"DatasetGroupArn": &group, "DatasetArns": [first.clone()]}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case update naming a deleted dataset: expected 404, got {status} with {got}"
    );
    assert_eq!(
        error_code(&got),
        "ResourceNotFoundException",
        "case update naming a deleted dataset: expected ResourceNotFoundException, got {got}"
    );
    let (status, _) = call(
        &router,
        "UpdateDatasetGroup",
        json!({"DatasetGroupArn": &group, "DatasetArns": []}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case update omitting the deleted datasets: expected 200, got {status}"
    );
    let (_, described) = call(
        &router,
        "DescribeDatasetGroup",
        json!({"DatasetGroupArn": &group}),
    )
    .await;
    assert_eq!(
        described["DatasetArns"],
        json!([]),
        "case update omitting the deleted datasets: expected an empty membership, got {described}"
    );

    // Deleting an already-deleted dataset is still 404.
    for gone in [&first, &second, &free] {
        let (status, got) = call(&router, "DeleteDataset", json!({"DatasetArn": gone})).await;
        assert_eq!(
            status, 404,
            "case delete twice {gone}: expected 404, got {status} with {got}"
        );
        assert_eq!(
            error_code(&got),
            "ResourceNotFoundException",
            "case delete twice {gone}: expected ResourceNotFoundException, got {got}"
        );
    }
}

#[tokio::test]
async fn the_same_name_in_another_region_is_another_resource() {
    let router = router();
    let name = "shared_name";
    let body = json!({
        "DatasetName": name,
        "Domain": "RETAIL",
        "DatasetType": "TARGET_TIME_SERIES",
        "DataFrequency": "D",
        "Schema": schema_json(RETAIL_TARGET),
    });

    // The ARN carries the credential-scope region, so the same name signed for another
    // region is another dataset — while a repeat in the SAME region is refused.
    let (status, created) = call(&router, "CreateDataset", body.clone()).await;
    assert_eq!(
        status, 200,
        "case {REGION}/{name}: expected 200, got {status} with {created}"
    );
    assert_eq!(
        created["DatasetArn"].as_str(),
        Some(expected_dataset_arn(name).as_str()),
        "case {REGION}/{name}: expected the ARN to carry the signing region, got {created}"
    );

    for other_region in ["eu-west-1", "ap-southeast-2"] {
        let (status, created) =
            call_signed_for(&router, other_region, "CreateDataset", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {other_region}/{name}: expected 200, got {status} with {created}"
        );
        let expected = format!(
            "arn:aws:forecast:{other_region}:{}:dataset/{name}",
            engine::EMULATOR_ACCOUNT_ID
        );
        assert_eq!(
            created["DatasetArn"].as_str(),
            Some(expected.as_str()),
            "case {other_region}/{name}: expected a distinct regional ARN, got {created}"
        );

        let (status, got) = call_signed_for(
            &router,
            other_region,
            "DescribeDataset",
            json!({"DatasetArn": &expected}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {other_region}/{name}: expected DescribeDataset 200, got {status} with {got}"
        );

        // And the repeat in that region is the one that clashes.
        let (status, got) =
            call_signed_for(&router, other_region, "CreateDataset", body.clone()).await;
        assert_eq!(
            status, 403,
            "case {other_region}/{name} repeated: expected 403, got {status} with {got}"
        );
        assert_eq!(
            error_code(&got),
            "ResourceAlreadyExistsException",
            "case {other_region}/{name} repeated: expected ResourceAlreadyExistsException, got {got}"
        );
    }
}

#[tokio::test]
async fn deleting_a_group_removes_the_group_and_keeps_its_datasets() {
    let router = router();
    let member = create_dataset(&router, &plain_case("kept_dataset", "RETAIL")).await;
    let keep = create_dataset(&router, &plain_case("other_dataset", "RETAIL")).await;
    let group = create_group(&router, "doomed", "RETAIL", std::slice::from_ref(&member)).await;
    let other = create_group(&router, "survivor", "RETAIL", std::slice::from_ref(&keep)).await;

    let (status, got) = call(
        &router,
        "DeleteDatasetGroup",
        json!({"DatasetGroupArn": &group}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete group {group}: expected 200, got {status} with {got}"
    );
    let (status, _) = call(
        &router,
        "DescribeDatasetGroup",
        json!({"DatasetGroupArn": &group}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case delete group {group}: expected a later DescribeDatasetGroup to answer 404, got {status}"
    );
    let groups = collect_listing(
        &router,
        "ListDatasetGroups",
        "DatasetGroups",
        "DatasetGroupArn",
        None,
    )
    .await;
    assert_eq!(
        groups,
        vec![other.clone()],
        "case delete group {group}: expected only the surviving group, got {groups:?}"
    );

    // Deleting a group deletes only the grouping: both datasets are still there.
    let datasets = collect_listing(&router, "ListDatasets", "Datasets", "DatasetArn", None).await;
    let mut expected = vec![member.clone(), keep.clone()];
    expected.sort();
    assert_eq!(
        datasets, expected,
        "case delete group {group}: deleting a group must not delete its datasets, got {datasets:?}"
    );
    for dataset in [&member, &keep] {
        let (status, got) = call(&router, "DeleteDataset", json!({"DatasetArn": dataset})).await;
        assert_eq!(
            status, 200,
            "case delete {dataset} after its group: expected 200, got {status} with {got}"
        );
    }
    let (_, described) = call(
        &router,
        "DescribeDatasetGroup",
        json!({"DatasetGroupArn": &other}),
    )
    .await;
    assert_eq!(
        described["DatasetArns"],
        json!([keep.clone()]),
        "case surviving group {other}: expected it to keep reporting its written membership, got {described}"
    );
}

/// One row of the retained-state ceiling table: a store, the operation that grows it and
/// the ceiling it is bounded by.
#[derive(Debug, Clone, Copy)]
struct CeilingCase {
    operation: &'static str,
    ceiling: usize,
    /// The `items` member of the listing that enumerates the store.
    items_member: &'static str,
    arn_member: &'static str,
    list_operation: &'static str,
}

/// Both stores this service keeps, each with the ceiling `src/state.rs` declares for it.
const CEILING_TABLE: &[CeilingCase] = &[
    CeilingCase {
        operation: "CreateDataset",
        ceiling: service_com_amazonaws_forecast_amazonforecast::state::MAX_DATASETS,
        items_member: "Datasets",
        arn_member: "DatasetArn",
        list_operation: "ListDatasets",
    },
    CeilingCase {
        operation: "CreateDatasetGroup",
        ceiling: service_com_amazonaws_forecast_amazonforecast::state::MAX_DATASET_GROUPS,
        items_member: "DatasetGroups",
        arn_member: "DatasetGroupArn",
        list_operation: "ListDatasetGroups",
    },
];

/// A create body for row `index` of a ceiling case.
fn ceiling_create_body(case: &CeilingCase, index: usize) -> Value {
    if case.operation == "CreateDataset" {
        json!({
            "DatasetName": format!("bulk_{index}"),
            "Domain": "RETAIL",
            "DatasetType": "TARGET_TIME_SERIES",
            "Schema": schema_json(RETAIL_TARGET),
        })
    } else {
        json!({
            "DatasetGroupName": format!("bulk_{index}"),
            "Domain": "RETAIL",
        })
    }
}

/// Retained state is BOUNDED: each store accepts creates up to its ceiling and then
/// refuses with the modelled `LimitExceededException` (HTTP 409) instead of allocating,
/// and the refused record is not in the store afterwards. Asserted on BOTH stores, each
/// with its own (different) ceiling, so the rule is not a property of one map.
#[tokio::test]
async fn a_store_refuses_the_create_that_would_cross_its_ceiling() {
    for case in CEILING_TABLE {
        let router = router();
        for index in 0..case.ceiling {
            let (status, body) =
                call(&router, case.operation, ceiling_create_body(case, index)).await;
            assert_eq!(
                status, 200,
                "case {case:?} entry {index}: expected 200 below the ceiling, got {status} with {body}"
            );
        }

        // The create that would cross the ceiling.
        let (status, body) = call(
            &router,
            case.operation,
            ceiling_create_body(case, case.ceiling),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409 at the ceiling, got {status} with {body}"
        );
        let got = error_code(&body);
        assert!(
            got.ends_with("LimitExceededException"),
            "case {case:?}: expected LimitExceededException at the ceiling, got {got}"
        );

        // Nothing was stored for the refused call: the store still holds exactly the
        // entries it accepted.
        let stored = collect_listing(
            &router,
            case.list_operation,
            case.items_member,
            case.arn_member,
            Some(100),
        )
        .await;
        assert_eq!(
            stored.len(),
            case.ceiling,
            "case {case:?}: expected the store to hold exactly its ceiling after the refusal, got {}",
            stored.len()
        );
    }
}

/// A group's membership is bounded too: `ArnList` carries no modelled `@length`, so both
/// operations that accept one refuse a list longer than
/// `MAX_DATASET_ARNS_PER_GROUP` — `CreateDatasetGroup` with the modelled
/// `LimitExceededException` (409), `UpdateDatasetGroup`, which models no limit error,
/// with the modelled `InvalidInputException` (400) — and the group keeps the membership
/// it had. The table spans both entry points and both sides of the bound.
#[tokio::test]
async fn a_membership_longer_than_the_ceiling_is_refused_at_both_entry_points() {
    const CEILING: usize = service_com_amazonaws_forecast_amazonforecast::state::MAX_DATASET_ARNS_PER_GROUP;

    let router = router();
    // One real dataset, repeated: the membership length is what is under test, and a
    // repeated ARN is as long a list as a distinct one.
    let member = create_dataset(&router, &plain_case("bounded_member", "RETAIL")).await;
    let at_ceiling: Vec<String> = std::iter::repeat_n(member.clone(), CEILING).collect();
    let past_ceiling: Vec<String> = std::iter::repeat_n(member.clone(), CEILING + 1).collect();

    // At the ceiling: accepted, and stored verbatim.
    let group = create_group(&router, "bounded_group", "RETAIL", &at_ceiling).await;
    let (_, described) = call(
        &router,
        "DescribeDatasetGroup",
        json!({"DatasetGroupArn": &group}),
    )
    .await;
    assert_eq!(
        described["DatasetArns"].as_array().map(Vec::len),
        Some(CEILING),
        "case membership at the ceiling ({CEILING}): expected it stored verbatim, got {described}"
    );

    // Past the ceiling, on CreateDatasetGroup.
    let (status, body) = call(
        &router,
        "CreateDatasetGroup",
        json!({
            "DatasetGroupName": "too_many_members",
            "Domain": "RETAIL",
            "DatasetArns": past_ceiling.clone(),
        }),
    )
    .await;
    assert_eq!(
        status, 409,
        "case CreateDatasetGroup with {} members: expected HTTP 409, got {status} with {body}",
        past_ceiling.len()
    );
    let got = error_code(&body);
    assert!(
        got.ends_with("LimitExceededException"),
        "case CreateDatasetGroup with {} members: expected LimitExceededException, got {got}",
        past_ceiling.len()
    );

    // Past the ceiling, on UpdateDatasetGroup.
    let (status, body) = call(
        &router,
        "UpdateDatasetGroup",
        json!({"DatasetGroupArn": &group, "DatasetArns": past_ceiling.clone()}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case UpdateDatasetGroup with {} members: expected HTTP 400, got {status} with {body}",
        past_ceiling.len()
    );
    let got = error_code(&body);
    assert!(
        got.ends_with("InvalidInputException"),
        "case UpdateDatasetGroup with {} members: expected InvalidInputException, got {got}",
        past_ceiling.len()
    );

    // The refused update changed nothing: the group still reports the accepted list.
    let (_, described) = call(
        &router,
        "DescribeDatasetGroup",
        json!({"DatasetGroupArn": &group}),
    )
    .await;
    assert_eq!(
        described["DatasetArns"].as_array().map(Vec::len),
        Some(CEILING),
        "case refused update: expected the membership unchanged at {CEILING}, got {described}"
    );

    // And the group the refused create named was never stored.
    let groups = collect_listing(
        &router,
        "ListDatasetGroups",
        "DatasetGroups",
        "DatasetGroupArn",
        None,
    )
    .await;
    assert_eq!(
        groups,
        vec![group.clone()],
        "case refused create: expected only the accepted group in state, got {groups:?}"
    );
}
