// SPDX-License-Identifier: CC0-1.0

//! Adopting Amazon SQS queues that ALREADY EXIST into a CloudFormation stack, the way
//! `cdk import` does: `CreateChangeSet(ChangeSetType=IMPORT, ResourcesToImport=…)` followed
//! by `ExecuteChangeSet`.
//!
//! The rule under test: **executing a change set that ADOPTS a resource records that resource
//! under the identifier it already exists under — that identifier is its `PhysicalResourceId`,
//! what `Ref` resolves to and the name every value the stack reports for it is read off — and
//! nothing is asked of the service that owns it, so no resource is created and the one adopted
//! is left exactly as it was, however many later deployments re-declare it.**
//!
//! Two crates have to agree for that to hold (CloudFormation plans and executes the import,
//! Amazon SQS owns the queue and is the only side that can say whether a queue was created or
//! left alone), so the suite lives here.
//!
//! The rows differ along what the rule generalises over: whether the template STATES the
//! resource's name or leaves it to CloudFormation, and which variant of the type the
//! declaration asks for (standard or FIFO, whose name carries the suffix Amazon SQS requires).
//! The refusal half is the same rule's converse: an identifier the owning service does not
//! hold is nothing to adopt, so the plan FAILS naming it and the stack is untouched.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery wire readers this suite shares with every other CloudFormation suite here.
use crate::query_wire::{elements, form_encode, outputs_of};

/// The text of the FIRST `<tag>` of an XML answer, read through the shared DECODING reader —
/// a template body and an error message are documents of their own, so they compare as the
/// service wrote them rather than as their escaping.
fn first_element(xml: &str, tag: &str) -> Option<String> {
    elements(xml, tag).into_iter().next()
}

/// The region every request here is signed for, and so the region the stack's queues belong
/// to.
const REGION: &str = "us-east-1";

/// The account id every ARN and queue url this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// Long enough for a stack's clock-owed transitions to come due.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(4_000);

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
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
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One CloudFormation call that must be accepted.
async fn cloudformation_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = cloudformation(router, action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with {body}"
    );
    body
}

/// Call one Amazon SQS operation over its awsJson1_0 wire.
async fn sqs(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One SQS call that must succeed.
async fn sqs_ok(router: &MultiServiceRouter, case: &str, operation: &str, request: Value) -> Value {
    let (status, parsed) = sqs(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// Every attribute SQS reports for the queue at `url`.
async fn reported_attributes(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
) -> BTreeMap<String, String> {
    let reported = sqs_ok(
        router,
        case,
        "GetQueueAttributes",
        json!({"QueueUrl": url, "AttributeNames": ["All"]}),
    )
    .await;
    reported
        .get("Attributes")
        .and_then(Value::as_object)
        .map(|attributes| {
            attributes
                .iter()
                .filter_map(|(name, value)| {
                    value.as_str().map(|value| (name.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every queue url Amazon SQS holds whose name starts with `prefix`.
async fn queues_named(router: &MultiServiceRouter, case: &str, prefix: &str) -> Vec<String> {
    let listed = sqs_ok(
        router,
        case,
        "ListQueues",
        json!({"QueueNamePrefix": prefix}),
    )
    .await;
    listed
        .get("QueueUrls")
        .and_then(Value::as_array)
        .map(|urls| {
            urls.iter()
                .filter_map(|url| url.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The logical id the imported queue is declared under, and so the resource every assertion
/// below addresses.
const ADOPTED: &str = "Adopted";

/// The logical id of the queue the stack created ITSELF before the import, which every row
/// carries so the import acts on a stack that already has resources of its own.
const KEPT: &str = "Kept";

/// The queue url `CreateQueue` mints for `name`, which is this type's physical resource id
/// ("Ref returns the queue URL").
fn queue_url(name: &str) -> String {
    format!("https://sqs.{REGION}.amazonaws.com/{ACCOUNT_ID}/{name}")
}

/// The ARN of the queue called `name`, which is that resource's `Arn` attribute.
fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{ACCOUNT_ID}:{name}")
}

/// The template the stack is deployed with BEFORE the import: the one queue it creates itself.
fn before_the_import() -> String {
    format!(r#"{{"Resources":{{"{KEPT}":{{"Type":"AWS::SQS::Queue","Properties":{{}}}}}}}}"#)
}

/// The template `cdk import` submits: the deployed one plus the declaration of the queue being
/// adopted, which carries the `DeletionPolicy` every imported resource must, and outputs
/// naming the adopted resource so what the stack publishes for it is observable.
fn with_the_adopted_queue(properties: &str) -> String {
    format!(
        r#"{{"Resources":{{
            "{KEPT}":{{"Type":"AWS::SQS::Queue","Properties":{{}}}},
            "{ADOPTED}":{{"Type":"AWS::SQS::Queue","DeletionPolicy":"Retain",
                          "Properties":{properties}}}}},
           "Outputs":{{"AdoptedRef":{{"Value":{{"Ref":"{ADOPTED}"}}}},
                       "AdoptedArn":{{"Value":{{"Fn::GetAtt":["{ADOPTED}","Arn"]}}}},
                       "AdoptedName":{{"Value":{{"Fn::GetAtt":["{ADOPTED}","QueueName"]}}}}}}}}"#
    )
}

/// One row of the adoption table: a queue that exists outside CloudFormation, and the
/// declaration the import brings it under.
struct AdoptionRow {
    case: &'static str,
    stack_name: &'static str,
    /// The name the queue was created with OUTSIDE CloudFormation — the resource the import
    /// adopts.
    queue_name: &'static str,
    /// The attributes that queue was created with, which importing it must not change.
    attributes: Value,
    /// The `Properties` the imported resource is declared with. A row that states the queue's
    /// name there is the case where CloudFormation can compute the identifier itself; one that
    /// states none leaves the name to CloudFormation, so the identifier can only come from the
    /// resource that already exists.
    properties: String,
}

/// RULE: executing an IMPORT change set adopts the resources it names under the identifiers
/// they already exist under — nothing is created in the owning service, the adopted resource is
/// left exactly as it was, and a later deployment of the same template keeps that identity.
#[tokio::test]
async fn an_executed_import_adopts_the_queue_that_already_exists() {
    let router = build_router();
    let rows = vec![
        AdoptionRow {
            case: "a queue the template does not name",
            stack_name: "adopt-generated",
            queue_name: "outside-plain",
            attributes: json!({"VisibilityTimeout": "42"}),
            properties: "{}".to_string(),
        },
        AdoptionRow {
            case: "a queue the template names itself",
            stack_name: "adopt-named",
            queue_name: "outside-named",
            attributes: json!({"VisibilityTimeout": "77"}),
            properties: r#"{"QueueName":"outside-named"}"#.to_string(),
        },
        AdoptionRow {
            case: "a FIFO queue, whose name carries the suffix Amazon SQS requires",
            stack_name: "adopt-fifo",
            queue_name: "outside-fifo.fifo",
            attributes: json!({"FifoQueue": "true"}),
            properties: r#"{"FifoQueue":true}"#.to_string(),
        },
    ];

    for row in &rows {
        let case = row.case;
        // The stack as it stands before the import, deployed the way `cdk deploy` deploys it:
        // a CREATE change set, which leaves the stack in REVIEW_IN_PROGRESS, and the execute
        // that makes it exist.
        let before = before_the_import();
        cloudformation_ok(
            &router,
            case,
            "CreateChangeSet",
            &[
                ("StackName", row.stack_name),
                ("ChangeSetName", "deploy"),
                ("ChangeSetType", "CREATE"),
                ("TemplateBody", &before),
            ],
        )
        .await;
        tokio::time::sleep(PAST_THE_STACK_SPAN).await;
        let body =
            cloudformation_ok(&router, case, "DescribeStacks", &[("StackName", row.stack_name)])
                .await;
        assert_eq!(
            first_element(&body, "StackStatus").as_deref(),
            Some("REVIEW_IN_PROGRESS"),
            "case {case:?}: a CREATE change set leaves the stack in review, got {body}"
        );
        cloudformation_ok(
            &router,
            case,
            "ExecuteChangeSet",
            &[("StackName", row.stack_name), ("ChangeSetName", "deploy")],
        )
        .await;
        tokio::time::sleep(PAST_THE_STACK_SPAN).await;
        let body =
            cloudformation_ok(&router, case, "DescribeStacks", &[("StackName", row.stack_name)])
                .await;
        assert_eq!(
            first_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: the executed CREATE change set must deploy the stack, got {body}"
        );
        // ...and the template it executed is the stack's from here on, which is what the
        // deploying tool reads back before it plans an import.
        for stage in ["", "Original", "Processed"] {
            let mut members = vec![("StackName", row.stack_name)];
            if !stage.is_empty() {
                members.push(("TemplateStage", stage));
            }
            let body = cloudformation_ok(&router, case, "GetTemplate", &members).await;
            assert_eq!(
                first_element(&body, "TemplateBody").as_deref(),
                Some(before.as_str()),
                "case {case:?}: GetTemplate(TemplateStage={stage:?}) must serve the executed \
                 template, got {body}"
            );
        }

        // ...and the queue that exists OUTSIDE CloudFormation, the one the import adopts.
        let mut create = json!({"QueueName": row.queue_name});
        create["Attributes"] = row.attributes.clone();
        let created = sqs_ok(&router, case, "CreateQueue", create).await;
        let url = created
            .get("QueueUrl")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            url,
            queue_url(row.queue_name),
            "case {case:?}: expected the queue's own url, got {created}"
        );
        let before_adoption = reported_attributes(&router, case, &url).await;

        // The IMPORT change set: the plan reports the resource as adopted under the
        // identifier it already has. It is created under the SAME change-set name the deploy
        // used, which is what a deploying tool reuses — so the one that deployed the stack is
        // deleted first, exactly as that tool deletes it.
        let template = with_the_adopted_queue(&row.properties);
        // ...and the identifier the request names it by is the one the template summary
        // publishes for the type, which is where a deploying tool reads it from.
        let summary = cloudformation_ok(
            &router,
            case,
            "GetTemplateSummary",
            &[("TemplateBody", &template)],
        )
        .await;
        assert!(
            elements(&summary, "ResourceIdentifiers")
                .iter()
                .any(|identifiers| identifiers.contains("<member>QueueUrl</member>")),
            "case {case:?}: the summary must publish the type's own identifier property, got \
             {summary}"
        );
        cloudformation_ok(
            &router,
            case,
            "DeleteChangeSet",
            &[("StackName", row.stack_name), ("ChangeSetName", "deploy")],
        )
        .await;
        cloudformation_ok(
            &router,
            case,
            "CreateChangeSet",
            &[
                ("StackName", row.stack_name),
                ("ChangeSetName", "deploy"),
                ("ChangeSetType", "IMPORT"),
                ("TemplateBody", &template),
                ("ResourcesToImport.member.1.ResourceType", "AWS::SQS::Queue"),
                ("ResourcesToImport.member.1.LogicalResourceId", ADOPTED),
                (
                    "ResourcesToImport.member.1.ResourceIdentifier.entry.1.key",
                    "QueueUrl",
                ),
                (
                    "ResourcesToImport.member.1.ResourceIdentifier.entry.1.value",
                    &url,
                ),
            ],
        )
        .await;
        tokio::time::sleep(PAST_THE_STACK_SPAN).await;
        let body = cloudformation_ok(
            &router,
            case,
            "DescribeChangeSet",
            &[("StackName", row.stack_name), ("ChangeSetName", "deploy")],
        )
        .await;
        assert_eq!(
            (
                first_element(&body, "Status").unwrap_or_default(),
                elements(&body, "Action"),
                elements(&body, "PhysicalResourceId"),
            ),
            (
                "CREATE_COMPLETE".to_string(),
                vec!["Import".to_string()],
                vec![url.clone()],
            ),
            "case {case:?}: expected one Import of the queue that exists, got {body}"
        );

        // Executing it brings that queue under the stack's management.
        cloudformation_ok(
            &router,
            case,
            "ExecuteChangeSet",
            &[("StackName", row.stack_name), ("ChangeSetName", "deploy")],
        )
        .await;
        tokio::time::sleep(PAST_THE_STACK_SPAN).await;
        let body =
            cloudformation_ok(&router, case, "DescribeStacks", &[("StackName", row.stack_name)])
                .await;
        assert_eq!(
            first_element(&body, "StackStatus").as_deref(),
            Some("IMPORT_COMPLETE"),
            "case {case:?}: an executed import completes as an import, got {body}"
        );
        // What the stack publishes for the adopted resource addresses the queue that really
        // exists: its url, its ARN and its name.
        let expected_outputs: BTreeMap<String, String> = [
            ("AdoptedRef".to_string(), url.clone()),
            ("AdoptedArn".to_string(), queue_arn(row.queue_name)),
            ("AdoptedName".to_string(), row.queue_name.to_string()),
        ]
        .into_iter()
        .collect();
        let outputs = outputs_of(&body);
        assert_eq!(
            outputs, expected_outputs,
            "case {case:?}: expected {expected_outputs:?}, got {outputs:?}"
        );
        let resource = cloudformation_ok(
            &router,
            case,
            "DescribeStackResource",
            &[
                ("StackName", row.stack_name),
                ("LogicalResourceId", ADOPTED),
            ],
        )
        .await;
        assert_eq!(
            (
                first_element(&resource, "PhysicalResourceId").unwrap_or_default(),
                first_element(&resource, "ResourceStatus").unwrap_or_default(),
            ),
            (url.clone(), "IMPORT_COMPLETE".to_string()),
            "case {case:?}: the adopted resource must be reported as imported under the \
             identifier it already had, got {resource}"
        );
        // Nothing was asked of Amazon SQS about it: the queue is exactly as it was, and no
        // second queue was created under the name CloudFormation would have generated.
        let after_adoption = reported_attributes(&router, case, &url).await;
        assert_eq!(
            after_adoption, before_adoption,
            "case {case:?}: importing a queue must leave it untouched, got {after_adoption:?} \
             after {before_adoption:?}"
        );
        let generated_prefix = format!("{}-{ADOPTED}", row.stack_name);
        let generated = queues_named(&router, case, &generated_prefix).await;
        assert!(
            generated.is_empty(),
            "case {case:?}: the import must create no queue of its own, got {generated:?}"
        );

        // ...and the deployment that follows the import — `cdk import` applies the stack's
        // tags with one — keeps the adopted identity rather than replacing the resource.
        cloudformation_ok(
            &router,
            case,
            "UpdateStack",
            &[
                ("StackName", row.stack_name),
                ("TemplateBody", &template),
                ("Tags.member.1.Key", "Owner"),
                ("Tags.member.1.Value", "the-import"),
            ],
        )
        .await;
        tokio::time::sleep(PAST_THE_STACK_SPAN).await;
        let body =
            cloudformation_ok(&router, case, "DescribeStacks", &[("StackName", row.stack_name)])
                .await;
        assert_eq!(
            first_element(&body, "StackStatus").as_deref(),
            Some("UPDATE_COMPLETE"),
            "case {case:?}: the deployment after the import must complete, got {body}"
        );
        let resource = cloudformation_ok(
            &router,
            case,
            "DescribeStackResource",
            &[
                ("StackName", row.stack_name),
                ("LogicalResourceId", ADOPTED),
            ],
        )
        .await;
        assert_eq!(
            first_element(&resource, "PhysicalResourceId").as_deref(),
            Some(url.as_str()),
            "case {case:?}: the resource must keep the identity it was adopted under, got \
             {resource}"
        );
        let after_update = reported_attributes(&router, case, &url).await;
        assert_eq!(
            after_update.get("CreatedTimestamp"),
            before_adoption.get("CreatedTimestamp"),
            "case {case:?}: the adopted queue must not be re-created, got {after_update:?} \
             after {before_adoption:?}"
        );
        let generated = queues_named(&router, case, &generated_prefix).await;
        assert!(
            generated.is_empty(),
            "case {case:?}: no deployment after the import may create a queue of its own, got \
             {generated:?}"
        );
    }
}

/// RULE's converse: an identifier the owning service does NOT hold is no resource to adopt, so
/// the change set settles FAILED naming the type and the identifier, and the stack is left
/// exactly as it was.
#[tokio::test]
async fn an_import_of_a_queue_that_does_not_exist_fails_the_plan() {
    let router = build_router();
    let before = before_the_import();
    cloudformation_ok(
        &router,
        "the stack before the refused import",
        "CreateStack",
        &[("StackName", "adopt-absent"), ("TemplateBody", &before)],
    )
    .await;
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    // `(case, the change set's name, the identifier the request names)`.
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "a queue url of the right form that nothing holds",
            "absent-url",
            queue_url("never-created"),
        ),
        (
            "a queue url of another account's queue",
            "absent-other",
            format!("https://sqs.{REGION}.amazonaws.com/111122223333/elsewhere"),
        ),
        (
            "an identifier that is no queue url at all",
            "absent-name",
            "not-a-queue-url".to_string(),
        ),
    ];

    for (case, change_set, identifier) in &rows {
        let template = with_the_adopted_queue("{}");
        cloudformation_ok(
            &router,
            case,
            "CreateChangeSet",
            &[
                ("StackName", "adopt-absent"),
                ("ChangeSetName", change_set),
                ("ChangeSetType", "IMPORT"),
                ("TemplateBody", &template),
                ("ResourcesToImport.member.1.ResourceType", "AWS::SQS::Queue"),
                ("ResourcesToImport.member.1.LogicalResourceId", ADOPTED),
                (
                    "ResourcesToImport.member.1.ResourceIdentifier.entry.1.key",
                    "QueueUrl",
                ),
                (
                    "ResourcesToImport.member.1.ResourceIdentifier.entry.1.value",
                    identifier,
                ),
            ],
        )
        .await;
        tokio::time::sleep(PAST_THE_STACK_SPAN).await;
        let body = cloudformation_ok(
            &router,
            case,
            "DescribeChangeSet",
            &[("StackName", "adopt-absent"), ("ChangeSetName", change_set)],
        )
        .await;
        let expected = format!(
            "Resource of type 'AWS::SQS::Queue' with identifier '{identifier}' was not found."
        );
        assert_eq!(
            (
                first_element(&body, "Status").unwrap_or_default(),
                first_element(&body, "StatusReason").unwrap_or_default(),
            ),
            ("FAILED".to_string(), expected.clone()),
            "case {case:?}: expected a failed plan naming the identifier nothing holds, got \
             {body}"
        );
        // ...and the failed plan is one the stack still HOLDS, which is how a deploying tool
        // reads what became of the import it asked for.
        let listed =
            cloudformation_ok(&router, case, "ListChangeSets", &[("StackName", "adopt-absent")])
                .await;
        assert_eq!(
            (
                elements(&listed, "ChangeSetName").contains(&(*change_set).to_string()),
                elements(&listed, "Status")
                    .iter()
                    .filter(|status| *status == "FAILED")
                    .count(),
            ),
            (true, 1),
            "case {case:?}: expected the failed change set to be listed once, got {listed}"
        );
        // The stack is untouched: the plan was never carried out.
        let body =
            cloudformation_ok(&router, case, "DescribeStacks", &[("StackName", "adopt-absent")])
                .await;
        assert_eq!(
            first_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: the stack must be left as it was, got {body}"
        );
        // One change set at a time, as the deploying tool that reuses one name leaves it:
        // this one is deleted before the next row's is created.
        cloudformation_ok(
            &router,
            case,
            "DeleteChangeSet",
            &[("StackName", "adopt-absent"), ("ChangeSetName", change_set)],
        )
        .await;
    }
}

/// RULE's limit: the identity carried forward is the one of a resource the stack ADOPTED. A
/// resource the stack CREATED itself keeps being identified by the name CloudFormation
/// computes for it, so a declaration whose generated name changes — one that asks for the
/// variant whose name the owning service requires a suffix on — is a DIFFERENT resource, which
/// the deployment creates.
#[tokio::test]
async fn a_self_created_queue_is_still_identified_by_the_name_cloudformation_computes() {
    let router = build_router();
    let case = "a nameless queue whose declaration asks for the FIFO variant";
    let standard = format!(
        r#"{{"Resources":{{"{KEPT}":{{"Type":"AWS::SQS::Queue","Properties":{{}}}}}},
            "Outputs":{{"KeptRef":{{"Value":{{"Ref":"{KEPT}"}}}}}}}}"#
    );
    cloudformation_ok(
        &router,
        case,
        "CreateStack",
        &[("StackName", "suffix-toggle"), ("TemplateBody", &standard)],
    )
    .await;
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let body =
        cloudformation_ok(&router, case, "DescribeStacks", &[("StackName", "suffix-toggle")]).await;
    let created = outputs_of(&body)
        .get("KeptRef")
        .cloned()
        .unwrap_or_default();
    assert!(
        !created.is_empty() && !created.ends_with(".fifo"),
        "case {case:?}: expected the stack to publish a standard queue's url, got {body}"
    );

    // The same declaration asking for the FIFO variant: its generated name carries the suffix
    // Amazon SQS requires, so it is another queue — and the one the stack had is gone.
    let fifo = format!(
        r#"{{"Resources":{{"{KEPT}":{{"Type":"AWS::SQS::Queue",
            "Properties":{{"FifoQueue":true}}}}}},
            "Outputs":{{"KeptRef":{{"Value":{{"Ref":"{KEPT}"}}}}}}}}"#
    );
    cloudformation_ok(
        &router,
        case,
        "UpdateStack",
        &[("StackName", "suffix-toggle"), ("TemplateBody", &fifo)],
    )
    .await;
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let body =
        cloudformation_ok(&router, case, "DescribeStacks", &[("StackName", "suffix-toggle")]).await;
    let replaced = outputs_of(&body)
        .get("KeptRef")
        .cloned()
        .unwrap_or_default();
    assert!(
        replaced.ends_with(".fifo") && replaced != created,
        "case {case:?}: expected a FIFO queue of its own, got {replaced:?} after {created:?} \
         ({body})"
    );
    let (status, reported) = sqs(
        &router,
        "GetQueueAttributes",
        &json!({"QueueUrl": replaced, "AttributeNames": ["All"]}),
    )
    .await;
    assert_eq!(
        (
            status,
            reported
                .pointer("/Attributes/FifoQueue")
                .and_then(Value::as_str),
        ),
        (200, Some("true")),
        "case {case:?}: expected Amazon SQS to hold the FIFO queue the update created, got \
         {status} {reported}"
    );
}
