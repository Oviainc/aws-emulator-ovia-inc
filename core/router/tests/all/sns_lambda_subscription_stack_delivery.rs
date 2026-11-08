// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation-deployed SNS -> Lambda pipeline invokes its function ONCE PER PUBLISHED
//! MESSAGE, and each invocation carries that message in the `Records` envelope a function's
//! event is read from.
//!
//! The rule under test: **SNS "delivers the message to each endpoint that is subscribed to the
//! topic" (`Publish`) — per MESSAGE, so a permitted `lambda` subscription is invoked as many
//! times as messages were published to its topic, each invocation carrying one of them and no
//! message carried twice.** The permission boundary that decides whether the delivery happens
//! at all is the sibling suite's
//! ([`crate::sns_lambda_subscription_permission_boundary`]); this is the multiplicity, over the
//! same deployed estate and the same wire, so neither suite writes its own harness: the
//! router, the template, the role and the queue reader are that suite's.
//!
//! The observation is made where a client makes it — on the sink queue the deployed function
//! forwards each record's message to — so an invocation that did not happen, or happened
//! twice, is visible as a body missing from or repeated on that queue.

use std::collections::BTreeMap;
use std::time::Instant;

use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, outputs_of};
use crate::sns_lambda_subscription_permission_boundary::{
    build_router, cloudformation, drain, execution_role, sns, template, CREATE_COMPLETE,
    DELIVERY_BUDGET, LOOK_INTERVAL,
};

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;

/// How many messages this suite publishes to the deployed topic. More than one, because the
/// rule is per message: a pipeline that invoked its function once for a burst would pass a
/// single-message assertion.
const PUBLISHED: usize = 3;

/// Everything the queue at `url` holds once it holds `wanted` bodies — or everything it holds
/// at the deadline, which is what a pipeline that under-delivered leaves.
async fn collected(router: &MultiServiceRouter, url: &str, wanted: usize) -> Vec<String> {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    let mut seen: Vec<String> = Vec::new();
    loop {
        seen.extend(drain(router, url).await);
        if seen.len() >= wanted || Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// RULE: a deployed, permitted `lambda` subscription is invoked once per published message,
/// each invocation carrying one of them.
#[tokio::test]
async fn a_deployed_subscription_invokes_its_function_once_per_published_message() {
    let router = build_router();
    let stack = "sns-lambda-multiplicity";

    // The execution role the template names, created where that service holds it.
    execution_role(&router).await;

    // The permitted estate: topic, function, subscription and the grant, as the sibling
    // suite's template declares them.
    cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &template(stack, true)),
        ],
    )
    .await;
    let (status, described) = wait_for_stack_status(&router, stack, CREATE_COMPLETE).await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "the pipeline must deploy, got {status} with {described}"
    );
    let outputs = outputs_of(&described);
    let topic = outputs
        .get("TopicArn")
        .cloned()
        .unwrap_or_else(|| panic!("the stack must publish its TopicArn, got {described}"));
    let queue_url = outputs
        .get("QueueUrl")
        .cloned()
        .unwrap_or_else(|| panic!("the stack must publish its QueueUrl, got {described}"));

    let mut published: BTreeMap<String, usize> = BTreeMap::new();
    for round in 0..PUBLISHED {
        let marker = format!("{stack}-marker-{round}");
        let answered = sns(
            &router,
            "Publish",
            &[("TopicArn", &topic), ("Message", &marker)],
        )
        .await;
        assert!(
            !element(&answered, "MessageId").is_empty(),
            "the publish must be answered with a MessageId, got {answered}"
        );
        published.insert(marker, 1);
    }

    let forwarded = collected(&router, &queue_url, PUBLISHED).await;
    let mut delivered: BTreeMap<String, usize> = BTreeMap::new();
    for body in &forwarded {
        *delivered.entry(body.clone()).or_default() += 1;
    }
    assert_eq!(
        delivered, published,
        "the deployed function must be invoked once per published message, each carrying one \
         of them: expected {published:?}, got {delivered:?}"
    );
}
