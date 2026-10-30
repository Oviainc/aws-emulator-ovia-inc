// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GenerateRandom`, driven over the wire through the shared
//! [`super::harness`].
//!
//! The rule under test: a request states the length of the random byte string through
//! `NumberOfBytes` and nothing else, and a served request answers that many freshly drawn
//! bytes — so no two calls answer the same string and nothing in the answer follows from
//! the request beyond its length. A length the member's modelled range does not admit, and
//! an absent one, are refused with `ValidationException` in the envelope the boundary
//! validator states a constraint in, and a refusal carries no bytes at all. A request that
//! names a `CustomKeyStoreId` is judged against the custom key stores the account and
//! Region hold: an id that names none is refused, an external key store is not valid for
//! this parameter, and an CloudHSM key store serves the request only while it is
//! connected. Nothing is persisted, so the key store a served call named reads back
//! exactly as it did before.
//!
//! The rows differ along what the rule generalises over: the length asked for (each
//! published bound of `NumberOfBytes`, one past each of them, and sizes in between), what
//! backs the key store a request names (an CloudHSM cluster, an external key store proxy
//! on a public endpoint and one behind a VPC endpoint service), the connection state the
//! key store is in, and the Region the key store belongs to.
//!
//! The key store rows seed the records DIRECTLY into the store — the same records a
//! create leaves behind — so a row fixes the connection state it is about without a
//! create per row, and then drive `GenerateRandom` over the wire, exactly as
//! [`super::custom_key_stores`] does for the reader. What a create itself answers is
//! [`super::create_custom_key_store`].

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kms_trentservice::state::{
    mint_custom_key_store_id, service_state, CloudHsmCluster, CustomKeyStore,
    CustomKeyStoreBacking, XksProxy,
};

use super::harness::{call, error_code, error_message, ok, refused, router, router_with_engine,
                     unb64, REGION};

/// The operation under test.
const OPERATION: &str = "GenerateRandom";

/// The member that states the length, as a validation clause spells it (lowerCamel).
const MEMBER: &str = "numberOfBytes";

/// The published bounds of the length: `NumberOfBytesType` is `@range(min: 1, max: 1024)`
/// (`smithy_query(query="shape",shape="NumberOfBytesType")`).
const MIN_BYTES: i64 = 1;
/// See [`MIN_BYTES`].
const MAX_BYTES: i64 = 1024;

/// A Region other than the one a row is requested through: a custom key store is a
/// regional resource, so the same account's key stores in another Region name nothing
/// here.
const OTHER_REGION: &str = "eu-west-2";

/// How short a string this suite still asks to be unique across calls. Below it a repeat
/// is a plausible draw rather than a defect — two one-byte strings collide once in 256 —
/// so the shortest rows pin only their length.
const UNIQUE_FROM: usize = 8;

/// A served request answers exactly the number of bytes it asked for, drawn fresh for
/// that call, and carries no `CiphertextForRecipient`: "this field is included in the
/// response only when the Recipient parameter in the request includes a valid attestation
/// document" (`smithy_query(query="doc",shape="GenerateRandomResponse")`).
#[tokio::test]
async fn a_served_request_answers_a_fresh_string_of_the_length_it_asked_for() {
    let router = router();

    // The lengths walk both published bounds of `NumberOfBytes` and sizes in between.
    let lengths: [i64; 8] = [MIN_BYTES, 2, 7, 16, 32, 100, 513, MAX_BYTES];
    let mut seen: HashSet<Vec<u8>> = HashSet::new();

    for length in lengths {
        // Each length is asked for twice, so the freshness of the answer is asserted
        // against another answer to the SAME request.
        for attempt in 1..=2 {
            let case = format!("{length} bytes, call {attempt}");
            let answer = ok(&router, OPERATION, &json!({"NumberOfBytes": length})).await;
            let plaintext = unb64(&case, &answer, "Plaintext");
            assert_eq!(
                i64::try_from(plaintext.len()).unwrap_or(-1),
                length,
                "case {case:?}: expected a {length}-byte string, got {} bytes",
                plaintext.len()
            );
            assert!(
                answer.get("CiphertextForRecipient").is_none(),
                "case {case:?}: a request with no Recipient must carry no \
                 CiphertextForRecipient, got {answer}"
            );
            if plaintext.len() >= UNIQUE_FROM {
                assert!(
                    seen.insert(plaintext.clone()),
                    "case {case:?}: every call must answer a freshly drawn string, got one \
                     an earlier call already answered: {plaintext:?}"
                );
            }
        }
    }
}

/// The bytes are drawn rather than derived: many calls for the same length answer
/// distinct strings, and no string is a constant run.
#[tokio::test]
async fn repeated_calls_for_one_length_answer_distinct_strings() {
    let router = router();
    let length = 32usize;
    let draws = 24usize;
    let mut seen: HashSet<Vec<u8>> = HashSet::new();

    for attempt in 1..=draws {
        let case = format!("draw {attempt} of {length} bytes");
        let answer = ok(&router, OPERATION, &json!({"NumberOfBytes": length})).await;
        let plaintext = unb64(&case, &answer, "Plaintext");
        assert!(
            plaintext.iter().any(|byte| *byte != plaintext[0]),
            "case {case:?}: a drawn string must not be a constant run, got {plaintext:?}"
        );
        assert!(
            seen.insert(plaintext.clone()),
            "case {case:?}: expected a string no earlier draw answered, got {plaintext:?}"
        );
    }
    assert_eq!(
        seen.len(),
        draws,
        "case table: expected {draws} distinct strings, got {}",
        seen.len()
    );
}

/// One row of the length table that is refused: what the request states, and the
/// `ValidationException` message it is refused with.
struct RefusedLength {
    case: &'static str,
    /// The members the request carries.
    body: Value,
    /// The clause the refusal states, inside the envelope [`envelope`] builds.
    clause: String,
}

/// The envelope a refusal states one clause in, as the boundary validator states it.
fn envelope(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// Every way a request can fail to state a length the member admits: not at all, and
/// outside each published bound of `NumberOfBytes`.
fn refused_lengths() -> Vec<RefusedLength> {
    vec![
        RefusedLength {
            case: "no NumberOfBytes at all",
            body: json!({}),
            clause: format!(
                "Value null at '{MEMBER}' failed to satisfy constraint: Member must not be null"
            ),
        },
        RefusedLength {
            case: "one below the published minimum",
            body: json!({"NumberOfBytes": MIN_BYTES - 1}),
            clause: format!(
                "Value '{}' at '{MEMBER}' failed to satisfy constraint: Member must have value \
                 greater than or equal to {MIN_BYTES}",
                MIN_BYTES - 1
            ),
        },
        RefusedLength {
            case: "a negative length",
            body: json!({"NumberOfBytes": -64}),
            clause: format!(
                "Value '-64' at '{MEMBER}' failed to satisfy constraint: Member must have value \
                 greater than or equal to {MIN_BYTES}"
            ),
        },
        RefusedLength {
            case: "one above the published maximum",
            body: json!({"NumberOfBytes": MAX_BYTES + 1}),
            clause: format!(
                "Value '{}' at '{MEMBER}' failed to satisfy constraint: Member must have value \
                 less than or equal to {MAX_BYTES}",
                MAX_BYTES + 1
            ),
        },
        RefusedLength {
            case: "far above the published maximum",
            body: json!({"NumberOfBytes": 1_048_576}),
            clause: format!(
                "Value '1048576' at '{MEMBER}' failed to satisfy constraint: Member must have \
                 value less than or equal to {MAX_BYTES}"
            ),
        },
    ]
}

/// A request that does not state a length the member admits is refused with HTTP 400
/// `ValidationException`, in the envelope a constraint refusal is stated in, and answers
/// no bytes.
#[tokio::test]
async fn a_request_that_states_no_admissible_length_is_refused() {
    let router = router();
    for row in refused_lengths() {
        let (status, payload) = call(&router, REGION, OPERATION, &row.body).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, "ValidationException"),
            "case {:?}: expected HTTP 400 ValidationException, got {status} with {payload}",
            row.case
        );
        assert_eq!(
            error_message(&payload),
            envelope(&row.clause),
            "case {:?}: expected the constraint envelope, got {payload}",
            row.case
        );
        assert!(
            payload.get("Plaintext").is_none(),
            "case {:?}: a refusal must carry no bytes, got {payload}",
            row.case
        );
    }
}

/// What a request naming one of the seeded key stores is answered with.
enum Outcome {
    /// Served: the bytes are generated in the key store the request named.
    Served,
    /// Refused with this wire error code and no message, the way the custom key store
    /// family's refusals are recorded.
    RefusedWithoutMessage(&'static str),
    /// Refused with this wire error code and a message naming the key store.
    RefusedNaming(&'static str),
}

/// One row of the key store table: the record seeded into the store, the Region the
/// request is made through, and what the request is answered with.
struct StoreRow {
    case: &'static str,
    store: CustomKeyStore,
    region: &'static str,
    outcome: Outcome,
}

/// A fresh custom key store id, minted the way the service mints one.
fn new_id() -> String {
    mint_custom_key_store_id().expect("the platform produces entropy for a key store id")
}

/// A key store backed by an CloudHSM cluster, in `connection_state`.
fn cloud_hsm(region: &str, name: &str, connection_state: &str) -> CustomKeyStore {
    let mut store = CustomKeyStore::new(
        new_id(),
        region.to_string(),
        name.to_string(),
        CustomKeyStoreBacking::CloudHsm(CloudHsmCluster {
            cloud_hsm_cluster_id: "cluster-random".to_string(),
            trust_anchor_certificate: "-----BEGIN CERTIFICATE-----\ncmFuZG9t=\n\
                                       -----END CERTIFICATE-----"
                .to_string(),
        }),
    );
    store.connection_state = connection_state.to_string();
    store
}

/// A key store backed by an external key store proxy, in `connection_state`. The VPC
/// endpoint service members are held whatever the connectivity is, so the two external
/// rows differ in how the proxy is reached.
fn external(
    region: &str,
    name: &str,
    connectivity: &str,
    connection_state: &str,
) -> CustomKeyStore {
    let vpc = connectivity == "VPC_ENDPOINT_SERVICE";
    let mut store = CustomKeyStore::new(
        new_id(),
        region.to_string(),
        name.to_string(),
        CustomKeyStoreBacking::External(XksProxy {
            connectivity: connectivity.to_string(),
            access_key_id: "ARIA0000000000000000".to_string(),
            uri_endpoint: "https://xks.example.com".to_string(),
            uri_path: "/kms/xks/v1".to_string(),
            vpc_endpoint_service_name: vpc
                .then(|| "com.amazonaws.vpce.us-east-1.vpce-svc-random".to_string()),
            vpc_endpoint_service_owner: vpc.then(|| engine::emulator_account_id().to_string()),
        }),
    );
    store.connection_state = connection_state.to_string();
    store
}

/// The key store table: what backs the key store, the connection state it is in, and the
/// Region it belongs to, each against what `GenerateRandom` answers for it.
///
/// The connection states are the values `ConnectionStateType` declares
/// (`smithy_query(query="shape",shape="ConnectionStateType")`), and only `CONNECTED`
/// serves the request: "you requested the GenerateRandom operation in an CloudHSM key
/// store that is not connected. This operation is valid only when the CloudHSM key store
/// ConnectionState is CONNECTED"
/// (`smithy_query(query="doc",shape="CustomKeyStoreInvalidStateException")`).
fn store_rows() -> Vec<StoreRow> {
    vec![
        StoreRow {
            case: "a connected CloudHSM key store",
            store: cloud_hsm(REGION, "random-connected", "CONNECTED"),
            region: REGION,
            outcome: Outcome::Served,
        },
        StoreRow {
            case: "an CloudHSM key store that was never connected",
            store: cloud_hsm(REGION, "random-disconnected", "DISCONNECTED"),
            region: REGION,
            outcome: Outcome::RefusedWithoutMessage("CustomKeyStoreInvalidStateException"),
        },
        StoreRow {
            case: "an CloudHSM key store still connecting",
            store: cloud_hsm(REGION, "random-connecting", "CONNECTING"),
            region: REGION,
            outcome: Outcome::RefusedWithoutMessage("CustomKeyStoreInvalidStateException"),
        },
        StoreRow {
            case: "an CloudHSM key store disconnecting",
            store: cloud_hsm(REGION, "random-disconnecting", "DISCONNECTING"),
            region: REGION,
            outcome: Outcome::RefusedWithoutMessage("CustomKeyStoreInvalidStateException"),
        },
        StoreRow {
            case: "an CloudHSM key store whose connection attempt failed",
            store: cloud_hsm(REGION, "random-failed", "FAILED"),
            region: REGION,
            outcome: Outcome::RefusedWithoutMessage("CustomKeyStoreInvalidStateException"),
        },
        StoreRow {
            case: "a connected external key store on a public endpoint",
            store: external(REGION, "random-xks-public", "PUBLIC_ENDPOINT", "CONNECTED"),
            region: REGION,
            outcome: Outcome::RefusedNaming("UnsupportedOperationException"),
        },
        StoreRow {
            case: "a disconnected external key store behind a VPC endpoint service",
            store: external(
                REGION,
                "random-xks-vpc",
                "VPC_ENDPOINT_SERVICE",
                "DISCONNECTED",
            ),
            region: REGION,
            outcome: Outcome::RefusedNaming("UnsupportedOperationException"),
        },
        StoreRow {
            case: "a connected CloudHSM key store of another Region",
            store: cloud_hsm(OTHER_REGION, "random-elsewhere", "CONNECTED"),
            region: REGION,
            outcome: Outcome::RefusedWithoutMessage("CustomKeyStoreNotFoundException"),
        },
    ]
}

/// Seed `rows` into the store the router was built over, so every later request observes
/// them.
fn seed(engine: &Arc<engine::InMemoryEngine>, rows: &[StoreRow]) {
    let state = service_state(&**engine).expect("the KMS state store is reachable");
    let mut state = state.lock();
    for row in rows {
        state.insert_custom_key_store(row.store.clone());
    }
}

/// The entry `DescribeCustomKeyStores` reports for one key store id, through `region`.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    custom_key_store_id: &str,
) -> Value {
    let answer = super::harness::ok_in(
        router,
        region,
        "DescribeCustomKeyStores",
        &json!({"CustomKeyStoreId": custom_key_store_id}),
    )
    .await;
    let entry = answer["CustomKeyStores"][0].clone();
    assert_eq!(
        entry["CustomKeyStoreId"].as_str(),
        Some(custom_key_store_id),
        "case {case:?}: expected the seeded key store {custom_key_store_id} to be reported, \
         got {answer}"
    );
    entry
}

/// A `CustomKeyStoreId` is judged against the custom key stores the account and Region
/// hold, and only a connected CloudHSM key store serves the request. A served call
/// persists nothing, so the key store reads back exactly as it did before.
#[tokio::test]
async fn a_custom_key_store_id_is_judged_against_the_key_stores_the_account_holds() {
    let (router, engine) = router_with_engine();
    let rows = store_rows();
    seed(&engine, &rows);

    for row in &rows {
        let id = row.store.custom_key_store_id.clone();
        let before = described(&router, row.case, row.store.region.as_str(), &id).await;
        let body = json!({"NumberOfBytes": 64, "CustomKeyStoreId": id});
        let (status, payload) = call(&router, row.region, OPERATION, &body).await;

        match row.outcome {
            Outcome::Served => {
                assert_eq!(
                    status, 200,
                    "case {:?}: expected HTTP 200, got {status} with {payload}",
                    row.case
                );
                let plaintext = unb64(row.case, &payload, "Plaintext");
                assert_eq!(
                    plaintext.len(),
                    64,
                    "case {:?}: expected a 64-byte string, got {} bytes",
                    row.case,
                    plaintext.len()
                );
            }
            Outcome::RefusedWithoutMessage(expected) => {
                assert_eq!(
                    (status, error_code(&payload).as_str()),
                    (400, expected),
                    "case {:?}: expected HTTP 400 {expected}, got {status} with {payload}",
                    row.case
                );
                assert_eq!(
                    error_message(&payload),
                    "",
                    "case {:?}: this refusal carries the error code alone, got {payload}",
                    row.case
                );
            }
            Outcome::RefusedNaming(expected) => {
                assert_eq!(
                    (status, error_code(&payload).as_str()),
                    (400, expected),
                    "case {:?}: expected HTTP 400 {expected}, got {status} with {payload}",
                    row.case
                );
                assert!(
                    error_message(&payload).contains(&id),
                    "case {:?}: the refusal must name the key store the request named, got \
                     {payload}",
                    row.case
                );
            }
        }
        if !matches!(row.outcome, Outcome::Served) {
            assert!(
                payload.get("Plaintext").is_none(),
                "case {:?}: a refusal must carry no bytes, got {payload}",
                row.case
            );
        }

        // The call read the key store; it never wrote to it, so what a later caller sees
        // is what the earlier one saw.
        let after = described(&router, row.case, row.store.region.as_str(), &id).await;
        assert_eq!(
            after, before,
            "case {:?}: the call must leave the key store as it found it, got {after} after \
             {before}",
            row.case
        );
    }

    // An id this account and Region holds no key store under names none at all.
    let (status, code, message) = refused(
        &router,
        REGION,
        OPERATION,
        &json!({"NumberOfBytes": 16, "CustomKeyStoreId": new_id()}),
    )
    .await;
    assert_eq!(
        (status, code.as_str(), message.as_str()),
        (400, "CustomKeyStoreNotFoundException", ""),
        "case an id naming no key store: expected HTTP 400 CustomKeyStoreNotFoundException \
         with no message, got {status} {code} {message:?}"
    );
}

/// What `GenerateRandom` cannot honour it refuses, and a refusal carries no bytes: an
/// attestation document no enclave produced cannot be verified here, so the capability is
/// reported as not implemented rather than answered with a `CiphertextForRecipient`
/// nothing encrypted.
#[tokio::test]
async fn a_request_for_an_enclave_recipient_is_refused() {
    let router = router();
    let body = json!({
        "NumberOfBytes": 32,
        "Recipient": {
            "KeyEncryptionAlgorithm": "RSAES_OAEP_SHA_256",
            "AttestationDocument": super::harness::b64(b"not an attestation"),
        },
    });
    let (status, payload) = call(&router, REGION, OPERATION, &body).await;
    assert_eq!(
        (status, error_code(&payload).as_str()),
        (501, "NotImplemented"),
        "case an enclave recipient: expected HTTP 501 NotImplemented, got {status} with \
         {payload}"
    );
    assert!(
        payload.get("Plaintext").is_none() && payload.get("CiphertextForRecipient").is_none(),
        "case an enclave recipient: a refusal must carry no bytes, got {payload}"
    );
}
