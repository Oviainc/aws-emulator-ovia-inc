// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListFindings`, driven over the wire.
//!
//! Every case here goes through the real router: an `http::Request` is dispatched to the
//! mounted services, so the restJson1 routing, the boundary constraint validation, the
//! handler and the response serialization all run exactly as they do in the emulator
//! binary. Nothing reaches into a `state` module directly — the resources a finding is
//! about are created through their OWN service's API (IAM's `CreateRole` /
//! `UpdateAssumeRolePolicy`, S3's `CreateBucket` / `PutBucketPolicy`) and the findings are
//! only believed as `ListFindings` reports them.
//!
//! RULE under test: an external access analyzer reports, for every resource of its own
//! account whose resource-based policy this emulator holds, one finding per principal
//! OUTSIDE that account that an `Allow` statement of that policy grants access to — marked
//! `isPublic` when the statement admits every principal with nothing in its `Condition`
//! binding the caller — and none for a statement that grants nothing or grants only inside
//! the account.
//!
//! The table spans the classes that rule generalises over: both resource types whose
//! policy this emulator holds (an IAM role's trust policy, an S3 bucket's bucket policy),
//! every principal form the IAM grammar keys (`AWS` as an account id, as an account root
//! ARN and as a role ARN, `Service`, `Federated`, the `*` wildcard and `NotPrincipal`),
//! `Allow` against `Deny`, an unscoped wildcard against a scoped one, one statement against
//! several, and an internal principal against an external one. The recorded
//! external-trust-is-reported and service-principal-trust-is-not cases are two of its rows.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_accessanalyzer_accessanalyzer::ops::types::{
    FINDING_SOURCE_TYPE_VALUES, FINDING_STATUS_VALUES, ORDER_BY_VALUES, RESOURCE_TYPE_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::is_timestamp;

/// The region every request is signed for; it is the region the analyzer's ARN carries.
const REGION: &str = "us-east-1";

/// An account that is not this emulator's own — the external one a finding reports.
const EXTERNAL_ACCOUNT: &str = "111111111111";

/// A second external account, so a case can report two findings that differ only in which
/// account they name.
const OTHER_ACCOUNT: &str = "222222222222";

/// The IAM action a role trust policy grants for a principal to be able to assume the role.
const ASSUME_ROLE: &str = "sts:AssumeRole";

/// The account every analyzer and every resource here belongs to: the emulator's own, which
/// is the zone of trust an account analyzer monitors.
fn own_account() -> String {
    engine::emulator_account_id().to_string()
}

/// One response, reduced to what the assertions read.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    fn code(&self) -> &str {
        self.error_code.as_deref().unwrap_or("")
    }

    /// A member of the response body, treating JSON `null` as absent the way an omitted
    /// optional member is.
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    /// The findings this reply reports, in the order it reports them.
    fn findings(&self) -> Vec<Value> {
        self.member("findings")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("expected a findings list in {:?}", self.body))
    }

    /// The `name` of the single `fieldList` entry a field validation refusal carries.
    fn field_name(&self) -> String {
        self.member("fieldList")
            .and_then(Value::as_array)
            .and_then(|fields| fields.first())
            .and_then(|field| field.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn message(&self) -> String {
        self.member("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// An emulator holding the analyzer service beside the services that own the resources it
/// analyzes: the policies are read out of IAM's and S3's own stores, so both have to be
/// mounted for a finding to exist at all.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Nothing here is signed: the `Authorization` header carries only the credential
        // scope, which is where a service reads the Region and the caller's access key id.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_accessanalyzer_accessanalyzer::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// One restJson1 call to the analyzer service.
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match body {
            Some(value) => serde_json::to_vec(&value).expect("the request body serializes"),
            None => Vec::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://access-analyzer.{REGION}.amazonaws.com{path}"))
            .header("host", format!("access-analyzer.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/access-analyzer/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let error_code = error_code.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// Create an analyzer and answer the ARN it reports, which every findings call is
    /// scoped by.
    async fn analyzer(&self, case: &str, name: &str, r#type: &str) -> String {
        let reply = self
            .send(
                "PUT",
                "/analyzer",
                Some(json!({"analyzerName": name, "type": r#type})),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: CreateAnalyzer must succeed, got {reply:?}"
        );
        reply
            .member("arn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected an arn in {:?}", reply.body))
            .to_string()
    }

    /// `POST /finding` with the given request body.
    async fn list_findings(&self, body: Value) -> Reply {
        self.send("POST", "/finding", Some(body)).await
    }

    /// One awsQuery call to IAM.
    async fn iam(&self, action: &str, params: &[(&str, String)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("https://iam.amazonaws.com/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iam/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// Create an IAM role carrying `trust` as its trust policy, and answer the ARN IAM
    /// minted for it.
    async fn create_role(&self, case: &str, name: &str, trust: &str) -> String {
        let (status, body) = self
            .iam(
                "CreateRole",
                &[
                    ("RoleName", name.to_string()),
                    ("AssumeRolePolicyDocument", trust.to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateRole must succeed, got {status} with {body}"
        );
        xml_member(&body, "Arn")
            .unwrap_or_else(|| panic!("case {case:?}: expected an Arn in {body}"))
    }

    /// Replace a role's trust policy through IAM's own `UpdateAssumeRolePolicy`.
    async fn update_trust(&self, case: &str, name: &str, trust: &str) {
        let (status, body) = self
            .iam(
                "UpdateAssumeRolePolicy",
                &[
                    ("RoleName", name.to_string()),
                    ("PolicyDocument", trust.to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateAssumeRolePolicy must succeed, got {status} with {body}"
        );
    }

    /// One S3 request, addressed path-style to the Regional endpoint.
    async fn s3(&self, method: &str, uri: &str, body: &[u8]) -> (u16, String) {
        let host = format!("s3.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/s3/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("a well-formed S3 request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// Create a bucket and put `policy` on it, and answer the bucket's ARN.
    async fn create_bucket(&self, case: &str, name: &str, policy: &str) -> String {
        let (status, body) = self.s3("PUT", &format!("/{name}"), b"").await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateBucket must succeed, got {status} with {body}"
        );
        let (status, body) = self
            .s3("PUT", &format!("/{name}?policy"), policy.as_bytes())
            .await;
        assert!(
            (200..300).contains(&status),
            "case {case:?}: PutBucketPolicy must succeed, got {status} with {body}"
        );
        format!("arn:aws:s3:::{name}")
    }
}

/// A form value encoded the way an SDK encodes one in an awsQuery body: every character
/// outside the unreserved set is percent-escaped.
fn form_encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The text of the first `<name>…</name>` element of an awsQuery XML response.
fn xml_member(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// A one-statement resource policy: `effect` to `principal` for `actions`, with an optional
/// `Resource` and `Condition`.
fn policy(
    effect: &str,
    principal: Value,
    actions: Value,
    resource: Option<&str>,
    condition: Option<Value>,
) -> String {
    let mut statement = json!({"Effect": effect, "Principal": principal, "Action": actions});
    if let Some(resource) = resource {
        statement["Resource"] = json!(resource);
    }
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    json!({"Version": "2012-10-17", "Statement": [statement]}).to_string()
}

/// A trust policy: `Allow` to `principal` for [`ASSUME_ROLE`].
fn trust(principal: Value) -> String {
    policy("Allow", principal, json!(ASSUME_ROLE), None, None)
}

/// What one expected finding is asserted on: the principal it reports (type and value),
/// whether the access is public, the actions it reports, the resource type and the
/// `sources` types it carries.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Expected {
    principal: (String, String),
    is_public: bool,
    actions: Vec<String>,
    resource_type: String,
    sources: Vec<String>,
}

impl Expected {
    fn role(kind: &str, value: impl Into<String>, is_public: bool, actions: &[&str]) -> Self {
        Self {
            principal: (kind.to_string(), value.into()),
            is_public,
            actions: actions.iter().map(|action| action.to_string()).collect(),
            resource_type: "AWS::IAM::Role".to_string(),
            sources: Vec::new(),
        }
    }

    fn bucket(kind: &str, value: impl Into<String>, actions: &[&str]) -> Self {
        Self {
            principal: (kind.to_string(), value.into()),
            is_public: false,
            actions: actions.iter().map(|action| action.to_string()).collect(),
            resource_type: "AWS::S3::Bucket".to_string(),
            // "It is populated for Amazon S3 bucket findings": the bucket's own policy
            // granted the access.
            sources: vec!["POLICY".to_string()],
        }
    }
}

/// One finding off the wire, reduced to the same shape.
fn reduce(finding: &Value) -> Expected {
    let principal: Vec<(String, String)> = finding
        .get("principal")
        .and_then(Value::as_object)
        .map(|map| {
            let mut entries: Vec<(String, String)> = map
                .iter()
                .map(|(kind, value)| (kind.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect();
            entries.sort();
            entries
        })
        .unwrap_or_default();
    assert!(
        principal.len() <= 1,
        "a finding reports one principal, got {finding:?}"
    );
    Expected {
        principal: principal
            .into_iter()
            .next()
            .unwrap_or_else(|| (String::new(), String::new())),
        is_public: finding
            .get("isPublic")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        actions: finding
            .get("action")
            .and_then(Value::as_array)
            .map(|actions| {
                actions
                    .iter()
                    .map(|action| action.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default(),
        resource_type: finding
            .get("resourceType")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        sources: finding
            .get("sources")
            .and_then(Value::as_array)
            .map(|sources| {
                sources
                    .iter()
                    .map(|source| {
                        source
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// A string member of a finding.
fn text(finding: &Value, name: &str) -> String {
    finding
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// RULE: the findings of an external access analyzer are the external access the account's
/// own resource-based policies grant.
///
/// Rows differ along the resource type whose policy carries the grant, the principal form
/// the statement names, the effect, the scoping of a wildcard and the number of statements;
/// the recorded cross-account-trust and service-principal-trust rows are the first two.
#[tokio::test]
async fn findings_are_the_external_access_the_accounts_policies_grant() {
    let own = own_account();
    let external_role = format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:role/peer");

    // (case, the resource the policy is attached to and its document, the findings expected)
    let cases: Vec<(&str, Resource, Vec<Expected>)> = vec![
        (
            "a role trust policy granting another account's root",
            Resource::Role(trust(
                json!({"AWS": format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:root")}),
            )),
            // The external entity the grant reaches is the ACCOUNT, which a finding names
            // by its id whichever of the two equivalent spellings the policy used.
            vec![Expected::role("AWS", EXTERNAL_ACCOUNT, false, &[ASSUME_ROLE])],
        ),
        (
            "a role trust policy granting only a service principal",
            Resource::Role(trust(json!({"Service": "lambda.amazonaws.com"}))),
            // A service acting on the account's own behalf is not an entity outside it.
            vec![],
        ),
        (
            "a role trust policy granting another account by id",
            Resource::Role(trust(json!({"AWS": EXTERNAL_ACCOUNT}))),
            vec![Expected::role("AWS", EXTERNAL_ACCOUNT, false, &[ASSUME_ROLE])],
        ),
        (
            "a role trust policy granting a role in another account",
            Resource::Role(trust(json!({"AWS": external_role.clone()}))),
            vec![Expected::role(
                "AWS",
                external_role.clone(),
                false,
                &[ASSUME_ROLE],
            )],
        ),
        (
            "a role trust policy granting a principal of this account",
            Resource::Role(trust(json!({"AWS": format!("arn:aws:iam::{own}:root")}))),
            vec![],
        ),
        (
            "a role trust policy granting a federated identity provider",
            Resource::Role(policy(
                "Allow",
                json!({"Federated": "accounts.google.com"}),
                json!("sts:AssumeRoleWithWebIdentity"),
                None,
                None,
            )),
            vec![Expected::role(
                "Federated",
                "accounts.google.com",
                false,
                &["sts:AssumeRoleWithWebIdentity"],
            )],
        ),
        (
            "a role trust policy granting every principal",
            Resource::Role(trust(json!("*"))),
            vec![Expected::role("AWS", "*", true, &[ASSUME_ROLE])],
        ),
        (
            "a role trust policy granting every principal under a condition",
            Resource::Role(policy(
                "Allow",
                json!("*"),
                json!(ASSUME_ROLE),
                None,
                Some(json!({"StringEquals": {"aws:PrincipalOrgID": "o-example"}})),
            )),
            // Every principal, but a condition binds the caller: reported, not public.
            vec![Expected::role("AWS", "*", false, &[ASSUME_ROLE])],
        ),
        (
            "a role trust policy excluding this account with NotPrincipal",
            Resource::Role(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "NotPrincipal": {"AWS": own.clone()},
                        "Action": ASSUME_ROLE,
                    }],
                })
                .to_string(),
            ),
            vec![Expected::role("AWS", "*", true, &[ASSUME_ROLE])],
        ),
        (
            "a role trust policy denying another account",
            Resource::Role(policy(
                "Deny",
                json!({"AWS": EXTERNAL_ACCOUNT}),
                json!(ASSUME_ROLE),
                None,
                None,
            )),
            vec![],
        ),
        (
            "a role trust policy granting two accounts in two statements",
            Resource::Role(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {
                            "Effect": "Allow",
                            "Principal": {"AWS": EXTERNAL_ACCOUNT},
                            "Action": ASSUME_ROLE,
                        },
                        {
                            "Effect": "Allow",
                            "Principal": {"AWS": OTHER_ACCOUNT},
                            "Action": ["sts:AssumeRole", "sts:TagSession"],
                        },
                    ],
                })
                .to_string(),
            ),
            vec![
                Expected::role("AWS", EXTERNAL_ACCOUNT, false, &[ASSUME_ROLE]),
                Expected::role(
                    "AWS",
                    OTHER_ACCOUNT,
                    false,
                    &["sts:AssumeRole", "sts:TagSession"],
                ),
            ],
        ),
        (
            "a bucket policy granting another account",
            Resource::Bucket(policy(
                "Allow",
                json!({"AWS": EXTERNAL_ACCOUNT}),
                json!("s3:GetObject"),
                Some("arn:aws:s3:::findings-bucket/*"),
                None,
            )),
            vec![Expected::bucket("AWS", EXTERNAL_ACCOUNT, &["s3:GetObject"])],
        ),
        (
            "a bucket policy granting this account",
            Resource::Bucket(policy(
                "Allow",
                json!({"AWS": format!("arn:aws:iam::{own}:root")}),
                json!("s3:GetObject"),
                Some("arn:aws:s3:::findings-bucket/*"),
                None,
            )),
            vec![],
        ),
    ];

    for (case, resource, expected) in cases {
        let emulator = Emulator::new();
        let analyzer = emulator.analyzer(case, "findings-analyzer", "ACCOUNT").await;
        let resource_arn = match &resource {
            Resource::Role(document) => {
                emulator
                    .create_role(case, "findings-role", document)
                    .await
            }
            Resource::Bucket(document) => {
                emulator
                    .create_bucket(case, "findings-bucket", document)
                    .await
            }
        };

        // Unfiltered, and filtered by the resource the way the recorded call filters: both
        // report the same findings, because the account holds one analyzable resource.
        for (scope, body) in [
            ("unfiltered", json!({"analyzerArn": analyzer})),
            (
                "filtered by resource",
                json!({
                    "analyzerArn": analyzer,
                    "filter": {"resource": {"eq": [resource_arn.clone()]}},
                }),
            ),
        ] {
            let reply = emulator.list_findings(body).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?} ({scope}): ListFindings must answer 200, got {reply:?}"
            );
            let findings = reply.findings();
            let mut reported: Vec<Expected> = findings.iter().map(reduce).collect();
            let mut wanted: Vec<Expected> = expected.clone();
            reported.sort();
            wanted.sort();
            assert_eq!(
                reported, wanted,
                "case {case:?} ({scope}): expected {wanted:?}, got {findings:?}"
            );

            for finding in &findings {
                assert_eq!(
                    text(finding, "resource"),
                    resource_arn,
                    "case {case:?} ({scope}): the finding must name the analyzed resource, \
                     got {finding:?}"
                );
                assert_eq!(
                    text(finding, "resourceOwnerAccount"),
                    own,
                    "case {case:?} ({scope}): the finding must name the owning account, \
                     got {finding:?}"
                );
                assert_eq!(
                    text(finding, "status"),
                    "ACTIVE",
                    "case {case:?} ({scope}): a reported finding is ACTIVE, got {finding:?}"
                );
                assert!(
                    FINDING_STATUS_VALUES.contains(&text(finding, "status").as_str()),
                    "case {case:?} ({scope}): the status must be modelled, got {finding:?}"
                );
                assert!(
                    RESOURCE_TYPE_VALUES.contains(&text(finding, "resourceType").as_str()),
                    "case {case:?} ({scope}): the resourceType must be modelled, got {finding:?}"
                );
                assert!(
                    !text(finding, "id").is_empty(),
                    "case {case:?} ({scope}): the finding must carry an id, got {finding:?}"
                );
                assert!(
                    finding.get("error").is_none() || finding["error"].is_null(),
                    "case {case:?} ({scope}): an analyzable policy reports no error, \
                     got {finding:?}"
                );
                for instant in ["createdAt", "analyzedAt", "updatedAt"] {
                    assert!(
                        is_timestamp(finding.get(instant)),
                        "case {case:?} ({scope}): {instant} must be an instant of this run, \
                         got {finding:?}"
                    );
                }
                assert!(
                    finding
                        .get("condition")
                        .is_some_and(|condition| condition.is_object()),
                    "case {case:?} ({scope}): condition is a required member, got {finding:?}"
                );
                for source in finding
                    .get("sources")
                    .and_then(Value::as_array)
                    .unwrap_or(&Vec::new())
                {
                    let kind = source.get("type").and_then(Value::as_str).unwrap_or_default();
                    assert!(
                        FINDING_SOURCE_TYPE_VALUES.contains(&kind),
                        "case {case:?} ({scope}): a source type must be modelled, got {source:?}"
                    );
                }
            }
        }
    }
}

/// Which resource a row attaches its policy to.
#[derive(Debug)]
enum Resource {
    /// An IAM role, whose trust policy is the document.
    Role(String),
    /// An S3 general purpose bucket, whose bucket policy is the document.
    Bucket(String),
}

/// The condition keys a statement constrains travel on the finding, so a filter can ask
/// about them: the member is "the condition in the analyzed policy statement that resulted
/// in a finding".
#[tokio::test]
async fn a_findings_condition_reports_the_statements_condition_keys() {
    let emulator = Emulator::new();
    let analyzer = emulator
        .analyzer("condition keys", "condition-analyzer", "ACCOUNT")
        .await;
    emulator
        .create_role(
            "condition keys",
            "condition-role",
            &policy(
                "Allow",
                json!({"AWS": EXTERNAL_ACCOUNT}),
                json!(ASSUME_ROLE),
                None,
                Some(json!({"StringEquals": {"sts:ExternalId": "shared-secret"}})),
            ),
        )
        .await;

    let reply = emulator
        .list_findings(json!({"analyzerArn": analyzer}))
        .await;
    assert_eq!(
        reply.status, 200,
        "case \"condition keys\": ListFindings must answer 200, got {reply:?}"
    );
    let findings = reply.findings();
    assert_eq!(
        findings.len(),
        1,
        "case \"condition keys\": expected one finding, got {findings:?}"
    );
    assert_eq!(
        findings[0].get("condition"),
        Some(&json!({"sts:ExternalId": "shared-secret"})),
        "case \"condition keys\": expected the statement's condition, got {:?}",
        findings[0]
    );
}

/// A read of the findings reports the same findings as the one before it: the analysis is a
/// function of the stored policies, so nothing about reading it moves.
///
/// And when the trust is NARROWED through IAM, the next read reports the access the policy
/// now grants — which for a trust that no longer names anyone outside the account is none.
#[tokio::test]
async fn findings_follow_the_stored_policy_and_are_stable_across_reads() {
    let own = own_account();
    let emulator = Emulator::new();
    let analyzer = emulator
        .analyzer("stability", "stable-analyzer", "ACCOUNT")
        .await;
    emulator
        .create_role(
            "stability",
            "stable-role",
            &trust(json!({"AWS": format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:root")})),
        )
        .await;

    let first = emulator
        .list_findings(json!({"analyzerArn": analyzer}))
        .await;
    let second = emulator
        .list_findings(json!({"analyzerArn": analyzer}))
        .await;
    assert_eq!(
        first.findings(),
        second.findings(),
        "case \"two reads\": a read must report the same findings as the one before it, \
         got {:?} then {:?}",
        first.findings(),
        second.findings()
    );
    assert_eq!(
        first.findings().len(),
        1,
        "case \"two reads\": expected one finding, got {:?}",
        first.findings()
    );

    // Narrow the trust to this account alone: the external access is gone, so no finding
    // reports it any more.
    emulator
        .update_trust(
            "narrowed",
            "stable-role",
            &trust(json!({"AWS": format!("arn:aws:iam::{own}:root")})),
        )
        .await;
    let narrowed = emulator
        .list_findings(json!({"analyzerArn": analyzer}))
        .await;
    assert_eq!(
        narrowed.findings(),
        Vec::<Value>::new(),
        "case \"narrowed\": a trust naming only this account grants no external access, \
         got {:?}",
        narrowed.findings()
    );
}

/// RULE: `filter` returns the findings every criterion holds for, and `sort` orders them by
/// one member of a finding.
///
/// Rows differ along the operator (`eq`, `neq`, `contains`, `exists`), the member it reads
/// (a scalar, a map addressed at one key, a boolean) and whether the criterion holds.
#[tokio::test]
async fn a_filter_selects_findings_and_a_sort_orders_them() {
    let emulator = Emulator::new();
    let analyzer = emulator
        .analyzer("filtering", "filter-analyzer", "ACCOUNT")
        .await;
    // Two roles, so a criterion can tell one finding from another: one grants an external
    // account, the other grants every principal.
    let named = emulator
        .create_role(
            "filtering",
            "filter-role-a",
            &trust(json!({"AWS": EXTERNAL_ACCOUNT})),
        )
        .await;
    let public = emulator
        .create_role("filtering", "filter-role-b", &trust(json!("*")))
        .await;

    for (case, filter, expected) in [
        (
            "eq on resource",
            json!({"resource": {"eq": [named.clone()]}}),
            vec![named.clone()],
        ),
        (
            "neq on resource",
            json!({"resource": {"neq": [named.clone()]}}),
            vec![public.clone()],
        ),
        (
            "contains on resource",
            json!({"resource": {"contains": ["filter-role-b"]}}),
            vec![public.clone()],
        ),
        (
            "eq on principal.AWS",
            json!({"principal.AWS": {"eq": [EXTERNAL_ACCOUNT]}}),
            vec![named.clone()],
        ),
        (
            "eq on isPublic",
            json!({"isPublic": {"eq": ["true"]}}),
            vec![public.clone()],
        ),
        (
            "exists on principal",
            json!({"principal": {"exists": true}}),
            vec![named.clone(), public.clone()],
        ),
        (
            "exists on error is false for an analyzable policy",
            json!({"error": {"exists": true}}),
            vec![],
        ),
        (
            "two criteria are a conjunction",
            json!({
                "resource": {"eq": [named.clone()]},
                "status": {"eq": ["ACTIVE"]},
            }),
            vec![named.clone()],
        ),
        (
            "a criterion nothing satisfies",
            json!({"resourceType": {"eq": ["AWS::SQS::Queue"]}}),
            vec![],
        ),
    ] {
        let reply = emulator
            .list_findings(json!({"analyzerArn": analyzer, "filter": filter}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: ListFindings must answer 200, got {reply:?}"
        );
        let reported: Vec<String> = reply
            .findings()
            .iter()
            .map(|finding| text(finding, "resource"))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected findings for {expected:?}, got {reported:?}"
        );
    }

    // `sort` orders by one member, in either direction the model's `OrderBy` lists.
    let ascending: Vec<String> = emulator
        .list_findings(json!({
            "analyzerArn": analyzer,
            "sort": {"attributeName": "resource", "orderBy": ORDER_BY_VALUES[0]},
        }))
        .await
        .findings()
        .iter()
        .map(|finding| text(finding, "resource"))
        .collect();
    let descending: Vec<String> = emulator
        .list_findings(json!({
            "analyzerArn": analyzer,
            "sort": {"attributeName": "resource", "orderBy": ORDER_BY_VALUES[1]},
        }))
        .await
        .findings()
        .iter()
        .map(|finding| text(finding, "resource"))
        .collect();
    assert_eq!(
        ascending,
        vec![named.clone(), public.clone()],
        "case \"sort ASC\": expected the findings in resource order, got {ascending:?}"
    );
    let mut reversed = ascending.clone();
    reversed.reverse();
    assert_eq!(
        descending, reversed,
        "case \"sort DESC\": expected the reverse of {ascending:?}, got {descending:?}"
    );
}

/// RULE: `maxResults`/`nextToken` page over the findings, so a caller that follows the
/// tokens to exhaustion sees every finding exactly once and the last page carries no token.
///
/// Rows differ along the page size, from one finding per page to more than the result holds.
#[tokio::test]
async fn pagination_walks_every_finding_exactly_once() {
    let emulator = Emulator::new();
    let analyzer = emulator
        .analyzer("pagination", "paging-analyzer", "ACCOUNT")
        .await;
    for (index, account) in [EXTERNAL_ACCOUNT, OTHER_ACCOUNT, "333333333333"]
        .iter()
        .enumerate()
    {
        emulator
            .create_role(
                "pagination",
                &format!("paging-role-{index}"),
                &trust(json!({"AWS": *account})),
            )
            .await;
    }

    let whole: Vec<String> = emulator
        .list_findings(json!({"analyzerArn": analyzer}))
        .await
        .findings()
        .iter()
        .map(|finding| text(finding, "id"))
        .collect();
    assert_eq!(
        whole.len(),
        3,
        "case \"the whole result\": expected three findings, got {whole:?}"
    );

    for page_size in [1, 2, 3, 10] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({"analyzerArn": analyzer, "maxResults": page_size});
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let reply = emulator.list_findings(body).await;
            assert_eq!(
                reply.status, 200,
                "case \"maxResults={page_size}\": ListFindings must answer 200, got {reply:?}"
            );
            let findings = reply.findings();
            assert!(
                findings.len() <= page_size as usize,
                "case \"maxResults={page_size}\": a page carries at most {page_size} findings, \
                 got {findings:?}"
            );
            walked.extend(findings.iter().map(|finding| text(finding, "id")));
            token = reply
                .member("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= 8,
                "case \"maxResults={page_size}\": the walk must terminate, got {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, whole,
            "case \"maxResults={page_size}\": the walk must report every finding exactly once"
        );
    }
}

/// RULE: a request this operation cannot answer is refused with the modelled error for what
/// is wrong with it, never with an empty success.
///
/// Rows differ along what is wrong: the analyzer, its family, the filter, the sort and the
/// paging members.
#[tokio::test]
async fn refusals_name_what_the_caller_has_to_change() {
    let emulator = Emulator::new();
    let external = emulator
        .analyzer("refusals", "refusal-analyzer", "ACCOUNT")
        .await;
    let unused = emulator
        .analyzer("refusals", "refusal-unused", "ACCOUNT_UNUSED_ACCESS")
        .await;
    let missing = format!(
        "arn:aws:access-analyzer:{REGION}:{}:analyzer/never-created",
        own_account()
    );

    for (case, body, status, code, field) in [
        (
            "an analyzer this account never created",
            json!({"analyzerArn": missing}),
            404,
            "ResourceNotFoundException",
            None,
        ),
        (
            "an unused access analyzer",
            json!({"analyzerArn": unused}),
            400,
            "ValidationException",
            None,
        ),
        (
            "a filter key naming no member of a finding",
            json!({"analyzerArn": external, "filter": {"notAMember": {"eq": ["x"]}}}),
            400,
            "ValidationException",
            Some("filter"),
        ),
        (
            "a sort attribute naming no member of a finding",
            json!({"analyzerArn": external, "sort": {"attributeName": "notAMember"}}),
            400,
            "ValidationException",
            Some("sort.attributeName"),
        ),
        (
            "an order the model does not list",
            json!({
                "analyzerArn": external,
                "sort": {"attributeName": "resource", "orderBy": "SIDEWAYS"},
            }),
            400,
            "ValidationException",
            // `orderBy` is a modelled enum, so the boundary refuses a value outside it
            // before the handler runs: that refusal names the member in its message rather
            // than in a `fieldList`.
            None,
        ),
        (
            "a page size below one",
            json!({"analyzerArn": external, "maxResults": 0}),
            400,
            "ValidationException",
            Some("maxResults"),
        ),
        (
            "a token this service never issued",
            json!({"analyzerArn": external, "nextToken": "not-a-token"}),
            400,
            "ValidationException",
            Some("nextToken"),
        ),
    ] {
        let reply = emulator.list_findings(body).await;
        assert_eq!(
            (reply.status, reply.code()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {reply:?}"
        );
        if let Some(field) = field {
            assert_eq!(
                reply.field_name(),
                field,
                "case {case:?}: the refusal must name {field:?}, got {reply:?}"
            );
        }
    }

    // The family refusal is the service's fixed text for an action the resource does not
    // support: it names neither the operation nor the analyzer.
    let refused = emulator
        .list_findings(json!({"analyzerArn": unused}))
        .await;
    assert_eq!(
        refused.message(),
        "Unsupported operation",
        "case \"an unused access analyzer\": expected the fixed unsupported-action text, \
         got {refused:?}"
    );
}
