// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHEN `CreateCustomKeyStore` settles the external key store proxy
//! a request describes, driven over the wire through the shared [`super::harness`].
//!
//! The rule under test: a `CreateCustomKeyStore` request whose `CustomKeyStoreType` is
//! `EXTERNAL_KEY_STORE` is settled against the proxy DURING that call — a proxy that does
//! not answer is refused HTTP 400 `XksProxyUriUnreachableException` and the request brings
//! no custom key store into existence, so the probe is never deferred to
//! `ConnectCustomKeyStore` and a client is never handed a key store nothing serves.
//! Nothing the request SAYS changes that: the verdict is a property of whether a proxy
//! answers at the endpoint and API path, not of how either is spelled.
//!
//! [`super::external_key_store_proxy`] pins the refusal itself — its status, code and
//! sentence — by counting the records the store holds, and this suite pins what the rest
//! of the family sees afterwards, through the public API alone: the name the refused
//! request asked for resolves to no key store for `DescribeCustomKeyStores`, every value
//! that request carried stays FREE (the identical request earns the same unreachable
//! refusal rather than a name, endpoint, URI or endpoint-service collision, which a
//! half-built record would have produced), and the Region lists nothing. The expectation
//! itself, and the request builder its rows are written with, are that suite's
//! ([`super::external_key_store_proxy::UNREACHABLE_MESSAGE`],
//! [`super::external_key_store_proxy::external_request`]) rather than a second copy.
//!
//! The rows differ along what the rule generalises over: the connectivity, both ends of
//! `XksProxyUriEndpointType`'s `@length(10..128)`, both documented API path forms, whether
//! an Amazon VPC endpoint service and its owner are named, the shortest and the longest
//! friendly name `CustomKeyStoreNameType` admits, and the Region the request is signed
//! for.

use serde_json::{json, Value};

use super::external_key_store_proxy::{external_request, OTHER_REGION, UNREACHABLE_MESSAGE};
use super::harness::{call, error_code, error_message, ok_in, refused, router, REGION};

/// One row of the table: the case it stands for, the Region the request is signed for,
/// the friendly name it asks for (which is also what the reader is asked about
/// afterwards), and the request itself.
struct Row {
    case: &'static str,
    region: &'static str,
    name: String,
    request: Value,
}

impl Row {
    /// A row for an external key store on a public endpoint.
    fn public(
        case: &'static str,
        region: &'static str,
        name: String,
        endpoint: &str,
        path: &str,
    ) -> Self {
        let request = external_request(&name, "PUBLIC_ENDPOINT", endpoint, path);
        Self {
            case,
            region,
            name,
            request,
        }
    }

    /// A row for an external key store reached through an Amazon VPC endpoint service,
    /// whose owner the request names only when `owner` is `Some`: "if not provided, the
    /// Amazon Web Services account ID calling the action will be used"
    /// (`smithy_query(query="doc",shape="CreateCustomKeyStoreRequest",member="XksProxyVpcEndpointServiceOwner")`).
    fn through_endpoint_service(
        case: &'static str,
        region: &'static str,
        name: String,
        endpoint: &str,
        path: &str,
        service_name: &str,
        owner: Option<&str>,
    ) -> Self {
        let mut request = external_request(&name, "VPC_ENDPOINT_SERVICE", endpoint, path);
        request["XksProxyVpcEndpointServiceName"] = json!(service_name);
        if let Some(owner) = owner {
            request["XksProxyVpcEndpointServiceOwner"] = json!(owner);
        }
        Self {
            case,
            region,
            name,
            request,
        }
    }
}

/// An endpoint of exactly `length` characters: `https://` followed by a host of
/// `[a-zA-Z0-9.-]`, which is what `XksProxyUriEndpointType` admits
/// (`smithy_query(query="shape",shape="XksProxyUriEndpointType")` —
/// `^https://[a-zA-Z0-9.-]+$`, `@length(10..128)`). A length that leaves no room for a
/// host is not an endpoint of that shape, so it is a mistake in the caller.
fn endpoint_of_length(length: usize) -> String {
    const SCHEME: &str = "https://";
    assert!(
        length > SCHEME.len(),
        "case endpoint_of_length({length}): an endpoint needs a host after {SCHEME:?}"
    );
    format!("{SCHEME}{}", "a".repeat(length - SCHEME.len()))
}

/// The table. Every row is a request whose own properties are valid — each value is one
/// the modelled constraints admit — so every row gets as far as the proxy, which is what
/// this suite is about.
fn rows() -> Vec<Row> {
    vec![
        Row::public(
            "the documented public endpoint and API path",
            REGION,
            "xks-create-probe".to_string(),
            "https://xks.example.com",
            "/kms/xks/v1",
        ),
        Row::public(
            "the shortest endpoint the model admits, and the shortest friendly name",
            REGION,
            // `CustomKeyStoreNameType` is `@length(1..256)`
            // (`smithy_query(query="shape",shape="CustomKeyStoreNameType")`).
            "x".to_string(),
            &endpoint_of_length(10),
            "/kms/xks/v9",
        ),
        Row::public(
            "the longest endpoint the model admits, and the longest friendly name",
            OTHER_REGION,
            "n".repeat(256),
            &endpoint_of_length(128),
            // `^(/[a-zA-Z0-9\/_-]+/kms/xks/v\d{1,2})$|^(/kms/xks/v\d{1,2})$`
            // (`smithy_query(query="shape",shape="XksProxyUriPathType")`), here in its
            // prefixed form at the longest version the pattern admits.
            "/edge_proxy-1/south/kms/xks/v99",
        ),
        Row::through_endpoint_service(
            "an endpoint service in another Region, owned by another account",
            OTHER_REGION,
            "xks-create-probe-vpce".to_string(),
            "https://xks-vpce.example.com",
            "/example/kms/xks/v12",
            "eu.amazonaws.vpce.eu-west-2.vpce-svc-1234567890abcdef0",
            Some("111122223333"),
        ),
    ]
}

/// The external key store a request describes is settled with the proxy before the call
/// answers: an unreachable proxy is refused, and the whole family then agrees that
/// nothing was created and that every value the request named is still free.
#[tokio::test]
async fn an_unreachable_proxy_is_settled_during_create_and_leaves_the_family_empty() {
    let router = router();

    for row in rows() {
        let case = row.case;

        let (status, payload) =
            call(&router, row.region, "CreateCustomKeyStore", &row.request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "XksProxyUriUnreachableException",
            "case {case:?}: a proxy reached during the create and not answering is the \
             unreachable refusal, got {payload}"
        );
        assert_eq!(
            error_message(&payload),
            UNREACHABLE_MESSAGE,
            "case {case:?}: expected the fixed sentence of the class, got {:?}",
            error_message(&payload)
        );
        assert!(
            payload.get("CustomKeyStoreId").is_none(),
            "case {case:?}: a refused create hands out no id, got {payload}"
        );

        // The reader resolves the name the refused request asked for to NO key store, so
        // a client cannot act on a store the create never made.
        let (read_status, read_code, read_message) = refused(
            &router,
            row.region,
            "DescribeCustomKeyStores",
            &json!({"CustomKeyStoreName": row.name}),
        )
        .await;
        assert_eq!(
            (read_status, read_code.as_str()),
            (400, "CustomKeyStoreNotFoundException"),
            "case {case:?}: the refused name names no key store, got {read_status} \
             ({read_code})"
        );
        assert!(
            read_message.is_empty(),
            "case {case:?}: that refusal carries the error code alone, got {read_message:?}"
        );

        // Nothing the refused request named was taken: the identical request earns the
        // SAME unreachable refusal, where a half-built record would have made its name,
        // endpoint, URI or endpoint service collide instead.
        let (again_status, again_payload) =
            call(&router, row.region, "CreateCustomKeyStore", &row.request).await;
        assert_eq!(
            (again_status, error_code(&again_payload).as_str()),
            (400, "XksProxyUriUnreachableException"),
            "case {case:?}: the request's values are still free, so the same request is \
             refused the same way, got {again_status} with {again_payload}"
        );
    }

    // Every Region the table signed for lists no custom key store at all.
    for region in [REGION, OTHER_REGION] {
        let listed = ok_in(&router, region, "DescribeCustomKeyStores", &json!({})).await;
        let entries = listed["CustomKeyStores"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            entries.is_empty(),
            "case {region:?}: no refused create may leave a key store behind, got {listed}"
        );
    }
}

/// The verdict is the proxy's, not the request's: the same endpoint and API path asked for
/// under another friendly name and another connectivity is refused identically, and so is
/// a request that names a different endpoint and path — no spelling of either is treated
/// as reachable.
#[tokio::test]
async fn the_unreachable_verdict_does_not_depend_on_how_the_request_is_spelled() {
    let router = router();

    let variants: Vec<(&str, Value)> = vec![
        (
            "a proxy on a public endpoint",
            external_request(
                "xks-spelling-public",
                "PUBLIC_ENDPOINT",
                "https://xks-spelling.example.com",
                "/kms/xks/v1",
            ),
        ),
        (
            "the same endpoint and path, asked for through an endpoint service",
            {
                let mut request = external_request(
                    "xks-spelling-vpce",
                    "VPC_ENDPOINT_SERVICE",
                    "https://xks-spelling.example.com",
                    "/kms/xks/v1",
                );
                request["XksProxyVpcEndpointServiceName"] =
                    json!("com.amazonaws.vpce.us-east-1.vpce-svc-0fedcba9876543210");
                request
            },
        ),
        (
            "the same host written with a trailing dot",
            external_request(
                "xks-spelling-dotted",
                "PUBLIC_ENDPOINT",
                "https://xks-spelling.example.com.",
                "/kms/xks/v1",
            ),
        ),
        (
            "another endpoint, on the other documented path form",
            external_request(
                "xks-spelling-other",
                "PUBLIC_ENDPOINT",
                "https://other-proxy.example.org",
                "/tenant-7/kms/xks/v2",
            ),
        ),
    ];

    for (case, request) in variants {
        let (status, payload) = call(&router, REGION, "CreateCustomKeyStore", &request).await;
        assert_eq!(
            (
                status,
                error_code(&payload).as_str(),
                error_message(&payload).as_str()
            ),
            (400, "XksProxyUriUnreachableException", UNREACHABLE_MESSAGE),
            "case {case:?}: how the request is spelled decides nothing, got {status} with \
             {payload}"
        );
    }

    let listed = ok_in(&router, REGION, "DescribeCustomKeyStores", &json!({})).await;
    assert!(
        listed["CustomKeyStores"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .is_empty(),
        "case after every refusal: the Region holds no custom key store, got {listed}"
    );
}
