// SPDX-License-Identifier: CC0-1.0

//! How an Application Component and a physical resource are IDENTIFIED, end to
//! end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a
//! restJson1 request — and asserts the response BODY, not just the status.
//!
//! The rules under test:
//!
//! * an Application Component is addressable by the identifier the CLIENT chose:
//!   the request's `id` when it names one, else the component's `name`. The
//!   operations whose own output member is the component report its whole record,
//!   `additionalInfo` map included — empty rather than absent for a component
//!   carrying no attributes.
//! * a stored physical resource carries the account and region that own its
//!   physical identifier — the request's, else the pair an ARN identifier itself
//!   names, else the caller's own — and a `resourceName`: the request's, else its
//!   logical identifier when that is a modelled `EntityName` no other resource of
//!   the version already answers to. Every member
//!   `DescribeAppVersionResource` addresses a resource by is therefore readable
//!   back from what the create persisted.
//!
//! Each suite in this crate carries its own wire harness, as its siblings do.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_resiliencehub_awsresiliencehub::state::DRAFT_APP_VERSION;
use service_com_amazonaws_resiliencehub_awsresiliencehub::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for — NOT the service's own fallback
/// region, so a resource reported in it can only have taken it from the caller.
const REGION: &str = "us-west-2";

/// The account this emulator serves, and so the one that owns a physical resource
/// whose owner neither the request nor an ARN identifier names.
const CALLER_ACCOUNT: &str = "000000000000";

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

/// An application to hang components and resources off, and its ARN.
async fn create_an_app(router: &MultiServiceRouter, name: &str) -> String {
    let reply = post(router, "/create-app", &json!({"name": name})).await;
    reply.ok(name)["app"]["appArn"]
        .as_str()
        .expect("a minted appArn")
        .to_string()
}

/// A row of the component table: what the request says about the component's
/// identity, and the id the rule says it is addressable by afterwards.
struct ComponentRow {
    label: &'static str,
    /// The request's required `name`.
    name: String,
    /// The request's optional `id`.
    id: Option<&'static str>,
    /// The request's `additionalInfo`, when it carries one.
    additional_info: Option<Value>,
    /// The id the component answers to once created.
    expected_id: String,
}

/// A `name` at the top of `String255`'s modelled `length` (1..255, from
/// `smithy_query query="operation" operation="CreateAppVersionAppComponent"`),
/// which `EntityName255` — the shape `AppComponent$id` targets — also admits.
fn max_length_name() -> String {
    format!("c{}", "x".repeat(254))
}

/// The table: one row per form the identity rule has to hold for. The rows differ
/// in whether the request names an `id`, whether that id is the name, the name's
/// length and character set, and whether the component carries attributes.
fn component_table() -> Vec<ComponentRow> {
    let named = |label: &'static str, name: &str| ComponentRow {
        label,
        name: name.to_string(),
        id: None,
        additional_info: None,
        expected_id: name.to_string(),
    };
    vec![
        named("a name and no id", "orders-tier"),
        named("a one-character name", "c"),
        named("a name carrying every character an id keeps", "Orders_tier-2"),
        ComponentRow {
            label: "a name at the top of String255's 1..255 length",
            name: max_length_name(),
            id: None,
            additional_info: None,
            expected_id: max_length_name(),
        },
        ComponentRow {
            // `name` is a free-form `String255` in the model, and the service does
            // not rename what the client chose.
            label: "a name outside EntityName255's character set",
            name: "payments tier.1".to_string(),
            id: None,
            additional_info: None,
            expected_id: "payments tier.1".to_string(),
        },
        ComponentRow {
            label: "an id that is not the name",
            name: "database-tier".to_string(),
            id: Some("ac-42"),
            additional_info: None,
            expected_id: "ac-42".to_string(),
        },
        ComponentRow {
            label: "an id equal to the name",
            name: "networking-tier".to_string(),
            id: Some("networking-tier"),
            additional_info: None,
            expected_id: "networking-tier".to_string(),
        },
        ComponentRow {
            label: "one additionalInfo entry and no id",
            name: "queue-tier".to_string(),
            id: None,
            // The one key AWS documents for `additionalInfo`.
            additional_info: Some(json!({
                "failover-regions": ["[{\"region\":\"us-west-2\"}]"],
            })),
            expected_id: "queue-tier".to_string(),
        },
        ComponentRow {
            label: "additionalInfo and an id of its own",
            name: "storage-tier".to_string(),
            id: Some("ac-storage"),
            additional_info: Some(json!({
                "failover-regions": ["[{\"region\":\"eu-west-1\"}]"],
            })),
            expected_id: "ac-storage".to_string(),
        },
    ]
}

/// A component is addressable by the identifier the client chose, and the
/// component's own projection carries its `additionalInfo` map.
#[tokio::test]
async fn an_app_component_answers_to_the_identifier_the_client_chose() {
    let router = emulator();
    let app_arn = create_an_app(&router, "identity-components").await;

    for (index, row) in component_table().into_iter().enumerate() {
        let case = row.label;
        let mut request = json!({
            "appArn": app_arn,
            "name": row.name,
            "type": "AWS::ResilienceHub::ComputeAppComponent",
        });
        if let Some(id) = row.id {
            request["id"] = json!(id);
        }
        if let Some(additional_info) = row.additional_info.clone() {
            request["additionalInfo"] = additional_info;
        }

        let expected_component = json!({
            "id": row.expected_id,
            "name": row.name,
            "type": "AWS::ResilienceHub::ComputeAppComponent",
            "additionalInfo": row.additional_info.clone().unwrap_or_else(|| json!({})),
        });

        let created = post(&router, "/create-app-version-app-component", &request).await;
        assert_eq!(
            created.ok(case)["appComponent"],
            expected_component,
            "case {case:?}: expected {expected_component:?}, got {:?}",
            created.body
        );

        // The identifier the response reports is the one the component answers
        // to: a client never has to parse an id back out of a create response.
        let described = post(
            &router,
            "/describe-app-version-app-component",
            &json!({
                "appArn": app_arn,
                "appVersion": DRAFT_APP_VERSION,
                "id": row.expected_id,
            }),
        )
        .await;
        assert_eq!(
            described.ok(case)["appComponent"],
            expected_component,
            "case {case:?}: expected {expected_component:?} read back, got {:?}",
            described.body
        );

        // ... and the mapping a client built from component NAME to resource
        // holds: a resource naming the component reports the same id. The
        // resource's own identifiers are the loop's, so a long component name
        // cannot push them past their own modelled lengths.
        let resource = post(
            &router,
            "/create-app-version-resource",
            &json!({
                "appArn": app_arn,
                "logicalResourceId": {"identifier": format!("Resource-{index}")},
                "physicalResourceId": format!("resource-{index}"),
                "resourceType": "AWS::SQS::Queue",
                "appComponents": [row.name],
            }),
        )
        .await;
        assert_eq!(
            resource.ok(case)["physicalResource"]["appComponents"][0]["id"],
            json!(row.expected_id),
            "case {case:?}: the resource must report the component's own id, got {:?}",
            resource.body
        );
    }
}

/// A component the resource create has to invent is addressable by the name the
/// request bound it under, and a second resource naming it binds to that one.
#[tokio::test]
async fn a_component_created_by_a_resource_answers_to_the_name_it_was_bound_by() {
    let router = emulator();
    let app_arn = create_an_app(&router, "identity-implicit-components").await;

    let created = post(
        &router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "logicalResourceId": {"identifier": "ImplicitComponents"},
            "physicalResourceId": "implicit-components",
            "resourceType": "AWS::ECS::Cluster",
            "appComponents": ["invented-tier", "second-invented-tier"],
        }),
    )
    .await;
    let components = created.ok("a resource naming two unknown components")["physicalResource"]
        ["appComponents"]
        .clone();
    for (index, name) in ["invented-tier", "second-invented-tier"].iter().enumerate() {
        assert_eq!(
            components[index]["id"],
            json!(name),
            "case {name:?}: a component created for a resource answers to its name, \
             got {components:?}"
        );
        let described = post(
            &router,
            "/describe-app-version-app-component",
            &json!({"appArn": app_arn, "appVersion": DRAFT_APP_VERSION, "id": name}),
        )
        .await;
        assert_eq!(
            described.ok(name)["appComponent"]["name"],
            json!(name),
            "case {name:?}: the component must be readable under that id, got {:?}",
            described.body
        );
    }
}

/// A name the service DERIVED cannot refuse a create: two resources that differ
/// only in the stack their one logical identifier lives in are both stored, and
/// the name stays with the first of them.
#[tokio::test]
async fn a_derived_name_never_refuses_a_second_resource() {
    let router = emulator();
    let app_arn = create_an_app(&router, "identity-derived-names").await;

    let bodies = [
        (
            "the first resource, in no stack",
            json!({
                "appArn": app_arn,
                "logicalResourceId": {"identifier": "shared-identifier"},
                "physicalResourceId": "shared-identifier-one",
                "resourceType": "AWS::SQS::Queue",
                "appComponents": ["queue-tier"],
            }),
            // The derived name is free, so this resource takes it.
            Some("shared-identifier"),
        ),
        (
            "the same identifier in a stack",
            json!({
                "appArn": app_arn,
                "logicalResourceId": {
                    "identifier": "shared-identifier",
                    "logicalStackName": "another-stack",
                },
                "physicalResourceId": "shared-identifier-two",
                "resourceType": "AWS::SQS::Queue",
                "appComponents": ["queue-tier"],
            }),
            // The name is the first resource's, and this one is a different
            // logical resource, so it is stored without a name.
            None,
        ),
    ];

    for (case, body, expected_name) in bodies {
        let created = post(&router, "/create-app-version-resource", &body).await;
        assert_eq!(
            created.ok(case)["physicalResource"]["resourceName"],
            expected_name.map(Value::from).unwrap_or(Value::Null),
            "case {case:?}: expected resourceName {expected_name:?}, got {:?}",
            created.body
        );

        // Both resources are stored, each under its own logical id.
        let described = post(
            &router,
            "/describe-app-version-resource",
            &json!({
                "appArn": app_arn,
                "appVersion": DRAFT_APP_VERSION,
                "logicalResourceId": body["logicalResourceId"],
            }),
        )
        .await;
        assert_eq!(
            described.ok(case)["physicalResource"]["physicalResourceId"]["identifier"],
            body["physicalResourceId"],
            "case {case:?}: each logical resource must read back as itself, got {:?}",
            described.body
        );
    }

    // The name still resolves to exactly the resource that holds it.
    let by_name = post(
        &router,
        "/describe-app-version-resource",
        &json!({
            "appArn": app_arn,
            "appVersion": DRAFT_APP_VERSION,
            "resourceName": "shared-identifier",
        }),
    )
    .await;
    assert_eq!(
        by_name.ok("the name's holder")["physicalResource"]["physicalResourceId"]["identifier"],
        json!("shared-identifier-one"),
        "the derived name must address the resource that took it, got {:?}",
        by_name.body
    );
}

/// A row of the resource table: a `CreateAppVersionResource` body (minus the
/// `appArn`) and the identity members the rule says the stored resource carries.
struct ResourceRow {
    label: &'static str,
    request: fn() -> Value,
    /// The `PhysicalIdentifierType` the identifier's form implies.
    identifier_type: &'static str,
    /// The region and account the rule resolves as the resource's owner.
    region: &'static str,
    account: &'static str,
    /// The `resourceName` the stored resource carries, `None` when the resource
    /// can carry none.
    name: Option<String>,
}

/// A logical identifier of `length` characters, which is a modelled `EntityName`
/// exactly while `length` is within 2..60.
fn identifier_of_length(length: usize) -> String {
    format!("L{}", "x".repeat(length - 1))
}

/// The table: one row per form the resource-identity rule has to hold for. The
/// rows differ in the identifier's form, which half of the owning pair the request
/// names, whether the request names the resource, and — on the name derived from
/// the logical identifier — the modelled `EntityName` bounds and one past each.
fn resource_table() -> Vec<ResourceRow> {
    vec![
        ResourceRow {
            label: "a native identifier, with neither owner nor name named",
            request: || {
                json!({
                    "logicalResourceId": {"identifier": "orders-queue"},
                    "physicalResourceId": "orders-queue",
                    "resourceType": "AWS::SQS::Queue",
                    "appComponents": ["queue-tier"],
                })
            },
            identifier_type: "Native",
            region: REGION,
            account: CALLER_ACCOUNT,
            name: Some("orders-queue".to_string()),
        },
        ResourceRow {
            label: "an ARN identifier naming its own region and account",
            request: || {
                json!({
                    "logicalResourceId": {"identifier": "FailoverTable"},
                    "physicalResourceId": "arn:aws:dynamodb:eu-west-3:111122223333:table/Orders",
                    "resourceType": "AWS::DynamoDB::Table",
                    "appComponents": ["database-tier"],
                })
            },
            identifier_type: "Arn",
            region: "eu-west-3",
            account: "111122223333",
            name: Some("FailoverTable".to_string()),
        },
        ResourceRow {
            label: "an ARN identifier naming neither region nor account",
            request: || {
                json!({
                    "resourceName": "static-site",
                    "logicalResourceId": {"identifier": "StaticSite"},
                    // An S3 bucket ARN carries no region and no account.
                    "physicalResourceId": "arn:aws:s3:::static-site-bucket",
                    "resourceType": "AWS::S3::Bucket",
                    "appComponents": ["storage-tier"],
                })
            },
            identifier_type: "Arn",
            region: REGION,
            account: CALLER_ACCOUNT,
            name: Some("static-site".to_string()),
        },
        ResourceRow {
            label: "a request naming only the owning region",
            request: || {
                json!({
                    "logicalResourceId": {"identifier": "RegionOnly"},
                    "physicalResourceId": "region-only",
                    "resourceType": "AWS::SNS::Topic",
                    "awsRegion": "ap-southeast-2",
                    "appComponents": ["messaging-tier"],
                })
            },
            identifier_type: "Native",
            region: "ap-southeast-2",
            account: CALLER_ACCOUNT,
            name: Some("RegionOnly".to_string()),
        },
        ResourceRow {
            label: "a request naming only the owning account",
            request: || {
                json!({
                    "logicalResourceId": {"identifier": "AccountOnly"},
                    "physicalResourceId": "account-only",
                    "resourceType": "AWS::SNS::Topic",
                    "awsAccountId": "444455556666",
                    "appComponents": ["messaging-tier"],
                })
            },
            identifier_type: "Native",
            region: REGION,
            account: "444455556666",
            name: Some("AccountOnly".to_string()),
        },
        ResourceRow {
            label: "a request's own pair overriding the identifier's ARN",
            request: || {
                json!({
                    "resourceName": "cross-account-topic",
                    "logicalResourceId": {"identifier": "CrossAccountTopic"},
                    "physicalResourceId": "arn:aws:sns:eu-west-3:111122223333:alerts",
                    "resourceType": "AWS::SNS::Topic",
                    "awsRegion": "eu-central-1",
                    "awsAccountId": "777788889999",
                    "appComponents": ["messaging-tier"],
                })
            },
            identifier_type: "Arn",
            region: "eu-central-1",
            account: "777788889999",
            name: Some("cross-account-topic".to_string()),
        },
        ResourceRow {
            label: "a logical identifier at the bottom of EntityName's 2..60 length",
            request: || {
                json!({
                    "logicalResourceId": {"identifier": identifier_of_length(2)},
                    "physicalResourceId": "shortest-named",
                    "resourceType": "AWS::SQS::Queue",
                    "appComponents": ["queue-tier"],
                })
            },
            identifier_type: "Native",
            region: REGION,
            account: CALLER_ACCOUNT,
            name: Some(identifier_of_length(2)),
        },
        ResourceRow {
            label: "a logical identifier one character below EntityName's length",
            request: || {
                json!({
                    "logicalResourceId": {"identifier": identifier_of_length(1)},
                    "physicalResourceId": "too-short-to-name",
                    "resourceType": "AWS::SQS::Queue",
                    "appComponents": ["queue-tier"],
                })
            },
            identifier_type: "Native",
            region: REGION,
            account: CALLER_ACCOUNT,
            name: None,
        },
        ResourceRow {
            label: "a logical identifier at the top of EntityName's 2..60 length",
            request: || {
                json!({
                    "logicalResourceId": {"identifier": identifier_of_length(60)},
                    "physicalResourceId": "longest-named",
                    "resourceType": "AWS::SQS::Queue",
                    "appComponents": ["queue-tier"],
                })
            },
            identifier_type: "Native",
            region: REGION,
            account: CALLER_ACCOUNT,
            name: Some(identifier_of_length(60)),
        },
        ResourceRow {
            label: "a logical identifier one character past EntityName's length",
            request: || {
                json!({
                    "logicalResourceId": {"identifier": identifier_of_length(61)},
                    "physicalResourceId": "too-long-to-name",
                    "resourceType": "AWS::SQS::Queue",
                    "appComponents": ["queue-tier"],
                })
            },
            identifier_type: "Native",
            region: REGION,
            account: CALLER_ACCOUNT,
            name: None,
        },
        ResourceRow {
            label: "a logical identifier outside EntityName's character set",
            request: || {
                json!({
                    "logicalResourceId": {"identifier": "orders.queue/1"},
                    "physicalResourceId": "dotted-identifier",
                    "resourceType": "AWS::SQS::Queue",
                    "appComponents": ["queue-tier"],
                })
            },
            identifier_type: "Native",
            region: REGION,
            account: CALLER_ACCOUNT,
            name: None,
        },
        ResourceRow {
            label: "a resourceName the request names itself, unlike the logical id",
            request: || {
                json!({
                    "resourceName": "orders-function",
                    "logicalResourceId": {"identifier": "OrdersFunction"},
                    "physicalResourceId": "arn:aws:lambda:us-east-1:000000000000:function:orders",
                    "resourceType": "AWS::Lambda::Function",
                    "appComponents": ["compute-tier"],
                })
            },
            identifier_type: "Arn",
            region: "us-east-1",
            account: "000000000000",
            name: Some("orders-function".to_string()),
        },
    ]
}

/// Every stored resource carries its owning account and region and its name, and
/// each of them addresses it on the read path.
#[tokio::test]
async fn a_physical_resource_carries_its_owner_and_its_name() {
    let router = emulator();
    let app_arn = create_an_app(&router, "identity-resources").await;

    for row in resource_table() {
        let case = row.label;
        let mut request = (row.request)();
        request["appArn"] = json!(app_arn);

        let created = post(&router, "/create-app-version-resource", &request).await;
        let resource = created.ok(case)["physicalResource"].clone();

        assert_eq!(
            resource["physicalResourceId"],
            json!({
                "identifier": request["physicalResourceId"],
                "type": row.identifier_type,
                "awsRegion": row.region,
                "awsAccountId": row.account,
            }),
            "case {case:?}: expected the identifier owned by {} in {}, got {resource:?}",
            row.account,
            row.region
        );
        assert_eq!(
            resource["resourceName"],
            row.name
                .as_deref()
                .map(Value::from)
                .unwrap_or(Value::Null),
            "case {case:?}: expected resourceName {:?}, got {resource:?}",
            row.name
        );

        // Each member the read path addresses a resource by resolves to exactly
        // what the create persisted — including the owning pair, which narrows a
        // lookup by physical identifier.
        let mut selectors = vec![
            json!({"logicalResourceId": request["logicalResourceId"]}),
            json!({
                "physicalResourceId": request["physicalResourceId"],
                "awsRegion": row.region,
                "awsAccountId": row.account,
            }),
        ];
        if let Some(name) = row.name.as_deref() {
            selectors.push(json!({"resourceName": name}));
        }
        for selector in selectors {
            let mut body = json!({"appArn": app_arn, "appVersion": DRAFT_APP_VERSION});
            for (member, value) in selector.as_object().expect("a selector object") {
                body[member] = value.clone();
            }
            let described = post(&router, "/describe-app-version-resource", &body).await;
            assert_eq!(
                described.ok(case)["physicalResource"],
                resource,
                "case {case:?}: describe by {selector:?} must return what the create \
                 persisted, got {:?}",
                described.body
            );
        }
    }
}
