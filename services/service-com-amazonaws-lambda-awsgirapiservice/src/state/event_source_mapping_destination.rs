// SPDX-License-Identifier: CC0-1.0

//! WHICH destinations an event source mapping may hand a record of a failed batch to.
//!
//! `DestinationConfig` is "a configuration object that specifies the destination of an event
//! after Lambda processes it", and the destination inside it is a resource Lambda WRITES to:
//! `OnFailure$Destination` is "the Amazon Resource Name (ARN) of the destination resource",
//! and the resources it may name are enumerated — "you can configure an Amazon SNS topic,
//! Amazon SQS queue, Amazon S3 bucket, or Kafka topic as the destination". A value of any
//! other service is a resource Lambda has no delivery to make to, so a mapping stored with
//! one would report a dead-letter path that nothing could ever write a dropped batch to.
//!
//! So the two calls that state a mapping's destinations ([`crate::ops::create_event_source_mapping`]
//! and [`crate::ops::update_event_source_mapping`]) hold every destination a request names
//! to that enumeration, and refuse one it does not name with the modelled
//! `InvalidParameterValueException` before anything is stored or changed.
//!
//! A READ of the request alone: it decides, and changes nothing.

use crate::ops::types::DestinationConfig;

/// The ARN vendor fields of the destination services an event source mapping delivers a
/// record of a failed batch to, as [`service_common::arn::vendor`] reads one.
///
/// One entry per kind `OnFailure$Destination` enumerates for the sources a mapping's
/// destination configuration applies to: "you can configure an Amazon SNS topic, Amazon SQS
/// queue, Amazon S3 bucket, or Kafka topic as the destination" — the Kafka topic being the
/// one of the four that is not an ARN at all (see [`KAFKA_TOPIC_SCHEME`]).
const MAPPING_DESTINATION_VENDORS: &[&str] = &["sns", "sqs", "s3"];

/// How a Kafka topic destination is addressed: the one non-ARN form `DestinationArn`'s own
/// pattern admits (`^$|kafka://([^.]([a-zA-Z0-9\-_.]{0,248}))|arn:…`).
const KAFKA_TOPIC_SCHEME: &str = "kafka://";

/// Whether `destination` names one of the resources a mapping's destination may be (see
/// [`MAPPING_DESTINATION_VENDORS`]).
///
/// A value that is no ARN and no Kafka topic URI names no resource of any service, so it is
/// not one of them either.
#[must_use]
pub fn is_supported_mapping_destination(destination: &str) -> bool {
    if destination.starts_with(KAFKA_TOPIC_SCHEME) {
        return true;
    }
    service_common::arn::vendor(destination)
        .is_some_and(|vendor| MAPPING_DESTINATION_VENDORS.contains(&vendor))
}

/// The sentence a request whose `DestinationConfig` names a destination of a service a
/// mapping delivers nothing to is refused with.
///
/// Entirely FIXED text — it names neither the destination nor the kinds that ARE delivered
/// to, so a caller compares it exactly — and it states the one thing the value got wrong:
/// the SERVICE the destination belongs to is not one a mapping delivers to.
pub const UNSUPPORTED_MAPPING_DESTINATION_MESSAGE: &str =
    "The service specified by the TargetArn isn't supported as a destination.";

/// The refusal for a request whose `DestinationConfig` names a destination of a service a
/// mapping delivers nothing to — and `None` when every destination it names is one of them,
/// which includes a configuration naming none at all.
#[must_use]
pub fn unsupported_mapping_destination(
    destination_config: Option<&DestinationConfig>,
) -> Option<String> {
    // The destinations a request STATES, in the one reading of a `DestinationConfig` this
    // crate has — which holds the empty string the member's pattern admits as no destination
    // at all, so taking a destination away is not refused as a destination of nothing.
    super::requested_destinations(destination_config)
        .iter()
        .any(|destination| !is_supported_mapping_destination(destination))
        .then(|| UNSUPPORTED_MAPPING_DESTINATION_MESSAGE.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::{OnFailure, OnSuccess};

    /// A destination is judged on the SERVICE it names: each kind the member enumerates is
    /// accepted whatever resource of that service it addresses, and a resource of any other
    /// service — or a value that is no resource at all — is not.
    #[test]
    fn only_the_enumerated_destination_kinds_are_supported() {
        // (case, the destination, whether a mapping may deliver to it)
        let rows: [(&str, &str, bool); 12] = [
            (
                "an Amazon SNS topic",
                "arn:aws:sns:us-east-1:123456789012:failed-batches",
                true,
            ),
            (
                "an Amazon SQS queue",
                "arn:aws:sqs:eu-west-1:123456789012:order-dlq",
                true,
            ),
            ("an Amazon S3 bucket", "arn:aws:s3:::failed-batches", true),
            ("a Kafka topic", "kafka://failed-batches", true),
            (
                "an Amazon DynamoDB table",
                "arn:aws:dynamodb:us-east-1:123456789012:table/orders",
                false,
            ),
            (
                "an Amazon Kinesis data stream",
                "arn:aws:kinesis:us-east-1:123456789012:stream/orders",
                false,
            ),
            (
                "a Lambda function",
                "arn:aws:lambda:us-east-1:123456789012:function:on-failure",
                false,
            ),
            (
                "an Amazon EventBridge event bus",
                "arn:aws:events:us-east-1:123456789012:event-bus/failures",
                false,
            ),
            (
                "an Amazon CloudWatch Logs log group",
                "arn:aws:logs:us-east-1:123456789012:log-group:/aws/lambda/failures",
                false,
            ),
            (
                "an ARN of a partition of its own",
                "arn:aws-us-gov:sqs:us-gov-west-1:123456789012:order-dlq",
                true,
            ),
            ("a name that is no ARN at all", "order-dlq", false),
            ("a URI of another scheme", "https://example.org/", false),
        ];

        for (case, destination, supported) in rows {
            let decided = is_supported_mapping_destination(destination);
            assert_eq!(
                decided, supported,
                "case {case:?}: expected is_supported_mapping_destination({destination:?}) to be \
                 {supported:?}, got {decided:?}"
            );
        }
    }

    /// The refusal is over the configuration as a whole: a configuration naming a
    /// destination no mapping delivers to is refused with the one fixed sentence, and one
    /// naming only supported destinations — or none — is refused with nothing.
    #[test]
    fn a_configuration_is_refused_for_the_destination_no_mapping_delivers_to() {
        let topic = "arn:aws:sns:us-east-1:123456789012:failed-batches";
        let table = "arn:aws:dynamodb:us-east-1:123456789012:table/orders";

        // (case, the configuration, whether it is refused)
        let rows: [(&str, Option<DestinationConfig>, bool); 6] = [
            ("no configuration at all", None, false),
            (
                "a configuration naming no slot",
                Some(DestinationConfig {
                    on_success: None,
                    on_failure: None,
                }),
                false,
            ),
            (
                "an on-failure slot naming a supported destination",
                Some(destination_config(None, Some(topic))),
                false,
            ),
            (
                "an on-failure slot cleared with the empty destination the pattern admits",
                Some(destination_config(None, Some(""))),
                false,
            ),
            (
                "an on-failure slot naming a destination of another service",
                Some(destination_config(None, Some(table))),
                true,
            ),
            (
                "an on-success slot naming a destination of another service",
                Some(destination_config(Some(table), Some(topic))),
                true,
            ),
        ];

        for (case, config, refused) in rows {
            let message = unsupported_mapping_destination(config.as_ref());
            let expected =
                refused.then(|| UNSUPPORTED_MAPPING_DESTINATION_MESSAGE.to_string());
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    /// A `DestinationConfig` stating the two slots, each with the destination given.
    fn destination_config(
        on_success: Option<&str>,
        on_failure: Option<&str>,
    ) -> DestinationConfig {
        DestinationConfig {
            on_success: on_success.map(|destination| OnSuccess {
                destination: Some(destination.to_string()),
            }),
            on_failure: on_failure.map(|destination| OnFailure {
                destination: Some(destination.to_string()),
            }),
        }
    }
}
