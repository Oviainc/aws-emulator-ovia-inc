// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateCustomKeyStore`, driven over the wire through the
//! shared [`super::harness`].
//!
//! The rule under test: `UpdateCustomKeyStore` applies to the custom key store its
//! `CustomKeyStoreId` names every property the request carries, answers the empty 200
//! it documents ("this operation does not return any property values"), and leaves the
//! change to be read back through `DescribeCustomKeyStores`. Whether a property may be
//! changed at all depends on the connection state the key store is in, and each
//! property's OWN documentation says which: a property documented "to change this
//! value, the <CloudHSM|external> key store must be disconnected" is refused with
//! `CustomKeyStoreInvalidStateException` while the `ConnectionState` is anything other
//! than `DISCONNECTED`, and a property documented as changeable while the key store "is
//! connected or disconnected" is applied in every connection state. A refused update
//! changes nothing at all, and the friendly name stays unique in the account.
//!
//! Every assertion is about that rule over a TABLE whose rows differ in the property
//! the request changes (the friendly name, the CloudHSM cluster, the kmsuser password,
//! the proxy URI endpoint and path, the proxy connectivity, the VPC endpoint service
//! name and owner, the proxy authentication credential, and no property at all),
//! crossed with the connection state the key store is in — every
//! `ConnectionStateType` value the model declares, from the generated
//! `CONNECTION_STATE_TYPE_VALUES` set — and, for the friendly name, in the length and
//! the holder of the name (the shortest and the longest the modelled length admits,
//! one past it, a name another key store of the Region holds, and one a key store of
//! another Region of the same account holds).
//!
//! The rows seed the records DIRECTLY into the store — the same records a create leaves
//! behind — so a row fixes the connection state and the Region it is about without a
//! create per row, and then drive the operation over the wire, exactly as
//! [`super::custom_key_stores`] does. What a create itself answers is
//! [`super::create_custom_key_store`].

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kms_trentservice::ops::types::CONNECTION_STATE_TYPE_VALUES;
use service_com_amazonaws_kms_trentservice::state::{
    mint_custom_key_store_id, service_state, CloudHsmCluster, CustomKeyStore,
    CustomKeyStoreBacking, XksProxy,
};

use super::harness::{call, error_code, ok, ok_in, router_with_engine, REGION};

/// The Region a row belongs to when the row is about the account-wide scope of the
/// friendly name: a custom key store is regional, but its name "must be unique in the
/// Amazon Web Services account"
/// (`smithy_query(query="doc",shape="UpdateCustomKeyStoreRequest",member="NewCustomKeyStoreName")`).
const OTHER_REGION: &str = "eu-west-2";

/// The connection state a freshly created custom key store is in, and the one every
/// property may be changed in.
const DISCONNECTED: &str = "DISCONNECTED";

/// The longest friendly name `CustomKeyStoreNameType` admits
/// (`smithy_query(query="shape",shape="CustomKeyStoreNameType")` — `@length(1..256)`).
const MAX_NAME_LEN: usize = 256;

/// What backs the key store a row is about, which also decides which properties it can
/// hold at all.
enum Backing {
    /// An CloudHSM cluster.
    CloudHsm,
    /// An external key store proxy reached on a public endpoint.
    PublicProxy,
    /// An external key store proxy reached through a VPC endpoint service.
    VpcProxy,
}

/// One row: the property change, the key store it is made on, whether the property's
/// documentation requires a disconnected key store, and the members the reported record
/// must carry afterwards (as JSON pointers into the `DescribeCustomKeyStores` entry).
/// Every other member must be exactly what it was.
struct Change {
    case: &'static str,
    backing: Backing,
    request: Value,
    requires_disconnected: bool,
    expected: &'static [(&'static str, &'static str)],
}

/// The table of property changes.
fn changes() -> Vec<Change> {
    vec![
        Change {
            // "To change this value, the custom key store can be connected or
            // disconnected."
            case: "the friendly name",
            backing: Backing::CloudHsm,
            request: json!({"NewCustomKeyStoreName": "renamed-key-store"}),
            requires_disconnected: false,
            expected: &[("/CustomKeyStoreName", "renamed-key-store")],
        },
        Change {
            case: "no property at all",
            backing: Backing::CloudHsm,
            request: json!({}),
            requires_disconnected: false,
            expected: &[],
        },
        Change {
            // "To change this value, the CloudHSM key store must be disconnected."
            case: "the related CloudHSM cluster",
            backing: Backing::CloudHsm,
            request: json!({"CloudHsmClusterId": "cluster-5kq2jmzvb7tx"}),
            requires_disconnected: true,
            expected: &[("/CloudHsmClusterId", "cluster-5kq2jmzvb7tx")],
        },
        Change {
            // "To change this value, the CloudHSM key store must be disconnected." No
            // reader reports the kmsuser password, so a successful change leaves the
            // reported record exactly as it was.
            case: "the kmsuser password",
            backing: Backing::CloudHsm,
            request: json!({"KeyStorePassword": "kmsuser-pw"}),
            requires_disconnected: true,
            expected: &[],
        },
        Change {
            // "You can change this value when the external key store is connected or
            // disconnected."
            case: "the proxy API path",
            backing: Backing::PublicProxy,
            request: json!({"XksProxyUriPath": "/example/kms/xks/v2"}),
            requires_disconnected: false,
            expected: &[("/XksProxyConfiguration/UriPath", "/example/kms/xks/v2")],
        },
        Change {
            // "You can change this value when the external key store is connected or
            // disconnected." The access key id is the half the reader reports; the
            // secret is held by the proxy, not by KMS.
            case: "the proxy authentication credential",
            backing: Backing::PublicProxy,
            request: json!({
                "XksProxyAuthenticationCredential": {
                    "AccessKeyId": "UVWXYZ234567ABCDEFGH",
                    "RawSecretAccessKey": "c2VjcmV0LXRoZS1wcm94eS1ob2xkcy1hbmQta21zLWtub3dz",
                },
            }),
            requires_disconnected: false,
            expected: &[(
                "/XksProxyConfiguration/AccessKeyId",
                "UVWXYZ234567ABCDEFGH",
            )],
        },
        Change {
            // "To change this value, the external key store must be disconnected."
            case: "the proxy URI endpoint",
            backing: Backing::PublicProxy,
            request: json!({"XksProxyUriEndpoint": "https://xks-moved.example.com"}),
            requires_disconnected: true,
            expected: &[(
                "/XksProxyConfiguration/UriEndpoint",
                "https://xks-moved.example.com",
            )],
        },
        Change {
            // "To change this value, the external key store must be disconnected." A
            // connectivity change to the VPC endpoint service also names the endpoint
            // service the proxy is then reached through, and the endpoint it is reached
            // at.
            case: "the proxy connectivity",
            backing: Backing::PublicProxy,
            request: json!({
                "XksProxyConnectivity": "VPC_ENDPOINT_SERVICE",
                "XksProxyUriEndpoint": "https://xks-vpce.example.com",
                "XksProxyVpcEndpointServiceName":
                    "com.amazonaws.vpce.us-east-1.vpce-svc-0abcdef1234567890",
            }),
            requires_disconnected: true,
            expected: &[
                ("/XksProxyConfiguration/Connectivity", "VPC_ENDPOINT_SERVICE"),
                (
                    "/XksProxyConfiguration/UriEndpoint",
                    "https://xks-vpce.example.com",
                ),
                (
                    "/XksProxyConfiguration/VpcEndpointServiceName",
                    "com.amazonaws.vpce.us-east-1.vpce-svc-0abcdef1234567890",
                ),
            ],
        },
        Change {
            // "To change this value, the external key store must be disconnected."
            case: "the VPC endpoint service name",
            backing: Backing::VpcProxy,
            request: json!({
                "XksProxyVpcEndpointServiceName":
                    "com.amazonaws.vpce.us-east-1.vpce-svc-1234567890abcdef0",
            }),
            requires_disconnected: true,
            expected: &[(
                "/XksProxyConfiguration/VpcEndpointServiceName",
                "com.amazonaws.vpce.us-east-1.vpce-svc-1234567890abcdef0",
            )],
        },
        Change {
            // "To change this value, the external key store must be disconnected."
            case: "the VPC endpoint service owner",
            backing: Backing::VpcProxy,
            request: json!({"XksProxyVpcEndpointServiceOwner": "444455556666"}),
            requires_disconnected: true,
            expected: &[(
                "/XksProxyConfiguration/VpcEndpointServiceOwner",
                "444455556666",
            )],
        },
    ]
}

/// A custom key store record of `backing`, in `connection_state`, as the service holds
/// one.
fn record(backing: &Backing, region: &str, name: &str, connection_state: &str) -> CustomKeyStore {
    let backing = match backing {
        Backing::CloudHsm => CustomKeyStoreBacking::CloudHsm(CloudHsmCluster {
            cloud_hsm_cluster_id: "cluster-2vkpqmfjzck".to_string(),
            trust_anchor_certificate:
                "-----BEGIN CERTIFICATE-----\nZmlyc3Q=\n-----END CERTIFICATE-----".to_string(),
        }),
        Backing::PublicProxy => CustomKeyStoreBacking::External(XksProxy {
            connectivity: "PUBLIC_ENDPOINT".to_string(),
            access_key_id: "ABCDEFGHIJKLMNOPQRST".to_string(),
            uri_endpoint: "https://xks.example.com".to_string(),
            uri_path: "/kms/xks/v1".to_string(),
            vpc_endpoint_service_name: None,
            vpc_endpoint_service_owner: None,
        }),
        Backing::VpcProxy => CustomKeyStoreBacking::External(XksProxy {
            connectivity: "VPC_ENDPOINT_SERVICE".to_string(),
            access_key_id: "GHIJKLMNOPQRSTUVWXYZ".to_string(),
            uri_endpoint: "https://xks-private.example.com".to_string(),
            uri_path: "/kms/xks/v1".to_string(),
            vpc_endpoint_service_name: Some(
                "com.amazonaws.vpce.us-east-1.vpce-svc-00000000000000000".to_string(),
            ),
            vpc_endpoint_service_owner: Some("111122223333".to_string()),
        }),
    };
    let mut store = CustomKeyStore::new(
        mint_custom_key_store_id().expect("the platform produces entropy for a key store id"),
        region.to_string(),
        name.to_string(),
        backing,
    );
    store.connection_state = connection_state.to_string();
    store
}

/// Seed key store records into the store the router was built over, in order.
fn seed(engine: &Arc<engine::InMemoryEngine>, stores: &[CustomKeyStore]) {
    let state = service_state(&**engine).expect("the KMS state store is reachable");
    let mut state = state.lock();
    for store in stores {
        state.insert_custom_key_store(store.clone());
    }
}

/// The entry `DescribeCustomKeyStores` reports for one key store.
async fn reported(router: &MultiServiceRouter, custom_key_store_id: &str) -> Value {
    let answer = ok(
        router,
        "DescribeCustomKeyStores",
        &json!({"CustomKeyStoreId": custom_key_store_id}),
    )
    .await;
    answer["CustomKeyStores"][0].clone()
}

/// Call `UpdateCustomKeyStore` in `region`, requiring a failure, and report its status
/// and wire error code.
async fn refused_in(router: &MultiServiceRouter, region: &str, request: &Value) -> (u16, String) {
    let (status, payload) = call(router, region, "UpdateCustomKeyStore", request).await;
    assert_ne!(
        status, 200,
        "case {request}: expected a failure, got HTTP 200 with {payload}"
    );
    (status, error_code(&payload))
}

/// Call `UpdateCustomKeyStore`, requiring the empty 200 it documents.
async fn updated(router: &MultiServiceRouter, case: &str, request: &Value) {
    let answer = ok_in(router, REGION, "UpdateCustomKeyStore", request).await;
    let empty = answer.is_null() || answer.as_object().is_some_and(|members| members.is_empty());
    assert!(
        empty,
        "case {case}: UpdateCustomKeyStore returns no property values, got {answer}"
    );
}

/// Set `pointer` (a JSON pointer into a reported entry, such as
/// `/XksProxyConfiguration/UriPath`), INSERTING the member when the record did not
/// carry one before.
fn set_member(case: &str, entry: &mut Value, pointer: &str, value: &str) {
    let (parent, member) = pointer
        .rsplit_once('/')
        .unwrap_or_else(|| panic!("case {case}: {pointer} is not a JSON pointer"));
    let Some(target) = entry.pointer_mut(parent) else {
        panic!("case {case}: expected {pointer} to name a member of the reported entry");
    };
    let Some(members) = target.as_object_mut() else {
        panic!("case {case}: expected an object at {parent} of the reported entry");
    };
    members.insert(member.to_string(), json!(value));
}

/// Each property is applied — or refused as a connection-state failure — exactly as its
/// own documentation says, in every connection state the model declares, and nothing
/// else about the key store changes either way.
#[tokio::test]
async fn update_custom_key_store_applies_each_property_in_the_states_it_documents() {
    for connection_state in CONNECTION_STATE_TYPE_VALUES {
        for change in changes() {
            let case = format!("{} of a {connection_state} key store", change.case);
            // A fresh store per row, so no row observes what another row changed and no
            // two key stores contend for one proxy URI or friendly name.
            let (router, engine) = router_with_engine();
            let store = record(&change.backing, REGION, "seeded", connection_state);
            let custom_key_store_id = store.custom_key_store_id.clone();
            seed(&engine, &[store]);

            let before = reported(&router, &custom_key_store_id).await;
            let mut request = change.request.clone();
            request["CustomKeyStoreId"] = json!(custom_key_store_id);

            if change.requires_disconnected && *connection_state != DISCONNECTED {
                let (status, code) = refused_in(&router, REGION, &request).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, "CustomKeyStoreInvalidStateException"),
                    "case {case}: expected HTTP 400 CustomKeyStoreInvalidStateException, got \
                     {status} {code}"
                );
                let after = reported(&router, &custom_key_store_id).await;
                assert_eq!(
                    after, before,
                    "case {case}: a refused update changes nothing, got {after}"
                );
                continue;
            }

            updated(&router, &case, &request).await;
            let mut want = before.clone();
            for (pointer, value) in change.expected {
                set_member(&case, &mut want, pointer, value);
            }
            let after = reported(&router, &custom_key_store_id).await;
            assert_eq!(
                after, want,
                "case {case}: expected exactly the properties the request names to change, \
                 got {after}"
            );
            // Updating a property is not a connection change: the key store keeps the
            // connection state it was in.
            assert_eq!(
                after["ConnectionState"].as_str().unwrap_or_default(),
                *connection_state,
                "case {case}: an update moves no connection state, got {after}"
            );
        }
    }
}

/// The friendly name is unique in the ACCOUNT: a name another key store of the account
/// holds is refused, in this Region or any other, while the name a key store already
/// has is not a conflict with itself, and the name is bounded by the modelled length.
#[tokio::test]
async fn update_custom_key_store_keeps_the_friendly_name_unique_in_the_account() {
    let (router, engine) = router_with_engine();
    let held = record(&Backing::CloudHsm, REGION, "primary", DISCONNECTED);
    let twin = record(&Backing::CloudHsm, REGION, "taken-here", DISCONNECTED);
    let elsewhere = record(&Backing::CloudHsm, OTHER_REGION, "taken-there", DISCONNECTED);
    let custom_key_store_id = held.custom_key_store_id.clone();
    seed(&engine, &[held, twin, elsewhere]);

    // Rows: the name a rename asks for, and what must come of it. They differ in who
    // holds the name and in the length of it.
    let rows: Vec<(&str, String, Option<&str>)> = vec![
        (
            "a name another key store of this Region holds",
            "taken-here".to_string(),
            Some("CustomKeyStoreNameInUseException"),
        ),
        (
            "a name a key store of another Region of the account holds",
            "taken-there".to_string(),
            Some("CustomKeyStoreNameInUseException"),
        ),
        ("the name it already has", "primary".to_string(), None),
        ("the shortest name the model admits", "a".to_string(), None),
        (
            "the longest name the model admits",
            "n".repeat(MAX_NAME_LEN),
            None,
        ),
        (
            "one character past the longest",
            "n".repeat(MAX_NAME_LEN + 1),
            Some("ValidationException"),
        ),
        (
            "the empty name",
            String::new(),
            Some("ValidationException"),
        ),
    ];
    let mut current = "primary".to_string();
    for (case, new_name, expected_code) in rows {
        let request = json!({
            "CustomKeyStoreId": custom_key_store_id,
            "NewCustomKeyStoreName": new_name,
        });
        match expected_code {
            Some(expected) => {
                let (status, code) = refused_in(&router, REGION, &request).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected),
                    "case {case}: expected HTTP 400 {expected}, got {status} {code}"
                );
                let entry = reported(&router, &custom_key_store_id).await;
                assert_eq!(
                    entry["CustomKeyStoreName"].as_str().unwrap_or_default(),
                    current,
                    "case {case}: a refused rename leaves the name as it was, got {entry}"
                );
            }
            None => {
                updated(&router, case, &request).await;
                let entry = reported(&router, &custom_key_store_id).await;
                assert_eq!(
                    entry["CustomKeyStoreName"].as_str().unwrap_or_default(),
                    new_name,
                    "case {case}: expected the reported name to be the new one, got {entry}"
                );
                // The reader finds the key store under the name it now has, so the
                // rename is what a later caller resolves by.
                let by_name = ok(
                    &router,
                    "DescribeCustomKeyStores",
                    &json!({"CustomKeyStoreName": new_name}),
                )
                .await;
                assert_eq!(
                    by_name["CustomKeyStores"][0]["CustomKeyStoreId"]
                        .as_str()
                        .unwrap_or_default(),
                    custom_key_store_id,
                    "case {case}: the renamed key store answers to its new name, got {by_name}"
                );
                current = new_name;
            }
        }
    }

    // The key store whose name a refused rename asked for is untouched, in its own
    // Region.
    let there = ok_in(
        &router,
        OTHER_REGION,
        "DescribeCustomKeyStores",
        &json!({"CustomKeyStoreName": "taken-there"}),
    )
    .await;
    assert_eq!(
        there["CustomKeyStores"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "case after the refusals: the key store of the other Region still holds its name, \
         got {there}"
    );
}

/// A connectivity change to the VPC endpoint service is applied only when the key store
/// is left naming the endpoint service it would then be reached through — the one the
/// request names, or the one the key store already holds.
#[tokio::test]
async fn update_custom_key_store_requires_an_endpoint_service_for_vpc_connectivity() {
    const SERVICE_NAME: &str = "com.amazonaws.vpce.us-east-1.vpce-svc-0abcdef1234567890";

    let (router, engine) = router_with_engine();
    let public = record(&Backing::PublicProxy, REGION, "xks-public", DISCONNECTED);
    // A proxy on a public endpoint that already names an endpoint service: the reader
    // does not report the name while the connectivity is PUBLIC_ENDPOINT, but the record
    // holds it, so a connectivity change needs nothing more.
    let mut named = record(&Backing::VpcProxy, REGION, "xks-named", DISCONNECTED);
    if let CustomKeyStoreBacking::External(proxy) = &mut named.backing {
        proxy.connectivity = "PUBLIC_ENDPOINT".to_string();
        proxy.uri_endpoint = "https://xks-named.example.com".to_string();
    }
    let public_id = public.custom_key_store_id.clone();
    let named_id = named.custom_key_store_id.clone();
    seed(&engine, &[public, named]);

    // Row 1: the connectivity change that leaves no endpoint service is refused, and
    // changes nothing.
    let before = reported(&router, &public_id).await;
    let (status, code) = refused_in(
        &router,
        REGION,
        &json!({
            "CustomKeyStoreId": public_id,
            "XksProxyConnectivity": "VPC_ENDPOINT_SERVICE",
            "XksProxyUriEndpoint": "https://xks-vpce.example.com",
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (
            400,
            "XksProxyVpcEndpointServiceInvalidConfigurationException"
        ),
        "case VPC connectivity with no endpoint service: expected HTTP 400 \
         XksProxyVpcEndpointServiceInvalidConfigurationException, got {status} {code}"
    );
    let after = reported(&router, &public_id).await;
    assert_eq!(
        after, before,
        "case VPC connectivity with no endpoint service: a refused update changes nothing, \
         got {after}"
    );

    // Row 2: the same change, naming the endpoint service, is applied.
    updated(
        &router,
        "VPC connectivity with the endpoint service",
        &json!({
            "CustomKeyStoreId": public_id,
            "XksProxyConnectivity": "VPC_ENDPOINT_SERVICE",
            "XksProxyUriEndpoint": "https://xks-vpce.example.com",
            "XksProxyVpcEndpointServiceName": SERVICE_NAME,
        }),
    )
    .await;
    let entry = reported(&router, &public_id).await;
    assert_eq!(
        entry["XksProxyConfiguration"]["Connectivity"]
            .as_str()
            .unwrap_or_default(),
        "VPC_ENDPOINT_SERVICE",
        "case VPC connectivity with the endpoint service: expected the connectivity to \
         change, got {entry}"
    );
    assert_eq!(
        entry["XksProxyConfiguration"]["VpcEndpointServiceName"]
            .as_str()
            .unwrap_or_default(),
        SERVICE_NAME,
        "case VPC connectivity with the endpoint service: expected the endpoint service to \
         be reported, got {entry}"
    );

    // Row 3: a key store that already names an endpoint service needs nothing more.
    updated(
        &router,
        "VPC connectivity of a key store that already names one",
        &json!({
            "CustomKeyStoreId": named_id,
            "XksProxyConnectivity": "VPC_ENDPOINT_SERVICE",
            "XksProxyUriEndpoint": "https://xks-named-vpce.example.com",
        }),
    )
    .await;
    let entry = reported(&router, &named_id).await;
    assert_eq!(
        entry["XksProxyConfiguration"]["VpcEndpointServiceName"]
            .as_str()
            .unwrap_or_default(),
        "com.amazonaws.vpce.us-east-1.vpce-svc-00000000000000000",
        "case VPC connectivity of a key store that already names one: expected the held \
         endpoint service to be reported, got {entry}"
    );

    // Row 4: back to the public endpoint, where the endpoint service members are not
    // reported at all.
    updated(
        &router,
        "back to the public endpoint",
        &json!({
            "CustomKeyStoreId": public_id,
            "XksProxyConnectivity": "PUBLIC_ENDPOINT",
            "XksProxyUriEndpoint": "https://xks-public-again.example.com",
        }),
    )
    .await;
    let entry = reported(&router, &public_id).await;
    assert_eq!(
        entry["XksProxyConfiguration"]["Connectivity"]
            .as_str()
            .unwrap_or_default(),
        "PUBLIC_ENDPOINT",
        "case back to the public endpoint: expected the connectivity to change, got {entry}"
    );
    for member in ["VpcEndpointServiceName", "VpcEndpointServiceOwner"] {
        assert!(
            entry["XksProxyConfiguration"][member].is_null(),
            "case back to the public endpoint: {member} is reported only for a proxy reached \
             through an endpoint service, got {entry}"
        );
    }
}
