// SPDX-License-Identifier: CC0-1.0

//! The DynamoDB Streams seam.
//!
//! DynamoDB Streams models no create or delete API of its own: "a stream comes into
//! existence" when a `StreamSpecification` on `CreateTable`/`UpdateTable` enables
//! one, and the STREAMS service owns the registry its own consumers (`ListStreams`,
//! `DescribeStream`, `GetShardIterator`, `GetRecords`) read. So enabling a stream
//! here has to register it THERE, on the same shared engine, rather than keeping a
//! second copy of the resource in this service's store: what this service keeps is
//! the table's latest stream identity, projected into every `TableDescription`.
//!
//! It lives in its own module, re-exported from [`crate::state`], so the seam is one
//! file to read rather than a section at the end of the store.

use std::time::SystemTime;

use crate::ops::types::{KeySchemaElement, StreamSpecification};
use crate::state::{RequestError, TableStream};

/// The `StreamViewType` the specification asks for, refused when the pair cannot
/// hold together.
///
/// `StreamViewType` "determines what information is written to the stream for this
/// table" (`StreamSpecification$StreamViewType`), so a specification that turns the
/// stream ON without naming one asks for a stream whose contents are undefined and
/// is refused; a specification that turns it OFF names nothing to capture, so the
/// member is not read at all.
pub fn stream_view_type(
    specification: &StreamSpecification,
) -> Result<Option<&str>, RequestError> {
    use ::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810 as streams;

    if !specification.stream_enabled {
        return Ok(None);
    }
    match specification.stream_view_type.as_deref() {
        // The member is an enum in the model, so the boundary validator has already
        // refused a value outside the four it declares. The registry parses the same
        // four, and a value it cannot parse is still a BAD REQUEST rather than a
        // server fault, so the two agree here instead of at the seam.
        Some(view_type) if streams::state::StreamViewType::from_wire(view_type).is_some() => {
            Ok(Some(view_type))
        }
        Some(view_type) => Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: StreamViewType {view_type} is not one of \
             {}",
            // Rendered from this service's own model value list, so the message and
            // the member's declared enum cannot drift.
            crate::ops::types::STREAM_VIEW_TYPE_VALUES.join(", ")
        ))),
        None => Err(RequestError::Validation(
            "One or more parameter values were invalid: StreamViewType is required when \
             StreamEnabled is true"
                .to_string(),
        )),
    }
}

/// Why a stream could not be registered for a table.
#[derive(Debug, Clone)]
pub enum StreamFault {
    /// The DynamoDB Streams store cannot be reached at all (the engine is not the
    /// shared in-memory one), which a handler answers with its modelled SERVER
    /// error rather than with a table whose stream nothing registered.
    StoreUnavailable,
    /// The streams registry refused the registration and said why: a quota of the
    /// registry, which the control plane answers with its own
    /// `LimitExceededException`.
    Rejected(String),
    /// An invariant of this seam did not hold — a server fault, worded for what
    /// actually went wrong rather than as an unreachable store.
    Internal(&'static str),
}

/// Register a stream for `table_name` in the DynamoDB Streams store and report the
/// table's new latest-stream identity.
///
/// This is the publishing seam: the stream, its shards and its records live in the
/// streams service's state, so the ARN and label reported here are the ones its own
/// `ListStreams`/`DescribeStream` answer with.
///
/// LOCKING: this takes the STREAMS store's lock, and a handler calls it while
/// holding this service's own. That order is the only one in the emulator — the
/// streams service never reaches into the DynamoDB store — so the two cannot
/// deadlock, and the work done under both is bounded by constants (one prefix range
/// over the table's own streams and at most `MAX_STREAMS_PER_TABLE` label probes),
/// never by the request or by state a caller has grown.
pub fn register_table_stream<Eng: Send + Sync + 'static>(
    engine: &Eng,
    table_name: &str,
    region: &str,
    key_schema: &[KeySchemaElement],
    view_type: &str,
    at: SystemTime,
) -> Result<TableStream, StreamFault> {
    use ::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810 as streams;

    // The view type is validated by `stream_view_type` before any of this, so a
    // value the registry cannot parse is a bad request that never reaches here.
    let captured = streams::state::StreamViewType::from_wire(view_type).ok_or(
        StreamFault::Internal("the stream view type was not validated before registration"),
    )?;
    let store = streams::state::service_state(engine).ok_or(StreamFault::StoreUnavailable)?;
    let mut store = store.lock();
    // Everything the clock already owes the registry first, so a stream past the
    // documented 24 hours of its disable is reclaimed before this registration is
    // counted against the registry's quotas.
    store.materialize_due_expiries(at);
    let arn = store
        .register_stream(
            &streams::state::StreamRegistration {
                table_name: table_name.to_string(),
                stream_view_type: captured,
                // The stream records a change by the table's own primary key, so the
                // registry is given the schema that key is read with.
                key_schema: key_schema
                    .iter()
                    .map(|element| streams::ops::types::KeySchemaElement {
                        attribute_name: element.attribute_name.clone(),
                        key_type: element.key_type.clone(),
                    })
                    .collect(),
                region: region.to_string(),
            },
            at,
        )
        .map_err(|rejected| StreamFault::Rejected(rejected.to_string()))?;
    // The label is read back off the REGISTERED stream rather than derived again
    // here, so the table and the streams service cannot report two different
    // labels for one stream.
    let label = store
        .stream(&arn)
        .ok_or(StreamFault::Internal(
            "the registered stream could not be read back",
        ))?
        .stream_label
        .clone();
    Ok(TableStream {
        arn,
        label,
        view_type: view_type.to_string(),
        enabled: true,
    })
}

/// Turn a table's stream OFF in the DynamoDB Streams store.
///
/// The stream stays registered and keeps the records it already carries — AWS
/// deletes a disabled stream only after 24 hours — so a consumer can still discover
/// it and drain it; it reports `DISABLING` and then `DISABLED`, and takes no further
/// records. `false` when the streams store cannot be reached or holds no stream with
/// that ARN.
pub fn disable_table_stream<Eng: Send + Sync + 'static>(
    engine: &Eng,
    stream_arn: &str,
    at: SystemTime,
) -> bool {
    use ::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810 as streams;

    streams::state::service_state(engine)
        .is_some_and(|store| store.lock().disable_stream(stream_arn, at))
}
