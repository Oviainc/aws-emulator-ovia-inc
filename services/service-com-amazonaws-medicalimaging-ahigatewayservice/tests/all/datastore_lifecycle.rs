// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the data store lifecycle and the DICOM import job loop.
//!
//! Every case drives real HTTP through the production front door
//! (`MultiServiceRouter::dispatch`), so what is exercised is the wire behaviour a
//! `medical-imaging` SDK client sees: restJson1 routes, bodies, statuses and error
//! codes. Nothing reaches into the state struct.
//!
//! The suites are TABLES: each row is one input along the axis the rule generalises
//! over (which optional members are present, name length, tag count, enum value, page
//! size, token form, id shape), and every assertion names its case and the actual
//! value.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    DATASTORE_ACTIVATION_DELAY, DATASTORE_DELETION_DELAY, IMPORT_JOB_COMPLETION_DELAY,
    MAX_DATASTORES, MAX_IMPORT_JOBS_PER_DATASTORE,
};

/// The region every suite signs for unless it names another: the one
/// [`Emulator::call`] uses.
pub(crate) const DEFAULT_TEST_REGION: &str = "us-east-1";

/// A SigV4 header naming this service's signing name, which is how the router's
/// `can_handle` resolves the request to this service, and whose credential scope is
/// where the request's REGION comes from. Signatures are not verified (see
/// `set_request_authentication(false)`).
pub(crate) fn sigv4_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20260101/{region}/medical-imaging/aws4_request, SignedHeaders=host, Signature=abc"
    )
}

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// `body`, decoded from the gzip format a response declared it in — what a client does
/// with a payload whose `Content-Encoding` names a compression.
///
/// Decoded through the crate's own inverse of the compression it applied
/// (`crate::ops::get_image_set_metadata::decoded_metadata_blob`), so this suite reads the
/// blob the way the handler wrote it rather than stating the format a second time. A body
/// that does not decode is handed back as it stands, so a case about a malformed answer
/// still sees what arrived rather than an empty one.
fn decompress_gzip(body: &[u8]) -> Vec<u8> {
    service_com_amazonaws_medicalimaging_ahigatewayservice::ops::get_image_set_metadata::decoded_metadata_blob(body)
        .unwrap_or_else(|| body.to_vec())
}

/// One HTTP answer, reduced to what these cases assert on.
///
/// `pub(crate)` because the sibling suites in `tests/all/` drive the same front door
/// and assert on the same three things; one harness, not one per suite.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
    /// The RAW response body, for an operation whose `@httpPayload` is a blob rather
    /// than JSON (`GetImageFrame`'s `imageFrameBlob`), where [`Answer::body`] is null.
    ///
    /// DECODED first, as a client decodes a body: a payload the answer declares a
    /// `Content-Encoding` for ([`Answer::content_encoding`]) is decompressed here, so
    /// every case reads the CONTENT the answer carried rather than its wire encoding.
    pub(crate) body_bytes: Vec<u8>,
    /// The `Content-Type` the answer carried, empty when it carried none.
    pub(crate) content_type: String,
    /// The `Content-Encoding` the answer carried, empty when it carried none — the
    /// compression a blob payload is in (`GetImageSetMetadata`'s
    /// `imageSetMetadataBlob`).
    pub(crate) content_encoding: String,
    /// The wire error code (`x-amzn-errortype`, else the body's `__type`), with any
    /// `namespace#` prefix stripped. Empty on a success.
    pub(crate) error_code: String,
}

impl Answer {
    pub(crate) fn field(&self, path: &[&str]) -> &Value {
        let mut cursor = &self.body;
        for key in path {
            cursor = &cursor[*key];
        }
        cursor
    }

    pub(crate) fn string(&self, path: &[&str]) -> String {
        self.field(path).as_str().unwrap_or_default().to_string()
    }
}

/// The emulator under test: one shared engine behind the production router.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The SAME engine the router handed every operation. Held so a suite can seed a
    /// resource this service's implemented operations cannot yet mint themselves (an
    /// image set, which only a DICOM import creates) and then drive the real HTTP
    /// surface over it. No case asserts THROUGH this handle — assertions read the wire.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_medicalimaging_ahigatewayservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// The shared engine, for seeding only — see the field's note.
    pub(crate) fn engine(&self) -> &engine::InMemoryEngine {
        &self.engine
    }

    /// One call signed for [`DEFAULT_TEST_REGION`].
    pub(crate) async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        self.call_in(DEFAULT_TEST_REGION, method, uri, body).await
    }

    /// One call signed for `region`: the credential scope and the `host` header both
    /// name it, exactly as an SDK client configured for that region sends.
    pub(crate) async fn call_in(
        &self,
        region: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> Answer {
        let payload = body.map(|value| value.to_string()).unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("medical-imaging.{region}.amazonaws.com"))
            .header("authorization", sigv4_for(region))
            .header("content-type", "application/json")
            .body(Bytes::from(payload))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let content_encoding = response
            .headers()
            .get("content-encoding")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        // What a client has once it has decoded the answer: a body the response declares
        // a compression for is decompressed before anything reads it.
        let payload = if content_encoding == "gzip" {
            decompress_gzip(response.body())
        } else {
            response.body().to_vec()
        };
        let body: Value = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        let raw_code = if header_code.is_empty() {
            body["__type"].as_str().unwrap_or_default().to_string()
        } else {
            header_code
        };
        let error_code = raw_code
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default()
            .to_string();
        Answer {
            status,
            body,
            body_bytes: payload,
            content_type,
            content_encoding,
            error_code,
        }
    }

    pub(crate) async fn create_datastore(&self, body: Value) -> Answer {
        self.call("POST", "/datastore", Some(body)).await
    }

    pub(crate) async fn get_datastore(&self, datastore_id: &str) -> Answer {
        self.call("GET", &format!("/datastore/{datastore_id}"), None)
            .await
    }

    pub(crate) async fn delete_datastore(&self, datastore_id: &str) -> Answer {
        self.call("DELETE", &format!("/datastore/{datastore_id}"), None)
            .await
    }

    pub(crate) async fn list_datastores(&self, query: &str) -> Answer {
        let uri = if query.is_empty() {
            "/datastore".to_string()
        } else {
            format!("/datastore?{query}")
        };
        self.call("GET", &uri, None).await
    }

    pub(crate) async fn start_import_job(&self, datastore_id: &str, body: Value) -> Answer {
        self.call(
            "POST",
            &format!("/startDICOMImportJob/datastore/{datastore_id}"),
            Some(body),
        )
        .await
    }

    pub(crate) async fn get_import_job(&self, datastore_id: &str, job_id: &str) -> Answer {
        self.call(
            "GET",
            &format!("/getDICOMImportJob/datastore/{datastore_id}/job/{job_id}"),
            None,
        )
        .await
    }

    /// Create a data store and wait out the activation the clock owes it, so a case
    /// that is not about the transition starts from an `ACTIVE` store. Signed for
    /// [`DEFAULT_TEST_REGION`]; [`Emulator::active_datastore_in`] chooses the region.
    pub(crate) async fn active_datastore(&self, datastore_name: &str, client_token: &str) -> String {
        self.active_datastore_in(DEFAULT_TEST_REGION, datastore_name, client_token)
            .await
    }

    /// [`Emulator::active_datastore`], in the region the create and the read are both
    /// addressed to — a data store exists in ONE region, so a case about a store
    /// elsewhere starts here.
    pub(crate) async fn active_datastore_in(
        &self,
        region: &str,
        datastore_name: &str,
        client_token: &str,
    ) -> String {
        let created = self
            .call_in(
                region,
                "POST",
                "/datastore",
                Some(json!({
                    "datastoreName": datastore_name,
                    "clientToken": client_token,
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "create {datastore_name} in {region}: expected 200, got {created:?}"
        );
        let datastore_id = created.string(&["datastoreId"]);
        tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_ACTIVATION_DELAY).await;
        let read = self
            .call_in(region, "GET", &format!("/datastore/{datastore_id}"), None)
            .await;
        assert_eq!(
            read.string(&["datastoreProperties", "datastoreStatus"]),
            "ACTIVE",
            "create {datastore_name} in {region}: expected ACTIVE after the activation span, \
             got {read:?}"
        );
        datastore_id
    }
}

/// How many times the configured span a suite waits before asserting the state AFTER a
/// clock-owed transition. Sleeping exactly one span would make the assertion race a
/// scheduling hiccup; a multiple cannot.
pub(crate) const SETTLE_MULTIPLE: u32 = 4;

/// A well-formed `[0-9a-z]{32}` resource id that no call ever mints (the ids `mint_id`
/// produces are SplitMix64 output, never all zeros), for a case that needs an id of a
/// resource this emulator does not hold.
///
/// `pub(crate)` beside the harness, so a suite needing one borrows this rather than
/// spelling it again.
pub(crate) fn unheld_id() -> String {
    "0".repeat(32)
}

/// `DatastoreId` is `^[0-9a-z]{32}$`; `JobId` is `^[0-9a-z]+$`, 1..32
/// (`smithy_query(query="operation",operation="CreateDatastore,StartDICOMImportJob")`).
fn is_lowercase_alnum(value: &str, exact_len: Option<usize>) -> bool {
    let len_ok = match exact_len {
        Some(len) => value.len() == len,
        None => (1..=32).contains(&value.len()),
    };
    len_ok
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
}

// ---------------------------------------------------------------------------
// CreateDatastore + GetDatastore
// ---------------------------------------------------------------------------

/// One row of the create/read table.
#[derive(Debug)]
struct CreateCase {
    case: &'static str,
    datastore_name: Option<String>,
    tags: &'static [(&'static str, &'static str)],
    kms_key_arn: Option<&'static str>,
    lambda_authorizer_arn: Option<&'static str>,
    lossless_storage_format: Option<&'static str>,
}

/// A 256-character `datastoreName`, the published upper bound of
/// `DatastoreName` (len 1..256).
fn max_length_name() -> String {
    "n".repeat(256)
}

/// RULE: a created data store is persisted whole, and a later read reports every
/// member back exactly as supplied, beside the service-generated id, ARN and
/// timestamps.
///
/// Rows span the axes the rule generalises over: which optional members are present,
/// `datastoreName` at both published length bounds, tag-map size, and each
/// `LosslessStorageFormat` the model declares.
#[tokio::test]
async fn a_created_datastore_reads_back_with_every_member_it_was_given() {
    // `LosslessStorageFormat` values come from
    // `smithy_query(query="shape",shape="LosslessStorageFormat")`: HTJ2K,
    // JPEG_2000_LOSSLESS. Both appear below; the issue cited neither.
    let max_name = max_length_name();
    let cases: Vec<CreateCase> = vec![
        CreateCase {
            case: "name only",
            datastore_name: Some("store.one".to_string()),
            tags: &[],
            kms_key_arn: None,
            lambda_authorizer_arn: None,
            lossless_storage_format: None,
        },
        CreateCase {
            case: "one-character name, the lower length bound",
            datastore_name: Some("a".to_string()),
            tags: &[],
            kms_key_arn: None,
            lambda_authorizer_arn: None,
            lossless_storage_format: None,
        },
        CreateCase {
            case: "256-character name, the upper length bound",
            datastore_name: Some(max_name),
            tags: &[],
            kms_key_arn: None,
            lambda_authorizer_arn: None,
            lossless_storage_format: None,
        },
        CreateCase {
            case: "punctuated name plus one tag",
            datastore_name: Some("nested/name#1-2_x.y".to_string()),
            tags: &[("team", "radiology")],
            kms_key_arn: None,
            lambda_authorizer_arn: None,
            lossless_storage_format: None,
        },
        CreateCase {
            case: "three tags and a KMS key ARN",
            datastore_name: Some("kms.store".to_string()),
            tags: &[("a", "1"), ("b", "2"), ("c", "3")],
            kms_key_arn: Some(
                "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012",
            ),
            lambda_authorizer_arn: None,
            lossless_storage_format: None,
        },
        CreateCase {
            case: "KMS alias ARN, lambda authorizer and HTJ2K",
            datastore_name: Some("full.store".to_string()),
            tags: &[("env", "test")],
            kms_key_arn: Some("arn:aws:kms:us-east-1:000000000000:alias/imaging_key"),
            lambda_authorizer_arn: Some(
                "arn:aws:lambda:us-east-1:000000000000:function:imaging-authorizer",
            ),
            lossless_storage_format: Some("HTJ2K"),
        },
        CreateCase {
            case: "JPEG_2000_LOSSLESS with a lambda authorizer version",
            datastore_name: Some("jpeg2000.store".to_string()),
            tags: &[],
            kms_key_arn: None,
            lambda_authorizer_arn: Some(
                "arn:aws:lambda:eu-west-1:000000000000:function:imaging-authorizer:2",
            ),
            lossless_storage_format: Some("JPEG_2000_LOSSLESS"),
        },
        CreateCase {
            case: "no datastoreName at all",
            datastore_name: None,
            tags: &[],
            kms_key_arn: None,
            lambda_authorizer_arn: None,
            lossless_storage_format: None,
        },
    ];

    let emulator = Emulator::new();
    let mut minted = BTreeSet::new();

    for (index, case) in cases.iter().enumerate() {
        let mut request = json!({ "clientToken": format!("create-token-{index}") });
        if let Some(name) = &case.datastore_name {
            request["datastoreName"] = json!(name);
        }
        if !case.tags.is_empty() {
            let tags: serde_json::Map<String, Value> = case
                .tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect();
            request["tags"] = Value::Object(tags);
        }
        if let Some(arn) = case.kms_key_arn {
            request["kmsKeyArn"] = json!(arn);
        }
        if let Some(arn) = case.lambda_authorizer_arn {
            request["lambdaAuthorizerArn"] = json!(arn);
        }
        if let Some(format) = case.lossless_storage_format {
            request["losslessStorageFormat"] = json!(format);
        }

        let started = Instant::now();
        let created = emulator.create_datastore(request).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200 from CreateDatastore, got {created:?}",
            case.case
        );
        let datastore_id = created.string(&["datastoreId"]);
        assert!(
            is_lowercase_alnum(&datastore_id, Some(32)),
            "case {:?}: expected a 32-character lowercase-alphanumeric datastoreId, got {datastore_id:?}",
            case.case
        );
        assert!(
            minted.insert(datastore_id.clone()),
            "case {:?}: expected a freshly minted datastoreId, got the already-seen {datastore_id:?}",
            case.case
        );
        assert_eq!(
            created.string(&["datastoreStatus"]),
            "CREATING",
            "case {:?}: expected CreateDatastore to answer CREATING, got {created:?}",
            case.case
        );

        let read = emulator.get_datastore(&datastore_id).await;
        let read_elapsed = started.elapsed();
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from GetDatastore, got {read:?}",
            case.case
        );
        assert_eq!(
            read.string(&["datastoreProperties", "datastoreId"]),
            datastore_id,
            "case {:?}: expected the created datastoreId back, got {read:?}",
            case.case
        );
        // `DatastoreProperties.datastoreName` is @required while the create request
        // leaves it optional, so a nameless store reports its own id as its name.
        let expected_name = case.datastore_name.as_deref().unwrap_or(&datastore_id);
        assert_eq!(
            read.string(&["datastoreProperties", "datastoreName"]),
            expected_name,
            "case {:?}: expected datastoreName {expected_name:?}, got {read:?}",
            case.case
        );
        assert_eq!(
            read.string(&["datastoreProperties", "datastoreArn"]),
            format!("arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{datastore_id}"),
            "case {:?}: expected the canonical data store ARN, got {read:?}",
            case.case
        );
        assert!(
            read.field(&["datastoreProperties", "createdAt"]).is_number(),
            "case {:?}: expected a createdAt timestamp, got {read:?}",
            case.case
        );
        assert!(
            read.field(&["datastoreProperties", "updatedAt"]).is_number(),
            "case {:?}: expected an updatedAt timestamp, got {read:?}",
            case.case
        );
        for (member, supplied) in [
            ("kmsKeyArn", case.kms_key_arn),
            ("lambdaAuthorizerArn", case.lambda_authorizer_arn),
            ("losslessStorageFormat", case.lossless_storage_format),
        ] {
            let actual = read.field(&["datastoreProperties", member]);
            match supplied {
                Some(expected) => assert_eq!(
                    actual.as_str(),
                    Some(expected),
                    "case {:?}: expected {member} {expected:?}, got {actual:?}",
                    case.case
                ),
                None => assert!(
                    actual.is_null(),
                    "case {:?}: expected {member} to be absent, got {actual:?}",
                    case.case
                ),
            }
        }
        // The transient status is the correct answer until the activation the clock
        // owes falls due; after it, ACTIVE is. Asserted unconditionally as that
        // disjunction, so no row is ever silently skipped.
        let observed_status = read.string(&["datastoreProperties", "datastoreStatus"]);
        assert!(
            observed_status == "CREATING" || read_elapsed >= DATASTORE_ACTIVATION_DELAY,
            "case {:?}: expected CREATING {read_elapsed:?} after the create (the activation span is {DATASTORE_ACTIVATION_DELAY:?}), got {observed_status:?}",
            case.case
        );
    }
}

/// RULE: the clock, not the reader, moves a data store out of `CREATING`; once it is
/// `ACTIVE` the status and `updatedAt` it reports are stable across reads.
#[tokio::test]
async fn a_creating_datastore_becomes_active_when_the_clock_owes_it() {
    let emulator = Emulator::new();
    let created = emulator
        .create_datastore(json!({ "datastoreName": "transition.store", "clientToken": "transition-1" }))
        .await;
    let datastore_id = created.string(&["datastoreId"]);

    tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_ACTIVATION_DELAY).await;

    let first = emulator.get_datastore(&datastore_id).await;
    let second = emulator.get_datastore(&datastore_id).await;
    for (case, read) in [("first read", &first), ("second read", &second)] {
        assert_eq!(
            read.string(&["datastoreProperties", "datastoreStatus"]),
            "ACTIVE",
            "case {case:?}: expected ACTIVE after the activation span, got {read:?}"
        );
    }
    assert_eq!(
        first.field(&["datastoreProperties", "updatedAt"]),
        second.field(&["datastoreProperties", "updatedAt"]),
        "case \"stability\": expected updatedAt to be unchanged between reads, got {:?} then {:?}",
        first.field(&["datastoreProperties", "updatedAt"]),
        second.field(&["datastoreProperties", "updatedAt"])
    );
}

/// RULE: `clientToken` is an idempotency key — repeating a create with a token
/// already used returns the resource that token minted, and a token never used mints
/// a new one.
///
/// Rows span the token forms `ClientToken` admits (len 1..64, `^[A-Za-z0-9._-]+$`).
#[tokio::test]
async fn repeating_a_create_with_the_same_client_token_returns_the_same_datastore() {
    let tokens: Vec<String> = vec![
        "a".to_string(),
        "simple-token".to_string(),
        "dotted.token_with-every.allowed_char-09".to_string(),
        "t".repeat(64),
    ];

    let emulator = Emulator::new();
    let mut minted = BTreeSet::new();

    for token in &tokens {
        let first = emulator
            .create_datastore(json!({ "datastoreName": "idem.store", "clientToken": token }))
            .await;
        assert_eq!(
            first.status, 200,
            "case {token:?}: expected HTTP 200 on the first create, got {first:?}"
        );
        let id = first.string(&["datastoreId"]);
        assert!(
            minted.insert(id.clone()),
            "case {token:?}: expected a distinct datastoreId per unused token, got the repeated {id:?}"
        );

        // A differing body with the same token must still resolve to the first store:
        // the token, not the payload, is the idempotency key.
        let repeat = emulator
            .create_datastore(json!({ "datastoreName": "a-different-name", "clientToken": token }))
            .await;
        assert_eq!(
            repeat.status, 200,
            "case {token:?}: expected HTTP 200 on the repeat create, got {repeat:?}"
        );
        assert_eq!(
            repeat.string(&["datastoreId"]),
            id,
            "case {token:?}: expected the repeat create to return {id:?}, got {repeat:?}"
        );

        let read = emulator.get_datastore(&id).await;
        assert_eq!(
            read.string(&["datastoreProperties", "datastoreName"]),
            "idem.store",
            "case {token:?}: expected the repeat create not to overwrite the store, got {read:?}"
        );
    }

    let listed = emulator.list_datastores("maxResults=50").await;
    let summaries = listed.field(&["datastoreSummaries"]).as_array().cloned().unwrap_or_default();
    assert_eq!(
        summaries.len(),
        tokens.len(),
        "case \"no duplicates\": expected exactly one store per token, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// ListDatastores
// ---------------------------------------------------------------------------

/// RULE: paging walks the whole collection exactly once — each page holds at most
/// `maxResults` items, the token continues where the previous page stopped, and the
/// last page carries no token.
///
/// Rows span page sizes from the published lower bound to the published upper bound of
/// `maxResults` (range 1..50), plus sizes either side of the collection size.
#[tokio::test]
async fn paging_list_datastores_walks_every_store_exactly_once() {
    const STORES: usize = 7;
    let emulator = Emulator::new();
    let mut expected = BTreeSet::new();
    for index in 0..STORES {
        let created = emulator
            .create_datastore(json!({
                "datastoreName": format!("page.store-{index}"),
                "clientToken": format!("page-token-{index}"),
            }))
            .await;
        expected.insert(created.string(&["datastoreId"]));
    }

    // 1 and 50 are `maxResults`' published bounds; 6, 7 and 8 straddle the collection.
    for page_size in [1_usize, 2, 6, 7, 8, 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= STORES + 2,
                "case maxResults={page_size}: paging did not terminate after {pages} pages"
            );
            let query = match &token {
                None => format!("maxResults={page_size}"),
                Some(value) => format!("maxResults={page_size}&nextToken={value}"),
            };
            let page = emulator.list_datastores(&query).await;
            assert_eq!(
                page.status, 200,
                "case maxResults={page_size}: expected HTTP 200, got {page:?}"
            );
            let summaries = page
                .field(&["datastoreSummaries"])
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                summaries.len() <= page_size,
                "case maxResults={page_size}: expected at most {page_size} summaries, got {}",
                summaries.len()
            );
            for summary in &summaries {
                let id = summary["datastoreId"].as_str().unwrap_or_default().to_string();
                assert_eq!(
                    summary["datastoreArn"].as_str(),
                    Some(
                        format!("arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{id}")
                            .as_str()
                    ),
                    "case maxResults={page_size}: expected the canonical ARN on every summary, got {summary:?}"
                );
                assert!(
                    summary["createdAt"].is_number(),
                    "case maxResults={page_size}: expected createdAt on every summary, got {summary:?}"
                );
                seen.push(id);
            }
            match page.field(&["nextToken"]).as_str() {
                Some(next) => {
                    assert_eq!(
                        summaries.len(),
                        page_size,
                        "case maxResults={page_size}: a non-final page must be full, got {}",
                        summaries.len()
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case maxResults={page_size}: a store was listed twice: {seen:?}"
        );
        assert_eq!(
            unique, expected,
            "case maxResults={page_size}: expected every created store exactly once, got {unique:?}"
        );
    }
}

/// RULE: a continuation token names a position in the collection's order, not a
/// surviving record — so when the store the token was minted at is deleted between
/// pages, the walk resumes at the first store that sorts at or after that position:
/// never back into a page already served, never past a store never seen.
///
/// Rows span where in the collection the deleted store sits (the first of the next
/// page, and the one after it), which is what a key-ordered resume has to get right.
#[tokio::test]
async fn paging_resumes_in_order_when_the_token_s_store_was_deleted() {
    for deleted_offset in [0_usize, 1] {
        let emulator = Emulator::new();
        for index in 0..5 {
            emulator
                .create_datastore(json!({
                    "datastoreName": format!("stale.store-{index}"),
                    "clientToken": format!("stale-token-{index}"),
                }))
                .await;
        }
        // Wait the activation out before the delete below: a store whose creation is
        // still in flight refuses the delete with `ConflictException`, and this case is
        // about the paging token, not about that guard
        // (`tests/all/datastore_status_guard.rs`).
        tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_ACTIVATION_DELAY).await;

        let full: Vec<String> = emulator
            .list_datastores("maxResults=50")
            .await
            .field(&["datastoreSummaries"])
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| summary["datastoreId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            full.len(),
            5,
            "case deleted_offset={deleted_offset}: expected five stores listed, got {full:?}"
        );

        let first_page = emulator.list_datastores("maxResults=2").await;
        let served: Vec<String> = first_page
            .field(&["datastoreSummaries"])
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| summary["datastoreId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            served,
            full[..2].to_vec(),
            "case deleted_offset={deleted_offset}: expected the first page to be the first two in order, got {served:?}"
        );
        let token = first_page
            .field(&["nextToken"])
            .as_str()
            .expect("a second page exists")
            .to_string();

        // Retire a store at or just after the position the token names.
        let victim = full[2 + deleted_offset].clone();
        let deleted = emulator.delete_datastore(&victim).await;
        assert_eq!(
            deleted.status, 200,
            "case deleted_offset={deleted_offset}: expected HTTP 200 from DeleteDatastore, got {deleted:?}"
        );
        tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_DELETION_DELAY).await;

        // Walk the rest with the now-stale token: every store not served and not
        // deleted must appear exactly once, in order.
        let expected: Vec<String> = full[2..]
            .iter()
            .filter(|id| **id != victim)
            .cloned()
            .collect();
        let mut resumed: Vec<String> = Vec::new();
        let mut cursor = Some(token);
        while let Some(next_token) = cursor {
            let page = emulator
                .list_datastores(&format!("maxResults=2&nextToken={next_token}"))
                .await;
            assert_eq!(
                page.status, 200,
                "case deleted_offset={deleted_offset}: expected HTTP 200 on resume, got {page:?}"
            );
            resumed.extend(
                page.field(&["datastoreSummaries"])
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|summary| summary["datastoreId"].as_str().unwrap_or_default().to_string()),
            );
            cursor = page.field(&["nextToken"]).as_str().map(ToString::to_string);
        }
        assert_eq!(
            resumed, expected,
            "case deleted_offset={deleted_offset}: expected the resume to serve {expected:?} in order, got {resumed:?}"
        );
        for already_served in &full[..2] {
            assert!(
                !resumed.contains(already_served),
                "case deleted_offset={deleted_offset}: expected the resume not to repeat {already_served:?}, got {resumed:?}"
            );
        }
    }
}

/// RULE: `datastoreStatus` filters the listing to the stores in exactly that status,
/// and every status the enum declares is accepted.
///
/// The catalog is every value of `DatastoreStatus`
/// (`smithy_query(query="shape",shape="DatastoreStatus")`): CREATING, CREATE_FAILED,
/// ACTIVE, DELETING, DELETED.
#[tokio::test]
async fn listing_filters_by_every_modelled_datastore_status() {
    let emulator = Emulator::new();
    let mut active = BTreeSet::new();
    for index in 0..3 {
        active.insert(
            emulator
                .active_datastore(&format!("filter.store-{index}"), &format!("filter-{index}"))
                .await,
        );
    }
    let deleting = active.iter().next().cloned().expect("three stores exist");
    active.remove(&deleting);
    let deleted = emulator.delete_datastore(&deleting).await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete one\": expected HTTP 200, got {deleted:?}"
    );

    // One row per modelled status, with the set of ids that status must select. The
    // `DELETED` row is empty because no deletion has LANDED here — the one delete above
    // is still in flight, so its store is `DELETING` — not because that status selects
    // nothing: `crate::datastore_deleted_window` is the suite for the landed deletion.
    let expectations: Vec<(&str, BTreeSet<String>)> = vec![
        ("CREATING", BTreeSet::new()),
        ("CREATE_FAILED", BTreeSet::new()),
        ("ACTIVE", active.clone()),
        ("DELETING", BTreeSet::from([deleting.clone()])),
        ("DELETED", BTreeSet::new()),
    ];

    for (status, expected) in &expectations {
        let page = emulator
            .list_datastores(&format!("datastoreStatus={status}&maxResults=50"))
            .await;
        assert_eq!(
            page.status, 200,
            "case {status:?}: expected HTTP 200, got {page:?}"
        );
        let got: BTreeSet<String> = page
            .field(&["datastoreSummaries"])
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| summary["datastoreId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            &got, expected,
            "case {status:?}: expected {expected:?}, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// DeleteDatastore
// ---------------------------------------------------------------------------

/// RULE: a deleted data store reports `DELETING`, and once the deletion the clock owes
/// has landed AND the record it left behind has been reaped it is gone — reads answer
/// `ResourceNotFoundException` and it leaves the listing — while every other store is
/// untouched. (The `DELETED` phase in between is
/// `crate::datastore_deleted_window`'s rule; this suite waits past both spans.)
///
/// Rows span how many other stores share the emulator, so the rule is asserted on a
/// collection, not just on a single store.
#[tokio::test]
async fn a_deleted_datastore_disappears_and_leaves_its_neighbours_alone() {
    for neighbours in [0_usize, 1, 3] {
        let emulator = Emulator::new();
        let victim = emulator
            .active_datastore("delete.victim", "delete-victim")
            .await;
        let mut survivors = BTreeSet::new();
        for index in 0..neighbours {
            survivors.insert(
                emulator
                    .active_datastore(
                        &format!("delete.survivor-{index}"),
                        &format!("delete-survivor-{index}"),
                    )
                    .await,
            );
        }

        let started = Instant::now();
        let deleted = emulator.delete_datastore(&victim).await;
        assert_eq!(
            deleted.status, 200,
            "case neighbours={neighbours}: expected HTTP 200 from DeleteDatastore, got {deleted:?}"
        );
        assert_eq!(
            deleted.string(&["datastoreId"]),
            victim,
            "case neighbours={neighbours}: expected the deleted id back, got {deleted:?}"
        );
        assert_eq!(
            deleted.string(&["datastoreStatus"]),
            "DELETING",
            "case neighbours={neighbours}: expected DELETING, got {deleted:?}"
        );
        // Read back immediately: the store is still DELETING until the deletion the
        // clock owes falls due. Asserted unconditionally as that disjunction, so no
        // row is ever silently skipped.
        let during = emulator.get_datastore(&victim).await;
        let during_elapsed = started.elapsed();
        let during_status = during.string(&["datastoreProperties", "datastoreStatus"]);
        assert!(
            during_status == "DELETING" || during_elapsed >= DATASTORE_DELETION_DELAY,
            "case neighbours={neighbours}: expected DELETING {during_elapsed:?} after the delete (the deletion span is {DATASTORE_DELETION_DELAY:?}), got {during:?}"
        );

        tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_DELETION_DELAY).await;

        let gone = emulator.get_datastore(&victim).await;
        assert_eq!(
            gone.status, 404,
            "case neighbours={neighbours}: expected HTTP 404 once the deletion landed and its record was reaped, got {gone:?}"
        );
        assert_eq!(
            gone.error_code, "ResourceNotFoundException",
            "case neighbours={neighbours}: expected ResourceNotFoundException, got {gone:?}"
        );

        let listed = emulator.list_datastores("maxResults=50").await;
        let remaining: BTreeSet<String> = listed
            .field(&["datastoreSummaries"])
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| summary["datastoreId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            remaining, survivors,
            "case neighbours={neighbours}: expected only the survivors to remain, got {remaining:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// StartDICOMImportJob + GetDICOMImportJob
// ---------------------------------------------------------------------------

/// One row of the import-job table.
#[derive(Debug)]
struct ImportCase {
    case: &'static str,
    job_name: Option<&'static str>,
    input_s3_uri: &'static str,
    output_s3_uri: &'static str,
    data_access_role_arn: &'static str,
    input_owner_account_id: Option<&'static str>,
    import_configuration: bool,
}

/// RULE: a submitted import job is persisted whole and reported back member for
/// member, it starts `SUBMITTED`, and the clock — not the poller — carries it to
/// `COMPLETED` with an `endedAt` at or after its `submittedAt`.
///
/// Rows span the axes the rule generalises over: job name present or absent, S3 URI
/// form (bucket root, nested prefix, dotted bucket name), role ARN partition, the
/// optional source-account id, and the optional `importConfiguration` union.
#[tokio::test]
async fn an_import_job_reads_back_whole_and_runs_to_completion() {
    let cases: Vec<ImportCase> = vec![
        ImportCase {
            case: "minimal job, no name",
            job_name: None,
            input_s3_uri: "s3://imaging-in/",
            output_s3_uri: "s3://imaging-out/",
            data_access_role_arn: "arn:aws:iam::000000000000:role/ImportRole",
            input_owner_account_id: None,
            import_configuration: false,
        },
        ImportCase {
            case: "named job with nested prefixes",
            job_name: Some("nightly.import#1"),
            input_s3_uri: "s3://imaging-in/studies/2026/01/",
            output_s3_uri: "s3://imaging-out/results/2026/01/",
            data_access_role_arn: "arn:aws:iam::000000000000:role/service-role/ImportRole",
            input_owner_account_id: Some("000000000000"),
            import_configuration: false,
        },
        ImportCase {
            case: "dotted bucket names, gov partition role and an import configuration",
            job_name: Some("metadata-enhance"),
            input_s3_uri: "s3://imaging.in.bucket/dicom",
            output_s3_uri: "s3://imaging.out.bucket/manifest",
            data_access_role_arn: "arn:aws-us-gov:iam::000000000000:role/ImportRole",
            input_owner_account_id: Some("000000000001"),
            import_configuration: true,
        },
    ];

    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("import.store", "import-store").await;
    let mut minted = BTreeSet::new();

    for (index, case) in cases.iter().enumerate() {
        let mut request = json!({
            "clientToken": format!("import-token-{index}"),
            "dataAccessRoleArn": case.data_access_role_arn,
            "inputS3Uri": case.input_s3_uri,
            "outputS3Uri": case.output_s3_uri,
        });
        if let Some(name) = case.job_name {
            request["jobName"] = json!(name);
        }
        if let Some(account) = case.input_owner_account_id {
            request["inputOwnerAccountId"] = json!(account);
        }
        if case.import_configuration {
            request["importConfiguration"] = json!({
                "dicomJsonMetadataImportConfiguration": {
                    "dicomMetadataMappings": [{
                        "studyInstanceUID": "1.2.840.113619.2.1.1",
                        "metadataFilePath": "metadata/study.json"
                    }]
                }
            });
        }

        let started = emulator.start_import_job(&datastore_id, request).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: expected HTTP 200 from StartDICOMImportJob, got {started:?}",
            case.case
        );
        let job_id = started.string(&["jobId"]);
        assert!(
            is_lowercase_alnum(&job_id, None),
            "case {:?}: expected a 1..32 lowercase-alphanumeric jobId, got {job_id:?}",
            case.case
        );
        assert!(
            minted.insert(job_id.clone()),
            "case {:?}: expected a freshly minted jobId, got the already-seen {job_id:?}",
            case.case
        );
        assert_eq!(
            started.string(&["jobStatus"]),
            "SUBMITTED",
            "case {:?}: expected SUBMITTED, got {started:?}",
            case.case
        );
        assert_eq!(
            started.string(&["datastoreId"]),
            datastore_id,
            "case {:?}: expected the data store id back, got {started:?}",
            case.case
        );
        let submitted_at = started.field(&["submittedAt"]).as_f64();
        assert!(
            submitted_at.is_some(),
            "case {:?}: expected a submittedAt timestamp, got {started:?}",
            case.case
        );

        // A repeat with the same token must not submit a second job.
        let repeat = emulator
            .start_import_job(
                &datastore_id,
                json!({
                    "clientToken": format!("import-token-{index}"),
                    "dataAccessRoleArn": case.data_access_role_arn,
                    "inputS3Uri": "s3://some-other-bucket/",
                    "outputS3Uri": "s3://some-other-bucket/out/",
                }),
            )
            .await;
        assert_eq!(
            repeat.string(&["jobId"]),
            job_id,
            "case {:?}: expected the repeat submit to return {job_id:?}, got {repeat:?}",
            case.case
        );

        // Poll the job the way a client does, and assert the status only ever moves
        // forward through the modelled order.
        let order = ["SUBMITTED", "IN_PROGRESS", "COMPLETED"];
        let mut highest = 0_usize;
        let deadline = Instant::now() + IMPORT_JOB_COMPLETION_DELAY * 20;
        let mut final_read = emulator.get_import_job(&datastore_id, &job_id).await;
        loop {
            assert_eq!(
                final_read.status, 200,
                "case {:?}: expected HTTP 200 from GetDICOMImportJob, got {final_read:?}",
                case.case
            );
            let status = final_read.string(&["jobProperties", "jobStatus"]);
            let rank = order
                .iter()
                .position(|known| *known == status)
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected one of {order:?}, got {status:?}",
                        case.case
                    )
                });
            assert!(
                rank >= highest,
                "case {:?}: expected the job status not to move backwards from {:?}, got {status:?}",
                case.case,
                order[highest]
            );
            highest = rank;
            if status == "COMPLETED" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {:?}: the import job never reached COMPLETED: {final_read:?}",
                case.case
            );
            tokio::time::sleep(IMPORT_JOB_COMPLETION_DELAY / 4).await;
            final_read = emulator.get_import_job(&datastore_id, &job_id).await;
        }

        let properties = "jobProperties";
        assert_eq!(
            final_read.string(&[properties, "jobId"]),
            job_id,
            "case {:?}: expected the submitted jobId back, got {final_read:?}",
            case.case
        );
        // `DICOMImportJobProperties.jobName` is @required while the submit leaves it
        // optional, so a nameless job reports its own id as its name.
        let expected_job_name = case.job_name.unwrap_or(&job_id);
        assert_eq!(
            final_read.string(&[properties, "jobName"]),
            expected_job_name,
            "case {:?}: expected jobName {expected_job_name:?}, got {final_read:?}",
            case.case
        );
        for (member, expected) in [
            ("datastoreId", datastore_id.as_str()),
            ("dataAccessRoleArn", case.data_access_role_arn),
            ("inputS3Uri", case.input_s3_uri),
            ("outputS3Uri", case.output_s3_uri),
        ] {
            assert_eq!(
                final_read.string(&[properties, member]),
                expected,
                "case {:?}: expected {member} {expected:?}, got {final_read:?}",
                case.case
            );
        }
        let ended_at = final_read.field(&[properties, "endedAt"]).as_f64();
        assert!(
            ended_at.is_some(),
            "case {:?}: expected endedAt once COMPLETED, got {final_read:?}",
            case.case
        );
        assert!(
            ended_at >= submitted_at,
            "case {:?}: expected endedAt {ended_at:?} at or after submittedAt {submitted_at:?}",
            case.case
        );
        let configuration = final_read.field(&[properties, "importConfiguration"]);
        assert_eq!(
            !configuration.is_null(),
            case.import_configuration,
            "case {:?}: expected importConfiguration present={}, got {configuration:?}",
            case.case,
            case.import_configuration
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One row of the refusal table.
#[derive(Debug)]
struct RefusalCase {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
    error_code: &'static str,
}

/// RULE: a request naming a resource that does not exist is refused with
/// `ResourceNotFoundException` (404), and one whose identifier cannot name a resource
/// at all is refused with `ValidationException` (400) — never an empty success.
///
/// Rows span both error classes across every route in this slice, and the id axis
/// (too short, too long, wrong character class) plus the published `maxResults`
/// bounds and one value past each.
#[tokio::test]
async fn unknown_and_malformed_identifiers_are_refused() {
    let emulator = Emulator::new();
    let known = emulator.active_datastore("refusal.store", "refusal-store").await;
    let unknown = "0123456789abcdef0123456789abcdef";

    let rows: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "GetDatastore on an unknown but well-formed id",
            method: "GET",
            uri: format!("/datastore/{unknown}"),
            body: None,
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "DeleteDatastore on an unknown but well-formed id",
            method: "DELETE",
            uri: format!("/datastore/{unknown}"),
            body: None,
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "GetDatastore on a 31-character id",
            method: "GET",
            uri: "/datastore/0123456789abcdef0123456789abcde".to_string(),
            body: None,
            status: 400,
            error_code: "ValidationException",
        },
        RefusalCase {
            case: "GetDatastore on a 33-character id",
            method: "GET",
            uri: "/datastore/0123456789abcdef0123456789abcdef0".to_string(),
            body: None,
            status: 400,
            error_code: "ValidationException",
        },
        RefusalCase {
            case: "GetDatastore on an uppercase id",
            method: "GET",
            uri: "/datastore/0123456789ABCDEF0123456789abcdef".to_string(),
            body: None,
            status: 400,
            error_code: "ValidationException",
        },
        RefusalCase {
            case: "GetDICOMImportJob on an unknown data store",
            method: "GET",
            uri: format!("/getDICOMImportJob/datastore/{unknown}/job/abc123"),
            body: None,
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "GetDICOMImportJob on an unknown job",
            method: "GET",
            uri: format!("/getDICOMImportJob/datastore/{known}/job/nosuchjob"),
            body: None,
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "StartDICOMImportJob on an unknown data store",
            method: "POST",
            uri: format!("/startDICOMImportJob/datastore/{unknown}"),
            body: Some(json!({
                "clientToken": "refusal-import",
                "dataAccessRoleArn": "arn:aws:iam::000000000000:role/ImportRole",
                "inputS3Uri": "s3://imaging-in/",
                "outputS3Uri": "s3://imaging-out/",
            })),
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "ListDatastores with a token this service never issued",
            method: "GET",
            uri: "/datastore?nextToken=not-a-token".to_string(),
            body: None,
            status: 400,
            error_code: "ValidationException",
        },
        RefusalCase {
            // Well-formed hex that decodes to UTF-8 but carries no marker from this
            // service: still not a token it issued.
            case: "ListDatastores with a forged but hex-decodable token",
            method: "GET",
            uri: "/datastore?nextToken=6161".to_string(),
            body: None,
            status: 400,
            error_code: "ValidationException",
        },
        RefusalCase {
            case: "ListDatastores with maxResults below the published minimum",
            method: "GET",
            uri: "/datastore?maxResults=0".to_string(),
            body: None,
            status: 400,
            error_code: "ValidationException",
        },
        RefusalCase {
            case: "ListDatastores with maxResults above the published maximum",
            method: "GET",
            uri: "/datastore?maxResults=51".to_string(),
            body: None,
            status: 400,
            error_code: "ValidationException",
        },
    ];

    for row in rows {
        let answer = emulator.call(row.method, &row.uri, row.body).await;
        assert_eq!(
            answer.status, row.status,
            "case {:?}: expected HTTP {}, got {answer:?}",
            row.case, row.status
        );
        assert_eq!(
            answer.error_code, row.error_code,
            "case {:?}: expected error code {:?}, got {answer:?}",
            row.case, row.error_code
        );
    }

    // The published `maxResults` bounds themselves are accepted.
    for accepted in [1_u32, 50] {
        let page = emulator
            .list_datastores(&format!("maxResults={accepted}"))
            .await;
        assert_eq!(
            page.status, 200,
            "case maxResults={accepted}: expected HTTP 200 at a published bound, got {page:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Bounds
// ---------------------------------------------------------------------------

/// RULE: the emulator's own capacity bounds are enforced before anything is persisted —
/// past the cap the call is refused with `ServiceQuotaExceededException` (402) and
/// nothing is stored, so no caller can grow this process's memory without limit.
///
/// Rows span both bounded collections (data stores, and import jobs within one data
/// store), each exercised at the cap and one past it.
#[tokio::test]
async fn the_emulators_capacity_bounds_are_enforced() {
    let emulator = Emulator::new();

    // Fill the data store map exactly to the cap.
    for index in 0..MAX_DATASTORES {
        let created = emulator
            .create_datastore(json!({
                "datastoreName": format!("cap.store-{index}"),
                "clientToken": format!("cap-token-{index}"),
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case \"datastore {index} of {MAX_DATASTORES}\": expected HTTP 200 at or below the cap, got {created:?}"
        );
    }

    let over = emulator
        .create_datastore(json!({
            "datastoreName": "cap.store-over",
            "clientToken": "cap-token-over",
        }))
        .await;
    assert_eq!(
        over.status, 402,
        "case \"one data store past the cap\": expected HTTP 402, got {over:?}"
    );
    assert_eq!(
        over.error_code, "ServiceQuotaExceededException",
        "case \"one data store past the cap\": expected ServiceQuotaExceededException, got {over:?}"
    );

    // Nothing was persisted for the refused call: the token minted no store, so the
    // collection is still exactly at the cap.
    let listed = emulator.list_datastores("maxResults=50").await;
    assert!(
        listed.field(&["nextToken"]).is_string(),
        "case \"one data store past the cap\": expected the listing to still page, got {listed:?}"
    );

    // Now the per-data-store import job bound, on a fresh emulator so the data store
    // cap above is not in play.
    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("cap.jobs", "cap-jobs").await;
    for index in 0..MAX_IMPORT_JOBS_PER_DATASTORE {
        let started = emulator
            .start_import_job(
                &datastore_id,
                json!({
                    "clientToken": format!("cap-job-token-{index}"),
                    "dataAccessRoleArn": "arn:aws:iam::000000000000:role/ImportRole",
                    "inputS3Uri": "s3://imaging-in/",
                    "outputS3Uri": "s3://imaging-out/",
                }),
            )
            .await;
        assert_eq!(
            started.status, 200,
            "case \"import job {index} of {MAX_IMPORT_JOBS_PER_DATASTORE}\": expected HTTP 200 at or below the cap, got {started:?}"
        );
    }

    let over = emulator
        .start_import_job(
            &datastore_id,
            json!({
                "clientToken": "cap-job-token-over",
                "dataAccessRoleArn": "arn:aws:iam::000000000000:role/ImportRole",
                "inputS3Uri": "s3://imaging-in/",
                "outputS3Uri": "s3://imaging-out/",
            }),
        )
        .await;
    assert_eq!(
        over.status, 402,
        "case \"one import job past the cap\": expected HTTP 402, got {over:?}"
    );
    assert_eq!(
        over.error_code, "ServiceQuotaExceededException",
        "case \"one import job past the cap\": expected ServiceQuotaExceededException, got {over:?}"
    );
}

/// RULE: the idempotency ledger does not outlive the resource it names — once the
/// deletion the clock owes has retired a data store, reusing that store's client token
/// mints a NEW store rather than resurrecting the retired id.
///
/// Rows span repeated create/delete cycles on one token, which is the loop that would
/// otherwise leave an entry behind on every pass.
#[tokio::test]
async fn the_idempotency_ledger_is_dropped_with_the_resource_it_named() {
    let emulator = Emulator::new();
    let mut seen = BTreeSet::new();

    for cycle in 0..3 {
        let created = emulator
            .create_datastore(json!({
                "datastoreName": "recycled.store",
                "clientToken": "recycled-token",
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case cycle={cycle}: expected HTTP 200 from CreateDatastore, got {created:?}"
        );
        let datastore_id = created.string(&["datastoreId"]);
        assert!(
            seen.insert(datastore_id.clone()),
            "case cycle={cycle}: expected a NEW datastoreId after the previous store was retired, got the earlier {datastore_id:?}"
        );

        // Only a settled store is deletable, so the creation is waited out first
        // (`tests/all/datastore_status_guard.rs` is where that guard is asserted).
        tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_ACTIVATION_DELAY).await;
        let deleted = emulator.delete_datastore(&datastore_id).await;
        assert_eq!(
            deleted.status, 200,
            "case cycle={cycle}: expected HTTP 200 from DeleteDatastore, got {deleted:?}"
        );
        tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_DELETION_DELAY).await;

        let listed = emulator.list_datastores("maxResults=50").await;
        let remaining = listed
            .field(&["datastoreSummaries"])
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            remaining.is_empty(),
            "case cycle={cycle}: expected every store retired, got {listed:?}"
        );
    }
}
