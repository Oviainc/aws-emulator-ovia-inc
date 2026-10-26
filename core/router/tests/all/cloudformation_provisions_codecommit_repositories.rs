// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::CodeCommit::Repository` resources its template declares — in CodeCommit, with the
//! trigger configuration the declaration carries.
//!
//! The rule under test: **a stack reports a resource as created only where the service that
//! owns it created it, through that service's own public API and with every declared
//! property applied, so the handle the stack publishes addresses a resource that service
//! really holds — and deleting the stack releases it there.** For a repository the handle is
//! its NAME ("Ref returns the repository name"), so right after the create waiter returns
//! `GetRepository` and `GetRepositoryTriggers` answer for the name the stack published,
//! rather than CodeCommit answering `RepositoryDoesNotExistException` for a repository its
//! own stack just reported complete. Two crates have to agree for that to hold
//! (CloudFormation makes the calls, CodeCommit serves them), so the suite lives here: a
//! crate-local test can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! repository or leaves the name to CloudFormation, whether it is written in JSON or YAML,
//! which optional properties it declares (description, encryption key, tags), how many
//! triggers it declares and which members each of those carries (both destination kinds,
//! every event set, branch scopes, custom data), and the Region the stack is deployed in.
//!
//! The deletion half and the refusal half are rows of the same rule: deleting the stack
//! deletes the repositories it created, and a declaration CodeCommit REFUSES leaves the
//! stack `ROLLBACK_COMPLETE` naming the resource — with nothing half-created behind it,
//! which for this type means no repository left holding no triggers.
//!
//! The awsQuery form encoder and the XML element readers are the test crate's own
//! ([`crate::query_wire`]), not another copy.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due, so the stack reports the deployment it completed. A repository has no transient
/// status of its own: `CreateRepository` answers with one that is usable at once.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The longest repository name CodeCommit accepts (`RepositoryName`, `@length(min: 1,
/// max: 100)` in that service's own model), which a GENERATED name is held to.
const MAX_REPOSITORY_NAME: usize = 100;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every
/// other suite here; the in-process callback CloudFormation makes signs its request either
/// way (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_codecommit_codecommit_20150413::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one CodeCommit operation over its awsJson1_1 wire, signed for `region`.
async fn codecommit(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("codecommit.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CodeCommit_20150413.{operation}"))
        .header("authorization", authorization("codecommit", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// The ARN a repository of `name` carries in `region`: the same total function of the
/// Region, the account and the name CodeCommit itself keys a repository by.
fn repository_arn(region: &str, name: &str) -> String {
    format!("arn:aws:codecommit:{region}:{ACCOUNT}:{name}")
}

/// The ARN of an Amazon SNS topic in `region` — one of the two destination kinds a
/// repository trigger may target.
fn topic_arn(region: &str, topic: &str) -> String {
    format!("arn:aws:sns:{region}:{ACCOUNT}:{topic}")
}

/// The ARN of a Lambda function in `region` — the other destination kind.
fn function_arn(region: &str, function: &str) -> String {
    format!("arn:aws:lambda:{region}:{ACCOUNT}:function:{function}")
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as. Each
/// `member` of the list carries exactly one `OutputKey` and one `OutputValue`, so the two
/// document-ordered lists pair up.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// The error code an awsJson answer carries, without the namespace a `__type` may qualify
/// it with.
fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .map(|code| code.rsplit('#').next().unwrap_or(code).to_string())
        .unwrap_or_default()
}

/// One row of the table: a template, and what the rule says the repository it declares must
/// then be in CodeCommit.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the repository is created in and
    /// the one its ARN carries.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn(&str) -> String,
    /// The name the template declares, or `None` for a template that names nothing — whose
    /// repository carries the name CloudFormation generated.
    declared_name: Option<&'static str>,
    /// The `repositoryDescription` `GetRepository` must report, if the row declares one.
    expected_description: Option<&'static str>,
    /// The `kmsKeyId` it must report, if the row declares one.
    expected_kms_key_id: Option<&'static str>,
    /// The triggers `GetRepositoryTriggers` must report, built from what the row declares.
    expected_triggers: fn(&str) -> Value,
    /// The tags the repository's own declaration asks for.
    expected_tags: &'static [(&'static str, &'static str)],
    /// The output keys whose value must be the repository's NAME, each written with an
    /// intrinsic over the resource — "Ref returns the repository name".
    name_outputs: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "a named repository in JSON with a description, tags and one trigger on every \
               event",
        region: "us-east-1",
        template: |region| {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "a CodeCommit repository with a trigger",
                "Resources": {
                    "Repo": {
                        "Type": "AWS::CodeCommit::Repository",
                        "Properties": {
                            "RepositoryName": "orders-service",
                            "RepositoryDescription": "the orders service's sources",
                            "Tags": [
                                { "Key": "Owner", "Value": "orders" },
                                { "Key": "Tier", "Value": "prod" }
                            ],
                            "Triggers": [{
                                "Name": "notify-releases",
                                "DestinationArn": topic_arn(region, "releases"),
                                "Events": ["all"],
                                "Branches": []
                            }]
                        }
                    }
                },
                "Outputs": {
                    "RepoRef": { "Value": { "Ref": "Repo" } },
                    "RepoSub": {
                        "Value": { "Fn::Sub": ["${Name}", { "Name": { "Ref": "Repo" } }] }
                    }
                }
            })
            .to_string()
        },
        declared_name: Some("orders-service"),
        expected_description: Some("the orders service's sources"),
        expected_kms_key_id: None,
        expected_triggers: |region| {
            json!([{
                "name": "notify-releases",
                "destinationArn": topic_arn(region, "releases"),
                "events": ["all"],
                "branches": [],
            }])
        },
        expected_tags: &[("Owner", "orders"), ("Tier", "prod")],
        name_outputs: &["RepoRef", "RepoSub"],
    },
    Row {
        case: "a named repository in YAML with two triggers, one per destination kind",
        region: "us-west-2",
        template: |region| {
            [
                "Resources:".to_string(),
                "  Repo:".to_string(),
                "    Type: AWS::CodeCommit::Repository".to_string(),
                "    Properties:".to_string(),
                "      RepositoryName: billing-service".to_string(),
                "      Triggers:".to_string(),
                "        - Name: to-a-topic".to_string(),
                format!("          DestinationArn: {}", topic_arn(region, "pushes")),
                "          Events:".to_string(),
                "            - createReference".to_string(),
                "            - deleteReference".to_string(),
                "          Branches:".to_string(),
                "            - main".to_string(),
                "            - release/2026".to_string(),
                "          CustomData: team=platform".to_string(),
                "        - Name: to-a-function".to_string(),
                format!(
                    "          DestinationArn: {}",
                    function_arn(region, "on-push")
                ),
                "          Events:".to_string(),
                "            - updateReference".to_string(),
                "Outputs:".to_string(),
                "  RepoRef:".to_string(),
                "    Value: !Ref Repo".to_string(),
                "".to_string(),
            ]
            .join("\n")
        },
        declared_name: Some("billing-service"),
        expected_description: None,
        expected_kms_key_id: None,
        expected_triggers: |region| {
            json!([
                {
                    "name": "to-a-topic",
                    "destinationArn": topic_arn(region, "pushes"),
                    "events": ["createReference", "deleteReference"],
                    "branches": ["main", "release/2026"],
                    "customData": "team=platform",
                },
                {
                    "name": "to-a-function",
                    "destinationArn": function_arn(region, "on-push"),
                    "events": ["updateReference"],
                },
            ])
        },
        expected_tags: &[],
        name_outputs: &["RepoRef"],
    },
    Row {
        case: "a repository the template does not name, with an encryption key and no trigger",
        region: "eu-west-1",
        template: |_| {
            json!({
                "Resources": {
                    "Repo": {
                        "Type": "AWS::CodeCommit::Repository",
                        "Properties": {
                            "KmsKeyId": "arn:aws:kms:eu-west-1:000000000000:key/12345678-1234-1234-1234-123456789012",
                            "Tags": { "Owner": "platform" }
                        }
                    }
                },
                "Outputs": {
                    "RepoRef": { "Value": { "Ref": "Repo" } }
                }
            })
            .to_string()
        },
        declared_name: None,
        expected_description: None,
        expected_kms_key_id: Some(
            "arn:aws:kms:eu-west-1:000000000000:key/12345678-1234-1234-1234-123456789012",
        ),
        expected_triggers: |_| json!([]),
        expected_tags: &[("Owner", "platform")],
        name_outputs: &["RepoRef"],
    },
];

/// Every row: the name the stack reports for its `AWS::CodeCommit::Repository` is a
/// repository CodeCommit really holds, configured as the template declared it — including
/// the trigger configuration, which `GetRepositoryTriggers` reports as if
/// `PutRepositoryTriggers` had written it — and deleting the stack releases it there.
#[tokio::test]
async fn a_stack_provisions_the_codecommit_repository_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("codecommit-repo-stack-{index}");
        let template = (row.template)(region);

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template),
                ("Tags.member.1.Key", "Stack"),
                ("Tags.member.1.Value", "codecommit-suite"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let physical_id = opt_element(&resources, "PhysicalResourceId")
            .unwrap_or_else(|| panic!("case {case:?}: no <PhysicalResourceId> in {resources}"));

        // "Ref returns the repository name": the physical id IS the name the template asked
        // for, or the one CloudFormation generated for a template that named nothing — which
        // is held to the name CodeCommit accepts.
        if let Some(declared) = row.declared_name {
            assert_eq!(
                physical_id, declared,
                "case {case:?}: expected the physical id to be the declared name {declared:?}, \
                 got {physical_id:?}"
            );
        }
        assert!(
            !physical_id.is_empty()
                && physical_id.chars().count() <= MAX_REPOSITORY_NAME
                && physical_id
                    .chars()
                    .all(|character| character.is_alphanumeric()
                        || character == '_'
                        || character == '.'
                        || character == '-'),
            "case {case:?}: expected a name inside CodeCommit's own RepositoryName rule, got \
             {physical_id:?}"
        );

        // The value the STACK publishes for the resource is that same name, however the
        // template reads it ("Ref returns the repository name").
        let outputs = outputs_of(&described);
        for key in row.name_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&physical_id),
                "case {case:?}: expected output {key:?} to be the repository name \
                 {physical_id:?}, got {:?}",
                outputs.get(*key)
            );
        }

        // And CodeCommit itself resolves that name to the repository the template declared.
        let (status, repository) = codecommit(
            &router,
            region,
            "GetRepository",
            &json!({ "repositoryName": physical_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetRepository on the name the stack \
             reported, got {status} with body {repository}"
        );
        let metadata = repository
            .get("repositoryMetadata")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            metadata.get("repositoryName").and_then(Value::as_str),
            Some(physical_id.as_str()),
            "case {case:?}: expected the metadata to report the name {physical_id:?}, got \
             {metadata}"
        );
        let arn = repository_arn(region, &physical_id);
        assert_eq!(
            metadata.get("Arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the repository's ARN to be {arn}, got {metadata}"
        );
        assert_eq!(
            metadata
                .get("repositoryDescription")
                .and_then(Value::as_str),
            row.expected_description,
            "case {case:?}: expected the declared description, got {metadata}"
        );
        assert_eq!(
            metadata.get("kmsKeyId").and_then(Value::as_str),
            row.expected_kms_key_id,
            "case {case:?}: expected the declared encryption key, got {metadata}"
        );

        // The trigger configuration the template's `Triggers` declared, read through the
        // same call a client reads one with.
        let (status, configured) = codecommit(
            &router,
            region,
            "GetRepositoryTriggers",
            &json!({ "repositoryName": physical_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetRepositoryTriggers, got {status} with \
             body {configured}"
        );
        let expected_triggers = (row.expected_triggers)(region);
        assert_eq!(
            configured.get("triggers"),
            Some(&expected_triggers),
            "case {case:?}: expected the declared triggers {expected_triggers}, got {configured}"
        );
        let configuration_id = configured.get("configurationId").and_then(Value::as_str);
        assert_eq!(
            configuration_id.is_some(),
            !expected_triggers
                .as_array()
                .is_some_and(|triggers| triggers.is_empty()),
            "case {case:?}: a configurationId identifies a configuration that was written, \
             got {configured}"
        );

        // The tags the declaration asked for are on the repository, beside the stack's own.
        // The three `aws:cloudformation:*` tags are NOT: CodeCommit refuses that namespace
        // to a caller, and CloudFormation must not fail a stack over tags it adds itself.
        let (status, listed) = codecommit(
            &router,
            region,
            "ListTagsForResource",
            &json!({ "resourceArn": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource, got {status} with body \
             {listed}"
        );
        let reported: BTreeMap<String, String> = listed
            .get("tags")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case:?}: no 'tags' map in {listed}"))
            .iter()
            .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
            .collect();
        for (key, value) in row.expected_tags {
            assert_eq!(
                reported.get(*key).map(String::as_str),
                Some(*value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {reported:?}"
            );
        }
        assert_eq!(
            reported.get("Stack").map(String::as_str),
            Some("codecommit-suite"),
            "case {case:?}: expected the stack's own tag to be propagated, got {reported:?}"
        );
        assert!(
            !reported.keys().any(|key| key.starts_with("aws:")),
            "case {case:?}: expected no reserved tag key to be sent to CodeCommit, got \
             {reported:?}"
        );

        // Deleting the stack releases the repository CodeCommit holds for it.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = codecommit(
            &router,
            region,
            "GetRepository",
            &json!({ "repositoryName": physical_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from GetRepository once the stack deleted the \
             repository, got {status} with body {gone}"
        );
        assert_eq!(
            error_code(&gone),
            "RepositoryDoesNotExistException",
            "case {case:?}: expected RepositoryDoesNotExistException once the repository was \
             deleted, got {gone}"
        );
    }
}

/// A declaration CodeCommit REFUSES does not report a resource nothing created: the stack
/// fails with that service's own refusal, and nothing half-created is left behind.
///
/// The rows differ along WHAT is refused and by WHICH of the two calls the type's arm
/// makes: a repository name a client already holds (refused by the create), a trigger whose
/// destination is not an ARN and one whose destination is in another Region (both refused by
/// the trigger write, after the repository was created — so each also checks that the
/// repository that write was for is gone again), and a property this emulator maps onto no
/// call at all, which is refused rather than silently dropped.
#[tokio::test]
async fn a_repository_the_owning_service_refuses_fails_the_stack() {
    let router = build_router();
    let region = "us-east-2";

    // The repository a client already holds, which the first row's template re-declares.
    let (status, created) = codecommit(
        &router,
        region,
        "CreateRepository",
        &json!({
            "repositoryName": "already-there",
            "repositoryDescription": "the client's own repository",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the client's own CreateRepository to succeed, got {status} with body {created}"
    );

    // (case, the resource's declared properties, the repository name the declaration asks
    // for — which must hold no repository once the stack has failed, except where a client
    // created it).
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a repository name a client already holds",
            json!({ "RepositoryName": "already-there" }),
            "already-there",
        ),
        (
            "a trigger whose destination is not an ARN at all",
            json!({
                "RepositoryName": "repo-with-a-bad-trigger",
                "Triggers": [{
                    "Name": "broken",
                    "DestinationArn": "not-a-valid-sns-arn",
                    "Events": ["all"],
                }],
            }),
            "repo-with-a-bad-trigger",
        ),
        (
            "a trigger whose destination is in another Region",
            json!({
                "RepositoryName": "repo-with-a-foreign-trigger",
                "Triggers": [{
                    "Name": "elsewhere",
                    "DestinationArn": topic_arn("eu-central-1", "releases"),
                    "Events": ["all"],
                }],
            }),
            "repo-with-a-foreign-trigger",
        ),
        (
            "a property this emulator maps onto no call",
            json!({
                "RepositoryName": "repo-with-code",
                "Code": { "S3": { "Bucket": "a-bucket", "Key": "a-bundle.zip" } },
            }),
            "repo-with-code",
        ),
    ];

    for (index, (case, properties, repository)) in rows.into_iter().enumerate() {
        let stack_name = format!("codecommit-refusal-stack-{index}");
        let template = json!({
            "Resources": { "Repo": { "Type": "AWS::CodeCommit::Repository", "Properties": properties } }
        })
        .to_string();
        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;

        let (_, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the failed create to be rolled back, got {described}"
        );
        let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
        assert!(
            reason.contains("Repo"),
            "case {case:?}: expected the failure to name the resource, got {reason:?}"
        );

        let (status, read) = codecommit(
            &router,
            region,
            "GetRepository",
            &json!({ "repositoryName": repository }),
        )
        .await;
        if repository == "already-there" {
            // The repository the CLIENT created is still exactly as it was: the stack's
            // failure neither replaced nor removed it.
            assert_eq!(
                status, 200,
                "case {case:?}: expected the client's repository to still resolve, got {status} \
                 with body {read}"
            );
            assert_eq!(
                read.pointer("/repositoryMetadata/repositoryDescription")
                    .and_then(Value::as_str),
                Some("the client's own repository"),
                "case {case:?}: expected the client's own repository untouched, got {read}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected no repository to be left behind by the failed stack, \
                 got {status} with body {read}"
            );
            assert_eq!(
                error_code(&read),
                "RepositoryDoesNotExistException",
                "case {case:?}: expected RepositoryDoesNotExistException for the failed \
                 resource, got {read}"
            );
        }
    }
}

/// A stack that declares no resource of this type at all still reports no CodeCommit
/// repository: the arm is reached by the TYPE, not by anything in the request.
///
/// This is the control row of the rule above — a stack whose resource types this emulator
/// leaves declared-only must not create a repository, and must still complete.
#[tokio::test]
async fn a_stack_declaring_no_repository_creates_none() {
    let router = build_router();
    let region = "ap-northeast-1";
    let template = json!({
        "Resources": {
            "Waiter": {
                "Type": "AWS::CloudFormation::WaitConditionHandle",
                "Properties": {}
            }
        }
    })
    .to_string();
    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", "no-repository-stack"), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;
    let (_, described) = cloudformation(
        &router,
        region,
        "DescribeStacks",
        &[("StackName", "no-repository-stack")],
    )
    .await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "expected the stack to complete, got {described}"
    );

    let (status, listed) = codecommit(&router, region, "ListRepositories", &json!({})).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from ListRepositories, got {status} with body {listed}"
    );
    let repositories = listed
        .get("repositories")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        repositories.is_empty(),
        "expected no repository to exist, got {listed}"
    );
}
