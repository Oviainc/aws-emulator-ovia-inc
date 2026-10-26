// SPDX-License-Identifier: CC0-1.0

//! @op GetSendStatistics
//!
//! Reports the account's sending activity as the 15-minute data points SES
//! aggregates it into.
//!
//! HAND-AUTHORED, like `get_send_quota.rs`: the scaffold op-stub generator
//! declined `GetSendStatistics` because the model binds it to the
//! `smithy.api#Unit` input shape, which the generator does not emit yet. Without
//! this file the operation was modeled but UNWIRED — absent from the service's
//! `operations(...)` allow-list, so the framework answered a caller addressing it
//! through the unroutable-operation path (`400 InvalidAction`) instead of serving
//! it. The `//! @op GetSendStatistics` line above is what puts it back in that
//! allow-list, and `types/get_send_statistics_types.rs` carries its shapes.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `GetSendStatistics::new(engine)` and drives it through the centralized request
//! walk: deserialize → validate → `handle` → serialize. You write ONLY the `handle`
//! body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! ---
//!
//! # Modeled contract for `GetSendStatistics`
//!
//! Provides sending statistics for the current Amazon Web Services Region. The result
//! is a list of data points, representing the last two weeks of sending activity. Each
//! data point in the list contains statistics for a 15-minute period of time. You can
//! execute this operation no more than once per second.
//!
//! ## Input
//!
//! (no members: the modeled input is `smithy.api#Unit`)
//!
//! ## Output
//!
//! * `SendDataPoints` (list<SendDataPoint>) — A list of data points, each of which
//!   represents 15 minutes of activity. A `SendDataPoint` carries `Timestamp` (time of
//!   the data point), `DeliveryAttempts` (number of emails that have been sent),
//!   `Bounces`, `Complaints` and `Rejects` (long, default 0 each).
//!
//! ## Errors
//!
//! (none)

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetSendStatisticsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ses-simpleemailservice.json");

// The operation's MODELED errors. `GetSendStatistics` declares none, so this
// enum is UNINHABITED, which is correct and expected: there is nothing to
// construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ses#GetSendStatistics")]
#[derive(Debug)]
pub enum GetSendStatisticsErrorModeled {}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_send_statistics::GetSendStatisticsError`). The wrapper is also
/// the only place an operation whose model declares no error at all can refuse a
/// request.
pub type GetSendStatisticsError = crate::scaffolded::Scaffolded<GetSendStatisticsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant.
impl From<GetSendStatisticsErrorModeled> for GetSendStatisticsError {
    fn from(modeled: GetSendStatisticsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetSendStatistics` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetSendStatistics<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetSendStatistics<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetSendStatistics<Eng>
{
    type Input = GetSendStatisticsInput;
    type Output = GetSendStatisticsOutput;
    type Error = GetSendStatisticsError;

    const NAME: &'static str = "GetSendStatistics";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_SEND_STATISTICS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_SEND_STATISTICS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The modeled input has no members, and `ctx` carries only request
        // metadata; this operation reads neither.
        let _ = (&input, ctx);
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // See `verify_email_identity.rs`: an engine with no store for this
            // service cannot serve the operation at all, and it models no server
            // error of its own.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "GetSendStatistics",
            ));
        };
        let state = state.lock();

        // Reporting only, and DERIVED: the data points are the send log bucketed
        // into the periods the operation documents, computed on every read, so a
        // period's numbers cannot drift from the messages the log holds and the
        // list shortens again as activity ages out of the window.
        let send_data_points = state
            .send_data_points(std::time::SystemTime::now())
            .into_iter()
            .map(|(period_start, deliveries)| SendDataPoint {
                // "Time of the data point": the start of the 15-minute period the
                // counters below cover.
                timestamp: Some(::aws_smithy_types::DateTime::from(period_start)),
                // "Number of emails that have been sent" in that period.
                delivery_attempts: i64::try_from(deliveries).unwrap_or(i64::MAX),
                // A bounce, a complaint and a reject are each an event this
                // emulator's state holds none of: it accepts a send or refuses it
                // (a refused send is never logged), and it delivers no mail, so
                // nothing can bounce or be complained about. Reported as the
                // periods they were observed in — none — rather than invented.
                bounces: 0,
                complaints: 0,
                rejects: 0,
            })
            .collect();

        Ok(GetSendStatisticsOutput {
            // Present even when empty: an account that has sent nothing has an
            // empty list of data points, which is a different answer from an
            // absent member.
            send_data_points: Some(send_data_points),
        })
    }
}
