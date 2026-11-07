// SPDX-License-Identifier: CC0-1.0

//! Server-side authorization, end to end: both EC2 Instance Connect operations decide on
//! every call whether the principal the request's credential speaks for may push the key it
//! asks to push.
//!
//! The rule under test: A SIGNED CALL IS DECIDED AGAINST THE CALLING PRINCIPAL'S IAM
//! IDENTITY-BASED POLICIES BEFORE THE OPERATION RESOLVES ANYTHING — the call proceeds only
//! when a statement in force allows `ec2-instance-connect:<OperationName>` on the ARN of the
//! EC2 instance it names, an explicit `Deny` about the request refuses it whatever any
//! `Allow` says, and a call no statement allows is refused as well. A refusal carries the
//! error code `AccessDeniedException` with HTTP 403 and AWS's own sentence; an allowed call
//! falls through to the instance, so a well-formed id nothing holds comes back as
//! `EC2InstanceNotFoundException` rather than as an authorization failure.
//!
//! Nothing here is seeded into IAM's store by hand: every user is created through IAM's own
//! `CreateUser`, every policy put through `PutUserPolicy` and removed through
//! `DeleteUserPolicy`, and every credential minted through `CreateAccessKey` — so the calls
//! below carry an access key id a client would really hold, in their SigV4 credential scope.
//!
//! The tables span what the rule generalises over: the operation and the action it is
//! performed under, the action and resource patterns a statement names (exact, prefix
//! wildcard, whole wildcard, another instance), the effect and the order the statements are
//! written in, the condition keys a push publishes (the instance's tags under both spellings
//! AWS gives them, and the OS user the key is pushed for), a policy put and then deleted, and
//! the two principals the decision cannot refuse.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_ec2instanceconnect_awsec2instanceconnectservice as ec2ic;
use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;

use ec2ic::state::{Instance, InstanceState, SERIAL_CONSOLE_PORT};

use super::instance_connect_key_push::{error_code, ssh_key, SEND_SERIAL, SEND_SSH};

/// The Region every call below is signed for; the instance ARNs the policies name are taken
/// in it, because the decision is made about the ARN of the Region the request was signed
/// for.
const REGION: &str = "us-east-1";

/// An access key id that names no IAM user and no STS session, which is what the account's
/// own published long-term credential looks like to a service: it resolves to the ambient
/// identity, the account itself, which holds no IAM policies.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The IAM API version an awsQuery body carries.
const IAM_API_VERSION: &str = "2010-05-08";

/// The EC2 API version an `ec2Query` body carries.
const EC2_API_VERSION: &str = "2016-11-15";

/// The name pattern a client resolving an Amazon Linux 2 x86_64 gp2 AMI matches on — the
/// published name form of that variant — which is how a launch picks the AMI to launch.
const AL2_NAME_PATTERN: &str = "amzn2-ami-hvm-*-x86_64-gp2";

/// The OS user every `SendSSHPublicKey` below pushes for, and the value the `ec2:osuser`
/// clauses are decided against.
const OS_USER: &str = "ec2-user";

/// The instance every table row pushes at: registered with this service, running, Nitro (so
/// the serial-console push is not refused for the instance type), and tagged, so a clause over
/// a resource tag can be decided.
const INSTANCE_ID: &str = "i-0a1b2c3d4e5f60718";

/// The zone the registered instance sits in.
const ZONE: &str = "us-east-1a";

/// A second well-formed instance id, registered nowhere: what a statement scoped to another
/// instance names, and the id the recorded flow pushes at.
const ABSENT_INSTANCE_ID: &str = "i-0fedcba9876543210";

/// The tag key and value the registered instance carries, which the `ResourceTag` clauses are
/// decided against.
const TAG_KEY: &str = "Environment";
const TAG_VALUE: &str = "staging";

/// One emulator: a shared engine carrying EC2 Instance Connect, the IAM service the users and
/// their policies are made with, and the STS service a credential is resolved through — the
/// same set `core/router`'s `build_router` assembles, narrowed to what this rule touches.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    /// The emulator the tables run on. EC2 is NOT mounted, so no service can answer for an
    /// instance and the push reads this service's own fallback registry — which is what lets a
    /// row choose the instance's tags without launching one.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Nothing here signs: the `Authorization` header carries only the credential scope,
        // which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(ec2ic::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(iam::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(sts::EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// The same emulator with Amazon EC2 mounted beside the rest: the owner of an instance,
    /// so a push is decided about the instance EC2 really holds and the tags EC2 reports for
    /// it.
    fn with_ec2() -> Self {
        let mut emulator = Self::new();
        emulator
            .router
            .register_service(ec2::EmulatedService::new(Arc::clone(&emulator.engine)));
        emulator
    }

    /// This service's store in the shared engine — reached exactly as a handler reaches it.
    fn state(
        &self,
    ) -> engine::ServiceState<
        ec2ic::state::ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState,
    > {
        ec2ic::state::service_state(&*self.engine).expect("the service state must be reachable")
    }

    /// One EC2 Instance Connect call, with `access_key_id` in the SigV4 credential scope.
    async fn connect(&self, access_key_id: &str, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("ec2-instance-connect.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", target)
            .header("authorization", authorization(access_key_id, "ec2-instance-connect"))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// `SendSSHPublicKey` at `instance_id`, signed with `access_key_id`.
    async fn push_ssh(
        &self,
        access_key_id: &str,
        instance_id: &str,
        os_user: &str,
        key: &str,
    ) -> (u16, Value) {
        self.connect(
            access_key_id,
            SEND_SSH,
            &json!({
                "InstanceId": instance_id,
                "InstanceOSUser": os_user,
                "SSHPublicKey": key,
            }),
        )
        .await
    }

    /// `SendSerialConsoleSSHPublicKey` at `instance_id`, signed with `access_key_id`.
    async fn push_serial(
        &self,
        access_key_id: &str,
        instance_id: &str,
        key: &str,
    ) -> (u16, Value) {
        self.connect(
            access_key_id,
            SEND_SERIAL,
            &json!({
                "InstanceId": instance_id,
                "SerialPort": SERIAL_CONSOLE_PORT,
                "SSHPublicKey": key,
            }),
        )
        .await
    }

    /// One awsQuery call to another service mounted here: `signing_name` picks it, `version`
    /// its API version.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, &str)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_value(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization(ACCOUNT_KEY, signing_name))
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// One IAM call, made with the account's own credential, which must succeed.
    async fn iam(&self, case: &str, action: &str, params: &[(&str, &str)]) -> String {
        let (status, body) = self.query("iam", IAM_API_VERSION, action, params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: IAM {action} must succeed, got {status} {body}"
        );
        body
    }

    /// One Amazon EC2 call, made with the account's own credential, which must succeed.
    async fn ec2(&self, case: &str, action: &str, params: &[(&str, &str)]) -> String {
        let (status, body) = self.query("ec2", EC2_API_VERSION, action, params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: EC2 {action} must succeed, got {status} {body}"
        );
        body
    }

    /// Create the IAM user `user_name`, give it `policy` as an inline policy when one is
    /// supplied, mint a long-term access key for it, and answer that key's id — the
    /// credential the caller then signs its key pushes with.
    async fn iam_user_key(&self, case: &str, user_name: &str, policy: Option<&str>) -> String {
        self.iam(case, "CreateUser", &[("UserName", user_name)]).await;
        if let Some(policy) = policy {
            self.put_user_policy(case, user_name, policy).await;
        }
        let minted = self
            .iam(case, "CreateAccessKey", &[("UserName", user_name)])
            .await;
        xml_element(&minted, "AccessKeyId").unwrap_or_else(|| {
            panic!("case {case:?}: CreateAccessKey must answer an AccessKeyId, got {minted}")
        })
    }

    /// Put (or replace) the one inline policy of `user_name`.
    async fn put_user_policy(&self, case: &str, user_name: &str, policy: &str) {
        self.iam(
            case,
            "PutUserPolicy",
            &[
                ("UserName", user_name),
                ("PolicyName", INLINE_POLICY_NAME),
                ("PolicyDocument", policy),
            ],
        )
        .await;
    }

    /// Delete that inline policy again, which is what makes the grant (or the deny) stop.
    async fn delete_user_policy(&self, case: &str, user_name: &str) {
        self.iam(
            case,
            "DeleteUserPolicy",
            &[("UserName", user_name), ("PolicyName", INLINE_POLICY_NAME)],
        )
        .await;
    }

    /// Register the running, Nitro, tagged instance every table row pushes at, and turn the
    /// account's serial-console access on so a serial-console row is not refused for the
    /// account.
    fn register_the_instance(&self) {
        let state = self.state();
        let mut guard = state.lock();
        guard.register_instance(
            Instance::new(INSTANCE_ID, ZONE, "m5.large", InstanceState::Running, true)
                .with_tags([(TAG_KEY, TAG_VALUE)]),
        );
        guard.serial_console_access_enabled = true;
    }
}

/// The name of the one inline policy each user below carries.
const INLINE_POLICY_NAME: &str = "probe";

/// A SigV4 credential scope naming `signing_name` and `access_key_id`, which is how the router
/// picks the service out and how a service reads the caller's key id. The harness switches
/// request authentication off, so the signature itself is never verified.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
    )
}

/// Percent-encode one `application/x-www-form-urlencoded` component: every byte outside RFC
/// 3986's unreserved set is escaped, which is what an SDK does to a policy document's braces,
/// quotes and colons.
fn form_value(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let rest = xml.get(start..)?;
    let end = rest.find(&close)?;
    rest.get(..end).map(str::to_string)
}

/// The `message` of an error body, or the empty string for a success.
fn message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The ARN of an EC2 instance in the Region every call is signed for — the resource a
/// statement has to match, built the way EC2 mints it.
fn instance_arn(instance_id: &str) -> String {
    format!(
        "arn:aws:ec2:{REGION}:{}:instance/{instance_id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The IAM user ARN of `user_name` at the default path — the principal a refusal names.
fn user_arn(user_name: &str) -> String {
    format!("arn:aws:iam::{}:user/{user_name}", engine::EMULATOR_ACCOUNT_ID)
}

/// An identity policy document carrying `statements` verbatim.
fn policy(statements: &str) -> String {
    format!(r#"{{"Version":"2012-10-17","Statement":[{statements}]}}"#)
}

/// One `Effect`/`Action`/`Resource` statement, with an optional `Condition` clause.
fn statement(effect: &str, action: &str, resource: &str, condition: Option<&str>) -> String {
    let condition = match condition {
        Some(clause) => format!(r#","Condition":{clause}"#),
        None => String::new(),
    };
    format!(
        r#"{{"Effect":"{effect}","Action":"{action}","Resource":"{resource}"{condition}}}"#
    )
}

/// A `StringEquals` clause over one condition key.
fn string_equals(key: &str, value: &str) -> String {
    format!(r#"{{"StringEquals":{{"{key}":"{value}"}}}}"#)
}

/// What the decision must come to for one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The call is allowed and proceeds to the instance.
    Allowed,
    /// No statement in force allows the action.
    NoAllow,
    /// A statement in force denies it outright.
    ExplicitDeny,
}

impl Expected {
    /// The reason AWS closes the refusal sentence with, for a refused row.
    fn reason(self, action: &str) -> Option<String> {
        match self {
            Self::Allowed => None,
            Self::NoAllow => Some(format!(
                "because no identity-based policy allows the {action} action"
            )),
            Self::ExplicitDeny => {
                Some("with an explicit deny in an identity-based policy".to_string())
            }
        }
    }
}

/// Assert that one answer is the refusal `expected` describes, or an allowed call.
fn assert_decision(
    case: &str,
    user_name: &str,
    action: &str,
    instance_id: &str,
    expected: Expected,
    status: u16,
    body: &Value,
) {
    match expected.reason(action) {
        None => assert_ne!(
            (status, error_code(body).as_str()),
            (403, "AccessDeniedException"),
            "case {case:?}: an allowed call must not be refused by authorization, got {status} \
             {body}"
        ),
        Some(reason) => {
            assert_eq!(
                (status, error_code(body).as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: expected HTTP 403 AccessDeniedException, got {status} {body}"
            );
            assert_eq!(
                message(body),
                format!(
                    "User: {} is not authorized to perform: {action} on resource: {} {reason}",
                    user_arn(user_name),
                    instance_arn(instance_id)
                ),
                "case {case:?}: expected AWS's refusal sentence, got {body}"
            );
        }
    }
}

/// One row of the identity-policy table.
struct Row {
    case: &'static str,
    /// The statements of the caller's one inline policy, or `None` for a user carrying none.
    statements: Option<String>,
    /// The instance the push names.
    instance_id: &'static str,
    expected: Expected,
}

/// RULE: a key push is allowed only where a statement of the caller's identity-based policies
/// allows this service's action on the ARN of the instance it names, and an explicit `Deny`
/// about the request refuses it whatever an `Allow` says.
///
/// The rows span the action patterns (exact, this service's prefix, the whole wildcard, the
/// other operation's action), the resource patterns (the instance's own ARN, another
/// instance's, the wildcard), the effects and their order, and the condition keys a push
/// publishes.
#[tokio::test]
async fn a_push_is_allowed_only_where_an_identity_policy_allows_the_action_on_the_instance() {
    let send = "ec2-instance-connect:SendSSHPublicKey";
    let serial = "ec2-instance-connect:SendSerialConsoleSSHPublicKey";
    let allow_everything = statement("Allow", send, "*", None);
    let rows = vec![
        Row {
            case: "a principal carrying no policy at all",
            statements: None,
            instance_id: INSTANCE_ID,
            expected: Expected::NoAllow,
        },
        Row {
            case: "the action allowed on every resource",
            statements: Some(allow_everything.clone()),
            instance_id: INSTANCE_ID,
            expected: Expected::Allowed,
        },
        Row {
            case: "this service's whole action prefix",
            statements: Some(statement("Allow", "ec2-instance-connect:*", "*", None)),
            instance_id: INSTANCE_ID,
            expected: Expected::Allowed,
        },
        Row {
            case: "every action of every service",
            statements: Some(statement("Allow", "*", "*", None)),
            instance_id: INSTANCE_ID,
            expected: Expected::Allowed,
        },
        Row {
            case: "the action allowed on this instance's own ARN",
            statements: Some(statement("Allow", send, &instance_arn(INSTANCE_ID), None)),
            instance_id: INSTANCE_ID,
            expected: Expected::Allowed,
        },
        Row {
            case: "the action allowed on another instance's ARN",
            statements: Some(statement(
                "Allow",
                send,
                &instance_arn(ABSENT_INSTANCE_ID),
                None,
            )),
            instance_id: INSTANCE_ID,
            expected: Expected::NoAllow,
        },
        Row {
            case: "only the serial-console action allowed",
            statements: Some(statement("Allow", serial, "*", None)),
            instance_id: INSTANCE_ID,
            expected: Expected::NoAllow,
        },
        Row {
            case: "an allow and a deny, the deny written second",
            statements: Some(format!(
                "{allow_everything},{}",
                statement("Deny", send, "*", None)
            )),
            instance_id: INSTANCE_ID,
            expected: Expected::ExplicitDeny,
        },
        Row {
            case: "an allow and a deny, the deny written first",
            statements: Some(format!(
                "{},{allow_everything}",
                statement("Deny", send, "*", None)
            )),
            instance_id: INSTANCE_ID,
            expected: Expected::ExplicitDeny,
        },
        Row {
            case: "a deny scoped to this instance alone",
            statements: Some(format!(
                "{allow_everything},{}",
                statement("Deny", send, &instance_arn(INSTANCE_ID), None)
            )),
            instance_id: INSTANCE_ID,
            expected: Expected::ExplicitDeny,
        },
        Row {
            case: "a deny scoped to another instance",
            statements: Some(format!(
                "{allow_everything},{}",
                statement("Deny", send, &instance_arn(ABSENT_INSTANCE_ID), None)
            )),
            instance_id: INSTANCE_ID,
            expected: Expected::Allowed,
        },
        Row {
            case: "a global resource-tag clause the instance's tags satisfy",
            statements: Some(statement(
                "Allow",
                send,
                "*",
                Some(&string_equals(
                    &format!("aws:ResourceTag/{TAG_KEY}"),
                    TAG_VALUE,
                )),
            )),
            instance_id: INSTANCE_ID,
            expected: Expected::Allowed,
        },
        Row {
            case: "a global resource-tag clause the instance's tags do not satisfy",
            statements: Some(statement(
                "Allow",
                send,
                "*",
                Some(&string_equals(
                    &format!("aws:ResourceTag/{TAG_KEY}"),
                    "production",
                )),
            )),
            instance_id: INSTANCE_ID,
            expected: Expected::NoAllow,
        },
        Row {
            case: "the same clause in EC2's own key namespace",
            statements: Some(statement(
                "Allow",
                send,
                "*",
                Some(&string_equals(
                    &format!("ec2:ResourceTag/{TAG_KEY}"),
                    TAG_VALUE,
                )),
            )),
            instance_id: INSTANCE_ID,
            expected: Expected::Allowed,
        },
        Row {
            case: "a resource-tag clause over a tag key the instance does not carry",
            statements: Some(statement(
                "Allow",
                send,
                "*",
                Some(&string_equals("aws:ResourceTag/Owner", TAG_VALUE)),
            )),
            instance_id: INSTANCE_ID,
            expected: Expected::NoAllow,
        },
        Row {
            case: "an OS-user clause the push satisfies",
            statements: Some(statement(
                "Allow",
                send,
                "*",
                Some(&string_equals("ec2:osuser", OS_USER)),
            )),
            instance_id: INSTANCE_ID,
            expected: Expected::Allowed,
        },
        Row {
            case: "an OS-user clause naming another user",
            statements: Some(statement(
                "Allow",
                send,
                "*",
                Some(&string_equals("ec2:osuser", "root")),
            )),
            instance_id: INSTANCE_ID,
            expected: Expected::NoAllow,
        },
        Row {
            case: "an allow for every resource, pushing at an instance nothing holds",
            statements: Some(allow_everything.clone()),
            instance_id: ABSENT_INSTANCE_ID,
            expected: Expected::Allowed,
        },
        Row {
            case: "a deny, pushing at an instance nothing holds",
            statements: Some(format!(
                "{allow_everything},{}",
                statement("Deny", send, "*", None)
            )),
            instance_id: ABSENT_INSTANCE_ID,
            expected: Expected::ExplicitDeny,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        emulator.register_the_instance();
        let user_name = format!("probe-{index}");
        let document = row.statements.as_ref().map(|statements| policy(statements));
        let key_id = emulator
            .iam_user_key(row.case, &user_name, document.as_deref())
            .await;

        let pushed = ssh_key(&user_name);
        let (status, body) = emulator
            .push_ssh(&key_id, row.instance_id, OS_USER, &pushed)
            .await;
        assert_decision(
            row.case,
            &user_name,
            "ec2-instance-connect:SendSSHPublicKey",
            row.instance_id,
            row.expected,
            status,
            &body,
        );

        // An allowed call falls through to the instance, which is the only thing that can
        // decide it from here: the registered instance takes the key, and an id nothing holds
        // is reported as not found. A refused call stores nothing at all.
        let state = emulator.state();
        let stored = state
            .lock()
            .active_ssh_key(row.instance_id, OS_USER, Instant::now())
            .map(|key| key.public_key.clone());
        match (row.expected, row.instance_id) {
            (Expected::Allowed, ABSENT_INSTANCE_ID) => {
                assert_eq!(
                    error_code(&body),
                    "EC2InstanceNotFoundException",
                    "case {:?}: an allowed push at an instance nothing holds must be reported \
                     as not found, got {status} {body}",
                    row.case
                );
                assert_eq!(
                    stored, None,
                    "case {:?}: nothing may be stored for an instance nothing holds",
                    row.case
                );
            }
            (Expected::Allowed, _) => {
                assert_eq!(
                    status, 200,
                    "case {:?}: an allowed push at the registered instance must succeed, got \
                     {status} {body}",
                    row.case
                );
                assert_eq!(
                    stored.as_deref(),
                    Some(pushed.as_str()),
                    "case {:?}: the allowed push must store the key it pushed",
                    row.case
                );
            }
            _ => assert_eq!(
                stored, None,
                "case {:?}: a refused push must store no key",
                row.case
            ),
        }
    }
}

/// RULE: the decision is made from the documents in force AT THE INSTANT OF THE CALL, so a
/// policy just put decides immediately and one just deleted stops deciding immediately — and
/// the authorization decision precedes the instance lookup, so the same call answers
/// `AccessDeniedException` while a deny is in force and `EC2InstanceNotFoundException` once it
/// is gone.
#[tokio::test]
async fn a_policy_put_and_then_deleted_decides_the_same_call_differently() {
    let case = "a deny put and then deleted";
    let emulator = Emulator::new();
    let user_name = "least-privilege";
    let send = "ec2-instance-connect:SendSSHPublicKey";
    let allow = statement("Allow", send, "*", None);
    let key_id = emulator
        .iam_user_key(case, user_name, Some(&policy(&allow)))
        .await;
    let pushed = ssh_key("propagation");

    // With the Allow alone the call is authorized and falls through to the instance, which
    // nothing holds.
    let (status, body) = emulator
        .push_ssh(&key_id, ABSENT_INSTANCE_ID, OS_USER, &pushed)
        .await;
    assert_eq!(
        error_code(&body),
        "EC2InstanceNotFoundException",
        "case {case:?}: with the allow in force the call must reach the instance lookup, got \
         {status} {body}"
    );

    // An explicit deny on the same action refuses the identical call.
    let denied = policy(&format!("{allow},{}", statement("Deny", send, "*", None)));
    emulator.put_user_policy(case, user_name, &denied).await;
    let (status, body) = emulator
        .push_ssh(&key_id, ABSENT_INSTANCE_ID, OS_USER, &pushed)
        .await;
    assert_decision(
        case,
        user_name,
        send,
        ABSENT_INSTANCE_ID,
        Expected::ExplicitDeny,
        status,
        &body,
    );

    // Deleting the document removes BOTH statements, so the same call is refused for carrying
    // no grant rather than for the deny.
    emulator.delete_user_policy(case, user_name).await;
    let (status, body) = emulator
        .push_ssh(&key_id, ABSENT_INSTANCE_ID, OS_USER, &pushed)
        .await;
    assert_decision(
        case,
        user_name,
        send,
        ABSENT_INSTANCE_ID,
        Expected::NoAllow,
        status,
        &body,
    );

    // Putting the allow back restores the instance lookup: the grant is read at the instant of
    // the call, not cached from the first one.
    emulator.put_user_policy(case, user_name, &policy(&allow)).await;
    let (status, body) = emulator
        .push_ssh(&key_id, ABSENT_INSTANCE_ID, OS_USER, &pushed)
        .await;
    assert_eq!(
        error_code(&body),
        "EC2InstanceNotFoundException",
        "case {case:?}: with the allow back in force the call must reach the instance lookup \
         again, got {status} {body}"
    );
}

/// RULE: each operation is performed under ITS OWN action, so a policy granting one does not
/// grant the other.
#[tokio::test]
async fn each_operation_is_authorized_under_its_own_action() {
    let send = "ec2-instance-connect:SendSSHPublicKey";
    let serial = "ec2-instance-connect:SendSerialConsoleSSHPublicKey";
    // Each row: which action the caller holds, and what each of the two pushes must come to.
    let rows = [
        ("only the key push", send, Expected::Allowed, Expected::NoAllow),
        (
            "only the serial-console push",
            serial,
            Expected::NoAllow,
            Expected::Allowed,
        ),
    ];

    for (index, (case, granted, ssh_expected, serial_expected)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        emulator.register_the_instance();
        let user_name = format!("single-action-{index}");
        let key_id = emulator
            .iam_user_key(
                case,
                &user_name,
                Some(&policy(&statement("Allow", granted, "*", None))),
            )
            .await;

        let pushed = ssh_key(case);
        let (status, body) = emulator
            .push_ssh(&key_id, INSTANCE_ID, OS_USER, &pushed)
            .await;
        assert_decision(
            case,
            &user_name,
            send,
            INSTANCE_ID,
            ssh_expected,
            status,
            &body,
        );

        let (status, body) = emulator.push_serial(&key_id, INSTANCE_ID, &pushed).await;
        assert_decision(
            case,
            &user_name,
            serial,
            INSTANCE_ID,
            serial_expected,
            status,
            &body,
        );

        // Whichever of the two the policy allowed is the one whose key is stored.
        let state = emulator.state();
        let guard = state.lock();
        let now = Instant::now();
        let ssh_stored = guard.active_ssh_key(INSTANCE_ID, OS_USER, now).is_some();
        let serial_stored = guard
            .active_serial_console_key(INSTANCE_ID, SERIAL_CONSOLE_PORT, now)
            .is_some();
        assert_eq!(
            (ssh_stored, serial_stored),
            (
                ssh_expected == Expected::Allowed,
                serial_expected == Expected::Allowed
            ),
            "case {case:?}: only the push the policy allowed may have stored a key, got \
             (ssh {ssh_stored}, serial {serial_stored})"
        );
    }
}

/// RULE: a credential that names no IAM user is the account's own published credential, which
/// holds no IAM policies and is served as the account — the position AWS account root
/// credentials are in — so the gate refuses neither it nor a call made with no credential
/// scope at all.
#[tokio::test]
async fn the_accounts_own_credential_is_not_refused() {
    let emulator = Emulator::new();
    emulator.register_the_instance();
    for (case, key_id) in [
        ("the account's published key", ACCOUNT_KEY),
        ("a key no IAM user holds", "AKIAIOSFODNN7EXAMPLE"),
    ] {
        let pushed = ssh_key(case);
        let (status, body) = emulator
            .push_ssh(key_id, INSTANCE_ID, OS_USER, &pushed)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the account's own credential must be served, got {status} {body}"
        );
        let state = emulator.state();
        let stored = state
            .lock()
            .active_ssh_key(INSTANCE_ID, OS_USER, Instant::now())
            .map(|key| key.public_key.clone());
        assert_eq!(
            stored.as_deref(),
            Some(pushed.as_str()),
            "case {case:?}: the served push must store the key it pushed"
        );
    }
}

/// RULE: the tags a resource-tag clause is decided against are the TARGET'S OWN, as the
/// service that owns the instance reports them — so an instance EC2 launched and tagged
/// through its own `CreateTags` is what the same unchanged policy is decided about, and this
/// service's own registry has no say where EC2 can be asked.
#[tokio::test]
async fn a_resource_tag_clause_is_decided_from_the_tags_ec2_reports() {
    let launched = "an instance EC2 launched and tagged";
    let emulator = Emulator::with_ec2();
    let send = "ec2-instance-connect:SendSSHPublicKey";

    // Resolve the published Amazon Linux 2 AMI the documented way and launch one instance of
    // it, so the push is decided about the instance EC2 holds.
    let images = emulator
        .ec2(
            launched,
            "DescribeImages",
            &[
                ("Owner.1", "amazon"),
                ("Filter.1.Name", "name"),
                ("Filter.1.Value.1", AL2_NAME_PATTERN),
                ("Filter.2.Name", "state"),
                ("Filter.2.Value.1", "available"),
            ],
        )
        .await;
    let image_id = xml_element(&images, "imageId").unwrap_or_else(|| {
        panic!(
            "case {launched:?}: the published catalog must report an available Amazon Linux 2 \
             AMI for {AL2_NAME_PATTERN}, got {images}"
        )
    });
    let run = emulator
        .ec2(
            launched,
            "RunInstances",
            &[
                ("ImageId", &image_id),
                ("InstanceType", "m5.large"),
                ("MinCount", "1"),
                ("MaxCount", "1"),
            ],
        )
        .await;
    let instance_id = xml_element(&run, "instanceId")
        .unwrap_or_else(|| panic!("case {launched:?}: RunInstances must mint an instance: {run}"));

    // EC2 reports a launched instance `pending` and owes it `running` one transition span
    // later; a push is refused for the state until then, which is not what this rule is about.
    tokio::time::sleep(ec2::state::TRANSITION_DELAY + Duration::from_millis(400)).await;
    emulator
        .ec2(
            launched,
            "CreateTags",
            &[
                ("ResourceId.1", &instance_id),
                ("Tag.1.Key", TAG_KEY),
                ("Tag.1.Value", TAG_VALUE),
            ],
        )
        .await;

    // Each row: the value the clause compares the instance's tag against, and what the push
    // must come to.
    let rows = [
        ("the tag value EC2 reports", TAG_VALUE, Expected::Allowed),
        ("another tag value", "production", Expected::NoAllow),
    ];
    for (index, (case, compared, expected)) in rows.into_iter().enumerate() {
        let user_name = format!("tagged-{index}");
        let document = policy(&statement(
            "Allow",
            send,
            "*",
            Some(&string_equals(
                &format!("aws:ResourceTag/{TAG_KEY}"),
                compared,
            )),
        ));
        let key_id = emulator
            .iam_user_key(case, &user_name, Some(&document))
            .await;
        let pushed = ssh_key(case);
        let (status, body) = emulator
            .push_ssh(&key_id, &instance_id, OS_USER, &pushed)
            .await;
        assert_decision(
            case,
            &user_name,
            send,
            &instance_id,
            expected,
            status,
            &body,
        );
        if expected == Expected::Allowed {
            assert_eq!(
                status, 200,
                "case {case:?}: the allowed push at the instance EC2 holds must succeed, got \
                 {status} {body}"
            );
            let state = emulator.state();
            let stored = state
                .lock()
                .active_ssh_key(&instance_id, OS_USER, Instant::now())
                .map(|key| key.public_key.clone());
            assert_eq!(
                stored.as_deref(),
                Some(pushed.as_str()),
                "case {case:?}: the allowed push must store the key it pushed"
            );
        }
    }
}
