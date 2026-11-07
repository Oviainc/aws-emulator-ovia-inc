// SPDX-License-Identifier: CC0-1.0

//! A stack policy submitted BY REFERENCE — `StackPolicyURL`, `StackPolicyDuringUpdateURL` —
//! end to end over the awsQuery wire.
//!
//! RULE: a stack policy reaches CloudFormation either inline, as the request's own policy
//! body, or by reference, as a URL naming "a policy (maximum size: 16 KB) located in an
//! Amazon S3 bucket in the same Amazon Web Services Region as the stack". The reference says
//! WHERE the document is, not what it means: the object it addresses is read and the
//! operation then does exactly what the same body submitted inline would make it do — the
//! same policy stored on the stack, the same verdict on the next update — while a URL
//! addressing no object this account can read is a `ValidationError` naming its own member.
//!
//! The rows differ along what the rule generalises over: WHICH operation takes the reference
//! (`CreateStack`, `UpdateStack`, `SetStackPolicy`), which MEMBER carries it (the policy
//! stored on the stack and the one that overrides it for a single update), the addressing
//! style of the URL, and — for the refusals — what about the reference cannot be resolved (a
//! key that is not there, a scheme that is not supported, a URL carrying no key, and a
//! request that submits the document both ways).

use crate::change_set_lifecycle::settle_the_store;
use crate::harness::{call, create_stack, member, router_with_engine, REGION};
use crate::stack_lifecycle::{describe_stack, xml_text, xml_texts};
use crate::template_by_reference::{create_staging_bucket, stage, STAGING_BUCKET};
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

/// A template declaring two parameters, so an update can change one of them while a policy
/// protects the other.
fn template(protected: &str, free: &str) -> String {
    format!(
        r#"{{"Resources":{{
             "Protected":{{"Type":"AWS::SSM::Parameter",
               "Properties":{{"Type":"String","Value":"{protected}"}}}},
             "Free":{{"Type":"AWS::SSM::Parameter",
               "Properties":{{"Type":"String","Value":"{free}"}}}}}}}}"#
    )
}

/// The blanket permission every documented stack policy that means to allow anything
/// carries, which the documents protecting ONE resource sit behind.
const ALLOW_EVERYTHING: &str =
    r#"{"Effect":"Allow","Action":"Update:*","Principal":"*","Resource":"*"}"#;

/// One policy document from the statements it carries, in document order.
fn policy_of(statements: &[&str]) -> String {
    format!(r#"{{"Statement":[{}]}}"#, statements.join(","))
}

/// A policy that denies every update action on `Protected` and allows the rest.
fn protecting_policy() -> String {
    policy_of(&[
        ALLOW_EVERYTHING,
        r#"{"Effect":"Deny","Action":"Update:*","Principal":"*",
            "Resource":"LogicalResourceId/Protected"}"#,
    ])
}

/// A policy that allows every update action on every resource: the temporary override a
/// caller supplies to get a protected resource updated once.
fn permissive_policy() -> String {
    policy_of(&[ALLOW_EVERYTHING])
}

/// The virtual-hosted URL of one staged object, the addressing style the CDK's own uploads
/// use. Source: Amazon S3's documented request styles.
fn virtual_hosted(key: &str) -> String {
    format!("https://{STAGING_BUCKET}.s3.{REGION}.amazonaws.com/{key}")
}

/// The path-style URL of the same object, so a row can vary the addressing without varying
/// the object.
fn path_style(key: &str) -> String {
    format!("https://s3.{REGION}.amazonaws.com/{STAGING_BUCKET}/{key}")
}

/// `GetStackPolicy` on one stack: the status and the body.
async fn get_stack_policy(router: &MultiServiceRouter, stack: &str) -> (u16, String) {
    call(router, "GetStackPolicy", &[member("StackName", stack)]).await
}

/// The policy document `GetStackPolicy` reports for one stack.
async fn stored_policy(router: &MultiServiceRouter, stack: &str) -> Option<String> {
    let (_, body) = get_stack_policy(router, stack).await;
    xml_text(&body, "StackPolicyBody")
}

/// Which operation a row submits the reference on, and under which member.
#[derive(Clone, Copy, Debug)]
enum Submitted {
    /// `CreateStack(StackPolicyURL)`.
    Create,
    /// `UpdateStack(StackPolicyURL)`, which replaces the stored document.
    Update,
    /// `SetStackPolicy(StackPolicyURL)`.
    Set,
}

/// One row of the applied table: a staged policy object, submitted by reference on one
/// operation, which must become the document the stack carries.
struct Applied {
    case: &'static str,
    stack: &'static str,
    key: &'static str,
    /// The URL spelling the row addresses the staged object with.
    url: fn(&str) -> String,
    submitted: Submitted,
}

fn applied_rows() -> Vec<Applied> {
    vec![
        Applied {
            case: "CreateStack takes the reference, addressed virtual-hosted",
            stack: "policy-by-reference-created",
            key: "policies/created.json",
            url: virtual_hosted,
            submitted: Submitted::Create,
        },
        Applied {
            case: "UpdateStack takes the reference, addressed path-style",
            stack: "policy-by-reference-updated",
            key: "nested/prefix/updated.json",
            url: path_style,
            submitted: Submitted::Update,
        },
        Applied {
            case: "SetStackPolicy takes the reference",
            stack: "policy-by-reference-set",
            key: "set.json",
            url: virtual_hosted,
            submitted: Submitted::Set,
        },
    ]
}

#[tokio::test]
async fn a_stack_policy_staged_in_amazon_s3_becomes_the_policy_the_stack_carries() {
    let (router, engine) = router_with_engine();
    create_staging_bucket(&router).await;

    for row in applied_rows() {
        let case = row.case;
        let policy = protecting_policy();
        stage(&router, row.key, &policy).await;
        let url = (row.url)(row.key);

        // The stack the row acts on. A row that submits the reference on the CREATE carries
        // it here; the others create the stack with no policy at all first.
        let created_with: Vec<(String, String)> = match row.submitted {
            Submitted::Create => vec![member("StackPolicyURL", &url)],
            _ => Vec::new(),
        };
        let (code, body) =
            create_stack(&router, row.stack, &template("v1", "v1"), &created_with).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the stack must be created, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;

        match row.submitted {
            Submitted::Create => {}
            Submitted::Update => {
                // An update that submits nothing but a new policy is still a change to the
                // stack, so it is not the no-op CloudFormation refuses.
                let (code, body) = call(
                    &router,
                    "UpdateStack",
                    &[
                        member("StackName", row.stack),
                        member("UsePreviousTemplate", "true"),
                        member("StackPolicyURL", &url),
                    ],
                )
                .await;
                assert_eq!(
                    code, 200,
                    "case {case:?}: the update must apply the staged policy, got {code} {body}"
                );
                settle_the_store(&router, &engine).await;
                let (code, body) = describe_stack(&router, row.stack).await;
                assert_eq!(
                    xml_text(&body, "StackStatus").as_deref(),
                    Some(status::UPDATE_COMPLETE),
                    "case {case:?}: the policy-only update must complete, got {code} {body}"
                );
            }
            Submitted::Set => {
                let (code, body) = call(
                    &router,
                    "SetStackPolicy",
                    &[
                        member("StackName", row.stack),
                        member("StackPolicyURL", &url),
                    ],
                )
                .await;
                assert_eq!(
                    code, 200,
                    "case {case:?}: SetStackPolicy must read the staged policy, got {code} \
                     {body}"
                );
            }
        }

        // The staged document, byte for byte, is what the stack now carries: nothing it
        // reports came from the request, which named only the URL.
        let stored = stored_policy(&router, row.stack).await;
        assert_eq!(
            stored.as_deref(),
            Some(policy.as_str()),
            "case {case:?}: expected the staged policy on the stack, got {stored:?}"
        );

        // ...and it GOVERNS: the update the policy denies fails on the protected resource
        // and the stack rolls back, exactly as the same document submitted inline makes it.
        let (code, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", row.stack),
                member("TemplateBody", &template("v2", "v2")),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the denied update is accepted and then fails, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        let (code, body) = describe_stack(&router, row.stack).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::UPDATE_ROLLBACK_COMPLETE),
            "case {case:?}: the staged policy must deny the update, got {code} {body}"
        );
        let (code, body) = call(
            &router,
            "DescribeStackEvents",
            &[member("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the events must be readable, got {code} {body}"
        );
        let reasons: Vec<String> = xml_texts(&body, "ResourceStatusReason");
        assert!(
            reasons.iter().any(|reason| reason
                == "Action denied by stack policy: Statement [#2] does not allow \
                    [Update:*] for resource [LogicalResourceId/Protected];"),
            "case {case:?}: expected the staged policy's own denial, got {reasons:?}"
        );
        assert!(
            xml_texts(&body, "ResourceStatus")
                .iter()
                .any(|reported| reported == resource_status::UPDATE_FAILED),
            "case {case:?}: the protected resource's update must fail, got {body}"
        );
    }
}

#[tokio::test]
async fn a_policy_staged_in_amazon_s3_overrides_the_stored_one_for_one_update() {
    let (router, engine) = router_with_engine();
    create_staging_bucket(&router).await;
    stage(&router, "policies/stored.json", &protecting_policy()).await;
    stage(&router, "policies/override.json", &permissive_policy()).await;

    let stack = "policy-by-reference-override";
    let (code, body) = create_stack(
        &router,
        stack,
        &template("v1", "v1"),
        &[member(
            "StackPolicyURL",
            &virtual_hosted("policies/stored.json"),
        )],
    )
    .await;
    assert_eq!(
        code, 200,
        "the stack must be created with the staged policy, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;

    // The override is read from Amazon S3 and governs THIS update alone, so the protected
    // resource's change goes through.
    let (code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member("TemplateBody", &template("v2", "v1")),
            member(
                "StackPolicyDuringUpdateURL",
                &virtual_hosted("policies/override.json"),
            ),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"the overridden update\": expected it to be accepted, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;
    let (code, body) = describe_stack(&router, stack).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::UPDATE_COMPLETE),
        "case \"the overridden update\": expected it to complete, got {code} {body}"
    );

    // ...and it was never stored: the stack still carries the document it was created with,
    // so the NEXT update is denied again.
    let stored = stored_policy(&router, stack).await;
    assert_eq!(
        stored.as_deref(),
        Some(protecting_policy().as_str()),
        "case \"after the override\": expected the stored policy to stand, got {stored:?}"
    );
    let (code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member("TemplateBody", &template("v3", "v1")),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"the update after the override\": expected it to be accepted, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;
    let (code, body) = describe_stack(&router, stack).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::UPDATE_ROLLBACK_COMPLETE),
        "case \"the update after the override\": the stored policy must deny it, got {code} \
         {body}"
    );
}

/// One row of the refusal table: a reference that resolves to no readable policy, or a
/// request that submits the document twice, and the `ValidationError` message it answers.
struct Refused {
    case: &'static str,
    /// The members the row adds to a `SetStackPolicy` and a `CreateStack` request.
    members: Vec<(String, String)>,
    message: String,
}

fn refusal_rows(present_key: &str) -> Vec<Refused> {
    vec![
        Refused {
            // A key the bucket does not hold: a stack operation reports the object's absence
            // as absence, naming the bucket and key the URL addressed, which is the refusal
            // the recorded `CreateStack(StackPolicyURL)` check carries. (A STACK SET
            // operation words the same refusal as what Amazon S3 answered —
            // `template_by_reference.rs` is the suite that drives that one.)
            case: "a key the staging bucket does not hold",
            members: vec![member(
                "StackPolicyURL",
                &virtual_hosted("policies/absent.json"),
            )],
            message: format!(
                "S3 object does not exist. Bucket: {STAGING_BUCKET}, Key: policies/absent.json. \
                 Error: NoSuchKey"
            ),
        },
        Refused {
            case: "a bucket nothing created",
            members: vec![member(
                "StackPolicyURL",
                &format!("https://no-such-policy-bucket.s3.{REGION}.amazonaws.com/{present_key}"),
            )],
            message: "S3 error: The specified bucket does not exist\nFor more information check \
                      http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html"
                .to_string(),
        },
        Refused {
            case: "a scheme the documentation does not support",
            members: vec![member(
                "StackPolicyURL",
                &format!("s3://{STAGING_BUCKET}/{present_key}"),
            )],
            message: "The location for an Amazon S3 bucket must start with https://".to_string(),
        },
        Refused {
            case: "a host that is no Amazon S3 endpoint and carries no object",
            members: vec![member(
                "StackPolicyURL",
                "https://policies.example.com/deny.json",
            )],
            message: "StackPolicyURL must be a supported URL.".to_string(),
        },
        Refused {
            case: "an https URL carrying no key at all",
            members: vec![member(
                "StackPolicyURL",
                &format!("https://{STAGING_BUCKET}.s3.{REGION}.amazonaws.com/"),
            )],
            message: "StackPolicyURL must reference a valid S3 object to which you have access."
                .to_string(),
        },
        Refused {
            case: "the policy submitted both inline and by reference",
            members: vec![
                member("StackPolicyBody", &permissive_policy()),
                member("StackPolicyURL", &virtual_hosted(present_key)),
            ],
            message: "You cannot specify both StackPolicyBody and StackPolicyURL".to_string(),
        },
    ]
}

#[tokio::test]
async fn a_stack_policy_reference_that_resolves_to_no_readable_object_is_refused() {
    let (router, engine) = router_with_engine();
    create_staging_bucket(&router).await;
    let present_key = "policies/present.json";
    stage(&router, present_key, &permissive_policy()).await;

    // The stack `SetStackPolicy` and `UpdateStack` act on, which no refusal below may
    // change.
    let stack = "policy-by-reference-refused";
    let (code, body) = create_stack(&router, stack, &template("v1", "v1"), &[]).await;
    assert_eq!(
        code, 200,
        "the stack the refusals act on must exist, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;

    for row in refusal_rows(present_key) {
        let case = row.case;
        // The rule is the same for every operation that takes the reference, so each row is
        // driven through all three of them.
        for (action, head) in [
            (
                "CreateStack",
                vec![
                    member("StackName", "policy-by-reference-never-created"),
                    member("TemplateBody", &template("v1", "v1")),
                ],
            ),
            (
                "UpdateStack",
                vec![
                    member("StackName", stack),
                    member("TemplateBody", &template("v2", "v2")),
                ],
            ),
            ("SetStackPolicy", vec![member("StackName", stack)]),
        ] {
            let mut members = head;
            members.extend(row.members.iter().cloned());
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
            assert_eq!(
                message, row.message,
                "case {case:?}: {action} must name what could not be read, expected {:?}, got \
                 {message:?}",
                row.message
            );
        }
    }

    // Nothing above changed the stack: a refused reference writes neither a policy nor a
    // template.
    let stored = stored_policy(&router, stack).await;
    assert_eq!(
        stored, None,
        "case \"after the refusals\": the stack must carry no policy, got {stored:?}"
    );
    let (code, body) = describe_stack(&router, "policy-by-reference-never-created").await;
    assert_eq!(
        code, 400,
        "case \"the refused create\": expected no stack to exist, got {code} {body}"
    );
}

