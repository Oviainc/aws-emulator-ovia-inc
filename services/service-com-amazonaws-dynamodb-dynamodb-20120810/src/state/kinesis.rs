// SPDX-License-Identifier: CC0-1.0

//! The Kinesis seam: what a table's streaming destination resolves to in the KINESIS
//! service's own state, and the refusal a command against that destination earns when it
//! asks for the configuration the destination already replicates under.
//!
//! Beside the other destination refusals in [`super::table_settings`]: these are about the
//! COMMAND, and about the stream it names, rather than about the record's lifecycle
//! status, so they are stated here and re-exported through [`super`] like the rest.

use super::*;

/// How the command that CHANGES a destination's configuration is refused when it asks
/// for the precision the destination ALREADY replicates under: the change is a no-op,
/// so the service states that nothing is left to do rather than reporting a change it
/// did not make.
///
/// The destination is named by the TABLE ID and the Kinesis stream ARN (`kdsArn`) — the
/// identity the stored record carries, read from this service's own state rather than
/// echoed from the request — and the precision is the one the request asked for.
pub fn kinesis_precision_unchanged(
    table_id: &str,
    stream_arn: &str,
    precision: &str,
) -> RequestError {
    RequestError::Validation(format!(
        "Invalid Request: Precision is already set to the desired value of {precision} for \
         tableId: {table_id}, kdsArn: {stream_arn}"
    ))
}

/// Whether the Kinesis data stream `stream_arn` names EXISTS in `region`.
///
/// A destination is a Kinesis data stream, and the streams are the KINESIS service's own
/// resources, so whether one exists is answered from that service's store on the shared
/// engine — the same place its `DescribeStream` answers from — rather than from a second
/// copy of the registry kept here. A replication that cannot be completed because there is
/// no such stream is what `DestinationStatus` has `ENABLE_FAILED` for
/// ([`super::KinesisDestination::enable_failing`]).
///
/// `false` when the stream is not there, and when the Kinesis store cannot be reached at
/// all (the engine is not the shared in-memory one): either way nothing resolves the ARN,
/// so nothing can replicate to it.
///
/// LOCKING: this takes the KINESIS store's lock, and the caller must NOT hold this
/// service's own while it does — the one lock order this seam uses, as
/// [`super::register_table_stream`] describes for the streams registry. The work under
/// that lock is one walk over the region's streams, bounded by the registry's own quota.
pub fn kinesis_stream_exists<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    stream_arn: &str,
    at: SystemTime,
) -> bool {
    use ::service_com_amazonaws_kinesis_kinesis_20131202 as kinesis;

    kinesis::state::service_state(engine).is_some_and(|store| {
        store
            .lock()
            .stream_by_arn(at, region, stream_arn)
            .is_some()
    })
}

/// The Kinesis data stream a `StreamArn` resolves to for a table of `region`: its name, or
/// the reason an enable naming it cannot be completed.
///
/// `StreamArn` is modelled as a length-bounded string, so the API ACCEPTS every value that
/// clears that bound; this is what the value then resolves to, and each way it resolves to
/// no stream is a distinct [`KinesisEnableFailure`] the destination describes itself with:
/// the value is no stream ARN, it names a stream of another Region — a table replicates
/// only to a stream of its own Region — or this Region's endpoint holds no stream under it.
///
/// LOCKING: as [`kinesis_stream_exists`], whose lookup this is stated over.
pub fn resolve_kinesis_stream<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    stream_arn: &str,
    at: SystemTime,
) -> Result<String, KinesisEnableFailure> {
    let Some((stream_region, name)) = parse_kinesis_stream_arn(stream_arn) else {
        return Err(KinesisEnableFailure::NotAStreamArn);
    };
    if stream_region != region {
        return Err(KinesisEnableFailure::AnotherRegion(stream_region));
    }
    if !kinesis_stream_exists(engine, region, stream_arn, at) {
        return Err(KinesisEnableFailure::NoSuchStream);
    }
    Ok(name)
}
