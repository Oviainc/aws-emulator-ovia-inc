// SPDX-License-Identifier: CC0-1.0

//! What `CreateCloudFormationChangeSet` deploys, end to end over HTTP.
//!
//! Three rules, each asserted over a TABLE of inputs rather than one recorded request:
//!
//! 1. a change set deploys the template of ONE published version of the application —
//!    the version the request names, or the application's latest when it names none —
//!    and what the call answers with is a real AWS CloudFormation change set over THAT
//!    template, for the stack the request names;
//! 2. a deployment must acknowledge the capabilities the version's template requires,
//!    derived from the resources the template declares against the catalogue the
//!    `Capabilities` member documents, and the same list is what
//!    `Version$RequiredCapabilities` reports;
//! 3. a request that names no deployable version — an application with none, a version
//!    the application does not carry, an application the catalogue does not hold — is
//!    refused with the modelled `BadRequestException`, and NOTHING is created in
//!    CloudFormation;
//! 4. a deployment records which application, and which version of it, it was made from
//!    on the change set and so on the stack, and acknowledges `CAPABILITY_IAM` to
//!    CloudFormation for every deployment it makes;
//! 5. the acknowledgement gate decides between two outcomes and no third: an accepted
//!    deployment leaves the stack `serverlessrepo-<StackName>` in `REVIEW_IN_PROGRESS`
//!    with an executable change set, and a refused one leaves no stack at all — asserted
//!    over versions of ONE application whose derived requirement differs.
//!
//! The emulator under test mounts this service and CloudFormation on ONE engine, which
//! is what the assembled emulator does: the handler creates the change set in
//! CloudFormation over the in-process callback, so every assertion below reads it back
//! from CloudFormation over the awsQuery wire exactly as a client does.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::ComAmazonawsCloudformationCloudformationState;
use service_com_amazonaws_cloudformation_cloudformation::EmulatedService as CloudFormation;
use service_com_amazonaws_serverlessapplicationrepository_serverlessapplicationrepository::state::ComAmazonawsServerlessapplicationrepositoryServerlessapplicationrepositoryState;
use service_com_amazonaws_serverlessapplicationrepository_serverlessapplicationrepository::EmulatedService as ServerlessRepo;
use smithy_server_runtime::MultiServiceRouter;

use crate::applications::{minimal, path_label};
use crate::version_templates::SAM_TRANSFORM;

/// The region every request below is signed for, and so the region every minted ARN
/// carries. Deliberately not the emulator's `us-east-1` default.
const REGION: &str = "us-west-2";

/// The canonical account id every emulated ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One emulator: the Serverless Application Repository and CloudFormation on a single
/// shared engine, driven through the same `dispatch` the production front door uses.
struct Deploy {
    router: MultiServiceRouter,
    /// The one shared engine, so a case can make a transition CloudFormation owes due
    /// without waiting for the wall clock.
    engine: Arc<engine::InMemoryEngine>,
}

impl Deploy {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here; the handlers are.
        router.set_request_authentication(false);
        router.register_service(ServerlessRepo::new(Arc::clone(&engine)));
        router.register_service(CloudFormation::new(Arc::clone(&engine)));
        // Executing a change set makes CloudFormation provision the template's resources
        // in the service that OWNS each type, so Amazon SQS — the owner of the
        // `AWS::SQS::Queue` the executed deployment below declares — is mounted beside the
        // two, exactly as `core/router/src/lib.rs` mounts them. (A stack whose resources
        // reach no owning service ends in `CREATE_FAILED`, which the CloudFormation
        // crate's `unprovisioned_resources.rs` pins; the rule under test here needs the
        // stack to exist, so the owner is mounted rather than the failure asserted.)
        router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router, engine }
    }

    /// Make every status transition CloudFormation's store owes due NOW, so an ordinary
    /// read materialises it exactly as the clock would have.
    ///
    /// A change set's creation and a stack operation's completion each become due a
    /// moment after the call that starts them; pulling the due instants into the past is
    /// what keeps a case about the emulator's own settling rather than a race with the
    /// machine's load.
    fn settle_cloudformation(&self) {
        let state = self
            .engine
            .state::<ComAmazonawsCloudformationCloudformationState>();
        let mut held = state.lock();
        let due = SystemTime::now() - Duration::from_secs(1);
        for stack in &mut held.stacks {
            if let Some(pending) = stack.pending.as_mut() {
                pending.due_at = due;
            }
        }
        for change_set in &mut held.change_sets {
            if let Some(pending) = change_set.pending.as_mut() {
                pending.due_at = due;
            }
        }
    }

    /// A restJson1 call to the Serverless Application Repository, answered with the
    /// status and the parsed body.
    async fn repo(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = match body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/serverlessrepo/\
                     aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .header("content-type", "application/json")
            .body(payload)
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.body().to_vec();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// An awsQuery call to CloudFormation — form-encoded body in, XML envelope out —
    /// exactly as an SDK client makes it.
    async fn cloudformation(&self, action: &str, members: &[(&str, &str)]) -> (u16, String) {
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
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/cloudformation/\
                     aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8(response.into_body().to_vec()).expect("a UTF-8 body");
        (status, body)
    }

    /// Publish `name` with one version per `(semantic_version, template)` entry, and
    /// answer the minted `ApplicationId`.
    async fn given_application(&self, name: &str, versions: &[(&str, Value)]) -> String {
        let (status, body) = self.repo("POST", "/applications", Some(minimal(name))).await;
        assert_eq!(status, 201, "creating {name:?}: expected 201, got {body}");
        let application_id = text(&body, "applicationId");
        for (semantic_version, template) in versions {
            let path = format!(
                "/applications/{}/versions/{semantic_version}",
                path_label(&application_id)
            );
            let (status, body) = self
                .repo(
                    "PUT",
                    &path,
                    Some(json!({ "templateBody": template.to_string() })),
                )
                .await;
            assert_eq!(
                status, 201,
                "publishing {name:?} {semantic_version}: expected 201, got {body}"
            );
        }
        application_id
    }

    /// Prepare a CloudFormation template of `semantic_version`, the call that mints the
    /// `TemplateId` a deployment may name.
    async fn create_template(&self, application_id: &str, semantic_version: &str) -> (u16, Value) {
        let path = format!("/applications/{}/templates", path_label(application_id));
        self.repo(
            "POST",
            &path,
            Some(json!({ "semanticVersion": semantic_version })),
        )
        .await
    }

    /// End the life of every prepared template, the way the CLOCK ends it at its own
    /// `ExpirationTime`: a case that waited six hours for it would not finish. A template
    /// whose life has ended is the one status a deploy refuses, now that a create reports
    /// `ACTIVE` from its own response.
    fn expire_templates(&self) {
        let state = self
            .engine
            .state::<ComAmazonawsServerlessapplicationrepositoryServerlessapplicationrepositoryState>();
        let mut held = state.lock();
        let gone = SystemTime::now() - Duration::from_secs(1);
        for template in held.templates.values_mut() {
            template.expires_at = gone;
        }
    }

    async fn get_application(&self, application_id: &str, semantic_version: &str) -> Value {
        let path = format!(
            "/applications/{}?semanticVersion={semantic_version}",
            path_label(application_id)
        );
        let (status, body) = self.repo("GET", &path, None).await;
        assert_eq!(
            status, 200,
            "reading {application_id} {semantic_version}: expected 200, got {body}"
        );
        body
    }

    async fn create_change_set(&self, application_id: &str, body: Value) -> (u16, Value) {
        let path = format!("/applications/{}/changesets", path_label(application_id));
        self.repo("POST", &path, Some(body)).await
    }
}

/// Percent-encode one form value, so a template body carrying `{`, `"` or `&` survives
/// the body of an awsQuery request.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// A string member of a JSON object, or `""` when it is absent.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The wire error code restJson1 carries as the body's `__type`.
fn error_code(body: &Value) -> String {
    text(body, "__type")
}

/// Undo the XML text escaping the awsQuery serializer applies.
fn xml_unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Every `<tag>` text in an XML document, in document order.
fn xml_texts(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(xml_unescape(&after[..end]));
        rest = &after[end + close.len()..];
    }
    found
}

/// The first `<tag>` text, or `""` when the document carries none.
fn xml_text(xml: &str, tag: &str) -> String {
    xml_texts(xml, tag).into_iter().next().unwrap_or_default()
}

/// The `<member>` texts inside the first `<tag>` block — how awsQuery renders a list, read
/// out of the block so a `<member>` of some other list cannot be mistaken for one of these.
fn xml_list(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let Some(start) = xml.find(&open) else {
        return Vec::new();
    };
    let after = &xml[start + open.len()..];
    let Some(end) = after.find(&close) else {
        return Vec::new();
    };
    xml_texts(&after[..end], "member")
}

/// The `(Key, Value)` tag pairs inside the document's first `<Tags>` block — how awsQuery
/// renders a tag list, read out of the block so another list's members cannot be mistaken
/// for tags.
fn xml_tags(xml: &str) -> Vec<(String, String)> {
    let Some(start) = xml.find("<Tags>") else {
        return Vec::new();
    };
    let after = &xml[start + "<Tags>".len()..];
    let Some(end) = after.find("</Tags>") else {
        return Vec::new();
    };
    let block = &after[..end];
    xml_texts(block, "Key")
        .into_iter()
        .zip(xml_texts(block, "Value"))
        .collect()
}

/// CloudFormation's `TemplateStage` value naming the body as SUBMITTED (model shape
/// `TemplateStage`: `Original` | `Processed`, whose doc gives `Processed` as the default).
/// The published body is read back under this stage, since the other one is that body with
/// the SAM transform applied.
const TEMPLATE_STAGE_ORIGINAL: &str = "Original";

/// A SAM template over `resources`: a packaged AWS SAM template is a CloudFormation
/// template declaring the SAM transform.
fn sam_template(resources: Value) -> Value {
    json!({ "Transform": SAM_TRANSFORM, "Resources": resources })
}

/// A one-queue SAM template, the smallest publishable application.
fn one_queue() -> Value {
    sam_template(json!({ "Queue": { "Type": "AWS::SQS::Queue" } }))
}

/// What a deployment acknowledges to CloudFormation when it acknowledged `acknowledged`
/// to this service: `CAPABILITY_IAM` for every deployment, then each value the request
/// named that CloudFormation's own `Capability` enum declares, once
/// (`CAPABILITY_RESOURCE_POLICY` is this service's own and is not one of them).
fn forwarded_capabilities(acknowledged: &[&str]) -> Vec<String> {
    let mut forwarded = vec![CAPABILITY_IAM.to_string()];
    for capability in acknowledged {
        if *capability == "CAPABILITY_RESOURCE_POLICY" {
            continue;
        }
        if !forwarded.iter().any(|held| held == capability) {
            forwarded.push((*capability).to_string());
        }
    }
    forwarded
}

/// The acknowledgement a deployment of a published application always carries.
const CAPABILITY_IAM: &str = "CAPABILITY_IAM";

/// The two tags a deployment records its own provenance under, in the order the change
/// set carries them: which application was deployed, and which version of it.
fn provenance_tags(application_id: &str, semantic_version: &str) -> Vec<(String, String)> {
    vec![
        (
            "serverlessrepo:applicationId".to_string(),
            application_id.to_string(),
        ),
        (
            "serverlessrepo:semanticVersion".to_string(),
            semantic_version.to_string(),
        ),
    ]
}

/// The name the deployment's stack is held under in CloudFormation: this service owns
/// the stacks it deploys, under `serverlessrepo-<StackName>`.
fn deployed_stack(stack_name: &str) -> String {
    format!("serverlessrepo-{stack_name}")
}

/// Assert a change set ARN is the form CloudFormation mints:
/// `arn:aws:cloudformation:<region>:<account>:changeSet/<name>/<uuid>`.
fn assert_change_set_arn(case: &str, change_set_id: &str) {
    let prefix = format!("arn:aws:cloudformation:{REGION}:{ACCOUNT}:changeSet/");
    assert!(
        change_set_id.starts_with(&prefix),
        "case {case:?}: expected a change set ARN starting {prefix:?}, got {change_set_id:?}"
    );
    let rest = &change_set_id[prefix.len()..];
    let mut segments = rest.split('/');
    let name = segments.next().unwrap_or_default();
    let uuid = segments.next().unwrap_or_default();
    assert!(
        !name.is_empty()
            && name.starts_with(|c: char| c.is_ascii_alphabetic())
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "case {case:?}: the change set name must satisfy CloudFormation's \
         [a-zA-Z][-a-zA-Z0-9]*, got {name:?}"
    );
    assert!(
        uuid.len() == 36 && uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        "case {case:?}: expected a 36-character uuid after the name, got {uuid:?}"
    );
    assert!(
        segments.next().is_none(),
        "case {case:?}: expected exactly <name>/<uuid> after the ARN prefix, got {rest:?}"
    );
}

/// One row of the deployment table: the case name, the versions the application
/// publishes, the `SemanticVersion` the request names (if any) and the version that must
/// therefore be deployed.
type DeploymentCase<'a> = (&'a str, Vec<(&'a str, Value)>, Option<&'a str>, &'a str);

/// RULE 1: the change set deploys the template of the version the request names — the
/// application's latest when it names none — and it is a CloudFormation change set over
/// that very template, for the stack the request named.
///
/// The rows differ in how many versions the application carries, which one the request
/// selects (none, the latest explicitly, an older one, a prerelease), and how many
/// resources of which types the deployed template declares.
#[tokio::test]
async fn a_change_set_deploys_the_named_versions_template() {
    let deploy = Deploy::new();

    // (case, published versions, requested semanticVersion, the version it must deploy)
    let cases: Vec<DeploymentCase> = vec![
        (
            "one version, none named",
            vec![("1.0.0", one_queue())],
            None,
            "1.0.0",
        ),
        (
            "three versions, none named: the latest is deployed",
            vec![
                ("1.0.0", one_queue()),
                (
                    "1.2.0",
                    sam_template(json!({ "Topic": { "Type": "AWS::SNS::Topic" } })),
                ),
                (
                    "2.0.1",
                    sam_template(json!({
                        "Queue": { "Type": "AWS::SQS::Queue" },
                        "Topic": { "Type": "AWS::SNS::Topic" },
                        "Bucket": { "Type": "AWS::S3::Bucket" }
                    })),
                ),
            ],
            None,
            "2.0.1",
        ),
        (
            "three versions, an older one named",
            vec![
                ("1.0.0", one_queue()),
                (
                    "1.2.0",
                    sam_template(json!({ "Topic": { "Type": "AWS::SNS::Topic" } })),
                ),
                ("2.0.1", one_queue()),
            ],
            Some("1.2.0"),
            "1.2.0",
        ),
        (
            "a prerelease version named",
            vec![
                ("1.0.0", one_queue()),
                (
                    "2.1.0-beta.1",
                    sam_template(json!({
                        "Table": { "Type": "AWS::DynamoDB::Table" },
                        "Queue": { "Type": "AWS::SQS::Queue" }
                    })),
                ),
            ],
            Some("2.1.0-beta.1"),
            "2.1.0-beta.1",
        ),
    ];

    for (index, (case, versions, requested, deployed)) in cases.iter().enumerate() {
        let application_id = deploy
            .given_application(&format!("deploy-app-{index}"), versions)
            .await;
        let stack_name = format!("deploy-stack-{index}");
        let mut body = json!({ "stackName": stack_name });
        if let Some(requested) = requested {
            body["semanticVersion"] = json!(requested);
        }

        let (status, answer) = deploy.create_change_set(&application_id, body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {answer}"
        );
        assert_eq!(
            text(&answer, "applicationId"),
            application_id,
            "case {case:?}: the answer must name the application it deployed, got {answer}"
        );
        assert_eq!(
            text(&answer, "semanticVersion"),
            *deployed,
            "case {case:?}: expected version {deployed} to be deployed, got {answer}"
        );
        let change_set_id = text(&answer, "changeSetId");
        assert_change_set_arn(case, &change_set_id);
        let stack_id = text(&answer, "stackId");
        let stack_prefix = format!(
            "arn:aws:cloudformation:{REGION}:{ACCOUNT}:stack/{}/",
            deployed_stack(&stack_name)
        );
        assert!(
            stack_id.starts_with(&stack_prefix),
            "case {case:?}: expected a stack ARN starting {stack_prefix:?}, got {stack_id:?}"
        );

        // The change set CloudFormation holds under that ARN is over the template the
        // named version published — read back through CloudFormation's own GetTemplate.
        // The published body VERBATIM is the `Original` stage: a packaged SAM template
        // declares a transform, so CloudFormation's other stage (`Processed`, its default)
        // is that body with the transform applied rather than the body this service
        // published.
        let expected_body = versions
            .iter()
            .find(|(semantic_version, _)| semantic_version == deployed)
            .map(|(_, template)| template.to_string())
            .expect("the deployed version is one of the published ones");
        let (status, xml) = deploy
            .cloudformation(
                "GetTemplate",
                &[
                    ("ChangeSetName", &change_set_id),
                    ("TemplateStage", TEMPLATE_STAGE_ORIGINAL),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the change set's template must be readable, got {status} {xml}"
        );
        assert_eq!(
            xml_text(&xml, "TemplateBody"),
            expected_body,
            "case {case:?}: the change set must carry the published template"
        );

        let (status, xml) = deploy
            .cloudformation("DescribeChangeSet", &[("ChangeSetName", &change_set_id)])
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the change set must be describable, got {status} {xml}"
        );
        assert_eq!(
            xml_text(&xml, "StackId"),
            stack_id,
            "case {case:?}: the change set must belong to the stack the call answered with"
        );
        let mut planned = xml_texts(&xml, "LogicalResourceId");
        planned.sort();
        let mut declared: Vec<String> = expected_resource_names(&expected_body);
        declared.sort();
        assert_eq!(
            planned, declared,
            "case {case:?}: the change set must plan exactly the template's resources"
        );
    }
}

/// The logical ids the `Resources` of a template body declares.
fn expected_resource_names(template_body: &str) -> Vec<String> {
    serde_json::from_str::<Value>(template_body)
        .expect("the test's own template is JSON")
        .get("Resources")
        .and_then(Value::as_object)
        .map(|resources| resources.keys().cloned().collect())
        .unwrap_or_default()
}

/// RULE 2: a deployment must acknowledge the capabilities the version's template
/// requires, and `Version$RequiredCapabilities` reports that same list.
///
/// The catalogue is the `Capabilities` member's own documentation
/// (`smithy_query(query="doc", shape="CreateCloudFormationChangeSetRequest",
/// member="Capabilities")`): the four IAM resource types, the six resource-policy types,
/// and a nested application. The rows cover one entry of each list — including
/// `AWS::IAM::Policy`, which is in BOTH — a named and an unnamed IAM resource, a template
/// that requires nothing, and a template that requires three capabilities at once.
#[tokio::test]
async fn a_deployment_must_acknowledge_the_templates_required_capabilities() {
    let deploy = Deploy::new();

    // (case, the template's Resources, the capabilities it requires)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "a queue requires nothing",
            json!({ "Queue": { "Type": "AWS::SQS::Queue" } }),
            vec![],
        ),
        (
            "an unnamed role requires CAPABILITY_IAM",
            json!({ "Role": { "Type": "AWS::IAM::Role" } }),
            vec!["CAPABILITY_IAM"],
        ),
        (
            "a role with a custom name requires CAPABILITY_NAMED_IAM",
            json!({
                "Role": {
                    "Type": "AWS::IAM::Role",
                    "Properties": { "RoleName": "my-own-role" }
                }
            }),
            vec!["CAPABILITY_NAMED_IAM"],
        ),
        (
            "an instance profile with a custom name requires CAPABILITY_NAMED_IAM",
            json!({
                "Profile": {
                    "Type": "AWS::IAM::InstanceProfile",
                    "Properties": { "InstanceProfileName": "my-own-profile" }
                }
            }),
            vec!["CAPABILITY_NAMED_IAM"],
        ),
        (
            "a queue policy requires CAPABILITY_RESOURCE_POLICY",
            json!({ "Policy": { "Type": "AWS::SQS::QueuePolicy" } }),
            vec!["CAPABILITY_RESOURCE_POLICY"],
        ),
        (
            "a lambda permission requires CAPABILITY_RESOURCE_POLICY",
            json!({ "Invoke": { "Type": "AWS::Lambda::Permission" } }),
            vec!["CAPABILITY_RESOURCE_POLICY"],
        ),
        (
            "an IAM policy is in both lists",
            json!({ "Attached": { "Type": "AWS::IAM::Policy" } }),
            vec!["CAPABILITY_IAM", "CAPABILITY_RESOURCE_POLICY"],
        ),
        (
            "a nested application requires CAPABILITY_AUTO_EXPAND",
            json!({ "Nested": { "Type": "AWS::Serverless::Application" } }),
            vec!["CAPABILITY_AUTO_EXPAND"],
        ),
        (
            "three capabilities at once",
            json!({
                "Role": {
                    "Type": "AWS::IAM::Role",
                    "Properties": { "RoleName": "my-own-role" }
                },
                "Group": { "Type": "AWS::IAM::Group" },
                "Bucket": { "Type": "AWS::S3::BucketPolicy" },
                "Nested": { "Type": "AWS::Serverless::Application" }
            }),
            vec![
                "CAPABILITY_AUTO_EXPAND",
                "CAPABILITY_NAMED_IAM",
                "CAPABILITY_RESOURCE_POLICY",
            ],
        ),
    ];

    for (index, (case, resources, required)) in cases.iter().enumerate() {
        let application_id = deploy
            .given_application(
                &format!("capability-app-{index}"),
                &[("1.0.0", sam_template(resources.clone()))],
            )
            .await;

        // The reporting side: the published version declares the same requirement.
        let version = deploy.get_application(&application_id, "1.0.0").await;
        let reported: Vec<String> = version["version"]["requiredCapabilities"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|capability| capability.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported, *required,
            "case {case:?}: expected requiredCapabilities {required:?}, got {version}"
        );

        // Acknowledging nothing: accepted only when the template requires nothing.
        let (status, answer) = deploy
            .create_change_set(
                &application_id,
                json!({ "stackName": format!("capability-stack-{index}-bare") }),
            )
            .await;
        if required.is_empty() {
            assert_eq!(
                status, 201,
                "case {case:?}: a template requiring nothing must deploy without \
                 capabilities, got {status} with {answer}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 without the required capabilities, got \
                 {status} with {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "BadRequestException",
                "case {case:?}: expected the modelled BadRequestException, got {answer}"
            );
            let message = text(&answer, "message");
            assert!(
                message.starts_with("Required capabilities ["),
                "case {case:?}: expected the service's capability refusal, got {message:?}"
            );
            for capability in required {
                assert!(
                    message.contains(capability),
                    "case {case:?}: the refusal must name {capability}, got {message:?}"
                );
            }
        }

        // Acknowledging exactly what the template requires: always accepted.
        let (status, answer) = deploy
            .create_change_set(
                &application_id,
                json!({
                    "stackName": format!("capability-stack-{index}-ack"),
                    "capabilities": required
                }),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: acknowledging {required:?} must deploy, got {status} with {answer}"
        );

        // What CloudFormation is asked to acknowledge is this service's own IAM
        // acknowledgement plus the subset of the request's that CloudFormation's own
        // `Capability` enum declares (CAPABILITY_IAM | CAPABILITY_NAMED_IAM |
        // CAPABILITY_AUTO_EXPAND): CAPABILITY_RESOURCE_POLICY is this service's own.
        // See [`forwarded_capabilities`].
        let change_set_id = text(&answer, "changeSetId");
        let (status, xml) = deploy
            .cloudformation("DescribeChangeSet", &[("ChangeSetName", &change_set_id)])
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the change set must be describable, got {status} {xml}"
        );
        let forwarded = forwarded_capabilities(required);
        assert_eq!(
            xml_list(&xml, "Capabilities"),
            forwarded,
            "case {case:?}: CloudFormation must be asked to acknowledge {forwarded:?}, \
             got {xml}"
        );
    }

    // CAPABILITY_NAMED_IAM is the stronger acknowledgement of the same resources, so it
    // covers a CAPABILITY_IAM requirement; the converse does not hold.
    let unnamed = deploy
        .given_application(
            "capability-coverage-unnamed",
            &[("1.0.0", sam_template(json!({ "Role": { "Type": "AWS::IAM::Role" } })))],
        )
        .await;
    let (status, answer) = deploy
        .create_change_set(
            &unnamed,
            json!({
                "stackName": "capability-coverage-stack-a",
                "capabilities": ["CAPABILITY_NAMED_IAM"]
            }),
        )
        .await;
    assert_eq!(
        status, 201,
        "case \"named covers unnamed\": CAPABILITY_NAMED_IAM must cover a CAPABILITY_IAM \
         requirement, got {status} with {answer}"
    );
    let named = deploy
        .given_application(
            "capability-coverage-named",
            &[(
                "1.0.0",
                sam_template(json!({
                    "Role": {
                        "Type": "AWS::IAM::Role",
                        "Properties": { "RoleName": "pinned" }
                    }
                })),
            )],
        )
        .await;
    let (status, answer) = deploy
        .create_change_set(
            &named,
            json!({
                "stackName": "capability-coverage-stack-b",
                "capabilities": ["CAPABILITY_IAM"]
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"unnamed does not cover named\": expected 400, got {status} with {answer}"
    );
    assert!(
        text(&answer, "message").contains("CAPABILITY_NAMED_IAM"),
        "case \"unnamed does not cover named\": the refusal must name the capability \
         still missing, got {answer}"
    );
}

/// RULE 2 over the OTHER form a packaged SAM template is written in: the requirement is a
/// function of the resources the template declares, not of whether the publisher wrote it
/// as JSON or as YAML.
///
/// The same resources as the `CAPABILITY_NAMED_IAM` row of the table above, published as
/// YAML, declare the same requirement — so a deployment that acknowledges nothing is
/// refused by THIS service, with its own capability refusal, and one that acknowledges the
/// derived capability deploys and is planned by CloudFormation.
#[tokio::test]
async fn a_yaml_template_declares_the_same_requirement_as_its_json_spelling() {
    let deploy = Deploy::new();
    // The same application as the CAPABILITY_NAMED_IAM row above, in YAML.
    let yaml = "Transform: AWS::Serverless-2016-10-31\n\
                Resources:\n  \
                  Role:\n    \
                    Type: AWS::IAM::Role\n    \
                    Properties:\n      \
                      RoleName: my-own-role\n";

    let (status, created) = deploy
        .repo("POST", "/applications", Some(minimal("yaml-app")))
        .await;
    assert_eq!(status, 201, "creating the application: got {status} {created}");
    let application_id = text(&created, "applicationId");
    let (status, published) = deploy
        .repo(
            "PUT",
            &format!(
                "/applications/{}/versions/1.0.0",
                path_label(&application_id)
            ),
            Some(json!({ "templateBody": yaml })),
        )
        .await;
    assert_eq!(
        status, 201,
        "publishing the YAML template: got {status} {published}"
    );
    assert_eq!(
        published["requiredCapabilities"],
        json!(["CAPABILITY_NAMED_IAM"]),
        "the YAML spelling of a named IAM resource declares the same requirement its JSON \
         spelling does, got {published}"
    );

    // Derived here, so the refusal of an unacknowledged deployment is THIS service's.
    let (status, answer) = deploy
        .create_change_set(&application_id, json!({ "stackName": "yaml-stack" }))
        .await;
    assert_eq!(
        status, 400,
        "a requirement this service derived is this service's to refuse, got \
         {status} with {answer}"
    );
    assert_eq!(
        error_code(&answer),
        "BadRequestException",
        "expected the modelled BadRequestException, got {answer}"
    );
    let message = text(&answer, "message");
    assert!(
        message.starts_with("Required capabilities [") && message.contains("CAPABILITY_NAMED_IAM"),
        "the refusal must be this service's and name the capability required, got {message:?}"
    );

    // Acknowledged, the same deployment is made, and what is forwarded is the same set the
    // JSON spelling of this template forwards.
    let (status, answer) = deploy
        .create_change_set(
            &application_id,
            json!({
                "stackName": "yaml-stack",
                "capabilities": ["CAPABILITY_NAMED_IAM"]
            }),
        )
        .await;
    assert_eq!(
        status, 201,
        "acknowledging what the template requires must deploy, got {status} with {answer}"
    );
    let change_set_id = text(&answer, "changeSetId");
    let (status, xml) = deploy
        .cloudformation("DescribeChangeSet", &[("ChangeSetName", &change_set_id)])
        .await;
    assert_eq!(
        status, 200,
        "the change set must be describable, got {status} {xml}"
    );
    assert_eq!(
        xml_list(&xml, "Capabilities"),
        forwarded_capabilities(&["CAPABILITY_NAMED_IAM"]),
        "the request's acknowledgement is forwarded beside this service's own, got {xml}"
    );
    assert!(
        xml_texts(&xml, "LogicalResourceId").contains(&"Role".to_string()),
        "CloudFormation reads the YAML and plans its resources, got {xml}"
    );
}

/// One row of the refusal table: the case name, the application the request names, the
/// request's extra members, and the status, wire error code, body `errorCode` and message
/// fragment the refusal must carry.
type RefusalCase<'a> = (&'a str, &'a str, Value, u16, &'a str, &'a str, &'a str);

/// RULE 3: a request this service cannot resolve to a deployable version is refused —
/// with HTTP 404 `NotFoundException` for a resource the catalogue does not hold, and the
/// modelled 400 `BadRequestException` for a request outside the contract — and nothing is
/// created in CloudFormation either way.
///
/// The rows span the ways a request fails to name a deployable version: an application
/// with no versions at all, a version the application does not carry, an application the
/// catalogue does not hold, a `SemanticVersion` that is not a semantic version, a
/// `TemplateId` naming no template, and a `Capabilities` value outside the documented set.
#[tokio::test]
async fn a_change_set_is_refused_when_no_published_version_is_named() {
    let deploy = Deploy::new();
    let versionless = deploy.given_application("refusal-versionless", &[]).await;
    let published = deploy
        .given_application("refusal-published", &[("1.0.0", one_queue())])
        .await;
    let absent = format!("arn:aws:serverlessrepo:{REGION}:{ACCOUNT}:applications/not-published");

    let cases: Vec<RefusalCase> = vec![
        (
            "an application with no versions",
            &versionless,
            json!({}),
            400,
            "BadRequestException",
            "InvalidRequestException",
            "application with no version",
        ),
        (
            "a version the application does not carry",
            &published,
            json!({ "semanticVersion": "9.9.9" }),
            404,
            "NotFoundException",
            "VersionNotFoundException",
            "does not have a version 9.9.9",
        ),
        (
            "an application the catalogue does not hold",
            &absent,
            json!({}),
            404,
            "NotFoundException",
            "ApplicationNotFoundException",
            "could not be found",
        ),
        (
            "an application the catalogue does not hold, named with a TemplateId",
            &absent,
            json!({ "templateId": "0ba3f2a9-1e42-4b2a-9f2a-1c0d9e7b5a31" }),
            404,
            "NotFoundException",
            "ApplicationNotFoundException",
            "could not be found",
        ),
        (
            "a SemanticVersion that is not one",
            &published,
            json!({ "semanticVersion": "latest" }),
            400,
            "BadRequestException",
            "InvalidRequestException",
            "semantic version",
        ),
        (
            "a TemplateId naming no template of this application",
            &published,
            json!({ "templateId": "0ba3f2a9-1e42-4b2a-9f2a-1c0d9e7b5a31" }),
            404,
            "NotFoundException",
            "TemplateNotFoundException",
            "Template with id 0ba3f2a9-1e42-4b2a-9f2a-1c0d9e7b5a31 can not be found.",
        ),
        (
            "a Capabilities value outside the documented set",
            &published,
            json!({ "capabilities": ["CAPABILITY_EVERYTHING"] }),
            400,
            "BadRequestException",
            "InvalidRequestException",
            "Capabilities must each be one of",
        ),
    ];

    for (index, (case, application_id, extra, expected_status, wire_code, error_code_member, fragment)) in
        cases.iter().enumerate()
    {
        let stack_name = format!("refusal-stack-{index}");
        let mut body = json!({ "stackName": stack_name });
        for (member, value) in extra.as_object().expect("an object of extra members") {
            body[member] = value.clone();
        }

        let (status, answer) = deploy.create_change_set(application_id, body).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            *wire_code,
            "case {case:?}: expected the modelled {wire_code}, got {answer}"
        );
        assert_eq!(
            answer.get("errorCode"),
            Some(&json!(error_code_member)),
            "case {case:?}: the body's errorCode names the condition, got {answer}"
        );
        assert!(
            text(&answer, "message").contains(fragment),
            "case {case:?}: the refusal must name {fragment:?}, got {answer}"
        );

        // Nothing was created for it: CloudFormation holds no stack of that name.
        let (status, xml) = deploy
            .cloudformation("DescribeStacks", &[("StackName", &deployed_stack(&stack_name))])
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused deployment must leave no stack behind, got \
             {status} {xml}"
        );
        assert!(
            xml.contains("does not exist"),
            "case {case:?}: expected CloudFormation to report no such stack, got {xml}"
        );
    }
}

/// One row of the pass-through table: the case name, the request's extra members, the
/// change set name it must carry (`None` for one this service mints), its description,
/// and the (key, value) parameters and tags it must carry.
type PassThroughCase<'a> = (
    &'a str,
    Value,
    Option<&'a str>,
    &'a str,
    Vec<(&'a str, &'a str)>,
    Vec<(&'a str, &'a str)>,
);

/// The members that "correspond to the parameter of the same name for the AWS
/// CloudFormation CreateChangeSet API" reach CloudFormation: the change set a client then
/// describes carries the name, description, parameter values, tags and notification ARNs
/// the deployment was asked for.
///
/// The rows differ in which of those the request carries, and in how many values each
/// one holds — including a request that carries none of them, which is what makes the
/// minted change set name observable.
#[tokio::test]
async fn the_change_set_carries_what_the_deployment_asked_for() {
    let deploy = Deploy::new();
    // The queue's name comes from a template PARAMETER the application declares, so the
    // parameter overrides below have something to override — a `Ref` to a name the template
    // does not declare is a template CloudFormation refuses.
    let template = json!({
        "Transform": SAM_TRANSFORM,
        "Parameters": { "QueueName": { "Type": "String", "Default": "passthrough" } },
        "Resources": {
            "Queue": {
                "Type": "AWS::SQS::Queue",
                "Properties": { "QueueName": { "Ref": "QueueName" } }
            }
        }
    });
    let application_id = deploy
        .given_application("passthrough-app", &[("1.0.0", template)])
        .await;

    // (case, the request's extra members, expected change set name, expected
    // description, expected (key, value) parameters, expected (key, value) tags)
    //
    // A request that overrides no parameter still deploys with the one the template
    // declares, at the `Default` it declares for it: that is the value in force, which is
    // what the change set CloudFormation creates reports for it.
    let cases: Vec<PassThroughCase> = vec![
        (
            "nothing but the stack",
            json!({}),
            None,
            "",
            vec![("QueueName", "passthrough")],
            vec![],
        ),
        (
            "a named change set with a description",
            json!({ "changeSetName": "my-change-set", "description": "deploy the queue" }),
            Some("my-change-set"),
            "deploy the queue",
            vec![("QueueName", "passthrough")],
            vec![],
        ),
        (
            "one parameter override and one tag",
            json!({
                "changeSetName": "parameterised",
                "parameterOverrides": [{ "name": "QueueName", "value": "orders" }],
                "tags": [{ "key": "env", "value": "test" }]
            }),
            Some("parameterised"),
            "",
            vec![("QueueName", "orders")],
            vec![("env", "test")],
        ),
        (
            "two tags and a notification topic",
            json!({
                "changeSetName": "tagged",
                "parameterOverrides": [{ "name": "QueueName", "value": "payments" }],
                "tags": [
                    { "key": "env", "value": "prod" },
                    { "key": "owner", "value": "payments-team" }
                ],
                "notificationArns": [
                    format!("arn:aws:sns:{REGION}:{ACCOUNT}:stack-events")
                ]
            }),
            Some("tagged"),
            "",
            vec![("QueueName", "payments")],
            vec![("env", "prod"), ("owner", "payments-team")],
        ),
    ];

    for (index, (case, extra, change_set_name, description, parameters, tags)) in
        cases.iter().enumerate()
    {
        let mut body = json!({ "stackName": format!("passthrough-stack-{index}") });
        for (member, value) in extra.as_object().expect("an object of extra members") {
            body[member] = value.clone();
        }
        let (status, answer) = deploy.create_change_set(&application_id, body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {answer}"
        );
        let change_set_id = text(&answer, "changeSetId");
        assert_change_set_arn(case, &change_set_id);

        let (status, xml) = deploy
            .cloudformation("DescribeChangeSet", &[("ChangeSetName", &change_set_id)])
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the change set must be describable, got {status} {xml}"
        );
        match change_set_name {
            Some(expected) => assert_eq!(
                xml_text(&xml, "ChangeSetName"),
                *expected,
                "case {case:?}: the change set must carry the requested name"
            ),
            // A request that names none is served the name this service minted, which is
            // the one the ARN carries.
            None => {
                let minted = xml_text(&xml, "ChangeSetName");
                assert!(
                    change_set_id.contains(&format!("changeSet/{minted}/")),
                    "case {case:?}: the minted name {minted:?} must be the one in the ARN \
                     {change_set_id:?}"
                );
            }
        }
        assert_eq!(
            xml_text(&xml, "Description"),
            *description,
            "case {case:?}: the change set must carry the requested description"
        );
        let keys = xml_texts(&xml, "ParameterKey");
        let values = xml_texts(&xml, "ParameterValue");
        let reported: Vec<(String, String)> = keys.into_iter().zip(values).collect();
        let expected: Vec<(String, String)> = parameters
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: the change set must carry the requested parameter values"
        );
        let tag_keys = xml_texts(&xml, "Key");
        let tag_values = xml_texts(&xml, "Value");
        let reported_tags: Vec<(String, String)> = tag_keys.into_iter().zip(tag_values).collect();
        let expected_tags: Vec<(String, String)> = provenance_tags(&application_id, "1.0.0")
            .into_iter()
            .chain(
                tags.iter()
                    .map(|(key, value)| (key.to_string(), value.to_string())),
            )
            .collect();
        assert_eq!(
            reported_tags, expected_tags,
            "case {case:?}: the change set must carry the deployment's provenance tags \
             and then the requested ones, got {reported_tags:?}"
        );
    }
}

/// RULE 1, continued: the stack a deployment targets is one stack across deployments —
/// the first one brings it into existence, and a later deployment of another version
/// UPDATES that same stack rather than failing as a second create of it.
///
/// This is the recorded units' own path: create the change set, execute it in
/// CloudFormation, then deploy the next version onto the same stack. A deployment that
/// asked CloudFormation for a `CREATE` change set the second time would be refused with
/// `AlreadyExistsException`, so the 201 and the unchanged stack id are what pin the rule.
#[tokio::test]
async fn a_later_deployment_updates_the_stack_the_first_one_created() {
    let deploy = Deploy::new();
    let first_template = one_queue();
    let second_template = sam_template(json!({
        "Queue": { "Type": "AWS::SQS::Queue" },
        "Second": { "Type": "AWS::SQS::Queue" }
    }));
    let application_id = deploy
        .given_application(
            "redeploy-app",
            &[
                ("1.0.0", first_template.clone()),
                ("2.0.0", second_template.clone()),
            ],
        )
        .await;
    let stack_name = "redeploy-stack";
    // The stack CloudFormation holds it under, which is what a client then reads.
    let deployed = deployed_stack(stack_name);

    let (status, answer) = deploy
        .create_change_set(
            &application_id,
            json!({ "stackName": stack_name, "semanticVersion": "1.0.0" }),
        )
        .await;
    assert_eq!(
        status, 201,
        "the first deployment must be accepted, got {status} with {answer}"
    );
    let stack_id = text(&answer, "stackId");
    let first_change_set = text(&answer, "changeSetId");

    // Execute it, as a client deploying an application does.
    deploy.settle_cloudformation();
    let (status, xml) = deploy
        .cloudformation("ExecuteChangeSet", &[("ChangeSetName", &first_change_set)])
        .await;
    assert_eq!(
        status, 200,
        "the change set must be executable, got {status} {xml}"
    );
    deploy.settle_cloudformation();
    let (status, xml) = deploy
        .cloudformation("DescribeStacks", &[("StackName", &deployed)])
        .await;
    assert_eq!(status, 200, "the stack must resolve, got {status} {xml}");
    assert_eq!(
        xml_text(&xml, "StackId"),
        stack_id,
        "the executed stack must be the one the deployment answered with, got {xml}"
    );
    assert_eq!(
        xml_text(&xml, "StackStatus"),
        "CREATE_COMPLETE",
        "the executed stack must have completed, got {xml}"
    );
    let (status, xml) = deploy
        .cloudformation(
            "GetTemplate",
            &[
                ("StackName", &deployed),
                ("TemplateStage", TEMPLATE_STAGE_ORIGINAL),
            ],
        )
        .await;
    assert_eq!(status, 200, "the stack's template must read, got {status} {xml}");
    assert_eq!(
        xml_text(&xml, "TemplateBody"),
        first_template.to_string(),
        "the deployed stack must carry the published template of version 1.0.0"
    );
    // And the stage CloudFormation serves by DEFAULT is that same template with the SAM
    // transform applied: the deployed document, which no longer declares the transform.
    let (status, xml) = deploy
        .cloudformation("GetTemplate", &[("StackName", &deployed)])
        .await;
    assert_eq!(
        status, 200,
        "the stack's processed template must read, got {status} {xml}"
    );
    let processed = xml_text(&xml, "TemplateBody");
    assert!(
        !processed.contains(SAM_TRANSFORM),
        "the processed stage of a deployed application declares no transform, got {processed}"
    );
    assert!(
        processed.contains("\"Queue\""),
        "the processed stage keeps the resources the published template declares, got \
         {processed}"
    );

    // The second deployment, onto the same stack.
    let (status, answer) = deploy
        .create_change_set(
            &application_id,
            json!({ "stackName": stack_name, "semanticVersion": "2.0.0" }),
        )
        .await;
    assert_eq!(
        status, 201,
        "a deployment onto the stack the first one created must be accepted, got \
         {status} with {answer}"
    );
    assert_eq!(
        text(&answer, "stackId"),
        stack_id,
        "the second deployment must target the same stack, got {answer}"
    );
    let second_change_set = text(&answer, "changeSetId");
    assert_ne!(
        second_change_set, first_change_set,
        "each deployment is its own change set, got {answer}"
    );
    let (status, xml) = deploy
        .cloudformation(
            "GetTemplate",
            &[
                ("ChangeSetName", &second_change_set),
                ("TemplateStage", TEMPLATE_STAGE_ORIGINAL),
            ],
        )
        .await;
    assert_eq!(
        status, 200,
        "the second change set's template must read, got {status} {xml}"
    );
    assert_eq!(
        xml_text(&xml, "TemplateBody"),
        second_template.to_string(),
        "the second change set must carry the published template of version 2.0.0"
    );
    let (status, xml) = deploy
        .cloudformation("DescribeChangeSet", &[("ChangeSetName", &second_change_set)])
        .await;
    assert_eq!(
        status, 200,
        "the second change set must be describable, got {status} {xml}"
    );
    assert!(
        xml_texts(&xml, "LogicalResourceId").contains(&"Second".to_string()),
        "the second change set must plan the resource version 2.0.0 adds, got {xml}"
    );
}

/// RULE: a `TemplateId` names a template this service PREPARED for the application
/// (`CreateCloudFormationTemplate`), and the deployment is of the version that template
/// was prepared from — for as long as that template is `ACTIVE`, which it is from the
/// create that minted it until its own `ExpirationTime` passes.
///
/// The rows prepare a template from each of two published versions, so what the request
/// names and what gets deployed are independent of which version is the latest.
#[tokio::test]
async fn a_change_set_deploys_the_version_a_prepared_template_names() {
    let deploy = Deploy::new();
    let older = one_queue();
    let latest = sam_template(json!({ "Topic": { "Type": "AWS::SNS::Topic" } }));
    let application_id = deploy
        .given_application(
            "template-deploy-app",
            &[("1.0.0", older.clone()), ("2.0.0", latest.clone())],
        )
        .await;

    // (case, the version the template is prepared from, that version's template, another
    // version the request must not be allowed to name beside the template)
    let cases: [(&str, &str, &Value, &str); 2] = [
        ("the older version", "1.0.0", &older, "2.0.0"),
        ("the latest version", "2.0.0", &latest, "1.0.0"),
    ];

    for (index, (case, prepared_from, template, other_version)) in cases.iter().enumerate() {
        let (status, prepared) = deploy.create_template(&application_id, prepared_from).await;
        assert_eq!(
            status, 201,
            "case {case:?}: preparing a template must succeed, got {status} with {prepared}"
        );
        let template_id = text(&prepared, "templateId");
        assert_eq!(
            text(&prepared, "semanticVersion"),
            *prepared_from,
            "case {case:?}: the prepared template must name the version it was prepared \
             from, got {prepared}"
        );
        let stack_name = format!("template-stack-{index}");

        // The prepared template is deployable from the create that minted it: the change
        // set deploys the version it was prepared from, whatever the application's latest
        // version is.
        let (status, answer) = deploy
            .create_change_set(
                &application_id,
                json!({ "stackName": stack_name, "templateId": template_id }),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: an ACTIVE template must deploy, got {status} with {answer}"
        );
        assert_eq!(
            text(&answer, "semanticVersion"),
            *prepared_from,
            "case {case:?}: the deployment must be of the prepared version, got {answer}"
        );
        let change_set_id = text(&answer, "changeSetId");
        let (status, xml) = deploy
            .cloudformation(
                "GetTemplate",
                &[
                    ("ChangeSetName", &change_set_id),
                    ("TemplateStage", TEMPLATE_STAGE_ORIGINAL),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the change set's template must read, got {status} {xml}"
        );
        assert_eq!(
            xml_text(&xml, "TemplateBody"),
            template.to_string(),
            "case {case:?}: the change set must carry the prepared version's template"
        );

        // A `SemanticVersion` that is not the one the template was prepared from names a
        // second deployment, so the request is refused rather than one of the two chosen.
        let (status, answer) = deploy
            .create_change_set(
                &application_id,
                json!({
                    "stackName": format!("template-mismatch-{index}"),
                    "templateId": template_id,
                    "semanticVersion": other_version
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a TemplateId and a different SemanticVersion must be refused, \
             got {status} with {answer}"
        );
        assert!(
            text(&answer, "message").contains("was prepared from version"),
            "case {case:?}: the refusal must say which version the template carries, got \
             {answer}"
        );

        // The one status that refuses a deploy is the end of the template's own life: once
        // the clock has passed its `ExpirationTime` the template is gone, and the refusal
        // names that status.
        deploy.expire_templates();
        let (status, answer) = deploy
            .create_change_set(
                &application_id,
                json!({
                    "stackName": format!("template-expired-{index}"),
                    "templateId": template_id
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an EXPIRED template must not deploy, got {status} with {answer}"
        );
        assert!(
            text(&answer, "message").contains("EXPIRED"),
            "case {case:?}: the refusal must name the status that refused it, got {answer}"
        );
    }
}

/// One row of the provenance table: the case name, the `SemanticVersion` the request names
/// (`None` for the application's latest), the version therefore deployed, what the request
/// acknowledges, and the tags it asks for.
type ProvenanceCase<'a> = (
    &'a str,
    Option<&'a str>,
    &'a str,
    Vec<&'a str>,
    Vec<(&'a str, &'a str)>,
);

/// RULE 4: a deployment records WHICH application and WHICH version of it the deployment
/// was made from, as the `serverlessrepo:applicationId` / `serverlessrepo:semanticVersion`
/// tags on the change set and so on the stack it is executed onto, and it acknowledges
/// `CAPABILITY_IAM` to CloudFormation for every deployment it makes, whatever the request
/// acknowledged to this service.
///
/// The rows differ in which version is deployed (the latest, or an older one the request
/// names), in what the request acknowledges (nothing, the same acknowledgement this
/// service always makes, a stronger one, one CloudFormation does not model) and in the
/// tags it asks for (none, one, two, and one that names a provenance key — which the
/// deployment's own value is not displaced by).
#[tokio::test]
async fn a_deployment_records_the_application_and_version_it_came_from() {
    let deploy = Deploy::new();
    let application_id = deploy
        .given_application(
            "provenance-app",
            &[
                ("1.0.0", one_queue()),
                (
                    "2.0.0",
                    sam_template(json!({
                        "Queue": { "Type": "AWS::SQS::Queue" },
                        "Second": { "Type": "AWS::SQS::Queue" }
                    })),
                ),
            ],
        )
        .await;

    let cases: Vec<ProvenanceCase> = vec![
        ("the latest version, nothing else asked for", None, "2.0.0", vec![], vec![]),
        (
            "an older version the request names",
            Some("1.0.0"),
            "1.0.0",
            vec![],
            vec![],
        ),
        (
            "one tag of the caller's own",
            None,
            "2.0.0",
            vec![],
            vec![("env", "test")],
        ),
        (
            "the same acknowledgement the service always makes",
            Some("1.0.0"),
            "1.0.0",
            vec!["CAPABILITY_IAM"],
            vec![("team", "platform")],
        ),
        (
            "a stronger acknowledgement and one CloudFormation does not model",
            None,
            "2.0.0",
            vec!["CAPABILITY_NAMED_IAM", "CAPABILITY_RESOURCE_POLICY"],
            vec![("env", "prod"), ("owner", "payments-team")],
        ),
        (
            "a tag naming a provenance key",
            Some("1.0.0"),
            "1.0.0",
            vec![],
            vec![
                ("serverlessrepo:applicationId", "somebody-elses-application"),
                ("env", "staging"),
            ],
        ),
    ];

    for (index, (case, requested_version, deployed_version, acknowledged, tags)) in
        cases.iter().enumerate()
    {
        let stack_name = format!("provenance-stack-{index}");
        let mut body = json!({ "stackName": stack_name });
        if let Some(requested_version) = requested_version {
            body["semanticVersion"] = json!(requested_version);
        }
        if !acknowledged.is_empty() {
            body["capabilities"] = json!(acknowledged);
        }
        if !tags.is_empty() {
            body["tags"] = Value::Array(
                tags.iter()
                    .map(|(key, value)| json!({ "key": key, "value": value }))
                    .collect(),
            );
        }
        let (status, answer) = deploy.create_change_set(&application_id, body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {answer}"
        );
        assert_eq!(
            text(&answer, "semanticVersion"),
            *deployed_version,
            "case {case:?}: expected the deployment to be of {deployed_version}, got {answer}"
        );

        // The provenance tags first, then the caller's own — minus any that names a
        // provenance key, which the deployment's own value holds.
        let expected_tags: Vec<(String, String)> =
            provenance_tags(&application_id, deployed_version)
                .into_iter()
                .chain(
                    tags.iter()
                        .filter(|(key, _)| !key.starts_with("serverlessrepo:"))
                        .map(|(key, value)| (key.to_string(), value.to_string())),
                )
                .collect();
        let expected_capabilities = forwarded_capabilities(acknowledged);

        let change_set_id = text(&answer, "changeSetId");
        let (status, xml) = deploy
            .cloudformation("DescribeChangeSet", &[("ChangeSetName", &change_set_id)])
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the change set must be describable, got {status} {xml}"
        );
        let reported_tags = xml_tags(&xml);
        assert_eq!(
            reported_tags, expected_tags,
            "case {case:?}: expected the change set's tags to be {expected_tags:?}, got \
             {reported_tags:?}"
        );
        let reported_capabilities = xml_list(&xml, "Capabilities");
        assert_eq!(
            reported_capabilities, expected_capabilities,
            "case {case:?}: expected the change set to acknowledge {expected_capabilities:?}, \
             got {reported_capabilities:?}"
        );

        // The stack the deployment lands on carries the same record, which is what a
        // client reads back from CloudFormation after deploying the application.
        deploy.settle_cloudformation();
        let (status, xml) = deploy
            .cloudformation("ExecuteChangeSet", &[("ChangeSetName", &change_set_id)])
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the change set must be executable, got {status} {xml}"
        );
        deploy.settle_cloudformation();
        let (status, xml) = deploy
            .cloudformation("DescribeStacks", &[("StackName", &deployed_stack(&stack_name))])
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the deployed stack must resolve, got {status} {xml}"
        );
        let stack_status = xml_text(&xml, "StackStatus");
        assert_eq!(
            stack_status, "CREATE_COMPLETE",
            "case {case:?}: expected the executed stack to have completed, got \
             {stack_status:?}"
        );
        let stack_tags = xml_tags(&xml);
        assert_eq!(
            stack_tags, expected_tags,
            "case {case:?}: expected the stack's tags to be {expected_tags:?}, got \
             {stack_tags:?}"
        );
        let stack_capabilities = xml_list(&xml, "Capabilities");
        assert_eq!(
            stack_capabilities, expected_capabilities,
            "case {case:?}: expected the stack to acknowledge {expected_capabilities:?}, got \
             {stack_capabilities:?}"
        );
    }
}

/// The versions of ONE application RULE 5 is asserted over: the same application published
/// three times, differing only in what raises the acknowledgement bar — an IAM resource
/// whose name the template leaves to CloudFormation, the same resource with its name pinned
/// by the template, and a template declaring no IAM-affecting resource at all.
///
/// The resource types and the naming rule are the `Capabilities` member's own documentation
/// (`smithy_query(query="doc", shape="CreateCloudFormationChangeSetRequest",
/// member="Capabilities")`): the four IAM types require `CAPABILITY_IAM`, and a custom
/// (pinned) name on one of them raises the requirement to `CAPABILITY_NAMED_IAM`.
fn gated_versions() -> Vec<(&'static str, Value, Vec<&'static str>)> {
    vec![
        (
            "1.0.0",
            sam_template(json!({ "Role": { "Type": "AWS::IAM::Role" } })),
            vec!["CAPABILITY_IAM"],
        ),
        (
            "1.1.0",
            sam_template(json!({
                "Role": {
                    "Type": "AWS::IAM::Role",
                    "Properties": { "RoleName": "pinned-by-the-template" }
                }
            })),
            vec!["CAPABILITY_NAMED_IAM"],
        ),
        ("1.2.0", one_queue(), vec![]),
    ]
}

/// One accepted row: the case, the version the request names, and what it acknowledges.
type AcceptedDeployment<'a> = (&'a str, &'a str, Vec<&'a str>);

/// One refused row: the case, the version the request names, what it acknowledges, the
/// HTTP status and wire error code it is refused with, and the sentence the refusal carries.
type RefusedDeployment<'a> = (&'a str, &'a str, Vec<&'a str>, u16, &'a str, String);

/// RULE 5: the acknowledgement gate has exactly two outcomes. An accepted deployment
/// answers 201 and leaves the stack `serverlessrepo-<StackName>` in `REVIEW_IN_PROGRESS` —
/// the shell a CREATE change set brings into existence, with the change set itself
/// `CREATE_COMPLETE` and `AVAILABLE`, so a client can execute it — and a refused one
/// leaves no stack of that name at all, not even that shell.
///
/// Both halves are asserted over versions of ONE application whose derived requirement
/// differs (`gated_versions`), read back AFTER every publish, so what each version requires
/// is a function of THAT version's own template and a later publish rewrites no earlier
/// version's list. The accepted rows differ in which version is deployed and in how the
/// acknowledgement satisfies the requirement (nothing required and nothing acknowledged,
/// exactly the requirement, the stronger `CAPABILITY_NAMED_IAM` covering a
/// `CAPABILITY_IAM` requirement, and more acknowledged than required); the refused rows
/// differ in why they fail — a version the application never published, no acknowledgement
/// at all, and the weaker acknowledgement against the named requirement.
#[tokio::test]
async fn the_acknowledgement_gate_either_reviews_the_stack_or_creates_nothing() {
    let deploy = Deploy::new();
    let versions = gated_versions();
    let published: Vec<(&str, Value)> = versions
        .iter()
        .map(|(semantic_version, template, _)| (*semantic_version, template.clone()))
        .collect();
    let application_id = deploy.given_application("deploy-gate-app", &published).await;

    // What each version requires is derived from its own template, and the later publishes
    // have already happened by the time each one is read back.
    for (semantic_version, _template, required) in &versions {
        let version = deploy.get_application(&application_id, semantic_version).await;
        let reported = version["version"]["requiredCapabilities"].clone();
        assert_eq!(
            reported,
            json!(required),
            "case {semantic_version:?}: expected requiredCapabilities {required:?}, got \
             {reported}"
        );
    }

    let accepted: Vec<AcceptedDeployment> = vec![
        (
            "a template requiring nothing, acknowledging nothing",
            "1.2.0",
            vec![],
        ),
        (
            "exactly the requirement",
            "1.0.0",
            vec!["CAPABILITY_IAM"],
        ),
        (
            "exactly the requirement, with the name pinned",
            "1.1.0",
            vec!["CAPABILITY_NAMED_IAM"],
        ),
        (
            "the stronger acknowledgement covers the weaker requirement",
            "1.0.0",
            vec!["CAPABILITY_NAMED_IAM"],
        ),
        (
            "more acknowledged than the template requires",
            "1.2.0",
            vec!["CAPABILITY_IAM", "CAPABILITY_AUTO_EXPAND"],
        ),
    ];

    for (index, (case, semantic_version, acknowledged)) in accepted.iter().enumerate() {
        let stack_name = format!("deploy-gate-stack-{index}");
        let (status, answer) = deploy
            .create_change_set(
                &application_id,
                json!({
                    "stackName": stack_name,
                    "semanticVersion": semantic_version,
                    "capabilities": acknowledged
                }),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {answer}"
        );
        assert_eq!(
            text(&answer, "semanticVersion"),
            *semantic_version,
            "case {case:?}: expected the deployment to be of {semantic_version}, got {answer}"
        );
        let stack_id = text(&answer, "stackId");
        let change_set_id = text(&answer, "changeSetId");

        // The stack the deployment created, and nothing has executed it: the change set
        // brought a stack with no template and no resources into existence.
        let (status, xml) = deploy
            .cloudformation(
                "DescribeStacks",
                &[("StackName", &deployed_stack(&stack_name))],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the deployment's stack must resolve, got {status} {xml}"
        );
        assert_eq!(
            xml_text(&xml, "StackId"),
            stack_id,
            "case {case:?}: the stack must be the one the deployment answered with, got {xml}"
        );
        let stack_status = xml_text(&xml, "StackStatus");
        assert_eq!(
            stack_status, "REVIEW_IN_PROGRESS",
            "case {case:?}: an unexecuted deployment's stack must be under review, got \
             {stack_status:?}"
        );

        // And the change set is executable: the plan settled, so a client can execute it.
        deploy.settle_cloudformation();
        let (status, xml) = deploy
            .cloudformation("DescribeChangeSet", &[("ChangeSetName", &change_set_id)])
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the change set must be describable, got {status} {xml}"
        );
        let change_set_status = xml_text(&xml, "Status");
        assert_eq!(
            change_set_status, "CREATE_COMPLETE",
            "case {case:?}: the created change set must have a complete plan, got \
             {change_set_status:?}"
        );
        let execution_status = xml_text(&xml, "ExecutionStatus");
        assert_eq!(
            execution_status, "AVAILABLE",
            "case {case:?}: the created change set must be executable, got \
             {execution_status:?}"
        );
    }

    let refused: Vec<RefusedDeployment> = vec![
        (
            "a version the application never published",
            "9.9.9",
            vec!["CAPABILITY_NAMED_IAM"],
            404,
            "NotFoundException",
            format!("Application {application_id} does not have a version 9.9.9"),
        ),
        (
            "no acknowledgement at all",
            "1.0.0",
            vec![],
            400,
            "BadRequestException",
            "Required capabilities [CAPABILITY_IAM] were not provided.".to_string(),
        ),
        (
            "the weaker acknowledgement against a pinned name",
            "1.1.0",
            vec!["CAPABILITY_IAM"],
            400,
            "BadRequestException",
            "Required capabilities [CAPABILITY_NAMED_IAM] were not provided.".to_string(),
        ),
    ];

    for (index, (case, semantic_version, acknowledged, expected_status, wire_code, message)) in
        refused.iter().enumerate()
    {
        let stack_name = format!("deploy-gate-refused-{index}");
        let (status, answer) = deploy
            .create_change_set(
                &application_id,
                json!({
                    "stackName": stack_name,
                    "semanticVersion": semantic_version,
                    "capabilities": acknowledged
                }),
            )
            .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            *wire_code,
            "case {case:?}: expected the modelled {wire_code}, got {answer}"
        );
        assert_eq!(
            text(&answer, "message"),
            *message,
            "case {case:?}: expected the refusal {message:?}, got {answer}"
        );

        // Nothing was created for it — not even the REVIEW_IN_PROGRESS shell an accepted
        // deployment leaves behind.
        deploy.settle_cloudformation();
        let (status, xml) = deploy
            .cloudformation(
                "DescribeStacks",
                &[("StackName", &deployed_stack(&stack_name))],
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused deployment must leave no stack behind, got {status} {xml}"
        );
        assert!(
            xml.contains("does not exist"),
            "case {case:?}: expected CloudFormation to report no such stack, got {xml}"
        );
    }
}
