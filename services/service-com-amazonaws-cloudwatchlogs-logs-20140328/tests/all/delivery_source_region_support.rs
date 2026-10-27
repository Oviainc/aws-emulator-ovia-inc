// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WORDING of `PutDeliverySource`'s refusal of a log sender this
//! endpoint does not serve — which Regions that refusal covers, that a refused put stores
//! nothing, and that the served Region serves it, are the subject of
//! `tests/all/delivery_sources.rs`.
//!
//! The rule under test: that refusal's message is a function of the REQUEST alone — the one
//! sentence "<action> action is not supported for this LogType in region <region>", naming the
//! action taken and the Region the request reached, with the literal word for the log-type
//! parameter rather than its value. So one away Region yields the SAME sentence for every
//! sender, log type and partition: neither the resource, nor the Region the sender is reached
//! through, nor the constraint-violation prefix this service puts on a parameter that failed a
//! modelled constraint appears in it. That prefix belongs to the neighbouring refusal of a
//! `resourceArn` that is not an ARN at all, which keeps its own template.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a message
//! a handler does not actually answer cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: each global log
//! sender the `logType` catalogue names, in each partition that publishes one global endpoint,
//! with the log type that catalogue names for it, refused from each Region that is not the one
//! it is reached through — and, for the neighbouring refusal, three forms a value that is no
//! ARN takes.

use serde_json::json;

use crate::harness::{Logs, REGION};

/// The operation under test.
const PUT: &str = "PutDeliverySource";

/// The code this operation models for both refusals below.
const VALIDATION: &str = "ValidationException";

/// The prefix this service puts on a parameter that failed a MODELLED constraint — the one
/// thing the refusal under test must not borrow, since no modelled constraint is violated by
/// a well-formed ARN of a sender served elsewhere (`resourceArn` targets a free-form `Arn`:
/// `smithy_query(query="shape",shape="PutDeliverySourceRequest")`).
const CONSTRAINT_PREFIX: &str = "1 validation error detected";

/// One global log sender: the resource a put names, the single Region it is reached through,
/// and a `logType` its own catalogue names for it.
///
/// Source: the global senders the `logType` member names — "For CloudFront, the valid value is
/// ACCESS_LOGS", "For Route 53 Global Resolver, the valid value is GLOBAL_RESOLVER_LOGS", "For
/// Amazon Web Services Shield Advanced, the valid value is FLOW_LOGS"
/// (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`) — each with
/// the Region its partition publishes its one global endpoint in: the commercial partition's in
/// US East (N. Virginia), the China partitions' in Beijing, the AWS GovCloud (US) partition's
/// in US-Gov-West.
#[derive(Debug)]
struct Sender {
    /// What this row varies, for the assertion messages.
    label: &'static str,
    /// `resourceArn` — "The ARN of the Amazon Web Services resource that is generating and
    /// sending logs."
    resource_arn: &'static str,
    /// The one Region this sender is reached through, and so the only one a delivery source
    /// for it can be created in.
    home_region: &'static str,
    /// The `logType` the catalogue names for this sender.
    log_type: &'static str,
}

/// Every global sender, across every partition that publishes a global endpoint.
const SENDERS: [Sender; 7] = [
    Sender {
        label: "a CloudFront distribution",
        resource_arn: "arn:aws:cloudfront::123456789012:distribution/E1EXAMPLE1EXAMPLE",
        home_region: "us-east-1",
        log_type: "ACCESS_LOGS",
    },
    Sender {
        label: "a Route 53 hosted zone",
        resource_arn: "arn:aws:route53:::hostedzone/Z1EXAMPLE1EXAMP",
        home_region: "us-east-1",
        log_type: "GLOBAL_RESOLVER_LOGS",
    },
    Sender {
        label: "a Shield Advanced protection",
        resource_arn: "arn:aws:shield::123456789012:protection/abcd1234-1234-abcd-1234-abcd1234abcd",
        home_region: "us-east-1",
        log_type: "FLOW_LOGS",
    },
    Sender {
        label: "a CloudFront distribution of the China partition",
        resource_arn: "arn:aws-cn:cloudfront::123456789012:distribution/E2EXAMPLE2EXAMPLE",
        home_region: "cn-north-1",
        log_type: "ACCESS_LOGS",
    },
    Sender {
        label: "a Shield Advanced protection of the China partition",
        resource_arn:
            "arn:aws-cn:shield::123456789012:protection/1111aaaa-2222-bbbb-3333-cccc4444dddd",
        home_region: "cn-north-1",
        log_type: "FLOW_LOGS",
    },
    Sender {
        label: "a Route 53 hosted zone of the GovCloud partition",
        resource_arn: "arn:aws-us-gov:route53:::hostedzone/Z3EXAMPLE3EXAMP",
        home_region: "us-gov-west-1",
        log_type: "GLOBAL_RESOLVER_LOGS",
    },
    Sender {
        label: "a CloudFront distribution of the GovCloud partition",
        resource_arn: "arn:aws-us-gov:cloudfront::123456789012:distribution/E3EXAMPLE3EXAMPLE",
        home_region: "us-gov-west-1",
        log_type: "ACCESS_LOGS",
    },
];

/// The whole sentence a put of a sender served elsewhere is refused with, as the service must
/// word it: the action, then fixed text naming the log-type parameter, then the Region the
/// request reached.
fn expected_message(region: &str) -> String {
    format!("{PUT} action is not supported for this LogType in region {region}")
}

/// The refusal's message depends on the REQUEST and on nothing else: in one Region every
/// sender served elsewhere is refused with one and the same sentence, which carries neither
/// the resource, nor the Region that sender is reached through, nor the log type named, nor
/// the constraint-violation prefix.
#[tokio::test]
async fn the_refusal_message_names_only_the_action_and_the_requests_region() {
    let logs = Logs::new();

    for region in ["us-east-1", "cn-north-1", "us-gov-west-1", REGION, "us-west-2"] {
        let expected = expected_message(region);
        // The senders this Region does not serve: in it, each must be refused with `expected`
        // and nothing else, however its ARN, partition and log type differ.
        let served_elsewhere = SENDERS
            .iter()
            .enumerate()
            .filter(|(_index, sender)| sender.home_region != region);

        for (index, sender) in served_elsewhere {
            let case = sender.label;
            let body = json!({
                "name": format!("sender-{index}-{region}"),
                "resourceArn": sender.resource_arn,
                "logType": sender.log_type,
            });
            let (code, message) = logs.refusal_in_region(region, PUT, &body).await;
            assert_eq!(
                code, VALIDATION,
                "case {case:?} put in {region}: expected {VALIDATION}, got {code}"
            );
            assert_eq!(
                message, expected,
                "case {case:?} put in {region}: expected {expected:?}, got {message:?}"
            );
            // Asserted beside the equality so a regression says WHICH value leaked.
            for (what, leaked) in [
                ("the resource ARN", sender.resource_arn),
                ("the Region the sender is reached through", sender.home_region),
                ("the log type named", sender.log_type),
                ("the constraint-violation prefix", CONSTRAINT_PREFIX),
            ] {
                assert!(
                    !message.contains(leaked),
                    "case {case:?} put in {region}: the refusal must not carry {what} \
                     ({leaked}), got {message:?}"
                );
            }
        }
    }
}

/// The neighbouring refusal keeps its own template: a `resourceArn` that is not an ARN at all
/// DOES fail to parse as the member's type, so it is refused with this service's
/// constraint-violation sentence naming the member and the value — the prefix the refusal
/// above must not borrow.
#[tokio::test]
async fn a_resource_arn_that_is_no_arn_keeps_the_constraint_violation_template() {
    let logs = Logs::new();

    for (index, candidate) in [
        "distribution/E1EXAMPLE1EXAMPLE",
        "arn:aws:cloudfront",
        "arn-aws-cloudfront--123456789012-distribution/E1",
    ]
    .into_iter()
    .enumerate()
    {
        let body = json!({
            "name": format!("not-an-arn-{index}"),
            "resourceArn": candidate,
            "logType": "ACCESS_LOGS",
        });
        let (code, message) = logs.refusal(PUT, &body).await;
        assert_eq!(
            code, VALIDATION,
            "case {candidate:?}: expected {VALIDATION}, got {code}"
        );
        let expected = format!("{CONSTRAINT_PREFIX}: Unable to parse resourceArn {candidate}");
        assert_eq!(
            message, expected,
            "case {candidate:?}: expected {expected:?}, got {message:?}"
        );
    }
}
