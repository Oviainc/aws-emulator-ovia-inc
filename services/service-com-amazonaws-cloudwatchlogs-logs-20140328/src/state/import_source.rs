// SPDX-License-Identifier: CC0-1.0

//! Resolving the data source an import task reads from, against the service that holds it.
//!
//! `CreateImportTask` "starts an import from a data source to CloudWatch Log", and
//! "currently, CloudTrail Event Data Store is the only supported data source": its
//! `importSourceArn` therefore names a resource of ANOTHER service, which this service
//! resolves where that service holds it rather than assuming. CloudTrail is reached the way
//! the framework documents a handler reaching another emulated service
//! ([`smithy_server_runtime::OperationContext::callback`]): an AWS SDK client over the
//! in-process callback's HTTP client and credentials, so nothing here opens a socket or
//! leaves the host. The question is put with `GetEventDataStore`, the operation CloudTrail
//! documents for reading one store ("Returns information about an event data store specified
//! as either an ARN or the ID portion of the ARN").
//!
//! The one caller-supplied value that reaches the client's AUTHORITY is the signing Region,
//! and it is narrowed before it gets there ([`service_common::arn::region_label`]), so no
//! request can re-point this client at a host of its own choosing: it reaches the emulator's
//! own CloudTrail and nothing else, whatever source a caller names. The client itself is the
//! shared callback client every call to another service of this emulator is made through
//! ([`crate::state::callback_client`]).

use super::callback_client::callback_client;

/// What the owning service answers about the source an import names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportSource {
    /// CloudTrail holds an event data store under the ARN, in a state an import may read
    /// from.
    Active,
    /// CloudTrail holds the event data store, but it is not in a state an import may read
    /// from.
    Inactive,
    /// No event data store answers to the ARN: CloudTrail reported none under it, or the
    /// ARN names no event data store at all.
    Absent,
    /// The source could not be resolved — an emulator assembled with no CloudTrail to ask,
    /// or any other failure to ask it. NOTHING is then established about the source, so it
    /// grounds no refusal.
    Unresolved,
}

/// The ARN service segment of the one supported data source's owning service.
const SOURCE_SERVICE: &str = "cloudtrail";

/// The ARN resource-type segment of the one supported data source: an event data store
/// (`arn:aws:cloudtrail:<region>:<account>:eventdatastore/<id>`).
const SOURCE_RESOURCE_TYPE: &str = "eventdatastore/";

/// What CloudTrail reports about the source an import task would read from `source_arn`.
///
/// The ARN is read first: the only supported data source is a CloudTrail Lake event data
/// store, so an ARN naming a resource of any other kind answers to no store CloudTrail could
/// hold and is [`ImportSource::Absent`] without asking it.
pub async fn resolve(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    source_arn: &str,
) -> ImportSource {
    if !names_event_data_store(source_arn) {
        return ImportSource::Absent;
    }
    // Which Region the store is looked up in decides nothing here — the emulator serves one
    // CloudTrail over the in-process callback — so a scope that carries no Region label falls
    // back to the default rather than putting caller text in the authority.
    let region =
        ::service_common::arn::region_label(region).unwrap_or(crate::state::DEFAULT_REGION);
    let client = callback_client!(aws_sdk_cloudtrail, ctx.callback(), "cloudtrail", region);
    match client
        .get_event_data_store()
        .event_data_store(source_arn)
        .send()
        .await
    {
        // CloudTrail holds the store, and states the status it is in.
        Ok(store) => match store.status() {
            Some(status) if is_active(status) => ImportSource::Active,
            // A status outside the set an import may read from.
            Some(_) => ImportSource::Inactive,
            // CloudTrail stated no status at all, so nothing is established about whether the
            // store may be read from and nothing is refused on it.
            None => ImportSource::Active,
        },
        // CloudTrail says no event data store of this account answers to the ARN: the
        // refusal is grounded in its answer.
        Err(error) if reported_absent(&error) => ImportSource::Absent,
        // Any other answer is this emulator failing to ask, not CloudTrail reporting on the
        // store, so it refuses nothing — and it is worth a warning, because a deployment with
        // no CloudTrail to ask is one where this resolution does not run. The caller's own
        // value is rendered in its escaped `Debug` form, so a source ARN carrying a newline
        // cannot forge a log line.
        Err(error) => {
            ::tracing::warn!(
                import_source = ?source_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an import task's source could not be resolved against CloudTrail"
            );
            ImportSource::Unresolved
        }
    }
}

/// Whether `arn` names a CloudTrail Lake event data store — the one kind of resource that is
/// a supported import source.
fn names_event_data_store(arn: &str) -> bool {
    event_data_store_id(arn).is_some()
}

/// The event data store id an import source ARN carries
/// (`arn:aws:cloudtrail:<region>:<account>:eventdatastore/<id>`), or `None` when the ARN names
/// no event data store.
///
/// THE one reading of an import source ARN: the resolution above asks the owning service about
/// a source this places, and the managed destination log group an accepted import delivers
/// into is named after the store this answers
/// ([`crate::state::import_destination_log_group_name`]).
pub fn event_data_store_id(arn: &str) -> Option<&str> {
    let (service, resource) = crate::state::arn_service_and_resource(arn)?;
    if service != SOURCE_SERVICE {
        return None;
    }
    let id = resource.strip_prefix(SOURCE_RESOURCE_TYPE)?;
    (!id.is_empty()).then_some(id)
}

/// Whether the status CloudTrail reports for an event data store is one an import may read
/// from.
///
/// "The specified source must be in an ACTIVE state", and the model's `EventDataStoreStatus`
/// enum carries no `ACTIVE`: the statuses in which a store is there and in service are the
/// one it is created in and the one it settles into. This is an ALLOW-LIST, so a status that
/// is on its way out (queued for deletion) or not in service (an ingestion change) is not
/// read from, and a status the enum gains later is not either until it is named here.
fn is_active(status: &::aws_sdk_cloudtrail::types::EventDataStoreStatus) -> bool {
    matches!(
        status,
        ::aws_sdk_cloudtrail::types::EventDataStoreStatus::Created
            | ::aws_sdk_cloudtrail::types::EventDataStoreStatus::Enabled
    )
}

/// Whether a CloudTrail `GetEventDataStore` failure is CloudTrail reporting that no event
/// data store answers to the identifier — either because it holds none under it, or because
/// the identifier is in no form a store is named by — as opposed to the call not reaching
/// CloudTrail at all.
fn reported_absent(
    error: &::aws_sdk_cloudtrail::error::SdkError<
        ::aws_sdk_cloudtrail::operation::get_event_data_store::GetEventDataStoreError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error.as_service_error().is_some_and(|service_error| {
        service_error.is_event_data_store_not_found_exception()
            || service_error.is_event_data_store_arn_invalid_exception()
    })
}
