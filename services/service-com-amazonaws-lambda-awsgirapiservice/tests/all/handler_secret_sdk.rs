// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING handler reads back from Secrets Manager, end to end over the wire:
//! Lambda, Secrets Manager, KMS, IAM and the token service on ONE engine, and a `python3.x`
//! function whose code does `import boto3` and fetches a secret.
//!
//! Two rules are pinned here.
//!
//! 1. A member the called operation's model types as a BLOB is handed to the handler as the
//!    `bytes` the answer carries, decoded from the base64 an AWS JSON protocol travels it
//!    as — never as that base64 text. So a handler does with it what it does with bytes on a
//!    managed runtime, and the bytes it sees are the ones the service holds.
//! 2. A secret under a CUSTOMER MANAGED key is read by decrypting it through that key on the
//!    CALLER's behalf, so the read needs the caller's authority to use the key beside the
//!    authority to read the secret: an execution role whose policies do not reach
//!    `kms:Decrypt` on that key is refused, and the same read is served once they do.
//!
//! The rows differ along what each rule generalises over: for the first, the member the value
//! travels in and the bytes it carries (text, bytes no text encoding carries, every byte
//! value, the published length bounds of the value); for the second, whether the secret names
//! a customer managed key at all and which of the two authorities the role's own policy
//! reaches.

use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::invoke_request_id::one_invocation_stream_at_a_time;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The module and attribute every package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// The access-key id standing for the account's own published credential, which is what every
/// request this suite makes on its own behalf presents.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The name of the inline identity policy a row writes on the execution role.
const INLINE_POLICY: &str = "inline";

/// The error code the AWS common authorization refusal carries, which is what a read the
/// secret's key does not permit is answered with.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The whole message Secrets Manager refuses an operation with when the KMS call it makes for
/// the secret's key is not allowed for the caller (the sentence that service's own
/// `state::envelope` states, which names neither principal, action nor key).
const KMS_ACCESS_DENIED_MESSAGE: &str = "Access to KMS is not allowed";

/// Hands out a `ClientRequestToken` no other call in this suite uses: a raw HTTP request to
/// Secrets Manager names the version it creates itself ("you must generate a
/// ClientRequestToken and include it in the request", `CreateSecretRequest$ClientRequestToken`).
static TOKENS: AtomicU64 = AtomicU64::new(0);

/// A fresh token inside the modelled 32..64 length bound of `ClientRequestTokenType`.
fn fresh_token() -> String {
    format!("{:0>36}", TOKENS.fetch_add(1, Ordering::Relaxed))
}

/// Lambda, Secrets Manager, KMS, IAM and the token service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's SDK call is dispatched back into this
/// same router, so the secret it reads is the secret this suite wrote.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the Region and the caller's access key id.
        // The handler's OWN call is signed by the runtime, with the credential it was given.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// A SigV4 `Authorization` whose credential scope names `service`, which is what routes
    /// the request. The signature is never checked here.
    fn authorization(service: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{service}/\
             aws4_request, SignedHeaders=host, Signature=not-verified"
        )
    }

    /// POST one `awsJson1_1` operation of `service`, as the account's own credential.
    async fn json(
        &self,
        service: &str,
        target_prefix: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let host = format!("{service}.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}/"))
            .header("host", host)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header("authorization", Self::authorization(service))
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One Secrets Manager operation that must be served.
    async fn secrets(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self
            .json("secretsmanager", "secretsmanager", operation, body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from Secrets Manager {operation}, got {status} with \
             {parsed}"
        );
        parsed
    }

    /// One KMS operation that must be served.
    async fn kms(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.json("kms", "TrentService", operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from KMS {operation}, got {status} with {parsed}"
        );
        parsed
    }

    /// One awsQuery call to IAM or the token service, whatever it answers.
    async fn query_any(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", Self::authorization(signing_name))
            .body(bytes::Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (
            status,
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// One awsQuery call that must be served.
    async fn query(
        &self,
        case: &str,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> String {
        let (status, answered) = self.query_any(signing_name, version, action, params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {signing_name} {action} must succeed, got {status} {answered}"
        );
        answered
    }

    /// Create the execution role `role_name`, trusted by Lambda, carrying `policy` as its
    /// inline identity policy — written with IAM's own operations, so the role a session is
    /// minted of is the record `iam:CreateRole` wrote.
    async fn execution_role(&self, case: &str, role_name: &str, policy: &str) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"Service": "lambda.amazonaws.com"},
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        self.query(
            case,
            "iam",
            "2010-05-08",
            "CreateRole",
            &[
                ("RoleName", role_name.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        self.role_policy(case, role_name, policy).await;
        format!(
            "arn:aws:iam::{}:role/{role_name}",
            engine::emulator_account_id()
        )
    }

    /// The identity policy in force on `role_name` from now on, so a row is about the grant
    /// in force when the handler calls rather than the one in force when the role was made.
    async fn role_policy(&self, case: &str, role_name: &str, document: &str) {
        self.query(
            case,
            "iam",
            "2010-05-08",
            "PutRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", INLINE_POLICY.to_string()),
                ("PolicyDocument", document.to_string()),
            ],
        )
        .await;
    }

    /// One Lambda `restJson1` call, as its status, headers and parsed body.
    async fn lambda(&self, method: &str, uri: &str, body: &[u8]) -> (u16, http::HeaderMap, Value) {
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("lambda.{REGION}.amazonaws.com"))
            .header("authorization", Self::authorization("lambda"))
            .header("content-type", "application/json")
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        let parsed: Value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
        };
        (parts.status.as_u16(), parts.headers, parsed)
    }

    /// Create a function whose code is `source`, running as `role_arn`, and wait out the
    /// provisioning window.
    async fn function(&self, case: &str, name: &str, role_arn: &str, source: &str) {
        let (status, _headers, created) = self
            .lambda(
                "POST",
                FUNCTIONS_URI,
                &serde_json::to_vec(&json!({
                    "FunctionName": name,
                    "Role": role_arn,
                    "Runtime": "python3.12",
                    "Handler": HANDLER,
                    "Timeout": 10,
                    "Code": {
                        "ZipFile": base64::engine::general_purpose::STANDARD
                            .encode(package(source)),
                    },
                }))
                .expect("the create body serializes"),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({created})"
        );
        tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
    }

    /// `Invoke` the function synchronously with `event`, answering the `FunctionError` signal
    /// and the payload the run produced.
    async fn invoke(&self, case: &str, name: &str, event: &Value) -> (Option<String>, Value) {
        let (status, headers, payload) = self
            .lambda(
                "POST",
                &format!("{FUNCTIONS_URI}/{name}/invocations"),
                event.to_string().as_bytes(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from Invoke, got {status} ({payload})"
        );
        let signal = headers
            .get("x-amz-function-error")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        (signal, payload)
    }

    /// The reads one invocation of the suite's handler reports, one answer per read.
    async fn reads(&self, case: &str, function_name: &str, reads: Vec<Value>) -> Vec<Value> {
        let (signal, payload) = self
            .invoke(case, function_name, &json!({"reads": reads}))
            .await;
        assert_eq!(
            signal, None,
            "case {case:?}: the handler must run to completion, got the function error \
             {signal:?} with {payload}"
        );
        payload["reads"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the handler reported no reads in {payload}"))
            .clone()
    }

    /// Mint a symmetric KMS key in [`REGION`] under its DEFAULT key policy — the one AWS
    /// writes, which "gives the Amazon Web Services account that owns the KMS key permission
    /// to use it" and so leaves each principal's own identity policies to decide
    /// (`CreateKeyRequest$Policy`) — and answer its ARN.
    async fn create_key(&self, case: &str) -> String {
        let created = self.kms(case, "CreateKey", &json!({})).await;
        created["KeyMetadata"]["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateKey reported no Arn in {created}"))
            .to_string()
    }

    /// Create the secret `name` holding `value`, under the key `kms_key_id` names when a row
    /// names one, and answer its ARN.
    async fn create_secret(
        &self,
        case: &str,
        name: &str,
        kms_key_id: Option<&str>,
        value: &Value,
    ) -> String {
        let mut body = json!({"Name": name, "ClientRequestToken": fresh_token()});
        let members = body.as_object_mut().expect("the body is an object");
        if let Some(key) = kms_key_id {
            members.insert("KmsKeyId".to_string(), json!(key));
        }
        if let Value::Object(value) = value {
            members.extend(value.clone());
        }
        let created = self.secrets(case, "CreateSecret", &body).await;
        created["ARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateSecret reported no ARN in {created}"))
            .to_string()
    }
}

/// `application/x-www-form-urlencoded` for one awsQuery parameter value.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// A .zip deployment package holding `source` as `lambda_function.py`, written in memory the
/// way a caller uploads one.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// One identity policy document granting `actions` on every resource.
fn identity_policy(actions: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": actions, "Resource": "*"}],
    })
    .to_string()
}

/// The handler every test below runs: one `GetSecretValue` per read the event names, reporting
/// what the SDK handed it — including the PYTHON TYPE of each value member, which is what the
/// blob rule is about, and the base64 of a binary value, which only a bytes-like value
/// encodes.
const HANDLER_SOURCE: &str = r#"
import base64

import boto3
import botocore.exceptions

CLIENT = boto3.client("secretsmanager")


def _read(read):
    try:
        answered = CLIENT.get_secret_value(SecretId=read["secret_id"], **read.get("arguments", {}))
    except botocore.exceptions.ClientError as error:
        return {
            "ok": False,
            "code": error.response["Error"]["Code"],
            "message": error.response["Error"]["Message"],
            "status": error.response["ResponseMetadata"]["HTTPStatusCode"],
        }
    report = {
        "ok": True,
        "name": answered.get("Name"),
        "members": sorted(name for name in answered if name != "ResponseMetadata"),
    }
    if "SecretString" in answered:
        report["string_type"] = type(answered["SecretString"]).__name__
        report["secret_string"] = answered["SecretString"]
    if "SecretBinary" in answered:
        carried = answered["SecretBinary"]
        report["binary_type"] = type(carried).__name__
        report["binary_length"] = len(carried)
        # Encoding the value is itself the assertion: `b64encode` takes a bytes-like object
        # and refuses text, which is what a blob handed over as its base64 text would be.
        report["secret_binary"] = base64.b64encode(carried).decode("ascii")
    return report


def lambda_handler(event, context):
    return {"reads": [_read(read) for read in event["reads"]]}
"#;

/// One row of the value table: the members a `CreateSecret` carries the value in, and what the
/// handler must read back for each.
struct ValueRow {
    case: &'static str,
    /// The text a string-valued secret holds, if this row is one.
    text: Option<String>,
    /// The bytes a binary-valued secret holds, if this row is one.
    bytes: Option<Vec<u8>>,
}

impl ValueRow {
    fn string(case: &'static str, text: &str) -> Self {
        Self {
            case,
            text: Some(text.to_string()),
            bytes: None,
        }
    }

    fn binary(case: &'static str, bytes: Vec<u8>) -> Self {
        Self {
            case,
            text: None,
            bytes: Some(bytes),
        }
    }

    /// The value members the create carries: `SecretBinary` travels as the base64 an AWS JSON
    /// protocol carries a blob as.
    fn wire(&self) -> Value {
        match (&self.text, &self.bytes) {
            (Some(text), _) => json!({"SecretString": text}),
            (_, Some(bytes)) => json!({
                "SecretBinary": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
            _ => Value::Null,
        }
    }
}

/// The value rows: both members a secret value travels in, and for the binary one the byte
/// contents the rule generalises over — bytes no text encoding carries, every byte value, and
/// the published length bounds of `SecretBinaryType` (`@length` min 1, max 65536, Secrets
/// Manager's own Smithy model).
fn value_rows() -> Vec<ValueRow> {
    vec![
        ValueRow::string("a string value", "s3cr3t-text"),
        ValueRow::string(
            "a string value holding a JSON document",
            &json!({"username": "admin", "password": "hunter2"}).to_string(),
        ),
        ValueRow::binary("a binary value of one byte, the shortest a value may be", vec![0]),
        ValueRow::binary(
            "a binary value no text encoding carries",
            vec![0x00, 0xff, 0xfe, 0x80, 0x0a],
        ),
        ValueRow::binary("a binary value of every byte value", (0u8..=255).collect()),
        ValueRow::binary(
            "a binary value of the longest length a value may be",
            (0..65536).map(|index| (index % 251) as u8).collect(),
        ),
    ]
}

/// RULE: a member the called operation's model types as a blob is handed to the handler as the
/// `bytes` the answer carries — decoded from the base64 an AWS JSON protocol travels it as —
/// so a handler does with it what it does with bytes, while a string member stays `str`.
#[tokio::test]
async fn a_blob_member_of_an_answer_is_handed_to_the_handler_as_bytes() {
    let _stream = one_invocation_stream_at_a_time().await;
    let role_name = "lambda-secret-reader";
    let function_name = "secret-reader";
    let emulator = Emulator::new();
    let role = emulator
        .execution_role(
            "setup",
            role_name,
            &identity_policy(&["secretsmanager:GetSecretValue"]),
        )
        .await;
    emulator
        .function("setup", function_name, &role, HANDLER_SOURCE)
        .await;

    let rows = value_rows();
    let mut reads = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let name = format!("handler-value-{index}");
        emulator
            .create_secret(row.case, &name, None, &row.wire())
            .await;
        reads.push(json!({"secret_id": name}));
    }
    let answers = emulator.reads("the value table", function_name, reads).await;
    assert_eq!(
        answers.len(),
        rows.len(),
        "expected one answer per read, got {answers:?}"
    );
    for (row, answer) in rows.iter().zip(&answers) {
        let case = row.case;
        assert_eq!(
            answer["ok"],
            json!(true),
            "case {case:?}: expected the read to be served, got {answer}"
        );
        if let Some(text) = &row.text {
            assert_eq!(
                answer["string_type"],
                json!("str"),
                "case {case:?}: expected a string value as str, got {answer}"
            );
            assert_eq!(
                answer["secret_string"],
                json!(text),
                "case {case:?}: expected the text the secret holds, got {answer}"
            );
            assert_eq!(
                answer["members"],
                json!(["ARN", "CreatedDate", "Name", "SecretString", "VersionId", "VersionStages"]),
                "case {case:?}: expected the output members of a string-valued read, got {answer}"
            );
            continue;
        }
        let bytes = row.bytes.as_ref().expect("a row carries one of the two");
        assert_eq!(
            answer["binary_type"],
            json!("bytes"),
            "case {case:?}: expected a blob member as bytes, got {answer}"
        );
        assert_eq!(
            answer["binary_length"],
            json!(bytes.len()),
            "case {case:?}: expected the length of the bytes the secret holds, got {answer}"
        );
        assert_eq!(
            answer["secret_binary"],
            json!(base64::engine::general_purpose::STANDARD.encode(bytes)),
            "case {case:?}: expected the bytes the secret holds, got {answer}"
        );
    }
}

/// One row of the key table: the key the secret is held under, and whether the read of it must
/// be served.
struct KeyRow {
    case: &'static str,
    /// The key this row's secret is held under.
    key: Key,
    served: bool,
}

/// Which key the row's secret is held under, which is what decides whether the role's grant
/// reaches it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Key {
    /// The customer managed key the role's own policy names.
    Granted,
    /// Another customer managed key, which that policy does not name.
    Ungranted,
    /// No key of the caller's at all: the Amazon Web Services managed key, which "names no
    /// key a caller can act on".
    ServiceManaged,
}

/// The key rows, all read in ONE invocation under ONE role policy: what varies is the key the
/// value is held under and whether the role's `kms:Decrypt` grant names it.
const KEY_ROWS: &[KeyRow] = &[
    KeyRow {
        case: "a secret under the customer managed key the role may use",
        key: Key::Granted,
        served: true,
    },
    KeyRow {
        case: "a secret under a customer managed key the role's grant does not name",
        key: Key::Ungranted,
        served: false,
    },
    KeyRow {
        case: "a secret on the service-managed key, which needs no grant of its own",
        key: Key::ServiceManaged,
        served: true,
    },
];

/// RULE: a secret under a customer managed key is decrypted through that key on the CALLER's
/// behalf, so a handler's read of it needs its execution role's authority to use THAT key
/// (`kms:Decrypt`) beside the authority to read the secret: a key the role's grant does not
/// reach refuses the read, the key it does reach serves it, and a secret on the
/// service-managed key needs no grant of its own — while taking the authority to read the
/// secret away refuses even the key the role may use.
#[tokio::test]
async fn a_handlers_read_of_a_secret_under_a_customer_managed_key_needs_kms_decrypt() {
    let _stream = one_invocation_stream_at_a_time().await;
    let role_name = "lambda-cmk-secret-reader";
    let function_name = "cmk-secret-reader";
    let emulator = Emulator::new();
    let granted_key = emulator.create_key("setup").await;
    let ungranted_key = emulator.create_key("setup").await;
    // The role may read every secret, and may use ONE of the two keys.
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": "secretsmanager:GetSecretValue",
                "Resource": "*",
            },
            {"Effect": "Allow", "Action": "kms:Decrypt", "Resource": granted_key},
        ],
    })
    .to_string();
    let role = emulator.execution_role("setup", role_name, &policy).await;
    emulator
        .function("setup", function_name, &role, HANDLER_SOURCE)
        .await;

    let mut reads = Vec::new();
    let mut values = Vec::new();
    for (index, row) in KEY_ROWS.iter().enumerate() {
        let name = format!("handler-key-{index}");
        let text = format!("kms-protected-{index}");
        let key = match row.key {
            Key::Granted => Some(granted_key.as_str()),
            Key::Ungranted => Some(ungranted_key.as_str()),
            Key::ServiceManaged => None,
        };
        emulator
            .create_secret(row.case, &name, key, &json!({"SecretString": text}))
            .await;
        reads.push(json!({"secret_id": name}));
        values.push((name, text));
    }
    let answers = emulator.reads("the key table", function_name, reads).await;
    assert_eq!(
        answers.len(),
        KEY_ROWS.len(),
        "expected one answer per read of the table, got {answers:?}"
    );
    for ((row, answer), (_name, text)) in KEY_ROWS.iter().zip(&answers).zip(&values) {
        let case = row.case;
        assert_eq!(
            answer["ok"],
            json!(row.served),
            "case {case:?}: expected the read to be served={}, got {answer}",
            row.served
        );
        if row.served {
            assert_eq!(
                answer["secret_string"],
                json!(text),
                "case {case:?}: expected the value the secret holds, got {answer}"
            );
            continue;
        }
        assert_eq!(
            answer["code"],
            json!(ACCESS_DENIED_CODE),
            "case {case:?}: expected the common authorization refusal, got {answer}"
        );
        assert_eq!(
            answer["message"],
            json!(KMS_ACCESS_DENIED_MESSAGE),
            "case {case:?}: expected Secrets Manager's own sentence for a KMS refusal, got \
             {answer}"
        );
    }

    // The OTHER authority, read the other way: with `secretsmanager:GetSecretValue` gone the
    // read of the key the role may still use is refused too, so the two are separate grants
    // rather than one.
    let case = "the role keeps kms:Decrypt and loses secretsmanager:GetSecretValue";
    emulator
        .role_policy(
            case,
            role_name,
            &json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Effect": "Allow", "Action": "kms:Decrypt", "Resource": granted_key},
                ],
            })
            .to_string(),
        )
        .await;
    let answers = emulator
        .reads(
            case,
            function_name,
            vec![json!({"secret_id": values[0].0.clone()})],
        )
        .await;
    assert_eq!(
        answers[0]["ok"],
        json!(false),
        "case {case:?}: expected the read to be refused, got {:?}",
        answers[0]
    );
    assert_eq!(
        answers[0]["code"],
        json!(ACCESS_DENIED_CODE),
        "case {case:?}: expected the common authorization refusal, got {:?}",
        answers[0]
    );
}
