// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the custom key store READER, `DescribeCustomKeyStores`,
//! driven over the wire through the shared [`super::harness`].
//!
//! The rule under test: `DescribeCustomKeyStores` reports the custom key stores
//! persisted for the account and Region the request was signed for, in creation
//! order — narrowed to the single key store that either `CustomKeyStoreId` or
//! `CustomKeyStoreName` names (not both, which is refused as an invalid request),
//! and refused with `CustomKeyStoreNotFoundException` when the selector names none —
//! paging the selection by `Limit` and by a `Marker` it minted for this list, and
//! projecting each record so that a member a key store of that type or connection
//! state has no value for is ABSENT rather than empty. Both refusals carry the error
//! code alone. It writes nothing, so two reads of an untouched store answer the same.
//!
//! Every assertion is about that rule over a TABLE, whose rows differ in what backs
//! the key store (an CloudHSM cluster, an external key store proxy on a public
//! endpoint, one behind a VPC endpoint service), in the connection state the key
//! store is in (never connected, connected, a failed connection attempt, a
//! connection that failed and was later made), in the friendly name (the shortest
//! and the longest the modelled length admits), in the Region the key store belongs
//! to, in which member selects it, and in the page size.
//!
//! `CreateCustomKeyStore` is still a scaffold stub with its own issue, so the rows
//! seed the records DIRECTLY into the store — the same records a create leaves
//! behind — and then drive `DescribeCustomKeyStores` over the wire, exactly as
//! [`super::multi_region_primary`] does for the replicas it cannot create yet.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kms_trentservice::ops::types::{
    CONNECTION_ERROR_CODE_TYPE_VALUES, CONNECTION_STATE_TYPE_VALUES,
    CUSTOM_KEY_STORE_TYPE_VALUES, XKS_PROXY_CONNECTIVITY_TYPE_VALUES,
};
use service_com_amazonaws_kms_trentservice::state::{
    encode_marker, mint_custom_key_store_id, service_state, CloudHsmCluster, CustomKeyStore,
    CustomKeyStoreBacking, XksProxy,
};

use super::harness::{call, error_code, failure_in, ok_in, router, router_with_engine, REGION};

/// The Region a row belongs to when the row is about regional scoping: a custom key
/// store is a regional resource, so the same account's key stores in another Region
/// are not reported here.
const OTHER_REGION: &str = "eu-west-2";

/// The longest friendly name `CustomKeyStoreNameType` admits
/// (`smithy_query(query="shape",shape="CustomKeyStoreNameType")` — `@length(1..256)`).
const MAX_NAME_LEN: usize = 256;

/// The largest page `Limit` admits: `LimitType`'s modelled `@range(1..1000)`
/// (`smithy_query(query="operation",operation="DescribeCustomKeyStores")`).
const MAX_LIMIT: i64 = 1000;

/// One row: the case it stands for, the record seeded into the store, and the entry
/// `DescribeCustomKeyStores` must report for it, without its `CreationDate` (which
/// the clock, not the row, decides).
struct Row {
    case: &'static str,
    store: CustomKeyStore,
    expected: Value,
}

impl Row {
    fn id(&self) -> String {
        self.store.custom_key_store_id.clone()
    }

    fn name(&self) -> String {
        self.store.custom_key_store_name.clone()
    }
}

/// A fresh custom key store id, minted the way the service mints one.
fn new_id() -> String {
    mint_custom_key_store_id().expect("the platform produces entropy for a key store id")
}

/// A row for a key store backed by an CloudHSM cluster. `connection_error_code` is
/// what the record holds, which the reader reports only while the connection state
/// is `FAILED`.
fn cloud_hsm_row(
    case: &'static str,
    region: &str,
    name: &str,
    cloud_hsm_cluster_id: &str,
    trust_anchor_certificate: &str,
    connection_state: &str,
    connection_error_code: Option<&str>,
) -> Row {
    let id = new_id();
    let mut store = CustomKeyStore::new(
        id.clone(),
        region.to_string(),
        name.to_string(),
        CustomKeyStoreBacking::CloudHsm(CloudHsmCluster {
            cloud_hsm_cluster_id: cloud_hsm_cluster_id.to_string(),
            trust_anchor_certificate: trust_anchor_certificate.to_string(),
        }),
    );
    store.connection_state = connection_state.to_string();
    store.connection_error_code = connection_error_code.map(str::to_string);

    let mut expected = json!({
        "CustomKeyStoreId": id,
        "CustomKeyStoreName": name,
        "CustomKeyStoreType": "AWS_CLOUDHSM",
        "CloudHsmClusterId": cloud_hsm_cluster_id,
        "TrustAnchorCertificate": trust_anchor_certificate,
        "ConnectionState": connection_state,
    });
    if let Some(code) = connection_error_code.filter(|_| connection_state == "FAILED") {
        expected["ConnectionErrorCode"] = json!(code);
    }
    Row {
        case,
        store,
        expected,
    }
}

/// A row for a key store backed by an external key store proxy. The VPC endpoint
/// service members are held on the record whatever the connectivity is, so the row
/// pins that the reader reports them only for a proxy reached through one.
#[allow(clippy::too_many_arguments)]
fn external_row(
    case: &'static str,
    region: &str,
    name: &str,
    connectivity: &str,
    access_key_id: &str,
    uri_endpoint: &str,
    uri_path: &str,
    vpc_endpoint_service_name: &str,
    vpc_endpoint_service_owner: &str,
    connection_state: &str,
    connection_error_code: Option<&str>,
) -> Row {
    let id = new_id();
    let mut store = CustomKeyStore::new(
        id.clone(),
        region.to_string(),
        name.to_string(),
        CustomKeyStoreBacking::External(XksProxy {
            connectivity: connectivity.to_string(),
            access_key_id: access_key_id.to_string(),
            uri_endpoint: uri_endpoint.to_string(),
            uri_path: uri_path.to_string(),
            vpc_endpoint_service_name: Some(vpc_endpoint_service_name.to_string()),
            vpc_endpoint_service_owner: Some(vpc_endpoint_service_owner.to_string()),
        }),
    );
    store.connection_state = connection_state.to_string();
    store.connection_error_code = connection_error_code.map(str::to_string);

    let mut proxy = json!({
        "Connectivity": connectivity,
        "AccessKeyId": access_key_id,
        "UriEndpoint": uri_endpoint,
        "UriPath": uri_path,
    });
    if connectivity == "VPC_ENDPOINT_SERVICE" {
        proxy["VpcEndpointServiceName"] = json!(vpc_endpoint_service_name);
        proxy["VpcEndpointServiceOwner"] = json!(vpc_endpoint_service_owner);
    }
    let mut expected = json!({
        "CustomKeyStoreId": id,
        "CustomKeyStoreName": name,
        "CustomKeyStoreType": "EXTERNAL_KEY_STORE",
        "ConnectionState": connection_state,
        "XksProxyConfiguration": proxy,
    });
    if let Some(code) = connection_error_code.filter(|_| connection_state == "FAILED") {
        expected["ConnectionErrorCode"] = json!(code);
    }
    Row {
        case,
        store,
        expected,
    }
}

/// The table: the key stores seeded into the store, in the order they are created.
///
/// The connection states and connection error codes are entries of the modelled
/// `ConnectionStateType` and `ConnectionErrorCodeType` value sets
/// (`service_com_amazonaws_kms_trentservice::ops::types`, generated from the model),
/// and none of them is the value the issue cited.
fn rows() -> Vec<Row> {
    vec![
        cloud_hsm_row(
            "an CloudHSM key store that has never been connected",
            REGION,
            "a",
            "cluster-2vkpqmfjzck",
            "-----BEGIN CERTIFICATE-----\nZmlyc3Q=\n-----END CERTIFICATE-----",
            "DISCONNECTED",
            None,
        ),
        cloud_hsm_row(
            "an CloudHSM key store whose connection attempt failed",
            REGION,
            &"n".repeat(MAX_NAME_LEN),
            "cluster-4xvbjs6r2tzqvmi",
            "-----BEGIN CERTIFICATE-----\nc2Vjb25k\n-----END CERTIFICATE-----",
            "FAILED",
            Some("SUBNET_NOT_FOUND"),
        ),
        cloud_hsm_row(
            "an CloudHSM key store that failed once and is connected now",
            REGION,
            "hsm-recovered",
            "cluster-7bzcmrtvj4k5",
            "-----BEGIN CERTIFICATE-----\ndGhpcmQ=\n-----END CERTIFICATE-----",
            "CONNECTED",
            Some("INSUFFICIENT_CLOUDHSM_HSMS"),
        ),
        external_row(
            "an external key store proxy on a public endpoint",
            REGION,
            "xks-public",
            "PUBLIC_ENDPOINT",
            "ABCDEFGHIJKLMNOPQRST",
            "https://xks.example.com",
            "/kms/xks/v1",
            "com.amazonaws.vpce.us-east-1.vpce-svc-0abcdef1234567890",
            "111122223333",
            "CONNECTED",
            None,
        ),
        external_row(
            "an external key store proxy behind a VPC endpoint service",
            REGION,
            "xks-vpce",
            "VPC_ENDPOINT_SERVICE",
            "UVWXYZ234567ABCDEFGHIJKLMNOP",
            "https://xks-vpce.example.com",
            "/example/path/kms/xks/v12",
            "com.amazonaws.vpce.us-east-1.vpce-svc-1234567890abcdef0",
            "444455556666",
            "CONNECTED",
            None,
        ),
        external_row(
            "an external key store proxy that could not be reached",
            REGION,
            "xks-unreachable",
            "VPC_ENDPOINT_SERVICE",
            "GHIJKLMNOPQRSTUVWXYZ23",
            "https://xks-down.example.com",
            "/kms/xks/v2",
            "eu.amazonaws.vpce.eu-west-2.vpce-svc-2222233333444445",
            "777788889999",
            "FAILED",
            Some("XKS_PROXY_NOT_REACHABLE"),
        ),
        cloud_hsm_row(
            "a key store of another Region of the same account",
            OTHER_REGION,
            "elsewhere",
            "cluster-5kq2jmzvb7tx",
            "-----BEGIN CERTIFICATE-----\nb3RoZXI=\n-----END CERTIFICATE-----",
            "CONNECTED",
            None,
        ),
    ]
}

/// Seed `rows` into the store the router was built over, in order, so every later
/// request observes them.
fn seed(engine: &Arc<engine::InMemoryEngine>, rows: &[Row]) {
    let state = service_state(&**engine).expect("the KMS state store is reachable");
    let mut state = state.lock();
    for row in rows {
        state.insert_custom_key_store(row.store.clone());
    }
}

/// Forget one key store, as a delete would — what makes a `Marker` minted for the
/// list it was in resume nowhere.
fn forget(engine: &Arc<engine::InMemoryEngine>, custom_key_store_id: &str) {
    let state = service_state(&**engine).expect("the KMS state store is reachable");
    let mut state = state.lock();
    state.custom_key_stores.remove(custom_key_store_id);
    state
        .custom_key_store_order
        .retain(|held| held != custom_key_store_id);
}

/// One `DescribeCustomKeyStores` answer: its entries, its `Truncated` flag and its
/// `NextMarker`, with the invariant between the last two checked here once.
async fn describe_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    request: &Value,
) -> (Vec<Value>, bool, Option<String>) {
    let answer = ok_in(router, region, "DescribeCustomKeyStores", request).await;
    let entries = answer["CustomKeyStores"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a CustomKeyStores list in {answer}"))
        .clone();
    let truncated = answer["Truncated"]
        .as_bool()
        .unwrap_or_else(|| panic!("case {case}: expected a Truncated flag in {answer}"));
    let next_marker = answer["NextMarker"].as_str().map(str::to_string);
    assert_eq!(
        next_marker.is_some(),
        truncated,
        "case {case}: NextMarker is present exactly while the list is truncated, got {answer}"
    );
    (entries, truncated, next_marker)
}

/// [`describe_in`], for the Region most rows live in.
async fn describe(
    router: &MultiServiceRouter,
    case: &str,
    request: &Value,
) -> (Vec<Value>, bool, Option<String>) {
    describe_in(router, case, REGION, request).await
}

/// One reported entry without its `CreationDate`, having checked that the entry
/// carries one: the instant comes from the clock, so the row fixes every other
/// member and this one only has to be a timestamp.
fn without_creation_date(case: &str, entry: &Value) -> Value {
    let mut members = entry
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: expected a key store entry, got {entry}"))
        .clone();
    let creation_date = members.remove("CreationDate").unwrap_or(Value::Null);
    assert!(
        creation_date.as_f64().is_some_and(|seconds| seconds > 0.0),
        "case {case}: expected a CreationDate timestamp, got {entry}"
    );
    Value::Object(members)
}

/// Every enumerated member an entry reports carries a value the MODEL admits, from
/// the generated value sets in
/// `service_com_amazonaws_kms_trentservice::ops::types`.
fn assert_modelled_values(case: &str, entry: &Value) {
    for (member, values) in [
        ("ConnectionState", CONNECTION_STATE_TYPE_VALUES),
        ("CustomKeyStoreType", CUSTOM_KEY_STORE_TYPE_VALUES),
        ("ConnectionErrorCode", CONNECTION_ERROR_CODE_TYPE_VALUES),
    ] {
        if let Some(reported) = entry[member].as_str() {
            assert!(
                values.contains(&reported),
                "case {case}: {member} must be a modelled value, got {reported:?}"
            );
        }
    }
    if let Some(connectivity) = entry["XksProxyConfiguration"]["Connectivity"].as_str() {
        assert!(
            XKS_PROXY_CONNECTIVITY_TYPE_VALUES.contains(&connectivity),
            "case {case}: Connectivity must be a modelled value, got {connectivity:?}"
        );
    }
}

/// With no selector, `DescribeCustomKeyStores` reports every custom key store of the
/// account and Region it was signed for, in creation order, projecting each record
/// onto exactly the members a key store of its type and connection state has — and
/// it reports nothing at all for an account that has none.
#[tokio::test]
async fn describe_custom_key_stores_reports_the_key_stores_of_the_account_and_region() {
    let empty = router();
    let (entries, truncated, next_marker) =
        describe(&empty, "no custom key stores at all", &json!({})).await;
    assert!(
        entries.is_empty() && !truncated && next_marker.is_none(),
        "case no custom key stores at all: expected an empty, untruncated list, got \
         {entries:?} truncated={truncated}"
    );

    let (router, engine) = router_with_engine();
    let rows = rows();
    seed(&engine, &rows);

    let (entries, truncated, next_marker) = describe(&router, "every key store", &json!({})).await;
    let expected_here: Vec<&Row> = rows.iter().filter(|row| row.store.region == REGION).collect();
    assert_eq!(
        entries.len(),
        expected_here.len(),
        "case every key store: expected {} entries of {REGION}, got {entries:?}",
        expected_here.len()
    );
    assert!(
        !truncated && next_marker.is_none(),
        "case every key store: a list that fits is not truncated, got truncated={truncated} \
         marker={next_marker:?}"
    );
    for (row, entry) in expected_here.iter().zip(&entries) {
        assert_modelled_values(row.case, entry);
        assert_eq!(
            without_creation_date(row.case, entry),
            row.expected,
            "case {}: expected exactly the members a key store of its type and connection \
             state has, got {entry}",
            row.case
        );
    }

    // Reading reports; it never changes what a later caller sees.
    let (again, _, _) = describe(&router, "read twice", &json!({})).await;
    assert_eq!(
        again, entries,
        "case read twice: a report must not change what the next read answers"
    );

    // A custom key store is a regional resource: another Region's endpoint of the
    // same account serves only its own.
    let (elsewhere, _, _) = describe_in(&router, "another Region", OTHER_REGION, &json!({})).await;
    let other: Vec<&Row> = rows
        .iter()
        .filter(|row| row.store.region == OTHER_REGION)
        .collect();
    assert_eq!(
        elsewhere.len(),
        other.len(),
        "case another Region: expected {} entries of {OTHER_REGION}, got {elsewhere:?}",
        other.len()
    );
    for (row, entry) in other.iter().zip(&elsewhere) {
        assert_eq!(
            without_creation_date(row.case, entry),
            row.expected,
            "case {}: another Region's endpoint reports its own key stores, got {entry}",
            row.case
        );
    }
}

/// Either selector narrows the report to the ONE key store it names, and reports the
/// identical record the unnarrowed list does.
#[tokio::test]
async fn describe_custom_key_stores_narrows_to_the_key_store_a_selector_names() {
    let (router, engine) = router_with_engine();
    let rows = rows();
    seed(&engine, &rows);

    for row in rows.iter().filter(|row| row.store.region == REGION) {
        for (selector, request) in [
            ("by id", json!({"CustomKeyStoreId": row.id()})),
            ("by name", json!({"CustomKeyStoreName": row.name()})),
        ] {
            let case = format!("{} {selector}", row.case);
            let (entries, truncated, next_marker) = describe(&router, &case, &request).await;
            assert_eq!(
                entries.len(),
                1,
                "case {case}: a selector names at most one key store, got {entries:?}"
            );
            assert!(
                !truncated && next_marker.is_none(),
                "case {case}: a single-entry list is not truncated, got truncated={truncated}"
            );
            assert_eq!(
                without_creation_date(&case, &entries[0]),
                row.expected,
                "case {case}: the narrowed report must be the record itself, got {:?}",
                entries[0]
            );
        }
    }
}

/// A selector that names no custom key store of this account and Region is refused
/// as a missing resource — never as an empty list — and a request that gives both
/// selectors at once is refused as an invalid request. Both refusals carry the error
/// code alone, with no message member.
#[tokio::test]
async fn describe_custom_key_stores_refuses_a_selection_that_names_no_key_store() {
    let (router, engine) = router_with_engine();
    let rows = rows();
    seed(&engine, &rows);
    let here = &rows[0];
    let elsewhere = rows
        .iter()
        .find(|row| row.store.region == OTHER_REGION)
        .expect("the table holds a key store of another Region");

    let refusals: Vec<(String, Value, &str)> = vec![
        (
            "an id no key store has".to_string(),
            json!({"CustomKeyStoreId": new_id()}),
            "CustomKeyStoreNotFoundException",
        ),
        (
            "a name no key store has".to_string(),
            json!({"CustomKeyStoreName": "no-such-key-store"}),
            "CustomKeyStoreNotFoundException",
        ),
        (
            "an id that is really a name".to_string(),
            json!({"CustomKeyStoreId": here.name()}),
            "CustomKeyStoreNotFoundException",
        ),
        (
            "a name that is really an id".to_string(),
            json!({"CustomKeyStoreName": here.id()}),
            "CustomKeyStoreNotFoundException",
        ),
        (
            "the id of a key store of another Region".to_string(),
            json!({"CustomKeyStoreId": elsewhere.id()}),
            "CustomKeyStoreNotFoundException",
        ),
        (
            "the name of a key store of another Region".to_string(),
            json!({"CustomKeyStoreName": elsewhere.name()}),
            "CustomKeyStoreNotFoundException",
        ),
        (
            "both selectors, naming the same key store".to_string(),
            json!({"CustomKeyStoreId": here.id(), "CustomKeyStoreName": here.name()}),
            "ValidationException",
        ),
        (
            "both selectors, naming two different key stores".to_string(),
            json!({"CustomKeyStoreId": here.id(), "CustomKeyStoreName": elsewhere.name()}),
            "ValidationException",
        ),
        (
            "both selectors, neither naming anything".to_string(),
            json!({"CustomKeyStoreId": new_id(), "CustomKeyStoreName": "no-such-key-store"}),
            "ValidationException",
        ),
    ];
    for (case, request, expected) in refusals {
        let (status, payload) = call(&router, REGION, "DescribeCustomKeyStores", &request).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, expected),
            "case {case}: expected HTTP 400 {expected}, got {status} with {payload}"
        );
        // The refusal is the error code alone: the service sends no message text with
        // it, so none is invented.
        for member in ["message", "Message"] {
            let text = payload[member].as_str().unwrap_or_default();
            assert!(
                text.is_empty(),
                "case {case}: the refusal carries no {member} text, got {payload}"
            );
        }
    }
}

/// `Limit` bounds a page and `Marker` resumes after the last key store of the
/// previous one, so the pages of any page size concatenate to the whole list exactly
/// once each; a `Limit` outside the modelled range, and a `Marker` this list did not
/// mint, are refused.
#[tokio::test]
async fn describe_custom_key_stores_pages_the_list_by_limit_and_marker() {
    let (router, engine) = router_with_engine();
    let rows = rows();
    seed(&engine, &rows);
    let here: Vec<String> = rows
        .iter()
        .filter(|row| row.store.region == REGION)
        .map(Row::id)
        .collect();

    // One row per page size: the smallest page, pages that divide the list and ones
    // that do not, the exact length, one past it, and the largest `Limit` the
    // modelled range admits.
    for limit in [1_i64, 2, 3, 5, here.len() as i64, here.len() as i64 + 1, MAX_LIMIT] {
        let case = format!("Limit {limit}");
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"Limit": limit});
            if let Some(token) = &marker {
                request["Marker"] = json!(token);
            }
            let (entries, truncated, next_marker) = describe(&router, &case, &request).await;
            pages += 1;
            assert!(
                i64::try_from(entries.len()).unwrap_or(i64::MAX) <= limit,
                "case {case}: a page holds at most {limit} entries, got {}",
                entries.len()
            );
            for entry in &entries {
                walked.push(
                    entry["CustomKeyStoreId"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case}: expected an id in {entry}"))
                        .to_string(),
                );
            }
            if !truncated {
                break;
            }
            marker = next_marker;
            assert!(
                pages <= here.len(),
                "case {case}: paging must terminate, walked {walked:?}"
            );
        }
        assert_eq!(
            walked, here,
            "case {case}: the pages must concatenate to the whole list in creation order"
        );
        let expected_pages = here.len().div_ceil(usize::try_from(limit).unwrap_or(1));
        assert_eq!(
            pages,
            expected_pages.max(1),
            "case {case}: expected {} pages of {} key stores",
            expected_pages.max(1),
            here.len()
        );
    }

    // A `Limit` outside `LimitType`'s modelled 1..1000 range never reaches the
    // listing at all.
    for limit in [0_i64, -1, MAX_LIMIT + 1] {
        let case = format!("Limit {limit}");
        let (status, code) = failure_in(
            &router,
            REGION,
            "DescribeCustomKeyStores",
            &json!({"Limit": limit}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a Limit outside the modelled range is refused, got {status} {code}"
        );
    }

    // A `Marker` is a token THIS list minted, and names a key store the selection
    // still holds.
    let (_, _, first_page_marker) =
        describe(&router, "a marker to reuse", &json!({"Limit": 1})).await;
    let minted = first_page_marker.expect("a truncated list carries a NextMarker");
    let mut invalid: Vec<(String, String)> = vec![
        (
            "a marker that is not a token at all".to_string(),
            "not-a-token".to_string(),
        ),
        (
            "a marker minted for another listing".to_string(),
            encode_marker(&here[0]),
        ),
        (
            "a token of the right shape over nothing".to_string(),
            hex::encode("custom-key-stores/1/"),
        ),
    ];
    // The key store a minted marker names is gone: the token resumes nowhere, rather
    // than at whichever key store now sits in that position.
    forget(&engine, &here[0]);
    invalid.push((
        "a marker whose key store is gone".to_string(),
        minted.clone(),
    ));
    for (case, marker) in invalid {
        let (status, payload) = call(
            &router,
            REGION,
            "DescribeCustomKeyStores",
            &json!({"Marker": marker}),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, "InvalidMarkerException"),
            "case {case}: expected HTTP 400 InvalidMarkerException, got {status} with {payload}"
        );
        for member in ["message", "Message"] {
            let text = payload[member].as_str().unwrap_or_default();
            assert!(
                text.is_empty(),
                "case {case}: the refusal carries no {member} text, got {payload}"
            );
        }
    }

    let (entries, _, _) = describe(&router, "after forgetting one", &json!({})).await;
    assert_eq!(
        entries.len(),
        here.len() - 1,
        "case after forgetting one: the reader reports what the store holds, got {entries:?}"
    );
}
