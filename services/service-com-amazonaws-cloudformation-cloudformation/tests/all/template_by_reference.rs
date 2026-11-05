// SPDX-License-Identifier: CC0-1.0

//! A template submitted BY REFERENCE — `TemplateURL` — end to end over the awsQuery wire.
//!
//! RULE: a template reaches CloudFormation either inline, as the request's own
//! `TemplateBody`, or by reference, as a `TemplateURL` naming an Amazon S3 object ("the URL
//! must point to a template located in an Amazon S3 bucket … must start with `https://`").
//! The reference says WHERE the body is, not what it means: the object it addresses is read
//! and the operation then does exactly what the same body submitted inline would make it
//! do — the same change set, the same stack, the same verdict — while a URL addressing no
//! object this account can read is a `ValidationError` naming the bucket and key it
//! addressed.
//!
//! Every case drives the real request path for BOTH services through one router — the
//! CloudFormation calls over the awsQuery wire (the harness `stack_lifecycle.rs` owns) and
//! the Amazon S3 calls as a client makes them — so the template a stack deploys is an object
//! a client's own `PutObject` staged, which is what `cdk deploy` and
//! `aws cloudformation package` do before they deploy.
//!
//! The rows differ along what the rule generalises over: which ADDRESSING STYLE the URL
//! uses (virtual-hosted and path-style, regional and global), the spelling of the staged
//! template (JSON and YAML), the shape of the key (a bare name and a nested prefix), WHICH
//! operation takes the reference, and — for the refusals — what about the reference cannot
//! be resolved (a key that is not there, a bucket that is not there, a scheme that is not
//! supported, a URL carrying no key at all).

use serde_json::json;

use crate::change_set_lifecycle::{
    create_change_set, describe_change_set, execute_change_set, settle_the_store,
};
use crate::harness::{call, create_stack, get_template, member, router_with_engine, REGION};
use crate::stack_lifecycle::{describe_stack, xml_text, xml_texts};
use crate::stack_resource_provisioning::s3;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    change_action, change_set_status, status,
};
use smithy_server_runtime::MultiServiceRouter;

/// The bucket a client stages its templates in, as `cdk deploy` stages a synthesized
/// template in the bootstrap bucket before it creates the change set.
pub(crate) const STAGING_BUCKET: &str = "cfn-staging-templates";

/// The `Description` every staged template carries and NO request member does: an answer
/// reporting it can only have come from the object that was read.
const STAGED_DESCRIPTION: &str = "staged in Amazon S3, not inline";

/// Create the staging bucket, once per router.
///
/// `pub(crate)` because more than one suite stages a template object: `stack_refactors.rs`
/// submits a refactor's final state by reference, which is the same staging this suite
/// performs, so there is ONE way a test puts a template in Amazon S3.
pub(crate) async fn create_staging_bucket(router: &MultiServiceRouter) {
    let created = s3(router, "PUT", &format!("/{STAGING_BUCKET}"), b"").await;
    assert_eq!(
        created.status, 200,
        "the staging bucket must be created first, got {} {}",
        created.status, created.body
    );
}

/// Stage one template object, as a client's own `PutObject` stores it.
pub(crate) async fn stage(router: &MultiServiceRouter, key: &str, body: &str) {
    let stored = s3(
        router,
        "PUT",
        &format!("/{STAGING_BUCKET}/{key}"),
        body.as_bytes(),
    )
    .await;
    assert_eq!(
        stored.status, 200,
        "the template {key} must be staged first, got {} {}",
        stored.status, stored.body
    );
}

/// The addressing styles a `TemplateURL` may spell, each naming the SAME object.
///
/// Source: Amazon S3's documented request styles — virtual-hosted
/// (`https://<bucket>.s3.<region>.amazonaws.com/<key>`, with the global
/// `https://<bucket>.s3.amazonaws.com/<key>`) and path-style
/// (`https://s3.<region>.amazonaws.com/<bucket>/<key>`).
#[derive(Clone, Copy, Debug)]
enum Style {
    VirtualHostedRegional,
    VirtualHostedGlobal,
    PathStyleRegional,
    PathStyleGlobal,
}

impl Style {
    /// The URL this style spells for one object of the staging bucket.
    fn url(self, key: &str) -> String {
        match self {
            Style::VirtualHostedRegional => {
                format!("https://{STAGING_BUCKET}.s3.{REGION}.amazonaws.com/{key}")
            }
            Style::VirtualHostedGlobal => {
                format!("https://{STAGING_BUCKET}.s3.amazonaws.com/{key}")
            }
            Style::PathStyleRegional => {
                format!("https://s3.{REGION}.amazonaws.com/{STAGING_BUCKET}/{key}")
            }
            Style::PathStyleGlobal => format!("https://s3.amazonaws.com/{STAGING_BUCKET}/{key}"),
        }
    }
}

/// One JSON template declaring a single bucket, with the staged `Description`.
fn json_template(bucket_name: &str) -> String {
    json!({
        "Description": STAGED_DESCRIPTION,
        "Resources": {
            "Data": {
                "Type": "AWS::S3::Bucket",
                "Properties": {"BucketName": bucket_name}
            }
        }
    })
    .to_string()
}

/// The same template in the YAML spelling CloudFormation also reads.
fn yaml_template(bucket_name: &str) -> String {
    format!(
        "Description: {STAGED_DESCRIPTION}\nResources:\n  Data:\n    Type: AWS::S3::Bucket\n    \
         Properties:\n      BucketName: {bucket_name}\n"
    )
}

/// One row of the deploy table: a staged object, addressed in one style, deployed through a
/// change set.
struct Staged {
    case: &'static str,
    /// The key the template is staged under, which varies in shape as well as in name.
    key: &'static str,
    /// The template body staged under that key, in the spelling the row is about.
    body: String,
    style: Style,
    stack_name: &'static str,
    change_set_name: &'static str,
    /// The bucket the staged template declares, which must exist in Amazon S3 once the
    /// change set has been executed.
    declared_bucket: &'static str,
}

fn staged_rows() -> Vec<Staged> {
    vec![
        Staged {
            case: "a JSON template addressed virtual-hosted and regional (the CDK's form)",
            key: "cdk/assets/main.template.json",
            body: json_template("by-reference-json-regional"),
            style: Style::VirtualHostedRegional,
            stack_name: "by-reference-json-regional",
            change_set_name: "cs-json-regional",
            declared_bucket: "by-reference-json-regional",
        },
        Staged {
            case: "a JSON template addressed path-style and regional",
            key: "flat.json",
            body: json_template("by-reference-json-path"),
            style: Style::PathStyleRegional,
            stack_name: "by-reference-json-path",
            change_set_name: "cs-json-path",
            declared_bucket: "by-reference-json-path",
        },
        Staged {
            case: "a YAML template addressed virtual-hosted and global",
            key: "templates/nested/prefix/main.yaml",
            body: yaml_template("by-reference-yaml-global"),
            style: Style::VirtualHostedGlobal,
            stack_name: "by-reference-yaml-global",
            change_set_name: "cs-yaml-global",
            declared_bucket: "by-reference-yaml-global",
        },
        Staged {
            case: "a YAML template addressed path-style and global",
            key: "main.yml",
            body: yaml_template("by-reference-yaml-path"),
            style: Style::PathStyleGlobal,
            stack_name: "by-reference-yaml-path",
            change_set_name: "cs-yaml-path",
            declared_bucket: "by-reference-yaml-path",
        },
    ]
}

#[tokio::test]
async fn a_change_set_created_from_a_staged_template_deploys_the_stack_it_declares() {
    let (router, engine) = router_with_engine();
    create_staging_bucket(&router).await;

    for row in staged_rows() {
        let case = row.case;
        stage(&router, row.key, &row.body).await;
        let url = row.style.url(row.key);

        let (code, body) = create_change_set(
            &router,
            row.stack_name,
            row.change_set_name,
            &[
                member("ChangeSetType", "CREATE"),
                member("TemplateURL", &url),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: CreateChangeSet({url}) must create the change set, got {code} {body}"
        );
        let stack_id = xml_text(&body, "StackId").unwrap_or_default();
        assert!(
            !stack_id.is_empty(),
            "case {case:?}: expected a StackId for the change set's stack, got {body}"
        );

        // The change set carries the STAGED body, byte for byte: nothing it plans from came
        // from the request, which named only the URL.
        let (code, body) = call(
            &router,
            "GetTemplate",
            &[
                member("ChangeSetName", row.change_set_name),
                member("StackName", row.stack_name),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the change set's template must be readable, got {code} {body}"
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(row.body.as_str()),
            "case {case:?}: expected the staged body, got {body}"
        );

        // The plan is over the resources the staged template declares.
        settle_the_store(&router, &engine).await;
        let (code, body) =
            describe_change_set(&router, row.change_set_name, Some(row.stack_name)).await;
        assert_eq!(
            code, 200,
            "case {case:?}: DescribeChangeSet must answer, got {code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(change_set_status::CREATE_COMPLETE),
            "case {case:?}: a change set with changes completes, got {body}"
        );
        assert_eq!(
            xml_texts(&body, "Action"),
            vec![change_action::ADD.to_string()],
            "case {case:?}: the staged template's one resource is an addition, got {body}"
        );
        assert_eq!(
            xml_texts(&body, "LogicalResourceId"),
            vec!["Data".to_string()],
            "case {case:?}: the change names the staged resource, got {body}"
        );

        // Executing the plan deploys that template: the stack completes and the bucket it
        // declares exists in Amazon S3.
        let (code, body) =
            execute_change_set(&router, row.change_set_name, Some(row.stack_name)).await;
        assert_eq!(
            code, 200,
            "case {case:?}: ExecuteChangeSet must succeed, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        let (code, body) = describe_stack(&router, row.stack_name).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the stack must stay readable, got {code} {body}"
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {case:?}: the staged template's stack must complete, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Description").as_deref(),
            Some(STAGED_DESCRIPTION),
            "case {case:?}: the stack reports the staged template's Description, got {body}"
        );
        let held = s3(&router, "HEAD", &format!("/{}", row.declared_bucket), b"").await;
        assert_eq!(
            held.status, 200,
            "case {case:?}: the bucket the staged template declares must exist, got {} {}",
            held.status, held.body
        );

        // The executed stack holds the staged body, so a later read of the stack resolves
        // it without reading Amazon S3 again.
        let (code, body) = get_template(&router, row.stack_name, None).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the stack's template must be readable, got {code} {body}"
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(row.body.as_str()),
            "case {case:?}: the staged body became the stack's, got {body}"
        );
    }
}

/// How a row's call is read back: the operation that stores what it read, and the member
/// that must carry the staged body.
enum ReadBack {
    /// Nothing is stored: the answer itself is the whole of what the call produced.
    Answer,
    /// `GetTemplate(StackName)` must answer the staged body.
    StackTemplate(&'static str),
    /// `DescribeStackSet(StackSetName)` must report the staged body.
    StackSetTemplate(&'static str),
}

/// One row of the operations table: an operation that takes a `TemplateURL`, and what the
/// staged body must have reached.
struct ByReference {
    case: &'static str,
    action: &'static str,
    members: Vec<(String, String)>,
    /// A tag of the ANSWER whose text must be the staged `Description`, where the operation
    /// reports on the template it read.
    described_in: Option<&'static str>,
    readback: ReadBack,
}

#[tokio::test]
async fn every_operation_that_takes_a_template_url_answers_from_the_staged_object() {
    let (router, engine) = router_with_engine();
    create_staging_bucket(&router).await;
    let key = "shared/main.template.json";
    let staged = json_template("by-reference-shared-bucket");
    stage(&router, key, &staged).await;
    let url = Style::VirtualHostedRegional.url(key);

    // `UpdateStack` revises a stack that exists, so one is created inline first — with a
    // template that differs from the staged one only in its `Description`, which is what
    // makes the update an update.
    let inline = staged.replace(STAGED_DESCRIPTION, "inline, before the staged revision");
    let (code, body) = create_stack(&router, "by-reference-updated", &inline, &[]).await;
    assert_eq!(
        code, 200,
        "the stack an update revises must be created first, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;

    let rows: Vec<ByReference> = vec![
        ByReference {
            case: "ValidateTemplate",
            action: "ValidateTemplate",
            members: vec![member("TemplateURL", &url)],
            described_in: Some("Description"),
            readback: ReadBack::Answer,
        },
        ByReference {
            case: "GetTemplateSummary",
            action: "GetTemplateSummary",
            members: vec![member("TemplateURL", &url)],
            described_in: Some("Description"),
            readback: ReadBack::Answer,
        },
        ByReference {
            case: "EstimateTemplateCost",
            action: "EstimateTemplateCost",
            members: vec![member("TemplateURL", &url)],
            // The estimate's answer is the calculator URL of the estimate it saved, not the
            // template's own text.
            described_in: None,
            readback: ReadBack::Answer,
        },
        ByReference {
            case: "CreateStack",
            action: "CreateStack",
            members: vec![
                member("StackName", "by-reference-created"),
                member("TemplateURL", &url),
            ],
            described_in: None,
            readback: ReadBack::StackTemplate("by-reference-created"),
        },
        ByReference {
            case: "UpdateStack",
            action: "UpdateStack",
            members: vec![
                member("StackName", "by-reference-updated"),
                member("TemplateURL", &url),
            ],
            described_in: None,
            readback: ReadBack::StackTemplate("by-reference-updated"),
        },
        ByReference {
            case: "CreateStackSet",
            action: "CreateStackSet",
            members: vec![
                member("StackSetName", "by-reference-set"),
                member("TemplateURL", &url),
            ],
            described_in: None,
            readback: ReadBack::StackSetTemplate("by-reference-set"),
        },
    ];

    for row in &rows {
        let case = row.case;
        let (code, body) = call(&router, row.action, &row.members).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the staged template must be read, got {code} {body}"
        );
        if let Some(tag) = row.described_in {
            assert_eq!(
                xml_text(&body, tag).as_deref(),
                Some(STAGED_DESCRIPTION),
                "case {case:?}: expected the staged template's {tag}, got {body}"
            );
        }
        if matches!(row.case, "EstimateTemplateCost") {
            let estimate = xml_text(&body, "Url").unwrap_or_default();
            assert!(
                estimate.starts_with("https://"),
                "case {case:?}: expected a calculator URL for the staged template, got \
                 {estimate:?}"
            );
        }
        settle_the_store(&router, &engine).await;
        match row.readback {
            ReadBack::Answer => {}
            ReadBack::StackTemplate(stack_name) => {
                let (code, body) = get_template(&router, stack_name, None).await;
                assert_eq!(
                    code, 200,
                    "case {case:?}: the stack's template must be readable, got {code} {body}"
                );
                assert_eq!(
                    xml_text(&body, "TemplateBody").as_deref(),
                    Some(staged.as_str()),
                    "case {case:?}: expected the staged body on the stack, got {body}"
                );
            }
            ReadBack::StackSetTemplate(stack_set_name) => {
                let (code, body) = call(
                    &router,
                    "DescribeStackSet",
                    &[member("StackSetName", stack_set_name)],
                )
                .await;
                assert_eq!(
                    code, 200,
                    "case {case:?}: the StackSet must be readable, got {code} {body}"
                );
                assert_eq!(
                    xml_text(&body, "TemplateBody").as_deref(),
                    Some(staged.as_str()),
                    "case {case:?}: expected the staged body on the StackSet, got {body}"
                );
            }
        }
    }
}

/// One row of the refusal table: a reference that resolves to no readable object, and the
/// message CloudFormation answers for it.
struct Unresolvable {
    case: &'static str,
    url: String,
    /// Text the `ValidationError` message must carry.
    message: String,
}

/// The `ValidationError` message CloudFormation answers for a read Amazon S3 refused without
/// establishing that the object is absent: what Amazon S3 said, and the line that error is
/// documented on.
fn s3_error(message: &str) -> String {
    format!(
        "S3 error: {message}\nFor more information check \
         http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html"
    )
}

#[tokio::test]
async fn a_template_url_that_resolves_to_no_readable_object_is_refused() {
    let (router, _engine) = router_with_engine();
    create_staging_bucket(&router).await;
    let key = "present.template.json";
    stage(&router, key, &json_template("by-reference-present")).await;

    let rows: Vec<Unresolvable> = vec![
        Unresolvable {
            // A key the bucket does not hold is an absence CloudFormation could see, and
            // reports as that.
            case: "a key the staging bucket does not hold",
            url: Style::VirtualHostedRegional.url("absent.template.json"),
            message: format!(
                "S3 object does not exist. Bucket: {STAGING_BUCKET}, Key: \
                 absent.template.json. Error: NoSuchKey"
            ),
        },
        Unresolvable {
            // The same absence, for a well-formed version id the bucket holds no version
            // under: the code Amazon S3 named for it travels into the same sentence.
            case: "an object version the staging bucket does not hold",
            url: format!(
                "{}?versionId=3HL4kqtJlcpXroDTDmJ1rmSpXd3dIbrH",
                Style::VirtualHostedRegional.url(key)
            ),
            message: format!(
                "S3 object does not exist. Bucket: {STAGING_BUCKET}, Key: {key}. Error: \
                 NoSuchVersion"
            ),
        },
        Unresolvable {
            // A bucket that is not there establishes nothing about an object, so what
            // CloudFormation reports is what Amazon S3 answered.
            case: "a bucket nothing created",
            url: format!("https://no-such-staging-bucket.s3.{REGION}.amazonaws.com/{key}"),
            message: s3_error("The specified bucket does not exist"),
        },
        Unresolvable {
            case: "a scheme the documentation does not support",
            url: format!("s3://{STAGING_BUCKET}/{key}"),
            message: "TemplateURL must be a supported URL.".to_string(),
        },
        Unresolvable {
            case: "an https URL carrying no key at all",
            url: format!("https://{STAGING_BUCKET}.s3.{REGION}.amazonaws.com/"),
            message: "TemplateURL must reference a valid S3 object to which you have access."
                .to_string(),
        },
    ];

    // The rule is the same for every operation that takes the reference, so each row is
    // driven through two of them: the one the CDK uses and the one a plain deploy uses.
    for row in &rows {
        let case = row.case;
        for (action, members) in [
            (
                "CreateChangeSet",
                vec![
                    member("StackName", "refused-by-reference"),
                    member("ChangeSetName", "cs-refused"),
                    member("ChangeSetType", "CREATE"),
                    member("TemplateURL", &row.url),
                ],
            ),
            (
                "CreateStack",
                vec![
                    member("StackName", "refused-by-reference"),
                    member("TemplateURL", &row.url),
                ],
            ),
        ] {
            let (code, body) = call(&router, action, &members).await;
            assert_eq!(
                code, 400,
                "case {case:?}: {action} must refuse the reference, got {code} {body}"
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?}: {action} must answer a ValidationError, got {body}"
            );
            let message = xml_text(&body, "Message").unwrap_or_default();
            assert!(
                message.contains(row.message.as_str()),
                "case {case:?}: {action} must name what could not be resolved ({:?}), got \
                 {message:?}",
                row.message
            );
        }
    }

    // Nothing above created a stack: a refused reference writes nothing.
    let (code, body) = describe_stack(&router, "refused-by-reference").await;
    assert_eq!(
        code, 400,
        "case \"the refused stack name\": expected no stack to exist, got {code} {body}"
    );
}
