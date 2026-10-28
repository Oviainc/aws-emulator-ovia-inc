// SPDX-License-Identifier: CC0-1.0

//! Fan-out of ingested log events to a log group's subscription-filter destinations.
//!
//! `PutSubscriptionFilter` configures "a real-time stream of log events ingested through
//! PutLogEvents" and has them "delivered to a specific destination", and it states what
//! the receiving service is handed: "When log events are sent to the receiving service,
//! they are Base64 encoded and compressed with the GZIP format." So an event a
//! `PutLogEvents` batch STORES is offered to every subscription filter of its log group,
//! the filter's own `filterPattern` decides which of the batch's events it carries, and
//! the ones it carries travel as gzip-framed JSON — the CloudWatch Logs subscription
//! envelope, whose members are the owner account, the log group, the log stream, the
//! filter name that selected the events and the events themselves ([`envelope`]).
//!
//! The envelope carries each event's message exactly as it was INGESTED. A log group's
//! `kmsKeyId` names the key the group's own stored log data is encrypted with
//! (`AssociateKmsKey`: "Associates the specified KMS key with either one log group in the
//! account, or with all stored CloudWatch Logs query insights results in the account"); it
//! is not a property of any subscription destination and no operation copies it to one, so
//! a filter on a group that names a key delivers the same envelope as a filter on a group
//! that names none.
//!
//! The four destination kinds are the ones `PutSubscriptionFilter$destinationArn`
//! documents, and each takes the envelope the way its own API accepts a payload:
//!
//! * a Kinesis data stream — one record per envelope, the gzip frame as its data;
//! * a Firehose delivery stream — one record per envelope, likewise;
//! * a Lambda function — invoked with the `awslogs` document, which carries the gzip frame
//!   base64-encoded, this being the one destination whose payload states that encoding
//!   itself rather than leaving it to a blob member's wire form;
//! * a CloudWatch Logs logical destination created with `PutDestination` — "a logical
//!   object that represents the actual delivery destination", so it is resolved to the
//!   `targetArn` it names ("the physical target where the log events are delivered (for
//!   example, a Kinesis stream)") and the envelope is delivered THERE.
//!
//! Each destination service is reached the way any emulated service reaches another: its
//! AWS SDK client driven over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so nothing here opens a socket
//! or leaves the host and this crate names no other service crate.
//!
//! Delivery is best effort, as it is for the real service: `PutLogEvents` "always
//! accepted" the batch, and whether a destination took the envelope is not part of its
//! answer, so a destination that refuses or does not exist is logged and the ingestion
//! stands.
//!
//! What one ingestion can cost is bounded by what the batch and the group already bound: a
//! log group holds at most [`super::MAX_SUBSCRIPTION_FILTERS_PER_GROUP`] filters, so a
//! batch owes at most that many envelopes per record-sized run of its events, and each run
//! is capped at [`MAX_RECORD_BYTES`] of the batch's own bytes
//! ([`record_sized_chunks`]) — one request therefore makes a bounded number of bounded
//! calls.

use std::io::Write;

use super::filter_pattern::FilterPattern;
use super::{ComAmazonawsCloudwatchlogsLogs20140328State, StoredEvent};

/// `messageType` of an envelope carrying ingested log events, as the subscription payload
/// spells it. The other value a destination sees is a health check CloudWatch Logs sends
/// the destination itself, which no ingestion produces, so this is the only one here.
pub const MESSAGE_TYPE_DATA: &str = "DATA_MESSAGE";

/// The digits a delivered event's `id` carries. CloudWatch Logs identifies a log event by
/// an opaque 56-character decimal string; it is minted from the store's own ingestion
/// ordering ([`event_id`]) rather than from anything in the request, so it is unique,
/// stable for the life of the event and monotone in ingestion order.
const EVENT_ID_DIGITS: usize = 56;

/// The widest ingestion sequence an event id reserves room for, as a power of ten: the
/// sequence is the low-order block of [`event_id`] and the ingestion instant the high-order
/// one, so the two never collide.
const EVENT_ID_SEQ_SCALE: u128 = 10_u128.pow(20);

/// The largest payload one delivered record may carry, from the Kinesis
/// `PutRecordsRequestEntry$Data` quota: "The maximum size of the data blob (the payload
/// before base64-encoding) is up to 1 MiB." A batch whose events would not fit is
/// delivered as several envelopes ([`deliveries`]) rather than as one oversized record.
const MAX_RECORD_BYTES: usize = 1_048_576;

/// What one event costs an envelope beyond its message, in bytes: the `id`, the timestamp,
/// the three member names, the quoting and the separators. An over-estimate, so the
/// greedy split in [`deliveries`] never produces a frame past [`MAX_RECORD_BYTES`].
const EVENT_ENVELOPE_OVERHEAD_BYTES: usize = 128;

/// The ARN vendor of each destination kind, as `PutSubscriptionFilter$destinationArn`
/// names them, paired with the ARN resource TYPE that kind's resource carries.
const KINESIS_STREAM: (&str, &str) = ("kinesis", "stream");
const FIREHOSE_DELIVERY_STREAM: (&str, &str) = ("firehose", "deliverystream");
const LAMBDA_FUNCTION: (&str, &str) = ("lambda", "function");
const LOGS_DESTINATION: (&str, &str) = ("logs", "destination");

/// One envelope a batch of ingested events owes one subscription filter, resolved to the
/// PHYSICAL destination it is delivered to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    /// The filter that selected the events, for the warning a failed delivery logs.
    pub filter_name: String,
    /// The ARN of the resource the envelope is delivered to: the filter's own
    /// `destinationArn`, or — for a logical destination — the `targetArn` that destination
    /// names.
    pub destination_arn: String,
    /// The key the envelope is placed under where the destination partitions what it is
    /// handed. `PutSubscriptionFilter$distribution`: "By default, log data is grouped by
    /// log stream, but the grouping can be set to random for a more even distribution",
    /// so a `ByLogStream` filter keys every envelope on the log stream it came from and a
    /// `Random` one keys each envelope on a value unique to it.
    pub partition_key: String,
    /// The gzip-framed envelope.
    pub payload: Vec<u8>,
}

/// A delivered log event's id: 56 decimal digits ([`EVENT_ID_DIGITS`]) over the ingestion
/// instant and the ingestion sequence the store stamped the event with, so two events
/// never share one and the order of the ids is the order of ingestion.
pub fn event_id(event: &StoredEvent) -> String {
    let ingested = u128::try_from(event.ingestion_time).unwrap_or(0);
    let value = ingested
        .saturating_mul(EVENT_ID_SEQ_SCALE)
        .saturating_add(u128::from(event.ingestion_seq));
    format!("{value:0>EVENT_ID_DIGITS$}")
}

/// The subscription envelope for `events`, as the receiving service reads it.
pub fn envelope(
    owner: &str,
    log_group_name: &str,
    log_stream_name: &str,
    filter_name: &str,
    events: &[&StoredEvent],
) -> String {
    ::serde_json::json!({
        "messageType": MESSAGE_TYPE_DATA,
        "owner": owner,
        "logGroup": log_group_name,
        "logStream": log_stream_name,
        "subscriptionFilters": [filter_name],
        "logEvents": events
            .iter()
            .map(|event| ::serde_json::json!({
                "id": event_id(event),
                "timestamp": event.timestamp,
                "message": event.message,
            }))
            .collect::<Vec<::serde_json::Value>>(),
    })
    .to_string()
}

/// `body` in a gzip frame, which is the framing the subscription payload is compressed
/// with.
fn gzip(body: &str) -> Vec<u8> {
    let mut encoder = ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
    // Writing to a `Vec` cannot fail, and neither can finishing the frame over one.
    let _ = encoder.write_all(body.as_bytes());
    encoder.finish().unwrap_or_default()
}

/// Every envelope the events `PutLogEvents` just stored on `log_stream_name` owe the
/// subscription filters of `log_group_name`.
///
/// Read under the store's own guard, so the filters a batch is fanned out through are the
/// ones the group held when the batch arrived. A filter whose pattern matches none of the
/// batch's events owes nothing, and neither does a filter whose destination resolves to
/// nothing this emulator can deliver to.
pub fn deliveries(
    store: &ComAmazonawsCloudwatchlogsLogs20140328State,
    log_group_name: &str,
    log_stream_name: &str,
    events: &[StoredEvent],
) -> Vec<Delivery> {
    let Some(group) = store.log_groups.get(log_group_name) else {
        return Vec::new();
    };
    let owner = ::engine::emulator_account_id();
    let mut deliveries = Vec::new();
    for filter in group.subscription_filters.values() {
        // A stored filter's pattern was parsed when the put configured it, so one that
        // does not parse here selects nothing rather than refusing the ingestion.
        let Ok(pattern) = FilterPattern::parse(&filter.filter_pattern) else {
            continue;
        };
        let matched: Vec<&StoredEvent> = events
            .iter()
            .filter(|event| pattern.match_message(&event.message).is_some())
            .collect();
        if matched.is_empty() {
            continue;
        }
        let Some(destination_arn) = physical_destination(store, &filter.destination_arn) else {
            continue;
        };
        for chunk in record_sized_chunks(&matched) {
            let body = envelope(
                owner,
                log_group_name,
                log_stream_name,
                &filter.filter_name,
                chunk,
            );
            // The model's `Distribution` has two values, so a filter that is not grouped
            // by log stream is the one that spreads: each envelope's first event's id is
            // unique to that envelope, so no two of them share a key.
            let partition_key = if filter.distribution == super::DEFAULT_DISTRIBUTION {
                log_stream_name.to_string()
            } else {
                chunk
                    .first()
                    .map_or_else(|| log_stream_name.to_string(), |event| event_id(event))
            };
            deliveries.push(Delivery {
                filter_name: filter.filter_name.clone(),
                destination_arn: destination_arn.clone(),
                partition_key,
                payload: gzip(&body),
            });
        }
    }
    deliveries
}

/// `matched` split into runs small enough that each one's envelope fits a single delivered
/// record ([`MAX_RECORD_BYTES`]). A single event larger than that is still delivered on its
/// own — the destination refuses it rather than this emulator dropping it silently.
fn record_sized_chunks<'a, 'e>(matched: &'a [&'e StoredEvent]) -> Vec<&'a [&'e StoredEvent]> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut bytes = 0;
    for (index, event) in matched.iter().enumerate() {
        let cost = event.message.len() + EVENT_ENVELOPE_OVERHEAD_BYTES;
        if index > start && bytes + cost > MAX_RECORD_BYTES {
            chunks.push(&matched[start..index]);
            start = index;
            bytes = 0;
        }
        bytes += cost;
    }
    if start < matched.len() {
        chunks.push(&matched[start..]);
    }
    chunks
}

/// The ARN of the resource a `destinationArn` actually delivers to: the value itself for a
/// destination that IS a physical resource, and the `targetArn` of the stored logical
/// destination for one that names a `PutDestination` destination. `None` when the value
/// names a logical destination this account does not hold, or a resource of no kind this
/// operation delivers to.
fn physical_destination(
    store: &ComAmazonawsCloudwatchlogsLogs20140328State,
    destination_arn: &str,
) -> Option<String> {
    let parsed = ::service_common::arn::parse(destination_arn)?;
    if parsed.vendor == LOGS_DESTINATION.0 {
        let name = ::service_common::arn::resource_id_of_type(destination_arn, LOGS_DESTINATION.1)?;
        let target = store.destinations.get(name)?.target_arn.clone();
        // A logical destination's target is a physical resource of one of the streaming
        // vendors, so it is not followed further: a destination naming another destination
        // addresses nothing.
        return is_deliverable(&target).then_some(target);
    }
    is_deliverable(destination_arn).then(|| destination_arn.to_string())
}

/// Whether an ARN names a physical resource this module delivers an envelope to.
fn is_deliverable(arn: &str) -> bool {
    [KINESIS_STREAM, FIREHOSE_DELIVERY_STREAM, LAMBDA_FUNCTION]
        .iter()
        .any(|(vendor, resource_type)| {
            ::service_common::arn::parse(arn).is_some_and(|parsed| parsed.vendor == *vendor)
                && ::service_common::arn::resource_id_of_type(arn, resource_type)
                    .is_some_and(|id| !id.is_empty())
        })
}

/// Hand every envelope of `deliveries` to its destination.
///
/// `fallback_region` is the Region the ingesting request was signed for, used only for a
/// destination ARN that names no Region of its own.
///
/// Every delivery goes to a service of THIS emulator: the vendor chooses which of the
/// clients below is used, and the Region label is held to the grammar a Region is written
/// in ([`service_common::arn::region_label`]) before it is put in an endpoint, so no value
/// a request carried can name a host. The transport is the in-process callback either way.
pub async fn deliver(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    fallback_region: &str,
    deliveries: Vec<Delivery>,
) {
    let fallback_region = ::service_common::arn::region_label(fallback_region)
        .unwrap_or(super::DEFAULT_REGION)
        .to_string();
    for delivery in deliveries {
        let Some(parsed) = ::service_common::arn::parse(&delivery.destination_arn) else {
            continue;
        };
        let vendor = parsed.vendor.to_string();
        let region = ::service_common::arn::region_of(&delivery.destination_arn, &vendor)
            .unwrap_or(&fallback_region)
            .to_string();
        let failure = if vendor == KINESIS_STREAM.0 {
            deliver_to_kinesis(ctx, &region, &delivery).await
        } else if vendor == FIREHOSE_DELIVERY_STREAM.0 {
            deliver_to_firehose(ctx, &region, &delivery).await
        } else if vendor == LAMBDA_FUNCTION.0 {
            deliver_to_lambda(ctx, &region, &delivery).await
        } else {
            Some("the destination names no kind a subscription filter delivers to".to_string())
        };
        if let Some(failure) = failure {
            // The ingestion stands either way: `PutLogEvents` answered the batch, so a
            // destination that would not take the envelope is reported here rather than in
            // that answer.
            ::tracing::warn!(
                filter_name = %delivery.filter_name,
                destination_arn = %delivery.destination_arn,
                error = %failure,
                "a subscription filter's destination did not take the delivered log events"
            );
        }
    }
}

/// Build one destination service's client over the in-process callback, against that
/// service's endpoint in `region`.
///
/// The transport is in-process, so the client disables what only a network client needs:
/// retries, timeouts, stalled-stream protection and identity caching each want a sleep
/// implementation the callback transport is not given, and there is nothing behind it to
/// retry.
macro_rules! callback_client {
    ($sdk:ident, $ctx:expr, $endpoint:literal, $region:expr) => {{
        let config = ::$sdk::Config::builder()
            .behavior_version(::$sdk::config::BehaviorVersion::latest())
            .http_client($ctx.callback().http_client())
            .credentials_provider($ctx.callback().aws_credentials_provider())
            .region(::$sdk::config::Region::new($region.to_string()))
            .endpoint_url(format!("https://{}.{}.amazonaws.com", $endpoint, $region))
            .retry_config(::$sdk::config::retry::RetryConfig::disabled())
            .timeout_config(::$sdk::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::$sdk::config::IdentityCache::no_cache())
            .stalled_stream_protection(::$sdk::config::StalledStreamProtectionConfig::disabled())
            .build();
        ::$sdk::Client::from_conf(config)
    }};
}

/// How a failed delivery is worded in the warning: the SDK's own rendering of the answer
/// the destination service gave.
fn why(error: &impl std::error::Error) -> String {
    ::aws_smithy_types::error::display::DisplayErrorContext(error).to_string()
}

/// Deliver one envelope to a Kinesis data stream, as one record keyed on the delivery's
/// partition key.
async fn deliver_to_kinesis(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    delivery: &Delivery,
) -> Option<String> {
    let stream_name =
        ::service_common::arn::resource_id_of_type(&delivery.destination_arn, KINESIS_STREAM.1)?;
    let client = callback_client!(aws_sdk_kinesis, ctx, "kinesis", region);
    client
        .put_record()
        .stream_name(stream_name)
        .partition_key(&delivery.partition_key)
        .data(::aws_sdk_kinesis::primitives::Blob::new(
            delivery.payload.clone(),
        ))
        .send()
        .await
        .err()
        .map(|error| why(&error))
}

/// Deliver one envelope to a Firehose delivery stream, as one record.
async fn deliver_to_firehose(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    delivery: &Delivery,
) -> Option<String> {
    let stream_name = ::service_common::arn::resource_id_of_type(
        &delivery.destination_arn,
        FIREHOSE_DELIVERY_STREAM.1,
    )?;
    let client = callback_client!(aws_sdk_firehose, ctx, "firehose", region);
    let record = ::aws_sdk_firehose::types::Record::builder()
        .data(::aws_sdk_firehose::primitives::Blob::new(
            delivery.payload.clone(),
        ))
        .build();
    match record {
        Ok(record) => client
            .put_record()
            .delivery_stream_name(stream_name)
            .record(record)
            .send()
            .await
            .err()
            .map(|error| why(&error)),
        Err(error) => Some(why(&error)),
    }
}

/// Deliver one envelope to a Lambda function, as the `awslogs` document a subscription
/// filter's Lambda destination is invoked with.
///
/// The invocation is asynchronous (`InvocationType` `Event`): the function's answer is
/// nothing the ingesting request reports, so nothing waits for it.
async fn deliver_to_lambda(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    delivery: &Delivery,
) -> Option<String> {
    let function =
        ::service_common::arn::resource_id_of_type(&delivery.destination_arn, LAMBDA_FUNCTION.1)?;
    let payload = ::serde_json::json!({
        "awslogs": {"data": ::aws_smithy_types::base64::encode(&delivery.payload)},
    })
    .to_string();
    let client = callback_client!(aws_sdk_lambda, ctx, "lambda", region);
    client
        .invoke()
        .function_name(function)
        .invocation_type(::aws_sdk_lambda::types::InvocationType::Event)
        .payload(::aws_smithy_types::Blob::new(payload.into_bytes()))
        .send()
        .await
        .err()
        .map(|error| why(&error))
}

#[cfg(test)]
mod tests {
    use super::{event_id, record_sized_chunks, EVENT_ID_DIGITS, MAX_RECORD_BYTES};
    use crate::state::StoredEvent;

    fn stored(message: &str, ingestion_seq: u64) -> StoredEvent {
        StoredEvent {
            timestamp: 1_700_000_000_000,
            message: message.to_string(),
            ingestion_time: 1_700_000_000_123,
            ingestion_seq,
        }
    }

    /// Every id is 56 decimal digits, and two events ingested one after the other get
    /// different ids in ingestion order.
    #[test]
    fn an_event_id_is_a_56_digit_decimal_unique_to_the_event() {
        let first = event_id(&stored("a", 1));
        let second = event_id(&stored("a", 2));
        for (case, id) in [("first", &first), ("second", &second)] {
            assert_eq!(
                id.len(),
                EVENT_ID_DIGITS,
                "case {case}: an event id carries {EVENT_ID_DIGITS} digits, got {id}"
            );
            assert!(
                id.chars().all(char::is_numeric),
                "case {case}: an event id is decimal, got {id}"
            );
        }
        assert!(
            first < second,
            "case ordering: ids follow ingestion order, got {first} then {second}"
        );
    }

    /// A batch whose events would not fit one record is split, and a batch that fits is
    /// not.
    #[test]
    fn events_are_split_into_record_sized_runs() {
        let small: Vec<StoredEvent> = (0..4).map(|seq| stored("x", seq)).collect();
        let borrowed: Vec<&StoredEvent> = small.iter().collect();
        let chunks = record_sized_chunks(&borrowed);
        assert_eq!(
            chunks.len(),
            1,
            "case small: four tiny events fit one record, got {} runs",
            chunks.len()
        );

        let big_message = "y".repeat(MAX_RECORD_BYTES / 3);
        let big: Vec<StoredEvent> = (0..3).map(|seq| stored(&big_message, seq)).collect();
        let borrowed: Vec<&StoredEvent> = big.iter().collect();
        let chunks = record_sized_chunks(&borrowed);
        assert_eq!(
            chunks.len(),
            2,
            "case large: three third-of-a-record events need two runs, got {} runs",
            chunks.len()
        );
        assert_eq!(
            chunks.iter().map(|chunk| chunk.len()).sum::<usize>(),
            3,
            "case large: every event lands in exactly one run, got {chunks:?}"
        );
    }
}
