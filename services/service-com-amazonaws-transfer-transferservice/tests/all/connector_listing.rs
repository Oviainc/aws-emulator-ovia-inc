// SPDX-License-Identifier: CC0-1.0

//! `ListConnectors` over the whole class of stored connector sets.
//!
//! Every call goes over the real wire path through the shared [`crate::harness`] —
//! `POST /` with `X-Amz-Target: TransferService.ListConnectors` — so what is asserted
//! is what an SDK sees.
//!
//! RULE under test: a listing reports EVERY stored connector exactly once, in a stable
//! `ConnectorId` order, projecting each one's stored `Arn`, `ConnectorId` and `Url`; it
//! answers at most `MaxResults` records per call with a `NextToken` only while more
//! remain, resumes exactly where that token points, refuses a token it did not issue
//! with `InvalidNextTokenException` (400), and changes nothing a later caller sees.
//!
//! The rows differ along what the rule generalises over: how many connectors are
//! stored (none, one, several), the form of each one's `Url` (absent, empty, AS2 over
//! HTTPS, SFTP, one at `Url`'s modelled maximum length) and the page bound requested
//! (both published bounds of `MaxResults`' modelled `range 1..1000`, sizes above,
//! below and equal to the stored count, and no bound at all).
//!
//! These suites SEED the store directly — through the one shared engine the handlers
//! read, see `state_roundtrip.rs` — rather than through `CreateConnector`, so a row can
//! carry a `Url` form a create does not store (absent on a service-managed connector,
//! or the empty string at `Url`'s modelled lower bound) and the listing is still
//! exercised over it. Nothing here bypasses the handler being tested: the records are
//! written exactly as `CreateConnector` writes them and read back over the wire.

use serde_json::{json, Value};

use service_com_amazonaws_transfer_transferservice::ops::types::SftpConnectorConfig;
use service_com_amazonaws_transfer_transferservice::state::{
    connector_arn, Connector, CONNECTOR_STATUS_ACTIVE, DEFAULT_CONNECTOR_EGRESS_TYPE,
    DEFAULT_MAX_CONCURRENT_CONNECTIONS, MAX_STORED_CONNECTORS,
};

use crate::harness::{drain_pages, Answer, Emulator, ACCOUNT, REGION, ROLE};

/// A url of exactly 255 characters: `Url`'s modelled `len 0..255` upper bound.
fn url_at_max_length() -> String {
    let prefix = "https://partner.example.com/";
    format!("{prefix}{}", "a".repeat(255 - prefix.len()))
}

/// The `Url` forms a stored connector can carry, one row each.
///
/// `None` is the VPC Lattice connector `Url`'s own documentation describes ("For VPC
/// Lattice type connectors, the URL must be null"); `Some("")` is the lower bound of
/// `Url`'s modelled `len 0..255`, [`url_at_max_length`] the upper one.
fn url_rows() -> Vec<Option<String>> {
    vec![
        Some("https://partner.example.com/as2".to_string()),
        None,
        Some(String::new()),
        Some("sftp://partner.example.com".to_string()),
        Some(url_at_max_length()),
    ]
}

/// Seed one connector per entry of `urls`, exactly as a connector write will: a minted
/// `ConnectorId`, the ARN built by the crate's own [`connector_arn`], and the url as
/// given. Returns the stored `(ConnectorId, Url)` pairs in `ConnectorId` order.
fn seed_connectors(emulator: &Emulator, urls: &[Option<String>]) -> Vec<(String, Option<String>)> {
    let store = emulator.store();
    let mut guard = store.lock();
    let mut seeded = Vec::with_capacity(urls.len());
    for url in urls {
        let connector_id = guard.mint_id("c");
        let connector = Connector {
            connector_id: connector_id.clone(),
            arn: connector_arn(REGION, &connector_id),
            url: url.clone(),
            // The rest of the record is what a service-managed SFTP connector write
            // stores (`CreateConnector` resolves exactly these); a LISTING projects
            // none of them, which is part of what these suites assert.
            as2_config: None,
            sftp_config: Some(SftpConnectorConfig {
                user_secret_id: Some("sftp-partner-credentials".to_string()),
                trusted_host_keys: None,
                max_concurrent_connections: DEFAULT_MAX_CONCURRENT_CONNECTIONS,
                ordered_user_secret_version_stages: None,
            }),
            access_role: ROLE.to_string(),
            logging_role: None,
            security_policy_name: None,
            egress_config: None,
            egress_type: DEFAULT_CONNECTOR_EGRESS_TYPE.to_string(),
            ip_address_type: None,
            status: CONNECTOR_STATUS_ACTIVE.to_string(),
            tags: Vec::new(),
        };
        guard.connectors.insert(connector_id.clone(), connector);
        seeded.push((connector_id, url.clone()));
    }
    seeded.sort();
    seeded
}

/// One `ListConnectors` answer and its `Connectors` member.
///
/// The decoded [`Answer`] travels along, so the status, the body and the harness's own
/// `error_code()` are read from the one place every other suite reads them.
async fn list_once(emulator: &Emulator, request: Value) -> (Answer, Vec<Value>) {
    let answer = emulator.call("ListConnectors", request).await;
    let items = answer
        .get("Connectors")
        .as_array()
        .cloned()
        .unwrap_or_default();
    (answer, items)
}

/// RULE (empty set): a store holding no connector answers HTTP 200 with an EMPTY
/// `Connectors` list and no `NextToken` — never a refusal, and never a token that would
/// walk a second page.
#[tokio::test]
async fn an_empty_store_lists_no_connectors_and_hands_out_no_token() {
    let emulator = Emulator::new();

    // Rows: no page bound, the smallest and the largest modelled one.
    for limit in [None, Some(1), Some(1000)] {
        let mut request = json!({});
        if let Some(limit) = limit {
            request["MaxResults"] = json!(limit);
        }
        let case = format!("empty store, MaxResults={limit:?}");
        let (answer, items) = list_once(&emulator, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            items.is_empty(),
            "case {case:?}: expected no connectors, got {items:?}"
        );
        assert!(
            answer.body.get("NextToken").is_none(),
            "case {case:?}: expected no NextToken, got body {}",
            answer.body
        );
    }
}

/// RULE: every stored connector is listed exactly once in `ConnectorId` order, with its
/// stored `Arn` and `ConnectorId`, and its `Url` reported only when it has one.
#[tokio::test]
async fn every_stored_connector_is_listed_once_with_its_stored_members() {
    let emulator = Emulator::new();
    let seeded = seed_connectors(&emulator, &url_rows());

    let (answer, items) = list_once(&emulator, json!({})).await;
    assert_eq!(
        answer.status, 200,
        "case \"unbounded listing\": expected HTTP 200, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        items.len(),
        seeded.len(),
        "case \"unbounded listing\": expected {} connector(s), got {items:?}",
        seeded.len()
    );

    for (index, (connector_id, url)) in seeded.iter().enumerate() {
        let case = format!("connector {connector_id} with Url={url:?}");
        let listed = items
            .get(index)
            .unwrap_or_else(|| panic!("case {case:?}: expected a record at {index}: {items:?}"));
        assert_eq!(
            listed.get("ConnectorId").and_then(Value::as_str),
            Some(connector_id.as_str()),
            "case {case:?}: expected the stored ConnectorId in order, got {listed:?}"
        );
        assert_eq!(
            listed.get("Arn").and_then(Value::as_str),
            Some(format!("arn:aws:transfer:{REGION}:{ACCOUNT}:connector/{connector_id}").as_str()),
            "case {case:?}: expected the stored ARN, got {:?}",
            listed.get("Arn")
        );
        match url {
            Some(url) => assert_eq!(
                listed.get("Url").and_then(Value::as_str),
                Some(url.as_str()),
                "case {case:?}: expected the stored Url, got {:?}",
                listed.get("Url")
            ),
            None => assert!(
                listed.get("Url").is_none_or(Value::is_null),
                "case {case:?}: expected no Url at all, got {:?}",
                listed.get("Url")
            ),
        }
    }

    // A read is not a write: a second listing answers the same page, and the store
    // still holds exactly what was seeded.
    let (_, again) = list_once(&emulator, json!({})).await;
    assert_eq!(
        again, items,
        "case \"second listing\": a read must not change what a later caller sees"
    );
    assert_eq!(
        emulator.store().lock().connectors.len(),
        seeded.len(),
        "case \"second listing\": expected the store untouched by the reads"
    );
}

/// RULE (same page contract as the other `@paginated` operations): a listing answers at
/// most `MaxResults` connectors, hands back a `NextToken` only while more remain, and
/// resuming from that token walks the full set exactly once in `ConnectorId` order.
///
/// The rows are the page bounds crossed with the stored count: both published bounds of
/// `MaxResults`' modelled `range 1..1000`, bounds below/equal/above the count, and no
/// bound at all.
#[tokio::test]
async fn list_connectors_honours_every_page_bound_and_resumes_from_its_token() {
    for stored_count in [1_usize, 2, 5] {
        let emulator = Emulator::new();
        let urls: Vec<Option<String>> = url_rows().into_iter().take(stored_count).collect();
        let seeded = seed_connectors(&emulator, &urls);
        let ids: Vec<String> = seeded.iter().map(|(id, _)| id.clone()).collect();

        for limit in [Some(1), Some(2), Some(4), Some(5), Some(6), Some(1000), None] {
            let case = format!("{stored_count} stored, MaxResults={limit:?}");
            let (page_sizes, keys) = drain_pages(
                &emulator,
                "ListConnectors",
                &json!({}),
                "Connectors",
                "ConnectorId",
                limit,
            )
            .await;
            assert_eq!(
                keys, ids,
                "case {case:?}: expected every connector once in ConnectorId order, got {keys:?}"
            );
            match limit {
                Some(limit) => {
                    let limit = usize::try_from(limit).expect("a positive page bound");
                    for size in &page_sizes {
                        assert!(
                            *size <= limit,
                            "case {case:?}: a page carried {size} records, above the bound {limit}"
                        );
                    }
                    let want_pages = ids.len().div_ceil(limit);
                    assert_eq!(
                        page_sizes.len(),
                        want_pages,
                        "case {case:?}: expected {want_pages} page(s), got {page_sizes:?}"
                    );
                }
                None => assert_eq!(
                    page_sizes,
                    vec![ids.len()],
                    "case {case:?}: expected a single unbounded page, got {page_sizes:?}"
                ),
            }
        }
    }
}

/// RULE (refusals): a `NextToken` this service did not issue is refused with
/// `InvalidNextTokenException` (400) rather than silently restarting the listing, and a
/// `MaxResults` outside the modelled `range 1..1000` is refused at the boundary while
/// both published bounds are served.
#[tokio::test]
async fn an_unissued_next_token_and_an_out_of_range_max_results_are_refused() {
    let emulator = Emulator::new();
    seed_connectors(&emulator, &url_rows());

    // Rows: not hex at all, odd-length hex, and valid hex that is not UTF-8.
    for token in ["not-a-token", "zz", "abc", "ff", "c0ffee00"] {
        let case = format!("NextToken={token:?}");
        let (answer, _) = list_once(&emulator, json!({"NextToken": token})).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidNextTokenException",
            "case {case:?}: expected InvalidNextTokenException, got body {}",
            answer.body
        );
    }

    // One PAST each published bound of `MaxResults`' modelled range 1..1000.
    for bad in [0, 1001] {
        let case = format!("MaxResults={bad}");
        let (answer, _) = list_once(&emulator, json!({"MaxResults": bad})).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got body {}",
            answer.body
        );
    }

    // Both published bounds themselves are served.
    for good in [1, 1000] {
        let case = format!("MaxResults={good}");
        let (answer, items) = list_once(&emulator, json!({"MaxResults": good})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            !items.is_empty(),
            "case {case:?}: expected the seeded connectors to be listed, got {items:?}"
        );
    }

    // A token the service DID issue resumes the listing rather than being refused.
    let (answer, first_page) = list_once(&emulator, json!({"MaxResults": 1})).await;
    let token = answer
        .get("NextToken")
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case \"issued token\": expected a NextToken, got {}",
                answer.body
            )
        })
        .to_string();
    let (resumed, second_page) =
        list_once(&emulator, json!({"MaxResults": 1, "NextToken": token})).await;
    assert_eq!(
        resumed.status, 200,
        "case \"issued token\": expected HTTP 200, got {} with body {}",
        resumed.status, resumed.body
    );
    assert_ne!(
        second_page, first_page,
        "case \"issued token\": expected the next record, got the first page again"
    );
}

/// RULE (the bounded store): the connector map has a CAPACITY, so a connector write has
/// a bound to refuse at instead of growing it without end —
/// `has_connector_capacity` answers true below [`MAX_STORED_CONNECTORS`], false at it,
/// and true again once a record is released; it is the connector counterpart of the
/// `has_server_capacity` bound the server map already carries. A store filled to the
/// bound still pages through `ListConnectors` exactly once.
///
/// The rows are the bound's neighbourhood: an empty store, one below the bound, at the
/// bound, and after a release. The constant is read from the crate so the test cannot
/// drift from the code.
#[tokio::test]
async fn the_connector_store_is_bounded_and_releases_its_capacity_again() {
    let emulator = Emulator::new();

    assert!(
        emulator.store().lock().has_connector_capacity(),
        "case \"empty store\": expected capacity for a connector"
    );

    let urls: Vec<Option<String>> = (0..MAX_STORED_CONNECTORS - 1)
        .map(|n| Some(format!("https://partner{n:03}.example.com/as2")))
        .collect();
    seed_connectors(&emulator, &urls);
    assert!(
        emulator.store().lock().has_connector_capacity(),
        "case \"one below the bound\": expected capacity for one more connector"
    );

    let last = seed_connectors(&emulator, &[None]);
    assert!(
        !emulator.store().lock().has_connector_capacity(),
        "case \"at the bound\": expected NO capacity at {MAX_STORED_CONNECTORS} connectors"
    );

    // Filled to the bound, the listing still walks every record exactly once.
    let (page_sizes, keys) = drain_pages(
        &emulator,
        "ListConnectors",
        &json!({}),
        "Connectors",
        "ConnectorId",
        Some(50),
    )
    .await;
    assert_eq!(
        keys.len(),
        MAX_STORED_CONNECTORS,
        "case \"listing a full store\": expected {MAX_STORED_CONNECTORS} records over {page_sizes:?}"
    );
    let mut unique = keys.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique, keys,
        "case \"listing a full store\": expected each ConnectorId once, in order"
    );

    // Releasing a record frees that capacity again.
    let released = last
        .first()
        .map(|(connector_id, _)| connector_id.clone())
        .expect("one connector was seeded last");
    emulator.store().lock().connectors.remove(&released);
    assert!(
        emulator.store().lock().has_connector_capacity(),
        "case \"after a release\": expected capacity again once a connector was released"
    );
}
