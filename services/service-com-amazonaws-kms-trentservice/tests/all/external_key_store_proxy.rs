// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the external key store proxy `CreateCustomKeyStore` has to
//! reach, driven over the wire through the shared [`super::harness`].
//!
//! The rule under test: a custom key store of type `EXTERNAL_KEY_STORE` is built over an
//! external key store proxy, and KMS reaches that proxy DURING the call — a proxy that
//! does not answer is refused HTTP 400 `XksProxyUriUnreachableException`, carrying the
//! fixed sentence of that class, and no key store is created. It is the one pre-flight
//! check a client has that an external key store is wired up, so a success has to mean
//! the proxy answered.
//!
//! No external key manager runs in this process and nothing here opens a connection out
//! of it, so no endpoint a request can name has a proxy behind it: every external key
//! store create is refused, and `DescribeCustomKeyStores` shows that nothing was
//! created. What the request SAYS cannot change that, which is what the rows are about:
//! they differ in the connectivity, the endpoint host, the API path (both documented
//! forms, with and without a prefix segment, and both ends of the `v\d{1,2}` version
//! the path's pattern admits), whether an Amazon VPC endpoint service and its owner are
//! named, the friendly name, and the Region the request is signed for.
//!
//! The refusals a request earns for its own properties — a missing member, a value
//! another key store holds — come BEFORE the proxy is reached and are
//! [`super::create_custom_key_store`]'s rule.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kms_trentservice::state::service_state;

use super::harness::{call, error_code, error_message, ok_in, router_with_engine, REGION};

/// The whole `message` KMS answers when it cannot reach the external key store proxy a
/// request describes: fixed text for the class, into which no endpoint, API path or key
/// store name is interpolated. Written out here so the assertion is about AWS's sentence
/// rather than about the one the handler holds. Shared with
/// [`super::external_key_store_create_probe`], which asserts the same sentence about what
/// the rest of the family then sees.
pub(crate) const UNREACHABLE_MESSAGE: &str = "AWS KMS cannot communicate with the external key store \
     proxy. This might be a transient network issue. If you see this error repeatedly, \
     verify that your external key store proxy is active and is connected to the network, \
     and that its endpoint URI is correct in your external key store.";

/// The Region a row is signed for when the row is about regional scoping.
pub(crate) const OTHER_REGION: &str = "eu-west-2";

/// An `XksProxyAuthenticationCredential` access key id: 20..30 characters of
/// `^[A-Z2-7]+$` (`smithy_query(query="shape",shape="XksProxyAuthenticationCredentialType")`).
const PROXY_ACCESS_KEY_ID: &str = "ABCDEFGHIJKLMNOPQRST";

/// Its raw secret access key: 43..64 characters of `^[a-zA-Z0-9\/+=]+$` (the same
/// shape).
const PROXY_SECRET: &str = "c2VjcmV0L3Byb3h5K2NyZWRlbnRpYWw9PTAxMjM0NTY3ODk=";

/// One row of the table: the case it stands for, the Region the request is signed for,
/// and the external key store the request asks for.
struct Row {
    case: &'static str,
    region: &'static str,
    request: Value,
}

/// A `CreateCustomKeyStore` request for an external key store, with the proxy members a
/// request of that type must carry
/// (`smithy_query(query="doc",shape="CreateCustomKeyStore")`). The ONE builder both this
/// suite and [`super::external_key_store_create_probe`] write their rows with.
pub(crate) fn external_request(
    name: &str,
    connectivity: &str,
    uri_endpoint: &str,
    uri_path: &str,
) -> Value {
    json!({
        "CustomKeyStoreName": name,
        "CustomKeyStoreType": "EXTERNAL_KEY_STORE",
        "XksProxyConnectivity": connectivity,
        "XksProxyUriEndpoint": uri_endpoint,
        "XksProxyUriPath": uri_path,
        "XksProxyAuthenticationCredential": {
            "AccessKeyId": PROXY_ACCESS_KEY_ID,
            "RawSecretAccessKey": PROXY_SECRET,
        },
    })
}

/// The table. The endpoints and API paths are ones `XksProxyUriEndpoint`'s
/// `^https://[a-zA-Z0-9.-]+$` and `XksProxyUriPath`'s
/// `^(/[a-zA-Z0-9\/_-]+/kms/xks/v\d{1,2})$|^(/kms/xks/v\d{1,2})$` admit
/// (`smithy_query(query="shape",shape="XksProxyUriEndpoint")` and
/// `smithy_query(query="shape",shape="XksProxyUriPathType")`), so every row is a request
/// whose own properties are valid and which therefore reaches the proxy.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a public endpoint on the documented API path",
            region: REGION,
            request: external_request(
                "xks-public",
                "PUBLIC_ENDPOINT",
                "https://xks.example.com",
                "/kms/xks/v1",
            ),
        },
        Row {
            case: "a public endpoint on a prefixed API path with the longest version",
            region: REGION,
            request: external_request(
                "xks-public-prefixed",
                "PUBLIC_ENDPOINT",
                "https://proxy-01.xks.example.com",
                "/prefix/kms/xks/v99",
            ),
        },
        Row {
            case: "a public endpoint whose host is a bare label",
            region: OTHER_REGION,
            request: external_request(
                "xks-public-bare-host",
                "PUBLIC_ENDPOINT",
                "https://proxy",
                "/kms/xks/v2",
            ),
        },
        Row {
            case: "a VPC endpoint service, with its owner left to the calling account",
            region: REGION,
            request: {
                let mut request = external_request(
                    "xks-vpce",
                    "VPC_ENDPOINT_SERVICE",
                    "https://xks-vpce.example.com",
                    "/kms/xks/v1",
                );
                request["XksProxyVpcEndpointServiceName"] =
                    json!("com.amazonaws.vpce.us-east-1.vpce-svc-0abcdef1234567890");
                request
            },
        },
        Row {
            case: "a VPC endpoint service owned by another account",
            region: OTHER_REGION,
            request: {
                let mut request = external_request(
                    "xks-vpce-owned",
                    "VPC_ENDPOINT_SERVICE",
                    "https://xks-vpce-two.example.com",
                    "/example/kms/xks/v12",
                );
                request["XksProxyVpcEndpointServiceName"] =
                    json!("eu.amazonaws.vpce.eu-west-2.vpce-svc-1234567890abcdef0");
                request["XksProxyVpcEndpointServiceOwner"] = json!("111122223333");
                request
            },
        },
    ]
}

/// How many custom key stores the store holds, across every Region — what a refused
/// create must leave exactly as it was.
fn held(engine: &Arc<engine::InMemoryEngine>) -> usize {
    let state = service_state(&**engine).expect("the KMS state store is reachable");
    let held = state.lock();
    held.custom_key_stores.len()
}

/// Every external key store `DescribeCustomKeyStores` reports in `region`, by name.
async fn reported_names(router: &MultiServiceRouter, region: &str) -> Vec<String> {
    let answer = ok_in(router, region, "DescribeCustomKeyStores", &json!({})).await;
    answer["CustomKeyStores"]
        .as_array()
        .unwrap_or_else(|| panic!("case {region}: expected a CustomKeyStores list in {answer}"))
        .iter()
        .filter_map(|entry| entry["CustomKeyStoreName"].as_str().map(str::to_string))
        .collect()
}

/// An external key store over a proxy that does not answer is refused, with the fixed
/// sentence of its class, and nothing is created.
#[tokio::test]
async fn an_external_key_store_over_an_unreachable_proxy_is_refused() {
    let (router, engine) = router_with_engine();
    let before = held(&engine);

    for row in rows() {
        let (status, payload) = call(&router, row.region, "CreateCustomKeyStore", &row.request).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} with {payload}",
            row.case
        );
        assert_eq!(
            error_code(&payload),
            "XksProxyUriUnreachableException",
            "case {:?}: a proxy that does not answer is the unreachable refusal, got {payload}",
            row.case
        );
        assert_eq!(
            error_message(&payload),
            UNREACHABLE_MESSAGE,
            "case {:?}: expected the fixed sentence of the class, got {:?}",
            row.case,
            error_message(&payload)
        );
        assert!(
            payload.get("CustomKeyStoreId").is_none(),
            "case {:?}: a refused create hands out no id, got {payload}",
            row.case
        );
        assert_eq!(
            held(&engine),
            before,
            "case {:?}: a refused create must persist nothing",
            row.case
        );
    }

    // The reader agrees: no key store of either Region's endpoint was created, so a
    // client that treated a create as "the proxy is wired up" has nothing to connect.
    for region in [REGION, OTHER_REGION] {
        let reported = reported_names(&router, region).await;
        assert!(
            reported.is_empty(),
            "case {region}: no external key store was created, got {reported:?}"
        );
    }
}

/// The same request made again is refused the same way: the probe decides nothing about
/// the store, so a retry neither creates a key store nor changes the refusal.
#[tokio::test]
async fn a_retried_external_key_store_create_is_refused_the_same_way() {
    let (router, engine) = router_with_engine();
    let before = held(&engine);
    let request = external_request(
        "xks-retried",
        "PUBLIC_ENDPOINT",
        "https://xks-retried.example.com",
        "/kms/xks/v1",
    );

    for attempt in 0..3 {
        let case = format!("attempt {attempt}");
        let (status, payload) = call(&router, REGION, "CreateCustomKeyStore", &request).await;
        assert_eq!(
            (status, error_code(&payload).as_str(), error_message(&payload).as_str()),
            (
                400,
                "XksProxyUriUnreachableException",
                UNREACHABLE_MESSAGE
            ),
            "case {case:?}: every attempt is refused the same way, got {status} with {payload}"
        );
        assert_eq!(
            held(&engine),
            before,
            "case {case:?}: a refused create must persist nothing"
        );
    }
}
