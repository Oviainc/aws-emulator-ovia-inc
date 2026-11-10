// SPDX-License-Identifier: CC0-1.0

//! Who last changed a parameter, end to end: `PutParameter` writes and the metadata reads
//! report the writer.
//!
//! The rule under test: a parameter write records the ARN of the principal whose
//! credential signed it — the identity a caller-identity read resolves for that same
//! credential — and every metadata read of that parameter reports that principal as the
//! version's `LastModifiedUser` ("Amazon Resource Name (ARN) of the Amazon Web Services
//! user who last changed the parameter",
//! `smithy_query(query="doc",shape="ParameterMetadata",member="LastModifiedUser")`).
//!
//! The suites are TABLES whose rows differ along what the rule generalises over: the KIND
//! of credential the write was signed with (the account's own published credential, an IAM
//! user's own long-term access key, the temporary credential of an assumed-role session),
//! the Region the write was signed for, and which version of a parameter is read. Nothing
//! is written into the table as the expected ARN: each row's expectation is read back from
//! `sts:GetCallerIdentity` with the very credential the write was signed with, so a writer
//! this service invented could not pass.
//!
//! Every call goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` for this service, and the awsQuery form for the
//! IAM and STS calls that make the principals — and nothing reaches into a state struct.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{REGION, TARGET_PREFIX};

/// A second Region, so the rule is seen to hold at whichever endpoint the write was signed
/// for rather than only at one.
const OTHER_REGION: &str = "eu-central-1";

/// The access-key id standing for the account's own published credential: neither STS nor
/// IAM holds a record of it, so it resolves to the identity this emulator publishes that
/// credential as.
const ACCOUNT_KEY: &str = "AKIDTEST";

/// A principal a write is signed as: the access key id it presents, and the ARN a
/// caller-identity read reports for it.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// A router with this service beside the IAM and STS services the principals are made
/// with, all on ONE shared engine — so the identity this service records is resolved from
/// the records those services wrote.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite signs nothing: the `Authorization` header carries only the credential
    // scope, which is where a service reads the caller's access key id and Region from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// One call to this service, signed for `region` with `access_key_id` in the credential
/// scope, returning `(status, body)`.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20240101/{region}/ssm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed)
}

/// One call to this service that must succeed, returning its body.
async fn ok(
    router: &MultiServiceRouter,
    case: &str,
    access_key_id: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, response) = call_as(router, access_key_id, region, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must succeed, got {status} {response}"
    );
    response
}

/// One awsQuery call to IAM or STS, signed with `access_key_id`, which must succeed.
async fn query(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, String)],
) -> String {
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
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20240101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {action:?}: the call must succeed, got {status} with {text}"
    );
    text
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC
/// 3986's unreserved characters literal — so a policy document's braces and quotes survive
/// the round trip.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an awsQuery response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The ARN `sts:GetCallerIdentity` reports for the credential `access_key_id` — the LIVE
/// identity of that credential, which is what each row's expectation is read from.
async fn caller_identity_arn(router: &MultiServiceRouter, access_key_id: &str) -> String {
    let answered = query(
        router,
        access_key_id,
        "sts",
        "2011-06-15",
        "GetCallerIdentity",
        &[],
    )
    .await;
    xml_element(&answered, "Arn")
        .unwrap_or_else(|| panic!("GetCallerIdentity answers an Arn, got {answered}"))
}

/// The account's own published credential, as a principal: nothing holds a record of it,
/// so STS reports the identity this emulator publishes it as.
async fn account_credential(router: &MultiServiceRouter) -> Principal {
    Principal {
        access_key_id: ACCOUNT_KEY.to_string(),
        arn: caller_identity_arn(router, ACCOUNT_KEY).await,
    }
}

/// Create an IAM user, mint a long-term access key for it, and answer that key beside the
/// ARN a caller-identity read reports for it.
async fn user(router: &MultiServiceRouter, user_name: &str) -> Principal {
    query(
        router,
        ACCOUNT_KEY,
        "iam",
        "2010-05-08",
        "CreateUser",
        &[("UserName", user_name.to_string())],
    )
    .await;
    let minted = query(
        router,
        ACCOUNT_KEY,
        "iam",
        "2010-05-08",
        "CreateAccessKey",
        &[("UserName", user_name.to_string())],
    )
    .await;
    let access_key_id = xml_element(&minted, "AccessKeyId")
        .unwrap_or_else(|| panic!("CreateAccessKey answers an AccessKeyId, got {minted}"));
    let arn = caller_identity_arn(router, &access_key_id).await;
    Principal {
        access_key_id,
        arn,
    }
}

/// Create an IAM role, assume it through STS, and answer the session's temporary access key
/// beside the ARN a caller-identity read reports for it.
async fn assumed_role(router: &MultiServiceRouter, role_name: &str) -> Principal {
    let account = engine::emulator_account_id();
    let trust = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"arn:aws:iam::{account}:root\"}},\
         \"Action\":\"sts:AssumeRole\"}}]}}"
    );
    query(
        router,
        ACCOUNT_KEY,
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    let assumed = query(
        router,
        ACCOUNT_KEY,
        "sts",
        "2011-06-15",
        "AssumeRole",
        &[
            ("RoleArn", format!("arn:aws:iam::{account}:role/{role_name}")),
            ("RoleSessionName", "writer".to_string()),
        ],
    )
    .await;
    let access_key_id = xml_element(&assumed, "AccessKeyId")
        .unwrap_or_else(|| panic!("AssumeRole answers an AccessKeyId, got {assumed}"));
    let arn = caller_identity_arn(router, &access_key_id).await;
    Principal {
        access_key_id,
        arn,
    }
}

/// `DescribeParameters` as the account's own credential, narrowed to the one name, and the
/// metadata it reported for it.
async fn described(router: &MultiServiceRouter, case: &str, region: &str, name: &str) -> Value {
    let response = ok(
        router,
        case,
        ACCOUNT_KEY,
        region,
        "DescribeParameters",
        &json!({ "ParameterFilters": [{ "Key": "Name", "Values": [name] }] }),
    )
    .await;
    response["Parameters"]
        .as_array()
        .and_then(|reported| reported.first())
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: a describe must report {name}, got {response}"))
}

/// Every entry `GetParameterHistory` reported for `name`, oldest first.
async fn history(router: &MultiServiceRouter, case: &str, region: &str, name: &str) -> Vec<Value> {
    let response = ok(
        router,
        case,
        ACCOUNT_KEY,
        region,
        "GetParameterHistory",
        &json!({ "Name": name }),
    )
    .await;
    response["Parameters"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: a history must report entries, got {response}"))
}

/// The `LastModifiedUser` of one reported metadata entry, or `None` when it reported none.
fn last_modified_user(reported: &Value) -> Option<&str> {
    reported["LastModifiedUser"].as_str()
}

/// RULE: whichever KIND of credential signed the write, and whichever Region it was signed
/// for, both metadata reads report the principal that credential speaks for — the ARN
/// `sts:GetCallerIdentity` answers for it.
#[tokio::test]
async fn the_metadata_reads_report_the_principal_whose_credential_wrote_the_parameter() {
    let router = emulator();
    // (case, the principal that writes, the Region it signs for)
    let rows: Vec<(&str, Principal, &str)> = vec![
        (
            "the account's own published credential",
            account_credential(&router).await,
            REGION,
        ),
        (
            "an IAM user's own long-term access key",
            user(&router, "parameter-writer").await,
            REGION,
        ),
        (
            "the temporary credential of an assumed-role session",
            assumed_role(&router, "parameter-writer-role").await,
            REGION,
        ),
        (
            "an IAM user writing at another Region's endpoint",
            user(&router, "other-region-writer").await,
            OTHER_REGION,
        ),
    ];

    for (case, writer, region) in &rows {
        let name = format!("/LastWriter/{}", writer.access_key_id);
        ok(
            &router,
            case,
            &writer.access_key_id,
            region,
            "PutParameter",
            &json!({ "Name": name, "Type": "String", "Value": "v" }),
        )
        .await;

        let reported = described(&router, case, region, &name).await;
        assert_eq!(
            last_modified_user(&reported),
            Some(writer.arn.as_str()),
            "case {case:?}: a describe must report the writer {:?}, got {reported}",
            writer.arn
        );

        let entries = history(&router, case, region, &name).await;
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: one write makes one history entry, got {entries:?}"
        );
        assert_eq!(
            last_modified_user(&entries[0]),
            Some(writer.arn.as_str()),
            "case {case:?}: a history entry must report the writer {:?}, got {:?}",
            writer.arn,
            entries[0]
        );
    }

    // The principals are DIFFERENT principals: a rule that reported one fixed identity for
    // every credential would pass every row above while recording nothing.
    let mut reported_writers: Vec<String> = Vec::new();
    for (case, writer, _) in &rows {
        assert!(
            !reported_writers.contains(&writer.arn),
            "case {case:?}: each row must write as its own principal, got {:?} twice",
            writer.arn
        );
        reported_writers.push(writer.arn.clone());
    }
}

/// RULE, per VERSION: each version records the principal that wrote THAT version, so a
/// history reports a different writer per entry while a describe — which reports a
/// parameter at its newest version — reports the one that wrote last.
#[tokio::test]
async fn each_version_reports_the_principal_that_wrote_that_version() {
    let router = emulator();
    let name = "/LastWriter/handed-over";
    // (case, the principal that writes this version)
    let writers: Vec<(&str, Principal)> = vec![
        ("the first version", account_credential(&router).await),
        ("the second version", user(&router, "second-writer").await),
        (
            "the third version",
            assumed_role(&router, "third-writer-role").await,
        ),
    ];

    for (case, writer) in &writers {
        ok(
            &router,
            case,
            &writer.access_key_id,
            REGION,
            "PutParameter",
            &json!({
                "Name": name,
                "Type": "String",
                "Value": format!("written by {}", writer.arn),
                "Overwrite": true,
            }),
        )
        .await;
    }

    let entries = history(&router, "the whole history", REGION, name).await;
    assert_eq!(
        entries.len(),
        writers.len(),
        "case \"the whole history\": expected one entry per write, got {entries:?}"
    );
    for (index, (case, writer)) in writers.iter().enumerate() {
        let entry = &entries[index];
        assert_eq!(
            last_modified_user(entry),
            Some(writer.arn.as_str()),
            "case {case:?}: expected the writer {:?}, got {entry}",
            writer.arn
        );
        assert_eq!(
            entry["Version"],
            json!(index as i64 + 1),
            "case {case:?}: expected version {}, got {entry}",
            index + 1
        );
    }

    let last = writers
        .last()
        .expect("the table carries at least one writer");
    let reported = described(&router, "the newest version", REGION, name).await;
    assert_eq!(
        last_modified_user(&reported),
        Some(last.1.arn.as_str()),
        "case \"the newest version\": expected the last writer {:?}, got {reported}",
        last.1.arn
    );
}

/// RULE, as a read: the writer a metadata read reports comes from the store, so repeated
/// reads report one writer and a read changes nothing a later caller sees.
#[tokio::test]
async fn repeated_reads_report_the_one_recorded_writer() {
    let router = emulator();
    let writer = user(&router, "stable-writer").await;
    let name = "/LastWriter/stable";
    ok(
        &router,
        "the write",
        &writer.access_key_id,
        REGION,
        "PutParameter",
        &json!({ "Name": name, "Type": "String", "Value": "v" }),
    )
    .await;

    // Read by each of the two metadata reads, twice over, and as a DIFFERENT identity than
    // the one that wrote: who asks selects nothing.
    for round in 1..=2 {
        let case = format!("round {round}");
        let reported = described(&router, &case, REGION, name).await;
        assert_eq!(
            last_modified_user(&reported),
            Some(writer.arn.as_str()),
            "case {case:?}: a describe must report {:?} on every read, got {reported}",
            writer.arn
        );
        let entries = history(&router, &case, REGION, name).await;
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: a read must add no version, got {entries:?}"
        );
        assert_eq!(
            last_modified_user(&entries[0]),
            Some(writer.arn.as_str()),
            "case {case:?}: a history must report {:?} on every read, got {:?}",
            writer.arn,
            entries[0]
        );
    }
}
