// SPDX-License-Identifier: CC0-1.0

//! Physical resources of an app version, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a
//! restJson1 request — and asserts the response BODY, not just the status.
//!
//! The rule under test: `CreateAppVersionResource` persists the physical resource
//! the request describes in the addressed application's DRAFT version, identified
//! by the whole `logicalResourceId`, deriving the `physicalResourceId`'s `type`
//! from the identifier's form (`Arn` for an ARN, `Native` otherwise) and its owning
//! region/account from the request or, failing that, from the identifier when it is
//! an ARN, and binding each `appComponents` name to the draft's component of that
//! name, creating one when the draft holds none; a later
//! `DescribeAppVersionResource` of the same `appArn`/`appVersion`, addressed by any
//! ONE of `resourceName`, `logicalResourceId` or `physicalResourceId`, returns that
//! stored resource member-for-member, and each link of the path that does not
//! resolve is refused with the modelled 404. A resource belongs to a SET of
//! components, so a repeated name binds once, and a request naming more than one
//! app version holds is refused.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_resiliencehub_awsresiliencehub::state::{
    DRAFT_APP_VERSION, MAX_APP_COMPONENTS_PER_APP_VERSION,
};
use service_com_amazonaws_resiliencehub_awsresiliencehub::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-east-1";

/// An emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These requests carry an unverifiable placeholder signature: this suite tests
    // the operations, not SigV4.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What a dispatched request answered.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype`; empty on success.
    error_code: String,
    body: Value,
}

impl Reply {
    /// The response body, asserting the call succeeded.
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?} {:?}",
            self.status, self.error_code, self.body
        );
        &self.body
    }

    /// Assert the call was refused with `status` and `code`.
    fn refused(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error_code.as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {} {:?} {:?}",
            self.status,
            self.error_code,
            self.body
        );
    }
}

/// POST one request through the router, exactly as the server front door does.
async fn post(router: &MultiServiceRouter, target: &str, body: &Value) -> Reply {
    let payload = serde_json::to_vec(body).expect("serializable body");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://resiliencehub.{REGION}.amazonaws.com{target}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/resiliencehub/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&raw)}))
    };
    Reply {
        status,
        error_code,
        body,
    }
}

/// An application to hang resources off, and its ARN.
async fn create_an_app(router: &MultiServiceRouter, name: &str) -> String {
    let reply = post(router, "/create-app", &json!({"name": name})).await;
    reply.ok(name)["app"]["appArn"]
        .as_str()
        .expect("a minted appArn")
        .to_string()
}

/// A row of the resource table: a `CreateAppVersionResource` body (minus the
/// `appArn`, which the test fills in) and what the rule says the stored
/// `physicalResourceId` carries for it.
///
/// The rows differ along what the rule generalises over: the identifier's form
/// (native id or ARN, in this region/account or a foreign one), whether the request
/// names the owning region and account itself, how many members the
/// `logicalResourceId` carries, how many Application Components the resource
/// belongs to, whether it is named, and whether `additionalInfo` is absent, one
/// entry or full width.
struct Resource {
    label: &'static str,
    request: fn() -> Value,
    /// The `PhysicalIdentifierType` the identifier's form implies.
    identifier_type: &'static str,
    /// The `physicalResourceId.awsRegion` / `.awsAccountId` the rule derives.
    region: Option<&'static str>,
    account: Option<&'static str>,
}

/// A native physical identifier at the top of `PhysicalResourceId$identifier`'s
/// modelled `length` (1..255, from `smithy_query query="shape"
/// shape="PhysicalResourceId"`).
fn max_length_identifier() -> String {
    format!("r{}", "x".repeat(254))
}

/// The table: one row per form of the request the rule has to hold for.
fn resource_table() -> Vec<Resource> {
    vec![
        Resource {
            label: "a native identifier, one component, nothing optional",
            request: || {
                json!({
                    "logicalResourceId": {"identifier": "orders-queue"},
                    "physicalResourceId": "orders-queue",
                    // `resourceType` is free-form `String255` in the model, so the
                    // service stores whatever CloudFormation type it is handed.
                    "resourceType": "AWS::SQS::Queue",
                    "appComponents": ["queue-tier"],
                })
            },
            identifier_type: "Native",
            region: None,
            account: None,
        },
        Resource {
            label: "an ARN identifier in this region and account",
            request: || {
                json!({
                    "resourceName": "orders-function",
                    "logicalResourceId": {"identifier": "OrdersFunction"},
                    "physicalResourceId":
                        "arn:aws:lambda:us-east-1:000000000000:function:orders",
                    "resourceType": "AWS::Lambda::Function",
                    "appComponents": ["compute-tier"],
                })
            },
            identifier_type: "Arn",
            region: Some("us-east-1"),
            account: Some("000000000000"),
        },
        Resource {
            label: "an ARN identifier owned by another region and account",
            request: || {
                json!({
                    "resourceName": "failover-table",
                    "logicalResourceId": {"identifier": "FailoverTable"},
                    "physicalResourceId":
                        "arn:aws:dynamodb:eu-west-3:111122223333:table/Orders",
                    "resourceType": "AWS::DynamoDB::Table",
                    "appComponents": ["database-tier"],
                })
            },
            identifier_type: "Arn",
            region: Some("eu-west-3"),
            account: Some("111122223333"),
        },
        Resource {
            label: "a native identifier whose region and account the request names",
            request: || {
                json!({
                    "resourceName": "static-site",
                    "logicalResourceId": {"identifier": "StaticSite"},
                    "physicalResourceId": "static-site-bucket",
                    "resourceType": "AWS::S3::Bucket",
                    "awsRegion": "us-west-2",
                    "awsAccountId": "444455556666",
                    "appComponents": ["storage-tier"],
                })
            },
            identifier_type: "Native",
            region: Some("us-west-2"),
            account: Some("444455556666"),
        },
        Resource {
            label: "a request's own region and account override the identifier's ARN",
            request: || {
                json!({
                    "resourceName": "cross-account-topic",
                    "logicalResourceId": {"identifier": "CrossAccountTopic"},
                    "physicalResourceId": "arn:aws:sns:eu-west-3:111122223333:alerts",
                    "resourceType": "AWS::SNS::Topic",
                    "awsRegion": "ap-southeast-2",
                    "awsAccountId": "777788889999",
                    "appComponents": ["messaging-tier"],
                })
            },
            identifier_type: "Arn",
            region: Some("ap-southeast-2"),
            account: Some("777788889999"),
        },
        Resource {
            label: "a logicalResourceId carrying every member it has",
            request: || {
                json!({
                    "resourceName": "every-member",
                    "logicalResourceId": {
                        "identifier": "WebTier",
                        "logicalStackName": "orders-stack",
                        "resourceGroupName": "orders-group",
                        "terraformSourceName": "orders.tfstate",
                        "eksSourceName": "orders-cluster/orders",
                    },
                    "physicalResourceId": "web-tier-asg",
                    "resourceType": "AWS::AutoScaling::AutoScalingGroup",
                    "appComponents": ["compute-tier"],
                })
            },
            identifier_type: "Native",
            region: None,
            account: None,
        },
        Resource {
            label: "a resource belonging to three Application Components",
            request: || {
                json!({
                    "resourceName": "shared-cluster",
                    "logicalResourceId": {"identifier": "SharedCluster"},
                    "physicalResourceId": "shared-cluster",
                    "resourceType": "AWS::ECS::Cluster",
                    // One name the draft already holds a component for (created by
                    // an earlier row) and two it does not.
                    "appComponents": ["compute-tier", "batch-tier", "edge-tier"],
                })
            },
            identifier_type: "Native",
            region: None,
            account: None,
        },
        Resource {
            label: "a resource belonging to no Application Component",
            request: || {
                json!({
                    "resourceName": "unassigned-resource",
                    "logicalResourceId": {"identifier": "UnassignedResource"},
                    "physicalResourceId": "unassigned-resource",
                    "resourceType": "AWS::EFS::FileSystem",
                    "appComponents": [],
                })
            },
            identifier_type: "Native",
            region: None,
            account: None,
        },
        Resource {
            label: "one additionalInfo entry",
            request: || {
                json!({
                    "resourceName": "with-additional-info",
                    "logicalResourceId": {"identifier": "WithAdditionalInfo"},
                    "physicalResourceId": "with-additional-info",
                    "resourceType": "AWS::RDS::DBCluster",
                    "appComponents": ["database-tier"],
                    // The one key AWS documents for `additionalInfo`.
                    "additionalInfo": {
                        "failover-regions": ["[{\"region\":\"us-west-2\",\"accounts\":[{\"id\":\"111122223333\"}]}]"],
                    },
                })
            },
            identifier_type: "Native",
            region: None,
            account: None,
        },
        Resource {
            label: "additionalInfo at the top of AdditionalInfoValueList's 1..10 length",
            request: || {
                json!({
                    "resourceName": "wide-additional-info",
                    "logicalResourceId": {"identifier": "WideAdditionalInfo"},
                    "physicalResourceId": "wide-additional-info",
                    "resourceType": "AWS::ApiGateway::RestApi",
                    "appComponents": ["edge-tier"],
                    "additionalInfo": {
                        "one": (1..=10).map(|n| format!("value-{n}")).collect::<Vec<_>>(),
                        "two": ["only-one"],
                    },
                })
            },
            identifier_type: "Native",
            region: None,
            account: None,
        },
        Resource {
            label: "a physicalResourceId at the top of the identifier's 1..255 length",
            request: || {
                json!({
                    "resourceName": "long-identifier",
                    "logicalResourceId": {"identifier": "LongIdentifier"},
                    "physicalResourceId": max_length_identifier(),
                    "resourceType": "AWS::ElasticLoadBalancingV2::LoadBalancer",
                    "appComponents": ["edge-tier"],
                })
            },
            identifier_type: "Native",
            region: None,
            account: None,
        },
        Resource {
            label: "a physicalResourceId at the bottom of the identifier's 1..255 length",
            request: || {
                json!({
                    "resourceName": "short-identifier",
                    "logicalResourceId": {"identifier": "ShortIdentifier"},
                    "physicalResourceId": "r",
                    "resourceType": "AWS::SNS::Topic",
                    "appComponents": ["messaging-tier"],
                })
            },
            identifier_type: "Native",
            region: None,
            account: None,
        },
        Resource {
            label: "two resources may share a logical identifier under different stacks",
            request: || {
                json!({
                    "resourceName": "orders-queue-in-a-stack",
                    "logicalResourceId": {
                        "identifier": "orders-queue",
                        "logicalStackName": "another-stack",
                    },
                    "physicalResourceId": "orders-queue-two",
                    "resourceType": "AWS::SQS::Queue",
                    "appComponents": ["queue-tier"],
                })
            },
            identifier_type: "Native",
            region: None,
            account: None,
        },
    ]
}

/// Assert `actual` is the physical resource `row`'s request describes.
///
/// Every member the rule fixes is checked: the caller's own members unchanged, the
/// derived physical identifier, the Application Component names in the order the
/// request listed them, and the two members a newly added resource is born with.
fn assert_resource(case: &str, actual: &Value, request: &Value, row: &Resource) {
    assert_eq!(
        actual["logicalResourceId"], request["logicalResourceId"],
        "case {case:?}: the logical id must be kept, got {actual:?}"
    );
    assert_eq!(
        actual["resourceType"], request["resourceType"],
        "case {case:?}: the resource type must be kept, got {actual:?}"
    );
    assert_eq!(
        actual["resourceName"],
        request
            .get("resourceName")
            .cloned()
            .unwrap_or(Value::Null),
        "case {case:?}: the resource name must be kept, got {actual:?}"
    );
    assert_eq!(
        actual["physicalResourceId"]["identifier"], request["physicalResourceId"],
        "case {case:?}: the physical identifier must be kept, got {actual:?}"
    );
    assert_eq!(
        actual["physicalResourceId"]["type"],
        json!(row.identifier_type),
        "case {case:?}: expected identifier type {:?}, got {actual:?}",
        row.identifier_type
    );
    assert_eq!(
        actual["physicalResourceId"]["awsRegion"],
        row.region.map(Value::from).unwrap_or(Value::Null),
        "case {case:?}: expected awsRegion {:?}, got {actual:?}",
        row.region
    );
    assert_eq!(
        actual["physicalResourceId"]["awsAccountId"],
        row.account.map(Value::from).unwrap_or(Value::Null),
        "case {case:?}: expected awsAccountId {:?}, got {actual:?}",
        row.account
    );
    assert_eq!(
        actual["additionalInfo"],
        request
            .get("additionalInfo")
            .cloned()
            .unwrap_or(Value::Null),
        "case {case:?}: additionalInfo must be kept, got {actual:?}"
    );
    assert_eq!(
        actual["excluded"],
        json!(false),
        "case {case:?}: a newly added resource is included in the assessment, got {actual:?}"
    );
    assert_eq!(
        actual["sourceType"],
        json!("AppTemplate"),
        "case {case:?}: a resource added through the API comes from the app template, \
         got {actual:?}"
    );

    let component_names: Vec<&str> = actual["appComponents"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no appComponents array in {actual:?}"))
        .iter()
        .map(|component| {
            component["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a component without a name in {actual:?}"))
        })
        .collect();
    let requested: Vec<&str> = request["appComponents"]
        .as_array()
        .expect("the row lists appComponents")
        .iter()
        .map(|name| name.as_str().expect("a component name"))
        .collect();
    assert_eq!(
        component_names, requested,
        "case {case:?}: the resource belongs to the components the request named, got {actual:?}"
    );
    for component in actual["appComponents"].as_array().into_iter().flatten() {
        assert!(
            component["type"].as_str().is_some_and(|ty| !ty.is_empty()),
            "case {case:?}: AppComponent$type is required by the model, got {component:?}"
        );
        assert!(
            component["id"].as_str().is_some_and(|id| !id.is_empty()),
            "case {case:?}: every stored component carries an id, got {component:?}"
        );
    }
}

/// The create/describe round trip, over the whole table: each row's resource is
/// persisted in the draft version and read back, by each selector that addresses
/// it, exactly as it was stored.
#[tokio::test]
async fn each_created_resource_is_read_back_from_the_draft_version() {
    let router = emulator();
    let app_arn = create_an_app(&router, "avr-roundtrip").await;

    for row in resource_table() {
        let case = row.label;
        let mut request = (row.request)();
        request["appArn"] = json!(app_arn);

        let created = post(&router, "/create-app-version-resource", &request).await;
        let created = created.ok(case).clone();

        assert_eq!(
            created["appArn"],
            json!(app_arn),
            "case {case:?}: the response must echo the addressed appArn, got {created:?}"
        );
        assert_eq!(
            created["appVersion"],
            json!(DRAFT_APP_VERSION),
            "case {case:?}: a create updates the draft version, got {created:?}"
        );
        assert_resource(case, &created["physicalResource"], &request, &row);

        // The round trip: the stored resource is what a later read returns,
        // whichever of the three selectors addresses it.
        let mut selectors = vec![json!({"logicalResourceId": request["logicalResourceId"]})];
        selectors.push(json!({"physicalResourceId": request["physicalResourceId"]}));
        if let Some(resource_name) = request.get("resourceName") {
            selectors.push(json!({"resourceName": resource_name}));
        }
        if let (Some(region), Some(account)) =
            (request.get("awsRegion"), request.get("awsAccountId"))
        {
            selectors.push(json!({
                "physicalResourceId": request["physicalResourceId"],
                "awsRegion": region,
                "awsAccountId": account,
            }));
        }

        for selector in selectors {
            let mut body = json!({"appArn": app_arn, "appVersion": DRAFT_APP_VERSION});
            for (member, value) in selector.as_object().expect("a selector object") {
                body[member] = value.clone();
            }
            let described = post(&router, "/describe-app-version-resource", &body).await;
            assert_eq!(
                described.ok(case),
                &json!({
                    "appArn": app_arn,
                    "appVersion": DRAFT_APP_VERSION,
                    "physicalResource": created["physicalResource"].clone(),
                }),
                "case {case:?}: describe by {selector:?} must return what the create persisted"
            );
        }
    }

    // Every row of the table is still there: a later create never replaces an
    // earlier resource.
    for row in resource_table() {
        let case = row.label;
        let request = (row.request)();
        let described = post(
            &router,
            "/describe-app-version-resource",
            &json!({
                "appArn": app_arn,
                "appVersion": DRAFT_APP_VERSION,
                "logicalResourceId": request["logicalResourceId"],
            }),
        )
        .await;
        assert_resource(
            case,
            &described.ok(case)["physicalResource"],
            &request,
            &row,
        );
    }
}

/// An Application Component the request names and the draft does not hold is
/// CREATED in the draft — readable through `DescribeAppVersionAppComponent` under
/// the id the resource reports — while a name the draft already holds binds to that
/// component instead of a second one.
#[tokio::test]
async fn named_app_components_are_created_once_and_then_reused() {
    let router = emulator();
    let app_arn = create_an_app(&router, "avr-components").await;

    // A component created the other way round: the resource below names it, so it
    // must bind to THIS component rather than create a second one.
    let existing = post(
        &router,
        "/create-app-version-app-component",
        &json!({
            "appArn": app_arn,
            "name": "compute-tier",
            "type": "AWS::ResilienceHub::ComputeAppComponent",
        }),
    )
    .await;
    let existing_id = existing.ok("an existing component")["appComponent"]["id"]
        .as_str()
        .expect("a minted component id")
        .to_string();

    let created = post(
        &router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "resourceName": "orders-function",
            "logicalResourceId": {"identifier": "OrdersFunction"},
            "physicalResourceId": "arn:aws:lambda:us-east-1:000000000000:function:orders",
            "resourceType": "AWS::Lambda::Function",
            "appComponents": ["compute-tier", "brand-new-tier"],
        }),
    )
    .await;
    let components = created.ok("a resource naming one old and one new component")
        ["physicalResource"]["appComponents"]
        .as_array()
        .expect("appComponents")
        .clone();
    assert_eq!(
        components.len(),
        2,
        "one entry per named component, got {components:?}"
    );
    assert_eq!(
        components[0],
        json!({
            "id": existing_id,
            "name": "compute-tier",
            "type": "AWS::ResilienceHub::ComputeAppComponent",
        }),
        "a name the draft already holds must bind to that component, got {components:?}"
    );

    // The created component is in the draft version, not synthesised into the
    // response: the implemented reader finds it under the id the resource reports.
    let created_id = components[1]["id"].as_str().expect("a minted id").to_string();
    let described = post(
        &router,
        "/describe-app-version-app-component",
        &json!({"appArn": app_arn, "appVersion": DRAFT_APP_VERSION, "id": created_id}),
    )
    .await;
    assert_eq!(
        described.ok("the created component")["appComponent"],
        components[1],
        "the component this call created must be stored in the draft version, got {:?}",
        described.body
    );

    // A second resource naming the same new component binds to it rather than
    // creating another.
    let second = post(
        &router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "resourceName": "orders-queue",
            "logicalResourceId": {"identifier": "OrdersQueue"},
            "physicalResourceId": "orders-queue",
            "resourceType": "AWS::SQS::Queue",
            "appComponents": ["brand-new-tier"],
        }),
    )
    .await;
    assert_eq!(
        second.ok("a second resource naming the new component")["physicalResource"]
            ["appComponents"],
        json!([components[1].clone()]),
        "a name the draft now holds must bind to that same component, got {:?}",
        second.body
    );

    // A resource belongs to a SET of components: a name repeated in one request —
    // or given once by name and once by the id it is stored under — binds once.
    let repeated = post(
        &router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "resourceName": "repeated-names",
            "logicalResourceId": {"identifier": "RepeatedNames"},
            "physicalResourceId": "repeated-names",
            "resourceType": "AWS::SQS::Queue",
            "appComponents": [
                "compute-tier",
                "compute-tier",
                existing_id.clone(),
                "brand-new-tier",
                "compute-tier",
            ],
        }),
    )
    .await;
    assert_eq!(
        repeated.ok("a request repeating component names")["physicalResource"]["appComponents"],
        json!([components[0].clone(), components[1].clone()]),
        "a repeated component name must bind once, got {:?}",
        repeated.body
    );

    // The list is clamped: a request naming more components than one app version
    // holds is refused with the modelled 402, and nothing is stored for it.
    let too_many: Vec<String> = (0..=MAX_APP_COMPONENTS_PER_APP_VERSION)
        .map(|n| format!("tier-{n}"))
        .collect();
    post(
        &router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "resourceName": "too-many-components",
            "logicalResourceId": {"identifier": "TooManyComponents"},
            "physicalResourceId": "too-many-components",
            "resourceType": "AWS::SQS::Queue",
            "appComponents": too_many,
        }),
    )
    .await
    .refused(
        "more component names than an app version holds",
        402,
        "ServiceQuotaExceededException",
    );
    post(
        &router,
        "/describe-app-version-resource",
        &json!({
            "appArn": app_arn,
            "appVersion": DRAFT_APP_VERSION,
            "logicalResourceId": {"identifier": "TooManyComponents"},
        }),
    )
    .await
    .refused(
        "the refused resource",
        404,
        "ResourceNotFoundException",
    );
}

/// A published version freezes the resources the draft held: a later write to the
/// draft cannot change what was published.
#[tokio::test]
async fn publishing_freezes_the_draft_s_resources() {
    let router = emulator();
    let app_arn = create_an_app(&router, "avr-publish").await;

    let before = post(
        &router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "resourceName": "published-resource",
            "logicalResourceId": {"identifier": "PublishedResource"},
            "physicalResourceId": "published-resource",
            "resourceType": "AWS::SQS::Queue",
            "appComponents": ["queue-tier"],
        }),
    )
    .await;
    let published_resource = before.ok("the resource to publish")["physicalResource"].clone();

    let published = post(&router, "/publish-app-version", &json!({"appArn": app_arn})).await;
    let app_version = published.ok("publish")["appVersion"]
        .as_str()
        .expect("a published appVersion")
        .to_string();

    // A write to the draft AFTER the publish.
    post(
        &router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "resourceName": "draft-only-resource",
            "logicalResourceId": {"identifier": "DraftOnlyResource"},
            "physicalResourceId": "draft-only-resource",
            "resourceType": "AWS::Lambda::Function",
            "appComponents": ["compute-tier"],
        }),
    )
    .await
    .ok("a resource added after the publish");

    for (case, version, expected_status) in [
        ("the published version", app_version.as_str(), 200),
        ("the draft version", DRAFT_APP_VERSION, 200),
    ] {
        let described = post(
            &router,
            "/describe-app-version-resource",
            &json!({
                "appArn": app_arn,
                "appVersion": version,
                "logicalResourceId": {"identifier": "PublishedResource"},
            }),
        )
        .await;
        assert_eq!(
            described.status, expected_status,
            "case {case:?}: the published resource is in both versions, got {:?}",
            described.body
        );
        assert_eq!(
            described.ok(case)["physicalResource"],
            published_resource,
            "case {case:?}: the frozen resource must be what the draft held, got {:?}",
            described.body
        );
    }

    // What the draft gained after the publish is NOT in the published version.
    post(
        &router,
        "/describe-app-version-resource",
        &json!({
            "appArn": app_arn,
            "appVersion": app_version,
            "logicalResourceId": {"identifier": "DraftOnlyResource"},
        }),
    )
    .await
    .refused(
        "a draft-only resource in the published version",
        404,
        "ResourceNotFoundException",
    );
    post(
        &router,
        "/describe-app-version-resource",
        &json!({
            "appArn": app_arn,
            "appVersion": DRAFT_APP_VERSION,
            "logicalResourceId": {"identifier": "DraftOnlyResource"},
        }),
    )
    .await
    .ok("a draft-only resource in the draft version");
}

/// Each link of a resource's path has to resolve, and a resource of one
/// application is never visible from another.
#[tokio::test]
async fn an_unresolvable_path_is_refused_with_the_modeled_404() {
    let router = emulator();
    let app_arn = create_an_app(&router, "avr-not-found").await;
    let other_arn = create_an_app(&router, "avr-not-found-other").await;
    let unknown_arn = format!(
        "arn:aws:resiliencehub:{REGION}:000000000000:app/00000000-0000-0000-0000-000000000000"
    );

    post(
        &router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "resourceName": "scoped-resource",
            "logicalResourceId": {"identifier": "ScopedResource"},
            "physicalResourceId": "scoped-resource",
            "resourceType": "AWS::SQS::Queue",
            "appComponents": ["queue-tier"],
        }),
    )
    .await
    .ok("a resource of the first app");

    for (case, body) in [
        (
            "a create under an unknown application",
            json!({
                "appArn": unknown_arn,
                "logicalResourceId": {"identifier": "ScopedResource"},
                "physicalResourceId": "scoped-resource",
                "resourceType": "AWS::SQS::Queue",
                "appComponents": ["queue-tier"],
            }),
        ),
    ] {
        post(&router, "/create-app-version-resource", &body)
            .await
            .refused(case, 404, "ResourceNotFoundException");
    }

    for (case, body) in [
        (
            "an unknown application",
            json!({
                "appArn": unknown_arn,
                "appVersion": DRAFT_APP_VERSION,
                "resourceName": "scoped-resource",
            }),
        ),
        (
            "an app version the application does not have",
            json!({
                "appArn": app_arn,
                "appVersion": "77",
                "resourceName": "scoped-resource",
            }),
        ),
        (
            "a resourceName the draft holds no resource for",
            json!({
                "appArn": app_arn,
                "appVersion": DRAFT_APP_VERSION,
                "resourceName": "no-such-resource",
            }),
        ),
        (
            "a logicalResourceId that differs only in a member",
            json!({
                "appArn": app_arn,
                "appVersion": DRAFT_APP_VERSION,
                "logicalResourceId": {
                    "identifier": "ScopedResource",
                    "logicalStackName": "not-the-stack",
                },
            }),
        ),
        (
            "a physicalResourceId narrowed to the wrong region",
            json!({
                "appArn": app_arn,
                "appVersion": DRAFT_APP_VERSION,
                "physicalResourceId": "scoped-resource",
                "awsRegion": "eu-west-3",
            }),
        ),
        (
            "another application's view of the same resource",
            json!({
                "appArn": other_arn,
                "appVersion": DRAFT_APP_VERSION,
                "resourceName": "scoped-resource",
            }),
        ),
    ] {
        post(&router, "/describe-app-version-resource", &body)
            .await
            .refused(case, 404, "ResourceNotFoundException");
    }
}

/// A describe has to address exactly one resource: the model's own documentation
/// says this API accepts only one of `resourceName`, `logicalResourceId` and
/// `physicalResourceId`.
#[tokio::test]
async fn a_describe_that_addresses_no_single_resource_is_refused() {
    let router = emulator();
    let app_arn = create_an_app(&router, "avr-selectors").await;

    for (case, body) in [
        (
            "no selector at all",
            json!({"appArn": app_arn, "appVersion": DRAFT_APP_VERSION}),
        ),
        (
            "two selectors",
            json!({
                "appArn": app_arn,
                "appVersion": DRAFT_APP_VERSION,
                "resourceName": "scoped-resource",
                "physicalResourceId": "scoped-resource",
            }),
        ),
        (
            "all three selectors",
            json!({
                "appArn": app_arn,
                "appVersion": DRAFT_APP_VERSION,
                "resourceName": "scoped-resource",
                "logicalResourceId": {"identifier": "ScopedResource"},
                "physicalResourceId": "scoped-resource",
            }),
        ),
        (
            "awsRegion with no physicalResourceId to narrow",
            json!({
                "appArn": app_arn,
                "appVersion": DRAFT_APP_VERSION,
                "resourceName": "scoped-resource",
                "awsRegion": REGION,
            }),
        ),
    ] {
        post(&router, "/describe-app-version-resource", &body)
            .await
            .refused(case, 400, "ValidationException");
    }
}

/// A resource's identity within one app version is its whole `logicalResourceId`,
/// and its `resourceName` is what one selector addresses it by: neither can be
/// taken twice.
#[tokio::test]
async fn a_resource_cannot_take_an_identity_already_in_use() {
    let router = emulator();
    let app_arn = create_an_app(&router, "avr-conflicts").await;

    let first = json!({
        "appArn": app_arn,
        "resourceName": "taken-name",
        "logicalResourceId": {"identifier": "TakenIdentifier"},
        "physicalResourceId": "taken",
        "resourceType": "AWS::SQS::Queue",
        "appComponents": ["queue-tier"],
    });
    post(&router, "/create-app-version-resource", &first)
        .await
        .ok("the first resource");

    for (case, body) in [
        ("the same logicalResourceId", first.clone()),
        (
            "the same logicalResourceId under another name",
            json!({
                "appArn": app_arn,
                "resourceName": "another-name",
                "logicalResourceId": {"identifier": "TakenIdentifier"},
                "physicalResourceId": "another",
                "resourceType": "AWS::SQS::Queue",
                "appComponents": ["queue-tier"],
            }),
        ),
        (
            "the same resourceName under another logical id",
            json!({
                "appArn": app_arn,
                "resourceName": "taken-name",
                "logicalResourceId": {"identifier": "AnotherIdentifier"},
                "physicalResourceId": "another",
                "resourceType": "AWS::SQS::Queue",
                "appComponents": ["queue-tier"],
            }),
        ),
    ] {
        post(&router, "/create-app-version-resource", &body)
            .await
            .refused(case, 409, "ConflictException");
    }
}

/// A replayed `clientToken` returns the resource its first call created; the same
/// token with different parameters is a conflict.
#[tokio::test]
async fn a_replayed_client_token_does_not_create_a_second_resource() {
    let router = emulator();
    let app_arn = create_an_app(&router, "avr-idempotency").await;

    let request = json!({
        "appArn": app_arn,
        "resourceName": "idempotent-resource",
        "logicalResourceId": {"identifier": "IdempotentResource"},
        "physicalResourceId": "idempotent-resource",
        "resourceType": "AWS::SQS::Queue",
        "appComponents": ["queue-tier"],
        "clientToken": "token-1",
    });
    let first = post(&router, "/create-app-version-resource", &request).await;
    let first = first.ok("the first call").clone();
    let replay = post(&router, "/create-app-version-resource", &request).await;
    assert_eq!(
        replay.ok("the replay"),
        &first,
        "a replayed clientToken must return the first result, got {:?}",
        replay.body
    );

    let mut changed = request.clone();
    changed["logicalResourceId"] = json!({"identifier": "SomethingElse"});
    changed["physicalResourceId"] = json!("something-else");
    changed["resourceName"] = json!("something-else");
    post(&router, "/create-app-version-resource", &changed)
        .await
        .refused("the same token with other parameters", 409, "ConflictException");

    // A replay still replays when the request addresses its components in a form
    // that resolves to the same SET: a repeated name, and a component named by the
    // id it is stored under rather than by its name.
    let existing = post(
        &router,
        "/create-app-version-app-component",
        &json!({
            "appArn": app_arn,
            "name": "compute-tier",
            "type": "AWS::ResilienceHub::ComputeAppComponent",
        }),
    )
    .await;
    let existing_id = existing.ok("an existing component")["appComponent"]["id"]
        .as_str()
        .expect("a minted component id")
        .to_string();

    let repeated = json!({
        "appArn": app_arn,
        "resourceName": "repeated-component-names",
        "logicalResourceId": {"identifier": "RepeatedComponentNames"},
        "physicalResourceId": "repeated-component-names",
        "resourceType": "AWS::SQS::Queue",
        "appComponents": ["queue-tier", "queue-tier", existing_id, "compute-tier"],
        "clientToken": "token-2",
    });
    let first = post(&router, "/create-app-version-resource", &repeated).await;
    let first = first.ok("a request repeating component names").clone();
    let replay = post(&router, "/create-app-version-resource", &repeated).await;
    assert_eq!(
        replay.ok("the replay of a repeating request"),
        &first,
        "a replay whose names resolve to the same components must return the first \
         result, got {:?}",
        replay.body
    );
}

/// A create refused for the per-version component ceiling stores NOTHING: the
/// components the request would have created are counted before any is written.
#[tokio::test]
async fn a_create_refused_for_the_component_ceiling_creates_no_component() {
    let router = emulator();
    let app_arn = create_an_app(&router, "avr-component-ceiling").await;

    // Fill the draft to one component short of the ceiling.
    for n in 0..MAX_APP_COMPONENTS_PER_APP_VERSION - 1 {
        post(
            &router,
            "/create-app-version-app-component",
            &json!({
                "appArn": app_arn,
                "name": format!("filler-{n}"),
                "type": "AWS::ResilienceHub::AppCommonAppComponent",
            }),
        )
        .await
        .ok("a filler component");
    }

    // Two new names need two slots and only one is left.
    post(
        &router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "resourceName": "over-the-ceiling",
            "logicalResourceId": {"identifier": "OverTheCeiling"},
            "physicalResourceId": "over-the-ceiling",
            "resourceType": "AWS::SQS::Queue",
            "appComponents": ["first-new-tier", "second-new-tier"],
        }),
    )
    .await
    .refused(
        "two new components with room for one",
        402,
        "ServiceQuotaExceededException",
    );

    // The refused call created neither component: the one remaining slot is still
    // free, so a request naming ONE new component succeeds.
    post(
        &router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "resourceName": "at-the-ceiling",
            "logicalResourceId": {"identifier": "AtTheCeiling"},
            "physicalResourceId": "at-the-ceiling",
            "resourceType": "AWS::SQS::Queue",
            "appComponents": ["first-new-tier"],
        }),
    )
    .await
    .ok("one new component with room for one");

    // And now there is none left.
    post(
        &router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "resourceName": "past-the-ceiling",
            "logicalResourceId": {"identifier": "PastTheCeiling"},
            "physicalResourceId": "past-the-ceiling",
            "resourceType": "AWS::SQS::Queue",
            "appComponents": ["second-new-tier"],
        }),
    )
    .await
    .refused(
        "a new component with no room left",
        402,
        "ServiceQuotaExceededException",
    );
}

/// A `physicalResourceId` longer than the stored
/// `PhysicalResourceId$identifier`'s modelled 255 characters is refused rather
/// than truncated; the bound itself and one below it are accepted.
#[tokio::test]
async fn a_physical_identifier_past_the_modeled_length_is_refused() {
    let router = emulator();
    let app_arn = create_an_app(&router, "avr-identifier-length").await;

    for (case, length, expected_status) in [
        ("one character", 1usize, 200u16),
        ("the modelled 255 ceiling", 255, 200),
        ("one past the ceiling", 256, 400),
    ] {
        let identifier = "r".repeat(length);
        let reply = post(
            &router,
            "/create-app-version-resource",
            &json!({
                "appArn": app_arn,
                "logicalResourceId": {"identifier": format!("Identifier{length}")},
                "physicalResourceId": identifier,
                "resourceType": "AWS::SQS::Queue",
                "appComponents": ["queue-tier"],
            }),
        )
        .await;
        assert_eq!(
            reply.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} {:?} {:?}",
            reply.status, reply.error_code, reply.body
        );
        if expected_status == 400 {
            reply.refused(case, 400, "ValidationException");
            // Nothing was persisted for the refused request.
            post(
                &router,
                "/describe-app-version-resource",
                &json!({
                    "appArn": app_arn,
                    "appVersion": DRAFT_APP_VERSION,
                    "logicalResourceId": {"identifier": format!("Identifier{length}")},
                }),
            )
            .await
            .refused(case, 404, "ResourceNotFoundException");
        }
    }
}
