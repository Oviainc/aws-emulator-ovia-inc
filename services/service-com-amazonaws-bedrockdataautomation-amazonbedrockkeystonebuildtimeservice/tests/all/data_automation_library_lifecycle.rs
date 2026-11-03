// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the DataAutomationLibrary lifecycle.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed-looking request — so routing, HTTP binding, constraint validation,
//! the handler and response serialization are all exercised, exactly as an AWS SDK
//! `bedrock-data-automation` client would.
//!
//! The rule under test is the one the operations' models state: a well-formed
//! `CreateDataAutomationLibrary` answers its modelled `@http` success code (201) with
//! a `libraryArn` inside the modelled `DataAutomationLibraryArn` pattern and the
//! library's ready `status`, and the library it answered about is PERSISTED, so
//! `GetDataAutomationLibrary` reports the members the create carried. The suites are
//! TABLE-driven: each asserts that rule over a set of inputs that differ along what it
//! generalises over (library name form and length, description absent/empty/at its
//! bound, encryption configuration present or absent, tag count, an idempotency token
//! present or absent), not over one recorded request.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state::ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState as ServiceState;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{encode_label, error_code, string_at, timestamp_at};

/// A SigV4 `Authorization` header for this service's signing name (`bedrock`, per
/// `OPERATIONS.md`). `can_handle` resolves the route key from it, and the create reads
/// the region out of its credential scope when minting an ARN.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20240101/{region}/bedrock/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// The emulator under test: one shared engine, one mounted service, request
/// authentication off so the suite signs nothing for real.
fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with a handle on the shared engine, so a case can assert what the
/// store HOLDS and not only what it answers.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Dispatch one request in `region` and return its status and decoded JSON body
/// (`Value::Null` for an empty body).
async fn call_in_region(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
    region: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "bedrock-data-automation.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", authorization(region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("serializable body"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, decoded)
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, method, uri, body, REGION).await
}

/// The region every case but the ARN-region one signs for.
const REGION: &str = "us-east-1";

/// `CreateDataAutomationLibrary`, per its modelled `@http`
/// `{method: PUT, uri: /data-automation-libraries/, code: 201}`.
async fn create_library(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "PUT", "/data-automation-libraries/", body).await
}

/// `GetDataAutomationLibrary`, per its modelled `@http`
/// `{method: POST, uri: /data-automation-libraries/{libraryArn}/}`.
async fn get_library(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    let uri = format!("/data-automation-libraries/{}/", encode_label(arn));
    call(router, "POST", &uri, &json!({})).await
}

/// Structural check of a minted library ARN against the modeled
/// `DataAutomationLibraryArn` pattern
/// `^arn:aws(|-cn|-iso|-iso-[a-z]|-us-gov):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:data-automation-library/[a-zA-Z0-9-]{12,36}$`
/// (source: `smithy_query(shape="DataAutomationLibrary")`), plus its 128-character
/// bound.
fn assert_well_formed_arn(arn: &str, expected_region: &str, case: &str) {
    assert!(
        arn.len() <= 128,
        "case {case:?}: libraryArn is {} characters, over the modeled 128: {arn}",
        arn.len()
    );
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "case {case:?}: libraryArn has {} colon-separated fields, expected 6: {arn}",
        fields.len()
    );
    assert_eq!(fields[0], "arn", "case {case:?}: ARN prefix, got {arn}");
    assert_eq!(fields[1], "aws", "case {case:?}: ARN partition, got {arn}");
    assert_eq!(fields[2], "bedrock", "case {case:?}: ARN service, got {arn}");
    assert_eq!(
        fields[3], expected_region,
        "case {case:?}: ARN region, got {arn}"
    );
    assert!(
        fields[4].len() == 12 && fields[4].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: ARN account must be 12 digits, got {arn}"
    );
    let id = fields[5]
        .strip_prefix("data-automation-library/")
        .unwrap_or_else(|| {
            panic!("case {case:?}: ARN resource must be data-automation-library/<id>, got {arn}")
        });
    assert!(
        (12..=36).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "case {case:?}: ARN resource id must be 12..36 of [a-zA-Z0-9-], got {id:?}"
    );
}

/// A `clientToken` of the modelled form: `@length(33..256)` with
/// `@pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`, so 33 characters at its lower bound.
fn client_token(seed: &str) -> String {
    let mut token = format!("tok-{seed}");
    while token.len() < 33 {
        token.push('a');
    }
    token
}

/// One create request, as the members that make the rows differ.
struct CreateCase {
    /// The case name every assertion reports.
    case: &'static str,
    /// `libraryName` (modelled `@length(1..128)`, `@pattern ^[a-zA-Z0-9-_]+$`).
    name: String,
    /// `libraryDescription`, or `None` to leave the member absent.
    description: Option<String>,
    /// `encryptionConfiguration.kmsKeyId`, or `None` to leave the whole structure
    /// absent.
    kms_key_id: Option<&'static str>,
    /// `encryptionConfiguration.kmsEncryptionContext`, only sent when
    /// [`Self::kms_key_id`] is.
    kms_context: Option<(&'static str, &'static str)>,
    /// How many tags the request carries; `None` leaves `tags` absent.
    tags: Option<usize>,
    /// The `@idempotencyToken` the request carries, or `None` to leave it absent.
    token: Option<String>,
}

/// The create cases: the whole class a well-formed create spans, one row per way the
/// request can differ.
///
/// The bounds the rows walk come from the model, not from a recording:
/// `libraryName` is `@length(1..128)` over `^[a-zA-Z0-9-_]+$` (both published bounds
/// and the whole character class appear below), `libraryDescription` is
/// `@length(0..300)` (absent, empty, punctuation out of its pattern's class, and the
/// 300-character bound), `tags` is `@length(0..200)`, and `encryptionConfiguration`
/// is the optional structure `{kmsKeyId (required), kmsEncryptionContext}`
/// (`smithy_query(shape="EncryptionConfiguration")`).
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "minimal: only the one required member, name at the 1-character bound",
            name: "l".to_string(),
            description: None,
            kms_key_id: None,
            kms_context: None,
            tags: None,
            token: None,
        },
        CreateCase {
            case: "hyphens and underscores, empty description at its 0 bound, no tags",
            name: "library-with_both-1".to_string(),
            description: Some(String::new()),
            kms_key_id: None,
            kms_context: None,
            tags: Some(0),
            token: None,
        },
        CreateCase {
            case: "digits-only name, punctuation description, key and context, one tag",
            name: "20261029".to_string(),
            description: Some("invoices (2026) - Q4: #1 \"golden\" set!".to_string()),
            kms_key_id: Some("alias/bedrock-data-automation"),
            kms_context: Some(("tenant", "acme")),
            tags: Some(1),
            token: Some(client_token("digits")),
        },
        CreateCase {
            case: "name at the 128-character upper bound, description at 300, many tags",
            name: "a".repeat(128),
            description: Some("d".repeat(300)),
            kms_key_id: Some(
                "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555",
            ),
            kms_context: None,
            tags: Some(5),
            token: Some(client_token("upper-bound")),
        },
        CreateCase {
            case: "mixed-case name, description, no encryption, two tags, token present",
            name: "MixedCase_Library-2".to_string(),
            description: Some("entities for contract review".to_string()),
            kms_key_id: None,
            kms_context: None,
            tags: Some(2),
            token: Some(client_token("mixed")),
        },
    ]
}

impl CreateCase {
    /// The request body this case sends: a member the case leaves `None` is ABSENT from
    /// the JSON, not sent as null, so the row exercises the handler's own defaulting.
    fn body(&self) -> Value {
        let mut body = json!({ "libraryName": self.name });
        if let Some(description) = &self.description {
            body["libraryDescription"] = json!(description);
        }
        if let Some(key) = self.kms_key_id {
            let mut encryption = json!({ "kmsKeyId": key });
            if let Some((name, value)) = self.kms_context {
                encryption["kmsEncryptionContext"] = json!({ name: value });
            }
            body["encryptionConfiguration"] = encryption;
        }
        if let Some(count) = self.tags {
            body["tags"] = Value::Array(
                (0..count)
                    .map(|index| json!({ "key": format!("k{index}"), "value": format!("v{index}") }))
                    .collect(),
            );
        }
        if let Some(token) = &self.token {
            body["clientToken"] = json!(token);
        }
        body
    }
}

#[tokio::test]
async fn a_well_formed_create_answers_the_modeled_201_and_persists_the_library() {
    let (router, engine) = emulator_with_engine();

    for spec in create_cases() {
        let case = spec.case;
        let (status, created) = create_library(&router, &spec.body()).await;

        // The rule this suite exists for: the modelled `@http` code, never a 5xx.
        assert_eq!(
            status, 201,
            "case {case:?}: expected the modeled CreateDataAutomationLibrary 201, \
             got {status} with {created:?}"
        );
        let arn = string_at(&created, "libraryArn", case);
        assert_well_formed_arn(&arn, REGION, case);
        // A library has nothing to provision, so the create answers the ready status
        // of the modelled pair `["ACTIVE", "DELETING"]`
        // (`smithy_query(shape="DataAutomationLibraryStatus")`); the other half is the
        // teardown's.
        assert_eq!(
            string_at(&created, "status", case),
            state::LIBRARY_ACTIVE,
            "case {case:?}: a just-created library is {}: {created:?}",
            state::LIBRARY_ACTIVE
        );

        // PERSISTED: the store holds the record the response named, under the name and
        // the values the request carried.
        {
            let store = engine.state::<ServiceState>();
            let held = store.lock();
            let record = held
                .libraries
                .get(&arn)
                .unwrap_or_else(|| panic!("case {case:?}: the store holds no library at {arn}"));
            assert_eq!(
                record.library_name, spec.name,
                "case {case:?}: the stored library carries the wrong libraryName"
            );
            assert_eq!(
                held.library_names.get(&spec.name).map(String::as_str),
                Some(arn.as_str()),
                "case {case:?}: the name index must resolve {:?} to {arn}",
                spec.name
            );
            assert_eq!(
                record.tags.len(),
                spec.tags.unwrap_or(0),
                "case {case:?}: the stored library carries the wrong tag count"
            );
            if let Some(token) = &spec.token {
                assert_eq!(
                    held.library_create_idempotency
                        .get(token)
                        .map(String::as_str),
                    Some(arn.as_str()),
                    "case {case:?}: the clientToken must resolve to {arn}"
                );
            }
        }

        // ROUND TRIP: the read answers with the record that was stored.
        let (status, payload) = get_library(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetDataAutomationLibrary 200, got {status} with {payload:?}"
        );
        let fetched = payload.get("library").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: GetDataAutomationLibrary returned no library: {payload:?}")
        });
        assert_eq!(
            string_at(&fetched, "libraryArn", case),
            arn,
            "case {case:?}: GetDataAutomationLibrary answered about another library: {fetched:?}"
        );
        assert_eq!(
            string_at(&fetched, "libraryName", case),
            spec.name,
            "case {case:?}: the wrong libraryName read back: {fetched:?}"
        );
        assert_eq!(
            fetched.get("libraryDescription").and_then(Value::as_str),
            spec.description.as_deref(),
            "case {case:?}: the wrong libraryDescription read back: {fetched:?}"
        );
        assert_eq!(
            string_at(&fetched, "status", case),
            state::LIBRARY_ACTIVE,
            "case {case:?}: a created library reads back {}: {fetched:?}",
            state::LIBRARY_ACTIVE
        );
        // The create's `encryptionConfiguration` is what a read reports as the
        // library's `kmsKeyId`/`kmsEncryptionContext`; absent in, absent out.
        assert_eq!(
            fetched.get("kmsKeyId").and_then(Value::as_str),
            spec.kms_key_id,
            "case {case:?}: the wrong kmsKeyId read back: {fetched:?}"
        );
        let read_context = fetched.get("kmsEncryptionContext");
        match spec.kms_context {
            Some((name, value)) => assert_eq!(
                read_context,
                Some(&json!({ name: value })),
                "case {case:?}: the wrong kmsEncryptionContext read back: {fetched:?}"
            ),
            None => assert!(
                read_context.is_none_or(Value::is_null),
                "case {case:?}: a create that named no kmsEncryptionContext must read \
                 back none, got {read_context:?}"
            ),
        }
        // A library stands up with nothing ingested into it, so it reports no entity
        // type — not one it was never given.
        assert_eq!(
            fetched.get("entityTypes").and_then(Value::as_array),
            Some(&Vec::new()),
            "case {case:?}: a just-created library holds no entityTypes: {fetched:?}"
        );
        // `creationTime` is `@required`, so a read that omitted it would be
        // unparseable; this also pins the modelled `date-time` wire form.
        let _ = timestamp_at(&fetched, "creationTime", case);
    }
}

#[tokio::test]
async fn every_create_mints_its_own_library_arn() {
    let router = emulator();
    let mut seen: Vec<String> = Vec::new();

    for spec in create_cases() {
        let case = spec.case;
        let (status, created) = create_library(&router, &spec.body()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {created:?}"
        );
        let arn = string_at(&created, "libraryArn", case);
        assert!(
            !seen.contains(&arn),
            "case {case:?}: libraryArn {arn} was already minted for another library: {seen:?}"
        );
        seen.push(arn);
    }
}

#[tokio::test]
async fn the_arn_region_is_the_one_the_request_was_signed_for() {
    let router = emulator();

    // Every row is the same create in a different credential scope, so the only thing
    // that may differ in the answer is the ARN's region field.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2", "us-gov-west-1"] {
        let body = json!({ "libraryName": format!("region-{}", region.replace('-', "_")) });
        let (status, created) =
            call_in_region(&router, "PUT", "/data-automation-libraries/", &body, region).await;
        assert_eq!(
            status, 201,
            "case {region:?}: expected 201, got {status} with {created:?}"
        );
        assert_well_formed_arn(&string_at(&created, "libraryArn", region), region, region);
    }
}

#[tokio::test]
async fn a_second_library_under_a_taken_name_is_the_modeled_conflict() {
    let (router, engine) = emulator_with_engine();

    let (status, created) = create_library(&router, &json!({ "libraryName": "contended_name" })).await;
    assert_eq!(
        status, 201,
        "the first create must succeed, got {status} with {created:?}"
    );
    let arn = string_at(&created, "libraryArn", "first create");

    // The rule is about the NAME, so the rows differ in everything else: a repeat of
    // the same request, and repeats that add a description, an encryption
    // configuration and tags.
    let repeats = [
        ("bare repeat", json!({ "libraryName": "contended_name" })),
        (
            "repeat with a description",
            json!({ "libraryName": "contended_name", "libraryDescription": "another one" }),
        ),
        (
            "repeat with encryption and tags",
            json!({
                "libraryName": "contended_name",
                "encryptionConfiguration": { "kmsKeyId": "alias/other" },
                "tags": [{ "key": "k", "value": "v" }],
            }),
        ),
    ];
    for (case, body) in repeats {
        let (status, payload) = create_library(&router, &body).await;
        assert_eq!(
            status, 409,
            "case {case:?}: a name already taken is the modeled ConflictException (409), \
             got {status} with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ConflictException",
            "case {case:?}: wrong error code in {payload:?}"
        );
        assert_eq!(
            payload.get("message").and_then(Value::as_str),
            Some(state::LIBRARY_NAME_CONFLICT_MESSAGE),
            "case {case:?}: wrong refusal message in {payload:?}"
        );
    }

    // A refused create left NOTHING behind: the one library of that name is still the
    // first one.
    let store = engine.state::<ServiceState>();
    let held = store.lock();
    assert_eq!(
        held.libraries.len(),
        1,
        "a refused create must not insert a record, store holds {:?}",
        held.libraries.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        held.library_names.get("contended_name").map(String::as_str),
        Some(arn.as_str()),
        "the name index must still resolve to the first library"
    );
}

#[tokio::test]
async fn an_idempotency_token_replay_returns_the_first_library() {
    let (router, engine) = emulator_with_engine();

    // Rows differ in how much of the request the replay repeats, because the token is
    // what decides: the same body, and a body that changes every other member.
    let token = client_token("replay");
    let first = json!({
        "libraryName": "replayed_library",
        "libraryDescription": "the first description",
        "clientToken": token,
    });
    let (status, created) = create_library(&router, &first).await;
    assert_eq!(
        status, 201,
        "the first create must succeed, got {status} with {created:?}"
    );
    let arn = string_at(&created, "libraryArn", "first create");

    let replays = [
        ("identical replay", first.clone()),
        (
            "replay that changes the name and description",
            json!({
                "libraryName": "a_different_name",
                "libraryDescription": "a different description",
                "clientToken": token,
            }),
        ),
    ];
    for (case, body) in replays {
        let (status, payload) = create_library(&router, &body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: a token replay answers the modeled 201, got {status} with {payload:?}"
        );
        assert_eq!(
            string_at(&payload, "libraryArn", case),
            arn,
            "case {case:?}: a replay must answer about the first library: {payload:?}"
        );
        assert_eq!(
            string_at(&payload, "status", case),
            state::LIBRARY_ACTIVE,
            "case {case:?}: wrong status in {payload:?}"
        );
    }

    // A fresh token under a fresh name creates a SECOND library: the token short-circuit
    // is keyed on the token, not on the operation.
    let (status, second) = create_library(
        &router,
        &json!({ "libraryName": "second_library", "clientToken": client_token("second") }),
    )
    .await;
    assert_eq!(
        status, 201,
        "a create under a fresh token must succeed, got {status} with {second:?}"
    );
    assert_ne!(
        string_at(&second, "libraryArn", "fresh token"),
        arn,
        "a fresh token must mint its own library: {second:?}"
    );

    let store = engine.state::<ServiceState>();
    let held = store.lock();
    assert_eq!(
        held.libraries.len(),
        2,
        "the two tokens created two libraries, store holds {:?}",
        held.libraries.keys().collect::<Vec<_>>()
    );
    assert!(
        !held.library_names.contains_key("a_different_name"),
        "a replay must not reserve the name it carried: {:?}",
        held.library_names.keys().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_library_arn_that_resolves_to_nothing_is_the_modeled_404() {
    let router = emulator();

    // Every row is a well-formed `DataAutomationLibraryArn` (so the modelled pattern
    // admits it and the request reaches the handler) that names no stored library: a
    // never-created id, and an id belonging to a different region and partition-legal
    // account field.
    let unknown = [
        (
            "never created",
            "arn:aws:bedrock:us-east-1:000000000000:data-automation-library/0000000a-00000000000000ff",
        ),
        (
            "another region",
            "arn:aws:bedrock:eu-west-1:000000000000:data-automation-library/dead-beef-1234",
        ),
        (
            "id at the 12-character lower bound",
            "arn:aws:bedrock:us-east-1:000000000000:data-automation-library/abcdefabcdef",
        ),
    ];
    for (case, arn) in unknown {
        let (status, payload) = get_library(&router, arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an unknown libraryArn is the modeled ResourceNotFoundException \
             (404), got {status} with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {case:?}: wrong error code in {payload:?}"
        );
        assert_eq!(
            payload.get("message").and_then(Value::as_str),
            Some(state::LIBRARY_ARN_NOT_FOUND_MESSAGE),
            "case {case:?}: wrong refusal message in {payload:?}"
        );
    }
}

#[tokio::test]
async fn reading_a_library_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator_with_engine();

    let (status, created) = create_library(
        &router,
        &json!({ "libraryName": "stable_library", "libraryDescription": "unchanged by reads" }),
    )
    .await;
    assert_eq!(
        status, 201,
        "the create must succeed, got {status} with {created:?}"
    );
    let arn = string_at(&created, "libraryArn", "create");

    let (_, first) = get_library(&router, &arn).await;
    let (_, second) = get_library(&router, &arn).await;
    assert_eq!(
        first, second,
        "two reads of one library must answer identically: {first:?} then {second:?}"
    );

    let store = engine.state::<ServiceState>();
    let held = store.lock();
    assert_eq!(
        held.libraries.len(),
        1,
        "reads must not add a record, store holds {:?}",
        held.libraries.keys().collect::<Vec<_>>()
    );
}
