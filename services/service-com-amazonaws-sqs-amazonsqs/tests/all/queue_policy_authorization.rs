// SPDX-License-Identifier: CC0-1.0

//! The queue-policy gate, end to end: every operation that acts on a queue decides, on
//! every call, whether the policies in force let the principal the request's credential
//! speaks for perform that operation's IAM action on that queue.
//!
//! The rule under test: at the instant of the call, an explicit `Deny` about the request —
//! in the queue's own `Policy` or in one of the calling principal's identity-based policies
//! — refuses it, whatever any grant says; otherwise the request needs a grant (an `Allow`
//! about it in those identity-based policies, or one in the queue's policy that names the
//! CALLER rather than its account), and anything else is refused `AccessDenied` at HTTP 403
//! with no effect. The queue's owner — the account's own published credential — keeps full
//! control of it, and only an explicit deny refuses that. A permissions boundary attached to
//! the principal CAPS what its identity-based policies grant and grants nothing of its own,
//! so a refusal names the document the grant was missing from.
//!
//! Two rules ride on it, both about the document rather than the decision: a `Policy` is
//! VALIDATED when it is written (an action outside this service's action set, or a principal
//! of this account that does not exist, is refused and the attribute is left as it was), and
//! it is STORED canonically (a bare account id reads back as that account's root ARN, and an
//! empty value removes the attribute).
//!
//! Nothing here is seeded into another service's store by hand: the users and roles are made
//! through IAM's own `CreateUser`/`CreateAccessKey`/`PutUserPolicy`/`CreateRole`, the session
//! credential comes from STS's own `AssumeRole`, and the SQS calls carry that credential's
//! access key id in their SigV4 credential scope — the same path a client takes.
//!
//! The tables span what the rule generalises over: where the grant comes from
//! (identity-based or resource-based), the principal spelling a statement names (the
//! caller's own ARN, its account's root, the wildcard, a role behind a session), the action
//! and resource patterns, the effect, a condition key the request can answer and one it
//! cannot, whether a permissions boundary is attached and what it admits, and the operation
//! (and therefore the action) being judged.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{account, error_code, expected_arn, form_encode, xml_element, REGION};

/// An access key id that names no IAM user and no STS session: the account's own published
/// credential, which is the position AWS account root credentials are in.
const OWNER_KEY: &str = "AKIDEXAMPLEOWNER";

/// The emulator the suite drives: Amazon SQS beside the IAM and STS services the principals
/// and their credentials are made with, all on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Nothing here signs: the `Authorization` header carries only the credential scope,
        // which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(Arc::clone(&engine)),
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

    /// One Amazon SQS call, presenting `access_key_id`.
    async fn sqs(&self, access_key_id: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("sqs.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("AmazonSQS.{operation}"))
            .header("authorization", authorization(access_key_id, "sqs"))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// One Amazon SQS call that must succeed.
    async fn sqs_ok(&self, case: &str, access_key_id: &str, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.sqs(access_key_id, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to succeed, got {status} with {parsed}"
        );
        parsed
    }

    /// One awsQuery call to IAM or STS.
    async fn query(
        &self,
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
            .header("authorization", authorization(OWNER_KEY, signing_name))
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// One IAM action, which must succeed.
    async fn iam(&self, case: &str, action: &str, parameters: &[(&str, String)]) -> String {
        let (status, body) = self.query("iam", "2010-05-08", action, parameters).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {action} to succeed, got {status} {body}"
        );
        body
    }

    /// Create a queue and answer its url.
    async fn queue(&self, case: &str, name: &str) -> String {
        let created = self
            .sqs_ok(case, OWNER_KEY, "CreateQueue", json!({"QueueName": name}))
            .await;
        created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateQueue answered {created}"))
            .to_string()
    }

    /// Write `document` as the queue's `Policy`, with the owner's own credential.
    async fn set_policy(&self, case: &str, url: &str, document: &Value) {
        self.sqs_ok(
            case,
            OWNER_KEY,
            "SetQueueAttributes",
            json!({"QueueUrl": url, "Attributes": {"Policy": document.to_string()}}),
        )
        .await;
    }

    /// The attributes `GetQueueAttributes` reports for the queue at `url`, read with the
    /// owner's own credential.
    async fn attributes(&self, case: &str, url: &str) -> BTreeMap<String, String> {
        let reported = self
            .sqs_ok(
                case,
                OWNER_KEY,
                "GetQueueAttributes",
                json!({"QueueUrl": url, "AttributeNames": ["All"]}),
            )
            .await;
        reported["Attributes"]
            .as_object()
            .map(|map| {
                map.iter()
                    .filter_map(|(name, value)| {
                        value.as_str().map(|value| (name.clone(), value.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Create an IAM user carrying `tags` (`(key, value)` pairs), give it `policy` as an
    /// inline policy when one is supplied, mint an access key for it, and answer
    /// `(the user's ARN, the access key id)`.
    async fn user(
        &self,
        case: &str,
        user_name: &str,
        policy: Option<&Value>,
        tags: &[(&str, &str)],
    ) -> (String, String) {
        let mut parameters = vec![("UserName", user_name.to_string())];
        for (index, (key, value)) in tags.iter().enumerate() {
            parameters.push((
                tag_key_parameter(index),
                (*key).to_string(),
            ));
            parameters.push((
                tag_value_parameter(index),
                (*value).to_string(),
            ));
        }
        let created = self.iam(case, "CreateUser", &parameters).await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("case {case:?}: CreateUser answered {created}"));
        if let Some(policy) = policy {
            self.iam(
                case,
                "PutUserPolicy",
                &[
                    ("UserName", user_name.to_string()),
                    ("PolicyName", "inline".to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
        }
        let minted = self
            .iam(
                case,
                "CreateAccessKey",
                &[("UserName", user_name.to_string())],
            )
            .await;
        let key = xml_element(&minted, "AccessKeyId")
            .unwrap_or_else(|| panic!("case {case:?}: CreateAccessKey answered no AccessKeyId"));
        (arn, key)
    }

    /// Create a managed policy carrying `document` and set it as `user_name`'s permissions
    /// boundary — the ceiling IAM caps that user's identity-based grants with — answering
    /// the policy's ARN.
    async fn boundary(&self, case: &str, user_name: &str, document: &Value) -> String {
        let created = self
            .iam(
                case,
                "CreatePolicy",
                &[
                    ("PolicyName", format!("{user_name}-ceiling")),
                    ("PolicyDocument", document.to_string()),
                ],
            )
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("case {case:?}: CreatePolicy answered {created}"));
        self.iam(
            case,
            "PutUserPermissionsBoundary",
            &[
                ("UserName", user_name.to_string()),
                ("PermissionsBoundary", arn.clone()),
            ],
        )
        .await;
        arn
    }

    /// Create an IAM role, assume it through STS with `session_tags`, and answer
    /// `(the role's ARN, the session's access key id)`.
    async fn assumed_role(
        &self,
        case: &str,
        role_name: &str,
        session_tags: &[(&str, &str)],
    ) -> (String, String) {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{}:root", account())},
                // Passing session tags needs the trust policy to allow `sts:TagSession`
                // beside the assume itself.
                "Action": ["sts:AssumeRole", "sts:TagSession"],
            }],
        });
        let created = self
            .iam(
                case,
                "CreateRole",
                &[
                    ("RoleName", role_name.to_string()),
                    ("AssumeRolePolicyDocument", trust.to_string()),
                ],
            )
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("case {case:?}: CreateRole answered {created}"));
        let mut parameters = vec![
            ("RoleArn", arn.clone()),
            ("RoleSessionName", "probe".to_string()),
        ];
        for (index, (key, value)) in session_tags.iter().enumerate() {
            parameters.push((tag_key_parameter(index), (*key).to_string()));
            parameters.push((tag_value_parameter(index), (*value).to_string()));
        }
        let (status, body) = self
            .query("sts", "2011-06-15", "AssumeRole", &parameters)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected AssumeRole to succeed, got {status} {body}"
        );
        let key = xml_element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("case {case:?}: AssumeRole answered no AccessKeyId"));
        (arn, key)
    }
}

/// The awsQuery parameter naming the key of the `Tags` entry at `index`, which both
/// `iam:CreateUser` and `sts:AssumeRole` take as a 1-based `member` list.
fn tag_key_parameter(index: usize) -> &'static str {
    match index {
        0 => "Tags.member.1.Key",
        _ => "Tags.member.2.Key",
    }
}

/// The awsQuery parameter naming the value of the `Tags` entry at `index`.
fn tag_value_parameter(index: usize) -> &'static str {
    match index {
        0 => "Tags.member.1.Value",
        _ => "Tags.member.2.Value",
    }
}

/// A SigV4-shaped `Authorization` header: its credential scope names the service and
/// carries the access key id a service resolves the caller from.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0"
    )
}

/// A policy document carrying `statements`.
fn policy(statements: Value) -> Value {
    json!({"Version": "2012-10-17", "Statement": statements})
}

/// The refusal a request the policies in force do not allow carries, as IAM words it: the
/// principal, the action in the lower case this service names it in, the resource, and
/// `reason` — the fact about those policies that refused it, which names the document the
/// grant was missing from.
fn refusal_message(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         {reason}"
    )
}

/// The refusal a request no policy allows carries, as IAM words it: the principal, the
/// action in the lower case this service names it in, the resource, and the fact that no
/// policy in force allows the action.
fn implicit_denial(principal: &str, action: &str, resource: &str) -> String {
    refusal_message(
        principal,
        action,
        resource,
        &format!("because no identity-based policy allows the {action} action"),
    )
}

/// Assert that `reply` is the `AccessDenied` refusal at HTTP 403, and answer its message.
fn denial(case: &str, (status, body): (u16, Value)) -> String {
    assert_eq!(
        (status, error_code(&body).as_str()),
        (403, "AccessDenied"),
        "case {case:?}: expected a 403 AccessDenied, got {status} with {body}"
    );
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: a refusal must carry a message, got {body}"))
        .to_string()
}

/// One row of the grant table: which document is in force, and whether the caller's send is
/// allowed under it.
struct GrantRow {
    case: &'static str,
    /// The queue's own policy, built from the caller's ARN and the queue's ARN.
    queue_policy: fn(&str, &str) -> Option<Value>,
    /// The caller's inline identity policy, built from the queue's ARN.
    identity_policy: fn(&str) -> Option<Value>,
    /// The tags the calling principal carries, which is what `aws:PrincipalTag/<key>`
    /// answers for.
    tags: &'static [(&'static str, &'static str)],
    allowed: bool,
}

/// A send is allowed exactly when the policies in force allow `sqs:SendMessage` on that
/// queue for the calling principal, and refused `AccessDenied` 403 — with nothing enqueued
/// — otherwise.
#[tokio::test]
async fn a_send_is_allowed_only_where_the_policies_in_force_allow_it() {
    let rows: &[GrantRow] = &[
        GrantRow {
            case: "no policy at all",
            queue_policy: |_, _| None,
            identity_policy: |_| None,
            tags: &[],
            allowed: false,
        },
        GrantRow {
            case: "the queue's policy names the caller",
            queue_policy: |caller, queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Principal": {"AWS": caller},
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                }])))
            },
            identity_policy: |_| None,
            tags: &[],
            allowed: true,
        },
        GrantRow {
            case: "the queue's policy names every principal",
            queue_policy: |_, queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "sqs:*",
                    "Resource": queue,
                }])))
            },
            identity_policy: |_| None,
            tags: &[],
            allowed: true,
        },
        GrantRow {
            // A statement naming the ACCOUNT delegates to it rather than granting: the
            // principal still needs a permission of its own.
            case: "the queue's policy names the caller's account root",
            queue_policy: |_, queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Principal": {"AWS": format!("arn:aws:iam::{}:root", account())},
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                }])))
            },
            identity_policy: |_| None,
            tags: &[],
            allowed: false,
        },
        GrantRow {
            case: "the caller's own identity policy allows the action",
            queue_policy: |_, _| None,
            identity_policy: |queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                }])))
            },
            tags: &[],
            allowed: true,
        },
        GrantRow {
            case: "an explicit deny beside a grant of the same action",
            queue_policy: |caller, queue| {
                Some(policy(json!([
                    {
                        "Effect": "Allow",
                        "Principal": {"AWS": caller},
                        "Action": "sqs:SendMessage",
                        "Resource": queue,
                    },
                    {
                        "Effect": "Deny",
                        "Principal": "*",
                        "Action": "sqs:SendMessage",
                        "Resource": queue,
                    },
                ])))
            },
            identity_policy: |queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Action": "sqs:*",
                    "Resource": queue,
                }])))
            },
            tags: &[],
            allowed: false,
        },
        GrantRow {
            case: "a grant of another action only",
            queue_policy: |caller, queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Principal": {"AWS": caller},
                    "Action": "sqs:ReceiveMessage",
                    "Resource": queue,
                }])))
            },
            identity_policy: |_| None,
            tags: &[],
            allowed: false,
        },
        GrantRow {
            case: "a grant naming another queue as its resource",
            queue_policy: |caller, queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Principal": {"AWS": caller},
                    "Action": "sqs:SendMessage",
                    "Resource": format!("{queue}-elsewhere"),
                }])))
            },
            identity_policy: |_| None,
            tags: &[],
            allowed: false,
        },
        GrantRow {
            case: "a grant whose action pattern covers the action",
            queue_policy: |caller, queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Principal": {"AWS": caller},
                    "Action": "sqs:Send*",
                    "Resource": queue,
                }])))
            },
            identity_policy: |_| None,
            tags: &[],
            allowed: true,
        },
        GrantRow {
            case: "a grant under a principal-tag condition the caller satisfies",
            queue_policy: |caller, queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Principal": {"AWS": caller},
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                    "Condition": {"StringEquals": {"aws:PrincipalTag/team": "payments"}},
                }])))
            },
            identity_policy: |_| None,
            tags: &[("team", "payments")],
            allowed: true,
        },
        GrantRow {
            case: "a grant under a principal-tag condition the caller does not satisfy",
            queue_policy: |caller, queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Principal": {"AWS": caller},
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                    "Condition": {"StringEquals": {"aws:PrincipalTag/team": "payments"}},
                }])))
            },
            identity_policy: |_| None,
            tags: &[("team", "billing")],
            allowed: false,
        },
        GrantRow {
            // The idiom that gates an action on a principal carrying a tag AT ALL: IAM's
            // `Null` operator asks whether the key is present, and the gate knows every tag
            // the principal carries, so the caller that has one is not denied by it.
            case: "a deny of every principal carrying no tag, by a caller that carries one",
            queue_policy: |_, queue| {
                Some(policy(json!([
                    {
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "sqs:SendMessage",
                        "Resource": queue,
                    },
                    {
                        "Effect": "Deny",
                        "Principal": "*",
                        "Action": "sqs:SendMessage",
                        "Resource": queue,
                        "Condition": {"Null": {"aws:PrincipalTag/team": "true"}},
                    },
                ])))
            },
            identity_policy: |_| None,
            tags: &[("team", "payments")],
            allowed: true,
        },
        GrantRow {
            case: "the same deny, by a caller that carries no tag at all",
            queue_policy: |_, queue| {
                Some(policy(json!([
                    {
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "sqs:SendMessage",
                        "Resource": queue,
                    },
                    {
                        "Effect": "Deny",
                        "Principal": "*",
                        "Action": "sqs:SendMessage",
                        "Resource": queue,
                        "Condition": {"Null": {"aws:PrincipalTag/team": "true"}},
                    },
                ])))
            },
            identity_policy: |_| None,
            tags: &[],
            allowed: false,
        },
        GrantRow {
            // `aws:SourceArn` is set for a call another service makes on a resource's
            // behalf, so a DIRECT caller cannot satisfy it and the grant is not one.
            case: "a grant under a source-arn condition a direct caller cannot answer",
            queue_policy: |caller, queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Principal": {"AWS": caller},
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                    "Condition": {"ArnLike": {"aws:SourceArn": format!("arn:aws:sns:{REGION}:{}:*", account())}},
                }])))
            },
            identity_policy: |_| None,
            tags: &[],
            allowed: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("grant-{index}");
        let url = emulator.queue(row.case, &name).await;
        let arn = expected_arn(&name);
        let (caller, key) = emulator
            .user(
                row.case,
                &format!("sender-{index}"),
                (row.identity_policy)(&arn).as_ref(),
                row.tags,
            )
            .await;
        if let Some(document) = (row.queue_policy)(&caller, &arn) {
            emulator.set_policy(row.case, &url, &document).await;
        }

        let body = format!("body of {}", row.case);
        let reply = emulator
            .sqs(&key, "SendMessage", &json!({"QueueUrl": url, "MessageBody": body}))
            .await;
        if row.allowed {
            assert_eq!(
                reply.0, 200,
                "case {:?}: expected the send to be allowed, got {} with {}",
                row.case, reply.0, reply.1
            );
            assert!(
                reply.1["MessageId"].is_string(),
                "case {:?}: an allowed send must answer a MessageId, got {}",
                row.case,
                reply.1
            );
        } else {
            let message = denial(row.case, reply);
            assert!(
                message.contains(&caller) && message.contains(&arn),
                "case {:?}: the refusal must name the principal and the queue, got {message:?}",
                row.case
            );
            // Nothing was enqueued: the queue still holds no message.
            let attributes = emulator.attributes(row.case, &url).await;
            assert_eq!(
                attributes.get("ApproximateNumberOfMessages").map(String::as_str),
                Some("0"),
                "case {:?}: a refused send must enqueue nothing, got {attributes:?}",
                row.case
            );
        }
    }
}

/// One row of the permissions-boundary table: the documents in force on the caller, and
/// what the gate must answer for a send.
struct BoundaryRow {
    case: &'static str,
    /// The caller's inline identity policy, built from the queue's ARN.
    identity_policy: fn(&str) -> Option<Value>,
    /// The permissions boundary attached to the caller, built from the queue's ARN.
    boundary: fn(&str) -> Option<Value>,
    /// The clause the refusal must carry, or `None` when the send must be allowed.
    refusal: Option<&'static str>,
}

/// The permissions boundary attached to the calling principal CAPS what its identity-based
/// policies grant and grants nothing of its own: a send is allowed only where both allow
/// it, a boundary that withheld the grant is the document the refusal names, and an
/// explicit `Deny` in the boundary refuses whatever the identity policy says.
#[tokio::test]
async fn a_permissions_boundary_caps_an_identity_grant_and_never_grants_one() {
    /// The clause for a request the identity policies allowed and the boundary did not.
    const NO_BOUNDARY_ALLOW: &str =
        "because no permissions boundary allows the sqs:sendmessage action";
    /// The clause for a request nothing allowed, which a boundary-only permission is.
    const NO_IDENTITY_ALLOW: &str =
        "because no identity-based policy allows the sqs:sendmessage action";

    let rows: &[BoundaryRow] = &[
        BoundaryRow {
            case: "a boundary admitting the action the identity policy grants",
            identity_policy: |queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                }])))
            },
            boundary: |queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Action": "sqs:*",
                    "Resource": queue,
                }])))
            },
            refusal: None,
        },
        BoundaryRow {
            case: "no boundary at all",
            identity_policy: |queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                }])))
            },
            boundary: |_| None,
            refusal: None,
        },
        BoundaryRow {
            case: "a boundary admitting another action only",
            identity_policy: |queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                }])))
            },
            boundary: |queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Action": "sqs:ReceiveMessage",
                    "Resource": queue,
                }])))
            },
            refusal: Some(NO_BOUNDARY_ALLOW),
        },
        BoundaryRow {
            case: "a boundary admitting the action on another queue only",
            identity_policy: |queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                }])))
            },
            boundary: |_| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Action": "sqs:SendMessage",
                    "Resource": expected_arn("somewhere-else"),
                }])))
            },
            refusal: Some(NO_BOUNDARY_ALLOW),
        },
        BoundaryRow {
            case: "a boundary denying the action it otherwise admits",
            identity_policy: |queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                }])))
            },
            boundary: |queue| {
                Some(policy(json!([
                    {"Effect": "Allow", "Action": "sqs:*", "Resource": queue},
                    {"Effect": "Deny", "Action": "sqs:SendMessage", "Resource": queue},
                ])))
            },
            refusal: Some("with an explicit deny in a permissions boundary"),
        },
        BoundaryRow {
            // The boundary is no grant: with nothing else allowing the call, the refusal
            // names the identity-based policies rather than the boundary.
            case: "a boundary admitting the action with no identity grant at all",
            identity_policy: |_| None,
            boundary: |queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Action": "sqs:*",
                    "Resource": queue,
                }])))
            },
            refusal: Some(NO_IDENTITY_ALLOW),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("ceiling-{index}");
        let url = emulator.queue(row.case, &name).await;
        let arn = expected_arn(&name);
        let user_name = format!("bounded-{index}");
        let (caller, key) = emulator
            .user(
                row.case,
                &user_name,
                (row.identity_policy)(&arn).as_ref(),
                &[],
            )
            .await;
        if let Some(document) = (row.boundary)(&arn) {
            emulator.boundary(row.case, &user_name, &document).await;
        }

        let reply = emulator
            .sqs(
                &key,
                "SendMessage",
                &json!({"QueueUrl": url, "MessageBody": format!("body of {}", row.case)}),
            )
            .await;
        match row.refusal {
            None => {
                assert_eq!(
                    reply.0, 200,
                    "case {:?}: expected the send to be allowed, got {} with {}",
                    row.case, reply.0, reply.1
                );
                assert!(
                    reply.1["MessageId"].is_string(),
                    "case {:?}: an allowed send must answer a MessageId, got {}",
                    row.case,
                    reply.1
                );
            }
            Some(reason) => {
                let message = denial(row.case, reply);
                assert_eq!(
                    message,
                    refusal_message(&caller, "sqs:sendmessage", &arn, reason),
                    "case {:?}: expected the refusal to name the document that withheld \
                     the grant, got {message:?}",
                    row.case
                );
                // Nothing was enqueued: the queue still holds no message.
                let attributes = emulator.attributes(row.case, &url).await;
                assert_eq!(
                    attributes.get("ApproximateNumberOfMessages").map(String::as_str),
                    Some("0"),
                    "case {:?}: a refused send must enqueue nothing, got {attributes:?}",
                    row.case
                );
            }
        }
    }
}

/// One row of the per-operation table: the call, and the action a policy has to allow for
/// it.
struct OperationRow {
    case: &'static str,
    operation: &'static str,
    action: &'static str,
    /// The request body, built from the queue's url.
    request: fn(&str) -> Value,
}

/// Every operation that acts on a queue is judged under its OWN action: a policy granting
/// one of them does not authorize another, and the refusal names the action that operation
/// is decided under.
#[tokio::test]
async fn each_operation_is_judged_under_its_own_action() {
    let rows: &[OperationRow] = &[
        OperationRow {
            case: "a send",
            operation: "SendMessage",
            action: "sqs:sendmessage",
            request: |url| json!({"QueueUrl": url, "MessageBody": "judged"}),
        },
        OperationRow {
            // The batch call is decided under its single-message sibling's action.
            case: "a batched send",
            operation: "SendMessageBatch",
            action: "sqs:sendmessage",
            request: |url| {
                json!({"QueueUrl": url, "Entries": [{"Id": "one", "MessageBody": "judged"}]})
            },
        },
        OperationRow {
            case: "a receive",
            operation: "ReceiveMessage",
            action: "sqs:receivemessage",
            request: |url| json!({"QueueUrl": url}),
        },
        OperationRow {
            case: "a delete of a message",
            operation: "DeleteMessage",
            action: "sqs:deletemessage",
            request: |url| json!({"QueueUrl": url, "ReceiptHandle": "no-such-handle"}),
        },
        OperationRow {
            case: "a purge",
            operation: "PurgeQueue",
            action: "sqs:purgequeue",
            request: |url| json!({"QueueUrl": url}),
        },
        OperationRow {
            case: "a report of the queue's attributes",
            operation: "GetQueueAttributes",
            action: "sqs:getqueueattributes",
            request: |url| json!({"QueueUrl": url, "AttributeNames": ["All"]}),
        },
        OperationRow {
            case: "a change to the queue's attributes",
            operation: "SetQueueAttributes",
            action: "sqs:setqueueattributes",
            request: |url| json!({"QueueUrl": url, "Attributes": {"DelaySeconds": "5"}}),
        },
        OperationRow {
            case: "a report of the queue's tags",
            operation: "ListQueueTags",
            action: "sqs:listqueuetags",
            request: |url| json!({"QueueUrl": url}),
        },
        OperationRow {
            case: "a tagging",
            operation: "TagQueue",
            action: "sqs:tagqueue",
            request: |url| json!({"QueueUrl": url, "Tags": {"owner": "payments"}}),
        },
        OperationRow {
            case: "an untagging",
            operation: "UntagQueue",
            action: "sqs:untagqueue",
            request: |url| json!({"QueueUrl": url, "TagKeys": ["owner"]}),
        },
        OperationRow {
            case: "a permission added to the queue's policy",
            operation: "AddPermission",
            action: "sqs:addpermission",
            request: |url| {
                json!({
                    "QueueUrl": url,
                    "Label": "shared",
                    "AWSAccountIds": ["210987654321"],
                    "Actions": ["SendMessage"],
                })
            },
        },
        OperationRow {
            case: "a delete of the queue",
            operation: "DeleteQueue",
            action: "sqs:deletequeue",
            request: |url| json!({"QueueUrl": url}),
        },
    ];

    let emulator = Emulator::new();
    // ONE principal whose only permission is `sqs:GetQueueUrl`: every row below asks for a
    // different action, so none of them is the one it holds.
    let url_only = policy(json!([{
        "Effect": "Allow",
        "Action": "sqs:GetQueueUrl",
        "Resource": "*",
    }]));
    let (caller, key) = emulator
        .user("one action only", "url-reader", Some(&url_only), &[])
        .await;

    for (index, row) in rows.iter().enumerate() {
        let name = format!("by-action-{index}");
        let url = emulator.queue(row.case, &name).await;
        let arn = expected_arn(&name);
        let message = denial(
            row.case,
            emulator.sqs(&key, row.operation, &(row.request)(&url)).await,
        );
        assert_eq!(
            message,
            implicit_denial(&caller, row.action, &arn),
            "case {:?}: expected the refusal to name {}, got {message:?}",
            row.case,
            row.action
        );
        // The action it DOES hold is allowed on the same queue, so the refusals above are
        // about the action asked for and not about the caller.
        emulator
            .sqs_ok(row.case, &key, "GetQueueUrl", json!({"QueueName": name}))
            .await;
        // And the refused call had no effect: the queue is still there, still untagged and
        // still carrying the delay it was created with.
        let attributes = emulator.attributes(row.case, &url).await;
        assert_eq!(
            attributes.get("DelaySeconds").map(String::as_str),
            Some("0"),
            "case {:?}: a refused call must change nothing, got {attributes:?}",
            row.case
        );
    }
}

/// One row of the session table: the case, whether the statement names the ROLE rather than
/// the session, the tags the session carries, and whether the statement is conditioned on
/// one of them.
type SessionRow = (
    &'static str,
    bool,
    &'static [(&'static str, &'static str)],
    bool,
);

/// A principal behind an assumed-role session is named by the session's own ARN and by the
/// ARN of the role it is a session of, so a statement naming either grants it — and the
/// `aws:PrincipalTag/<key>` a statement conditions on is answered by the SESSION's tags.
#[tokio::test]
async fn a_session_is_granted_by_a_statement_naming_it_or_its_role() {
    let rows: &[SessionRow] = &[
        ("a statement naming the role", true, &[], false),
        ("a statement naming the session", false, &[], false),
        (
            "a statement conditioned on a session tag the session carries",
            true,
            &[("team", "payments")],
            true,
        ),
    ];
    for (index, (case, by_role, tags, conditioned)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("session-{index}");
        let url = emulator.queue(case, &name).await;
        let arn = expected_arn(&name);
        let (role_arn, key) = emulator
            .assumed_role(case, &format!("sender-role-{index}"), tags)
            .await;
        let session_arn = format!(
            "arn:aws:sts::{}:assumed-role/sender-role-{index}/probe",
            account()
        );
        let named = if *by_role { &role_arn } else { &session_arn };
        let mut statement = json!({
            "Effect": "Allow",
            "Principal": {"AWS": named},
            "Action": "sqs:SendMessage",
            "Resource": arn,
        });
        if *conditioned {
            statement["Condition"] =
                json!({"StringEquals": {"aws:PrincipalTag/team": "payments"}});
        }
        emulator
            .set_policy(case, &url, &policy(json!([statement])))
            .await;
        let (status, body) = emulator
            .sqs(&key, "SendMessage", &json!({"QueueUrl": url, "MessageBody": "from a session"}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a statement naming {named} must grant the session, got {status} {body}"
        );
    }
}

/// One row of the owner table: the case, the queue's policy built from its ARN, and
/// whether the owner's own send is allowed under it.
type OwnerRow = (&'static str, fn(&str) -> Option<Value>, bool);

/// The queue's OWNER — the account's own published credential, which holds no IAM policies —
/// keeps full control of the queue, and only an explicit `Deny` in the queue's policy
/// refuses it.
#[tokio::test]
async fn the_owner_is_refused_only_by_an_explicit_deny() {
    let rows: &[OwnerRow] = &[
        ("no policy at all", |_| None, true),
        (
            "a policy that grants somebody else",
            |queue| {
                Some(policy(json!([{
                    "Effect": "Allow",
                    "Principal": {"AWS": "arn:aws:iam::210987654321:root"},
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                }])))
            },
            true,
        ),
        (
            "an explicit deny of every principal",
            |queue| {
                Some(policy(json!([{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                }])))
            },
            false,
        ),
        (
            "an explicit deny of the owning account",
            |queue| {
                Some(policy(json!([{
                    "Effect": "Deny",
                    "Principal": {"AWS": account()},
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                }])))
            },
            false,
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, document, allowed)) in rows.iter().enumerate() {
        let name = format!("owner-{index}");
        let url = emulator.queue(case, &name).await;
        let arn = expected_arn(&name);
        if let Some(document) = document(&arn) {
            emulator.set_policy(case, &url, &document).await;
        }
        let reply = emulator
            .sqs(
                OWNER_KEY,
                "SendMessage",
                &json!({"QueueUrl": url, "MessageBody": "from the owner"}),
            )
            .await;
        if *allowed {
            assert_eq!(
                reply.0, 200,
                "case {case:?}: the owner's send must be allowed, got {} with {}",
                reply.0, reply.1
            );
        } else {
            let message = denial(case, reply);
            assert!(
                message.ends_with("with an explicit deny in a resource-based policy"),
                "case {case:?}: the refusal must be attributed to the queue's policy, got \
                 {message:?}"
            );
        }
    }
}

/// A `Policy` value is judged when it is WRITTEN: a document whose `Action` names something
/// outside this service's action set, or whose `Principal` names an identity of this account
/// that does not exist, is refused `InvalidAttributeValue` at 400 and the attribute is left
/// exactly as it was.
#[tokio::test]
async fn a_policy_document_is_validated_when_it_is_written() {
    let emulator = Emulator::new();
    // A principal that DOES exist, for the rows about a principal.
    let (existing, _) = emulator.user("an existing user", "exists", None, &[]).await;
    // (case, the document, accepted)
    let rows: &[(&str, Value, bool)] = &[
        (
            "an action of this service",
            policy(json!([{"Effect": "Allow", "Principal": "*", "Action": "sqs:SendMessage", "Resource": "*"}])),
            true,
        ),
        (
            "every action of this service",
            policy(json!([{"Effect": "Allow", "Principal": "*", "Action": "sqs:*", "Resource": "*"}])),
            true,
        ),
        (
            // Every action name of this service is one a policy may name; how far the grant
            // REACHES is the evaluator's question at call time.
            "an account-level action of this service",
            policy(json!([{"Effect": "Allow", "Principal": "*", "Action": "sqs:ListQueues", "Resource": "*"}])),
            true,
        ),
        (
            "an action that changes who may reach the queue",
            policy(json!([{"Effect": "Allow", "Principal": "*", "Action": "sqs:SetQueueAttributes", "Resource": "*"}])),
            true,
        ),
        (
            "a batch action, which its single-message sibling also grants",
            policy(json!([{"Effect": "Allow", "Principal": "*", "Action": "sqs:SendMessageBatch", "Resource": "*"}])),
            true,
        ),
        (
            "an action no operation of this service has",
            policy(json!([{"Effect": "Allow", "Principal": "*", "Action": "sqs:NotARealAction", "Resource": "*"}])),
            false,
        ),
        (
            "an action of another service",
            policy(json!([{"Effect": "Allow", "Principal": "*", "Action": "s3:GetObject", "Resource": "*"}])),
            false,
        ),
        (
            "one bad action among good ones",
            policy(json!([{
                "Effect": "Allow",
                "Principal": "*",
                "Action": ["sqs:SendMessage", "sqs:Nonsense"],
                "Resource": "*",
            }])),
            false,
        ),
        (
            "an action named under NotAction",
            policy(json!([{"Effect": "Deny", "Principal": "*", "NotAction": "sqs:Nonsense", "Resource": "*"}])),
            false,
        ),
        (
            "an existing principal of this account",
            policy(json!([{"Effect": "Allow", "Principal": {"AWS": existing.clone()}, "Action": "sqs:SendMessage", "Resource": "*"}])),
            true,
        ),
        (
            "a principal of this account that does not exist",
            policy(json!([{
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{}:user/nobody-at-all", account())},
                "Action": "sqs:SendMessage",
                "Resource": "*",
            }])),
            false,
        ),
        (
            "a role of this account that does not exist",
            policy(json!([{
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{}:role/nobody-at-all", account())},
                "Action": "sqs:SendMessage",
                "Resource": "*",
            }])),
            false,
        ),
        (
            // Another account's principals are not this account's to judge.
            "a principal of another account",
            policy(json!([{
                "Effect": "Allow",
                "Principal": {"AWS": "arn:aws:iam::210987654321:user/elsewhere"},
                "Action": "sqs:SendMessage",
                "Resource": "*",
            }])),
            true,
        ),
        (
            "an account root, a bare account id and a service principal",
            policy(json!([
                {"Effect": "Allow", "Principal": {"AWS": format!("arn:aws:iam::{}:root", account())}, "Action": "sqs:SendMessage", "Resource": "*"},
                {"Effect": "Allow", "Principal": {"AWS": "210987654321"}, "Action": "sqs:SendMessage", "Resource": "*"},
                {"Effect": "Allow", "Principal": {"Service": "events.amazonaws.com"}, "Action": "sqs:SendMessage", "Resource": "*"},
            ])),
            true,
        ),
        (
            // A document stating nothing grants nothing and is accepted, which is what
            // keeps the validation about the statements a document DOES state.
            "a document stating no statement at all",
            json!({"Version": "2012-10-17"}),
            true,
        ),
        (
            // A policy document is an object; a value that is some other JSON is not one.
            "a JSON value that is not an object",
            json!([{"Effect": "Allow", "Principal": "*", "Action": "sqs:SendMessage"}]),
            false,
        ),
        (
            // An INEFFECTIVE statement is still one a caller may store: it grants nothing
            // and refuses nothing, so nothing about it is a value this service rejects.
            "a statement that states no effect at all",
            policy(json!([{"Principal": "*", "Action": "sqs:SendMessage", "Resource": "*"}])),
            true,
        ),
        (
            "a statement naming no action at all",
            policy(json!([{"Effect": "Allow", "Principal": "*", "Resource": "*"}])),
            true,
        ),
        (
            // An element that IS stated has to name an action of this service, so one
            // naming none is refused where leaving it out is not.
            "a statement whose action element names nothing",
            policy(json!([{"Effect": "Allow", "Principal": "*", "Action": [], "Resource": "*"}])),
            false,
        ),
        (
            "a statement whose action element is not a name at all",
            policy(json!([{"Effect": "Allow", "Principal": "*", "Action": {"sqs": "SendMessage"}, "Resource": "*"}])),
            false,
        ),
    ];

    let name = "validated";
    let url = emulator.queue("the validated queue", name).await;
    // The document the queue carries before each row, which a refused row must leave alone.
    let held = policy(json!([{
        "Effect": "Allow",
        "Principal": {"AWS": existing},
        "Action": "sqs:ReceiveMessage",
        "Resource": expected_arn(name),
    }]));
    emulator.set_policy("the held policy", &url, &held).await;

    for (case, document, accepted) in rows {
        let (status, body) = emulator
            .sqs(
                OWNER_KEY,
                "SetQueueAttributes",
                &json!({"QueueUrl": url, "Attributes": {"Policy": document.to_string()}}),
            )
            .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the document to be stored, got {status} with {body}"
            );
            // Put the queue back to the document the next row is judged against.
            emulator.set_policy(case, &url, &held).await;
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidAttributeValue"),
                "case {case:?}: expected a 400 InvalidAttributeValue, got {status} with {body}"
            );
            let attributes = emulator.attributes(case, &url).await;
            assert_eq!(
                attributes.get("Policy").map(String::as_str),
                Some(held.to_string().as_str()),
                "case {case:?}: a refused document must leave the stored one alone"
            );
        }
    }
}

/// A stored `Policy` is canonical: a `Principal` naming a bare account id reads back as that
/// account's root ARN, a document already canonical reads back byte for byte, and an empty
/// value removes the attribute.
#[tokio::test]
async fn a_stored_policy_reads_back_canonically() {
    let emulator = Emulator::new();
    let other = "210987654321";
    let root = format!("arn:aws:iam::{other}:root");
    // (case, the document written, the principals it must read back as)
    let rows: &[(&str, Value, Vec<String>)] = &[
        (
            "a bare account id",
            policy(json!([{"Effect": "Allow", "Principal": {"AWS": other}, "Action": "sqs:SendMessage", "Resource": "*"}])),
            vec![root.clone()],
        ),
        (
            "a root ARN, already canonical",
            policy(json!([{"Effect": "Allow", "Principal": {"AWS": root.clone()}, "Action": "sqs:SendMessage", "Resource": "*"}])),
            vec![root.clone()],
        ),
        (
            "a list of both spellings",
            policy(json!([{
                "Effect": "Allow",
                "Principal": {"AWS": [other, format!("arn:aws:iam::{}:root", account())]},
                "Action": "sqs:SendMessage",
                "Resource": "*",
            }])),
            vec![root.clone(), format!("arn:aws:iam::{}:root", account())],
        ),
        (
            "a bare account id under NotPrincipal",
            policy(json!([{"Effect": "Deny", "NotPrincipal": {"AWS": other}, "Action": "sqs:SendMessage", "Resource": "*"}])),
            vec![root.clone()],
        ),
    ];

    for (index, (case, document, expected)) in rows.iter().enumerate() {
        let name = format!("canonical-{index}");
        let url = emulator.queue(case, &name).await;
        emulator.set_policy(case, &url, document).await;
        let attributes = emulator.attributes(case, &url).await;
        let stored: Value = serde_json::from_str(
            attributes
                .get("Policy")
                .unwrap_or_else(|| panic!("case {case:?}: no Policy in {attributes:?}")),
        )
        .unwrap_or_else(|_| panic!("case {case:?}: the stored policy must be a document"));
        let statement = &stored["Statement"][0];
        let named = statement
            .get("Principal")
            .or_else(|| statement.get("NotPrincipal"))
            .and_then(|principal| principal.get("AWS"))
            .unwrap_or_else(|| panic!("case {case:?}: no principal in {stored}"));
        let read_back: Vec<String> = match named {
            Value::String(one) => vec![one.clone()],
            Value::Array(many) => many
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect(),
            other => panic!("case {case:?}: an unexpected principal {other}"),
        };
        assert_eq!(
            &read_back, expected,
            "case {case:?}: expected the principals {expected:?}, got {read_back:?}"
        );

        // The empty value removes the attribute, which is how a queue stops carrying a
        // policy.
        emulator
            .sqs_ok(
                case,
                OWNER_KEY,
                "SetQueueAttributes",
                json!({"QueueUrl": url, "Attributes": {"Policy": ""}}),
            )
            .await;
        let attributes = emulator.attributes(case, &url).await;
        assert!(
            !attributes.contains_key("Policy"),
            "case {case:?}: a cleared policy must leave no Policy attribute, got {attributes:?}"
        );
    }
}

/// A refused `DeleteQueue` has no effect: the queue, and everything it holds, survives.
#[tokio::test]
async fn a_refused_delete_leaves_the_queue() {
    let emulator = Emulator::new();
    let name = "survivor";
    let url = emulator.queue("the surviving queue", name).await;
    emulator
        .sqs_ok(
            "the surviving queue",
            OWNER_KEY,
            "SendMessage",
            json!({"QueueUrl": url, "MessageBody": "kept"}),
        )
        .await;
    let (_, key) = emulator.user("a caller with no grant", "deleter", None, &[]).await;

    let message = denial(
        "a delete nothing allows",
        emulator.sqs(&key, "DeleteQueue", &json!({"QueueUrl": url})).await,
    );
    assert!(
        message.contains("sqs:deletequeue"),
        "case 'a delete nothing allows': the refusal must name the action, got {message:?}"
    );
    let attributes = emulator.attributes("the surviving queue", &url).await;
    assert_eq!(
        attributes.get("ApproximateNumberOfMessages").map(String::as_str),
        Some("1"),
        "case 'the surviving queue': the queue and its message must survive, got {attributes:?}"
    );
}
