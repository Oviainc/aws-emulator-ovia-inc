// SPDX-License-Identifier: CC0-1.0

//! WHO `CreateWebLoginToken` says asked for the token, end to end over HTTP.
//!
//! RULE: the two identity members report the CALLER's identity, as the service that minted
//! the caller's credential knows it — `IamIdentity` is the NAME of that IAM identity, which
//! is the resource field of its ARN (`user/<path…><name>`,
//! `assumed-role/<role>/<session>`, `federated-user/<name>`:
//! `smithy_query(query="doc",shape="CreateWebLoginTokenResponse",member="IamIdentity")`
//! documents "the name of the IAM identity … For example,
//! `assumed-role/Admin/your-name`"), and `AirflowIdentity` is the Apache Airflow USER NAME
//! that identity maps to, its last path segment. Neither is a credential identifier, so
//! neither is ever the access key id the request presented, and the two differ in form.
//!
//! The identities are real: each row's caller is made with IAM's and STS's OWN operations
//! over the same engine — an IAM user and a long-term key of it, a session assumed on a
//! role, a federated user — so what this service reports is cross-checked against what
//! `sts:GetCallerIdentity` answers for the very same credential.
//!
//! The rows differ along what the rule generalises over: the KIND of identity (the
//! identity a key nothing holds resolves to, an IAM user, an assumed-role session, a
//! federated user), whether the identity's name carries a path, and the length of the name
//! (the one-character minimum and the 64-character maximum a role session name may carry).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The access key id the setup calls and the first row present: a key neither STS nor IAM
/// holds a record of, which is the position the long-term credential this emulator
/// publishes at start is in.
const UNHELD_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// A trust policy granting the account permission to assume a role, so the assumed-role
/// rows can be minted at all.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"sts:AssumeRole"}]}"#
);

/// The environment the token is minted for; one is enough, since the rule under test is
/// about the caller and not about the environment.
const ENVIRONMENT: &str = "TokenEnv";

/// `AirflowIdentity`'s modelled `@length(1, 64)`.
const AIRFLOW_IDENTITY_BOUNDS: std::ops::RangeInclusive<usize> = 1..=64;

/// One caller the rule is asserted for.
struct Row {
    case: &'static str,
    /// The access key id the `CreateWebLoginToken` call presents.
    access_key_id: String,
    /// The identity name the row expects: the resource field of the ARN the caller's
    /// identity carries.
    iam_identity: String,
    /// The Apache Airflow user name that identity maps to.
    airflow_identity: String,
}

/// The emulator these tests drive: one shared engine carrying this service beside the IAM
/// and STS services the callers are made with — the set `core/router`'s `build_router`
/// assembles, narrowed to what this rule touches.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Nothing here signs: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mwaa_amazonmwaa::EmulatedService::new(Arc::clone(&engine)),
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

    /// One restJson1 call to this service, signed (in the routing sense) with
    /// `access_key_id`.
    async fn mwaa(
        &self,
        access_key_id: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("a serialisable body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("api.airflow.{REGION}.amazonaws.com"))
            .header("authorization", authorization(access_key_id, "airflow"))
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let payload = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or(Value::Null)
        };
        (status, payload)
    }

    /// One awsQuery call to IAM or STS, signed with `access_key_id`.
    async fn query(
        &self,
        access_key_id: &str,
        signing_name: &str,
        version: &str,
        action: &str,
        parameters: &[(&str, String)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in parameters {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization(access_key_id, signing_name))
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// One awsQuery call that must succeed, with its body.
    async fn query_ok(
        &self,
        case: &str,
        access_key_id: &str,
        signing_name: &str,
        version: &str,
        action: &str,
        parameters: &[(&str, String)],
    ) -> String {
        let (status, body) = self
            .query(access_key_id, signing_name, version, action, parameters)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture {action} must succeed, got {status} with {body}"
        );
        body
    }

    /// The ARN `sts:GetCallerIdentity` answers for `access_key_id` — the identity this
    /// service's two members are asserted against.
    async fn caller_arn(&self, case: &str, access_key_id: &str) -> String {
        let body = self
            .query_ok(case, access_key_id, "sts", "2011-06-15", "GetCallerIdentity", &[])
            .await;
        xml_element(&body, "Arn")
            .unwrap_or_else(|| panic!("case {case:?}: GetCallerIdentity answered no Arn: {body}"))
    }

    /// An IAM user at `path` carrying a long-term access key, as a client makes one:
    /// answers the key id and the user's ARN.
    async fn iam_user(&self, case: &str, user_name: &str, path: &str) -> (String, String) {
        let created = self
            .query_ok(
                case,
                UNHELD_KEY,
                "iam",
                "2010-05-08",
                "CreateUser",
                &[
                    ("UserName", user_name.to_string()),
                    ("Path", path.to_string()),
                ],
            )
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("case {case:?}: CreateUser answered no Arn: {created}"));
        let minted = self
            .query_ok(
                case,
                UNHELD_KEY,
                "iam",
                "2010-05-08",
                "CreateAccessKey",
                &[("UserName", user_name.to_string())],
            )
            .await;
        let key = xml_element(&minted, "AccessKeyId").unwrap_or_else(|| {
            panic!("case {case:?}: CreateAccessKey answered no AccessKeyId: {minted}")
        });
        (key, arn)
    }

    /// A session assumed on a fresh role, as a client makes one: answers the session's
    /// access key id.
    async fn assumed_role_session(
        &self,
        case: &str,
        role_name: &str,
        session_name: &str,
    ) -> String {
        let created = self
            .query_ok(
                case,
                UNHELD_KEY,
                "iam",
                "2010-05-08",
                "CreateRole",
                &[
                    ("RoleName", role_name.to_string()),
                    ("AssumeRolePolicyDocument", TRUST_POLICY.to_string()),
                ],
            )
            .await;
        let role_arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("case {case:?}: CreateRole answered no Arn: {created}"));
        let assumed = self
            .query_ok(
                case,
                UNHELD_KEY,
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    ("RoleArn", role_arn),
                    ("RoleSessionName", session_name.to_string()),
                ],
            )
            .await;
        xml_element(&assumed, "AccessKeyId")
            .unwrap_or_else(|| panic!("case {case:?}: AssumeRole answered no AccessKeyId: {assumed}"))
    }

    /// A federated-user session, as a client makes one: answers its access key id.
    async fn federated_session(&self, case: &str, name: &str) -> String {
        let minted = self
            .query_ok(
                case,
                UNHELD_KEY,
                "sts",
                "2011-06-15",
                "GetFederationToken",
                &[("Name", name.to_string())],
            )
            .await;
        xml_element(&minted, "AccessKeyId").unwrap_or_else(|| {
            panic!("case {case:?}: GetFederationToken answered no AccessKeyId: {minted}")
        })
    }
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and
/// `access_key_id`. The signature is never checked: this harness runs with request
/// authentication off, and the access key id is what resolves the caller.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC
/// 3986's unreserved characters literal — so a trust policy's braces and quotes survive the
/// round trip.
///
/// A copy: an integration-test module cannot be imported across crates, so every service
/// suite that drives an awsQuery service holds its own.
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

/// The text of the first `<name>…</name>` element of an awsQuery response. A copy, for the
/// same reason as [`form_encode`].
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// A create body every member of which satisfies its modelled constraints.
fn valid_create_body() -> Value {
    json!({
        "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/mwaa-execution"),
        "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket",
        "DagS3Path": "dags",
        "NetworkConfiguration": {
            "SubnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
            "SecurityGroupIds": ["sg-00000000000000001"],
        },
    })
}

/// The string member `name` of a JSON answer.
fn member(case: &str, answer: &Value, name: &str) -> String {
    answer
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the answer carries no {name}: {answer}"))
        .to_string()
}

/// RULE: `CreateWebLoginToken` reports the caller's IAM identity NAME and the Apache
/// Airflow user name it maps to — never the access key id the request presented.
#[tokio::test]
async fn the_token_reports_the_calling_identity_and_the_airflow_user_it_maps_to() {
    let emulator = Emulator::new();
    let (status, created) = emulator
        .mwaa(
            UNHELD_KEY,
            "PUT",
            &format!("/environments/{ENVIRONMENT}"),
            Some(valid_create_body()),
        )
        .await;
    assert_eq!(
        status, 200,
        "the fixture CreateEnvironment must succeed, got {status} with {created}"
    );

    // The callers, each minted through the service that owns its credential class.
    let (analyst_key, _) = emulator
        .iam_user("an IAM user at the account root", "mwaa-analyst", "/")
        .await;
    let (engineer_key, _) = emulator
        .iam_user("an IAM user under a path", "mwaa-engineer", "/team/airflow/")
        .await;
    // `RoleSessionName` is `@length(2, 64)`: the rows below span a short session name and
    // one at that maximum, so `AirflowIdentity`'s own `@length(1, 64)` is exercised at its
    // upper bound by a name the session really carries.
    let long_session_name = format!("s{}", "x".repeat(63));
    let admin_session = emulator
        .assumed_role_session("a session of a role", "MwaaAdmin", "jane.doe")
        .await;
    let long_session = emulator
        .assumed_role_session(
            "a session whose name is at the modelled maximum",
            "MwaaOperator",
            &long_session_name,
        )
        .await;
    let federated = emulator
        .federated_session("a federated user", "Bob")
        .await;

    let rows = [
        Row {
            // Neither STS nor IAM holds this key, so it speaks for the identity the
            // emulator publishes at start: an IAM user of the account.
            case: "a key no session and no IAM record names",
            access_key_id: UNHELD_KEY.to_string(),
            iam_identity: "user/emulator".to_string(),
            airflow_identity: "emulator".to_string(),
        },
        Row {
            case: "an IAM user's own long-term access key",
            access_key_id: analyst_key,
            iam_identity: "user/mwaa-analyst".to_string(),
            airflow_identity: "mwaa-analyst".to_string(),
        },
        Row {
            // A user's path belongs to the identity's NAME and not to the Apache Airflow
            // user name, which is why the two members differ by more than a prefix here.
            case: "an IAM user whose name carries a path",
            access_key_id: engineer_key,
            iam_identity: "user/team/airflow/mwaa-engineer".to_string(),
            airflow_identity: "mwaa-engineer".to_string(),
        },
        Row {
            case: "a session assumed on a role",
            access_key_id: admin_session,
            iam_identity: "assumed-role/MwaaAdmin/jane.doe".to_string(),
            airflow_identity: "jane.doe".to_string(),
        },
        Row {
            case: "a session whose name is at the modelled maximum",
            access_key_id: long_session,
            iam_identity: format!("assumed-role/MwaaOperator/{long_session_name}"),
            airflow_identity: long_session_name.clone(),
        },
        Row {
            case: "a federated user",
            access_key_id: federated,
            iam_identity: "federated-user/Bob".to_string(),
            airflow_identity: "Bob".to_string(),
        },
    ];

    for row in rows {
        let Row {
            case,
            access_key_id,
            iam_identity,
            airflow_identity,
        } = row;

        let (status, answer) = emulator
            .mwaa(
                &access_key_id,
                "POST",
                &format!("/webtoken/{ENVIRONMENT}"),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateWebLoginToken must succeed, got {status} with {answer}"
        );

        let reported_iam = member(case, &answer, "IamIdentity");
        let reported_airflow = member(case, &answer, "AirflowIdentity");

        // The identity is the one STS reports for the very same credential, read down to
        // the NAME the member documents: an ARN's resource field.
        let caller_arn = emulator.caller_arn(case, &access_key_id).await;
        let expected_from_sts = caller_arn
            .splitn(6, ':')
            .nth(5)
            .expect("an ARN carries a resource field")
            .to_string();
        assert_eq!(
            reported_iam, expected_from_sts,
            "case {case:?}: expected the name of the identity STS reports ({caller_arn:?}), \
             got {reported_iam:?}"
        );
        assert_eq!(
            reported_iam, iam_identity,
            "case {case:?}: expected the identity name {iam_identity:?}, got {reported_iam:?}"
        );
        assert_ne!(
            reported_iam, access_key_id,
            "case {case:?}: the access key id is a credential identifier, never an identity"
        );
        assert!(
            !reported_iam.starts_with("arn:"),
            "case {case:?}: IamIdentity is the identity's name, not its ARN, got \
             {reported_iam:?}"
        );

        assert_eq!(
            reported_airflow, airflow_identity,
            "case {case:?}: expected the Apache Airflow user name {airflow_identity:?}, got \
             {reported_airflow:?}"
        );
        assert_ne!(
            reported_airflow, access_key_id,
            "case {case:?}: the access key id is a credential identifier, never a user name"
        );
        assert!(
            AIRFLOW_IDENTITY_BOUNDS.contains(&reported_airflow.chars().count()),
            "case {case:?}: AirflowIdentity is @length(1, 64), got {reported_airflow:?}"
        );
        assert!(
            !reported_airflow.contains('/'),
            "case {case:?}: an Apache Airflow user name carries no path, got \
             {reported_airflow:?}"
        );
        // The two members differ in form: the identity NAME states which kind of identity
        // it is and where it sits, the Airflow user name is the name alone.
        assert_ne!(
            reported_iam, reported_airflow,
            "case {case:?}: the identity name and the Apache Airflow user name differ in form"
        );
        assert!(
            reported_iam.ends_with(&reported_airflow),
            "case {case:?}: expected the Airflow user name to be the identity's last \
             segment, got {reported_iam:?} and {reported_airflow:?}"
        );

        // The token itself is still minted for the environment that holds it, per caller.
        assert!(
            !member(case, &answer, "WebToken").is_empty(),
            "case {case:?}: CreateWebLoginToken must report a token"
        );
    }
}

/// RULE (the same rule, read from the other side): two DIFFERENT callers of one
/// environment are reported as two different identities, so the members are computed from
/// the request's credential rather than from the environment or a fixed value.
#[tokio::test]
async fn two_callers_of_one_environment_are_reported_as_two_identities() {
    let emulator = Emulator::new();
    let (status, created) = emulator
        .mwaa(
            UNHELD_KEY,
            "PUT",
            &format!("/environments/{ENVIRONMENT}"),
            Some(valid_create_body()),
        )
        .await;
    assert_eq!(
        status, 200,
        "the fixture CreateEnvironment must succeed, got {status} with {created}"
    );

    let (first_key, _) = emulator.iam_user("the first caller", "pilot", "/").await;
    let (second_key, _) = emulator.iam_user("the second caller", "copilot", "/").await;

    let mut seen = Vec::new();
    for (case, key) in [("the first caller", first_key), ("the second caller", second_key)] {
        let (status, answer) = emulator
            .mwaa(&key, "POST", &format!("/webtoken/{ENVIRONMENT}"), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateWebLoginToken must succeed, got {status} with {answer}"
        );
        seen.push((
            member(case, &answer, "IamIdentity"),
            member(case, &answer, "AirflowIdentity"),
        ));
    }

    assert_ne!(
        seen[0], seen[1],
        "expected two callers of one environment to be reported as two identities, got \
         {seen:?}"
    );
}
