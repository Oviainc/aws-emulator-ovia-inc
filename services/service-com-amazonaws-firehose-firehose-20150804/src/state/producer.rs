// SPDX-License-Identifier: CC0-1.0

//! WHO is putting records on a stream.
//!
//! Most of what a stream does is decided by its own configuration, and one thing is not:
//! "Only requests from CloudWatch Logs are supported when CloudWatch Logs decompression is
//! enabled" (`InvalidSourceException`). A stream whose destination enables the CloudWatch
//! Logs pipeline ([`crate::state::CLOUDWATCH_LOGS_PROCESSOR_TYPES`]) therefore takes
//! records from THAT producer and refuses every other one, so the data plane has to tell
//! them apart.
//!
//! What says so is the CREDENTIAL the request presents. CloudWatch Logs forwards a
//! subscription filter's log events as its own service principal, acting for the log group
//! they were ingested into: it signs that one call with a credential it created for it alone
//! and records the principal and the resource under that credential's access key id
//! ([`::service_common::service_principal_calls`]). This reads the record back for the
//! access key id THIS request presents — the one thing about a request a client cannot
//! choose for itself, since presenting it takes the secret the emulator minted with it — and
//! so answers for the producer that actually made the call. Every other caller is an API
//! producer, which is what a direct `PutRecord` / `PutRecordBatch` is.

/// The service principal CloudWatch Logs forwards a subscription filter's log events as.
///
/// The same principal a filter's delivery role trusts
/// (`PutSubscriptionFilter$roleArn`, "an IAM role that grants CloudWatch Logs permissions
/// to deliver ingested log events to the destination stream").
pub const CLOUDWATCH_LOGS_SERVICE_PRINCIPAL: &str = "logs.amazonaws.com";

/// The producer one put came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Producer {
    /// A caller of the ingestion API: "`DirectPut`: Provider applications access the
    /// Firehose stream directly" (`CreateDeliveryStreamInput$DeliveryStreamType`).
    Api,
    /// CloudWatch Logs, forwarding the log events a subscription filter selected.
    CloudWatchLogs,
}

/// The producer the request `ctx` describes comes from.
#[must_use]
pub fn producer_of<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Producer {
    let Some(directory) = service_principal_calls(engine) else {
        return Producer::Api;
    };
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let account = ::engine::emulator_account_id();
    let now = ::std::time::SystemTime::now();
    let held = directory.lock();
    match held.call(&access_key_id, account, now) {
        Some(call) if call.principal == CLOUDWATCH_LOGS_SERVICE_PRINCIPAL => {
            Producer::CloudWatchLogs
        }
        _ => Producer::Api,
    }
}

/// The shared directory of the cross-service calls another service is making as its own
/// service principal, in the one shared engine: written by that service, read here.
fn service_principal_calls<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<::service_common::service_principal_calls::ServicePrincipalCalls>,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::service_principal_calls::ServicePrincipalCalls,
            >,
        )
}
