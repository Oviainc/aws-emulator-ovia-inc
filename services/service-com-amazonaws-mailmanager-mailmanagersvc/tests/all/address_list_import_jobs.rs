// SPDX-License-Identifier: CC0-1.0

//! Address-list import jobs, end to end over the wire.
//!
//! Every assertion here goes through the real request path — `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target: MailManagerSvc.<Operation>` for the
//! modelled operations, and a plain `PUT` of the published upload URL for an input file — so
//! what it pins is the emulator's HTTP behaviour, not an internal helper.
//!
//! The tests are tables: each row is one input drawn from the class the behaviour covers
//! (both `ImportDataType` values the model declares, several `JobName` shapes across the
//! modelled pattern and both length bounds, ids that name nothing, and the methods and
//! body sizes the upload target serves).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mailmanager_mailmanagersvc::state::{
    IMPORT_JOB_UPLOAD_PLANE, MAX_IMPORT_FILE_BYTES,
};
use service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

/// The endpoint host the test addresses, which the upload URL this service hands out has to
/// name: a client follows that URL against the endpoint it is already talking to.
const HOST: &str = "mail-manager.localhost:9297";

/// The `ImportDataType` values the model declares
/// (`smithy_query(query="shape",shape="ImportDataType")`: `CSV`, `JSON`).
const IMPORT_DATA_TYPES: [&str; 2] = ["CSV", "JSON"];

/// The `ImportJobStatus` a job that has imported nothing carries
/// (`smithy_query(query="shape",shape="ImportJobStatus")` lists `CREATED`, `PROCESSING`,
/// `COMPLETED`, `FAILED` and `STOPPED`).
const CREATED: &str = "CREATED";

/// One emulator holding one MailManager service on one shared engine.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, and the fixed test credential
        // below is not this process's signing key.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call one modelled operation and return `(http status, parsed body)`.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", HOST)
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("MailManagerSvc.{operation}"))
            // A signed SDK request routes by its SigV4 credential scope, whose 4th field
            // is this service's signing name (`ses`) and whose 3rd is the region.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{REGION}/ses/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a valid HTTP request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            json!({})
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Follow a published upload URL exactly as a client follows a pre-signed one: `method`
    /// against the path the URL names, with NO credential of any kind.
    async fn follow_upload_url(&self, method: &str, url: &str, file: Vec<u8>) -> (u16, Value) {
        let path = url
            .split_once(HOST)
            .map(|(_, path)| path.to_string())
            .unwrap_or_else(|| panic!("the upload URL {url:?} must name the endpoint {HOST:?}"));
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", HOST)
            .header("content-type", "text/csv")
            .body(bytes::Bytes::from(file))
            .expect("a valid HTTP request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            json!({})
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create an address list and return its `AddressListId`, failing the test on any
    /// refusal.
    async fn create_address_list(&self, name: &str) -> String {
        let (status, out) = self
            .call("CreateAddressList", json!({ "AddressListName": name }))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected CreateAddressList to answer 200, got {status} {out:?}"
        );
        out["AddressListId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no AddressListId in {out:?}"))
            .to_string()
    }

    /// Create an import job and return `(JobId, PreSignedUrl)`, failing the test on any
    /// refusal.
    async fn create_job(&self, case: &str, body: Value) -> (String, String) {
        let (status, out) = self
            .call("CreateAddressListImportJob", body.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateAddressListImportJob to answer 200, got {status} \
             {out:?}"
        );
        let job_id = out["JobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no JobId in {out:?}"))
            .to_string();
        let url = out["PreSignedUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no PreSignedUrl in {out:?}"))
            .to_string();
        (job_id, url)
    }
}

/// The wire error code of a failure body: the `__type` member, without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .split('#')
        .next_back()
        .unwrap_or_default()
        .to_string()
}

/// A minted `JobId` satisfies `JobId`'s modelled constraints: length 1..255 and pattern
/// `^[a-zA-Z0-9-]+$` (`smithy_query(query="shape",shape="JobId")`).
fn assert_well_formed_job_id(case: &str, id: &str) {
    assert!(
        (1..=255).contains(&id.len())
            && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "case {case:?}: JobId must match ^[a-zA-Z0-9-]+$ within 1..255, got {id:?}"
    );
}

/// Every created import job is readable back exactly as it was stored, whatever its name
/// and whichever input format it was created for — in the status a job that has imported
/// nothing carries, with no instant and no count of an import that has not run.
///
/// The names span `JobName`'s modelled pattern `^[a-zA-Z0-9_.-]+$` and its length bounds
/// 1..255 (`smithy_query(query="shape",shape="JobName")`), and the rows cycle through both
/// `ImportDataType` values and over two different address lists, so the job is read back
/// against the list it was created for rather than against the only one there is.
#[tokio::test]
async fn create_then_get_round_trips_every_job_shape() {
    let harness = Harness::new();
    let lists = [
        harness.create_address_list("allow.list-1").await,
        harness.create_address_list("deny_list_2").await,
    ];

    let names: [String; 6] = [
        "a".to_string(),
        "with_underscore".to_string(),
        "with.dots.in.it".to_string(),
        "with-hyphens-too".to_string(),
        "0123456789".to_string(),
        "j".repeat(255),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let data_type = IMPORT_DATA_TYPES[index % IMPORT_DATA_TYPES.len()];
        let address_list_id = &lists[index % lists.len()];
        let case = format!("name {name:?} of type {data_type}");

        let (job_id, url) = harness
            .create_job(
                &case,
                json!({
                    "AddressListId": address_list_id,
                    "Name": name,
                    "ImportDataFormat": { "ImportDataType": data_type },
                }),
            )
            .await;
        assert_well_formed_job_id(&case, &job_id);
        minted.push(job_id.clone());

        // The upload target is a locator for THIS job on the endpoint the caller reached.
        assert_eq!(
            url,
            format!("https://{HOST}/_aws/{IMPORT_JOB_UPLOAD_PLANE}/{job_id}"),
            "case {case:?}: unexpected PreSignedUrl, got {url:?}"
        );

        let (status, out) = harness
            .call("GetAddressListImportJob", json!({ "JobId": job_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetAddressListImportJob to answer 200, got {status} {out:?}"
        );
        assert_eq!(
            out["JobId"].as_str(),
            Some(job_id.as_str()),
            "case {case:?}: the job must report the id it was asked for, got {out:?}"
        );
        assert_eq!(
            out["Name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: the job must report the stored name, got {out:?}"
        );
        assert_eq!(
            out["AddressListId"].as_str(),
            Some(address_list_id.as_str()),
            "case {case:?}: the job must report the address list it imports into, got {out:?}"
        );
        assert_eq!(
            out["ImportDataFormat"]["ImportDataType"].as_str(),
            Some(data_type),
            "case {case:?}: the job must report the stored input format, got {out:?}"
        );
        assert_eq!(
            out["Status"].as_str(),
            Some(CREATED),
            "case {case:?}: a job that has imported nothing is {CREATED}, got {out:?}"
        );
        assert_eq!(
            out["PreSignedUrl"].as_str(),
            Some(url.as_str()),
            "case {case:?}: the job must report the upload target it was created with, got \
             {out:?}"
        );
        assert!(
            out["CreatedTimestamp"].as_f64().is_some_and(|t| t > 0.0),
            "case {case:?}: CreatedTimestamp must be a real instant, got {out:?}"
        );
        // Nothing has imported, so the members only a running import fills are absent.
        for member in [
            "StartTimestamp",
            "CompletedTimestamp",
            "ImportedItemsCount",
            "FailedItemsCount",
            "Error",
        ] {
            assert!(
                out.get(member).is_none_or(Value::is_null),
                "case {case:?}: a job that has not started reports no {member}, got {out:?}"
            );
        }

        // A read changes nothing a later caller sees: a second read answers the same.
        let (_, again) = harness
            .call("GetAddressListImportJob", json!({ "JobId": job_id }))
            .await;
        assert_eq!(
            again, out,
            "case {case:?}: GetAddressListImportJob must be stable across reads, got {again:?}"
        );
    }

    // Distinct creates mint distinct ids, even within one clock tick.
    let unique: std::collections::BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        unique.len(),
        minted.len(),
        "case mint-uniqueness: every create must mint a fresh JobId, got {minted:?}"
    );
}

/// An `AddressListId` that names no stored address list, and a `JobId` that names no stored
/// import job, are both refused at the status this service answers an id it cannot resolve
/// with (400) and create nothing — the address-list miss with the
/// `ResourceNotFoundException` code and the id it was handed, the job miss with the
/// `ValidationException` code and no id at all.
#[tokio::test]
async fn ids_that_name_nothing_are_refused() {
    let harness = Harness::new();
    let address_list_id = harness.create_address_list("present").await;
    let (job_id, _) = harness
        .create_job(
            "the live job",
            json!({
                "AddressListId": &address_list_id,
                "Name": "live.job-1",
                "ImportDataFormat": { "ImportDataType": "CSV" },
            }),
        )
        .await;

    // Rows: ids that satisfy the modelled pattern but name nothing — the shortest one, a
    // hyphenated one, the longest one, and a live id with one character changed.
    let unknown: [String; 4] = [
        "x".to_string(),
        "no-such-resource".to_string(),
        "9".repeat(255),
        format!("{}0", &address_list_id[..address_list_id.len() - 1]),
    ];
    for id in &unknown {
        if id == &address_list_id {
            continue;
        }
        let (status, out) = harness
            .call(
                "CreateAddressListImportJob",
                json!({
                    "AddressListId": id,
                    "Name": "orphan.job",
                    "ImportDataFormat": { "ImportDataType": "CSV" },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {id:?}: an import job for an unknown address list must answer 400, got \
             {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ResourceNotFoundException",
            "case {id:?}: unexpected error code, got {out:?}"
        );
        assert_eq!(
            out["Message"].as_str(),
            Some(format!("Could not find AddressList with id {id}").as_str()),
            "case {id:?}: unexpected refusal message, got {out:?}"
        );

        if id == &job_id {
            continue;
        }
        let (status, out) = harness
            .call("GetAddressListImportJob", json!({ "JobId": id }))
            .await;
        assert_eq!(
            status, 400,
            "case {id:?}: an unknown JobId must answer 400, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {id:?}: unexpected error code, got {out:?}"
        );
        assert_eq!(
            out["Message"].as_str(),
            Some("Invalid JobId"),
            "case {id:?}: unexpected refusal message, got {out:?}"
        );
    }

    // The job created before the misses is untouched by them.
    let (status, out) = harness
        .call("GetAddressListImportJob", json!({ "JobId": &job_id }))
        .await;
    assert_eq!(
        status, 200,
        "case live-after-misses: the stored job must still be readable, got {status} {out:?}"
    );
}

/// `ClientToken` is `@idempotencyToken`: the SAME token with the SAME request is a retry and
/// answers with the job the first call minted, upload target included; with a DIFFERENT
/// request it is not a retry and is refused with the modelled `ValidationException` (400) —
/// this operation models no conflict.
#[tokio::test]
async fn client_tokens_make_a_create_a_retry() {
    let harness = Harness::new();
    let address_list_id = harness.create_address_list("idempotent.list").await;

    // Rows vary the request a token is first used for: both input formats and both ends of
    // the name bound.
    let first_requests: [(&str, &str, &str); 3] = [
        ("csv", "job.one", "CSV"),
        ("json", "job_two", "JSON"),
        ("shortest-name", "k", "CSV"),
    ];
    for (case, name, data_type) in first_requests {
        let token = format!("tok-{case}");
        let body = json!({
            "AddressListId": &address_list_id,
            "Name": name,
            "ImportDataFormat": { "ImportDataType": data_type },
            "ClientToken": &token,
        });

        let (job_id, url) = harness.create_job(case, body.clone()).await;
        let (repeat_id, repeat_url) = harness.create_job(case, body.clone()).await;
        assert_eq!(
            (repeat_id.as_str(), repeat_url.as_str()),
            (job_id.as_str(), url.as_str()),
            "case {case:?}: a retry with ClientToken {token:?} must answer with the first \
             call's job"
        );

        // The same token with a different request is not a retry. Rows: a different name,
        // a different input format, a different address list.
        let other_list = harness
            .create_address_list(&format!("other.list-{case}"))
            .await;
        let differing: [(&str, Value); 3] = [
            (
                "different name",
                json!({
                    "AddressListId": &address_list_id,
                    "Name": format!("{name}-other"),
                    "ImportDataFormat": { "ImportDataType": data_type },
                    "ClientToken": &token,
                }),
            ),
            (
                "different input format",
                json!({
                    "AddressListId": &address_list_id,
                    "Name": name,
                    "ImportDataFormat": {
                        "ImportDataType": if data_type == "CSV" { "JSON" } else { "CSV" },
                    },
                    "ClientToken": &token,
                }),
            ),
            (
                "different address list",
                json!({
                    "AddressListId": &other_list,
                    "Name": name,
                    "ImportDataFormat": { "ImportDataType": data_type },
                    "ClientToken": &token,
                }),
            ),
        ];
        for (differs, request) in differing {
            let (status, out) = harness.call("CreateAddressListImportJob", request).await;
            assert_eq!(
                status, 400,
                "case {case:?}/{differs:?}: reusing a ClientToken for a different request must \
                 be refused, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ValidationException",
                "case {case:?}/{differs:?}: unexpected error code, got {out:?}"
            );
        }

        // The job the token recorded is unchanged by the refusals.
        let (status, out) = harness
            .call("GetAddressListImportJob", json!({ "JobId": &job_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the recorded job must survive a refused token reuse, got {status} \
             {out:?}"
        );
        assert_eq!(
            out["Name"].as_str(),
            Some(name),
            "case {case:?}: the recorded job must keep its name, got {out:?}"
        );
    }
}

/// A request outside the modelled constraints is refused with a 400 `ValidationException`
/// before anything is stored — never a 501 and never a create.
#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let harness = Harness::new();
    let address_list_id = harness.create_address_list("constrained.list").await;

    // Rows walk each published bound of the input and one step past it: `JobName`
    // (len 1..255, pattern `^[a-zA-Z0-9_.-]+$`), `AddressListId`'s pattern, the
    // `ImportDataType` enum, and each required member absent.
    let cases: [(&str, Value); 8] = [
        (
            "empty Name, one below the minimum length",
            json!({
                "AddressListId": &address_list_id,
                "Name": "",
                "ImportDataFormat": { "ImportDataType": "CSV" },
            }),
        ),
        (
            "a 256-character Name, one past the maximum length",
            json!({
                "AddressListId": &address_list_id,
                "Name": "n".repeat(256),
                "ImportDataFormat": { "ImportDataType": "CSV" },
            }),
        ),
        (
            "a space in Name, outside the pattern",
            json!({
                "AddressListId": &address_list_id,
                "Name": "not allowed",
                "ImportDataFormat": { "ImportDataType": "CSV" },
            }),
        ),
        (
            "punctuation in Name, outside the pattern",
            json!({
                "AddressListId": &address_list_id,
                "Name": "job!",
                "ImportDataFormat": { "ImportDataType": "CSV" },
            }),
        ),
        (
            "an underscore in AddressListId, outside its pattern",
            json!({
                "AddressListId": "not_an_id",
                "Name": "job.ok",
                "ImportDataFormat": { "ImportDataType": "CSV" },
            }),
        ),
        (
            "an ImportDataType the enum does not declare",
            json!({
                "AddressListId": &address_list_id,
                "Name": "job.ok",
                "ImportDataFormat": { "ImportDataType": "XML" },
            }),
        ),
        (
            "no Name at all",
            json!({
                "AddressListId": &address_list_id,
                "ImportDataFormat": { "ImportDataType": "CSV" },
            }),
        ),
        (
            "no ImportDataFormat at all",
            json!({ "AddressListId": &address_list_id, "Name": "job.ok" }),
        ),
    ];
    for (case, body) in cases {
        let (status, out) = harness.call("CreateAddressListImportJob", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
    }

    // The longest name the bound admits is accepted, so the refusals above are the bound
    // and not a blanket rejection.
    let (job_id, _) = harness
        .create_job(
            "255 characters, at the maximum length",
            json!({
                "AddressListId": &address_list_id,
                "Name": "m".repeat(255),
                "ImportDataFormat": { "ImportDataType": "JSON" },
            }),
        )
        .await;
    assert_well_formed_job_id("255 characters, at the maximum length", &job_id);
}

/// The upload target a job publishes is REALLY SERVED: a `PUT` of the input file to the URL
/// the create call handed out is accepted, whatever the file's size up to the bound this
/// emulator retains; a URL naming no stored job is not, and the target states which method
/// it serves. Accepting the file imports nothing by itself, so the job stays in the status
/// it was created in.
#[tokio::test]
async fn the_published_upload_url_serves_the_input_file() {
    let harness = Harness::new();
    let address_list_id = harness.create_address_list("upload.list").await;

    // Rows span the file sizes the target takes: empty, a small CSV, a small JSON
    // document, and exactly the largest body this emulator retains.
    let files: [(&str, Vec<u8>); 4] = [
        ("an empty file", Vec::new()),
        (
            "a CSV of addresses",
            b"address\nalice@example.com\nbob@example.org\n".to_vec(),
        ),
        (
            "a JSON document of addresses",
            br#"[{"address":"alice@example.com"}]"#.to_vec(),
        ),
        ("the largest retained body", vec![b'a'; MAX_IMPORT_FILE_BYTES]),
    ];
    for (case, file) in files {
        let data_type = if case.contains("JSON") { "JSON" } else { "CSV" };
        let (job_id, url) = harness
            .create_job(
                case,
                json!({
                    "AddressListId": &address_list_id,
                    "Name": "upload.job",
                    "ImportDataFormat": { "ImportDataType": data_type },
                }),
            )
            .await;

        let size = file.len();
        let (status, out) = harness.follow_upload_url("PUT", &url, file).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a PUT of {size} bytes to the published upload URL must be \
             accepted, got {status} {out:?}"
        );

        // The upload is not an import: nothing ran, so the job is still in the status it
        // was created in and still names the same upload target.
        let (status, read) = harness
            .call("GetAddressListImportJob", json!({ "JobId": &job_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the job must still be readable after the upload, got {status} \
             {read:?}"
        );
        assert_eq!(
            (read["Status"].as_str(), read["PreSignedUrl"].as_str()),
            (Some(CREATED), Some(url.as_str())),
            "case {case:?}: an upload imports nothing by itself, so the job keeps its status \
             and its target, got {read:?}"
        );
    }

    // A body past the bound this emulator retains is refused by the upload plane rather
    // than stored.
    let (_, url) = harness
        .create_job(
            "an oversized upload",
            json!({
                "AddressListId": &address_list_id,
                "Name": "oversized.job",
                "ImportDataFormat": { "ImportDataType": "CSV" },
            }),
        )
        .await;
    let (status, out) = harness
        .follow_upload_url("PUT", &url, vec![b'a'; MAX_IMPORT_FILE_BYTES + 1])
        .await;
    assert_eq!(
        status, 413,
        "case an oversized upload: a body past the retained bound must be refused, got \
         {status} {out:?}"
    );

    // Rows: the methods a published upload target does NOT serve, and a target naming no
    // stored job.
    let refused: [(&str, &str, String); 3] = [
        ("a GET of the upload target", "GET", url.clone()),
        ("a DELETE of the upload target", "DELETE", url.clone()),
        (
            "a PUT to a job that does not exist",
            "PUT",
            format!("https://{HOST}/_aws/{IMPORT_JOB_UPLOAD_PLANE}/no-such-import-job"),
        ),
    ];
    for (case, method, target) in refused {
        let (status, out) = harness
            .follow_upload_url(method, &target, b"address\n".to_vec())
            .await;
        assert_ne!(
            status, 200,
            "case {case:?}: must not be served as an upload, got {status} {out:?}"
        );
        let expected = if method == "PUT" { 404 } else { 405 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {out:?}"
        );
    }
}
