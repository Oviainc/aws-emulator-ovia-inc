// SPDX-License-Identifier: CC0-1.0

//! How this service answers an id that resolves to no stored resource, across the whole
//! address-list surface, end to end over the wire.
//!
//! RULE: this service answers a request whose id resolves to no stored resource at the
//! status a caller-input failure carries (HTTP 400), with the error code of the condition
//! and its own wording — the modelled `ResourceNotFoundException` and "Could not find
//! AddressList with id <the id the request named>" for an `AddressListId`, and the modelled
//! `ValidationException` with the fixed "Invalid JobId", which echoes no id, for an
//! import-job `JobId` — whichever operation did the resolving.
//!
//! Every assertion goes through the real request path (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target: MailManagerSvc.<Operation>`), so what
//! it pins is the emulator's HTTP behaviour rather than an internal helper. The tests are
//! tables: the rows are ids spanning `AddressListId`/`JobId`'s modelled pattern
//! `^[a-zA-Z0-9-]+$` and both ends of their 1..255 length
//! (`smithy_query(query="shape",shape="AddressListId,JobId")`), crossed with every
//! implemented operation that resolves one.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

/// One emulator holding one MailManager service on one shared engine.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, and the fixed test credential below
        // is not this process's signing key.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call one operation and return `(http status, parsed body)`.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("MailManagerSvc.{operation}"))
            // A signed SDK request routes by its SigV4 credential scope, whose 4th field is
            // this service's signing name (`ses`).
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

    /// Create an address list and return its `AddressListId`, failing the test on any
    /// refusal.
    async fn create_list(&self, name: &str) -> String {
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

    /// Create an import job on `address_list_id` and return its `JobId`, failing the test on
    /// any refusal.
    async fn create_job(&self, address_list_id: &str, name: &str) -> String {
        let (status, out) = self
            .call(
                "CreateAddressListImportJob",
                json!({
                    "AddressListId": address_list_id,
                    "Name": name,
                    "ImportDataFormat": { "ImportDataType": "CSV" },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected CreateAddressListImportJob to answer 200, got {status} \
             {out:?}"
        );
        out["JobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no JobId in {out:?}"))
            .to_string()
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

/// Ids that satisfy the modelled pattern `^[a-zA-Z0-9-]+$` but name nothing: the shortest
/// the 1..255 length admits, an alphanumeric one, a hyphenated one, a prefixed and
/// hex-bodied one, and the longest the length admits.
fn unresolvable_ids() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest id the length admits", "x".to_string()),
        ("an alphanumeric id", "0123456789abcdef".to_string()),
        ("a hyphenated id", "no-such-resource".to_string()),
        (
            "a prefixed, hex-bodied id",
            "al-00000000000000000000000000000000".to_string(),
        ),
        ("the longest id the length admits", "9".repeat(255)),
    ]
}

/// Assert one answer is the refusal the rule calls for: the status this service answers an
/// id it cannot resolve with, carrying `expected_code` and `expected_message`.
fn assert_unresolved_refusal(
    case: &str,
    status: u16,
    body: &Value,
    expected_code: &str,
    expected_message: &str,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: an id that resolves to nothing must answer 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        expected_code,
        "case {case:?}: unexpected error code, got {body:?}"
    );
    assert_eq!(
        body["Message"].as_str(),
        Some(expected_message),
        "case {case:?}: unexpected refusal message, got {body:?}"
    );
}

/// RULE: every operation that resolves an `AddressListId` refuses one that names no stored
/// address list the same way — HTTP 400, the `ResourceNotFoundException` code, message
/// "Could not find AddressList with id <that id>" — and the refusal leaves the store as it
/// was.
#[tokio::test]
async fn an_unresolvable_address_list_id_is_refused_the_same_way_everywhere() {
    let harness = Harness::new();
    let live = harness.create_list("resolution.list").await;
    let live_job = harness.create_job(&live, "resolution.job").await;

    for (row, id) in unresolvable_ids() {
        assert_ne!(
            id, live,
            "case {row:?}: the row must not name the live address list"
        );
        // Every implemented operation that resolves an `AddressListId`, with the rest of
        // each request inside the modelled constraints so the resolution is what refuses
        // it.
        let calls: [(&str, Value); 6] = [
            ("GetAddressList", json!({ "AddressListId": &id })),
            (
                "RegisterMemberToAddressList",
                json!({ "AddressListId": &id, "Address": "user@example.com" }),
            ),
            (
                "DeregisterMemberFromAddressList",
                json!({ "AddressListId": &id, "Address": "user@example.com" }),
            ),
            (
                "GetMemberOfAddressList",
                json!({ "AddressListId": &id, "Address": "user@example.com" }),
            ),
            ("ListMembersOfAddressList", json!({ "AddressListId": &id })),
            (
                "CreateAddressListImportJob",
                json!({
                    "AddressListId": &id,
                    "Name": "orphan.job",
                    "ImportDataFormat": { "ImportDataType": "JSON" },
                }),
            ),
        ];
        for (operation, body) in calls {
            let case = format!("{operation} with {row} ({id:?})");
            let (status, out) = harness.call(operation, body).await;
            assert_unresolved_refusal(
                &case,
                status,
                &out,
                "ResourceNotFoundException",
                &format!("Could not find AddressList with id {id}"),
            );
        }
    }

    // The resource that DOES resolve is untouched by every refusal above, so the rule is
    // about resolution and not a blanket rejection.
    let (status, out) = harness
        .call("GetAddressList", json!({ "AddressListId": &live }))
        .await;
    assert_eq!(
        (status, out["AddressListId"].as_str()),
        (200, Some(live.as_str())),
        "case live-after-refusals: the stored address list must still resolve, got {status} \
         {out:?}"
    );
    let (status, out) = harness
        .call("GetAddressListImportJob", json!({ "JobId": &live_job }))
        .await;
    assert_eq!(
        (status, out["JobId"].as_str()),
        (200, Some(live_job.as_str())),
        "case live-job-after-refusals: the stored import job must still resolve, got {status} \
         {out:?}"
    );
    let (status, out) = harness
        .call("ListMembersOfAddressList", json!({ "AddressListId": &live }))
        .await;
    assert_eq!(
        (status, out["Addresses"].as_array().map(Vec::len)),
        (200, Some(0)),
        "case live-members-after-refusals: no refusal may have registered a member, got \
         {status} {out:?}"
    );
}

/// RULE: an import-job `JobId` that resolves to no stored job is refused with HTTP 400, the
/// `ValidationException` code and the fixed message "Invalid JobId" — which echoes neither
/// the id the request named nor any other stored value.
#[tokio::test]
async fn an_unresolvable_job_id_is_refused_naming_no_id() {
    let harness = Harness::new();
    let live = harness.create_list("job.resolution.list").await;
    let live_job = harness.create_job(&live, "job.resolution.job").await;

    // One more row: the live job's id with its last character changed — a near miss, so the
    // refusal is about the id as a whole and not a prefix match.
    let mut rows = unresolvable_ids();
    let near_miss = format!(
        "{}{}",
        &live_job[..live_job.len() - 1],
        if live_job.ends_with('0') { '1' } else { '0' }
    );
    rows.push(("the live job's id with its last character changed", near_miss));

    for (row, id) in rows {
        assert_ne!(
            id, live_job,
            "case {row:?}: the row must not name the live job"
        );
        let case = format!("GetAddressListImportJob with {row} ({id:?})");
        let (status, out) = harness
            .call("GetAddressListImportJob", json!({ "JobId": &id }))
            .await;
        assert_unresolved_refusal(&case, status, &out, "ValidationException", "Invalid JobId");
        assert!(
            !out["Message"].as_str().unwrap_or_default().contains(&id),
            "case {case:?}: the refusal must not echo the id it was handed, got {out:?}"
        );
    }

    // The job that DOES resolve is still readable, unchanged by the refusals.
    let (status, out) = harness
        .call("GetAddressListImportJob", json!({ "JobId": &live_job }))
        .await;
    assert_eq!(
        (status, out["JobId"].as_str()),
        (200, Some(live_job.as_str())),
        "case live-job-after-refusals: the stored import job must still resolve, got {status} \
         {out:?}"
    );
}

/// RULE (boundary): the refusal above is about RESOLUTION, so an id outside `AddressListId`
/// or `JobId`'s modelled constraints is refused by the boundary validator instead — also a
/// 400 `ValidationException`, but never with the resolution wording, which would claim the
/// service looked an unacceptable id up.
#[tokio::test]
async fn ids_outside_the_modelled_constraints_are_refused_before_resolution() {
    let harness = Harness::new();

    // Rows: one step past each published bound — the empty string (below the 1..255
    // minimum), 256 characters (past the maximum), and characters outside
    // `^[a-zA-Z0-9-]+$`.
    let malformed: [(&str, String); 5] = [
        ("empty, one below the minimum length", String::new()),
        ("256 characters, one past the maximum", "a".repeat(256)),
        ("an underscore, outside the pattern", "not_an_id".to_string()),
        ("a space, outside the pattern", "not an id".to_string()),
        ("a dot, outside the pattern", "not.an.id".to_string()),
    ];
    for (row, id) in malformed {
        for (operation, body) in [
            ("GetAddressList", json!({ "AddressListId": &id })),
            ("GetAddressListImportJob", json!({ "JobId": &id })),
        ] {
            let case = format!("{operation} with {row} ({id:?})");
            let (status, out) = harness.call(operation, body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: an id outside the modelled constraints must answer 400, got \
                 {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ValidationException",
                "case {case:?}: unexpected error code, got {out:?}"
            );
            let message = out["Message"]
                .as_str()
                .or_else(|| out["message"].as_str())
                .unwrap_or_default()
                .to_string();
            assert!(
                !message.contains("Could not find AddressList with id")
                    && message != "Invalid JobId",
                "case {case:?}: a malformed id is refused at the boundary, not as a resolution \
                 miss, got {out:?}"
            );
        }
    }
}
