// SPDX-License-Identifier: CC0-1.0

//! The data source's service-role requirement, end to end over HTTP.
//!
//! `serviceRoleArn` is "the Identity and Access Management (IAM) service role Amazon
//! Resource Name (ARN) for the data source. The system assumes this role when
//! accessing the data source", and the model leaves it optional because some data
//! source types need no credentials at all. So the rule asserted here is:
//!
//! * a create (or update) of a data source whose `type` AppSync can only reach by
//!   assuming a role, with no `serviceRoleArn`, describes a data source AppSync could
//!   never invoke and is refused with `BadRequestException` (400) naming the absent
//!   member; the refusal writes nothing, so the name is absent from `ListDataSources`
//!   and `GetDataSource` for it is a 404. The same request WITH a role succeeds.
//! * the one type whose requirement depends on its configuration is HTTP: a plain
//!   endpoint needs no role, one whose `authorizationConfig` selects `AWS_IAM` — the
//!   only `AuthorizationType` value, i.e. "the authorization type is Signature
//!   Version 4" — does.
//!
//! The table spans every `DataSourceType` the model declares, each row carrying its
//! own per-type configuration so the refusal under test is the role's and not a
//! missing configuration's (that rule has its own suite, `data_sources.rs`).
//!
//! The harness (`call`, `create_api`, `router`, `Answer`) is the error-contract
//! suite's; `REGION` is the lifecycle suite's.

use serde_json::{json, Value};

use crate::error_contract::{call, create_api, router, Answer};
use crate::graphql_api_lifecycle::REGION;

/// Every `DataSourceType` the model declares, with whether AppSync must assume the
/// data source's service role to reach it.
///
/// Source: `smithy_query(query="shape", shape="DataSourceType")` for the value set,
/// and `DataSource$type` for what each value IS — a Lambda function, a DynamoDB
/// table, an OpenSearch domain, an EventBridge configuration, the Bedrock runtime and
/// a relational database are all reached with credentials AppSync assumes; `NONE`
/// ("there is no data source") and `HTTP` (a plain endpoint) are not. `HTTP` is listed
/// here in its credential-free form; the IAM-authorized form is its own case below.
const DATA_SOURCE_TYPES: &[(&str, bool)] = &[
    ("AWS_LAMBDA", true),
    ("AMAZON_DYNAMODB", true),
    ("AMAZON_ELASTICSEARCH", true),
    ("AMAZON_OPENSEARCH_SERVICE", true),
    ("AMAZON_EVENTBRIDGE", true),
    ("AMAZON_BEDROCK_RUNTIME", true),
    ("RELATIONAL_DATABASE", true),
    ("HTTP", false),
    ("NONE", false),
];

/// A role ARN of the shape `serviceRoleArn` carries.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/appsync-datasource";

/// The refusal a data source of a role-requiring type with no role takes.
const ROLE_REFUSAL: &str = "Service role arn not specified.";

/// The settings member a `DataSourceType` is configured through, as a request fragment
/// to merge into a create body — present on every row, so the only thing a row's
/// refusal can be about is the service role.
///
/// Source: `CreateDataSourceRequest`'s per-type configuration members and the members
/// each of those shapes requires. `NONE` and `AMAZON_BEDROCK_RUNTIME` have no
/// configuration member of their own, so they contribute none.
fn config_for(data_source_type: &str) -> Value {
    match data_source_type {
        "AWS_LAMBDA" => json!({
            "lambdaConfig": {
                "lambdaFunctionArn": format!("arn:aws:lambda:{REGION}:000000000000:function:orders"),
            },
        }),
        "AMAZON_DYNAMODB" => json!({
            "dynamodbConfig": { "tableName": "orders", "awsRegion": REGION },
        }),
        "AMAZON_ELASTICSEARCH" => json!({
            "elasticsearchConfig": {
                "endpoint": "https://search-orders.example.com",
                "awsRegion": REGION,
            },
        }),
        "AMAZON_OPENSEARCH_SERVICE" => json!({
            "openSearchServiceConfig": {
                "endpoint": "https://search-orders.example.com",
                "awsRegion": REGION,
            },
        }),
        "AMAZON_EVENTBRIDGE" => json!({
            "eventBridgeConfig": {
                "eventBusArn": format!("arn:aws:events:{REGION}:000000000000:event-bus/orders"),
            },
        }),
        "RELATIONAL_DATABASE" => json!({
            "relationalDatabaseConfig": {
                // `RelationalDatabaseSourceType`'s only value.
                "relationalDatabaseSourceType": "RDS_HTTP_ENDPOINT",
                "rdsHttpEndpointConfig": {
                    "awsRegion": REGION,
                    "dbClusterIdentifier": "orders-cluster",
                    "databaseName": "orders",
                },
            },
        }),
        "HTTP" => json!({
            "httpConfig": { "endpoint": "https://orders.example.com" },
        }),
        _ => json!({}),
    }
}

/// A `CreateDataSource`/`UpdateDataSource` body of this type and name, with
/// `serviceRoleArn` present only when `role` is `Some`, and any extra members merged
/// in after the per-type configuration (so a row can replace it).
fn request_body(name: &str, data_source_type: &str, role: Option<&str>, extra: Value) -> Value {
    let mut body = json!({ "name": name, "type": data_source_type });
    for fragment in [config_for(data_source_type), extra] {
        if let (Some(target), Some(source)) = (body.as_object_mut(), fragment.as_object()) {
            for (member, value) in source {
                target.insert(member.clone(), value.clone());
            }
        }
    }
    if let Some(role) = role {
        body["serviceRoleArn"] = json!(role);
    }
    body
}

/// Read one data source of an API back.
async fn get_data_source(
    router: &smithy_server_runtime::MultiServiceRouter,
    api_id: &str,
    name: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/apis/{api_id}/datasources/{name}"),
        None,
    )
    .await
}

/// Every data-source name the API's listing reports, paged through to the end.
async fn list_names(
    router: &smithy_server_runtime::MultiServiceRouter,
    api_id: &str,
) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let path = match &token {
            None => format!("/v1/apis/{api_id}/datasources"),
            Some(cursor) => format!("/v1/apis/{api_id}/datasources?nextToken={cursor}"),
        };
        let listed = call(router, "GET", &path, None).await;
        assert_eq!(
            listed.status, 200,
            "case ListDataSources: expected 200, got {listed:?}"
        );
        for data_source in listed.body["dataSources"].as_array().into_iter().flatten() {
            names.push(
                data_source["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        token = listed.body["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    names
}

/// The rule on the create: a type AppSync reaches by assuming a role is refused
/// without `serviceRoleArn` and created with one — on every `DataSourceType` the
/// model declares — and the refusal leaves nothing behind.
#[tokio::test]
async fn a_type_that_needs_a_service_role_is_refused_without_one() {
    let router = router();
    let api_id = create_api(&router, "Api_data_source_roles").await;

    for (data_source_type, needs_role) in DATA_SOURCE_TYPES {
        let without = format!("no_role_{data_source_type}");
        let case = format!("a {data_source_type} data source with no serviceRoleArn");
        let created = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/datasources"),
            Some(request_body(&without, data_source_type, None, json!({}))),
        )
        .await;

        if *needs_role {
            assert_eq!(
                created.status, 400,
                "case {case:?}: expected status 400, got {created:?}"
            );
            assert_eq!(
                created.error_type, "BadRequestException",
                "case {case:?}: expected error type BadRequestException, got {created:?}"
            );
            // The row carries its per-type configuration, so the refusal has to be
            // the ROLE's rather than a missing configuration's.
            assert_eq!(
                created.message, ROLE_REFUSAL,
                "case {case:?}: expected the refusal to name the absent member, got {created:?}"
            );

            // The refusal was effect-free: no data source of that name exists.
            let read = get_data_source(&router, &api_id, &without).await;
            assert_eq!(
                read.status, 404,
                "case {case:?}: expected the rejected name to exist nowhere, got {read:?}"
            );
            assert_eq!(
                read.error_type, "NotFoundException",
                "case {case:?}: expected error type NotFoundException, got {read:?}"
            );
            let listed = list_names(&router, &api_id).await;
            assert!(
                !listed.contains(&without),
                "case {case:?}: expected the listing not to hold the rejected name, \
                 got {listed:?}"
            );
        } else {
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected a type that needs no role to be created, \
                 got {created:?}"
            );
            assert_eq!(
                created.body["dataSource"]["serviceRoleArn"],
                Value::Null,
                "case {case:?}: expected no service role on the stored data source, \
                 got {created:?}"
            );
        }

        // The identical configuration WITH a role succeeds for every type, and the
        // stored data source reports it.
        let with = format!("with_role_{data_source_type}");
        let case = format!("a {data_source_type} data source with a serviceRoleArn");
        let created = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/datasources"),
            Some(request_body(
                &with,
                data_source_type,
                Some(ROLE_ARN),
                json!({}),
            )),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200, got {created:?}"
        );
        assert_eq!(
            created.body["dataSource"]["serviceRoleArn"].as_str(),
            Some(ROLE_ARN),
            "case {case:?}: expected the stored role, got {created:?}"
        );
        let read = get_data_source(&router, &api_id, &with).await;
        assert_eq!(
            read.body["dataSource"], created.body["dataSource"],
            "case {case:?}: expected the read to report the stored record, got {read:?}"
        );
    }

    // Only the data sources that were really created are listed.
    let expected: Vec<String> = DATA_SOURCE_TYPES
        .iter()
        .flat_map(|(data_source_type, needs_role)| {
            let mut names = Vec::new();
            if !needs_role {
                names.push(format!("no_role_{data_source_type}"));
            }
            names.push(format!("with_role_{data_source_type}"));
            names
        })
        .collect();
    let listed = list_names(&router, &api_id).await;
    assert_eq!(
        listed.len(),
        expected.len(),
        "case the whole listing: expected exactly the created data sources {expected:?}, \
         got {listed:?}"
    );
    for name in &expected {
        assert!(
            listed.contains(name),
            "case the whole listing: expected it to hold {name:?}, got {listed:?}"
        );
    }
}

/// The one type whose need for a role depends on its configuration: an HTTP endpoint
/// AppSync signs with IAM credentials needs a role, the same endpoint without that
/// authorization does not.
#[tokio::test]
async fn an_iam_authorized_http_endpoint_needs_a_service_role() {
    let router = router();
    let api_id = create_api(&router, "Api_http_auth").await;

    // The rows: no `authorizationConfig` at all, one naming AWS_IAM, and one naming
    // AWS_IAM with its IAM signing settings filled in.
    let rows: Vec<(&str, Value, bool)> = vec![
        ("a plain endpoint", json!({}), false),
        (
            "an IAM-authorized endpoint",
            json!({
                "httpConfig": {
                    "endpoint": "https://orders.example.com",
                    "authorizationConfig": { "authorizationType": "AWS_IAM" },
                },
            }),
            true,
        ),
        (
            "an IAM-authorized endpoint with signing settings",
            json!({
                "httpConfig": {
                    "endpoint": "https://orders.example.com",
                    "authorizationConfig": {
                        "authorizationType": "AWS_IAM",
                        "awsIamConfig": {
                            "signingRegion": REGION,
                            "signingServiceName": "execute-api",
                        },
                    },
                },
            }),
            true,
        ),
    ];

    for (index, (case, http_config, needs_role)) in rows.iter().enumerate() {
        let name = format!("http_{index}");
        let created = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/datasources"),
            Some(request_body(&name, "HTTP", None, http_config.clone())),
        )
        .await;
        if *needs_role {
            assert_eq!(
                created.status, 400,
                "case {case:?} without a role: expected status 400, got {created:?}"
            );
            assert_eq!(
                created.error_type, "BadRequestException",
                "case {case:?} without a role: expected error type BadRequestException, \
                 got {created:?}"
            );
            assert_eq!(
                created.message, ROLE_REFUSAL,
                "case {case:?} without a role: expected the role refusal, got {created:?}"
            );
            let read = get_data_source(&router, &api_id, &name).await;
            assert_eq!(
                read.status, 404,
                "case {case:?} without a role: expected nothing to have been stored, \
                 got {read:?}"
            );
        } else {
            assert_eq!(
                created.status, 200,
                "case {case:?} without a role: expected 200, got {created:?}"
            );
        }

        // With a role every one of them is created.
        let named = format!("http_role_{index}");
        let created = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/datasources"),
            Some(request_body(
                &named,
                "HTTP",
                Some(ROLE_ARN),
                http_config.clone(),
            )),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?} with a role: expected 200, got {created:?}"
        );
    }
}

/// The same requirement on the update: an update that would leave a role-requiring
/// data source without a role is refused and changes nothing, while one that sets a
/// role is applied and read back.
#[tokio::test]
async fn an_update_cannot_leave_a_data_source_without_the_role_its_type_needs() {
    let router = router();
    let api_id = create_api(&router, "Api_data_source_role_update").await;
    let name = "orders";

    // A type that needs no role at all, so the update is what first introduces the
    // requirement.
    let created = call(
        &router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(request_body(name, "NONE", None, json!({}))),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case create a data source: expected 200, got {created:?}"
    );

    for (data_source_type, needs_role) in DATA_SOURCE_TYPES {
        let case = format!("an update to {data_source_type} with no serviceRoleArn");
        let before = get_data_source(&router, &api_id, name).await;
        let updated = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/datasources/{name}"),
            Some(request_body(name, data_source_type, None, json!({}))),
        )
        .await;
        if *needs_role {
            assert_eq!(
                updated.status, 400,
                "case {case:?}: expected status 400, got {updated:?}"
            );
            assert_eq!(
                updated.message, ROLE_REFUSAL,
                "case {case:?}: expected the role refusal, got {updated:?}"
            );
            // A refused update changes nothing.
            let after = get_data_source(&router, &api_id, name).await;
            assert_eq!(
                after.body["dataSource"], before.body["dataSource"],
                "case {case:?}: expected the stored data source to be unchanged, got {after:?}"
            );
        } else {
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected 200 for a type that needs no role, got {updated:?}"
            );
        }

        // The same update WITH a role is applied, and the read reports it.
        let case = format!("an update to {data_source_type} with a serviceRoleArn");
        let updated = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/datasources/{name}"),
            Some(request_body(
                name,
                data_source_type,
                Some(ROLE_ARN),
                json!({}),
            )),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200, got {updated:?}"
        );
        let read = get_data_source(&router, &api_id, name).await;
        assert_eq!(
            read.body["dataSource"]["type"].as_str(),
            Some(*data_source_type),
            "case {case:?}: expected the new type, got {read:?}"
        );
        assert_eq!(
            read.body["dataSource"]["serviceRoleArn"].as_str(),
            Some(ROLE_ARN),
            "case {case:?}: expected the new role, got {read:?}"
        );
    }

    // An update of a name the API does not hold has nothing to change.
    let absent = call(
        &router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources/absent"),
        Some(request_body("absent", "NONE", None, json!({}))),
    )
    .await;
    assert_eq!(
        absent.status, 404,
        "case update an absent data source: expected status 404, got {absent:?}"
    );
    assert_eq!(
        absent.error_type, "NotFoundException",
        "case update an absent data source: expected error type NotFoundException, \
         got {absent:?}"
    );
}
