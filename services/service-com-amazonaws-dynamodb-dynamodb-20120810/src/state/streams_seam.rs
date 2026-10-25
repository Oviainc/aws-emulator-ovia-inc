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
//!
//! The DATA plane's half is [`publish_item_changes`]: an item write records what it
//! changed on this service's store and the handler hands the queue to the streams
//! registry at the end of the request, so the records a consumer reads are appended
//! THERE too.

use std::time::SystemTime;

use crate::ops::types::{KeySchemaElement, StreamSpecification};
use crate::state::{RequestError, TableStream};

/// A table's stream must not look readable before the table itself serves item
/// writes: a consumer that waits for the stream's shards and then writes an item
/// would otherwise reach a table still `CREATING`. The streams registry publishes a
/// stream's shards when its enable falls due, so that span has to be at least this
/// service's own `CREATING` -> `ACTIVE` span, and this is where the two are held
/// together rather than left to agree by accident.
const _: () = assert!(
    crate::state::STATUS_TRANSITION.as_nanos()
        <= ::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::state::STREAM_ENABLE_DELAY
            .as_nanos(),
    "a table must reach ACTIVE no later than its stream reports ENABLED"
);

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

/// Hand every item-level change the writes of this request made to the DynamoDB
/// Streams store, in the order they were made.
///
/// This is the DATA plane's half of the publishing seam: a write landed an item in
/// THIS service's store and queued what it changed
/// ([`crate::state::PendingStreamRecord`]); the records themselves are the streams
/// service's resources, so they are appended THERE, which is where every stream
/// consumer reads them.
///
/// Which modification a record describes is derived from the images the write queued:
/// an item that was not there before is an `INSERT`, one that was and still is a
/// `MODIFY`, and one that is gone a `REMOVE` (the model's `OperationType`). The
/// stream drops the images its own `StreamViewType` does not capture, so what a
/// consumer sees is never more than the stream was created to capture.
///
/// Publication cannot fail the write that made the change — the item is already
/// stored and the response is already decided, and DynamoDB publishes to a stream
/// asynchronously — so a change the registry will not take is LOGGED rather than
/// turned into an error for the caller.
///
/// LOCKING: like [`register_table_stream`], this takes the STREAMS store's lock while
/// the caller holds this service's own, which is the one order the emulator uses. The
/// work is bounded by the queue, and the queue by one request's writes.
pub fn publish_item_changes<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &mut crate::state::ComAmazonawsDynamodbDynamodb20120810State,
    at: SystemTime,
) {
    use ::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810 as streams;

    let changes = store.take_stream_records();
    if changes.is_empty() {
        return;
    }
    let Some(registry) = streams::state::service_state(engine) else {
        ::tracing::error!(
            changes = changes.len(),
            "the DynamoDB Streams store is unreachable, so item changes reached no stream"
        );
        return;
    };
    let mut registry = registry.lock();
    for change in changes {
        let event_name = match (&change.old_image, &change.new_image) {
            (None, Some(_)) => streams::state::OperationType::Insert,
            (Some(_), Some(_)) => streams::state::OperationType::Modify,
            (Some(_), None) => streams::state::OperationType::Remove,
            // A queued change with neither image describes no modification, so there
            // is nothing to publish; no write path queues one.
            (None, None) => continue,
        };
        let event = streams::state::RecordAppend {
            event_name,
            keys: streams_image(&change.keys),
            new_image: change.new_image.as_ref().map(streams_image),
            old_image: change.old_image.as_ref().map(streams_image),
        };
        if let Err(rejected) = registry.append_record(&change.stream_arn, &event, at) {
            ::tracing::error!(
                stream_arn = %change.stream_arn,
                event_name = event_name.as_str(),
                reason = ?rejected,
                "an item change could not be appended to its stream"
            );
        }
    }
}

/// One stored image, as the STREAMS service's own attribute map.
///
/// The two services model the same `AttributeValue` union, but each generates its own
/// type from its own model, so an image crossing the seam is rebuilt member for member
/// rather than transmuted. Recursion is bounded by the nesting limit every image is
/// held to before it is stored ([`crate::state::check_attribute_value`]), and the
/// registry checks the same limit again on append.
fn streams_image(
    image: &crate::state::Item,
) -> ::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::state::AttributeMap {
    image
        .iter()
        .map(|(name, value)| (name.clone(), streams_value(value)))
        .collect()
}

/// One stored attribute value, as the STREAMS service's own `AttributeValue`.
fn streams_value(
    value: &crate::ops::types::AttributeValue,
) -> ::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::ops::types::AttributeValue {
    use crate::ops::types::AttributeValue as Stored;
    use ::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::ops::types::AttributeValue as Published;

    match value {
        Stored::S(text) => Published::S(text.clone()),
        Stored::N(number) => Published::N(number.clone()),
        Stored::B(bytes) => Published::B(bytes.clone()),
        Stored::Ss(members) => Published::Ss(members.clone()),
        Stored::Ns(members) => Published::Ns(members.clone()),
        Stored::Bs(members) => Published::Bs(members.clone()),
        Stored::M(members) => Published::M(
            members
                .iter()
                .map(|(name, nested)| (name.clone(), streams_value(nested)))
                .collect(),
        ),
        Stored::L(members) => Published::L(members.iter().map(streams_value).collect()),
        Stored::Null(is_null) => Published::Null(*is_null),
        Stored::Bool(flag) => Published::Bool(*flag),
    }
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
