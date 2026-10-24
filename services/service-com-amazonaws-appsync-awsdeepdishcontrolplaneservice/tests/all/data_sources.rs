// SPDX-License-Identifier: CC0-1.0

//! Data sources, end to end over HTTP: `CreateDataSource` and the `GetDataSource`
//! that reads one back.
//!
//! The rules asserted here are stated over CLASSES of input rather than over the one
//! request that exposed the defect:
//!
//! 1. creating a data source persists it under its GraphQL API and answers the stored
//!    record — its ARN named under the API's own path, its `type`, its per-type
//!    configuration and its `metricsConfig` as the request set them, and a
//!    `description` only where the request carried a non-empty one (an empty
//!    description describes nothing, so the member is reported absent, exactly as
//!    for a request that omitted it) — and `GetDataSource` answers the same body.
//!    Asserted over a table holding every `DataSourceType` the model declares, with
//!    the description and `metricsConfig` states rotating across it;
//! 2. a data source of a type that is set up through a configuration member is
//!    refused without that member (400 `BadRequestException`), while a type
//!    configured through none is created without any;
//! 3. the name identifies the data source within its API: a second create under the
//!    same name is refused and leaves the stored one untouched, while the same name
//!    under a different API is a different data source;
//! 4. an API holds at most the AppSync quota of data sources: a create past it is
//!    refused and persists nothing, so repeating the call cannot grow the store
//!    without limit;
//! 5. an `apiId` that cannot name an API is a 400 `BadRequestException`, one that
//!    names no API this account holds a 404 `NotFoundException`, and a name the API
//!    does not hold a 404.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

// The router, the region it signs for and the API-creating helper are the crate's
// shared test harness, reused rather than copied.
use super::graphql_api_lifecycle::{create_api as create_graphql_api, router, REGION};

/// The account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// One answer from the emulator: the parts a client branches on.
#[derive(Debug, Clone)]
struct Answer {
    status: u16,
    /// The `x-amzn-errortype` header restJson1 resolves the error shape from.
    error_type: String,
    body: Value,
}

/// Send one request and return what the emulator answered.
///
/// The suite reads the error-type header, which the shared helper does not carry, so
/// it sends its own requests (as the association suite does).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://appsync.{REGION}.amazonaws.com{path}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/appsync/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// Create one GraphQL API through the shared helper and return its `apiId`.
async fn create_api(router: &MultiServiceRouter, name: &str) -> String {
    let api = create_graphql_api(
        router,
        json!({
            "name": name,
            "authenticationType": "API_KEY",
            "queryDepthLimit": 0,
            "resolverCountLimit": 0,
            "xrayEnabled": false,
        }),
    )
    .await;
    api["apiId"].as_str().unwrap_or_default().to_string()
}

/// Every `DataSourceType` the model declares, with the configuration member a data
/// source of that type is set up through and a minimal value for it.
///
/// Source: `smithy_query` shapes `DataSourceType` (the nine values), the per-type
/// configuration members of `CreateDataSourceRequest`, and the members each
/// configuration shape requires. `NONE` and `AMAZON_BEDROCK_RUNTIME` are configured
/// through no member of the request, so they carry none here.
fn type_catalog() -> Vec<(&'static str, Option<(&'static str, Value)>)> {
    vec![
        (
            "AMAZON_DYNAMODB",
            Some((
                "dynamodbConfig",
                json!({"tableName": "orders", "awsRegion": REGION}),
            )),
        ),
        (
            "AWS_LAMBDA",
            Some((
                "lambdaConfig",
                json!({
                    "lambdaFunctionArn":
                        format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:resolve"),
                }),
            )),
        ),
        (
            "AMAZON_ELASTICSEARCH",
            Some((
                "elasticsearchConfig",
                json!({"endpoint": "https://search.example.com", "awsRegion": REGION}),
            )),
        ),
        (
            "AMAZON_OPENSEARCH_SERVICE",
            Some((
                "openSearchServiceConfig",
                json!({"endpoint": "https://opensearch.example.com", "awsRegion": REGION}),
            )),
        ),
        (
            "HTTP",
            Some(("httpConfig", json!({"endpoint": "https://api.example.com"}))),
        ),
        (
            "RELATIONAL_DATABASE",
            Some((
                "relationalDatabaseConfig",
                json!({
                    "relationalDatabaseSourceType": "RDS_HTTP_ENDPOINT",
                    "rdsHttpEndpointConfig": {
                        "awsRegion": REGION,
                        "dbClusterIdentifier": "cluster",
                        "databaseName": "shop",
                        "schema": "public",
                        "awsSecretStoreArn":
                            format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:db"),
                    },
                }),
            )),
        ),
        (
            "AMAZON_EVENTBRIDGE",
            Some((
                "eventBridgeConfig",
                json!({"eventBusArn": format!("arn:aws:events:{REGION}:{ACCOUNT}:event-bus/bus")}),
            )),
        ),
        ("NONE", None),
        ("AMAZON_BEDROCK_RUNTIME", None),
    ]
}

/// The `description` states a create can leave behind: a description, an EMPTY
/// description, and no description at all.
const DESCRIPTIONS: &[Option<&str>] = &[Some("the data source"), Some(""), None];

/// The `metricsConfig` states a create can leave behind. Source: the
/// `DataSourceLevelMetricsConfig` enum (`ENABLED`, `DISABLED`), plus the request that
/// names none.
const METRICS_CONFIGS: &[Option<&str>] = &[Some("ENABLED"), Some("DISABLED"), None];

/// One row of the round-trip table: a data source to create.
#[derive(Debug, Clone)]
struct Row {
    name: String,
    data_source_type: &'static str,
    config: Option<(&'static str, Value)>,
    description: Option<&'static str>,
    metrics_config: Option<&'static str>,
}

/// The round-trip table: every `DataSourceType`, with the `description` and
/// `metricsConfig` states rotating across the rows so each state is exercised on
/// several types.
fn rows() -> Vec<Row> {
    type_catalog()
        .into_iter()
        .enumerate()
        .map(|(index, (data_source_type, config))| Row {
            name: format!("ds_{index}"),
            data_source_type,
            config,
            description: DESCRIPTIONS[index % DESCRIPTIONS.len()],
            metrics_config: METRICS_CONFIGS[index % METRICS_CONFIGS.len()],
        })
        .collect()
}

/// `row` as a `CreateDataSource` request body.
fn request_body(row: &Row) -> Value {
    let mut body = json!({"name": row.name, "type": row.data_source_type});
    let object = body.as_object_mut().expect("the body is an object");
    if let Some(description) = row.description {
        object.insert("description".to_string(), json!(description));
    }
    if let Some(metrics_config) = row.metrics_config {
        object.insert("metricsConfig".to_string(), json!(metrics_config));
    }
    if let Some((member, value)) = row.config.clone() {
        object.insert(member.to_string(), value);
    }
    body
}

/// Assert `data_source` reports exactly what `row` asked for, under `api_id`.
fn assert_projects(row: &Row, api_id: &str, data_source: &Value, source: &str) {
    assert_eq!(
        data_source["name"],
        json!(row.name),
        "case {row:?} ({source}): expected name {:?}, got {:?}",
        row.name,
        data_source["name"]
    );
    assert_eq!(
        data_source["type"],
        json!(row.data_source_type),
        "case {row:?} ({source}): expected type {}, got {:?}",
        row.data_source_type,
        data_source["type"]
    );
    let expected_arn = format!(
        "arn:aws:appsync:{REGION}:{ACCOUNT}:{}",
        state::data_source_resource(api_id, &row.name)
    );
    assert_eq!(
        data_source["dataSourceArn"],
        json!(expected_arn),
        "case {row:?} ({source}): expected dataSourceArn {expected_arn}, got {:?}",
        data_source["dataSourceArn"]
    );
    // A description is reported only where the request carried a non-empty one: an
    // empty description describes nothing, so it is reported absent just like an
    // omitted one.
    match row.description.filter(|description| !description.is_empty()) {
        Some(description) => assert_eq!(
            data_source["description"],
            json!(description),
            "case {row:?} ({source}): expected description {description:?}, got {:?}",
            data_source["description"]
        ),
        None => assert!(
            data_source["description"].is_null(),
            "case {row:?} ({source}): expected no description, got {:?}",
            data_source["description"]
        ),
    }
    match row.metrics_config {
        Some(metrics_config) => assert_eq!(
            data_source["metricsConfig"],
            json!(metrics_config),
            "case {row:?} ({source}): expected metricsConfig {metrics_config}, got {:?}",
            data_source["metricsConfig"]
        ),
        None => assert!(
            data_source["metricsConfig"].is_null(),
            "case {row:?} ({source}): expected no metricsConfig, got {:?}",
            data_source["metricsConfig"]
        ),
    }
    // Every member of the per-type configuration the request carried is reported
    // with the value it was given.
    if let Some((member, value)) = row.config.as_ref() {
        for (key, sent) in value.as_object().expect("the config is an object") {
            assert_eq!(
                &data_source[member][key], sent,
                "case {row:?} ({source}): expected {member}.{key} {sent:?}, got {:?}",
                data_source[member][key]
            );
        }
    }
    // ... and no configuration the request did not carry is reported, so a type
    // configured through no member reports no configuration at all.
    let carried = row.config.as_ref().map(|(member, _)| *member);
    for member in type_catalog()
        .iter()
        .filter_map(|(_, config)| config.as_ref().map(|(member, _)| *member))
        .filter(|member| Some(*member) != carried)
    {
        assert!(
            data_source[member].is_null(),
            "case {row:?} ({source}): expected no {member}, got {:?}",
            data_source[member]
        );
    }
}

/// Create one data source from `row` under `api_id` and return the answer.
async fn create_data_source(router: &MultiServiceRouter, api_id: &str, row: &Row) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(request_body(row)),
    )
    .await
}

#[tokio::test]
async fn create_persists_every_data_source_type_and_get_reads_it_back() {
    let router = router();
    let api_id = create_api(&router, "data-source-round-trip").await;

    for row in rows() {
        let created = create_data_source(&router, &api_id, &row).await;
        assert_eq!(
            created.status, 200,
            "case {row:?}: expected CreateDataSource 200, got {} {:?}",
            created.status, created.body
        );
        assert_projects(&row, &api_id, &created.body["dataSource"], "create");

        let read = call(
            &router,
            "GET",
            &format!("/v1/apis/{api_id}/datasources/{}", row.name),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {row:?}: expected GetDataSource 200, got {} {:?}",
            read.status, read.body
        );
        assert_projects(&row, &api_id, &read.body["dataSource"], "get");
        // The read answers the SAME record the create persisted, member for member.
        assert_eq!(
            read.body["dataSource"], created.body["dataSource"],
            "case {row:?}: expected GetDataSource to answer the created data source {:?}, got {:?}",
            created.body["dataSource"], read.body["dataSource"]
        );
    }
}

#[tokio::test]
async fn a_type_configured_through_a_member_is_refused_without_it() {
    let router = router();
    let api_id = create_api(&router, "data-source-config-required").await;

    for row in rows() {
        // The same row, stripped of its per-type configuration.
        let bare = Row {
            name: format!("{}_bare", row.name),
            config: None,
            ..row.clone()
        };
        let answer = create_data_source(&router, &api_id, &bare).await;
        match row.config.as_ref() {
            // A type that names a backend must say where to find it.
            Some((member, _)) => {
                assert_eq!(
                    answer.status, 400,
                    "case {bare:?}: expected 400 without {member}, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_type, "BadRequestException",
                    "case {bare:?}: expected BadRequestException without {member}, got {:?} {:?}",
                    answer.error_type, answer.body
                );
                let read = call(
                    &router,
                    "GET",
                    &format!("/v1/apis/{api_id}/datasources/{}", bare.name),
                    None,
                )
                .await;
                assert_eq!(
                    read.status, 404,
                    "case {bare:?}: a refused create persists nothing, got {} {:?}",
                    read.status, read.body
                );
            }
            // A type configured through no member is complete without one.
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {bare:?}: expected 200 for a type needing no config, got {} {:?}",
                    answer.status, answer.body
                );
                assert_projects(&bare, &api_id, &answer.body["dataSource"], "create");
            }
        }
    }
}

#[tokio::test]
async fn a_name_the_api_already_holds_is_refused_and_the_stored_one_is_untouched() {
    let router = router();
    let api_id = create_api(&router, "data-source-name-taken").await;

    for row in rows() {
        let created = create_data_source(&router, &api_id, &row).await;
        assert_eq!(
            created.status, 200,
            "case {row:?}: expected CreateDataSource 200, got {} {:?}",
            created.status, created.body
        );

        // Same name, a different type and description: the second create must not
        // replace a resource the caller did not name for replacement.
        let clash = Row {
            data_source_type: "NONE",
            config: None,
            description: Some("the second create"),
            metrics_config: None,
            ..row.clone()
        };
        let answer = create_data_source(&router, &api_id, &clash).await;
        assert_eq!(
            answer.status, 400,
            "case {clash:?}: expected 400 for a name the API holds, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "BadRequestException",
            "case {clash:?}: expected BadRequestException, got {:?} {:?}",
            answer.error_type, answer.body
        );

        let read = call(
            &router,
            "GET",
            &format!("/v1/apis/{api_id}/datasources/{}", row.name),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {row:?}: expected GetDataSource 200, got {} {:?}",
            read.status, read.body
        );
        assert_projects(&row, &api_id, &read.body["dataSource"], "after the clash");
    }
}

#[tokio::test]
async fn a_data_source_belongs_to_the_api_it_was_created_under() {
    let router = router();
    let first = create_api(&router, "data-source-scope-first").await;
    let second = create_api(&router, "data-source-scope-second").await;

    for row in rows() {
        let created = create_data_source(&router, &first, &row).await;
        assert_eq!(
            created.status, 200,
            "case {row:?}: expected CreateDataSource 200, got {} {:?}",
            created.status, created.body
        );

        // The other API does not hold it, however many data sources it holds of its
        // own.
        let read = call(
            &router,
            "GET",
            &format!("/v1/apis/{second}/datasources/{}", row.name),
            None,
        )
        .await;
        assert_eq!(
            read.status, 404,
            "case {row:?}: another API does not hold it, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_type, "NotFoundException",
            "case {row:?}: expected NotFoundException, got {:?} {:?}",
            read.error_type, read.body
        );

        // The same name under the other API is a DIFFERENT data source, named under
        // that API's own path.
        let twin = create_data_source(&router, &second, &row).await;
        assert_eq!(
            twin.status, 200,
            "case {row:?}: expected the same name under another API to be created, got {} {:?}",
            twin.status, twin.body
        );
        assert_projects(&row, &second, &twin.body["dataSource"], "the other API");
        assert_ne!(
            twin.body["dataSource"]["dataSourceArn"], created.body["dataSource"]["dataSourceArn"],
            "case {row:?}: two APIs' data sources share an ARN: {:?}",
            twin.body["dataSource"]["dataSourceArn"]
        );
    }
}

#[tokio::test]
async fn an_api_holds_at_most_the_quota_of_data_sources() {
    /// The AppSync service quota for data sources on ONE GraphQL API, which
    /// `CreateDataSource` refuses a create past, so repeating the call cannot grow
    /// the store without limit.
    const QUOTA: usize = 500;

    let router = router();
    let api_id = create_api(&router, "data-source-quota").await;
    let template = rows().swap_remove(0);

    for index in 0..QUOTA {
        let row = Row {
            name: format!("ds_quota_{index}"),
            ..template.clone()
        };
        let answer = create_data_source(&router, &api_id, &row).await;
        assert_eq!(
            answer.status, 200,
            "case data source {index} of the quota: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }

    let past = Row {
        name: "ds_past_the_quota".to_string(),
        ..template
    };
    let answer = create_data_source(&router, &api_id, &past).await;
    assert_eq!(
        answer.status, 400,
        "case {past:?}: expected 400 past the quota of {QUOTA}, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type, "BadRequestException",
        "case {past:?}: expected BadRequestException past the quota, got {:?} {:?}",
        answer.error_type, answer.body
    );

    let read = call(
        &router,
        "GET",
        &format!("/v1/apis/{api_id}/datasources/{}", past.name),
        None,
    )
    .await;
    assert_eq!(
        read.status, 404,
        "case {past:?}: a create refused past the quota persists nothing, got {} {:?}",
        read.status, read.body
    );
}

#[tokio::test]
async fn an_unnameable_api_is_a_bad_request_and_an_absent_one_a_not_found() {
    let router = router();
    let api_id = create_api(&router, "data-source-refusals").await;
    let row = &rows()[0];

    // An `apiId` longer than an AppSync API id holds more characters than one has,
    // so it names no API at all.
    let unnameable = "z".repeat(state::API_ID_LEN + 1);
    // An id of the right shape this account simply does not hold.
    let absent = "z".repeat(state::API_ID_LEN);

    for (api, expected_status, expected_error) in [
        (unnameable.as_str(), 400, "BadRequestException"),
        (absent.as_str(), 404, "NotFoundException"),
    ] {
        let created = create_data_source(&router, api, row).await;
        assert_eq!(
            created.status, expected_status,
            "case create under {api:?}: expected {expected_status}, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.error_type, expected_error,
            "case create under {api:?}: expected {expected_error}, got {:?} {:?}",
            created.error_type, created.body
        );

        let read = call(
            &router,
            "GET",
            &format!("/v1/apis/{api}/datasources/{}", row.name),
            None,
        )
        .await;
        assert_eq!(
            read.status, expected_status,
            "case get under {api:?}: expected {expected_status}, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_type, expected_error,
            "case get under {api:?}: expected {expected_error}, got {:?} {:?}",
            read.error_type, read.body
        );
    }

    // A name the API does not hold is a not-found, not an empty answer.
    let read = call(
        &router,
        "GET",
        &format!("/v1/apis/{api_id}/datasources/never_created"),
        None,
    )
    .await;
    assert_eq!(
        read.status, 404,
        "case get an uncreated name: expected 404, got {} {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.error_type, "NotFoundException",
        "case get an uncreated name: expected NotFoundException, got {:?} {:?}",
        read.error_type, read.body
    );
}
