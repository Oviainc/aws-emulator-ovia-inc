// SPDX-License-Identifier: CC0-1.0

//! The SNS -> Lambda permission boundary over TWO stacks deployed independently of each
//! other: one template synthesized twice, once with the `AWS::Lambda::Permission` beside the
//! `AWS::SNS::Subscription` and once without, each deployed as its own stack.
//!
//! The rule under test is the sibling suite's — a publish to a topic invokes every confirmed
//! `lambda`-protocol subscriber of it, and only where the FUNCTION's own resource-based
//! policy grants `lambda:InvokeFunction` to `sns.amazonaws.com` for that topic. What this
//! suite adds is the axis that rule is read along here: the two legs are SEPARATE
//! deployments, so each one's topic, function and sink belong to its own stack rather than to
//! one stack before and after an update — the shape a client gets from a template whose
//! synthesis is parameterised on whether the grant is emitted, and the shape in which a
//! permission-boundary regression in that template shows up.
//!
//! Everything but the deployment shape is the sibling suite's: its router, its awsQuery
//! helpers, its template and its queue observations
//! ([`crate::sns_lambda_subscription_permission_boundary`]), so there is one harness for the
//! two readings of the boundary rather than a copy per reading. The observation is still made
//! where a client makes it: on the sink queue the invoked function forwards to.

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{element, outputs_of};
use crate::sns_lambda_subscription_permission_boundary::{
    build_router, cloudformation, execution_role, sns, stayed_quiet, template, wait_for,
    CREATE_COMPLETE,
};

/// One leg: the case, the stack it deploys, whether that stack's template declares the grant,
/// the marker it publishes, and whether the subscribed function must be invoked.
type Leg = (&'static str, &'static str, bool, &'static str, bool);

/// Two stacks of the same template, deployed independently: the one that declares the grant
/// invokes its function and the one that does not invokes nothing, which is the whole
/// difference a client can observe between them.
#[tokio::test]
async fn two_separately_deployed_stacks_differ_only_in_whether_the_function_is_invoked() {
    let router = build_router();

    // The execution role both templates name, created where that service holds it.
    execution_role(&router).await;

    let legs: &[Leg] = &[
        (
            "a stack synthesized without the permission",
            "sns-lambda-unpermitted",
            false,
            "marker-broken",
            false,
        ),
        (
            "a stack synthesized with the permission",
            "sns-lambda-permitted",
            true,
            "marker-fixed",
            true,
        ),
    ];

    for (case, stack, with_permission, marker, invoked) in legs {
        cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", stack),
                ("TemplateBody", &template(stack, *with_permission)),
            ],
        )
        .await;
        let (status, described) = wait_for_stack_status(&router, stack, CREATE_COMPLETE).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: the stack must deploy, got {status} with {described}"
        );
        let outputs = outputs_of(&described);
        let topic = outputs.get("TopicArn").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: the stack must publish its TopicArn, got {described}")
        });
        let queue_url = outputs.get("QueueUrl").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: the stack must publish its QueueUrl, got {described}")
        });

        let published = sns(
            &router,
            "Publish",
            &[("TopicArn", &topic), ("Message", marker)],
        )
        .await;
        assert!(
            !element(&published, "MessageId").is_empty(),
            "case {case:?}: the publish must be accepted whether or not the delivery is \
             permitted, got {published}"
        );

        if *invoked {
            let delivered = wait_for(&router, &queue_url, marker).await;
            assert!(
                delivered.iter().any(|body| body.contains(marker)),
                "case {case:?}: expected the subscribed function to be invoked and to forward \
                 {marker:?} to its own stack's sink queue, got {delivered:?}"
            );
            continue;
        }
        let quiet = stayed_quiet(&router, &queue_url).await;
        assert!(
            quiet.is_empty(),
            "case {case:?}: expected the function never to be invoked, so nothing reaches its \
             own stack's sink queue, got {quiet:?}"
        );
    }
}
