// SPDX-License-Identifier: CC0-1.0

//! The namespace + workgroup lifecycle, end to end over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with
//! `POST /`, `X-Amz-Target: RedshiftServerless.<Operation>` and an awsJson1_1 body — so
//! the boundary validator, the protocol codec and the handler all take part, exactly as
//! they do for an SDK client.
//!
//! The rule under test: each of these operations computes its answer from the single
//! shared store, so a create is what every later read returns, an update is observable
//! through a read, and a delete is observable by its absence. The tables below span the
//! inputs the rule generalises over (value, count, region, page bound, port), not the one
//! request the issue quoted.
//!
//! The harness below (the emulator, the signed call, the error-code reader and the two
//! create helpers) is `pub(crate)` because the other suites in this test crate drive the
//! same wire path; see `tests/all/snapshot_and_usage_limit.rs`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every ARN this emulator mints carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// A SigV4 `Authorization` header scoped to this service in `region`. It is what routes
/// the request to this service AND what names the region its ARNs carry; signatures are
/// not verified here (authentication is off), only the scope is read.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/redshift-serverless/aws4_request, \
         SignedHeaders=host, Signature=deadbeef"
    )
}

/// One emulator with an empty store: the shared engine the router builds in production,
/// carrying only this service.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_redshiftserverless_redshiftserverless::EmulatedService::new(engine),
    );
    router
}

/// Call `operation` with `body`, signed for `region`; answer its HTTP status and parsed
/// JSON body.
pub(crate) async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("RedshiftServerless.{operation}"))
        .header("authorization", authorization(region))
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// [`call_in`] in `us-east-1`.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call_in(router, "us-east-1", operation, body).await
}

/// The error shape name out of an awsJson error envelope, with any namespace prefix
/// dropped (`com.amazonaws.redshiftserverless#ConflictException` -> `ConflictException`).
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .split('#')
        .next_back()
        .unwrap_or("")
        .to_string()
}

/// `base` with every member of `extra` set on it.
fn merged(base: Value, extra: &Value) -> Value {
    let mut object = base.as_object().cloned().unwrap_or_default();
    if let Some(extra) = extra.as_object() {
        for (key, value) in extra {
            object.insert(key.clone(), value.clone());
        }
    }
    Value::Object(object)
}

/// A canonical hyphenated UUID: 8-4-4-4-12 hex groups.
pub(crate) fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12])
        && value
            .chars()
            .all(|c| c == '-' || (c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
}

/// The member of `value` named by `path` (`"endpoint.address"`), or `Value::Null`.
pub(crate) fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment).unwrap_or(&Value::Null);
    }
    current
}

/// Create a namespace with `name`, asserting it succeeded.
pub(crate) async fn create_namespace(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, body) = call(router, "CreateNamespace", json!({ "namespaceName": name })).await;
    assert_eq!(
        status, 200,
        "CreateNamespace {name}: expected 200, got {status} with {body}"
    );
    body["namespace"].clone()
}

/// Create a workgroup with `name` in `namespace`, asserting it succeeded.
pub(crate) async fn create_workgroup(router: &MultiServiceRouter, name: &str, namespace: &str) -> Value {
    let (status, body) = call(
        router,
        "CreateWorkgroup",
        json!({ "workgroupName": name, "namespaceName": namespace }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateWorkgroup {name}: expected 200, got {status} with {body}"
    );
    body["workgroup"].clone()
}

// ---------------------------------------------------------------------------
// The lifecycle the issue walks: create -> read -> update -> delete, with the
// refusals each step owes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_namespace_and_workgroup_lifecycle_is_served_from_state() {
    let router = emulator();

    // CreateNamespace fills in the server-derived fields and stores the record.
    let (status, body) = call(
        &router,
        "CreateNamespace",
        json!({
            "namespaceName": "bootstrap-ns",
            "adminUsername": "admin",
            "adminUserPassword": "Passw0rd!",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateNamespace: expected 200, got {status} with {body}"
    );
    let created_namespace = body["namespace"].clone();
    let namespace_id = created_namespace["namespaceId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        is_uuid(&namespace_id),
        "CreateNamespace: namespaceId must be a UUID, got {namespace_id:?}"
    );
    assert_eq!(
        created_namespace["namespaceArn"],
        json!(format!(
            "arn:aws:redshift-serverless:us-east-1:{ACCOUNT}:namespace/{namespace_id}"
        )),
        "CreateNamespace: namespaceArn is derived from the id, got {:?}",
        created_namespace["namespaceArn"]
    );
    assert_eq!(
        created_namespace["status"],
        json!("AVAILABLE"),
        "CreateNamespace: got {:?}",
        created_namespace["status"]
    );
    assert_eq!(
        created_namespace["dbName"],
        json!("dev"),
        "CreateNamespace: dbName defaults to dev, got {:?}",
        created_namespace["dbName"]
    );
    assert!(
        created_namespace["creationDate"].is_string(),
        "CreateNamespace: creationDate is a date-time string, got {:?}",
        created_namespace["creationDate"]
    );
    assert!(
        !body.to_string().contains("Passw0rd!"),
        "CreateNamespace: the admin password must never be echoed, got {body}"
    );

    // GetNamespace reports the stored record, unchanged.
    let (status, body) = call(
        &router,
        "GetNamespace",
        json!({ "namespaceName": "bootstrap-ns" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetNamespace: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["namespace"], created_namespace,
        "GetNamespace must return the created record byte for byte, got {:?}",
        body["namespace"]
    );

    // Re-creating the same name is refused as a bad request, and the stored record is
    // untouched.
    let (status, body) = call(
        &router,
        "CreateNamespace",
        json!({ "namespaceName": "bootstrap-ns" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ConflictException"),
        "CreateNamespace on an existing name: got {status} with {body}"
    );

    // A workgroup needs its namespace to exist.
    let (status, body) = call(
        &router,
        "CreateWorkgroup",
        json!({ "workgroupName": "wg2", "namespaceName": "does-not-exist" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "CreateWorkgroup into an unknown namespace: got {status} with {body}"
    );

    // CreateWorkgroup derives id, ARN, endpoint and the capacity/port defaults.
    let (status, body) = call(
        &router,
        "CreateWorkgroup",
        json!({
            "workgroupName": "bootstrap-wg",
            "namespaceName": "bootstrap-ns",
            "baseCapacity": 32,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateWorkgroup: expected 200, got {status} with {body}"
    );
    let created_workgroup = body["workgroup"].clone();
    let workgroup_id = created_workgroup["workgroupId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        is_uuid(&workgroup_id),
        "CreateWorkgroup: workgroupId must be a UUID, got {workgroup_id:?}"
    );
    assert_eq!(
        created_workgroup["workgroupArn"],
        json!(format!(
            "arn:aws:redshift-serverless:us-east-1:{ACCOUNT}:workgroup/{workgroup_id}"
        )),
        "CreateWorkgroup: got {:?}",
        created_workgroup["workgroupArn"]
    );
    assert_eq!(
        created_workgroup["baseCapacity"],
        json!(32),
        "CreateWorkgroup: baseCapacity is the request's, got {:?}",
        created_workgroup["baseCapacity"]
    );
    assert_eq!(
        created_workgroup["port"],
        json!(5439),
        "CreateWorkgroup: port defaults to 5439, got {:?}",
        created_workgroup["port"]
    );
    assert_eq!(
        at(&created_workgroup, "endpoint.address"),
        &json!(format!(
            "bootstrap-wg.{ACCOUNT}.us-east-1.redshift-serverless.amazonaws.com"
        )),
        "CreateWorkgroup: endpoint.address is derived, got {:?}",
        at(&created_workgroup, "endpoint.address")
    );

    // GetWorkgroup reports the stored record. `status` is excluded only because the
    // clock-owed CREATING -> AVAILABLE transition may already have fallen due; the
    // transition itself is pinned by its own test below.
    let (status, body) = call(
        &router,
        "GetWorkgroup",
        json!({ "workgroupName": "bootstrap-wg" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetWorkgroup: expected 200, got {status} with {body}"
    );
    let read_workgroup = body["workgroup"].clone();
    for member in [
        "workgroupId",
        "workgroupArn",
        "workgroupName",
        "namespaceName",
        "baseCapacity",
        "port",
        "endpoint",
        "creationDate",
        "workgroupVersion",
        "publiclyAccessible",
        "enhancedVpcRouting",
    ] {
        assert_eq!(
            read_workgroup[member], created_workgroup[member],
            "GetWorkgroup {member}: expected the created value {:?}, got {:?}",
            created_workgroup[member], read_workgroup[member]
        );
    }

    // UpdateWorkgroup mutates the stored record in place.
    let (status, body) = call(
        &router,
        "UpdateWorkgroup",
        json!({ "workgroupName": "bootstrap-wg", "baseCapacity": 64, "port": 8191 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateWorkgroup: expected 200, got {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "GetWorkgroup",
        json!({ "workgroupName": "bootstrap-wg" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetWorkgroup after update: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["workgroup"]["baseCapacity"],
        json!(64),
        "GetWorkgroup after update: got {:?}",
        body["workgroup"]["baseCapacity"]
    );
    assert_eq!(
        at(&body["workgroup"], "endpoint.port"),
        &json!(8191),
        "GetWorkgroup after update: the endpoint follows the port, got {:?}",
        at(&body["workgroup"], "endpoint.port")
    );
    assert_eq!(
        body["workgroup"]["workgroupId"], created_workgroup["workgroupId"],
        "an update must not re-mint the workgroup id, got {:?}",
        body["workgroup"]["workgroupId"]
    );

    // A namespace a workgroup still references cannot be deleted.
    let (status, body) = call(
        &router,
        "DeleteNamespace",
        json!({ "namespaceName": "bootstrap-ns" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (409, "ConflictException"),
        "DeleteNamespace with a workgroup attached: got {status} with {body}"
    );

    // DeleteWorkgroup reports the removed record and the removal is observable.
    let (status, body) = call(
        &router,
        "DeleteWorkgroup",
        json!({ "workgroupName": "bootstrap-wg" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteWorkgroup: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["workgroup"]["workgroupId"], created_workgroup["workgroupId"],
        "DeleteWorkgroup returns the record it removed, got {:?}",
        body["workgroup"]
    );
    assert_eq!(
        body["workgroup"]["status"],
        json!("DELETING"),
        "DeleteWorkgroup: got {:?}",
        body["workgroup"]["status"]
    );
    let (status, body) = call(
        &router,
        "GetWorkgroup",
        json!({ "workgroupName": "bootstrap-wg" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "GetWorkgroup after the delete: got {status} with {body}"
    );
    let (status, body) = call(&router, "ListWorkgroups", json!({})).await;
    assert_eq!(
        (status, &body["workgroups"]),
        (200, &json!([])),
        "ListWorkgroups after the delete: got {status} with {body}"
    );

    // And now the namespace goes too.
    let (status, body) = call(
        &router,
        "DeleteNamespace",
        json!({ "namespaceName": "bootstrap-ns" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteNamespace: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["namespace"]["namespaceId"], created_namespace["namespaceId"],
        "DeleteNamespace returns the record it removed, got {:?}",
        body["namespace"]
    );
    assert_eq!(
        body["namespace"]["status"],
        json!("DELETING"),
        "DeleteNamespace: got {:?}",
        body["namespace"]["status"]
    );
    let (status, body) = call(
        &router,
        "GetNamespace",
        json!({ "namespaceName": "bootstrap-ns" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "GetNamespace after the delete: got {status} with {body}"
    );
    let (status, body) = call(&router, "ListNamespaces", json!({})).await;
    assert_eq!(
        (status, &body["namespaces"]),
        (200, &json!([])),
        "ListNamespaces after the delete: got {status} with {body}"
    );
}

/// One create case: a label, the members the request sets, and the members the stored
/// resource must then carry.
type CreateCase = (&'static str, Value, Vec<(&'static str, Value)>);

// ---------------------------------------------------------------------------
// What a create stores, over the span of inputs the rule covers.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_created_namespace_stores_each_members_value_and_the_documented_defaults() {
    // Every member comes from `smithy_query(query="operation", operation="CreateNamespace")`
    // and the `Namespace` shape; `dev` is the first database `Namespace$dbName` describes.
    let cases: [CreateCase; 6] = [
        (
            "no optional member set",
            json!({}),
            vec![
                ("dbName", json!("dev")),
                ("status", json!("AVAILABLE")),
                ("iamRoles", json!([])),
                ("logExports", json!([])),
            ],
        ),
        (
            "a named database",
            json!({ "dbName": "analytics" }),
            vec![("dbName", json!("analytics"))],
        ),
        (
            "an admin user",
            json!({ "adminUsername": "dbadmin", "adminUserPassword": "Passw0rd!" }),
            vec![("adminUsername", json!("dbadmin"))],
        ),
        (
            "two iam roles and a default",
            json!({
                "iamRoles": [
                    "arn:aws:iam::000000000000:role/one",
                    "arn:aws:iam::000000000000:role/two",
                ],
                "defaultIamRoleArn": "arn:aws:iam::000000000000:role/one",
            }),
            vec![
                (
                    "iamRoles",
                    json!([
                        "arn:aws:iam::000000000000:role/one",
                        "arn:aws:iam::000000000000:role/two",
                    ]),
                ),
                (
                    "defaultIamRoleArn",
                    json!("arn:aws:iam::000000000000:role/one"),
                ),
            ],
        ),
        (
            "three log exports",
            json!({ "logExports": ["useractivitylog", "userlog", "connectionlog"] }),
            vec![(
                "logExports",
                json!(["useractivitylog", "userlog", "connectionlog"]),
            )],
        ),
        (
            "a customer managed key",
            json!({ "kmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/abc" }),
            vec![(
                "kmsKeyId",
                json!("arn:aws:kms:us-east-1:000000000000:key/abc"),
            )],
        ),
    ];

    let router = emulator();
    for (index, (case, request, expected)) in cases.iter().enumerate() {
        let name = format!("ns-case-{index}");
        let (status, body) = call(
            &router,
            "CreateNamespace",
            merged(json!({ "namespaceName": name }), request),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateNamespace expected 200, got {status} with {body}"
        );
        let created = body["namespace"].clone();
        for (member, want) in expected {
            assert_eq!(
                &created[*member], want,
                "case {case:?}: {member} expected {want:?}, got {:?}", created[*member]
            );
        }

        // The create's answer and the read's answer are the same record.
        let (status, body) = call(&router, "GetNamespace", json!({ "namespaceName": name })).await;
        assert_eq!(
            (status, &body["namespace"]),
            (200, &created),
            "case {case:?}: GetNamespace must report the created record, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_created_workgroup_stores_each_members_value_and_the_documented_defaults() {
    // Defaults: `baseCapacity` 128 RPUs, and `port` 5439 — "Valid port ranges are
    // 5431-5455 and 8191-8215. The default is 5439." (`Workgroup$port` doc).
    let cases: [CreateCase; 8] = [
        (
            "no optional member set",
            json!({}),
            vec![
                ("baseCapacity", json!(128)),
                ("port", json!(5439)),
                ("publiclyAccessible", json!(false)),
                ("enhancedVpcRouting", json!(false)),
                ("subnetIds", json!([])),
                ("securityGroupIds", json!([])),
                ("configParameters", json!([])),
                ("ipAddressType", json!("ipv4")),
            ],
        ),
        (
            "the smallest documented base capacity step",
            json!({ "baseCapacity": 8 }),
            vec![("baseCapacity", json!(8))],
        ),
        (
            "a base capacity and a max capacity",
            json!({ "baseCapacity": 256, "maxCapacity": 512 }),
            vec![("baseCapacity", json!(256)), ("maxCapacity", json!(512))],
        ),
        (
            "a port at the low range's floor",
            json!({ "port": 5431 }),
            vec![("port", json!(5431))],
        ),
        (
            "a port in the high range",
            json!({ "port": 8215 }),
            vec![("port", json!(8215))],
        ),
        (
            "three subnets and one security group",
            json!({
                "subnetIds": ["subnet-1", "subnet-2", "subnet-3"],
                "securityGroupIds": ["sg-1"],
            }),
            vec![
                ("subnetIds", json!(["subnet-1", "subnet-2", "subnet-3"])),
                ("securityGroupIds", json!(["sg-1"])),
            ],
        ),
        (
            "two config parameters",
            json!({
                "configParameters": [
                    { "parameterKey": "query_group", "parameterValue": "etl" },
                    { "parameterKey": "require_ssl", "parameterValue": "true" },
                ],
            }),
            vec![(
                "configParameters",
                json!([
                    { "parameterKey": "query_group", "parameterValue": "etl" },
                    { "parameterKey": "require_ssl", "parameterValue": "true" },
                ]),
            )],
        ),
        (
            "public, vpc-routed and dualstack",
            json!({
                "publiclyAccessible": true,
                "enhancedVpcRouting": true,
                "ipAddressType": "dualstack",
            }),
            vec![
                ("publiclyAccessible", json!(true)),
                ("enhancedVpcRouting", json!(true)),
                ("ipAddressType", json!("dualstack")),
            ],
        ),
    ];

    let router = emulator();
    create_namespace(&router, "wg-cases-ns").await;
    for (index, (case, request, expected)) in cases.iter().enumerate() {
        let name = format!("wg-case-{index}");
        let (status, body) = call(
            &router,
            "CreateWorkgroup",
            merged(
                json!({ "workgroupName": name, "namespaceName": "wg-cases-ns" }),
                request,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateWorkgroup expected 200, got {status} with {body}"
        );
        let created = body["workgroup"].clone();
        for (member, want) in expected {
            assert_eq!(
                &created[*member], want,
                "case {case:?}: {member} expected {want:?}, got {:?}", created[*member]
            );
        }
        // The endpoint is derived from the name and the effective port, never supplied.
        assert_eq!(
            at(&created, "endpoint.port"),
            &created["port"],
            "case {case:?}: endpoint.port must equal port {:?}, got {:?}",
            created["port"],
            at(&created, "endpoint.port")
        );
        assert_eq!(
            at(&created, "endpoint.address"),
            &json!(format!(
                "{name}.{ACCOUNT}.us-east-1.redshift-serverless.amazonaws.com"
            )),
            "case {case:?}: got {:?}",
            at(&created, "endpoint.address")
        );

        let (status, body) = call(&router, "GetWorkgroup", json!({ "workgroupName": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetWorkgroup expected 200, got {status} with {body}"
        );
        for (member, want) in expected {
            assert_eq!(
                &body["workgroup"][*member], want,
                "case {case:?}: GetWorkgroup {member} expected {want:?}, got {:?}",
                body["workgroup"][*member]
            );
        }
    }
}

#[tokio::test]
async fn an_arn_and_an_endpoint_carry_the_region_the_request_is_signed_for() {
    // The rule generalises over region: nothing about it is specific to us-east-1.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let router = emulator();
        let (status, body) = call_in(
            &router,
            region,
            "CreateNamespace",
            json!({ "namespaceName": "region-ns" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region}: CreateNamespace expected 200, got {status} with {body}"
        );
        let namespace_arn = body["namespace"]["namespaceArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            namespace_arn.starts_with(&format!(
                "arn:aws:redshift-serverless:{region}:{ACCOUNT}:namespace/"
            )),
            "region {region}: got {namespace_arn:?}"
        );

        let (status, body) = call_in(
            &router,
            region,
            "CreateWorkgroup",
            json!({ "workgroupName": "region-wg", "namespaceName": "region-ns" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region}: CreateWorkgroup expected 200, got {status} with {body}"
        );
        assert_eq!(
            at(&body["workgroup"], "endpoint.address"),
            &json!(format!(
                "region-wg.{ACCOUNT}.{region}.redshift-serverless.amazonaws.com"
            )),
            "region {region}: got {:?}",
            at(&body["workgroup"], "endpoint.address")
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination: `maxResults` is a page bound, the token resumes where the page ended.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pagination_walks_every_record_exactly_once_whatever_the_page_bound() {
    for count in [0_usize, 1, 2, 5, 7] {
        for bound in [None, Some(1), Some(2), Some(3), Some(100)] {
            let router = emulator();
            let mut expected: Vec<String> = Vec::new();
            for index in 0..count {
                let name = format!("ns-{index:02}");
                create_namespace(&router, &name).await;
                create_workgroup(&router, &format!("wg-{index:02}"), &name).await;
                expected.push(name);
            }
            expected.sort();

            for (operation, items, member) in [
                ("ListNamespaces", "namespaces", "namespaceName"),
                ("ListWorkgroups", "workgroups", "workgroupName"),
            ] {
                let case = format!("{operation} over {count} records, maxResults {bound:?}");
                let mut seen: Vec<String> = Vec::new();
                let mut token: Option<String> = None;
                let mut pages = 0;
                loop {
                    let mut request = serde_json::Map::new();
                    if let Some(bound) = bound {
                        request.insert("maxResults".to_string(), json!(bound));
                    }
                    if let Some(token) = &token {
                        request.insert("nextToken".to_string(), json!(token));
                    }
                    let (status, body) =
                        call(&router, operation, Value::Object(request)).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected 200, got {status} with {body}"
                    );
                    let page = body[items].as_array().cloned().unwrap_or_default();
                    if let Some(bound) = bound {
                        assert!(
                            page.len() <= bound as usize,
                            "case {case:?}: a page holds at most maxResults, got {} entries",
                            page.len()
                        );
                    }
                    for entry in &page {
                        seen.push(entry[member].as_str().unwrap_or_default().to_string());
                    }
                    token = body["nextToken"].as_str().map(str::to_string);
                    pages += 1;
                    assert!(
                        pages <= count + 2,
                        "case {case:?}: pagination did not terminate after {pages} pages"
                    );
                    if token.is_none() {
                        break;
                    }
                    assert!(
                        !page.is_empty(),
                        "case {case:?}: a token was returned for an empty page"
                    );
                }

                let want: Vec<String> = match member {
                    "namespaceName" => expected.clone(),
                    _ => expected
                        .iter()
                        .map(|name| name.replace("ns-", "wg-"))
                        .collect(),
                };
                assert_eq!(
                    seen, want,
                    "case {case:?}: expected every record once in key order {want:?}, got {seen:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_page_token_the_service_never_issued_is_refused() {
    let router = emulator();
    create_namespace(&router, "token-ns").await;
    for operation in ["ListNamespaces", "ListWorkgroups"] {
        for token in ["not-a-token", "zz", "0"] {
            let (status, body) = call(&router, operation, json!({ "nextToken": token })).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "{operation} with nextToken {token:?}: got {status} with {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_list_filtered_to_another_owner_account_is_empty() {
    let router = emulator();
    create_namespace(&router, "owner-ns").await;
    create_workgroup(&router, "owner-wg", "owner-ns").await;

    let (status, body) = call(
        &router,
        "ListWorkgroups",
        json!({ "ownerAccount": ACCOUNT }),
    )
    .await;
    assert_eq!(
        (status, body["workgroups"].as_array().map(Vec::len)),
        (200, Some(1)),
        "ListWorkgroups for this account: got {status} with {body}"
    );

    let (status, body) = call(
        &router,
        "ListWorkgroups",
        json!({ "ownerAccount": "111122223333" }),
    )
    .await;
    assert_eq!(
        (status, &body["workgroups"]),
        (200, &json!([])),
        "ListWorkgroups for another account: got {status} with {body}"
    );
}

// ---------------------------------------------------------------------------
// Refusals.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_port_outside_the_documented_ranges_is_refused() {
    // `Workgroup$port`: "Valid port ranges are 5431-5455 and 8191-8215." Each published
    // bound and one step past it.
    let cases: [(i64, bool); 11] = [
        (5430, false),
        (5431, true),
        (5439, true),
        (5455, true),
        (5456, false),
        (8190, false),
        (8191, true),
        (8215, true),
        (8216, false),
        (1, false),
        (65535, false),
    ];

    let router = emulator();
    create_namespace(&router, "port-ns").await;
    create_workgroup(&router, "port-wg", "port-ns").await;

    for (index, (port, accepted)) in cases.iter().enumerate() {
        let name = format!("port-wg-{index}");
        let (status, body) = call(
            &router,
            "CreateWorkgroup",
            json!({ "workgroupName": name, "namespaceName": "port-ns", "port": port }),
        )
        .await;
        if *accepted {
            assert_eq!(
                (status, &body["workgroup"]["port"]),
                (200, &json!(port)),
                "CreateWorkgroup port {port}: got {status} with {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "CreateWorkgroup port {port}: got {status} with {body}"
            );
        }

        // The same rule holds on the update path.
        let (status, body) = call(
            &router,
            "UpdateWorkgroup",
            json!({ "workgroupName": "port-wg", "port": port }),
        )
        .await;
        if *accepted {
            assert_eq!(
                (status, &body["workgroup"]["port"]),
                (200, &json!(port)),
                "UpdateWorkgroup port {port}: got {status} with {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "UpdateWorkgroup port {port}: got {status} with {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_name_outside_the_modelled_constraint_is_refused() {
    // `NamespaceName` / `WorkgroupName`: @length(3..64) and @pattern(^[a-z0-9-]+$).
    let cases: [(&str, &str); 5] = [
        ("ab", "shorter than the 3 character minimum"),
        ("", "empty"),
        ("UPPER", "outside the lowercase pattern"),
        ("under_score", "outside the pattern's character set"),
        (
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "longer than the 64 character maximum",
        ),
    ];

    let router = emulator();
    for (name, why) in cases {
        let (status, body) = call(
            &router,
            "CreateNamespace",
            json!({ "namespaceName": name }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "CreateNamespace with a name {why}: got {status} with {body}"
        );
        let (status, body) = call(
            &router,
            "CreateWorkgroup",
            json!({ "workgroupName": name, "namespaceName": "any-namespace" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "CreateWorkgroup with a name {why}: got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn an_unknown_name_is_refused_on_every_name_addressed_operation() {
    // A resource NAME that resolves to nothing carries the modelled
    // `ResourceNotFoundException` with HTTP 400, the status this service answers a
    // name refusal with, whether the name is a namespace's or a workgroup's.
    let router = emulator();
    let cases: [(&str, Value, u16, &str); 6] = [
        (
            "GetNamespace",
            json!({ "namespaceName": "absent-ns" }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "UpdateNamespace",
            json!({ "namespaceName": "absent-ns", "logExports": ["userlog"] }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "DeleteNamespace",
            json!({ "namespaceName": "absent-ns" }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "GetWorkgroup",
            json!({ "workgroupName": "absent-wg" }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "UpdateWorkgroup",
            json!({ "workgroupName": "absent-wg" }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "DeleteWorkgroup",
            json!({ "workgroupName": "absent-wg" }),
            400,
            "ResourceNotFoundException",
        ),
    ];
    for (operation, request, expected_status, expected_code) in cases {
        let (status, body) = call(&router, operation, request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (expected_status, expected_code),
            "{operation} {request}: got {status} with {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The transition the clock owes a workgroup.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_workgroup_reaches_available_on_its_own_after_being_created_or_modified() {
    let router = emulator();
    create_namespace(&router, "settle-ns").await;

    // A caller acting straight after the create still observes the transient status.
    let created = create_workgroup(&router, "settle-wg", "settle-ns").await;
    assert_eq!(
        created["status"],
        json!("CREATING"),
        "CreateWorkgroup: got {:?}",
        created["status"]
    );
    let (_, body) = call(
        &router,
        "GetWorkgroup",
        json!({ "workgroupName": "settle-wg" }),
    )
    .await;
    assert_eq!(
        body["workgroup"]["status"],
        json!("CREATING"),
        "GetWorkgroup straight after the create: got {:?}",
        body["workgroup"]["status"]
    );

    // Nobody calls anything for the span; the clock alone settles it.
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    let (_, body) = call(
        &router,
        "GetWorkgroup",
        json!({ "workgroupName": "settle-wg" }),
    )
    .await;
    assert_eq!(
        body["workgroup"]["status"],
        json!("AVAILABLE"),
        "GetWorkgroup once the transition fell due: got {:?}",
        body["workgroup"]["status"]
    );

    // An update starts the same kind of transition, from MODIFYING.
    let (_, body) = call(
        &router,
        "UpdateWorkgroup",
        json!({ "workgroupName": "settle-wg", "baseCapacity": 64 }),
    )
    .await;
    assert_eq!(
        body["workgroup"]["status"],
        json!("MODIFYING"),
        "UpdateWorkgroup: got {:?}",
        body["workgroup"]["status"]
    );
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    let (_, body) = call(
        &router,
        "GetWorkgroup",
        json!({ "workgroupName": "settle-wg" }),
    )
    .await;
    assert_eq!(
        body["workgroup"]["status"],
        json!("AVAILABLE"),
        "GetWorkgroup once the update's transition fell due: got {:?}",
        body["workgroup"]["status"]
    );
    assert_eq!(
        body["workgroup"]["baseCapacity"],
        json!(64),
        "the update's value survives the transition, got {:?}",
        body["workgroup"]["baseCapacity"]
    );
}

#[tokio::test]
async fn the_store_is_bounded_by_the_accounts_quota_for_each_resource() {
    // AWS's "Quotas for Amazon Redshift Serverless": 25 namespaces and 25 workgroups per
    // account per Region. The quota bounds a NEW record, so re-creating an existing name
    // at the quota is still a conflict, and deleting one frees a slot.
    const QUOTA: usize = 25;
    let router = emulator();
    for index in 0..QUOTA {
        create_namespace(&router, &format!("quota-ns-{index:02}")).await;
    }

    let (status, body) = call(
        &router,
        "CreateNamespace",
        json!({ "namespaceName": "one-too-many" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "CreateNamespace at the namespace quota: got {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "CreateNamespace",
        json!({ "namespaceName": "quota-ns-00" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ConflictException"),
        "CreateNamespace of an existing name at the quota is still refused: \
         got {status} with {body}"
    );

    // Freeing a slot lets the next create through.
    let (status, _) = call(
        &router,
        "DeleteNamespace",
        json!({ "namespaceName": "quota-ns-00" }),
    )
    .await;
    assert_eq!(status, 200, "DeleteNamespace: got {status}");
    create_namespace(&router, "one-too-many").await;

    // The workgroup store carries its own quota.
    for index in 0..QUOTA {
        create_workgroup(&router, &format!("quota-wg-{index:02}"), "one-too-many").await;
    }
    let (status, body) = call(
        &router,
        "CreateWorkgroup",
        json!({ "workgroupName": "wg-too-many", "namespaceName": "one-too-many" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "CreateWorkgroup at the workgroup quota: got {status} with {body}"
    );
}

#[tokio::test]
async fn deleting_a_namespace_releases_its_name_and_its_workgroups_slot() {
    // A delete really removes the record: the same name creates again, with a NEW id.
    let router = emulator();
    let first = create_namespace(&router, "reuse-ns").await;
    let (status, _) = call(
        &router,
        "DeleteNamespace",
        json!({ "namespaceName": "reuse-ns" }),
    )
    .await;
    assert_eq!(status, 200, "DeleteNamespace: got {status}");
    let second = create_namespace(&router, "reuse-ns").await;
    assert_ne!(
        first["namespaceId"], second["namespaceId"],
        "re-creating a deleted namespace mints a new id, got {:?} twice",
        second["namespaceId"]
    );
}
