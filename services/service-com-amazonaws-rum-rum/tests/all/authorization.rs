// SPDX-License-Identifier: CC0-1.0

//! Server-side authorization, end to end: every CloudWatch RUM operation decides on every
//! call whether the principal the request's credential speaks for may perform that
//! operation's action on the resource it names.
//!
//! The rule under test: a request is allowed only when, at the instant of the call, the
//! policies in force allow the operation's IAM action on that resource — the calling
//! principal's identity-based policies, or (for an operation addressing one app monitor)
//! that monitor's resource-based policy — and an explicit `Deny` in either refuses
//! whatever the other says. Everything else is refused with the modelled
//! `AccessDeniedException` (HTTP 403). A second rule rides on it: a call that turns
//! CloudWatch Logs delivery ON additionally requires the caller's `logs:PutResourcePolicy`
//! permission, because RUM installs the delivery policy on the caller's behalf.
//!
//! Nothing here is seeded into another service's store by hand: the roles are created
//! through IAM's own `CreateRole`/`PutRolePolicy`, the session credential comes from STS's
//! own `AssumeRole`, and the RUM calls carry that credential's access key id in their
//! SigV4 credential scope — the same path a client takes. The tables span what the rule
//! generalises over: the operation and its action, where the grant comes from
//! (identity-based or resource-based), the action and resource patterns a statement names,
//! the effect, a condition key, and the principal spelling a resource policy uses.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The region every request below is signed for; the ARNs in the policies are taken in
/// it, because an app monitor's ARN carries the region the create was signed for.
pub(crate) const REGION: &str = "us-east-1";

/// An access key id that names no STS session, which is what the account's own published
/// long-term credential looks like to a service: STS resolves it to the ambient identity,
/// the account itself, which holds no IAM policies.
pub(crate) const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The account id every emulated ARN carries.
pub(crate) fn account() -> &'static str {
    engine::EMULATOR_ACCOUNT_ID
}

/// One HTTP answer from the RUM service.
pub(crate) struct Reply {
    pub status: u16,
    pub body: Bytes,
}

impl Reply {
    pub fn json(&self, case: &str) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: expected a JSON body, got {:?} ({error})",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code: restJson1 carries it as the body's `__type`.
    pub fn error_code(&self, case: &str) -> String {
        self.json(case)["__type"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: an error body must carry __type, got {:?}",
                    String::from_utf8_lossy(&self.body)
                )
            })
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The error body's `message`, for the one refusal whose wording AWS builds from the
    /// request.
    pub fn message(&self, case: &str) -> String {
        self.json(case)["message"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator the tests drive: one shared engine carrying the RUM service, the
/// CloudWatch Logs service (RUM provisions the vended log group by calling it), and the
/// IAM and STS services the roles and the session credentials are made with — the same
/// set `core/router`'s `build_router` assembles, narrowed to what these rules touch.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_rum_rum::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
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

    /// One RUM call, with `access_key_id` in the SigV4 credential scope.
    pub(crate) async fn rum(
        &self,
        access_key_id: &str,
        method: &str,
        path_and_query: &str,
        body: Option<Value>,
    ) -> Reply {
        let body_bytes = match body {
            Some(value) => {
                Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
            }
            None => Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://rum.{REGION}.amazonaws.com{path_and_query}"
            ))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/rum/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .header("content-type", "application/json")
            .body(body_bytes)
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        Reply {
            status: response.status().as_u16(),
            body: response.body().clone(),
        }
    }

    /// One awsQuery call to IAM or STS: `signing_name` picks the service, `version` its
    /// API version.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        access_key_id: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&super::support::percent_encoded(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// One CloudWatch Logs call (awsJson1_1).
    pub(crate) async fn logs(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("Logs_20140328.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/logs/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create an app monitor with the account's own credential and answer its minted id.
    pub(crate) async fn create_monitor(&self, name: &str) -> String {
        let reply = self
            .rum(
                ACCOUNT_KEY,
                "POST",
                "/appmonitor",
                Some(json!({"Name": name, "Domain": "example.com"})),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "creating the app monitor {name:?} must succeed, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        reply.json(name)["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateAppMonitor must answer an Id, got {}", reply.json(name)))
            .to_string()
    }

    /// Create an IAM role named `role_name`, give it `policy` as an inline policy when one
    /// is supplied, assume it through STS, and answer the access key id of the session —
    /// the credential a caller then signs its RUM requests with.
    pub(crate) async fn assumed_role_key(&self, role_name: &str, policy: Option<&str>) -> String {
        let trust = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Principal\":{{\"AWS\":\"arn:aws:iam::{}:root\"}},\
             \"Action\":\"sts:AssumeRole\"}}]}}",
            account()
        );
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                ACCOUNT_KEY,
                "CreateRole",
                &[
                    ("RoleName", role_name.to_string()),
                    ("AssumeRolePolicyDocument", trust),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "creating the role {role_name:?} must succeed, got {status} {body}"
        );
        if let Some(policy) = policy {
            let (status, body) = self
                .query(
                    "iam",
                    "2010-05-08",
                    ACCOUNT_KEY,
                    "PutRolePolicy",
                    &[
                        ("RoleName", role_name.to_string()),
                        ("PolicyName", "inline".to_string()),
                        ("PolicyDocument", policy.to_string()),
                    ],
                )
                .await;
            assert_eq!(
                status, 200,
                "putting the inline policy of {role_name:?} must succeed, got {status} {body}"
            );
        }
        let (status, body) = self
            .query(
                "sts",
                "2011-06-15",
                ACCOUNT_KEY,
                "AssumeRole",
                &[
                    ("RoleArn", role_arn(role_name)),
                    ("RoleSessionName", "probe".to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "assuming {role_name:?} must succeed, got {status} {body}"
        );
        xml_element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("AssumeRole must answer an AccessKeyId, got {body}"))
    }

    /// Create a customer-managed IAM policy with `document` and answer its ARN.
    pub(crate) async fn create_managed_policy(&self, name: &str, document: &str) -> String {
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                ACCOUNT_KEY,
                "CreatePolicy",
                &[
                    ("PolicyName", name.to_string()),
                    ("PolicyDocument", document.to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "creating the managed policy {name:?} must succeed, got {status} {body}"
        );
        xml_element(&body, "Arn")
            .unwrap_or_else(|| panic!("CreatePolicy must answer an Arn, got {body}"))
    }

    /// Attach the managed policy `policy_arn` to the role `role_name` through IAM's own
    /// `AttachRolePolicy`.
    pub(crate) async fn attach_role_policy(&self, role_name: &str, policy_arn: &str) {
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                ACCOUNT_KEY,
                "AttachRolePolicy",
                &[
                    ("RoleName", role_name.to_string()),
                    ("PolicyArn", policy_arn.to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "attaching {policy_arn:?} to {role_name:?} must succeed, got {status} {body}"
        );
    }

    /// Attach `document` to the app monitor as its resource policy, with the account's own
    /// credential.
    pub(crate) async fn put_resource_policy(&self, monitor: &str, document: &str) {
        let reply = self
            .rum(
                ACCOUNT_KEY,
                "PUT",
                &format!("/appmonitor/{monitor}/policy"),
                Some(json!({"PolicyDocument": document})),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "PutResourcePolicy on {monitor:?} must succeed, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }

    /// Remove the app monitor's resource policy, with the account's own credential.
    pub(crate) async fn delete_resource_policy(&self, monitor: &str) {
        let reply = self
            .rum(
                ACCOUNT_KEY,
                "DELETE",
                &format!("/appmonitor/{monitor}/policy"),
                None,
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "DeleteResourcePolicy on {monitor:?} must succeed, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }
}

/// The text of the first `<name>…</name>` element of an awsQuery response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The ARN of an IAM role of this account at the default path.
pub(crate) fn role_arn(role_name: &str) -> String {
    format!("arn:aws:iam::{}:role/{role_name}", account())
}

/// The ARN of an app monitor of this account in [`REGION`].
pub(crate) fn monitor_arn(name: &str) -> String {
    format!("arn:aws:rum:{REGION}:{}:appmonitor/{name}", account())
}

/// An identity-based policy with the statements given.
fn identity_policy(statements: &str) -> String {
    format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{statements}]}}")
}

/// One `Allow`/`Deny` statement over an action and a resource.
fn statement(effect: &str, action: &str, resource: &str) -> String {
    format!(
        "{{\"Effect\":\"{effect}\",\"Action\":\"{action}\",\"Resource\":\"{resource}\"}}"
    )
}

/// A resource policy granting `principal` one action on this monitor, with an optional
/// condition clause.
fn resource_policy(effect: &str, principal: &str, action: &str, condition: Option<&str>) -> String {
    let condition = condition
        .map(|clause| format!(",\"Condition\":{clause}"))
        .unwrap_or_default();
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"AWS\":\"{principal}\"}},\"Action\":\"{action}\",\
         \"Resource\":\"*\"{condition}}}]}}"
    )
}

/// A `PutRumEvents` body for the monitor whose id is `id`, carrying `alias` when one is
/// given.
pub(crate) fn rum_events_body(id: &str, alias: Option<&str>) -> Value {
    let mut body = json!({
        "BatchId": "11111111-2222-4333-8444-555555555555",
        "AppMonitorDetails": {"name": "any", "id": id, "version": "1.0.0"},
        "UserDetails": {
            "userId": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "sessionId": "ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb",
        },
        "RumEvents": [{
            "id": "12345678-1234-4234-8234-123456789012",
            "timestamp": 1_704_067_200,
            "type": "com.amazon.rum.page_view_event",
            "details": "{\"pageId\":\"/home\"}",
        }],
    });
    if let Some(alias) = alias {
        body["Alias"] = json!(alias);
    }
    body
}

/// One row of the `PutRumEvents` decision table.
struct DecisionCase {
    case: &'static str,
    /// The inline policy the calling role holds, when it holds one.
    identity: Option<String>,
    /// The resource policy on the app monitor, when it carries one.
    resource: Option<String>,
    /// The `Alias` the call carries, which is what the `rum:alias` condition key compares
    /// against.
    alias: Option<&'static str>,
    /// Whether the call must be allowed.
    allowed: bool,
}

/// `PutRumEvents` is accepted exactly when the policies in force allow
/// `rum:PutRumEvents` on the app monitor the call names, and refused with
/// `AccessDeniedException` otherwise.
#[tokio::test]
async fn put_rum_events_is_allowed_only_where_a_policy_in_force_allows_it() {
    let emulator = Emulator::new();
    let monitor = "decisions";
    let id = emulator.create_monitor(monitor).await;
    // A second monitor, so a grant scoped to ANOTHER resource is distinguishable from a
    // grant scoped to this one.
    let other = "decisions-other";
    emulator.create_monitor(other).await;
    // A principal that is not the caller, for the resource-policy row that names one.
    let stranger = role_arn("stranger");

    let rows: Vec<DecisionCase> = vec![
        DecisionCase {
            case: "a role with no policy at all, and no resource policy",
            identity: None,
            resource: None,
            alias: None,
            allowed: false,
        },
        DecisionCase {
            case: "an identity policy naming the action and this monitor",
            identity: Some(identity_policy(&statement(
                "Allow",
                "rum:PutRumEvents",
                &monitor_arn(monitor),
            ))),
            resource: None,
            alias: None,
            allowed: true,
        },
        DecisionCase {
            case: "an identity policy with a service wildcard on every resource",
            identity: Some(identity_policy(&statement("Allow", "rum:*", "*"))),
            resource: None,
            alias: None,
            allowed: true,
        },
        DecisionCase {
            case: "an identity policy scoped to another monitor's ARN",
            identity: Some(identity_policy(&statement(
                "Allow",
                "rum:PutRumEvents",
                &monitor_arn(other),
            ))),
            resource: None,
            alias: None,
            allowed: false,
        },
        DecisionCase {
            case: "an identity policy allowing another action on every resource",
            identity: Some(identity_policy(&statement(
                "Allow",
                "rum:GetAppMonitor",
                "*",
            ))),
            resource: None,
            alias: None,
            allowed: false,
        },
        DecisionCase {
            case: "an identity policy allowing the action and explicitly denying it",
            identity: Some(identity_policy(&format!(
                "{},{}",
                statement("Allow", "rum:*", "*"),
                statement("Deny", "rum:PutRumEvents", "*")
            ))),
            resource: None,
            alias: None,
            allowed: false,
        },
        DecisionCase {
            case: "no identity policy, a resource policy naming the session's role",
            identity: None,
            resource: Some(resource_policy(
                "Allow",
                &role_arn("decider"),
                "rum:PutRumEvents",
                None,
            )),
            alias: None,
            allowed: true,
        },
        DecisionCase {
            case: "no identity policy, a resource policy naming another principal",
            identity: None,
            resource: Some(resource_policy(
                "Allow",
                &stranger,
                "rum:PutRumEvents",
                None,
            )),
            alias: None,
            allowed: false,
        },
        DecisionCase {
            case: "no identity policy, a resource policy naming every principal",
            identity: None,
            resource: Some(resource_policy("Allow", "*", "rum:PutRumEvents", None)),
            alias: None,
            allowed: true,
        },
        DecisionCase {
            case: "an identity policy that allows and a resource policy that denies",
            identity: Some(identity_policy(&statement("Allow", "rum:*", "*"))),
            resource: Some(resource_policy("Deny", "*", "rum:PutRumEvents", None)),
            alias: None,
            allowed: false,
        },
        DecisionCase {
            case: "a resource policy conditioned on the rum:alias the call carries",
            identity: None,
            resource: Some(resource_policy(
                "Allow",
                "*",
                "rum:PutRumEvents",
                Some("{\"StringEquals\":{\"rum:alias\":\"shop-web\"}}"),
            )),
            alias: Some("shop-web"),
            allowed: true,
        },
        DecisionCase {
            case: "the same condition against another alias",
            identity: None,
            resource: Some(resource_policy(
                "Allow",
                "*",
                "rum:PutRumEvents",
                Some("{\"StringEquals\":{\"rum:alias\":\"shop-web\"}}"),
            )),
            alias: Some("other-alias"),
            allowed: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // A fresh role per row, so no row inherits another's grant. The role name is the
        // same for every row that the resource-policy rows name as a principal.
        let role = format!("decider-{index}");
        let key = emulator
            .assumed_role_key(&role, row.identity.as_deref())
            .await;
        if let Some(document) = row.resource.as_deref() {
            // The resource policy's principal rows name the role `decider`, so the
            // document is rewritten to this row's role: the RULE is about the session's
            // own principal, not about one name.
            let document = document.replace(&role_arn("decider"), &role_arn(&role));
            emulator.put_resource_policy(monitor, &document).await;
        }

        let reply = emulator
            .rum(
                &key,
                "POST",
                &format!("/appmonitors/{id}/"),
                Some(rum_events_body(&id, row.alias)),
            )
            .await;
        if row.allowed {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected PutRumEvents to be allowed, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
        } else {
            assert_eq!(
                reply.status, 403,
                "case {case:?}: expected PutRumEvents to be refused with HTTP 403, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(case),
                "AccessDeniedException",
                "case {case:?}: expected AccessDeniedException, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
        }

        if row.resource.is_some() {
            emulator.delete_resource_policy(monitor).await;
        }
    }
}

/// The decision is made per call, not cached at grant time: the same unprivileged
/// principal is refused, then allowed once a resource policy grants it, then refused again
/// once that policy is deleted — with nothing else changing in between.
#[tokio::test]
async fn a_resource_policy_grants_from_the_call_after_it_and_stops_at_its_deletion() {
    let emulator = Emulator::new();
    let monitor = "lifecycle";
    let id = emulator.create_monitor(monitor).await;
    let key = emulator.assumed_role_key("lifecycle-writer", None).await;
    let grant = resource_policy(
        "Allow",
        &role_arn("lifecycle-writer"),
        "rum:PutRumEvents",
        None,
    );

    let write = |case: &'static str| (case, rum_events_body(&id, None));

    // Before any grant exists.
    let path = format!("/appmonitors/{id}/");
    let (case, body) = write("before the grant");
    let reply = emulator.rum(&key, "POST", &path, Some(body)).await;
    assert_eq!(
        reply.status, 403,
        "case {case:?}: expected HTTP 403, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );

    emulator.put_resource_policy(monitor, &grant).await;
    let (case, body) = write("while the grant is in force");
    let reply = emulator.rum(&key, "POST", &path, Some(body)).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );

    emulator.delete_resource_policy(monitor).await;
    let (case, body) = write("after the grant is deleted");
    let reply = emulator.rum(&key, "POST", &path, Some(body)).await;
    assert_eq!(
        reply.status, 403,
        "case {case:?}: expected HTTP 403, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.error_code(case),
        "AccessDeniedException",
        "case {case:?}: expected AccessDeniedException, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
}

/// Every operation evaluates ITS OWN action: a principal allowed exactly one RUM action
/// may perform that operation and no other.
///
/// The rows are the operations this service implements, each with the action it is
/// authorized against and a request that would otherwise succeed.
#[tokio::test]
async fn each_operation_is_authorized_against_its_own_action() {
    // (case, the IAM action, the method, the path template, the body)
    struct OperationCase {
        action: &'static str,
        method: &'static str,
        /// `{name}` is replaced with the row's own app monitor name, `{id}` with its id.
        path: &'static str,
        body: Option<Value>,
    }
    let rows: [OperationCase; 10] = [
        OperationCase {
            action: "rum:GetAppMonitor",
            method: "GET",
            path: "/appmonitor/{name}",
            body: None,
        },
        OperationCase {
            action: "rum:PutRumMetricsDestination",
            method: "POST",
            path: "/rummetrics/{name}/metricsdestination",
            body: Some(json!({"Destination": "CloudWatch"})),
        },
        OperationCase {
            action: "rum:ListRumMetricsDestinations",
            method: "GET",
            path: "/rummetrics/{name}/metricsdestination",
            body: None,
        },
        OperationCase {
            action: "rum:UpdateAppMonitor",
            method: "PATCH",
            path: "/appmonitor/{name}",
            body: Some(json!({"Domain": "updated.example.com"})),
        },
        OperationCase {
            action: "rum:ListAppMonitors",
            method: "POST",
            path: "/appmonitors",
            body: None,
        },
        OperationCase {
            action: "rum:PutResourcePolicy",
            method: "PUT",
            path: "/appmonitor/{name}/policy",
            body: Some(json!({
                "PolicyDocument": "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\
                 \"Allow\",\"Principal\":{\"AWS\":\"*\"},\"Action\":\"rum:PutRumEvents\",\
                 \"Resource\":\"*\"}]}"
            })),
        },
        OperationCase {
            action: "rum:GetResourcePolicy",
            method: "GET",
            path: "/appmonitor/{name}/policy",
            body: None,
        },
        OperationCase {
            action: "rum:DeleteResourcePolicy",
            method: "DELETE",
            path: "/appmonitor/{name}/policy",
            body: None,
        },
        OperationCase {
            action: "rum:PutRumEvents",
            method: "POST",
            path: "/appmonitors/{id}/",
            body: None,
        },
        OperationCase {
            action: "rum:DeleteAppMonitor",
            method: "DELETE",
            path: "/appmonitor/{name}",
            body: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let monitor = format!("per-action-{index}");
        let id = emulator.create_monitor(&monitor).await;
        // Every row's monitor carries a resource policy, so `GetResourcePolicy` and
        // `DeleteResourcePolicy` have one to act on. It grants nothing (it names a
        // principal nobody is), so it never contributes to a decision below.
        emulator
            .put_resource_policy(
                &monitor,
                &resource_policy("Allow", &role_arn("nobody"), "rum:PutRumEvents", None),
            )
            .await;

        let allowed_key = emulator
            .assumed_role_key(
                &format!("allowed-{index}"),
                Some(&identity_policy(&statement("Allow", row.action, "*"))),
            )
            .await;
        // A role allowed a DIFFERENT RUM action, to show the grant is read action by
        // action rather than service by service.
        let other_action = if row.action == "rum:GetAppMonitor" {
            "rum:ListAppMonitors"
        } else {
            "rum:GetAppMonitor"
        };
        let refused_key = emulator
            .assumed_role_key(
                &format!("refused-{index}"),
                Some(&identity_policy(&statement("Allow", other_action, "*"))),
            )
            .await;

        let path = row.path.replace("{name}", &monitor).replace("{id}", &id);
        let body = match (row.body.clone(), row.action) {
            (Some(body), _) => Some(body),
            (None, "rum:PutRumEvents") => Some(rum_events_body(&id, None)),
            (None, _) => None,
        };

        let case = format!("{} refused for a principal allowed {other_action}", row.action);
        let reply = emulator
            .rum(&refused_key, row.method, &path, body.clone())
            .await;
        assert_eq!(
            reply.status, 403,
            "case {case:?}: expected HTTP 403, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(&case),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );

        let case = format!("{} allowed for a principal allowed exactly it", row.action);
        let reply = emulator.rum(&allowed_key, row.method, &path, body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }
}

/// `DeleteRumMetricsDestination` is authorized against `rum:DeleteRumMetricsDestination`
/// on the app monitor's ARN, like every other operation of this service — and a refused
/// delete removes nothing.
///
/// It needs its own case rather than a row in the table above because the request only
/// reaches a 200 when the destination it names is already stored, so each row has to put
/// one first. The rule generalises over the destination kind, so both are rows.
#[tokio::test]
async fn deleting_a_metrics_destination_is_authorized_and_a_refusal_removes_nothing() {
    // (case, the destination's `Destination`, its `DestinationArn` when it has one)
    let rows: [(&str, &str, Option<String>); 2] = [
        ("CloudWatch", "CloudWatch", None),
        (
            "Evidently",
            "Evidently",
            Some(engine::emulator_arn(
                "evidently",
                REGION,
                "experiment/authorized",
            )),
        ),
    ];
    for (index, (case, destination, destination_arn)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let monitor = format!("destination-auth-{index}");
        emulator.create_monitor(&monitor).await;

        let mut put_body = json!({"Destination": destination});
        let mut query = format!("destination={destination}");
        if let Some(arn) = destination_arn {
            put_body["DestinationArn"] = json!(arn);
            put_body["IamRoleArn"] = json!(engine::emulator_arn("iam", "", "role/rum-writer"));
            // A query value carrying an ARN's `:` and `/` reaches the server percent-encoded.
            query.push_str(&format!(
                "&destinationArn={}",
                super::support::percent_encoded(arn)
            ));
        }
        let path = format!("/rummetrics/{monitor}/metricsdestination");
        let reply = emulator
            .rum(ACCOUNT_KEY, "POST", &path, Some(put_body))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: seeding the destination must succeed, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );

        let refused_key = emulator
            .assumed_role_key(
                &format!("destination-refused-{index}"),
                Some(&identity_policy(&statement(
                    "Allow",
                    "rum:PutRumMetricsDestination",
                    "*",
                ))),
            )
            .await;
        let allowed_key = emulator
            .assumed_role_key(
                &format!("destination-allowed-{index}"),
                Some(&identity_policy(&statement(
                    "Allow",
                    "rum:DeleteRumMetricsDestination",
                    "*",
                ))),
            )
            .await;

        // A principal allowed the PUT action may not delete: the decision is made action
        // by action.
        let delete_path = format!("{path}?{query}");
        let reply = emulator
            .rum(&refused_key, "DELETE", &delete_path, None)
            .await;
        assert_eq!(
            reply.status, 403,
            "case {case:?}: expected HTTP 403 for a principal allowed only the put, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );

        // The refused delete removed nothing: the destination is still reported.
        let listed = emulator.rum(ACCOUNT_KEY, "GET", &path, None).await;
        assert_eq!(
            listed.json(case)["Destinations"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            1,
            "case {case:?}: a refused delete must leave the destination stored, got {:?}",
            String::from_utf8_lossy(&listed.body)
        );

        // The principal allowed exactly this action may delete it.
        let reply = emulator
            .rum(&allowed_key, "DELETE", &delete_path, None)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200 for a principal allowed the delete, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let listed = emulator.rum(ACCOUNT_KEY, "GET", &path, None).await;
        assert_eq!(
            listed.json(case)["Destinations"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            0,
            "case {case:?}: the authorized delete must remove the destination, got {:?}",
            String::from_utf8_lossy(&listed.body)
        );
    }
}

/// `CreateAppMonitor` is authorized too, against `rum:CreateAppMonitor` on the ARN of the
/// monitor it would mint — and a refused create mints nothing.
#[tokio::test]
async fn creating_an_app_monitor_is_authorized_and_a_refusal_mints_nothing() {
    let emulator = Emulator::new();
    let rows: [(&str, Option<String>, bool); 3] = [
        ("a role with no policy", None, false),
        (
            "a role allowed another action",
            Some(identity_policy(&statement("Allow", "rum:GetAppMonitor", "*"))),
            false,
        ),
        (
            "a role allowed the action on the monitor's own ARN",
            Some(identity_policy(&statement(
                "Allow",
                "rum:CreateAppMonitor",
                &monitor_arn("creates-2"),
            ))),
            true,
        ),
    ];
    for (index, (case, policy, allowed)) in rows.into_iter().enumerate() {
        let name = format!("creates-{index}");
        let key = emulator
            .assumed_role_key(&format!("creator-{index}"), policy.as_deref())
            .await;
        let reply = emulator
            .rum(
                &key,
                "POST",
                "/appmonitor",
                Some(json!({"Name": name, "Domain": "example.com"})),
            )
            .await;
        let expected = if allowed { 200 } else { 403 };
        assert_eq!(
            reply.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        // Whatever the decision was, the store must agree with it: the monitor exists
        // only where the create was allowed.
        let read = emulator
            .rum(ACCOUNT_KEY, "GET", &format!("/appmonitor/{name}"), None)
            .await;
        assert_eq!(
            read.status == 200,
            allowed,
            "case {case:?}: expected the monitor to exist = {allowed}, got {} {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
    }
}

/// A grant can come from a managed policy ATTACHED to the role as well as from an inline
/// one, and what the attachment grants is read from the attached document — except for an
/// AWS-managed policy, whose document the account does not hold, which this emulator
/// cannot read and therefore does not refuse on.
///
/// The rows differ along where the attached policy's document lives and what it says.
#[tokio::test]
async fn an_attached_managed_policy_is_read_as_the_document_it_names() {
    // (case, the document the attached policy carries, or None for an AWS-managed ARN
    //  whose document the account does not hold, whether PutRumEvents is allowed)
    let rows: [(&str, Option<String>, bool); 3] = [
        (
            "a customer-managed policy allowing the action",
            Some(identity_policy(&statement("Allow", "rum:PutRumEvents", "*"))),
            true,
        ),
        (
            "a customer-managed policy allowing another action",
            Some(identity_policy(&statement("Allow", "rum:GetAppMonitor", "*"))),
            false,
        ),
        (
            // An AWS-managed ARN: IAM accepts the attachment without the account
            // holding the policy, so RUM cannot see what it grants and does not refuse
            // on a grant it cannot read.
            "an AWS-managed policy, whose document the account does not hold",
            None,
            true,
        ),
    ];

    for (index, (case, document, allowed)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let monitor = format!("attached-{index}");
        let id = emulator.create_monitor(&monitor).await;
        let role = format!("attached-caller-{index}");
        // The role itself holds no inline policy: the attachment is the only grant.
        let key = emulator.assumed_role_key(&role, None).await;
        let policy_arn = match &document {
            Some(document) => {
                emulator
                    .create_managed_policy(&format!("attached-policy-{index}"), document)
                    .await
            }
            None => "arn:aws:iam::aws:policy/ReadOnlyAccess".to_string(),
        };
        emulator.attach_role_policy(&role, &policy_arn).await;

        let reply = emulator
            .rum(
                &key,
                "POST",
                &format!("/appmonitors/{id}/"),
                Some(rum_events_body(&id, None)),
            )
            .await;
        let expected = if allowed { 200 } else { 403 };
        assert_eq!(
            reply.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }
}

/// Turning CloudWatch Logs delivery on requires the caller's `logs:PutResourcePolicy`,
/// because RUM installs the log group's delivery policy on the caller's behalf. A refused
/// request changes nothing.
///
/// The rows differ along what the rule generalises over: how the caller's CloudWatch Logs
/// permission is withheld (not granted at all, or explicitly denied) or held, the
/// account's own credential, and the direction of the change — turning delivery OFF
/// installs no policy and so needs no permission.
#[tokio::test]
async fn enabling_cw_log_delivery_requires_the_callers_logs_put_resource_policy() {
    // (case, the role's inline policy, the CwLogEnabled the request asks for, allowed)
    let rows: [(&str, Option<String>, bool, bool); 5] = [
        (
            "a role allowed every rum action but no logs action",
            Some(identity_policy(&statement("Allow", "rum:*", "*"))),
            true,
            false,
        ),
        (
            "a role allowed every rum action and explicitly denied logs:PutResourcePolicy",
            Some(identity_policy(&format!(
                "{},{}",
                statement("Allow", "rum:*", "*"),
                statement("Deny", "logs:PutResourcePolicy", "*")
            ))),
            true,
            false,
        ),
        (
            "a role allowed both",
            Some(identity_policy(&format!(
                "{},{}",
                statement("Allow", "rum:*", "*"),
                statement("Allow", "logs:PutResourcePolicy", "*")
            ))),
            true,
            true,
        ),
        (
            "a role with no logs permission turning delivery OFF",
            Some(identity_policy(&statement("Allow", "rum:*", "*"))),
            false,
            true,
        ),
        (
            "the account's own credential",
            None,
            true,
            true,
        ),
    ];

    for (index, (case, policy, enable, allowed)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let monitor = format!("cw-log-{index}");
        emulator.create_monitor(&monitor).await;
        let key = match &policy {
            Some(policy) => {
                emulator
                    .assumed_role_key(&format!("cw-log-caller-{index}"), Some(policy))
                    .await
            }
            None => ACCOUNT_KEY.to_string(),
        };

        let reply = emulator
            .rum(
                &key,
                "PATCH",
                &format!("/appmonitor/{monitor}"),
                Some(json!({"CwLogEnabled": enable})),
            )
            .await;
        if allowed {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
        } else {
            assert_eq!(
                reply.status, 403,
                "case {case:?}: expected HTTP 403, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(case),
                "AccessDeniedException",
                "case {case:?}: expected AccessDeniedException, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
            // The wording AWS answers this refusal with is built from the request's
            // `Name` member.
            assert_eq!(
                reply.message(case),
                format!("Error updating the application {monitor}"),
                "case {case:?}: expected the refusal to name the application from the \
                 request, got {:?}",
                reply.message(case)
            );
        }

        // What the store now says, read with the account's own credential: the change
        // took effect only where it was allowed.
        let read = emulator
            .rum(ACCOUNT_KEY, "GET", &format!("/appmonitor/{monitor}"), None)
            .await;
        let cw_log = read.json(case)["AppMonitor"]["DataStorage"]["CwLog"].clone();
        let expected = allowed && enable;
        assert_eq!(
            cw_log["CwLogEnabled"].as_bool(),
            Some(expected),
            "case {case:?}: expected CwLogEnabled = {expected}, got {cw_log}"
        );
    }
}
