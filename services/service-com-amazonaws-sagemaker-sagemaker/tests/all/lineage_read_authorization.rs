// SPDX-License-Identifier: CC0-1.0

//! WHO may read this service's ML Lineage entities, driven end to end over the real wire path
//! (SageMaker, IAM and STS mounted on ONE shared engine, as the production router mounts every
//! service), so what is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: a lineage read evaluates the identity-based IAM policies in force on
//! the principal its credentials name against `sagemaker:<OperationName>` BEFORE it reads
//! anything — a request no statement allows (IAM's default deny), or one an explicit `Deny`
//! covers, is answered HTTP 400 `AccessDeniedException` and reports nothing about the entity,
//! while a request a matching `Allow` covers reaches the operation's own answer.
//!
//! The role, its policies and the session live in the IAM and STS stores of the SAME shared
//! engine — the stores those services' own handlers write — so the documents under test are
//! the ones `iam:PutRolePolicy` stores and the principal a refusal blames is the one
//! `sts:AssumeRole` minted the credential for.
//!
//! The tables span what the rule generalises over: the lineage read (an artifact, an action, a
//! context, the lineage group they belong to, that group's policy, and the three listings that
//! name no resource of their own), the document in force (none at all, another action of this
//! service, another service's actions, an explicit deny beside an allow, an allow of exactly
//! the action, of the whole service and of everything, and an allow scoped to another
//! resource), and the principal (an assumed-role session and the account's own published
//! credential).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::model_artifact_admission::{error_code, message, str_field, ACCOUNT, REGION};

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIDEXAMPLE";

/// The role every session below is assumed from, and the session's own name.
const ROLE: &str = "lineage-reader";
const SESSION: &str = "lineage-session";

/// The name of the inline policy the role carries.
const POLICY_NAME: &str = "inline";

/// SageMaker, IAM and STS on ONE shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(Arc::clone(&engine)),
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

    /// One SageMaker call signed with `access_key_id` — what names the principal the request
    /// speaks for.
    async fn sagemaker_as(
        &self,
        access_key_id: &str,
        target: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential={access_key_id}/20260101/{REGION}/sagemaker/aws4_request, \
                     SignedHeaders=host, Signature=not-verified"
                ),
            )
            .body(bytes::Bytes::from(body.to_string()))
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

    /// One SageMaker call, as the account's own credential, that must be served.
    async fn sagemaker_ok(&self, case: &str, target: &str, body: &Value) -> Value {
        let (status, answered) = self.sagemaker_as(ACCOUNT_KEY_ID, target, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {target}, got {status} with {answered}"
        );
        answered
    }

    /// One `awsQuery` action of `service`, signed with the account's own credential.
    async fn query(
        &self,
        service: &str,
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
            .header("host", format!("{service}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential={ACCOUNT_KEY_ID}/20260101/{REGION}/{service}/aws4_request, \
                     SignedHeaders=host, Signature=not-verified"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// Hold the role every session is assumed from, trusting this account's own principals.
    async fn given_role(&self) {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateRole",
                &[
                    ("RoleName", ROLE.to_string()),
                    ("AssumeRolePolicyDocument", trust),
                ],
            )
            .await;
        assert_eq!(status, 200, "the role must be created, got {status} {body}");
    }

    /// Attach `document` to the role as its inline policy, replacing whatever it carried —
    /// what `iam:PutRolePolicy` does. `None` takes the policy off it again, as
    /// `iam:DeleteRolePolicy` does.
    async fn given_role_policy(&self, document: Option<&Value>) {
        let (action, parameters) = match document {
            Some(document) => (
                "PutRolePolicy",
                vec![
                    ("RoleName", ROLE.to_string()),
                    ("PolicyName", POLICY_NAME.to_string()),
                    ("PolicyDocument", document.to_string()),
                ],
            ),
            None => (
                "DeleteRolePolicy",
                vec![
                    ("RoleName", ROLE.to_string()),
                    ("PolicyName", POLICY_NAME.to_string()),
                ],
            ),
        };
        let (status, body) = self.query("iam", "2010-05-08", action, &parameters).await;
        // Taking a policy off a role that carries none is IAM's own not-found, and leaves the
        // role in exactly the state the row asked for: no policy at all.
        let accepted = status == 200 || (document.is_none() && status == 404);
        assert!(
            accepted,
            "the role's policy must be stored, got {status} {body}"
        );
    }

    /// `sts:AssumeRole` on the role, answering the access key id the session was minted
    /// with — which is what names the session as the principal of a later request.
    async fn session_key_id(&self) -> String {
        let (status, body) = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    ("RoleArn", format!("arn:aws:iam::{ACCOUNT}:role/{ROLE}")),
                    ("RoleSessionName", SESSION.to_string()),
                ],
            )
            .await;
        assert_eq!(status, 200, "the role must be assumable, got {status} {body}");
        element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("no <AccessKeyId> in {body}"))
            .to_string()
    }
}

/// Percent-encode one `application/x-www-form-urlencoded` component with the maintained
/// RFC 3986 encoder.
fn form_encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The text of the first `<name>` element of an XML body.
fn element<'body>(body: &'body str, name: &str) -> Option<&'body str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(&body[start..end])
}

/// An identity policy with one statement over `actions` and `resources`.
fn statement(effect: &str, actions: Value, resources: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": effect, "Action": actions, "Resource": resources }],
    })
}

/// The ARN of the Region's one default lineage group, which every lineage entity belongs to.
fn lineage_group_arn() -> String {
    format!(
        "arn:aws:sagemaker:{REGION}:{ACCOUNT}:lineage-group/sagemaker-default-lineage-group"
    )
}

/// One lineage read: the operation, the request that addresses the seeded entity, and the
/// resource ARN an allow has to be scoped to for it.
struct Read {
    operation: &'static str,
    body: Value,
    resource: String,
}

/// RULE: every lineage read is authorized against the caller's identity-based policies for
/// `sagemaker:<OperationName>` before it reports anything.
#[tokio::test]
async fn a_lineage_read_is_refused_to_a_principal_iam_does_not_allow() {
    let emulator = Emulator::new();
    emulator.given_role().await;
    let session = emulator.session_key_id().await;

    // The entities every row reads, seeded as the account's own credential — which this gate
    // does not bound, so the seeding is not what is under test.
    let artifact = emulator
        .sagemaker_ok(
            "the artifact every row reads",
            "CreateArtifact",
            &json!({
                "ArtifactType": "DataSet",
                "Source": { "SourceUri": "s3://lineage-bucket/dataset.csv" },
            }),
        )
        .await;
    let artifact_arn = str_field(&artifact, "ArtifactArn");
    let action = emulator
        .sagemaker_ok(
            "the action every row reads",
            "CreateAction",
            &json!({
                "ActionName": "lineage-action",
                "ActionType": "ModelDeployment",
                "Source": { "SourceUri": "https://example.test/deployments/1" },
            }),
        )
        .await;
    let action_arn = str_field(&action, "ActionArn");
    let context = emulator
        .sagemaker_ok(
            "the context every row reads",
            "CreateContext",
            &json!({
                "ContextName": "lineage-context",
                "ContextType": "Endpoint",
                "Source": { "SourceUri": "https://example.test/endpoints/1" },
            }),
        )
        .await;
    let context_arn = str_field(&context, "ContextArn");

    let reads = vec![
        Read {
            operation: "DescribeArtifact",
            body: json!({ "ArtifactArn": artifact_arn }),
            resource: artifact_arn.clone(),
        },
        Read {
            operation: "DescribeAction",
            body: json!({ "ActionName": "lineage-action" }),
            resource: action_arn.clone(),
        },
        Read {
            operation: "DescribeContext",
            body: json!({ "ContextName": "lineage-context" }),
            resource: context_arn.clone(),
        },
        Read {
            operation: "DescribeLineageGroup",
            body: json!({ "LineageGroupName": "sagemaker-default-lineage-group" }),
            resource: lineage_group_arn(),
        },
        Read {
            operation: "GetLineageGroupPolicy",
            body: json!({ "LineageGroupName": "sagemaker-default-lineage-group" }),
            resource: lineage_group_arn(),
        },
        // A listing names no resource of its own, so it is judged against the resource IAM
        // writes for such a request.
        Read {
            operation: "ListArtifacts",
            body: json!({}),
            resource: "*".to_string(),
        },
        Read {
            operation: "ListAssociations",
            body: json!({}),
            resource: "*".to_string(),
        },
        Read {
            operation: "ListLineageGroups",
            body: json!({}),
            resource: "*".to_string(),
        },
    ];

    for read in &reads {
        // (case, the document in force on the role, whether the read is served)
        let rows: Vec<(&str, Option<Value>, bool)> = vec![
            ("no policy at all", None, false),
            (
                "an allow of everything",
                Some(statement("Allow", json!("*"), json!("*"))),
                true,
            ),
            (
                "an allow of the whole service",
                Some(statement("Allow", json!("sagemaker:*"), json!("*"))),
                true,
            ),
            (
                "an allow of exactly the action on exactly the resource",
                Some(statement(
                    "Allow",
                    json!(format!("sagemaker:{}", read.operation)),
                    json!(read.resource),
                )),
                true,
            ),
            (
                "an allow of another action of this service",
                Some(statement("Allow", json!("sagemaker:CreateArtifact"), json!("*"))),
                false,
            ),
            (
                "an allow of another service's actions",
                Some(statement("Allow", json!("s3:*"), json!("*"))),
                false,
            ),
            (
                "an allow scoped to another resource",
                Some(statement(
                    "Allow",
                    json!(format!("sagemaker:{}", read.operation)),
                    json!(format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:artifact/elsewhere")),
                )),
                false,
            ),
            (
                "an explicit deny beside the allow",
                Some(json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        { "Effect": "Allow", "Action": "sagemaker:*", "Resource": "*" },
                        {
                            "Effect": "Deny",
                            "Action": format!("sagemaker:{}", read.operation),
                            "Resource": "*",
                        },
                    ],
                })),
                false,
            ),
        ];

        for (case, document, served) in &rows {
            let case = format!("{} / {case}", read.operation);
            emulator.given_role_policy(document.as_ref()).await;
            let (status, answered) = emulator
                .sagemaker_as(&session, read.operation, &read.body)
                .await;

            if *served {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a principal IAM allows must be served, got {status} with \
                     {answered}"
                );
                continue;
            }
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, "AccessDeniedException"),
                "case {case:?}: a principal IAM does not allow must be refused, got {status} \
                 with {answered}"
            );
            assert!(
                message(&answered).contains(&format!(
                    "is not authorized to perform: sagemaker:{}",
                    read.operation
                )),
                "case {case:?}: the refusal must name the action, got {:?}",
                message(&answered)
            );
            assert!(
                message(&answered).contains(ROLE),
                "case {case:?}: the refusal must name the principal, got {:?}",
                message(&answered)
            );
            // A refused read reports nothing about the entity: no member of the answer
            // carries it.
            assert!(
                answered.get("ArtifactArn").is_none()
                    && answered.get("ActionArn").is_none()
                    && answered.get("ContextArn").is_none()
                    && answered.get("LineageGroupArn").is_none(),
                "case {case:?}: a refused read must report no entity, got {answered}"
            );
        }
    }

    // The account's own published credential holds no IAM policies, so it is not bounded by
    // this gate — whatever the role's policy says.
    emulator.given_role_policy(None).await;
    for read in &reads {
        let (status, answered) = emulator
            .sagemaker_as(ACCOUNT_KEY_ID, read.operation, &read.body)
            .await;
        assert_eq!(
            status, 200,
            "case {} / \"the account's own credential\": expected the read to be served, got \
             {status} with {answered}",
            read.operation
        );
    }
}
