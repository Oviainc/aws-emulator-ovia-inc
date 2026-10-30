// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the custom key store CONNECTION lifecycle,
//! `ConnectCustomKeyStore` and `DisconnectCustomKeyStore`, driven over the wire
//! through the shared [`super::harness`].
//!
//! The rule under test, for every operation of the custom key store family that
//! names one key store by id: the id is resolved against the custom key stores of
//! the account and Region the request was signed for, an id that names none is
//! refused with `CustomKeyStoreNotFoundException`, a key store whose
//! `ConnectionState` the operation does not admit is refused with
//! `CustomKeyStoreInvalidStateException`, and an admitted one answers the documented
//! empty 200 having STARTED the change: the key store enters the transient state at
//! once and reaches the settled state a span later, which `DescribeCustomKeyStores`
//! is what reports. Both refusals are HTTP 400 and carry the error code alone.
//!
//! Every assertion is about that rule over a TABLE, whose rows differ in what backs
//! the key store (an CloudHSM cluster, an external key store proxy on a public
//! endpoint, one behind a VPC endpoint service), in the `ConnectionState` the key
//! store is in (each of the five the model admits), in which operation is called, in
//! the form of the id that names nothing (a minted id no key store has, a friendly
//! name used as an id, the id of a key store of another Region, and the shortest and
//! longest id the modelled length admits), and in the Region the key store belongs
//! to.
//!
//! The rows seed the records DIRECTLY into the store — the same records a create
//! leaves behind — so a row fixes the connection state it is about without a create
//! per row, and then drive the operations over the wire, exactly as
//! [`super::custom_key_stores`] does for the reader. What a create itself answers is
//! [`super::create_custom_key_store`].

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kms_trentservice::state::{
    mint_custom_key_store_id, service_state, CloudHsmCluster, CustomKeyStore,
    CustomKeyStoreBacking, XksProxy, CUSTOM_KEY_STORE_CONNECTION_SPAN, MAX_CUSTOM_KEY_STORES,
};

use super::harness::{assert_empty, call, error_code, ok_in, router_with_engine, REGION};

/// The Region a row belongs to when the row is about regional scoping: a custom key
/// store is a regional resource, so an id minted through another Region's endpoint
/// names no key store here.
const OTHER_REGION: &str = "eu-west-2";

/// The shortest and longest `CustomKeyStoreId` the model admits
/// (`smithy_query(query="operation",operation="ConnectCustomKeyStore")` —
/// `CustomKeyStoreIdType`, `@length(1..64)`). An id of either length is a well-formed
/// request, so it is refused as a missing key store rather than as a bad input.
const MIN_ID_LEN: usize = 1;
/// See [`MIN_ID_LEN`].
const MAX_ID_LEN: usize = 64;

/// Long enough past [`CUSTOM_KEY_STORE_CONNECTION_SPAN`] that the change the call
/// started has certainly fallen due.
fn past_the_span() -> Duration {
    CUSTOM_KEY_STORE_CONNECTION_SPAN + Duration::from_millis(250)
}

/// What backs a seeded key store — one row of the table's first dimension.
#[derive(Clone, Copy)]
enum Backing {
    CloudHsm,
    PublicEndpointProxy,
    VpcEndpointServiceProxy,
}

impl Backing {
    fn case(self) -> &'static str {
        match self {
            Self::CloudHsm => "an CloudHSM key store",
            Self::PublicEndpointProxy => "an external key store on a public endpoint",
            Self::VpcEndpointServiceProxy => "an external key store behind a VPC endpoint service",
        }
    }

    /// The configuration a key store of this kind holds. The values are this
    /// emulator's; what the rows are about is the connection lifecycle, which is the
    /// same for every backing.
    fn configuration(self) -> CustomKeyStoreBacking {
        match self {
            Self::CloudHsm => CustomKeyStoreBacking::CloudHsm(CloudHsmCluster {
                cloud_hsm_cluster_id: "cluster-6tzmqkjvb3x".to_string(),
                trust_anchor_certificate:
                    "-----BEGIN CERTIFICATE-----\nY29ubmVjdA==\n-----END CERTIFICATE-----"
                        .to_string(),
            }),
            Self::PublicEndpointProxy => CustomKeyStoreBacking::External(XksProxy {
                connectivity: "PUBLIC_ENDPOINT".to_string(),
                access_key_id: "BCDEFGHIJKLMNOPQRSTU".to_string(),
                uri_endpoint: "https://xks-connect.example.com".to_string(),
                uri_path: "/kms/xks/v1".to_string(),
                vpc_endpoint_service_name: None,
                vpc_endpoint_service_owner: None,
            }),
            Self::VpcEndpointServiceProxy => CustomKeyStoreBacking::External(XksProxy {
                connectivity: "VPC_ENDPOINT_SERVICE".to_string(),
                access_key_id: "CDEFGHIJKLMNOPQRSTUV".to_string(),
                uri_endpoint: "https://xks-connect-vpce.example.com".to_string(),
                uri_path: "/example/kms/xks/v2".to_string(),
                vpc_endpoint_service_name: Some(
                    "com.amazonaws.vpce.us-east-1.vpce-svc-0fedcba9876543210".to_string(),
                ),
                vpc_endpoint_service_owner: Some("111122223333".to_string()),
            }),
        }
    }
}

/// Seed one custom key store in `region`, in the `ConnectionState` the row needs,
/// and answer its minted id. A connection state written here carries no change the
/// clock owes, which is exactly what a key store that has been sitting in that state
/// looks like.
fn seed(
    engine: &Arc<engine::InMemoryEngine>,
    region: &str,
    name: &str,
    backing: Backing,
    connection_state: &str,
    connection_error_code: Option<&str>,
) -> String {
    let id = mint_custom_key_store_id().expect("the platform produces entropy for a key store id");
    let mut store = CustomKeyStore::new(
        id.clone(),
        region.to_string(),
        name.to_string(),
        backing.configuration(),
    );
    store.connection_state = connection_state.to_string();
    store.connection_error_code = connection_error_code.map(str::to_string);
    let state = service_state(&**engine).expect("the KMS state store is reachable");
    assert!(
        state.lock().insert_custom_key_store(store),
        "case {name}: the store has room for the row's key store"
    );
    id
}

/// The one entry `DescribeCustomKeyStores` reports for `custom_key_store_id`.
async fn entry(router: &MultiServiceRouter, case: &str, custom_key_store_id: &str) -> Value {
    let answer = ok_in(
        router,
        REGION,
        "DescribeCustomKeyStores",
        &json!({"CustomKeyStoreId": custom_key_store_id}),
    )
    .await;
    let entries = answer["CustomKeyStores"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a CustomKeyStores list in {answer}"));
    assert_eq!(
        entries.len(),
        1,
        "case {case}: an id names at most one key store, got {entries:?}"
    );
    entries[0].clone()
}

/// The `ConnectionState` `DescribeCustomKeyStores` reports for a key store.
async fn connection_state(
    router: &MultiServiceRouter,
    case: &str,
    custom_key_store_id: &str,
) -> String {
    let entry = entry(router, case, custom_key_store_id).await;
    entry["ConnectionState"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a ConnectionState in {entry}"))
        .to_string()
}

/// Call `operation` for `custom_key_store_id` in [`REGION`], requiring the empty 200
/// AWS documents for it ("it returns a JSON object with no properties").
async fn accepted(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    custom_key_store_id: &str,
) {
    let answer = ok_in(
        router,
        REGION,
        operation,
        &json!({"CustomKeyStoreId": custom_key_store_id}),
    )
    .await;
    assert_empty(case, operation, &answer);
}

/// Call `operation` for `custom_key_store_id` in `region` and require HTTP 400 with
/// the error code `expected` and NO message text: this family answers its refusals
/// with the error code alone.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operation: &str,
    custom_key_store_id: &str,
    expected: &str,
) {
    let (status, payload) = call(
        router,
        region,
        operation,
        &json!({"CustomKeyStoreId": custom_key_store_id}),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload).as_str()),
        (400, expected),
        "case {case}: expected HTTP 400 {expected} from {operation}, got {status} with {payload}"
    );
    for member in ["message", "Message"] {
        let text = payload[member].as_str().unwrap_or_default();
        assert!(
            text.is_empty(),
            "case {case}: the refusal carries no {member} text, got {payload}"
        );
    }
}

/// The two operations under test. Both take the same single required member and
/// resolve it the same way, so every row of the not-found table is driven through
/// both.
const OPERATIONS: [&str; 2] = ["ConnectCustomKeyStore", "DisconnectCustomKeyStore"];

/// An id that names no custom key store of this account and Region is refused as a
/// missing key store by BOTH operations — never as a success and never as a 501.
#[tokio::test]
async fn connect_and_disconnect_refuse_an_id_that_names_no_key_store() {
    let (router, engine) = router_with_engine();
    let here = seed(
        &engine,
        REGION,
        "connectable",
        Backing::CloudHsm,
        "DISCONNECTED",
        None,
    );
    let elsewhere = seed(
        &engine,
        OTHER_REGION,
        "another-region",
        Backing::PublicEndpointProxy,
        "CONNECTED",
        None,
    );

    let unknown: Vec<(String, String)> = vec![
        (
            "an id no key store has".to_string(),
            mint_custom_key_store_id().expect("the platform produces entropy"),
        ),
        (
            "a friendly name used as an id".to_string(),
            "connectable".to_string(),
        ),
        (
            "the id of a key store of another Region".to_string(),
            elsewhere.clone(),
        ),
        (
            "the shortest id the model admits".to_string(),
            "c".repeat(MIN_ID_LEN),
        ),
        (
            "the longest id the model admits".to_string(),
            "c".repeat(MAX_ID_LEN),
        ),
    ];
    for operation in OPERATIONS {
        for (case, id) in &unknown {
            let case = format!("{case} for {operation}");
            refused(
                &router,
                &case,
                REGION,
                operation,
                id,
                "CustomKeyStoreNotFoundException",
            )
            .await;
        }
        // The key store of this Region is found, so the id form is not what the
        // refusals above are about.
        assert_eq!(
            connection_state(&router, "a refusal changes nothing", &here).await,
            "DISCONNECTED",
            "case a refusal changes nothing: a refused {operation} must not move a \
             key store"
        );
    }

    // The key store of the other Region is reachable through its OWN endpoint, which
    // is what makes the refusals above regional scoping rather than a missing record.
    let answer = ok_in(
        &router,
        OTHER_REGION,
        "ConnectCustomKeyStore",
        &json!({"CustomKeyStoreId": elsewhere}),
    )
    .await;
    assert_empty(
        "another Region's own endpoint",
        "ConnectCustomKeyStore",
        &answer,
    );
}

/// Connecting a key store starts the connection process without waiting for it: the
/// key store is CONNECTING at once and CONNECTED once the change falls due, for every
/// backing, and nothing else about the record changes.
#[tokio::test]
async fn connect_custom_key_store_starts_the_connection_process() {
    let (router, engine) = router_with_engine();
    for backing in [
        Backing::CloudHsm,
        Backing::PublicEndpointProxy,
        Backing::VpcEndpointServiceProxy,
    ] {
        let case = backing.case();
        let id = seed(
            &engine,
            REGION,
            &format!("connect-{}", backing.case().len()),
            backing,
            "DISCONNECTED",
            None,
        );
        let before = entry(&router, case, &id).await;

        accepted(&router, case, "ConnectCustomKeyStore", &id).await;

        // The span is never zero: a caller reading straight after the call still sees
        // the transient state.
        assert_eq!(
            connection_state(&router, case, &id).await,
            "CONNECTING",
            "case {case}: the connection process starts without completing"
        );
        // Reporting the transient state does not itself complete the change.
        assert_eq!(
            connection_state(&router, case, &id).await,
            "CONNECTING",
            "case {case}: a read must not move the connection state"
        );

        tokio::time::sleep(past_the_span()).await;
        assert_eq!(
            connection_state(&router, case, &id).await,
            "CONNECTED",
            "case {case}: the connection the call started falls due"
        );
        // Once settled it stays settled, however many times it is read.
        assert_eq!(
            connection_state(&router, case, &id).await,
            "CONNECTED",
            "case {case}: a settled connection state is stable across reads"
        );

        let after = entry(&router, case, &id).await;
        for member in [
            "CustomKeyStoreId",
            "CustomKeyStoreName",
            "CustomKeyStoreType",
            "CloudHsmClusterId",
            "TrustAnchorCertificate",
            "XksProxyConfiguration",
            "CreationDate",
        ] {
            assert_eq!(
                after[member], before[member],
                "case {case}: connecting changes only the connection state, but \
                 {member} became {:?}",
                after[member]
            );
        }
    }
}

/// Disconnecting a connected key store leaves it DISCONNECTING and then
/// DISCONNECTED, and a key store can be reconnected afterwards — "you can disconnect
/// and reconnect a custom key store at any time".
#[tokio::test]
async fn disconnect_custom_key_store_disconnects_and_can_be_reconnected() {
    let (router, engine) = router_with_engine();
    for backing in [
        Backing::CloudHsm,
        Backing::PublicEndpointProxy,
        Backing::VpcEndpointServiceProxy,
    ] {
        let case = backing.case();
        let id = seed(
            &engine,
            REGION,
            &format!("disconnect-{}", backing.case().len()),
            backing,
            "CONNECTED",
            None,
        );

        accepted(&router, case, "DisconnectCustomKeyStore", &id).await;
        assert_eq!(
            connection_state(&router, case, &id).await,
            "DISCONNECTING",
            "case {case}: the disconnection starts without completing"
        );

        tokio::time::sleep(past_the_span()).await;
        assert_eq!(
            connection_state(&router, case, &id).await,
            "DISCONNECTED",
            "case {case}: the disconnection the call started falls due"
        );

        accepted(&router, case, "ConnectCustomKeyStore", &id).await;
        tokio::time::sleep(past_the_span()).await;
        assert_eq!(
            connection_state(&router, case, &id).await,
            "CONNECTED",
            "case {case}: a disconnected key store can be reconnected"
        );
    }
}

/// Whether an operation is admitted for a key store in a given `ConnectionState`,
/// over every state the model admits: `ConnectCustomKeyStore` is refused for
/// DISCONNECTING and FAILED and `DisconnectCustomKeyStore` for DISCONNECTING and
/// DISCONNECTED; each "is valid for all other ConnectionState values".
#[tokio::test]
async fn connect_and_disconnect_refuse_a_connection_state_they_do_not_admit() {
    // (the state the key store is in, the operation called, whether it is admitted)
    let rows: [(&str, &str, bool); 10] = [
        ("DISCONNECTED", "ConnectCustomKeyStore", true),
        ("DISCONNECTED", "DisconnectCustomKeyStore", false),
        ("CONNECTED", "ConnectCustomKeyStore", true),
        ("CONNECTED", "DisconnectCustomKeyStore", true),
        ("CONNECTING", "ConnectCustomKeyStore", true),
        ("CONNECTING", "DisconnectCustomKeyStore", true),
        ("DISCONNECTING", "ConnectCustomKeyStore", false),
        ("DISCONNECTING", "DisconnectCustomKeyStore", false),
        ("FAILED", "ConnectCustomKeyStore", false),
        ("FAILED", "DisconnectCustomKeyStore", true),
    ];
    for (index, (state, operation, admitted)) in rows.into_iter().enumerate() {
        let (router, engine) = router_with_engine();
        let case = format!("{operation} on a {state} key store");
        // A transient state is reached the only way a key store reaches one: through
        // the call that starts the change. The others are what a key store sitting in
        // that state looks like.
        let (seeded, precondition) = match state {
            "CONNECTING" => ("DISCONNECTED", Some("ConnectCustomKeyStore")),
            "DISCONNECTING" => ("CONNECTED", Some("DisconnectCustomKeyStore")),
            settled => (settled, None),
        };
        let id = seed(
            &engine,
            REGION,
            &format!("state-row-{index}"),
            Backing::CloudHsm,
            seeded,
            // A key store that reports a failed connection attempt holds the code
            // that caused it; it is reported only while the state is FAILED.
            Some("NETWORK_ERRORS"),
        );
        if let Some(starter) = precondition {
            accepted(&router, &case, starter, &id).await;
            assert_eq!(
                connection_state(&router, &case, &id).await,
                state,
                "case {case}: the row's precondition must put the key store in {state}"
            );
        }

        if admitted {
            accepted(&router, &case, operation, &id).await;
            let expected = if operation == "ConnectCustomKeyStore" {
                "CONNECTING"
            } else {
                "DISCONNECTING"
            };
            assert_eq!(
                connection_state(&router, &case, &id).await,
                expected,
                "case {case}: an admitted call starts the change at once"
            );
        } else {
            refused(
                &router,
                &case,
                REGION,
                operation,
                &id,
                "CustomKeyStoreInvalidStateException",
            )
            .await;
            assert_eq!(
                connection_state(&router, &case, &id).await,
                state,
                "case {case}: a refused call leaves the connection state where it was"
            );
        }
    }
}

/// The documented recovery from a failed connection attempt: "to reconnect a custom
/// key store in a FAILED state, disconnect it (DisconnectCustomKeyStore), then
/// connect it (ConnectCustomKeyStore)".
#[tokio::test]
async fn a_failed_key_store_is_reconnected_by_disconnecting_it_first() {
    let (router, engine) = router_with_engine();
    let case = "a failed CloudHSM key store";
    let id = seed(
        &engine,
        REGION,
        "recovered",
        Backing::CloudHsm,
        "FAILED",
        Some("INSUFFICIENT_CLOUDHSM_HSMS"),
    );
    assert_eq!(
        entry(&router, case, &id).await["ConnectionErrorCode"],
        json!("INSUFFICIENT_CLOUDHSM_HSMS"),
        "case {case}: a failed key store reports what the failure was"
    );

    refused(
        &router,
        case,
        REGION,
        "ConnectCustomKeyStore",
        &id,
        "CustomKeyStoreInvalidStateException",
    )
    .await;

    accepted(&router, case, "DisconnectCustomKeyStore", &id).await;
    tokio::time::sleep(past_the_span()).await;
    assert_eq!(
        connection_state(&router, case, &id).await,
        "DISCONNECTED",
        "case {case}: disconnecting a failed key store disconnects it"
    );

    accepted(&router, case, "ConnectCustomKeyStore", &id).await;
    tokio::time::sleep(past_the_span()).await;
    let settled = entry(&router, case, &id).await;
    assert_eq!(
        settled["ConnectionState"],
        json!("CONNECTED"),
        "case {case}: a disconnected key store connects, got {settled}"
    );
    // "The ConnectionErrorCode appears only when the ConnectionState is FAILED", so
    // the code the earlier failure left behind is no longer reported.
    assert!(
        settled["ConnectionErrorCode"].is_null(),
        "case {case}: a connected key store reports no connection error code, got \
         {settled}"
    );
}

/// The map these operations resolve ids against is BOUNDED: it never grows past
/// `MAX_CUSTOM_KEY_STORES`, a full store persists NOTHING, and the key stores it
/// already holds keep working.
#[tokio::test]
async fn the_custom_key_store_map_is_bounded() {
    let (router, engine) = router_with_engine();
    let case = "a full key store map";
    let held = seed(
        &engine,
        REGION,
        "bounded",
        Backing::CloudHsm,
        "DISCONNECTED",
        None,
    );

    let state = service_state(&*engine).expect("the KMS state store is reachable");
    {
        let mut state = state.lock();
        while state.has_room_for_another_custom_key_store() {
            let id = mint_custom_key_store_id().expect("the platform produces entropy");
            let store = CustomKeyStore::new(
                id.clone(),
                REGION.to_string(),
                id.clone(),
                Backing::CloudHsm.configuration(),
            );
            assert!(
                state.insert_custom_key_store(store),
                "case {case}: a map with room persists another key store"
            );
        }
        assert_eq!(
            state.custom_key_stores.len(),
            MAX_CUSTOM_KEY_STORES,
            "case {case}: the map fills to its bound and no further"
        );

        let refused_id = mint_custom_key_store_id().expect("the platform produces entropy");
        let store = CustomKeyStore::new(
            refused_id.clone(),
            REGION.to_string(),
            refused_id.clone(),
            Backing::CloudHsm.configuration(),
        );
        assert!(
            !state.insert_custom_key_store(store),
            "case {case}: a full map persists nothing"
        );
        assert_eq!(
            (
                state.custom_key_stores.len(),
                state.custom_key_store_order.len(),
                state.custom_key_stores.contains_key(&refused_id)
            ),
            (MAX_CUSTOM_KEY_STORES, MAX_CUSTOM_KEY_STORES, false),
            "case {case}: neither the map nor the creation order grows past the bound"
        );
    }

    // What the map already holds still resolves and still moves.
    accepted(&router, case, "ConnectCustomKeyStore", &held).await;
    assert_eq!(
        connection_state(&router, case, &held).await,
        "CONNECTING",
        "case {case}: a key store the map holds still connects"
    );
}
