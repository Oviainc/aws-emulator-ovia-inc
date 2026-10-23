// SPDX-License-Identifier: CC0-1.0

//! The region a request addresses is the region its table is stored in and read from —
//! whichever field of its endpoint names that region.
//!
//! A request names ONE endpoint, and a region and a service name are two fields of that
//! endpoint. The framework resolves the service from the request in tiers (the SigV4
//! credential scope of the `Authorization` header, then the scope a presigned request
//! carries in `X-Amz-Credential`, then the `Host`/`:authority` endpoint label —
//! `emulator_server::resolve_route_key`), so the region this service keys its tables by
//! is resolved from the same tiers. The rule these tests pin:
//!
//! > a table is created in, and read back from, the region the request's endpoint names,
//! > in whichever of those fields it names it; a request whose endpoint names no region
//! > at all is served the emulator's default region.
//!
//! Each row of [`SIGNALS`] is one such field, and the tests cross every pair of them:
//! a `CreateTable` that arrives through one field and a `DescribeTable`/`ListTables`
//! that arrives through another must agree about which region's tables they are talking
//! about, and neither must see another region's table.
//!
//! Every call goes over the wire (`POST` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::DEFAULT_REGION;
use smithy_server_runtime::MultiServiceRouter;

// The emulator constructor and the refusal-code reader are the harness's, so there is
// one implementation of each.
use crate::emulator::{emulator, error_code};

/// How a request names the region of the endpoint it addresses.
///
/// One variant per tier the framework reads a request's endpoint from, in that order.
/// The host forms are the endpoint hosts AWS publishes for this service: the regional
/// one, its FIPS sibling and its dual-stack sibling, each of which carries the region in
/// a label of the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegionSignal {
    /// `Authorization: AWS4-HMAC-SHA256 Credential=…/<region>/dynamodb/aws4_request, …`
    SignedScope,
    /// A presigned request's `X-Amz-Credential` query parameter, which carries the same
    /// credential scope percent-encoded and no `Authorization` header at all.
    PresignedScope,
    /// `Host: dynamodb.<region>.amazonaws.com`.
    RegionalHost,
    /// `Host: dynamodb-fips.<region>.amazonaws.com`.
    FipsRegionalHost,
    /// The HTTP/2 `:authority` form of the dual-stack endpoint,
    /// `https://dynamodb.<region>.api.aws/`, carried on the URI rather than in a `Host`
    /// header.
    DualStackAuthority,
}

/// Every region field the tests span, which is every tier a region can arrive in.
const SIGNALS: [RegionSignal; 5] = [
    RegionSignal::SignedScope,
    RegionSignal::PresignedScope,
    RegionSignal::RegionalHost,
    RegionSignal::FipsRegionalHost,
    RegionSignal::DualStackAuthority,
];

impl RegionSignal {
    /// Put this region field on a request under construction.
    fn apply(self, builder: http::request::Builder, region: &str) -> http::request::Builder {
        match self {
            Self::SignedScope => builder.uri("/").header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/dynamodb/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            ),
            Self::PresignedScope => builder.uri(format!(
                "/?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIDTEST%2F20260101%2F\
                 {region}%2Fdynamodb%2Faws4_request&X-Amz-SignedHeaders=host&\
                 X-Amz-Signature=0000"
            )),
            Self::RegionalHost => builder
                .uri("/")
                .header("host", format!("dynamodb.{region}.amazonaws.com")),
            Self::FipsRegionalHost => builder
                .uri("/")
                .header("host", format!("dynamodb-fips.{region}.amazonaws.com")),
            Self::DualStackAuthority => {
                builder.uri(format!("https://dynamodb.{region}.api.aws/"))
            }
        }
    }
}

/// Send one operation whose endpoint names `region` through `signal`, and answer its
/// HTTP status and parsed body.
async fn call(
    router: &MultiServiceRouter,
    signal: RegionSignal,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let builder = http::Request::builder()
        .method("POST")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"));
    let request = signal
        .apply(builder, region)
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Send one operation that must succeed, and answer its parsed body.
async fn ok(
    router: &MultiServiceRouter,
    signal: RegionSignal,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, parsed) = call(router, signal, region, operation, body).await;
    assert_eq!(
        status, 200,
        "case {signal:?} {region} {operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The simplest `CreateTable` request body there is, under `name`: the rule under test is
/// about the region a table is keyed by, so nothing else about the table varies.
fn create_table_body(name: &str) -> Value {
    json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
        "BillingMode": "PAY_PER_REQUEST",
    })
}

/// The `TableNames` a `ListTables` response reports.
fn table_names(listed: &Value) -> Vec<String> {
    listed["TableNames"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|name| name.as_str().map(ToString::to_string))
        .collect()
}

#[tokio::test]
async fn a_table_is_read_back_in_the_region_its_endpoint_names() {
    let router = emulator();
    // Each row creates its table in a DIFFERENT region through a different field of the
    // endpoint, and names it for that field, so a read that resolves the wrong region
    // cannot find it by accident. The regions differ in their shape as well as their
    // value (two groups, three groups, and a long one), because the region is read out
    // of a host label by shape.
    let rows: [(RegionSignal, &str, &str); 5] = [
        (RegionSignal::SignedScope, "us-west-2", "SignedScopeTable"),
        (
            RegionSignal::PresignedScope,
            "eu-central-1",
            "PresignedScopeTable",
        ),
        (
            RegionSignal::RegionalHost,
            "ap-southeast-2",
            "RegionalHostTable",
        ),
        (
            RegionSignal::FipsRegionalHost,
            "us-gov-west-1",
            "FipsRegionalHostTable",
        ),
        (
            RegionSignal::DualStackAuthority,
            "sa-east-1",
            "DualStackAuthorityTable",
        ),
    ];
    // The region every row's table must be ABSENT from: it is not the region of any row.
    let elsewhere = "eu-north-1";

    for (creating, region, name) in rows {
        let created = ok(
            &router,
            creating,
            region,
            "CreateTable",
            &create_table_body(name),
        )
        .await;
        assert_eq!(
            created["TableDescription"]["TableArn"],
            json!(engine::emulator_arn(
                "dynamodb",
                region,
                &format!("table/{name}")
            )),
            "case create {creating:?} in {region}: the table's ARN names the region its \
             endpoint named, got {created}"
        );

        // Every other field of the endpoint names the same region, so every one of them
        // reads the same table back.
        for reading in SIGNALS {
            let described = ok(
                &router,
                reading,
                region,
                "DescribeTable",
                &json!({"TableName": name}),
            )
            .await;
            assert_eq!(
                described["Table"]["TableName"],
                json!(name),
                "case create {creating:?} then describe {reading:?} in {region}: the \
                 just-created table is reported back, got {described}"
            );
            assert_eq!(
                described["Table"]["TableArn"],
                created["TableDescription"]["TableArn"],
                "case create {creating:?} then describe {reading:?} in {region}: the same \
                 table, and so the same ARN, got {described}"
            );

            let listed = ok(&router, reading, region, "ListTables", &json!({})).await;
            assert!(
                table_names(&listed).contains(&name.to_string()),
                "case create {creating:?} then list {reading:?} in {region}: the region's \
                 tables include {name}, got {listed}"
            );

            // ...and a request whose endpoint names ANOTHER region is talking about
            // another region's tables, where this name is not taken.
            let (status, body) = call(
                &router,
                reading,
                elsewhere,
                "DescribeTable",
                &json!({"TableName": name}),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case describe {reading:?} in {elsewhere} a table created in {region}: a \
                 table name is unique within one region, got {status} with {body}"
            );
            let listed_elsewhere = ok(&router, reading, elsewhere, "ListTables", &json!({})).await;
            assert!(
                !table_names(&listed_elsewhere).contains(&name.to_string()),
                "case list {reading:?} in {elsewhere}: {name} was created in {region} and \
                 belongs to no other region, got {listed_elsewhere}"
            );
        }
    }
}

#[tokio::test]
async fn an_endpoint_that_names_no_region_is_served_the_default_region() {
    let router = emulator();
    // Two hosts that name no region: the legacy global endpoint (whose first label is
    // the service and whose remaining labels are not region-shaped) and a loopback
    // address a client reaches this emulator on. Neither may be read as a region, so
    // both are served the default region's store — and so see one another's tables.
    let hosts = ["dynamodb.amazonaws.com", "127.0.0.1:9297"];
    let name = "DefaultRegionTable";

    let created = host_call(&router, hosts[0], "CreateTable", &create_table_body(name)).await;
    assert_eq!(
        created.0, 200,
        "case create over {}: expected HTTP 200, got {:?}",
        hosts[0], created
    );
    assert_eq!(
        created.1["TableDescription"]["TableArn"],
        json!(engine::emulator_arn(
            "dynamodb",
            DEFAULT_REGION,
            &format!("table/{name}")
        )),
        "case create over {}: an endpoint naming no region is the default region, got {:?}",
        hosts[0],
        created.1
    );

    for host in hosts {
        let described = host_call(&router, host, "DescribeTable", &json!({"TableName": name})).await;
        assert_eq!(
            (described.0, described.1["Table"]["TableName"].clone()),
            (200, json!(name)),
            "case describe over {host}: every endpoint that names no region reads the \
             default region's tables, got {:?}",
            described.1
        );
    }

    // The same table is the one a request SIGNED for the default region sees, so the
    // host-addressed and the signed paths share one store rather than two.
    let signed = ok(
        &router,
        RegionSignal::SignedScope,
        DEFAULT_REGION,
        "DescribeTable",
        &json!({"TableName": name}),
    )
    .await;
    assert_eq!(
        signed["Table"]["TableArn"],
        created.1["TableDescription"]["TableArn"],
        "case describe signed for {DEFAULT_REGION}: the default region's store is the one \
         a region-less endpoint reached, got {signed}"
    );
}

#[tokio::test]
async fn a_credential_scope_that_names_no_region_name_is_served_the_default_region() {
    let router = emulator();
    // The region a credential scope states is used AS IT STANDS — it may name a region
    // this emulator has never heard of — but it has to be a region NAME: it becomes a
    // store key and a segment of every ARN the service mints, so a value outside a
    // region name's character set, or longer than one can be, names no region at all and
    // the request is served the default region rather than minting an ARN around the
    // caller's own text. One row per way a value is not a region name.
    let too_long = "a".repeat(65);
    let rows: [(&str, &str, &str); 5] = [
        ("an ARN delimiter", "us:east-1", "ScopeWithColon"),
        ("a path separator", "us/east-1", "ScopeWithSlash"),
        ("upper case", "US-EAST-1", "ScopeUpperCase"),
        ("an underscore", "us_east_1", "ScopeWithUnderscore"),
        (
            "longer than any region name",
            too_long.as_str(),
            "ScopeTooLong",
        ),
    ];

    for (case, scope_region, name) in rows {
        let created = ok(
            &router,
            RegionSignal::SignedScope,
            scope_region,
            "CreateTable",
            &create_table_body(name),
        )
        .await;
        assert_eq!(
            created["TableDescription"]["TableArn"],
            json!(engine::emulator_arn(
                "dynamodb",
                DEFAULT_REGION,
                &format!("table/{name}")
            )),
            "case {case:?} ({scope_region:?}): a scope that names no region name is served \
             the default region, got {created}"
        );
        // ...and the table really is in the default region's store, not in a bucket keyed
        // by the caller's own text.
        let described = ok(
            &router,
            RegionSignal::SignedScope,
            DEFAULT_REGION,
            "DescribeTable",
            &json!({"TableName": name}),
        )
        .await;
        assert_eq!(
            described["Table"]["TableName"],
            json!(name),
            "case {case:?} ({scope_region:?}): the table is the default region's, got \
             {described}"
        );
    }

    // A region no catalog knows is still a region NAME, so it is used as it stands: this
    // tier STATES the region rather than validating it against a list.
    let unknown = "zz-central-9";
    let created = ok(
        &router,
        RegionSignal::SignedScope,
        unknown,
        "CreateTable",
        &create_table_body("UnknownButWellFormedRegion"),
    )
    .await;
    assert_eq!(
        created["TableDescription"]["TableArn"],
        json!(engine::emulator_arn(
            "dynamodb",
            unknown,
            "table/UnknownButWellFormedRegion"
        )),
        "case an unknown but well-formed region: the scope states the region, got {created}"
    );
}

/// Send one operation to `host` with no credential scope at all, and answer its HTTP
/// status and parsed body. Used for the hosts that name no region, which the
/// [`RegionSignal`] rows cannot express.
async fn host_call(
    router: &MultiServiceRouter,
    host: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header("host", host)
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

