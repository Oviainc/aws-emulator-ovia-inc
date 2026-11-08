// SPDX-License-Identifier: CC0-1.0

//! WHICH principal a pass-role denial of this service names, driven over the wire
//! (`restJson1`) through the same router the emulator serves from.
//!
//! `tests/all/pass_role_authorization.rs` pins WHICH roles are passable and what each
//! refusal says; this suite varies the one input that decides the principal the denial
//! blames: the credential the request is signed with.
//!
//! The rule under test: the principal named is the identity the request's access key id
//! RESOLVES to — the session that key names, the IAM user the account holds the key for,
//! or the account's own principal for a key no store holds — which is exactly the
//! identity `sts:GetCallerIdentity` answers for the same credential. Nothing here writes
//! an expected ARN by hand: each row ASKS STS, mounted beside this service on the one
//! shared engine, and compares.
//!
//! The rows span what the rule generalises over — the kind of credential a call is signed
//! with (a key no store holds, a long-term key held for an IAM user, a second such key
//! held for a DIFFERENT user, a session of an assumed role, a session of another role) —
//! and each principal's name is deliberately unrelated to the text of the access key id
//! that reaches it, so an identity synthesised from that text cannot pass any row. One
//! further row presents a credential whose session the clock has already expired: the
//! resolution materialises that expiry, so the key names no session any more and both
//! answers are the account's own published principal rather than the role the credential
//! once carried.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// STS's awsQuery API version and SigV4 signing name: the scope its calls are made
/// under.
const STS_API_VERSION: &str = "2011-06-15";
const STS_SIGNING_NAME: &str = "sts";

/// A value that names no IAM role at all, which is what earns the denial under test.
const UNPASSABLE_ROLE: &str = "not-an-arn";

/// A well-formed `graphIdentifier` no graph holds: the pass-role decision is taken before
/// the graph is resolved, so no row has to create one.
const ABSENT_GRAPH: &str = "g-0000000000";

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// How long a seeded session lasts, as `sts:AssumeRole`'s `DurationSeconds` sets it.
const SESSION_DURATION: Duration = Duration::from_secs(3600);

/// This service and STS mounted on the SAME shared engine, so the identity STS answers
/// for a credential is resolved from the very records this service resolves it from.
fn emulator_with_sts() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(sts::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Dispatch one `restJson1` request to this service, signed (in the routing sense) with
/// `access_key_id`.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let payload = serde_json::to_vec(&body).expect("a JSON request body serializes");
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One STS awsQuery call, signed (in the routing sense) with `access_key_id`.
async fn sts_call(router: &MultiServiceRouter, access_key_id: &str, action: &str) -> (u16, String) {
    let body = format!("Action={action}&Version={STS_API_VERSION}");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{STS_SIGNING_NAME}.amazonaws.com/"))
        .header("host", format!("{STS_SIGNING_NAME}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20240101/{REGION}/\
                 {STS_SIGNING_NAME}/aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The text of the one `<name>…</name>` element an awsQuery response carries.
fn element(case: &str, body: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("case {case:?}: no </{name}> in {body}"))
        + start;
    body[start..end].to_string()
}

/// The ARN `sts:GetCallerIdentity` answers for a request signed with `access_key_id`:
/// the identity that credential resolves to, asked of the service that owns the question.
async fn identity_arn_from_sts(
    router: &MultiServiceRouter,
    case: &str,
    access_key_id: &str,
) -> String {
    let (status, body) = sts_call(router, access_key_id, "GetCallerIdentity").await;
    assert_eq!(
        status, 200,
        "case {case:?}: sts:GetCallerIdentity must answer the signing credential, got \
         {status} with {body}"
    );
    element(case, &body, "Arn")
}

/// Record, in the IAM store of the shared engine, that the account holds `access_key_id`
/// for an IAM user named `user_name` — the records `iam:CreateUser` and
/// `iam:CreateAccessKey` write — and answer that user's ARN.
fn hold_long_term_key_for(
    engine: &engine::InMemoryEngine,
    user_name: &str,
    access_key_id: &str,
) -> String {
    let arn = format!(
        "arn:aws:iam::{}:user/{user_name}",
        engine::emulator_account_id()
    );
    let state = iam::service_state(engine).expect("the in-memory engine carries the IAM store");
    let mut account = state.lock();
    account.users.insert(
        iam::name_key(user_name),
        User {
            path: "/".to_string(),
            user_name: user_name.to_string(),
            user_id: sts::state::unique_id("AIDA", &arn),
            arn: arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            password_last_used: None,
            permissions_boundary: None,
            tags: None,
        },
    );
    account.access_keys.insert(
        access_key_id.to_string(),
        iam::AccessKeyRecord {
            user_key: iam::name_key(user_name),
            status: iam::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
        },
    );
    arn
}

/// The identity STS reports for an assumed-role session of `role_name` under
/// `session_name`, built the way STS builds it, so what this suite seeds is a record STS
/// itself could have written.
fn assumed_role_identity(role_name: &str, session_name: &str) -> sts::state::Identity {
    let account = engine::emulator_account_id();
    let role_arn = format!("arn:aws:iam::{account}:role/{role_name}");
    sts::state::Identity {
        user_id: format!(
            "{}:{session_name}",
            sts::state::unique_id("AROA", &role_arn)
        ),
        account: account.to_string(),
        arn: format!("arn:aws:sts::{account}:assumed-role/{role_name}/{session_name}"),
    }
}

/// One assumed-role session keyed by `access_key_id`, vended `age` ago — so an `age` past
/// [`SESSION_DURATION`] is a credential the clock has already expired.
fn session(
    access_key_id: &str,
    role_name: &str,
    session_name: &str,
    age: Duration,
) -> sts::state::Session {
    let account = engine::emulator_account_id();
    sts::state::NewSession {
        role_arn: Some(format!("arn:aws:iam::{account}:role/{role_name}")),
        ..sts::state::NewSession::new(
            assumed_role_identity(role_name, session_name),
            sts::state::SessionKind::AssumedRole,
            i32::try_from(SESSION_DURATION.as_secs()).unwrap_or(i32::MAX),
        )
    }
    .into_session(access_key_id, SystemTime::now() - age)
}

/// Seed `seeded` into the STS session registry the shared engine holds — the registry
/// `sts:AssumeRole` writes and `sts:GetCallerIdentity` reads.
fn seed_session(engine: &engine::InMemoryEngine, seeded: &sts::state::Session) {
    sts::state::service_state(engine)
        .expect("the engine bears the STS store")
        .lock()
        .insert(seeded.clone())
        .expect("the session registry has room");
}

/// The request each pass-role operation is driven with: a request this service accepts in
/// every respect but the role it names.
fn pass_role_requests() -> Vec<(&'static str, &'static str, String, Value)> {
    vec![
        (
            "StartImportTask",
            "POST",
            format!("/graphs/{ABSENT_GRAPH}/importtasks"),
            json!({ "source": "s3://import-bucket/data/", "roleArn": UNPASSABLE_ROLE }),
        ),
        (
            "CreateGraphUsingImportTask",
            "POST",
            "/importtasks".to_string(),
            json!({
                "graphName": "denial-principal-graph",
                "source": "s3://import-bucket/data/",
                "roleArn": UNPASSABLE_ROLE,
            }),
        ),
        (
            "StartExportTask",
            "POST",
            "/exporttasks".to_string(),
            json!({
                "graphIdentifier": ABSENT_GRAPH,
                "roleArn": UNPASSABLE_ROLE,
                "format": "CSV",
                "destination": "s3://export-bucket/dump/",
                "kmsKeyIdentifier": KMS_KEY_ARN,
            }),
        ),
    ]
}

/// The sentence AWS refuses a value naming no role the caller may pass with, for the
/// principal `principal`.
fn not_authorized_denial(principal: &str) -> String {
    format!("User: {principal} is not authorized to perform: null")
}

/// The wire error code of a failure body (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Every pass-role operation, signed with `access_key_id`, is refused 403 naming
/// `principal`.
async fn assert_denial_names(
    router: &MultiServiceRouter,
    case: &str,
    access_key_id: &str,
    principal: &str,
) {
    for (operation, method, path, body) in pass_role_requests() {
        let row = format!("{case} / {operation}");
        let (status, answered) = call_as(router, access_key_id, method, &path, body).await;
        assert_eq!(
            status, 403,
            "case {row:?}: expected 403, got {status}: {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "AccessDeniedException",
            "case {row:?}: expected AccessDeniedException, got {answered}"
        );
        assert_eq!(
            answered["message"],
            json!(not_authorized_denial(principal)),
            "case {row:?}: expected {:?}, got {}",
            not_authorized_denial(principal),
            answered["message"]
        );
    }
}

#[tokio::test]
async fn the_denial_names_the_principal_sts_reports_for_the_credential() {
    let (router, engine) = emulator_with_sts();

    // A long-term key the account holds for an IAM user, and a second one held for a
    // DIFFERENT user: the user's name is nothing like the key id, so a principal
    // synthesised from the key id cannot match.
    let mary_key = "AKIDHELDFORFIRSTUSER";
    let mary = hold_long_term_key_for(&engine, "Mary_Major", mary_key);
    let li_key = "AKIDHELDFORSECONDUSR";
    let li = hold_long_term_key_for(&engine, "Li_Juan", li_key);

    // Two live role sessions, each of its own role.
    let importer = session(
        "ASIANEPTUNEIMPORTER1",
        "NeptuneImportRole",
        "import-session",
        Duration::ZERO,
    );
    let exporter = session(
        "ASIANEPTUNEEXPORTER1",
        "NeptuneExportRole",
        "export-session",
        Duration::ZERO,
    );
    for seeded in [&importer, &exporter] {
        seed_session(&engine, seeded);
    }

    // (case, the access key id presented, the principal it speaks for as STS reports it)
    let rows: Vec<(&str, &str)> = vec![
        ("a long-term key no store holds", "AKIDNOSTOREHOLDSTHIS"),
        ("a long-term key held for an IAM user", mary_key),
        ("a long-term key held for another user", li_key),
        ("a session of an assumed role", importer.access_key_id.as_str()),
        ("a session of another role", exporter.access_key_id.as_str()),
    ];

    let mut named: Vec<String> = Vec::new();
    for (case, access_key_id) in &rows {
        // The expectation comes from the service that owns the question, over the wire.
        let principal = identity_arn_from_sts(&router, case, access_key_id).await;
        assert_denial_names(&router, case, access_key_id, &principal).await;
        named.push(principal);
    }

    // The rows above are not one constant sentence asserted five times: the key no store
    // holds, the two users and the two sessions are each named as themselves.
    assert_eq!(
        named,
        vec![
            sts::state::ambient_identity().arn,
            mary,
            li,
            importer.identity.arn.clone(),
            exporter.identity.arn.clone(),
        ],
        "case the principals of the rows: each credential must be named as itself"
    );
}

#[tokio::test]
async fn a_credential_whose_session_has_expired_is_not_named_as_that_session() {
    let (router, engine) = emulator_with_sts();

    // Seeded an hour past its own expiry: the credential is gone as far as the clock is
    // concerned, which is the state a caller holding a stale session is in.
    let stale = session(
        "ASIANEPTUNEEXPIRED01",
        "NeptuneImportRole",
        "stale-session",
        SESSION_DURATION + Duration::from_secs(3600),
    );
    seed_session(&engine, &stale);

    // The expiry the clock has already made due is materialised by the resolution itself,
    // so the key names no session any more: STS reports the account's own published
    // principal for it, and the denial names exactly that.
    let case = "a session the clock has expired";
    let principal = identity_arn_from_sts(&router, case, &stale.access_key_id).await;
    assert_denial_names(&router, case, &stale.access_key_id, &principal).await;
    assert_ne!(
        principal, stale.identity.arn,
        "case {case:?}: a credential whose session is over must not be named as that \
         session, got {principal:?}"
    );
}
