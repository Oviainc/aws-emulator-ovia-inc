// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetAnalyzedResource`, driven over the wire.
//!
//! Every case here goes through the real router: an `http::Request` is dispatched to the
//! mounted services, so the restJson1 routing, the boundary constraint validation, the
//! handler and the response serialization all run exactly as they do in the emulator
//! binary. Nothing reaches into a `state` module directly — the resource the record is
//! about is created through its OWN service's API (IAM's `CreateRole` /
//! `UpdateAssumeRolePolicy`, S3's `CreateBucket` / `PutBucketPolicy`) and the record is
//! only believed as `GetAnalyzedResource` reports it.
//!
//! RULE under test: `GetAnalyzedResource` reports what an external access analyzer found
//! about ONE resource of its own account whose resource-based policy this emulator holds —
//! the resource itself, with its external-access findings aggregated into one record whose
//! access is public when any of them is, whose `actions` are every action they report and
//! whose `sharedVia` is how they say the access is granted — carrying none of those finding
//! members when the policy grants nothing outside the account, and no record at all for a
//! resource the analyzer analyzed nothing for.
//!
//! The table spans the classes that rule generalises over: both resource types whose policy
//! this emulator holds (an IAM role's trust policy, an S3 bucket's bucket policy), the
//! principal forms the IAM grammar keys (an account root, an account id, a role ARN, a
//! service principal, a federated provider, the `*` wildcard), `Allow` against `Deny`, an
//! unscoped wildcard against a scoped one, one statement against several, and a principal
//! inside the account against one outside it. The recorded service-principal-trust and
//! trust-narrowed cases are rows of it.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_accessanalyzer_accessanalyzer::ops::types::{
    FINDING_STATUS_VALUES, RESOURCE_TYPE_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{is_timestamp, label};

/// The region every request is signed for; it is the region the analyzer's ARN carries.
const REGION: &str = "us-east-1";

/// An account that is not this emulator's own — the external one a finding reports.
const EXTERNAL_ACCOUNT: &str = "111111111111";

/// A second external account, so a row can grant two accounts in two statements.
const OTHER_ACCOUNT: &str = "222222222222";

/// The IAM action a role trust policy grants for a principal to be able to assume the role.
const ASSUME_ROLE: &str = "sts:AssumeRole";

/// The `FindingStatus` a record aggregating findings carries: the status of those findings.
const ACTIVE: &str = "ACTIVE";

/// The `ResourceType` of an IAM role, from the model's `ResourceType` enum.
const ROLE_TYPE: &str = "AWS::IAM::Role";

/// The `ResourceType` of an Amazon S3 general purpose bucket, from the same enum.
const BUCKET_TYPE: &str = "AWS::S3::Bucket";

/// The account every analyzer and every resource here belongs to: the emulator's own, which
/// is the zone of trust an account external access analyzer monitors.
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

    /// The `resource` record this reply reports, or `None` when it reports none.
    fn resource(&self) -> Option<&Value> {
        self.member("resource")
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
/// mounted for a record to exist at all.
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

    /// Create an analyzer and answer the ARN it reports, which every read is scoped by.
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

    /// `GET /analyzed-resource?analyzerArn=…&resourceArn=…`, both members `@httpQuery`.
    async fn get_analyzed_resource(&self, analyzer_arn: &str, resource_arn: &str) -> Reply {
        self.send(
            "GET",
            &format!(
                "/analyzed-resource?analyzerArn={}&resourceArn={}",
                label(analyzer_arn),
                label(resource_arn)
            ),
            None,
        )
        .await
    }

    /// One awsQuery call to IAM.
    async fn iam(&self, action: &str, params: &[(&str, String)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&label(value));
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

/// Which resource a row attaches its policy to.
#[derive(Debug)]
enum Resource {
    /// An IAM role, whose trust policy is the document.
    Role(String),
    /// An S3 general purpose bucket, whose bucket policy is the document.
    Bucket(String),
}

/// What one expected record is asserted on: whether the access it reports is public, the
/// actions it reports, how the access is shared, the status of the findings it aggregates
/// (absent when it aggregates none) and the resource type.
#[derive(Debug, PartialEq, Eq)]
struct Expected {
    is_public: bool,
    actions: Vec<String>,
    shared_via: Vec<String>,
    status: Option<String>,
    resource_type: String,
}

impl Expected {
    /// A role whose trust policy grants the external access its findings report.
    fn role(is_public: bool, actions: &[&str]) -> Self {
        Self {
            is_public,
            actions: actions.iter().map(|action| action.to_string()).collect(),
            // "This is populated for Amazon S3 bucket findings": a role's record says
            // nothing about how the access is shared.
            shared_via: Vec::new(),
            status: Some(ACTIVE.to_string()),
            resource_type: ROLE_TYPE.to_string(),
        }
    }

    /// A role that was analyzed and grants no access outside the account: reported, with
    /// none of the members that speak about a finding.
    fn analyzed_role() -> Self {
        Self {
            is_public: false,
            actions: Vec::new(),
            shared_via: Vec::new(),
            status: None,
            resource_type: ROLE_TYPE.to_string(),
        }
    }

    /// A bucket whose bucket policy grants the external access its findings report.
    fn bucket(actions: &[&str]) -> Self {
        Self {
            is_public: false,
            actions: actions.iter().map(|action| action.to_string()).collect(),
            // The bucket's own policy granted the access, which is the only policy of a
            // bucket this analysis reads.
            shared_via: vec!["POLICY".to_string()],
            status: Some(ACTIVE.to_string()),
            resource_type: BUCKET_TYPE.to_string(),
        }
    }

    /// A bucket that was analyzed and grants no access outside the account.
    fn analyzed_bucket() -> Self {
        Self {
            is_public: false,
            actions: Vec::new(),
            shared_via: Vec::new(),
            status: None,
            resource_type: BUCKET_TYPE.to_string(),
        }
    }
}

/// One record off the wire, reduced to the same shape. The lists are sorted: what the rule
/// states about them is WHICH actions and sharings the record reports, not their order.
fn reduce(record: &Value) -> Expected {
    fn sorted(record: &Value, member: &str) -> Vec<String> {
        let mut values: Vec<String> = record
            .get(member)
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .map(|value| value.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        values.sort();
        values
    }
    Expected {
        is_public: record
            .get("isPublic")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| panic!("isPublic is a required member, got {record:?}")),
        actions: sorted(record, "actions"),
        shared_via: sorted(record, "sharedVia"),
        status: record
            .get("status")
            .filter(|status| !status.is_null())
            .map(|status| status.as_str().unwrap_or_default().to_string()),
        resource_type: text(record, "resourceType"),
    }
}

/// A string member of a record.
fn text(record: &Value, name: &str) -> String {
    record
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Everything a reported record must satisfy whatever the policy behind it says.
fn assert_well_formed(case: &str, record: &Value, resource_arn: &str, own: &str) {
    assert_eq!(
        text(record, "resourceArn"),
        resource_arn,
        "case {case:?}: the record must name the analyzed resource, got {record:?}"
    );
    assert_eq!(
        text(record, "resourceOwnerAccount"),
        own,
        "case {case:?}: the record must name the owning account, got {record:?}"
    );
    if let Some(status) = record.get("status").filter(|status| !status.is_null()) {
        assert!(
            FINDING_STATUS_VALUES.contains(&status.as_str().unwrap_or_default()),
            "case {case:?}: the status must be modelled, got {record:?}"
        );
    }
    assert!(
        RESOURCE_TYPE_VALUES.contains(&text(record, "resourceType").as_str()),
        "case {case:?}: the resourceType must be modelled, got {record:?}"
    );
    assert!(
        record.get("error").is_none() || record["error"].is_null(),
        "case {case:?}: an analyzable policy reports no error, got {record:?}"
    );
    for instant in ["createdAt", "analyzedAt", "updatedAt"] {
        assert!(
            is_timestamp(record.get(instant)),
            "case {case:?}: {instant} must be an instant of this run, got {record:?}"
        );
    }
}

/// RULE: the record reports the external access the resource's own policy grants, and a
/// resource whose policy grants none is reported as granting none.
///
/// Rows differ along the resource type whose policy carries the grant, the principal form
/// the statement names, the effect, the scoping of a wildcard and the number of statements;
/// the recorded service-principal-trust row is one of them.
#[tokio::test]
async fn an_analyzed_resource_is_the_external_access_its_policy_grants() {
    let own = own_account();
    let external_role = format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:role/peer");

    // (case, the resource the policy is attached to and its document, the record expected)
    let cases: Vec<(&str, Resource, Expected)> = vec![
        (
            "a role trust policy granting another account's root",
            Resource::Role(trust(
                json!({"AWS": format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:root")}),
            )),
            Expected::role(false, &[ASSUME_ROLE]),
        ),
        (
            "a role trust policy granting only a service principal",
            Resource::Role(trust(json!({"Service": "lambda.amazonaws.com"}))),
            // A service acting on the account's own behalf is not an entity outside it, so
            // the analyzer found no external access to report about the role.
            Expected::analyzed_role(),
        ),
        (
            "a role trust policy granting another account by id",
            Resource::Role(trust(json!({"AWS": EXTERNAL_ACCOUNT}))),
            Expected::role(false, &[ASSUME_ROLE]),
        ),
        (
            "a role trust policy granting a role in another account",
            Resource::Role(trust(json!({"AWS": external_role.clone()}))),
            Expected::role(false, &[ASSUME_ROLE]),
        ),
        (
            "a role trust policy granting a principal of this account",
            Resource::Role(trust(json!({"AWS": format!("arn:aws:iam::{own}:root")}))),
            Expected::analyzed_role(),
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
            Expected::role(false, &["sts:AssumeRoleWithWebIdentity"]),
        ),
        (
            "a role trust policy granting every principal",
            Resource::Role(trust(json!("*"))),
            Expected::role(true, &[ASSUME_ROLE]),
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
            Expected::role(false, &[ASSUME_ROLE]),
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
            Expected::analyzed_role(),
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
            // ONE record for the resource, carrying every action its findings report, each
            // once.
            Expected::role(false, &["sts:AssumeRole", "sts:TagSession"]),
        ),
        (
            "a role trust policy granting one account publicly and one by name",
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
                            "Principal": "*",
                            "Action": ASSUME_ROLE,
                        },
                    ],
                })
                .to_string(),
            ),
            // The access the resource grants is public when ANY of its findings is.
            Expected::role(true, &[ASSUME_ROLE]),
        ),
        (
            "a bucket policy granting another account",
            Resource::Bucket(policy(
                "Allow",
                json!({"AWS": EXTERNAL_ACCOUNT}),
                json!("s3:GetObject"),
                Some("arn:aws:s3:::analyzed-bucket/*"),
                None,
            )),
            Expected::bucket(&["s3:GetObject"]),
        ),
        (
            "a bucket policy granting this account",
            Resource::Bucket(policy(
                "Allow",
                json!({"AWS": format!("arn:aws:iam::{own}:root")}),
                json!("s3:GetObject"),
                Some("arn:aws:s3:::analyzed-bucket/*"),
                None,
            )),
            Expected::analyzed_bucket(),
        ),
    ];

    for (case, resource, expected) in cases {
        let emulator = Emulator::new();
        let analyzer = emulator.analyzer(case, "analyzed-analyzer", "ACCOUNT").await;
        let resource_arn = match &resource {
            Resource::Role(document) => {
                emulator
                    .create_role(case, "analyzed-role", document)
                    .await
            }
            Resource::Bucket(document) => {
                emulator
                    .create_bucket(case, "analyzed-bucket", document)
                    .await
            }
        };

        let reply = emulator
            .get_analyzed_resource(&analyzer, &resource_arn)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: GetAnalyzedResource must answer 200, got {reply:?}"
        );
        let record = reply.resource().unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a record for the analyzed resource, got {:?}",
                reply.body
            )
        });
        assert_eq!(
            reduce(record),
            expected,
            "case {case:?}: expected {expected:?}, got {record:?}"
        );
        assert_well_formed(case, record, &resource_arn, &own);
    }
}

/// RULE (its other half): a resource the analyzer analyzed NOTHING for is not reported at
/// all, whether the account holds no resource-based policy for it or it belongs to another
/// account.
///
/// Rows differ along WHY nothing was analyzed: a role of this account that was never
/// created, a bucket that was never created, a resource of another account, and a resource
/// type whose policy no service of this emulator holds. The account holds a reported
/// resource throughout, so what is asserted is that the answer is about the resource asked
/// for.
#[tokio::test]
async fn a_resource_the_analyzer_never_analyzed_is_not_reported() {
    let own = own_account();
    let emulator = Emulator::new();
    let analyzer = emulator.analyzer("nothing", "nothing-analyzer", "ACCOUNT").await;
    let reported = emulator
        .create_role(
            "nothing",
            "reported-role",
            &trust(json!({"AWS": EXTERNAL_ACCOUNT})),
        )
        .await;

    // The analyzer does report the account's one analyzable resource, so a missing record
    // below is about the resource asked for and not about an analyzer reporting nothing.
    let reply = emulator.get_analyzed_resource(&analyzer, &reported).await;
    assert!(
        reply.resource().is_some(),
        "case \"the reported resource\": expected a record, got {reply:?}"
    );

    for (case, resource_arn) in [
        (
            "a role of this account that was never created",
            format!("arn:aws:iam::{own}:role/never-created"),
        ),
        (
            "a bucket that was never created",
            "arn:aws:s3:::never-created".to_string(),
        ),
        (
            "a role of another account",
            format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:role/peer"),
        ),
        (
            "a resource type no service of this emulator holds a policy for",
            format!("arn:aws:sqs:{REGION}:{own}:never-created"),
        ),
    ] {
        let reply = emulator.get_analyzed_resource(&analyzer, &resource_arn).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: GetAnalyzedResource must answer 200, got {reply:?}"
        );
        assert!(
            reply.resource().is_none(),
            "case {case:?}: the analyzer analyzed no {resource_arn}, so no record is \
             reported, got {reply:?}"
        );
    }
}

/// RULE: the record is a function of the stored policy, so a read never changes what a later
/// read reports, and narrowing or widening the policy through its owning service is what
/// moves it.
///
/// The recorded trust-narrowed case is the middle step: a trust that no longer names anyone
/// outside the account grants no external access, so the analyzer has nothing to report
/// about the role.
#[tokio::test]
async fn the_record_follows_the_stored_policy_and_is_stable_across_reads() {
    let own = own_account();
    let emulator = Emulator::new();
    let analyzer = emulator.analyzer("stability", "stable-analyzer", "ACCOUNT").await;
    let role = emulator
        .create_role(
            "stability",
            "stable-role",
            &trust(json!({"AWS": format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:root")})),
        )
        .await;

    let first = emulator.get_analyzed_resource(&analyzer, &role).await;
    let second = emulator.get_analyzed_resource(&analyzer, &role).await;
    assert_eq!(
        first.resource(),
        second.resource(),
        "case \"two reads\": a read must report the same record as the one before it, got \
         {first:?} then {second:?}"
    );
    let record = first
        .resource()
        .unwrap_or_else(|| panic!("case \"two reads\": expected a record, got {first:?}"));
    assert_eq!(
        reduce(record),
        Expected::role(false, &[ASSUME_ROLE]),
        "case \"two reads\": expected the external trust the policy grants, got {record:?}"
    );

    // Narrow the trust to this account alone: the external access is gone, so the role is
    // reported as the analyzed resource it is, with nothing said about a finding.
    emulator
        .update_trust(
            "narrowed",
            "stable-role",
            &trust(json!({"AWS": format!("arn:aws:iam::{own}:root")})),
        )
        .await;
    let narrowed = emulator.get_analyzed_resource(&analyzer, &role).await;
    assert_eq!(
        narrowed.status, 200,
        "case \"narrowed\": GetAnalyzedResource must answer 200, got {narrowed:?}"
    );
    let narrowed_record = narrowed
        .resource()
        .unwrap_or_else(|| panic!("case \"narrowed\": expected a record, got {narrowed:?}"));
    assert_eq!(
        reduce(narrowed_record),
        Expected::analyzed_role(),
        "case \"narrowed\": a trust naming only this account grants no external access, got \
         {narrowed_record:?}"
    );

    // Widen it again: the record follows the policy back.
    emulator
        .update_trust("widened", "stable-role", &trust(json!("*")))
        .await;
    let widened = emulator.get_analyzed_resource(&analyzer, &role).await;
    let widened_record = widened
        .resource()
        .unwrap_or_else(|| panic!("case \"widened\": expected a record, got {widened:?}"));
    assert_eq!(
        reduce(widened_record),
        Expected::role(true, &[ASSUME_ROLE]),
        "case \"widened\": a trust granting every principal is public access, got \
         {widened_record:?}"
    );
}

/// RULE: a request this operation cannot answer is refused with the modelled error for what
/// is wrong with it, never with an empty success.
///
/// Rows differ along what is wrong: the analyzer, the analyzer's family and the shape of
/// the resource ARN.
#[tokio::test]
async fn refusals_name_what_the_caller_has_to_change() {
    let own = own_account();
    let emulator = Emulator::new();
    let external = emulator.analyzer("refusals", "refusal-analyzer", "ACCOUNT").await;
    let unused = emulator
        .analyzer("refusals", "refusal-unused", "ACCOUNT_UNUSED_ACCESS")
        .await;
    let missing = format!("arn:aws:access-analyzer:{REGION}:{own}:analyzer/never-created");
    let role = format!("arn:aws:iam::{own}:role/any-role");

    for (case, analyzer, resource, status, code) in [
        (
            "an analyzer this account never created",
            missing.as_str(),
            role.as_str(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an unused access analyzer",
            unused.as_str(),
            role.as_str(),
            400,
            "ValidationException",
        ),
        (
            "a resourceArn that is not an ARN",
            external.as_str(),
            "not-an-arn",
            400,
            "ValidationException",
        ),
    ] {
        let reply = emulator.get_analyzed_resource(analyzer, resource).await;
        assert_eq!(
            (reply.status, reply.code()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {reply:?}"
        );
    }

    // The analyzer this account never created is reported with the service's fixed text for
    // a missing analyzer, and the unsupported family with its fixed text for an action the
    // resource does not support: neither names the operation.
    let absent = emulator.get_analyzed_resource(&missing, &role).await;
    assert_eq!(
        absent.message(),
        "Analyzer not found",
        "case \"an analyzer this account never created\": expected the fixed not-found text, \
         got {absent:?}"
    );
    let refused = emulator.get_analyzed_resource(&unused, &role).await;
    assert_eq!(
        refused.message(),
        "Unsupported operation",
        "case \"an unused access analyzer\": expected the fixed unsupported-action text, got \
         {refused:?}"
    );
}
