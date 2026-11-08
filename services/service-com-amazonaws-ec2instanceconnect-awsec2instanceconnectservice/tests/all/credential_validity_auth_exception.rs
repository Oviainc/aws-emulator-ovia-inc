// SPDX-License-Identifier: CC0-1.0

//! What a key push answers when the credential it was signed with is not a valid one.
//!
//! The rule under test: A REQUEST WHOSE CREDENTIAL THE IDENTITY STORE DOES NOT HOLD AS VALID
//! SPEAKS FOR NO PRINCIPAL, so no identity-based policy is consulted and the call is refused
//! with the error this service's model declares for a credential that is not valid —
//! `AuthException`, carrying the sentence the model documents for it
//! (`smithy_query(query="doc",shape="AuthException")`: "Either your AWS credentials are not
//! valid or you do not have access to the EC2 instance.") — and never with the policy
//! refusal, whose `AccessDeniedException` sentence names a principal, an action, a resource
//! and the document that withheld the grant. A credential the store DOES hold as valid is
//! decided by that principal's policies exactly as before, which is the second table here.
//!
//! The invalid credential this emulator can hold is a session whose validity window has run
//! out: STS's own store answers `resolve_caller` with the expired record while it keeps it
//! (`EXPIRED_SESSION_RETENTION`).
//!
//! Nothing is poked into IAM's store by hand: every user, role, policy and long-term key is
//! made through IAM's own API, and every session credential is minted through STS's own
//! `GetSessionToken` or `AssumeRole`. The one thing a test cannot wait for is the clock, so a
//! row that needs an expired credential moves that session's expiry into the past in STS's
//! store — the record stays exactly what STS minted.
//!
//! The rows span what the rule generalises over: the operation (both of them), the kind of
//! session credential (a user session token and an assumed-role session), what the principal's
//! policies say (everything allowed, nothing allowed), and whether the instance the push names
//! exists at all.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ec2instanceconnect_awsec2instanceconnectservice as ec2ic;
use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;

use ec2ic::state::{Instance, InstanceState, SERIAL_CONSOLE_PORT};

use super::instance_connect_key_push::{error_code, ssh_key, SEND_SERIAL, SEND_SSH};

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// An access key id that names no IAM user and no STS session: the account's own published
/// credential, which the administrative calls below (IAM and STS) are made with.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The IAM API version an awsQuery body carries.
const IAM_API_VERSION: &str = "2010-05-08";

/// The STS API version an awsQuery body carries.
const STS_API_VERSION: &str = "2011-06-15";

/// The IAM user whose long-term key mints the user session tokens below.
const PROBE_USER: &str = "probe-user";

/// The IAM role the assumed-role sessions below are sessions of.
const PROBE_ROLE: &str = "probe-role";

/// The session name every `AssumeRole` below asks for.
const PROBE_SESSION: &str = "probe-session";

/// The name of the one inline policy a principal below carries.
const INLINE_POLICY_NAME: &str = "probe";

/// The OS user every `SendSSHPublicKey` below pushes for.
const OS_USER: &str = "ec2-user";

/// The instance every row pushes at: registered with this service, running and Nitro, so
/// neither operation is refused for the instance itself.
const INSTANCE_ID: &str = "i-0a1b2c3d4e5f60718";

/// The zone that instance sits in.
const ZONE: &str = "us-east-1a";

/// A second well-formed instance id, registered nowhere: the id a row uses to show that the
/// credential is decided BEFORE the instance is looked up.
const ABSENT_INSTANCE_ID: &str = "i-0fedcba9876543210";

/// The message `AuthException` carries, as this service's model documents the shape
/// (`smithy_query(query="doc",shape="AuthException")`).
const AUTH_EXCEPTION_MESSAGE: &str =
    "Either your AWS credentials are not valid or you do not have access to the EC2 instance.";

/// One emulator: a shared engine carrying EC2 Instance Connect, the IAM service the users,
/// roles and policies are made with, and the STS service the session credentials are minted
/// by — the same set `core/router`'s `build_router` assembles, narrowed to what this rule
/// touches. EC2 is not mounted, so the push reads this service's own fallback registry.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
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

    /// This service's store in the shared engine — reached exactly as a handler reaches it.
    fn state(
        &self,
    ) -> engine::ServiceState<
        ec2ic::state::ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState,
    > {
        ec2ic::state::service_state(&*self.engine).expect("the service state must be reachable")
    }

    /// Register the running, Nitro instance every row pushes at, and turn the account's
    /// serial-console access on so a serial-console row is not refused for the account.
    fn register_the_instance(&self) {
        let state = self.state();
        let mut guard = state.lock();
        guard.register_instance(Instance::new(
            INSTANCE_ID,
            ZONE,
            "m5.large",
            InstanceState::Running,
            /* serial_console_supported */ true,
        ));
        guard.serial_console_access_enabled = true;
    }

    /// How many keys of either kind this service holds right now.
    fn stored_key_count(&self) -> usize {
        let state = self.state();
        let guard = state.lock();
        guard.pushed_keys.len() + guard.serial_console_keys.len()
    }

    /// One EC2 Instance Connect call, with `access_key_id` in the SigV4 credential scope.
    async fn connect(&self, access_key_id: &str, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("ec2-instance-connect.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", target)
            .header(
                "authorization",
                authorization(access_key_id, "ec2-instance-connect"),
            )
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

    /// The push `target` names, at `instance_id`, signed with `access_key_id`.
    async fn push(
        &self,
        access_key_id: &str,
        target: &str,
        instance_id: &str,
        key: &str,
    ) -> (u16, Value) {
        let body = if target == SEND_SSH {
            json!({
                "InstanceId": instance_id,
                "InstanceOSUser": OS_USER,
                "SSHPublicKey": key,
            })
        } else {
            json!({
                "InstanceId": instance_id,
                "SerialPort": SERIAL_CONSOLE_PORT,
                "SSHPublicKey": key,
            })
        };
        self.connect(access_key_id, target, &body).await
    }

    /// One awsQuery call to another service mounted here, signed with `access_key_id`.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        access_key_id: &str,
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
            .header("host", format!("{signing_name}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization(access_key_id, signing_name))
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
        let (status, body) = self
            .query("iam", IAM_API_VERSION, ACCOUNT_KEY, action, params)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: IAM {action} must succeed, got {status} {body}"
        );
        body
    }

    /// One STS call, signed with `access_key_id`, which must succeed.
    async fn sts(
        &self,
        case: &str,
        access_key_id: &str,
        action: &str,
        params: &[(&str, &str)],
    ) -> String {
        let (status, body) = self
            .query("sts", STS_API_VERSION, access_key_id, action, params)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: STS {action} must succeed, got {status} {body}"
        );
        body
    }

    /// Mint one session credential of `kind` for a principal carrying `grant`, through the
    /// APIs a client would really use, and answer what the push is then decided about.
    async fn session_credential(&self, case: &str, kind: Credential, grant: Grant) -> Principal {
        match kind {
            Credential::UserSessionToken => {
                self.iam(case, "CreateUser", &[("UserName", PROBE_USER)])
                    .await;
                if let Some(document) = grant.document() {
                    self.iam(
                        case,
                        "PutUserPolicy",
                        &[
                            ("UserName", PROBE_USER),
                            ("PolicyName", INLINE_POLICY_NAME),
                            ("PolicyDocument", &document),
                        ],
                    )
                    .await;
                }
                let minted = self
                    .iam(case, "CreateAccessKey", &[("UserName", PROBE_USER)])
                    .await;
                let long_term = member(case, &minted, "AccessKeyId");
                // The user's own long-term key is what `GetSessionToken` must be called with
                // ("the long-term Amazon Web Services security credentials of an IAM user").
                let session = self.sts(case, &long_term, "GetSessionToken", &[]).await;
                Principal {
                    access_key_id: member(case, &session, "AccessKeyId"),
                    arn: format!(
                        "arn:aws:iam::{}:user/{PROBE_USER}",
                        engine::EMULATOR_ACCOUNT_ID
                    ),
                }
            }
            Credential::AssumedRole => {
                let role = self
                    .iam(
                        case,
                        "CreateRole",
                        &[
                            ("RoleName", PROBE_ROLE),
                            ("AssumeRolePolicyDocument", &trust_policy()),
                        ],
                    )
                    .await;
                let role_arn = member(case, &role, "Arn");
                if let Some(document) = grant.document() {
                    self.iam(
                        case,
                        "PutRolePolicy",
                        &[
                            ("RoleName", PROBE_ROLE),
                            ("PolicyName", INLINE_POLICY_NAME),
                            ("PolicyDocument", &document),
                        ],
                    )
                    .await;
                }
                let session = self
                    .sts(
                        case,
                        ACCOUNT_KEY,
                        "AssumeRole",
                        &[
                            ("RoleArn", &role_arn),
                            ("RoleSessionName", PROBE_SESSION),
                        ],
                    )
                    .await;
                Principal {
                    access_key_id: member(case, &session, "AccessKeyId"),
                    // `AssumedRoleUser.Arn`: the principal a refusal names for a session of
                    // this role.
                    arn: member(case, &session, "Arn"),
                }
            }
        }
    }

    /// Move the expiry of the session `access_key_id` names into the past, in STS's own
    /// store: the credential STS minted is untouched except for the one thing a test cannot
    /// wait for. The instant stays inside `EXPIRED_SESSION_RETENTION`, so STS keeps the
    /// record and reports the credential EXPIRED rather than unknown.
    fn let_the_session_expire(&self, case: &str, access_key_id: &str) {
        let state = sts::state::service_state(&*self.engine).expect("STS state is reachable");
        let mut guard = state.lock();
        let session = guard
            .sessions
            .get_mut(access_key_id)
            .unwrap_or_else(|| panic!("case {case:?}: STS must hold the session it just minted"));
        session.expires_at = SystemTime::now() - Duration::from_secs(60);
    }
}

/// Which kind of session credential a row presents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Credential {
    /// A session minted by `sts:GetSessionToken`, which speaks for the IAM user that asked
    /// for it, so the USER's policies decide the push.
    UserSessionToken,
    /// A session minted by `sts:AssumeRole`, which speaks for an assumed-role session, so the
    /// ROLE's policies decide the push.
    AssumedRole,
}

/// What the principal behind a row's credential is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grant {
    /// An inline policy allowing every action on every resource.
    Everything,
    /// No inline policy at all, so nothing allows the push.
    Nothing,
}

impl Grant {
    /// The inline policy document to put on the principal, or `None` for one carrying none.
    fn document(self) -> Option<String> {
        match self {
            Self::Everything => Some(
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#
                    .to_string(),
            ),
            Self::Nothing => None,
        }
    }
}

/// A minted credential and the principal a refusal would name for it.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// A SigV4 credential scope naming `signing_name` and `access_key_id`, which is how the router
/// picks the service out and how a service reads the caller's key id. The harness switches
/// request authentication off, so the signature itself is never verified.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
    )
}

/// A role trust policy admitting this account's own principals, written as `iam:CreateRole`
/// stores one — what makes the `AssumeRole` below succeed.
fn trust_policy() -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"arn:aws:iam::{}:root"}},"Action":"sts:AssumeRole"}}]}}"#,
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// Percent-encode one `application/x-www-form-urlencoded` component: every byte outside RFC
/// 3986's unreserved set is escaped, which is what an SDK does to a policy document's braces,
/// quotes and colons.
fn form_value(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The text of the first `<name>…</name>` element of an awsQuery response, or a panic naming
/// the case and the whole body.
fn member(case: &str, xml: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml
        .find(&open)
        .map(|at| at + open.len())
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {xml}"));
    let rest = &xml[start..];
    let end = rest
        .find(&close)
        .unwrap_or_else(|| panic!("case {case:?}: no </{name}> in {xml}"));
    rest[..end].to_string()
}

/// The `message` of an error body, or the empty string for a success.
fn message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The ARN of an EC2 instance in the Region every call is signed for — the resource the
/// policy decision is made about, built the way EC2 mints it.
fn instance_arn(instance_id: &str) -> String {
    format!(
        "arn:aws:ec2:{REGION}:{}:instance/{instance_id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The IAM action a target is performed under.
fn action_for(target: &str) -> String {
    let operation = target.rsplit('.').next().unwrap_or_default();
    format!("ec2-instance-connect:{operation}")
}

/// One row of the invalid-credential table.
struct InvalidRow {
    case: &'static str,
    credential: Credential,
    grant: Grant,
    target: &'static str,
    instance_id: &'static str,
}

/// RULE: a request signed with a credential the identity store does not hold as valid speaks
/// for no principal, so no policy is consulted and the push is refused with the modelled
/// `AuthException` and the sentence the model documents for it.
#[tokio::test]
async fn a_credential_that_is_not_valid_is_refused_as_the_modelled_auth_exception() {
    let rows = [
        InvalidRow {
            case: "a user session token, the user allowed everything, the SSH push",
            credential: Credential::UserSessionToken,
            grant: Grant::Everything,
            target: SEND_SSH,
            instance_id: INSTANCE_ID,
        },
        InvalidRow {
            case: "a user session token, the user allowed everything, the serial-console push",
            credential: Credential::UserSessionToken,
            grant: Grant::Everything,
            target: SEND_SERIAL,
            instance_id: INSTANCE_ID,
        },
        InvalidRow {
            case: "a user session token of a user carrying no policy at all",
            credential: Credential::UserSessionToken,
            grant: Grant::Nothing,
            target: SEND_SSH,
            instance_id: INSTANCE_ID,
        },
        InvalidRow {
            case: "an assumed-role session, the role allowed everything, the SSH push",
            credential: Credential::AssumedRole,
            grant: Grant::Everything,
            target: SEND_SSH,
            instance_id: INSTANCE_ID,
        },
        InvalidRow {
            case: "an assumed-role session, the role allowed everything, the serial-console push",
            credential: Credential::AssumedRole,
            grant: Grant::Everything,
            target: SEND_SERIAL,
            instance_id: INSTANCE_ID,
        },
        InvalidRow {
            case: "an assumed-role session of a role carrying no policy at all",
            credential: Credential::AssumedRole,
            grant: Grant::Nothing,
            target: SEND_SERIAL,
            instance_id: INSTANCE_ID,
        },
        // The credential is decided before anything about the instance is reported, so an id
        // nothing holds is still the credential's failure rather than a lookup's.
        InvalidRow {
            case: "an instance nothing holds",
            credential: Credential::UserSessionToken,
            grant: Grant::Everything,
            target: SEND_SSH,
            instance_id: ABSENT_INSTANCE_ID,
        },
    ];

    for row in rows {
        let InvalidRow {
            case,
            credential,
            grant,
            target,
            instance_id,
        } = row;
        let case = format!("{case} ({credential:?}, {grant:?}, {target})");
        let emulator = Emulator::new();
        emulator.register_the_instance();
        let principal = emulator.session_credential(&case, credential, grant).await;
        emulator.let_the_session_expire(&case, &principal.access_key_id);

        let (status, body) = emulator
            .push(
                &principal.access_key_id,
                target,
                instance_id,
                &ssh_key("expired"),
            )
            .await;

        assert_eq!(
            error_code(&body),
            "AuthException",
            "case {case:?}: expected the modelled AuthException, got {status} {body}"
        );
        assert_eq!(
            message(&body),
            AUTH_EXCEPTION_MESSAGE,
            "case {case:?}: expected the sentence the model documents for AuthException, got \
             {body}"
        );
        // `AuthException` is a `client` shape, and this service answers every client error
        // with HTTP 400 whatever `@httpError` the shape carries — the mapping
        // `error_status_and_input_constraints.rs` pins for the whole service.
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        // Nothing of the policy refusal: no principal was resolved and no document was read,
        // so neither may be blamed — and the credential is not quoted back either.
        assert!(
            !message(&body).contains("not authorized")
                && !message(&body).contains(&action_for(target))
                && !message(&body).contains(&principal.access_key_id)
                && !message(&body).contains(&principal.arn),
            "case {case:?}: the refusal must blame no principal, action, resource or \
             credential, got {body}"
        );
        assert_eq!(
            emulator.stored_key_count(),
            0,
            "case {case:?}: a refused push stores no key, state holds {}",
            emulator.stored_key_count()
        );
    }
}

/// One row of the valid-credential table.
struct ValidRow {
    case: &'static str,
    credential: Credential,
    grant: Grant,
    target: &'static str,
}

/// RULE (the other side of it): a credential the identity store DOES hold as valid is decided
/// by the policies of the principal it speaks for — allowed, or refused as not authorized with
/// `AccessDeniedException` and AWS's refusal sentence, which names that principal, the action,
/// the instance ARN and the rule that withheld the grant.
#[tokio::test]
async fn a_valid_session_credential_is_still_decided_by_the_principals_policies() {
    let rows = [
        ValidRow {
            case: "a user session token, the user allowed everything, the SSH push",
            credential: Credential::UserSessionToken,
            grant: Grant::Everything,
            target: SEND_SSH,
        },
        ValidRow {
            case: "a user session token, the user allowed everything, the serial-console push",
            credential: Credential::UserSessionToken,
            grant: Grant::Everything,
            target: SEND_SERIAL,
        },
        ValidRow {
            case: "a user session token of a user carrying no policy at all",
            credential: Credential::UserSessionToken,
            grant: Grant::Nothing,
            target: SEND_SSH,
        },
        ValidRow {
            case: "an assumed-role session, the role allowed everything, the SSH push",
            credential: Credential::AssumedRole,
            grant: Grant::Everything,
            target: SEND_SSH,
        },
        ValidRow {
            case: "an assumed-role session of a role carrying no policy at all",
            credential: Credential::AssumedRole,
            grant: Grant::Nothing,
            target: SEND_SERIAL,
        },
    ];

    for row in rows {
        let ValidRow {
            case,
            credential,
            grant,
            target,
        } = row;
        let case = format!("{case} ({credential:?}, {grant:?}, {target})");
        let emulator = Emulator::new();
        emulator.register_the_instance();
        let principal = emulator.session_credential(&case, credential, grant).await;

        let (status, body) = emulator
            .push(
                &principal.access_key_id,
                target,
                INSTANCE_ID,
                &ssh_key("live"),
            )
            .await;

        match grant {
            Grant::Everything => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (200, ""),
                    "case {case:?}: a live credential of a principal allowed everything must \
                     push, got {status} {body}"
                );
                assert_eq!(
                    body.get("Success").and_then(Value::as_bool),
                    Some(true),
                    "case {case:?}: expected Success true, got {body}"
                );
                assert_eq!(
                    emulator.stored_key_count(),
                    1,
                    "case {case:?}: the pushed key must be stored, state holds {}",
                    emulator.stored_key_count()
                );
            }
            Grant::Nothing => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (403, "AccessDeniedException"),
                    "case {case:?}: expected HTTP 403 AccessDeniedException, got {status} {body}"
                );
                let action = action_for(target);
                assert_eq!(
                    message(&body),
                    format!(
                        "User: {} is not authorized to perform: {action} on resource: {} \
                         because no identity-based policy allows the {action} action",
                        principal.arn,
                        instance_arn(INSTANCE_ID)
                    ),
                    "case {case:?}: expected AWS's refusal sentence, got {body}"
                );
                assert_eq!(
                    emulator.stored_key_count(),
                    0,
                    "case {case:?}: a refused push stores no key, state holds {}",
                    emulator.stored_key_count()
                );
            }
        }
    }
}
