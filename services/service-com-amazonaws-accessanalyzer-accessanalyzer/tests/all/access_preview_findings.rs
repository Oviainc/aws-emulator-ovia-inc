// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListAccessPreviewFindings`, driven over the wire.
//!
//! Every case here goes through the real router: an `http::Request` is dispatched to the
//! mounted service, so the restJson1 routing, the boundary constraint validation, the
//! handler and the response serialization all run exactly as they do in the emulator
//! binary. Nothing reaches into `crate::state` directly — a preview is created through
//! `CreateAccessPreview` and its findings are only believed as this operation reports
//! them.
//!
//! RULE under test: the findings of a completed access preview are the external access
//! its stored proposed configuration would allow — one finding per principal outside the
//! zone of trust (the account that owns the previewed resource) that an `Allow` statement
//! of a proposed policy grants access to, marked `isPublic` when the statement admits
//! every principal with nothing in its `Condition` binding the caller.
//!
//! The table spans the classes that rule generalises over: every `Configuration` union
//! member that carries a policy document and one that carries none, every principal type
//! the IAM grammar keys (`AWS` as an ARN and as a bare account id, `Service`,
//! `Federated`, `CanonicalUser`, the `*` wildcard and `NotPrincipal`), `Allow` against
//! `Deny`, a scoped wildcard against an unscoped one, one statement against several,
//! an internal principal against an external one, and a resource owned by another
//! account. The recorded public-S3-bucket-policy, IAM-role-trust-policy and
//! private-bucket-policy cases from the issue are three of its rows.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_accessanalyzer_accessanalyzer::ops::types::{
    FINDING_CHANGE_TYPE_VALUES, FINDING_SOURCE_TYPE_VALUES, FINDING_STATUS_VALUES,
    RESOURCE_TYPE_VALUES,
};
use service_com_amazonaws_accessanalyzer_accessanalyzer::state::ACCESS_PREVIEW_ANALYSIS_SPAN;

use crate::harness::{is_timestamp, Emulator, Reply};

/// How long a case waits past [`ACCESS_PREVIEW_ANALYSIS_SPAN`] before reading the
/// findings, so a loaded machine cannot read a preview a moment before its analysis is
/// due.
const SETTLE_MARGIN: Duration = Duration::from_millis(250);

/// The account every analyzer and every previewed resource here belongs to: the
/// emulator's own account, which is the zone of trust an account analyzer previews
/// against.
fn own_account() -> String {
    engine::emulator_account_id().to_string()
}

/// An account that is not [`own_account`] — the external one a finding reports.
const EXTERNAL_ACCOUNT: &str = "111111111111";

/// A second external account, for the cross-account access-point source.
const OTHER_ACCOUNT: &str = "222222222222";

/// A one-statement resource policy: `Allow` to `principal` for `actions`, with an
/// optional `Condition` block.
fn resource_policy(principal: Value, actions: Value, condition: Option<Value>) -> String {
    let mut statement = json!({
        "Effect": "Allow",
        "Principal": principal,
        "Action": actions,
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    json!({"Version": "2012-10-17", "Statement": [statement]}).to_string()
}

/// A policy document carrying `statements` verbatim.
fn policy_of(statements: Value) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// What one expected finding is asserted on: the principal it reports (type and value),
/// whether the access is public, the actions it reports (empty for a finding that reports
/// none) and the `sources` types it carries.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Expected {
    principal: (String, String),
    is_public: bool,
    actions: Vec<String>,
    sources: Vec<String>,
}

impl Expected {
    fn new(
        kind: &str,
        value: impl Into<String>,
        is_public: bool,
        actions: &[&str],
        sources: &[&str],
    ) -> Self {
        Self {
            principal: (kind.to_string(), value.into()),
            is_public,
            actions: actions.iter().map(|action| action.to_string()).collect(),
            sources: sources.iter().map(|source| source.to_string()).collect(),
        }
    }
}

/// One finding off the wire, reduced to the same shape.
fn reduce(finding: &Value) -> Expected {
    let principal = finding
        .get("principal")
        .and_then(Value::as_object)
        .map(|map| {
            let mut entries: Vec<(String, String)> = map
                .iter()
                .map(|(kind, value)| {
                    (
                        kind.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
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

/// The findings a reply reports, in the order it reports them.
fn findings(reply: &Reply) -> Vec<Value> {
    reply
        .member("findings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("expected a findings list in {:?}", reply.body))
}

/// Does `candidate` have the 8-4-4-4-12 lowercase-hex layout of a UUID, with the
/// version-8 nibble a DERIVED id carries (RFC 9562 §5.8)?
fn is_derived_uuid(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups.iter().all(|group| {
            group
                .chars()
                .all(|digit| digit.is_ascii_hexdigit() && !digit.is_uppercase())
        })
        && groups[2].starts_with('8')
}

/// The access-preview calls this suite drives, on top of the shared wire harness.
trait AccessPreviewCalls {
    async fn analyzer(&self, name: &str) -> String;
    async fn create_preview(&self, analyzer: &str, configurations: Value) -> Reply;
    async fn list_findings(&self, preview_id: &str, body: Value) -> Reply;
    /// Create a preview of one resource and wait until its analysis is due.
    async fn completed_preview(&self, analyzer: &str, resource: &str, configuration: Value)
        -> String;
}

impl AccessPreviewCalls for Emulator {
    async fn analyzer(&self, name: &str) -> String {
        let reply = self
            .create_analyzer(json!({"analyzerName": name, "type": "ACCOUNT"}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: CreateAnalyzer must succeed, got {reply:?}"
        );
        reply
            .member("arn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {name:?}: expected an arn in {:?}", reply.body))
            .to_string()
    }

    async fn create_preview(&self, analyzer: &str, configurations: Value) -> Reply {
        self.send(
            "PUT",
            "/access-preview",
            Some(json!({"analyzerArn": analyzer, "configurations": configurations})),
        )
        .await
    }

    async fn list_findings(&self, preview_id: &str, body: Value) -> Reply {
        self.send(
            "POST",
            &format!("/access-preview/{preview_id}"),
            Some(body),
        )
        .await
    }

    async fn completed_preview(
        &self,
        analyzer: &str,
        resource: &str,
        configuration: Value,
    ) -> String {
        let create = self
            .create_preview(analyzer, json!({resource: configuration}))
            .await;
        assert_eq!(
            create.status, 200,
            "CreateAccessPreview must succeed for {resource}, got {create:?}"
        );
        let id = create
            .member("id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("expected an id in {:?}", create.body))
            .to_string();
        tokio::time::sleep(ACCESS_PREVIEW_ANALYSIS_SPAN + SETTLE_MARGIN).await;
        id
    }
}

/// Every row of the main table: the proposed configuration, and the external access its
/// findings must report.
fn cases() -> Vec<(&'static str, String, &'static str, Value, Vec<Expected>)> {
    let own = own_account();
    let bucket = "arn:aws:s3:::preview-bucket".to_string();
    let role = format!("arn:aws:iam::{own}:role/preview-role");
    let queue = format!("arn:aws:sqs:us-east-1:{own}:preview-queue");
    let topic = format!("arn:aws:sns:us-east-1:{own}:preview-topic");
    let key = format!("arn:aws:kms:us-east-1:{own}:key/abcd");
    let secret = format!("arn:aws:secretsmanager:us-east-1:{own}:secret:preview");
    let file_system = format!("arn:aws:elasticfilesystem:us-east-1:{own}:file-system/fs-1");
    let repository = format!("arn:aws:ecr:us-east-1:{own}:repository/preview");
    let table = format!("arn:aws:dynamodb:us-east-1:{own}:table/preview");
    let stream = format!("arn:aws:dynamodb:us-east-1:{own}:table/preview/stream/2026");
    let directory_bucket = format!("arn:aws:s3express:us-east-1:{own}:bucket/preview--use1-az1--x-s3");
    let snapshot = format!("arn:aws:ec2:us-east-1:{own}:snapshot/snap-1");
    let own_access_point = format!("arn:aws:s3:us-east-1:{own}:accesspoint/preview-point");
    let other_access_point = format!("arn:aws:s3:us-east-1:{OTHER_ACCOUNT}:accesspoint/peer-point");
    let external_role = format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:role/peer");

    vec![
        (
            // The recorded `access-preview-detects-public-s3-bucket-policy` case.
            "a bucket policy granting every principal",
            bucket.clone(),
            "AWS::S3::Bucket",
            json!({"s3Bucket": {"bucketPolicy": resource_policy(
                json!("*"),
                json!("s3:GetObject"),
                None,
            )}}),
            vec![Expected::new("AWS", "*", true, &["s3:GetObject"], &["POLICY"])],
        ),
        (
            "a bucket policy granting every principal, scoped by an organization condition",
            bucket.clone(),
            "AWS::S3::Bucket",
            json!({"s3Bucket": {"bucketPolicy": resource_policy(
                json!({"AWS": "*"}),
                json!("s3:GetObject"),
                Some(json!({"StringEquals": {"aws:PrincipalOrgID": "o-abc123"}})),
            )}}),
            // Still external access — every principal of the organization — but not
            // public: the condition binds the caller.
            vec![Expected::new("AWS", "*", false, &["s3:GetObject"], &["POLICY"])],
        ),
        (
            // The recorded `access-preview-private-bucket-policy-no-external-finding` case.
            "a bucket policy granting only this account",
            bucket.clone(),
            "AWS::S3::Bucket",
            json!({"s3Bucket": {"bucketPolicy": resource_policy(
                json!({"AWS": own.clone()}),
                json!("s3:GetObject"),
                None,
            )}}),
            vec![],
        ),
        (
            "a bucket policy denying every principal",
            bucket.clone(),
            "AWS::S3::Bucket",
            json!({"s3Bucket": {"bucketPolicy": json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "s3:GetObject",
                }],
            }).to_string()}}),
            vec![],
        ),
        (
            // The recorded `access-preview-iam-role-external-trust-policy-finding` case:
            // a bare account id in a `Principal` is that account's root user, so the
            // finding reports the `:root` ARN IAM says it is equivalent to.
            "a role trust policy granting another account by id",
            role.clone(),
            "AWS::IAM::Role",
            json!({"iamRole": {"trustPolicy": resource_policy(
                json!({"AWS": EXTERNAL_ACCOUNT}),
                json!("sts:AssumeRole"),
                None,
            )}}),
            vec![Expected::new(
                "AWS",
                format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:root"),
                false,
                &["sts:AssumeRole"],
                &[],
            )],
        ),
        (
            "a role trust policy granting a role in another account",
            role.clone(),
            "AWS::IAM::Role",
            json!({"iamRole": {"trustPolicy": resource_policy(
                json!({"AWS": external_role.clone()}),
                json!("sts:AssumeRole"),
                None,
            )}}),
            vec![Expected::new(
                "AWS",
                external_role.clone(),
                false,
                &["sts:AssumeRole"],
                &[],
            )],
        ),
        (
            "a role trust policy granting a role of this account",
            role.clone(),
            "AWS::IAM::Role",
            json!({"iamRole": {"trustPolicy": resource_policy(
                json!({"AWS": format!("arn:aws:iam::{own}:role/peer")}),
                json!("sts:AssumeRole"),
                None,
            )}}),
            vec![],
        ),
        (
            "a role trust policy granting a service principal",
            role.clone(),
            "AWS::IAM::Role",
            json!({"iamRole": {"trustPolicy": resource_policy(
                json!({"Service": "lambda.amazonaws.com"}),
                json!("sts:AssumeRole"),
                None,
            )}}),
            vec![Expected::new(
                "Service",
                "lambda.amazonaws.com",
                false,
                &["sts:AssumeRole"],
                &[],
            )],
        ),
        (
            "a role trust policy granting a federated identity provider",
            role.clone(),
            "AWS::IAM::Role",
            json!({"iamRole": {"trustPolicy": resource_policy(
                json!({"Federated": "accounts.google.com"}),
                json!("sts:AssumeRoleWithWebIdentity"),
                None,
            )}}),
            vec![Expected::new(
                "Federated",
                "accounts.google.com",
                false,
                &["sts:AssumeRoleWithWebIdentity"],
                &[],
            )],
        ),
        (
            "a role trust policy granting every principal except this account",
            role.clone(),
            "AWS::IAM::Role",
            json!({"iamRole": {"trustPolicy": json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "NotPrincipal": {"AWS": own.clone()},
                    "Action": "sts:AssumeRole",
                }],
            }).to_string()}}),
            // `NotPrincipal` with `Allow` admits every principal the element does not
            // name, which includes every caller the account never named.
            vec![Expected::new("AWS", "*", true, &["sts:AssumeRole"], &[])],
        ),
        (
            "a role trust policy carrying two statements",
            role.clone(),
            "AWS::IAM::Role",
            json!({"iamRole": {"trustPolicy": policy_of(json!([
                {
                    "Effect": "Allow",
                    "Principal": {"AWS": own.clone()},
                    "Action": "sts:AssumeRole",
                },
                {
                    "Effect": "Allow",
                    "Principal": {"AWS": external_role.clone()},
                    "Action": "sts:TagSession",
                },
            ]))}}),
            vec![Expected::new(
                "AWS",
                external_role.clone(),
                false,
                &["sts:TagSession"],
                &[],
            )],
        ),
        (
            "a queue policy granting two external accounts in one statement",
            queue.clone(),
            "AWS::SQS::Queue",
            json!({"sqsQueue": {"queuePolicy": resource_policy(
                json!({"AWS": [EXTERNAL_ACCOUNT, OTHER_ACCOUNT]}),
                json!(["sqs:SendMessage", "sqs:ReceiveMessage"]),
                None,
            )}}),
            vec![
                Expected::new(
                    "AWS",
                    format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:root"),
                    false,
                    &["sqs:SendMessage", "sqs:ReceiveMessage"],
                    &[],
                ),
                Expected::new(
                    "AWS",
                    format!("arn:aws:iam::{OTHER_ACCOUNT}:root"),
                    false,
                    &["sqs:SendMessage", "sqs:ReceiveMessage"],
                    &[],
                ),
            ],
        ),
        (
            // The zone of trust follows the account that OWNS the previewed resource, not
            // the account that calls: this queue belongs to another account and grants
            // that same account, so no access leaves its zone of trust.
            "a queue owned by another account granting that account",
            format!("arn:aws:sqs:us-east-1:{OTHER_ACCOUNT}:peer-queue"),
            "AWS::SQS::Queue",
            json!({"sqsQueue": {"queuePolicy": resource_policy(
                json!({"AWS": OTHER_ACCOUNT}),
                json!("sqs:SendMessage"),
                None,
            )}}),
            vec![],
        ),
        (
            "a topic policy granting an external account",
            topic.clone(),
            "AWS::SNS::Topic",
            json!({"snsTopic": {"topicPolicy": resource_policy(
                json!({"AWS": EXTERNAL_ACCOUNT}),
                json!("sns:Publish"),
                None,
            )}}),
            vec![Expected::new(
                "AWS",
                format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:root"),
                false,
                &["sns:Publish"],
                &[],
            )],
        ),
        (
            "a key with one internal and one external key policy",
            key.clone(),
            "AWS::KMS::Key",
            json!({"kmsKey": {"keyPolicies": {
                "default": resource_policy(json!({"AWS": own.clone()}), json!("kms:Decrypt"), None),
                "shared": resource_policy(
                    json!({"AWS": EXTERNAL_ACCOUNT}),
                    json!("kms:Encrypt"),
                    None,
                ),
            }}}),
            vec![Expected::new(
                "AWS",
                format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:root"),
                false,
                &["kms:Encrypt"],
                &[],
            )],
        ),
        (
            "a secret policy granting a canonical user",
            secret.clone(),
            "AWS::SecretsManager::Secret",
            json!({"secretsManagerSecret": {"secretPolicy": resource_policy(
                json!({"CanonicalUser": "79a59df900b949e55d96a1e698fbacedfd6e09d98eacf8f8d5218e7cd47ef2be"}),
                json!("secretsmanager:GetSecretValue"),
                None,
            )}}),
            vec![Expected::new(
                "CanonicalUser",
                "79a59df900b949e55d96a1e698fbacedfd6e09d98eacf8f8d5218e7cd47ef2be",
                false,
                &["secretsmanager:GetSecretValue"],
                &[],
            )],
        ),
        (
            "a file system policy granting an external account",
            file_system.clone(),
            "AWS::EFS::FileSystem",
            json!({"efsFileSystem": {"fileSystemPolicy": resource_policy(
                json!({"AWS": external_role.clone()}),
                json!("elasticfilesystem:ClientMount"),
                None,
            )}}),
            vec![Expected::new(
                "AWS",
                external_role.clone(),
                false,
                &["elasticfilesystem:ClientMount"],
                &[],
            )],
        ),
        (
            // `NotAction` allows every action except the ones it lists, which is not the
            // listed set — so the finding reports no action rather than the wrong one.
            "a repository policy written with NotAction",
            repository.clone(),
            "AWS::ECR::Repository",
            json!({"ecrRepository": {"repositoryPolicy": policy_of(json!([{
                "Effect": "Allow",
                "Principal": {"AWS": EXTERNAL_ACCOUNT},
                "NotAction": "ecr:DeleteRepository",
            }]))}}),
            vec![Expected::new(
                "AWS",
                format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:root"),
                false,
                &[],
                &[],
            )],
        ),
        (
            "a table policy granting an external account two actions",
            table.clone(),
            "AWS::DynamoDB::Table",
            json!({"dynamodbTable": {"tablePolicy": resource_policy(
                json!({"AWS": EXTERNAL_ACCOUNT}),
                json!(["dynamodb:GetItem", "dynamodb:Query"]),
                None,
            )}}),
            vec![Expected::new(
                "AWS",
                format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:root"),
                false,
                &["dynamodb:GetItem", "dynamodb:Query"],
                &[],
            )],
        ),
        (
            "a stream policy granting an external account",
            stream.clone(),
            "AWS::DynamoDB::Stream",
            json!({"dynamodbStream": {"streamPolicy": resource_policy(
                json!({"AWS": EXTERNAL_ACCOUNT}),
                json!("dynamodb:GetRecords"),
                None,
            )}}),
            vec![Expected::new(
                "AWS",
                format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:root"),
                false,
                &["dynamodb:GetRecords"],
                &[],
            )],
        ),
        (
            "a directory bucket policy granting every principal",
            directory_bucket.clone(),
            "AWS::S3Express::DirectoryBucket",
            json!({"s3ExpressDirectoryBucket": {"bucketPolicy": resource_policy(
                json!("*"),
                json!("s3express:CreateSession"),
                None,
            )}}),
            vec![Expected::new(
                "AWS",
                "*",
                true,
                &["s3express:CreateSession"],
                &["POLICY"],
            )],
        ),
        (
            // An access point of this account is an `S3_ACCESS_POINT` source; the bucket
            // policy beside it is a `POLICY` source, so the two findings differ in how
            // the access is granted.
            "a bucket whose own policy and access point policy both grant outside",
            bucket.clone(),
            "AWS::S3::Bucket",
            json!({"s3Bucket": {
                "bucketPolicy": resource_policy(
                    json!({"AWS": EXTERNAL_ACCOUNT}),
                    json!("s3:GetObject"),
                    None,
                ),
                "accessPoints": {
                    own_access_point.clone(): {
                        "accessPointPolicy": resource_policy(
                            json!({"AWS": EXTERNAL_ACCOUNT}),
                            json!("s3:PutObject"),
                            None,
                        ),
                        "networkOrigin": {"internetConfiguration": {}},
                    },
                },
            }}),
            vec![
                Expected::new(
                    "AWS",
                    format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:root"),
                    false,
                    &["s3:PutObject"],
                    &["S3_ACCESS_POINT"],
                ),
                Expected::new(
                    "AWS",
                    format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:root"),
                    false,
                    &["s3:GetObject"],
                    &["POLICY"],
                ),
            ],
        ),
        (
            "a bucket whose access point is owned by another account",
            bucket.clone(),
            "AWS::S3::Bucket",
            json!({"s3Bucket": {
                "accessPoints": {
                    other_access_point.clone(): {
                        "accessPointPolicy": resource_policy(
                            json!({"AWS": EXTERNAL_ACCOUNT}),
                            json!("s3:GetObject"),
                            None,
                        ),
                    },
                },
            }}),
            vec![Expected::new(
                "AWS",
                format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:root"),
                false,
                &["s3:GetObject"],
                &["S3_ACCESS_POINT_ACCOUNT"],
            )],
        ),
        (
            // Shared through account ids rather than a policy document, so there is no
            // statement to read an action set out of and no finding is reported.
            "a snapshot shared without a policy document",
            snapshot.clone(),
            "AWS::EC2::Snapshot",
            json!({"ebsSnapshot": {"userIds": [EXTERNAL_ACCOUNT], "groups": ["all"]}}),
            vec![],
        ),
    ]
}

/// RULE: the findings of a completed preview are the external access its proposed
/// configuration would allow — one per principal outside the owning account that an
/// `Allow` statement grants to, `isPublic` exactly when the statement admits every
/// principal unbound by a condition — and none at all when the configuration grants no
/// external access.
#[tokio::test]
async fn findings_report_the_external_access_a_proposed_configuration_would_allow() {
    let emulator = Emulator::new();
    let analyzer = emulator.analyzer("finding_analyzer").await;
    let own = own_account();

    // Every preview is created first, then listed: the analyses run in parallel and the
    // whole table waits out ONE analysis span.
    let mut previews: Vec<(&str, String, &str, Vec<Expected>)> = Vec::new();
    for (case, resource, resource_type, configuration, expected) in cases() {
        let create = emulator
            .create_preview(&analyzer, json!({resource: configuration}))
            .await;
        assert_eq!(
            create.status, 200,
            "case {case:?}: CreateAccessPreview must succeed, got {create:?}"
        );
        let id = create
            .member("id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected an id in {:?}", create.body))
            .to_string();
        previews.push((case, id, resource_type, expected));
    }
    tokio::time::sleep(ACCESS_PREVIEW_ANALYSIS_SPAN + SETTLE_MARGIN).await;

    let mut ids: BTreeSet<String> = BTreeSet::new();
    for (case, preview_id, resource_type, expected) in previews {
        let reply = emulator
            .list_findings(&preview_id, json!({"analyzerArn": analyzer}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected ListAccessPreviewFindings to succeed, got {reply:?}"
        );
        let reported = findings(&reply);
        let reduced: Vec<Expected> = reported.iter().map(reduce).collect();
        assert_eq!(
            reduced, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        assert!(
            reply.member("nextToken").is_none(),
            "case {case:?}: a page holding every finding carries no nextToken, got {reply:?}"
        );

        for finding in &reported {
            assert!(
                is_derived_uuid(finding.get("id").and_then(Value::as_str).unwrap_or_default()),
                "case {case:?}: expected a derived-UUID finding id, got {finding:?}"
            );
            assert!(
                ids.insert(
                    finding
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                ),
                "case {case:?}: finding id {:?} was already reported for another finding",
                finding.get("id")
            );
            assert_eq!(
                finding.get("resourceType").and_then(Value::as_str),
                Some(resource_type),
                "case {case:?}: expected the configuration's resource type, got {finding:?}"
            );
            assert!(
                RESOURCE_TYPE_VALUES.contains(&resource_type),
                "case {case:?}: {resource_type:?} is not a modelled ResourceType"
            );
            assert_eq!(
                finding.get("status").and_then(Value::as_str),
                Some("ACTIVE"),
                "case {case:?}: newly previewed access is ACTIVE, got {finding:?}"
            );
            assert_eq!(
                finding.get("changeType").and_then(Value::as_str),
                Some("NEW"),
                "case {case:?}: previewed access the emulator holds no finding for is NEW, got {finding:?}"
            );
            assert!(
                is_timestamp(finding.get("createdAt")),
                "case {case:?}: expected a createdAt instant, got {finding:?}"
            );
            assert_eq!(
                finding.get("error").filter(|error| !error.is_null()),
                None,
                "case {case:?}: an analyzed policy reports no error, got {finding:?}"
            );
            assert_eq!(
                finding.get("existingFindingId").filter(|id| !id.is_null()),
                None,
                "case {case:?}: the emulator holds no existing findings, got {finding:?}"
            );
            // The owning account of every resource in the table is this account, named
            // either by the resource ARN or, for an ARN form that carries no account
            // segment, by the emulator's own account.
            let owner = finding
                .get("resourceOwnerAccount")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                owner == own || owner == OTHER_ACCOUNT,
                "case {case:?}: expected the resource's owning account, got {finding:?}"
            );
            for source in finding
                .get("sources")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let kind = source.get("type").and_then(Value::as_str).unwrap_or_default();
                assert!(
                    FINDING_SOURCE_TYPE_VALUES.contains(&kind),
                    "case {case:?}: {kind:?} is not a modelled FindingSourceType"
                );
            }
        }
    }

    // The two vocabularies every finding above was asserted against are the model's own.
    assert!(
        FINDING_STATUS_VALUES.contains(&"ACTIVE") && FINDING_CHANGE_TYPE_VALUES.contains(&"NEW"),
        "ACTIVE/NEW must be modelled FindingStatus/FindingChangeType values"
    );
}

/// RULE: the condition keys a statement constrains are reported with the finding, keyed
/// as the document spells them.
#[tokio::test]
async fn a_finding_reports_the_conditions_of_the_statement_that_granted_the_access() {
    let emulator = Emulator::new();
    let analyzer = emulator.analyzer("condition_analyzer").await;
    let bucket = "arn:aws:s3:::conditional-bucket";

    let cases: Vec<(&str, Option<Value>, Option<Value>)> = vec![
        ("no condition at all", None, None),
        (
            "one condition key with one value",
            Some(json!({"StringEquals": {"aws:PrincipalOrgID": "o-abc123"}})),
            Some(json!({"aws:PrincipalOrgID": "o-abc123"})),
        ),
        (
            "one condition key with two values",
            Some(json!({"StringEquals": {"aws:PrincipalAccount": [EXTERNAL_ACCOUNT, OTHER_ACCOUNT]}})),
            Some(json!({"aws:PrincipalAccount": format!("{EXTERNAL_ACCOUNT},{OTHER_ACCOUNT}")})),
        ),
        (
            "two condition keys under two operators",
            Some(json!({
                "StringEquals": {"aws:PrincipalOrgID": "o-abc123"},
                "Bool": {"aws:SecureTransport": true},
            })),
            Some(json!({
                "aws:PrincipalOrgID": "o-abc123",
                "aws:SecureTransport": "true",
            })),
        ),
    ];

    let mut previews: Vec<(&str, String, Option<Value>)> = Vec::new();
    for (case, condition, expected) in cases {
        let configuration = json!({"s3Bucket": {"bucketPolicy": resource_policy(
            json!({"AWS": EXTERNAL_ACCOUNT}),
            json!("s3:GetObject"),
            condition,
        )}});
        let create = emulator
            .create_preview(&analyzer, json!({bucket: configuration}))
            .await;
        assert_eq!(
            create.status, 200,
            "case {case:?}: CreateAccessPreview must succeed, got {create:?}"
        );
        previews.push((
            case,
            create
                .member("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            expected,
        ));
    }
    tokio::time::sleep(ACCESS_PREVIEW_ANALYSIS_SPAN + SETTLE_MARGIN).await;

    for (case, preview_id, expected) in previews {
        let reply = emulator
            .list_findings(&preview_id, json!({"analyzerArn": analyzer}))
            .await;
        let reported = findings(&reply);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected one finding, got {reported:?}"
        );
        let condition = reported[0].get("condition").filter(|value| !value.is_null());
        assert_eq!(
            condition,
            expected.as_ref(),
            "case {case:?}: expected condition {expected:?}, got {:?}",
            reported[0]
        );
    }
}

/// RULE: the findings of one preview are a function of its stored configuration, so
/// listing it again reports the same findings with the same ids — a read changes nothing.
#[tokio::test]
async fn listing_the_same_preview_twice_reports_the_same_findings() {
    let emulator = Emulator::new();
    let analyzer = emulator.analyzer("stable_analyzer").await;
    let bucket = "arn:aws:s3:::stable-bucket";
    let preview = emulator
        .completed_preview(
            &analyzer,
            bucket,
            json!({"s3Bucket": {"bucketPolicy": policy_of(json!([
                {
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "s3:GetObject",
                },
                {
                    "Effect": "Allow",
                    "Principal": {"AWS": EXTERNAL_ACCOUNT},
                    "Action": "s3:PutObject",
                },
            ]))}}),
        )
        .await;

    let first = emulator
        .list_findings(&preview, json!({"analyzerArn": analyzer}))
        .await;
    let second = emulator
        .list_findings(&preview, json!({"analyzerArn": analyzer}))
        .await;
    assert_eq!(
        findings(&first).len(),
        2,
        "expected one finding per granting statement, got {first:?}"
    );
    assert_eq!(
        findings(&first),
        findings(&second),
        "expected the same findings on a second read, got {first:?} then {second:?}"
    );
}

/// RULE: `maxResults` bounds a page and `nextToken` resumes after it, so a caller that
/// follows the tokens sees every finding exactly once.
#[tokio::test]
async fn findings_are_paged_by_max_results_and_next_token() {
    let emulator = Emulator::new();
    let analyzer = emulator.analyzer("paging_analyzer").await;
    let bucket = "arn:aws:s3:::paged-bucket";

    // Five granting statements, so a page size of 1, 2 and 5 all have a remainder or
    // exactly none.
    let accounts = [
        "100000000001",
        "100000000002",
        "100000000003",
        "100000000004",
        "100000000005",
    ];
    let statements: Vec<Value> = accounts
        .iter()
        .map(|account| {
            json!({
                "Effect": "Allow",
                "Principal": {"AWS": account},
                "Action": "s3:GetObject",
            })
        })
        .collect();
    let preview = emulator
        .completed_preview(
            &analyzer,
            bucket,
            json!({"s3Bucket": {"bucketPolicy": policy_of(json!(statements))}}),
        )
        .await;

    let whole = emulator
        .list_findings(&preview, json!({"analyzerArn": analyzer}))
        .await;
    let expected = findings(&whole);
    assert_eq!(
        expected.len(),
        accounts.len(),
        "expected one finding per granting statement, got {whole:?}"
    );

    for page_size in [1_i64, 2, 5, 6] {
        let mut walked: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({"analyzerArn": analyzer, "maxResults": page_size});
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let reply = emulator.list_findings(&preview, body).await;
            assert_eq!(
                reply.status, 200,
                "case page size {page_size}: expected a page, got {reply:?}"
            );
            let page = findings(&reply);
            assert!(
                page.len() as i64 <= page_size,
                "case page size {page_size}: a page holds at most maxResults findings, got {reply:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= accounts.len() + 1,
                "case page size {page_size}: paging must terminate, walked {walked:?}"
            );
            match reply
                .member("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            walked, expected,
            "case page size {page_size}: following the tokens must report every finding exactly once, got {walked:?}"
        );
    }
}

/// RULE: a paging argument this service did not issue is refused as a field validation,
/// naming the member at fault.
#[tokio::test]
async fn bad_paging_arguments_are_refused() {
    let emulator = Emulator::new();
    let analyzer = emulator.analyzer("paging_refusal_analyzer").await;
    let bucket = "arn:aws:s3:::refusing-bucket";
    let preview = emulator
        .completed_preview(
            &analyzer,
            bucket,
            json!({"s3Bucket": {"bucketPolicy": resource_policy(
                json!("*"),
                json!("s3:GetObject"),
                None,
            )}}),
        )
        .await;

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a maxResults below one",
            json!({"analyzerArn": analyzer, "maxResults": 0}),
            "maxResults",
        ),
        (
            "a nextToken this service did not issue",
            json!({"analyzerArn": analyzer, "nextToken": "not-a-token"}),
            "nextToken",
        ),
    ];
    for (case, body, member) in cases {
        let reply = emulator.list_findings(&preview, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected a 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {reply:?}"
        );
        let fields = reply
            .member("fieldList")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            fields
                .iter()
                .any(|field| field.get("name").and_then(Value::as_str) == Some(member)),
            "case {case:?}: expected {member:?} in the fieldList, got {reply:?}"
        );
    }
}

/// RULE: a `filter` criterion keeps the findings whose named member satisfies every
/// operator it carries, and a key naming no member of a finding is refused.
#[tokio::test]
async fn a_filter_keeps_the_findings_its_criteria_hold_for() {
    let emulator = Emulator::new();
    let analyzer = emulator.analyzer("filter_analyzer").await;
    let bucket = "arn:aws:s3:::filtered-bucket";
    let access_point = format!(
        "arn:aws:s3:us-east-1:{}:accesspoint/filtered-point",
        own_account()
    );
    let preview = emulator
        .completed_preview(
            &analyzer,
            bucket,
            json!({"s3Bucket": {
                "bucketPolicy": policy_of(json!([
                    {
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "s3:GetObject",
                    },
                    {
                        "Effect": "Allow",
                        "Principal": {"AWS": EXTERNAL_ACCOUNT},
                        "Action": "s3:PutObject",
                        "Condition": {"StringEquals": {"aws:PrincipalOrgID": "o-abc123"}},
                    },
                ])),
                "accessPoints": {
                    access_point.clone(): {
                        "accessPointPolicy": resource_policy(
                            json!({"AWS": OTHER_ACCOUNT}),
                            json!("s3:ListBucket"),
                            None,
                        ),
                    },
                },
            }}),
        )
        .await;

    let everything = emulator
        .list_findings(&preview, json!({"analyzerArn": analyzer}))
        .await;
    assert_eq!(
        findings(&everything).len(),
        3,
        "expected one finding per granting statement, got {everything:?}"
    );

    // Each row names the actions the surviving findings must report, which identifies
    // them: the three statements allow three different actions.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "isPublic equals true",
            json!({"isPublic": {"eq": ["true"]}}),
            vec!["s3:GetObject"],
        ),
        (
            "isPublic does not equal true",
            json!({"isPublic": {"neq": ["true"]}}),
            vec!["s3:ListBucket", "s3:PutObject"],
        ),
        (
            "the action list equals one action",
            json!({"action": {"eq": ["s3:PutObject"]}}),
            vec!["s3:PutObject"],
        ),
        (
            "the resource type is the previewed one",
            json!({"resourceType": {"eq": ["AWS::S3::Bucket"]}}),
            vec!["s3:GetObject", "s3:ListBucket", "s3:PutObject"],
        ),
        (
            "the resource type is another one",
            json!({"resourceType": {"eq": ["AWS::IAM::Role"]}}),
            vec![],
        ),
        (
            "the principal at AWS contains an account id",
            json!({"principal.AWS": {"contains": [OTHER_ACCOUNT]}}),
            vec!["s3:ListBucket"],
        ),
        (
            "a condition on the organization exists",
            json!({"condition.aws:PrincipalOrgID": {"exists": true}}),
            vec!["s3:PutObject"],
        ),
        (
            "no condition at all",
            json!({"condition": {"exists": false}}),
            vec!["s3:GetObject", "s3:ListBucket"],
        ),
        (
            "the access is granted through an access point",
            json!({"sources.type": {"eq": ["S3_ACCESS_POINT"]}}),
            vec!["s3:ListBucket"],
        ),
        (
            "two criteria that hold for one finding each",
            json!({
                "isPublic": {"eq": ["false"]},
                "sources.type": {"eq": ["POLICY"]},
            }),
            vec!["s3:PutObject"],
        ),
        (
            "two operators on one member",
            json!({"action": {"contains": ["s3:"], "neq": ["s3:GetObject"]}}),
            vec!["s3:ListBucket", "s3:PutObject"],
        ),
        (
            "an error member no finding carries",
            json!({"error": {"exists": true}}),
            vec![],
        ),
    ];

    for (case, filter, expected) in cases {
        let reply = emulator
            .list_findings(
                &preview,
                json!({"analyzerArn": analyzer, "filter": filter}),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected a filtered listing, got {reply:?}"
        );
        let mut actions: Vec<String> = findings(&reply)
            .iter()
            .flat_map(|finding| {
                finding
                    .get("action")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            })
            .map(|action| action.as_str().unwrap_or_default().to_string())
            .collect();
        actions.sort();
        assert_eq!(
            actions, expected,
            "case {case:?}: expected the findings allowing {expected:?}, got {reply:?}"
        );
    }

    for key in ["notAMember", "principal", "sources.detail"] {
        let reply = emulator
            .list_findings(
                &preview,
                json!({"analyzerArn": analyzer, "filter": {key: {"exists": true}}}),
            )
            .await;
        if key == "principal" {
            // A map-valued member may be filtered as a whole, so this one is accepted.
            assert_eq!(
                reply.status, 200,
                "case {key:?}: a member of a finding is a usable filter key, got {reply:?}"
            );
            continue;
        }
        assert_eq!(
            reply.status, 400,
            "case {key:?}: a filter key naming no member must be refused, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {key:?}: expected a ValidationException, got {reply:?}"
        );
    }

    // Every criterion is checked against every finding, so the count of them is capped
    // (MAX_FILTER_CRITERIA = 20): twenty are matched, twenty-one are refused.
    for count in [20_usize, 21] {
        let filter: Value = Value::Object(
            (0..count)
                .map(|nth| {
                    (
                        format!("condition.aws:ExampleKey{nth}"),
                        json!({"exists": false}),
                    )
                })
                .collect(),
        );
        let reply = emulator
            .list_findings(&preview, json!({"analyzerArn": analyzer, "filter": filter}))
            .await;
        let expected = if count > 20 { 400 } else { 200 };
        assert_eq!(
            reply.status, expected,
            "case {count} criteria: expected HTTP {expected}, got {reply:?}"
        );
        if expected == 400 {
            assert_eq!(
                reply.code(),
                "ValidationException",
                "case {count} criteria: expected a ValidationException, got {reply:?}"
            );
            assert!(
                reply
                    .member("fieldList")
                    .and_then(Value::as_array)
                    .is_some_and(|fields| fields
                        .iter()
                        .any(|field| field.get("name").and_then(Value::as_str) == Some("filter"))),
                "case {count} criteria: the refusal names the filter, got {reply:?}"
            );
        }
    }
}

/// RULE: the findings are the RESULT of the analysis, so a preview whose analysis has not
/// completed answers the modelled conflict rather than an empty list, and a preview or
/// analyzer the account does not hold answers the modelled 404.
#[tokio::test]
async fn a_preview_without_results_is_refused_rather_than_reported_empty() {
    let emulator = Emulator::new();
    let analyzer = emulator.analyzer("refusal_analyzer").await;
    let bucket = "arn:aws:s3:::refused-bucket";
    let analyzable = json!({"s3Bucket": {"bucketPolicy": resource_policy(
        json!("*"),
        json!("s3:GetObject"),
        None,
    )}});

    // Still CREATING: read straight after the create, before the analysis is due.
    let creating = emulator
        .create_preview(&analyzer, json!({bucket: analyzable.clone()}))
        .await;
    let creating_id = creating
        .member("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let reply = emulator
        .list_findings(&creating_id, json!({"analyzerArn": analyzer}))
        .await;
    assert_eq!(
        reply.status, 409,
        "a preview still being analyzed must answer 409, got {reply:?}"
    );
    assert_eq!(
        reply.code(),
        "ConflictException",
        "a preview still being analyzed must answer ConflictException, got {reply:?}"
    );
    assert_eq!(
        reply.member("resourceId").and_then(Value::as_str),
        Some(creating_id.as_str()),
        "the refusal names the preview asked for, got {reply:?}"
    );

    // FAILED: a configuration keyed by something that is not a resource ARN cannot be
    // analyzed, so the preview produced no findings at all.
    let failed = emulator
        .create_preview(&analyzer, json!({"not-an-arn": analyzable.clone()}))
        .await;
    let failed_id = failed
        .member("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    tokio::time::sleep(ACCESS_PREVIEW_ANALYSIS_SPAN + SETTLE_MARGIN).await;
    let reply = emulator
        .list_findings(&failed_id, json!({"analyzerArn": analyzer}))
        .await;
    assert_eq!(
        reply.status, 409,
        "a preview whose analysis failed must answer 409, got {reply:?}"
    );

    // An id of the right shape that no preview of this analyzer carries.
    let reply = emulator
        .list_findings(
            "00000000-0000-4000-8000-000000000000",
            json!({"analyzerArn": analyzer}),
        )
        .await;
    assert_eq!(
        reply.status, 404,
        "an access preview id the analyzer does not hold must answer 404, got {reply:?}"
    );
    assert_eq!(
        reply.member("resourceType").and_then(Value::as_str),
        Some("access-preview"),
        "the 404 reports the kind of resource that was not found, got {reply:?}"
    );

    // An analyzer this account does not hold.
    let reply = emulator
        .list_findings(
            &creating_id,
            json!({"analyzerArn": format!(
                "arn:aws:access-analyzer:us-east-1:{}:analyzer/absent",
                own_account()
            )}),
        )
        .await;
    assert_eq!(
        reply.status, 404,
        "an analyzer this account does not hold must answer 404, got {reply:?}"
    );
    assert_eq!(
        reply.member("resourceType").and_then(Value::as_str),
        Some("analyzer"),
        "the 404 reports the kind of resource that was not found, got {reply:?}"
    );

    // And never the scaffold's 501: the operation is implemented.
    let completed = emulator
        .completed_preview(&analyzer, bucket, analyzable)
        .await;
    let reply = emulator
        .list_findings(&completed, json!({"analyzerArn": analyzer}))
        .await;
    assert_eq!(
        reply.status, 200,
        "an implemented operation never answers the scaffold's 501, got {reply:?}"
    );
}

/// RULE: the derivation is bounded in the PRODUCT of its factors, so a configuration
/// granting external access to more principals than this emulator reports findings for is
/// refused as a field validation naming the preview, rather than materialised.
#[tokio::test]
async fn a_configuration_past_the_derivation_ceiling_is_refused() {
    let emulator = Emulator::new();
    let analyzer = emulator.analyzer("ceiling_analyzer").await;
    let bucket = "arn:aws:s3:::crowded-bucket";

    /// One statement granting `count` external accounts, so a policy of N statements
    /// grants N * count distinct accesses.
    fn granting(first: usize, count: usize) -> Value {
        let principals: Vec<String> = (first..first + count)
            .map(|nth| format!("{:012}", 100_000_000_000_u64 + nth as u64))
            .collect();
        json!({
            "Effect": "Allow",
            "Principal": {"AWS": principals},
            "Action": "s3:GetObject",
        })
    }

    // MAX_DERIVED_FINDINGS is 1_000: 900 accesses are reported, 1_200 are refused.
    let cases: &[(&str, usize, usize, u16)] = &[
        ("three statements of 300 principals", 3, 300, 200),
        ("four statements of 300 principals", 4, 300, 400),
    ];
    for (case, statements, principals, expected) in cases {
        let policy = policy_of(json!((0..*statements)
            .map(|nth| granting(nth * principals, *principals))
            .collect::<Vec<Value>>()));
        let preview = emulator
            .completed_preview(&analyzer, bucket, json!({"s3Bucket": {"bucketPolicy": policy}}))
            .await;
        let reply = emulator
            .list_findings(&preview, json!({"analyzerArn": analyzer, "maxResults": 1}))
            .await;
        assert_eq!(
            reply.status, *expected,
            "case {case:?}: expected HTTP {expected}, got {reply:?}"
        );
        if *expected == 400 {
            assert_eq!(
                reply.code(),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {reply:?}"
            );
            assert!(
                reply
                    .member("fieldList")
                    .and_then(Value::as_array)
                    .is_some_and(|fields| fields.iter().any(|field| field
                        .get("name")
                        .and_then(Value::as_str)
                        == Some("accessPreviewId"))),
                "case {case:?}: the refusal names the preview that was asked for, got {reply:?}"
            );
        } else {
            assert_eq!(
                findings(&reply).len(),
                1,
                "case {case:?}: expected the requested page size, got {reply:?}"
            );
        }
    }
}

/// RULE: a stored policy the analysis cannot read is reported as a finding carrying the
/// modelled `error` member — never as "this configuration grants no external access".
#[tokio::test]
async fn a_policy_the_analysis_cannot_read_is_reported_as_an_error_finding() {
    let emulator = Emulator::new();
    let analyzer = emulator.analyzer("error_analyzer").await;
    let bucket = "arn:aws:s3:::unreadable-bucket";

    // Each of these is a JSON OBJECT, so the preview completes, and none of them is a
    // resource policy the shared parser can read.
    let cases: Vec<(&str, Value)> = vec![
        ("a document with no Statement element", json!({"Version": "2012-10-17"})),
        (
            "a statement with no Effect",
            json!({"Statement": [{"Principal": "*", "Action": "s3:GetObject"}]}),
        ),
        (
            "a resource policy statement naming no principal",
            json!({"Statement": [{"Effect": "Allow", "Action": "s3:GetObject"}]}),
        ),
        (
            "a statement naming both Principal and NotPrincipal",
            json!({"Statement": [{
                "Effect": "Allow",
                "Principal": "*",
                "NotPrincipal": {"AWS": EXTERNAL_ACCOUNT},
                "Action": "s3:GetObject",
            }]}),
        ),
    ];

    let mut previews: Vec<(&str, String)> = Vec::new();
    for (case, document) in cases {
        let create = emulator
            .create_preview(
                &analyzer,
                json!({bucket: {"s3Bucket": {"bucketPolicy": document.to_string()}}}),
            )
            .await;
        assert_eq!(
            create.status, 200,
            "case {case:?}: CreateAccessPreview must succeed, got {create:?}"
        );
        previews.push((
            case,
            create
                .member("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ));
    }
    tokio::time::sleep(ACCESS_PREVIEW_ANALYSIS_SPAN + SETTLE_MARGIN).await;

    for (case, preview_id) in previews {
        let reply = emulator
            .list_findings(&preview_id, json!({"analyzerArn": analyzer}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected a listing, got {reply:?}"
        );
        let reported = findings(&reply);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected one error finding, got {reported:?}"
        );
        let error = reported[0]
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            error.contains("s3Bucket.bucketPolicy"),
            "case {case:?}: the error names the policy it could not analyze, got {reported:?}"
        );
        assert_eq!(
            reported[0].get("principal").filter(|value| !value.is_null()),
            None,
            "case {case:?}: an unanalyzed policy names no principal, got {reported:?}"
        );
    }
}
