// SPDX-License-Identifier: CC0-1.0

//! How an event source mapping reads an Amazon DYNAMODB table's STREAM: how that stream's
//! shards are listed, how one batch is taken off a shard at the position the mapping
//! carries, and what one record is handed to the function as.
//!
//! Everything a mapping does with the batch once it has it — metering its age, invoking
//! the function, checkpointing past what was processed — is the one rule for every shard
//! source and lives in [`super`]. This module is only the DynamoDB Streams half of the two
//! questions that differ per source, the other half being [`super::kinesis_stream`]:
//!
//! * THE SHARDS ARE LISTED BY DESCRIBING THE STREAM. A DynamoDB stream reports its shards
//!   as part of its description ("The shards that comprise the stream",
//!   `StreamDescription$Shards`), paged by `ExclusiveStartShardId` until the description
//!   reports no `LastEvaluatedShardId` — unlike a Kinesis data stream, which lists its
//!   shards through a call of their own.
//! * A POSITION STATED AS AN INSTANT IS READ AT THE SECOND IT FALLS IN. This source stamps
//!   a record's creation instant in whole Unix seconds
//!   (`StreamRecord$ApproximateCreationDateTime`), so a position stated any finer is
//!   floored to that second rather than placing the reader past every record stamped inside
//!   it (see [`shard_iterator`]). The same stamp is the only arrival instant this source
//!   reports, so the `IteratorAge` the poller meters for a batch of these records is that
//!   second's worth coarse — up to one second older than the record really is — rather than
//!   accurate to the millisecond.
//! * A RECORD IS HANDED OVER AS A STREAM RECORD. The envelope carries the change itself —
//!   which mutation it was and the item's keys and the images the stream's view type
//!   captures — rather than an opaque payload, and is rendered by the one reading of a
//!   DynamoDB stream record in this repository
//!   ([`::service_common::dynamodb_stream_event`]), so a consumer of this emulator's
//!   streams is handed the same document wherever it reads them.
//!
//! The stream is read through the DynamoDB Streams surface of this same emulator
//! (`DescribeStream`, `GetShardIterator`, `GetRecords`) over the in-process callback, so a
//! record is consumed only through the data plane a client would consume it through.

use super::{Clients, ShardRecord, ShardSourcedMapping, MAX_SHARD_PAGES};
use crate::state::ShardCursor;

/// Every shard of the mapped stream, in the order the stream describes them.
///
/// `DescribeStream` pages its shards, so the pages are followed — bounded by
/// [`MAX_SHARD_PAGES`], because the count is the table's and not this pass's.
pub(super) async fn shard_ids(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &ShardSourcedMapping,
) -> Result<Vec<String>, String> {
    let mut shards: Vec<String> = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..MAX_SHARD_PAGES {
        let mut request = clients
            .dynamodbstreams(callback, &run.region)
            .describe_stream()
            .stream_arn(&run.stream_arn);
        if let Some(after) = after.as_deref() {
            request = request.exclusive_start_shard_id(after);
        }
        let answer = request.send().await.map_err(|error| {
            format!(
                "the shards of {} could not be listed ({})",
                run.stream_arn,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
        let Some(description) = answer.stream_description else {
            break;
        };
        shards.extend(
            description
                .shards
                .unwrap_or_default()
                .into_iter()
                .filter_map(|shard| shard.shard_id),
        );
        // "If LastEvaluatedShardId is empty, then the 'last page' of results has been
        // processed and there is currently no more data to be retrieved."
        after = description.last_evaluated_shard_id;
        if after.is_none() {
            break;
        }
    }
    Ok(shards)
}

/// One batch off `shard` at `cursor`, each record already rendered as the envelope Lambda
/// hands a DynamoDB stream record over as; `None` for a shard with no position left to
/// read from.
pub(super) async fn read_batch(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &ShardSourcedMapping,
    shard: &str,
    cursor: &ShardCursor,
) -> Result<Option<Vec<ShardRecord>>, String> {
    let Some(iterator) = shard_iterator(callback, clients, run, shard, cursor).await? else {
        return Ok(None);
    };
    let records = records_at(callback, clients, run, &iterator).await?;
    let mut batch: Vec<ShardRecord> = Vec::with_capacity(records.len());
    for record in &records {
        let body = record.dynamodb.as_ref();
        // The sequence number is how the shard's position is kept and how a report of this
        // source names one record, so a record carrying none cannot be delivered and
        // checkpointed past: the batch fails and the records stay in the shard for a later
        // pass rather than the mapping reading on past a record it cannot place.
        let Some(sequence_number) = body.and_then(|body| body.sequence_number.clone()) else {
            return Err(format!(
                "a record of {shard} of {} carries no sequence number to read on from",
                run.stream_arn
            ));
        };
        batch.push(ShardRecord {
            sequence_number,
            arrived_at: body.and_then(|body| body.approximate_creation_date_time),
            envelope: ::service_common::dynamodb_stream_event::record_envelope(
                record,
                &run.stream_arn,
                &run.region,
            )?,
        });
    }
    Ok(Some(batch))
}

/// The iterator that reads `shard` from `cursor`, or `None` for a shard that has no
/// position left to read from.
///
/// A position stated as an INSTANT is floored to the second it falls in, because that is
/// the resolution this source stamps a record's creation instant at
/// (`StreamRecord$ApproximateCreationDateTime` is reported in whole Unix seconds): a
/// position stated any finer would place the reader PAST every record stamped inside that
/// same second, so the mapping would skip records it was asked to read. Flooring reads from
/// the start of that second instead, which can only hand over a record older than the
/// instant by less than the stamp's own resolution — and never loses one.
async fn shard_iterator(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &ShardSourcedMapping,
    shard: &str,
    cursor: &ShardCursor,
) -> Result<Option<String>, String> {
    use ::aws_sdk_dynamodbstreams::types::ShardIteratorType;

    let request = clients
        .dynamodbstreams(callback, &run.region)
        .get_shard_iterator()
        .stream_arn(&run.stream_arn)
        .shard_id(shard);
    let request = match cursor {
        ShardCursor::TrimHorizon => request.shard_iterator_type(ShardIteratorType::TrimHorizon),
        ShardCursor::AtTimestamp(instant) => request
            .shard_iterator_type(ShardIteratorType::AtTimestamp)
            .timestamp(floored_to_the_second(*instant)),
        ShardCursor::AfterSequenceNumber(sequence_number) => request
            .shard_iterator_type(ShardIteratorType::AfterSequenceNumber)
            .sequence_number(sequence_number),
    };
    let answer = request.send().await.map_err(|error| {
        format!(
            "no iterator could be taken on {shard} of {} ({})",
            run.stream_arn,
            ::aws_smithy_types::error::display::DisplayErrorContext(&error)
        )
    })?;
    Ok(answer.shard_iterator)
}

/// `instant` at the start of the whole Unix second it falls in — the resolution a record of
/// this source carries its creation instant at (see [`shard_iterator`]).
fn floored_to_the_second(instant: ::std::time::SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs(::aws_smithy_types::DateTime::from(instant).secs())
}

/// The records one batch holds: what the shard has at `iterator`, up to the mapping's
/// batch size.
async fn records_at(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &ShardSourcedMapping,
    iterator: &str,
) -> Result<Vec<::aws_sdk_dynamodbstreams::types::Record>, String> {
    let answer = clients
        .dynamodbstreams(callback, &run.region)
        .get_records()
        .shard_iterator(iterator)
        .limit(run.batch_size)
        .send()
        .await
        .map_err(|error| {
            format!(
                "a batch could not be read from {} ({})",
                run.stream_arn,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    Ok(answer.records.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::floored_to_the_second;

    /// RULE: a position stated as an instant is read at the start of the whole Unix second
    /// it falls in, which is the resolution this source stamps its records at.
    ///
    /// The rows differ along what the rule generalises over: where inside the second the
    /// instant falls (its start, just after it, the middle, and the last moment before the
    /// next one).
    #[test]
    fn a_position_stated_as_an_instant_is_read_at_the_second_it_falls_in() {
        let second = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        // (case, the instant, the whole second it must be read at)
        let rows: Vec<(&str, SystemTime, i64)> = vec![
            ("the-start-of-a-second", second, 1_800_000_000),
            (
                "a-millisecond-in",
                second + Duration::from_millis(1),
                1_800_000_000,
            ),
            (
                "halfway-through",
                second + Duration::from_millis(500),
                1_800_000_000,
            ),
            (
                "the-last-moment-of-it",
                second + Duration::from_nanos(999_999_999),
                1_800_000_000,
            ),
            (
                "the-next-second",
                second + Duration::from_secs(1),
                1_800_000_001,
            ),
        ];
        for (case, instant, expected) in rows {
            let floored = floored_to_the_second(instant);
            assert_eq!(
                (floored.secs(), floored.subsec_nanos()),
                (expected, 0),
                "case {case:?}: expected the whole second {expected}, got {floored:?}"
            );
        }
    }
}
