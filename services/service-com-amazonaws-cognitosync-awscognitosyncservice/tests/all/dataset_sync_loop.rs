// SPDX-License-Identifier: CC0-1.0

//! The dataset sync loop of Cognito Sync, end to end over HTTP.
//!
//! Every assertion here drives the real request path — `MultiServiceRouter::dispatch`,
//! the restJson1 codec, the generated routing table and the authored handlers — so what
//! it pins is the wire behaviour a client sees, not a handler's internals.
//!
//! The rules under test, all of them stated over a CLASS of inputs rather than one
//! recorded request:
//!
//! 1. A dataset's reported metadata is derived from the records stored for it:
//!    `NumRecords` is the record count, `DataStorage` the summed byte length of keys
//!    plus values, `DatasetSyncCount` the count the last successful `UpdateRecords`
//!    advanced it to, and `ListRecords`/`ListDatasets` `Count` the size of the page.
//! 2. `UpdateRecords` applies a batch only if no patch in it is behind the stored sync
//!    count of its record, and persists nothing otherwise (the operation's own doc).
//! 3. A record written by `UpdateRecords` reads back byte-identical through
//!    `ListRecords`, carrying the dataset's new sync count.
//! 4. Paging bounds a page at `MaxResults` and resumes exactly where the page stopped.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_cognitosync_awscognitosyncservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// An identity pool id matching the model's `^[\w-]+:[0-9a-f-]+$` pattern.
const POOL: &str = "us-east-1:00000000-0000-0000-0000-000000000001";
/// An identity id of that pool, matching the same pattern.
const IDENTITY: &str = "us-east-1:00000000-0000-0000-0000-0000000000a1";
/// A second identity, to pin that datasets are namespaced per identity.
const OTHER_IDENTITY: &str = "us-east-1:00000000-0000-0000-0000-0000000000b2";

/// One answer from the emulator: the status, the parsed body and the raw text (the raw
/// text is what an error assertion reads, so it never depends on an envelope's shape).
struct Reply {
    status: u16,
    body: Value,
    raw: String,
}

impl Reply {
    fn records(&self) -> Vec<(String, Option<String>, i64)> {
        self.body["Records"]
            .as_array()
            .map(|records| {
                records
                    .iter()
                    .map(|record| {
                        (
                            record["Key"].as_str().unwrap_or_default().to_string(),
                            record["Value"].as_str().map(str::to_string),
                            record["SyncCount"].as_i64().unwrap_or_default(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn record_keys(&self) -> Vec<String> {
        self.records().into_iter().map(|(key, _, _)| key).collect()
    }

    fn dataset_names(&self) -> Vec<String> {
        self.body["Datasets"]
            .as_array()
            .map(|datasets| {
                datasets
                    .iter()
                    .map(|dataset| {
                        dataset["DatasetName"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn next_token(&self) -> Option<String> {
        self.body["NextToken"].as_str().map(str::to_string)
    }

    fn sync_session_token(&self) -> String {
        self.body["SyncSessionToken"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    fn i64_at(&self, member: &str) -> Option<i64> {
        self.body[member].as_i64()
    }
}

/// The emulator under test: this one service mounted on one shared engine, reached
/// through the same router the production entrypoint serves.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn start() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the core's business and is exercised there; these
        // tests drive the service's own behaviour.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = body.map_or_else(Bytes::new, |body| Bytes::from(body.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://cognito-sync.us-east-1.amazonaws.com{path}"))
            // The host-style endpoint is what routes the request to this service.
            .header("host", "cognito-sync.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .body(payload)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let raw = String::from_utf8_lossy(&bytes).to_string();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Reply { status, body, raw }
    }

    async fn list_records(&self, identity: &str, dataset: &str, query: &str) -> Reply {
        let path =
            format!("/identitypools/{POOL}/identities/{identity}/datasets/{dataset}/records{query}");
        self.send("GET", &path, None).await
    }

    async fn update_records(
        &self,
        identity: &str,
        dataset: &str,
        token: &str,
        device_id: Option<&str>,
        patches: Value,
    ) -> Reply {
        let mut body = json!({ "SyncSessionToken": token, "RecordPatches": patches });
        if let Some(device_id) = device_id {
            body["DeviceId"] = json!(device_id);
        }
        let path = format!("/identitypools/{POOL}/identities/{identity}/datasets/{dataset}");
        self.send("POST", &path, Some(body)).await
    }

    async fn describe_dataset(&self, identity: &str, dataset: &str) -> Reply {
        let path = format!("/identitypools/{POOL}/identities/{identity}/datasets/{dataset}");
        self.send("GET", &path, None).await
    }

    async fn list_datasets(&self, identity: &str, query: &str) -> Reply {
        let path = format!("/identitypools/{POOL}/identities/{identity}/datasets{query}");
        self.send("GET", &path, None).await
    }

    async fn delete_dataset(&self, identity: &str, dataset: &str) -> Reply {
        let path = format!("/identitypools/{POOL}/identities/{identity}/datasets/{dataset}");
        self.send("DELETE", &path, None).await
    }

    /// `ListRecords` for a fresh sync session, then `UpdateRecords` with it: the pairing
    /// the model requires ("The SyncSessionToken returned by a previous call to
    /// ListRecords for this dataset and identity").
    async fn sync(
        &self,
        identity: &str,
        dataset: &str,
        device_id: Option<&str>,
        patches: Value,
    ) -> Reply {
        let session = self.list_records(identity, dataset, "").await;
        assert_eq!(
            session.status, 200,
            "ListRecords for a session token on '{dataset}': body {}",
            session.raw
        );
        let token = session.sync_session_token();
        self.update_records(identity, dataset, &token, device_id, patches)
            .await
    }
}

/// A `replace` patch, as a client sends it.
fn replace(key: &str, value: &str, sync_count: i64) -> Value {
    json!({ "Op": "replace", "Key": key, "Value": value, "SyncCount": sync_count })
}

/// A `remove` patch.
fn remove(key: &str, sync_count: i64) -> Value {
    json!({ "Op": "remove", "Key": key, "SyncCount": sync_count })
}

/// The whole workflow the issue's reproduction script walks, in order.
#[tokio::test]
async fn the_dataset_sync_loop_round_trips_from_first_list_to_delete() {
    let emulator = Emulator::start();

    // A dataset that was never written: not an error, just empty.
    let first = emulator.list_records(IDENTITY, "mygame", "").await;
    assert_eq!(
        first.status, 200,
        "ListRecords on an unwritten dataset: body {}",
        first.raw
    );
    assert_eq!(
        first.body["DatasetExists"].as_bool(),
        Some(false),
        "unwritten dataset: expected DatasetExists=false, got {}",
        first.raw
    );
    assert_eq!(
        first.i64_at("Count"),
        Some(0),
        "unwritten dataset: expected Count=0, got {}",
        first.raw
    );
    assert_eq!(
        first.records(),
        Vec::new(),
        "unwritten dataset: expected no records, got {}",
        first.raw
    );
    assert_eq!(
        first.i64_at("DatasetSyncCount"),
        Some(0),
        "unwritten dataset: expected DatasetSyncCount=0, got {}",
        first.raw
    );
    let first_token = first.sync_session_token();
    assert!(
        !first_token.is_empty(),
        "unwritten dataset: expected a SyncSessionToken, got {}",
        first.raw
    );

    // The first write creates the dataset implicitly and stamps sync count 1.
    let written = emulator
        .update_records(
            IDENTITY,
            "mygame",
            &first_token,
            Some("device-1"),
            json!([replace("highScore", "42", 0)]),
        )
        .await;
    assert_eq!(
        written.status, 200,
        "UpdateRecords on a new dataset: body {}",
        written.raw
    );
    assert_eq!(
        written.records(),
        vec![("highScore".to_string(), Some("42".to_string()), 1)],
        "UpdateRecords: expected highScore=42 at sync count 1, got {}",
        written.raw
    );

    // And the read sees exactly what was written.
    let second = emulator.list_records(IDENTITY, "mygame", "").await;
    assert_eq!(second.status, 200, "ListRecords after a write: {}", second.raw);
    assert_eq!(
        second.body["DatasetExists"].as_bool(),
        Some(true),
        "after a write: expected DatasetExists=true, got {}",
        second.raw
    );
    assert_eq!(
        (second.i64_at("Count"), second.i64_at("DatasetSyncCount")),
        (Some(1), Some(1)),
        "after a write: expected Count=1 and DatasetSyncCount=1, got {}",
        second.raw
    );
    assert_eq!(
        second.records(),
        vec![("highScore".to_string(), Some("42".to_string()), 1)],
        "after a write: expected the stored record back, got {}",
        second.raw
    );
    assert_eq!(
        second.body["LastModifiedBy"].as_str(),
        Some("device-1"),
        "after a write: expected LastModifiedBy=device-1, got {}",
        second.raw
    );
    let second_token = second.sync_session_token();
    assert_ne!(
        second_token, first_token,
        "each ListRecords mints its own sync session, got {second_token} twice"
    );

    // A stale patch is refused and changes nothing.
    let stale = emulator
        .update_records(
            IDENTITY,
            "mygame",
            &second_token,
            Some("device-1"),
            json!([replace("highScore", "7", 0)]),
        )
        .await;
    assert_eq!(
        stale.status, 409,
        "a stale patch: expected HTTP 409, got {} with body {}",
        stale.status, stale.raw
    );
    assert!(
        stale.raw.contains("ResourceConflictException"),
        "a stale patch: expected ResourceConflictException, got {}",
        stale.raw
    );
    let unchanged = emulator.list_records(IDENTITY, "mygame", "").await;
    assert_eq!(
        unchanged.records(),
        vec![("highScore".to_string(), Some("42".to_string()), 1)],
        "a refused patch must persist nothing, got {}",
        unchanged.raw
    );

    // Dataset metadata is computed from the stored records.
    let described = emulator.describe_dataset(IDENTITY, "mygame").await;
    assert_eq!(described.status, 200, "DescribeDataset: {}", described.raw);
    let dataset = &described.body["Dataset"];
    assert_eq!(
        dataset["DatasetName"].as_str(),
        Some("mygame"),
        "DescribeDataset: expected DatasetName=mygame, got {}",
        described.raw
    );
    assert_eq!(
        dataset["NumRecords"].as_i64(),
        Some(1),
        "DescribeDataset: expected NumRecords=1, got {}",
        described.raw
    );
    assert_eq!(
        dataset["DataStorage"].as_i64(),
        Some(("highScore".len() + "42".len()) as i64),
        "DescribeDataset: DataStorage is the summed byte length of keys and values, got {}",
        described.raw
    );
    assert_eq!(
        dataset["LastModifiedBy"].as_str(),
        Some("device-1"),
        "DescribeDataset: expected LastModifiedBy=device-1, got {}",
        described.raw
    );

    // The dataset is listed for its identity, and for no other.
    let listed = emulator.list_datasets(IDENTITY, "").await;
    assert_eq!(
        (listed.i64_at("Count"), listed.dataset_names()),
        (Some(1), vec!["mygame".to_string()]),
        "ListDatasets: expected the one stored dataset, got {}",
        listed.raw
    );
    let other = emulator.list_datasets(OTHER_IDENTITY, "").await;
    assert_eq!(
        (other.i64_at("Count"), other.dataset_names()),
        (Some(0), Vec::new()),
        "ListDatasets for another identity: datasets are per identity, got {}",
        other.raw
    );

    // Delete answers with the dataset as it stood, and the reads follow.
    let deleted = emulator.delete_dataset(IDENTITY, "mygame").await;
    assert_eq!(deleted.status, 200, "DeleteDataset: {}", deleted.raw);
    assert_eq!(
        (
            deleted.body["Dataset"]["DatasetName"].as_str(),
            deleted.body["Dataset"]["NumRecords"].as_i64()
        ),
        (Some("mygame"), Some(1)),
        "DeleteDataset: expected the deleted dataset back, got {}",
        deleted.raw
    );
    let gone = emulator.describe_dataset(IDENTITY, "mygame").await;
    assert_eq!(
        gone.status, 404,
        "DescribeDataset after delete: expected 404, got {} with body {}",
        gone.status, gone.raw
    );
    assert!(
        gone.raw.contains("ResourceNotFoundException"),
        "DescribeDataset after delete: expected ResourceNotFoundException, got {}",
        gone.raw
    );
    let emptied = emulator.list_datasets(IDENTITY, "").await;
    assert_eq!(
        (emptied.i64_at("Count"), emptied.dataset_names()),
        (Some(0), Vec::new()),
        "ListDatasets after delete: expected nothing listed, got {}",
        emptied.raw
    );
    let twice = emulator.delete_dataset(IDENTITY, "mygame").await;
    assert_eq!(
        twice.status, 404,
        "a second DeleteDataset: expected 404, got {} with body {}",
        twice.status, twice.raw
    );
}

/// One row of the round-trip table: a name, the patches sent, and the `(key, value)`
/// state the dataset must hold afterwards.
type RoundTripCase = (&'static str, Vec<Value>, Vec<(&'static str, Option<String>)>);

/// RULE 1 and 3 over a class of batches: whatever a batch writes reads back
/// byte-identical, and the dataset's `NumRecords`/`DataStorage` follow from it.
///
/// The rows differ along what the rule generalises over — record count, value size,
/// byte width of the characters, an empty and an absent value, and insertion order
/// against listing order.
#[tokio::test]
async fn whatever_a_batch_writes_reads_back_and_sizes_the_dataset() {
    let long_value = "x".repeat(4096);
    // (case, patches sent, expected (key, value) state afterwards)
    let cases: Vec<RoundTripCase> = vec![
        (
            "one ascii record",
            vec![replace("highScore", "42", 0)],
            vec![("highScore", Some("42".to_string()))],
        ),
        (
            "an empty value is a value",
            vec![replace("emptyValue", "", 0)],
            vec![("emptyValue", Some(String::new()))],
        ),
        (
            "a replace with no Value member at all",
            vec![json!({ "Op": "replace", "Key": "valueless", "SyncCount": 0 })],
            vec![("valueless", None)],
        ),
        (
            "several records, written out of order",
            vec![
                replace("zeta", "last", 0),
                replace("alpha", "first", 0),
                replace("mid", "middle", 0),
            ],
            vec![
                ("alpha", Some("first".to_string())),
                ("mid", Some("middle".to_string())),
                ("zeta", Some("last".to_string())),
            ],
        ),
        (
            "multi-byte characters count as their bytes",
            vec![replace("caf\u{e9}", "a\u{f1}ejo \u{1f600}", 0)],
            vec![("caf\u{e9}", Some("a\u{f1}ejo \u{1f600}".to_string()))],
        ),
        (
            "a 4 KiB value",
            vec![replace("blob", &long_value, 0)],
            vec![("blob", Some(long_value.clone()))],
        ),
        (
            "a remove in the same batch as the replaces",
            vec![
                replace("keep", "yes", 0),
                replace("drop", "no", 0),
                remove("drop", 0),
            ],
            vec![("keep", Some("yes".to_string()))],
        ),
    ];

    for (case, patches, expected) in cases {
        let emulator = Emulator::start();
        let written = emulator
            .sync(IDENTITY, "roundtrip", Some("device-1"), json!(patches))
            .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected UpdateRecords to succeed, got {} with body {}",
            written.status, written.raw
        );

        let expected_records: Vec<(String, Option<String>, i64)> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone(), 1))
            .collect();
        let listed = emulator.list_records(IDENTITY, "roundtrip", "").await;
        assert_eq!(
            listed.records(),
            expected_records,
            "case {case:?}: expected the written records back in key order, got {:?}",
            listed.records()
        );
        assert_eq!(
            listed.i64_at("Count"),
            Some(expected.len() as i64),
            "case {case:?}: Count is the size of the page, got {}",
            listed.raw
        );
        assert_eq!(
            listed.i64_at("DatasetSyncCount"),
            Some(1),
            "case {case:?}: one successful call advances the dataset sync count once, got {}",
            listed.raw
        );

        let expected_bytes: i64 = expected
            .iter()
            .map(|(key, value)| (key.len() + value.as_ref().map_or(0, String::len)) as i64)
            .sum();
        let described = emulator.describe_dataset(IDENTITY, "roundtrip").await;
        assert_eq!(
            (
                described.body["Dataset"]["NumRecords"].as_i64(),
                described.body["Dataset"]["DataStorage"].as_i64()
            ),
            (Some(expected.len() as i64), Some(expected_bytes)),
            "case {case:?}: expected NumRecords={} and DataStorage={expected_bytes}, got {}",
            expected.len(),
            described.raw
        );
    }
}

/// RULE 2: a patch presenting a sync count BEHIND the stored one is rejected with
/// `ResourceConflictException`, one presenting the stored count or a later one is
/// applied, and a rejected batch persists nothing at all.
#[tokio::test]
async fn a_batch_is_applied_only_when_no_patch_is_behind_the_stored_sync_count() {
    // (case, sync count presented for a record stored at sync count 1, expected status)
    let cases = [
        ("two behind", -1_i64, 409_u16),
        ("one behind", 0, 409),
        ("exactly the stored count", 1, 200),
        ("ahead of the stored count", 2, 200),
        ("far ahead of the stored count", 99, 200),
    ];

    for (case, presented, expected_status) in cases {
        let emulator = Emulator::start();
        let first = emulator
            .sync(
                IDENTITY,
                "concurrency",
                Some("device-1"),
                json!([replace("highScore", "42", 0)]),
            )
            .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the setup write must succeed, got {}",
            first.raw
        );

        // The batch carries a second, brand-new record too: if the batch is refused,
        // that record must not exist either.
        let patched = emulator
            .sync(
                IDENTITY,
                "concurrency",
                Some("device-2"),
                json!([
                    replace("highScore", "7", presented),
                    replace("fresh", "new", 0)
                ]),
            )
            .await;
        assert_eq!(
            patched.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with body {}",
            patched.status, patched.raw
        );

        let listed = emulator.list_records(IDENTITY, "concurrency", "").await;
        if expected_status == 409 {
            assert!(
                patched.raw.contains("ResourceConflictException"),
                "case {case:?}: expected ResourceConflictException, got {}",
                patched.raw
            );
            assert_eq!(
                listed.records(),
                vec![("highScore".to_string(), Some("42".to_string()), 1)],
                "case {case:?}: a refused batch persists nothing, got {:?}",
                listed.records()
            );
            assert_eq!(
                listed.i64_at("DatasetSyncCount"),
                Some(1),
                "case {case:?}: a refused batch leaves the dataset sync count alone, got {}",
                listed.raw
            );
        } else {
            assert_eq!(
                listed.records(),
                vec![
                    ("fresh".to_string(), Some("new".to_string()), 2),
                    ("highScore".to_string(), Some("7".to_string()), 2),
                ],
                "case {case:?}: an applied batch stamps every record with the new sync count, got {:?}",
                listed.records()
            );
            assert_eq!(
                listed.i64_at("DatasetSyncCount"),
                Some(2),
                "case {case:?}: a second successful call advances the sync count to 2, got {}",
                listed.raw
            );
        }
    }
}

/// RULE 4 for records: `MaxResults` bounds the page, `NextToken` resumes exactly where
/// it stopped, and walking the pages yields every record once, in key order.
#[tokio::test]
async fn list_records_pages_at_max_results_and_resumes_from_the_token() {
    let emulator = Emulator::start();
    let first_batch: Vec<Value> = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|key| replace(key, "v", 0))
        .collect();
    let setup = emulator
        .sync(IDENTITY, "paged", Some("device-1"), json!(first_batch))
        .await;
    assert_eq!(setup.status, 200, "the first batch must land: {}", setup.raw);
    let second_batch: Vec<Value> = ["f", "g"].iter().map(|key| replace(key, "v", 0)).collect();
    let setup = emulator
        .sync(IDENTITY, "paged", Some("device-1"), json!(second_batch))
        .await;
    assert_eq!(
        setup.status, 200,
        "the second batch must land: {}",
        setup.raw
    );

    let all_keys: Vec<String> = ["a", "b", "c", "d", "e", "f", "g"]
        .iter()
        .map(|key| (*key).to_string())
        .collect();

    for max_results in [1_usize, 2, 3, 7, 50] {
        let case = format!("MaxResults={max_results}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={max_results}&nextToken={token}"),
                None => format!("?maxResults={max_results}"),
            };
            let page = emulator.list_records(IDENTITY, "paged", &query).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected a page, got {} with body {}",
                page.status, page.raw
            );
            let keys = page.record_keys();
            assert!(
                keys.len() <= max_results,
                "case {case:?}: a page must be bounded by MaxResults, got {keys:?}"
            );
            assert_eq!(
                page.i64_at("Count"),
                Some(keys.len() as i64),
                "case {case:?}: Count must be the size of this page, got {}",
                page.raw
            );
            seen.extend(keys);
            token = page.next_token();
            pages += 1;
            assert!(
                pages <= 10,
                "case {case:?}: paging did not terminate after {pages} pages, seen {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all_keys,
            "case {case:?}: walking the pages must yield every record once, in key order"
        );
    }

    // `LastSyncCount` reports only what changed after it.
    let filters = [
        ("LastSyncCount absent", String::new(), all_keys.clone()),
        ("LastSyncCount=0", "?lastSyncCount=0".to_string(), all_keys),
        (
            "LastSyncCount=1",
            "?lastSyncCount=1".to_string(),
            vec!["f".to_string(), "g".to_string()],
        ),
        ("LastSyncCount=2", "?lastSyncCount=2".to_string(), Vec::new()),
    ];
    for (case, query, expected) in filters {
        let page = emulator.list_records(IDENTITY, "paged", &query).await;
        assert_eq!(
            page.record_keys(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            page.record_keys()
        );
        assert_eq!(
            page.i64_at("Count"),
            Some(expected.len() as i64),
            "case {case:?}: Count must match the records returned, got {}",
            page.raw
        );
    }

    // A page of fewer than one record is not a page.
    let refused = emulator
        .list_records(IDENTITY, "paged", "?maxResults=0")
        .await;
    assert_eq!(
        refused.status, 400,
        "MaxResults=0: expected HTTP 400, got {} with body {}",
        refused.status, refused.raw
    );
}

/// RULE 4 for datasets: the same paging rule over `ListDatasets`, whose `Count` is the
/// size of the page.
#[tokio::test]
async fn list_datasets_pages_over_the_datasets_of_one_identity() {
    let emulator = Emulator::start();
    for dataset in ["alpha", "beta", "gamma"] {
        let written = emulator
            .sync(IDENTITY, dataset, Some("device-1"), json!([replace("k", "v", 0)]))
            .await;
        assert_eq!(
            written.status, 200,
            "creating '{dataset}' must succeed, got {}",
            written.raw
        );
    }
    let all_names: Vec<String> = ["alpha", "beta", "gamma"]
        .iter()
        .map(|name| (*name).to_string())
        .collect();

    for max_results in [1_usize, 2, 3, 20] {
        let case = format!("MaxResults={max_results}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={max_results}&nextToken={token}"),
                None => format!("?maxResults={max_results}"),
            };
            let page = emulator.list_datasets(IDENTITY, &query).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected a page, got {} with body {}",
                page.status, page.raw
            );
            let names = page.dataset_names();
            assert!(
                names.len() <= max_results,
                "case {case:?}: a page must be bounded by MaxResults, got {names:?}"
            );
            assert_eq!(
                page.i64_at("Count"),
                Some(names.len() as i64),
                "case {case:?}: Count must be the size of this page, got {}",
                page.raw
            );
            seen.extend(names);
            token = page.next_token();
            pages += 1;
            assert!(
                pages <= 6,
                "case {case:?}: paging did not terminate after {pages} pages, seen {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all_names,
            "case {case:?}: walking the pages must yield every dataset once, in name order"
        );
    }

    let refused = emulator.list_datasets(IDENTITY, "?maxResults=-1").await;
    assert_eq!(
        refused.status, 400,
        "MaxResults=-1: expected HTTP 400, got {} with body {}",
        refused.status, refused.raw
    );
}

/// `UpdateRecords`'s `SyncSessionToken` is "The SyncSessionToken returned by a previous
/// call to ListRecords for this dataset and identity": anything else is refused, and the
/// refused call writes nothing.
#[tokio::test]
async fn update_records_spends_only_a_session_list_records_minted_for_that_dataset() {
    let emulator = Emulator::start();
    let session = emulator.list_records(IDENTITY, "owned", "").await;
    let token_for_owned = session.sync_session_token();
    let other_identity_session = emulator.list_records(OTHER_IDENTITY, "owned", "").await;
    let token_for_other_identity = other_identity_session.sync_session_token();

    // (case, token presented for dataset "target" under IDENTITY, expected status)
    let cases = [
        ("a token minted for another dataset", token_for_owned, 400_u16),
        (
            "a token minted for another identity",
            token_for_other_identity,
            400,
        ),
        ("a token this service never minted", "made-up".to_string(), 400),
        ("an empty-looking token", "0".repeat(32), 400),
    ];

    for (case, token, expected_status) in cases {
        let refused = emulator
            .update_records(
                IDENTITY,
                "target",
                &token,
                Some("device-1"),
                json!([replace("k", "v", 0)]),
            )
            .await;
        assert_eq!(
            refused.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with body {}",
            refused.status, refused.raw
        );
        assert!(
            refused.raw.contains("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {}",
            refused.raw
        );
        let described = emulator.describe_dataset(IDENTITY, "target").await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: a refused call must not create the dataset, got {} with body {}",
            described.status, described.raw
        );
    }

    // The token that belongs to the dataset works.
    let accepted = emulator
        .sync(IDENTITY, "target", Some("device-1"), json!([replace("k", "v", 0)]))
        .await;
    assert_eq!(
        accepted.status, 200,
        "a session minted for this dataset must be accepted, got {}",
        accepted.raw
    );
}

/// A deleted dataset is reported as deleted to a device whose copy predates the
/// deletion, and a dataset that never existed is not reported as deleted. Writing the
/// name again makes it a live dataset once more.
#[tokio::test]
async fn list_records_reports_a_dataset_deleted_after_the_requested_sync_count() {
    let emulator = Emulator::start();

    // Never written: nothing to delete locally.
    let never = emulator.list_records(IDENTITY, "neverwritten", "").await;
    assert_eq!(
        never.body["DatasetDeletedAfterRequestedSyncCount"].as_bool(),
        Some(false),
        "a dataset that never existed was never deleted, got {}",
        never.raw
    );

    let written = emulator
        .sync(
            IDENTITY,
            "doomed",
            Some("device-1"),
            json!([replace("k", "v", 0)]),
        )
        .await;
    assert_eq!(written.status, 200, "setup write: {}", written.raw);
    let deleted = emulator.delete_dataset(IDENTITY, "doomed").await;
    assert_eq!(deleted.status, 200, "DeleteDataset: {}", deleted.raw);

    for (case, query) in [
        ("no LastSyncCount", ""),
        ("LastSyncCount=0", "?lastSyncCount=0"),
        ("LastSyncCount=1", "?lastSyncCount=1"),
    ] {
        let after = emulator.list_records(IDENTITY, "doomed", query).await;
        assert_eq!(
            (
                after.body["DatasetExists"].as_bool(),
                after.body["DatasetDeletedAfterRequestedSyncCount"].as_bool()
            ),
            (Some(false), Some(true)),
            "case {case:?}: a copy predating the deletion must be told to drop it, got {}",
            after.raw
        );
    }

    // A later sync count than the deleted dataset ever had has nothing to drop.
    let ahead = emulator
        .list_records(IDENTITY, "doomed", "?lastSyncCount=2")
        .await;
    assert_eq!(
        ahead.body["DatasetDeletedAfterRequestedSyncCount"].as_bool(),
        Some(false),
        "a caller past the deleted dataset's sync count is not stale, got {}",
        ahead.raw
    );

    // Writing the name again makes it a live dataset, counted from scratch.
    let recreated = emulator
        .sync(
            IDENTITY,
            "doomed",
            Some("device-2"),
            json!([replace("k2", "v2", 0)]),
        )
        .await;
    assert_eq!(recreated.status, 200, "re-create: {}", recreated.raw);
    let live = emulator.list_records(IDENTITY, "doomed", "").await;
    assert_eq!(
        (
            live.body["DatasetExists"].as_bool(),
            live.body["DatasetDeletedAfterRequestedSyncCount"].as_bool(),
            live.i64_at("DatasetSyncCount")
        ),
        (Some(true), Some(false), Some(1)),
        "a re-created dataset is live again and counts from scratch, got {}",
        live.raw
    );
    assert_eq!(
        live.records(),
        vec![("k2".to_string(), Some("v2".to_string()), 1)],
        "a re-created dataset holds only what was written to it, got {:?}",
        live.records()
    );
}

/// One call may carry at most 1024 patches — a bound on the per-request work this
/// emulator does under its store lock, refused with the modelled `LimitExceededException`
/// and persisting nothing.
#[tokio::test]
async fn a_batch_carries_at_most_the_patch_limit() {
    // (case, patch count, expected status)
    let cases = [
        ("one under the limit", 1023_usize, 200_u16),
        ("exactly the limit", 1024, 200),
        ("one over the limit", 1025, 400),
    ];

    for (case, patch_count, expected_status) in cases {
        let emulator = Emulator::start();
        let patches: Vec<Value> = (0..patch_count)
            .map(|index| replace(&format!("k{index:05}"), "v", 0))
            .collect();
        let reply = emulator
            .sync(IDENTITY, "batched", Some("device-1"), json!(patches))
            .await;
        assert_eq!(
            reply.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with body {}",
            reply.status,
            reply.raw.chars().take(200).collect::<String>()
        );
        let described = emulator.describe_dataset(IDENTITY, "batched").await;
        if expected_status == 200 {
            assert_eq!(
                described.body["Dataset"]["NumRecords"].as_i64(),
                Some(patch_count as i64),
                "case {case:?}: every patch of an accepted batch must land, got {}",
                described.raw
            );
        } else {
            assert!(
                reply.raw.contains("LimitExceededException"),
                "case {case:?}: expected LimitExceededException, got {}",
                reply.raw.chars().take(200).collect::<String>()
            );
            assert_eq!(
                described.status, 404,
                "case {case:?}: a refused batch must not create the dataset, got {} with body {}",
                described.status, described.raw
            );
        }
    }
}

/// The bounds this emulator puts on its own growth, each refused with the modelled
/// `LimitExceededException` before anything is written: records per dataset, datasets
/// per identity, and the server-chosen page size.
#[tokio::test]
async fn the_store_bounds_are_enforced_before_a_write_lands() {
    // Records per dataset: 4096 is the ceiling, and the batch that would pass it is
    // refused whole, leaving the dataset exactly at the ceiling.
    let emulator = Emulator::start();
    for round in 0..4 {
        let patches: Vec<Value> = (0..1024)
            .map(|index| replace(&format!("k{:05}", round * 1024 + index), "v", 0))
            .collect();
        let written = emulator
            .sync(IDENTITY, "capped", Some("device-1"), json!(patches))
            .await;
        assert_eq!(
            written.status, 200,
            "round {round}: filling a dataset to its ceiling must succeed, got {}",
            written.raw.chars().take(200).collect::<String>()
        );
    }
    let over = emulator
        .sync(
            IDENTITY,
            "capped",
            Some("device-1"),
            json!([replace("one-too-many", "v", 0)]),
        )
        .await;
    assert_eq!(
        over.status, 400,
        "a record past the dataset ceiling: expected HTTP 400, got {} with body {}",
        over.status,
        over.raw.chars().take(200).collect::<String>()
    );
    assert!(
        over.raw.contains("LimitExceededException"),
        "a record past the dataset ceiling: expected LimitExceededException, got {}",
        over.raw.chars().take(200).collect::<String>()
    );
    let described = emulator.describe_dataset(IDENTITY, "capped").await;
    assert_eq!(
        described.body["Dataset"]["NumRecords"].as_i64(),
        Some(4096),
        "a refused batch must leave the dataset at its ceiling, got {}",
        described.raw
    );

    // The server's page size bounds a listing of that dataset, whatever the caller asks.
    for (case, query) in [
        ("MaxResults absent", ""),
        ("MaxResults above the server's page", "?maxResults=100000"),
    ] {
        let page = emulator.list_records(IDENTITY, "capped", query).await;
        assert_eq!(
            (page.i64_at("Count"), page.next_token().is_some()),
            (Some(1024), true),
            "case {case:?}: expected a 1024-record page and a NextToken, got Count={:?}",
            page.i64_at("Count")
        );
    }

    // Datasets per identity: 64 is the ceiling, and the 65th is refused.
    let emulator = Emulator::start();
    for index in 0..64 {
        let dataset = format!("ds{index:03}");
        let written = emulator
            .sync(IDENTITY, &dataset, Some("device-1"), json!([replace("k", "v", 0)]))
            .await;
        assert_eq!(
            written.status, 200,
            "dataset {dataset}: creating up to the per-identity ceiling must succeed, got {}",
            written.raw
        );
    }
    let over = emulator
        .sync(IDENTITY, "ds064", Some("device-1"), json!([replace("k", "v", 0)]))
        .await;
    assert_eq!(
        over.status, 400,
        "a dataset past the per-identity ceiling: expected HTTP 400, got {} with body {}",
        over.status, over.raw
    );
    assert!(
        over.raw.contains("LimitExceededException"),
        "a dataset past the per-identity ceiling: expected LimitExceededException, got {}",
        over.raw
    );
    let absent = emulator.describe_dataset(IDENTITY, "ds064").await;
    assert_eq!(
        absent.status, 404,
        "the refused dataset must not exist, got {} with body {}",
        absent.status, absent.raw
    );
    // Another identity is unaffected by the first one's ceiling.
    let other = emulator
        .sync(
            OTHER_IDENTITY,
            "ds064",
            Some("device-1"),
            json!([replace("k", "v", 0)]),
        )
        .await;
    assert_eq!(
        other.status, 200,
        "the ceiling is per identity, got {} with body {}",
        other.status, other.raw
    );
}

/// "a dataset can hold up to 1MB of key-value pairs" (the `Dataset` shape's doc): a
/// batch that would push it past that is refused whole, and one that lands exactly on
/// the limit is accepted.
#[tokio::test]
async fn a_dataset_holds_at_most_one_mebibyte_of_key_value_pairs() {
    const LIMIT: usize = 1_048_576;

    let emulator = Emulator::start();
    // Exactly on the limit: a one-byte key plus a value filling the rest.
    let at_limit = emulator
        .sync(
            IDENTITY,
            "sized",
            Some("device-1"),
            json!([replace("k", &"a".repeat(LIMIT - 1), 0)]),
        )
        .await;
    assert_eq!(
        at_limit.status, 200,
        "a dataset of exactly {LIMIT} bytes is within the limit, got {} with body {}",
        at_limit.status,
        at_limit.raw.chars().take(200).collect::<String>()
    );
    let described = emulator.describe_dataset(IDENTITY, "sized").await;
    assert_eq!(
        described.body["Dataset"]["DataStorage"].as_i64(),
        Some(LIMIT as i64),
        "the stored dataset must measure {LIMIT} bytes, got {}",
        described.raw
    );

    // One byte past it, in a batch that also carries a fresh record: nothing lands.
    let over = emulator
        .sync(
            IDENTITY,
            "oversized",
            Some("device-1"),
            json!([
                replace("key", &"a".repeat(LIMIT - 3), 0),
                replace("one-too-many", "x", 0)
            ]),
        )
        .await;
    assert_eq!(
        over.status, 400,
        "a batch past the dataset limit: expected HTTP 400, got {} with body {}",
        over.status,
        over.raw.chars().take(200).collect::<String>()
    );
    assert!(
        over.raw.contains("LimitExceededException"),
        "a batch past the dataset limit: expected LimitExceededException, got {}",
        over.raw.chars().take(200).collect::<String>()
    );
    let absent = emulator.describe_dataset(IDENTITY, "oversized").await;
    assert_eq!(
        absent.status, 404,
        "a refused batch must not create the dataset, got {} with body {}",
        absent.status, absent.raw
    );
}
