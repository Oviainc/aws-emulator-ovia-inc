// SPDX-License-Identifier: CC0-1.0

//! The DATA plane of a table's Kinesis streaming destination: the change records a landed
//! write delivers to the Kinesis data stream the destination names.
//!
//! The control plane's half is [`super::kinesis`] (what a `StreamArn` resolves to) and
//! [`super::table_settings`] (the destination record and its lifecycle). This file is the
//! other half: "starts table data replication to the specified Kinesis data stream" (the
//! `EnableKinesisStreamingDestination` doc) means the item-level changes a write makes are
//! DELIVERED, as Kinesis data records, to a stream the Kinesis service owns — so they are
//! appended THERE, where that service's own `GetRecords` reads them, exactly as the
//! DynamoDB Streams records are appended in the streams registry
//! ([`super::publish_item_changes`]).
//!
//! One record carries one item change, as the JSON document DynamoDB publishes for it: the
//! modification, the item's key, the images, and the instant the change was made —
//! `ApproximateCreationDateTime`, stated in the UNIT the destination's
//! `ApproximateCreationDateTimePrecision` names, which is the whole point of that setting.

use super::*;

/// `recordFormat`: the encoding of the document one record carries. DynamoDB publishes its
/// change records as JSON, which is also what the images are rendered in
/// ([`super::item_json`], the same rendering an export writes).
const RECORD_FORMAT: &str = "application/json";

/// `eventSource`: which service published the record.
const EVENT_SOURCE: &str = "aws:dynamodb";

/// `eventName`: the modification the record describes, derived from the images the write
/// queued exactly as the DynamoDB Streams record's `OperationType` is — an item that was
/// not there before is an insert, one that was and still is a modification, and one that is
/// gone a removal.
const EVENT_INSERT: &str = "INSERT";
const EVENT_MODIFY: &str = "MODIFY";
const EVENT_REMOVE: &str = "REMOVE";

/// Deliver every item change `changes` holds to the Kinesis data stream of each of its
/// table's delivering destinations.
///
/// A change is delivered once per destination that is taking changes at `at`
/// ([`Table::delivering_kinesis_destinations`]), read off the table the change was made on,
/// so a table whose registration has not settled, has failed, or has been stopped delivers
/// nothing.
///
/// Delivery cannot fail the write that made the change — the item is already stored and the
/// response is already decided, and DynamoDB replicates to the stream asynchronously — so a
/// record the Kinesis store will not take is LOGGED rather than turned into an error for the
/// caller.
///
/// LOCKING: this takes the KINESIS store's lock while the caller holds this service's own,
/// which is the one order this emulator uses (the Kinesis service never reaches into the
/// DynamoDB store). The work is bounded by the queue — one request's writes
/// ([`MAX_PENDING_STREAM_RECORDS`]) — times the destinations one table keeps
/// ([`MAX_KINESIS_DESTINATIONS_PER_TABLE`]).
pub fn deliver_item_changes<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &ComAmazonawsDynamodbDynamodb20120810State,
    changes: &[PendingStreamRecord],
    at: SystemTime,
) {
    use ::service_com_amazonaws_kinesis_kinesis_20131202 as kinesis;

    // The destinations are read BEFORE the Kinesis store is locked, so this service's own
    // state is never read under both locks.
    let deliveries: Vec<(&PendingStreamRecord, String, String)> = changes
        .iter()
        .flat_map(|change| {
            store
                .table(&change.region, &change.table_name)
                .into_iter()
                .flat_map(move |table| {
                    table
                        .delivering_kinesis_destinations(at)
                        .map(move |destination| {
                            (
                                change,
                                destination.stream_arn.clone(),
                                destination.precision_in_force().to_string(),
                            )
                        })
                })
        })
        .collect();
    if deliveries.is_empty() {
        return;
    }
    let Some(registry) = kinesis::state::service_state(engine) else {
        ::tracing::error!(
            deliveries = deliveries.len(),
            "the Kinesis store is unreachable, so item changes reached no streaming destination"
        );
        return;
    };
    let mut registry = registry.lock();
    for (nth, (change, stream_arn, precision)) in deliveries.into_iter().enumerate() {
        let data = record_document(change, &precision, at).into_bytes();
        // "All data records with the same partition key map to the same shard"
        // (`PutRecordInput$PartitionKey`): the key is derived from the ITEM the change is
        // about, so one item's changes are delivered in order through one shard whatever
        // the stream's shard map looks like.
        let partition_key = item_partition_key(&change.keys);
        let Ok(stream) = registry.resolve_stream_mut(at, &change.region, None, Some(&stream_arn))
        else {
            ::tracing::error!(
                stream_arn = %stream_arn,
                "a change could not be delivered: the destination's stream is no longer held"
            );
            continue;
        };
        if !stream.serves_shards(at) {
            ::tracing::error!(
                stream_arn = %stream_arn,
                status = stream.status(at),
                "a change could not be delivered: the destination's stream serves no shards"
            );
            continue;
        }
        if data.len() + partition_key.len() > stream.max_record_size_bytes() {
            ::tracing::error!(
                stream_arn = %stream_arn,
                record_bytes = data.len(),
                "a change could not be delivered: the record is larger than the stream takes"
            );
            continue;
        }
        // `nth` spreads one request's records over the stream's open shards the way one
        // `PutRecords` call's do, for a stream that distributes its records itself.
        match stream.placement(None, Some(&partition_key), nth as u64) {
            Ok(index) => {
                // The shard map answered this index a moment ago, under the same borrow, so
                // nothing reaches the `None`; it is reported rather than passing for a
                // record that was stored.
                if stream.append_at(index, Some(partition_key), data, at).is_none() {
                    ::tracing::error!(
                        stream_arn = %stream_arn,
                        "a change could not be delivered: the shard it was placed in is gone"
                    );
                }
            }
            Err(rejected) => ::tracing::error!(
                stream_arn = %stream_arn,
                reason = ?rejected,
                "a change could not be delivered: the stream placed no record for it"
            ),
        }
    }
}

/// The document one delivered record carries: the change, the table it was made on and the
/// instant it was made, as DynamoDB publishes them to a Kinesis data stream.
///
/// `ApproximateCreationDateTime` is the instant of the change, stated in the unit
/// `precision` names — "the precision of the Kinesis data stream timestamp. The values are
/// either MILLISECOND or MICROSECOND"
/// (`KinesisDataStreamDestination$ApproximateCreationDateTimePrecision`) — so a consumer
/// reads one unit per destination, and the member that names the unit travels with it so
/// the number is never ambiguous.
///
/// The images are the ones the write queued: an insert carries the new item, a removal the
/// old one, and a modification both. `SizeBytes` is the size of the item the change left
/// behind, measured the way every other size this store reports is ([`item_size`]).
fn record_document(change: &PendingStreamRecord, precision: &str, at: SystemTime) -> String {
    let mut modification = ::serde_json::Map::new();
    modification.insert(
        "ApproximateCreationDateTime".to_string(),
        ::serde_json::json!(creation_time(at, precision)),
    );
    modification.insert(
        "ApproximateCreationDateTimePrecision".to_string(),
        ::serde_json::json!(precision),
    );
    modification.insert("Keys".to_string(), item_json(&change.keys));
    if let Some(new_image) = change.new_image.as_ref() {
        modification.insert("NewImage".to_string(), item_json(new_image));
    }
    if let Some(old_image) = change.old_image.as_ref() {
        modification.insert("OldImage".to_string(), item_json(old_image));
    }
    modification.insert(
        "SizeBytes".to_string(),
        ::serde_json::json!(item_size(
            change
                .new_image
                .as_ref()
                .or(change.old_image.as_ref())
                .unwrap_or(&change.keys)
        )),
    );
    ::serde_json::json!({
        "awsRegion": change.region,
        // Every record identifies itself, the way a DynamoDB Streams record's `eventID`
        // does: a fresh id per record, minted the way this store mints every other.
        "eventID": ::uuid::Uuid::new_v4().to_string(),
        "eventName": event_name(change),
        // "The user identity of the record" describes a change DynamoDB itself made (an
        // expiring item's deletion is the one it publishes); a change a caller's write made
        // has none.
        "userIdentity": ::serde_json::Value::Null,
        "recordFormat": RECORD_FORMAT,
        "tableName": change.table_name,
        "dynamodb": ::serde_json::Value::Object(modification),
        "eventSource": EVENT_SOURCE,
    })
    .to_string()
}

/// Which modification the change describes, from the images it carries.
fn event_name(change: &PendingStreamRecord) -> &'static str {
    match (&change.old_image, &change.new_image) {
        (None, Some(_)) => EVENT_INSERT,
        (Some(_), Some(_)) => EVENT_MODIFY,
        _ => EVENT_REMOVE,
    }
}

/// The instant `at`, in the unit `precision` names: whole milliseconds for the enum's
/// `MILLISECOND` and whole microseconds for its `MICROSECOND`.
///
/// An instant before the epoch states no epoch time at all, so it counts as the epoch
/// rather than as a negative creation time.
fn creation_time(at: SystemTime, precision: &str) -> u128 {
    let since_epoch = at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    if precision == DESTINATION_PRECISION_MICROSECOND {
        since_epoch.as_micros()
    } else {
        since_epoch.as_millis()
    }
}

/// The partition key the records of ONE item are delivered under: a digest of the item's
/// own primary key, as the key renders in DynamoDB JSON.
///
/// A digest rather than the key itself, because `PartitionKey` is modelled 1..=256
/// characters while a DynamoDB key is not bounded by that, and because the key may carry
/// any bytes a `B` attribute holds. It is derived from the key alone, so every change to one
/// item is delivered under the same partition key and lands in the same shard, which is
/// what keeps that item's changes in order for a consumer.
fn item_partition_key(keys: &Item) -> String {
    use ::sha2::Digest;

    let rendered = item_json(keys).to_string();
    ::hex::encode(::sha2::Sha256::digest(rendered.as_bytes()))
}
