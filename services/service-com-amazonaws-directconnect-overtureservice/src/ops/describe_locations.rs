// SPDX-License-Identifier: CC0-1.0

//! @op DescribeLocations
//!
//! Lists the Direct Connect locations in the Region the request is signed for, from
//! the catalog [`crate::state`] holds. A pure read: the catalog is seeded when the
//! store is created, so this handler takes the guard, renders what is stored and
//! releases it — a `DescribeLocations` leaves the next caller seeing exactly what it
//! saw.
//!
//! Hand-authored rather than scaffolded: the op-stub generator declined this
//! operation because its input is `smithy.api#Unit` (recorded in
//! `src/ops/SKIPPED_STUBS.md`), so it had no handler file and no entry in the
//! service's `operations(...)` allow-list and was answered by the framework's
//! unroutable-operation path with an unmodelled 501. `build.rs` derives that
//! allow-list from the `//! @op <Name>` header of every `src/ops/*.rs`, so this file
//! is what wires it.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back;
//! never construct an engine in a handler.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only
//! — here the signed Region, which is what selects the locations.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeLocations`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-directconnect-overtureservice/smithy-model/com-amazonaws-directconnect-overtureservice.json")
//! ```
//!
//! Lists the Direct Connect locations in the current Amazon Web Services Region.
//! These are the locations that can be selected when calling CreateConnection or
//! CreateInterconnect.
//!
//! ## Input
//!
//! `smithy.api#Unit` — no members.
//!
//! ## Output
//!
//! * `locations` (LocationList) — The locations.
//!   * `locationCode` (LocationCode) — The code for the location.
//!   * `locationName` (LocationName) — The name of the location. This includes the name of
//!     the colocation partner and the physical site of the building.
//!   * `region` (Region) — The Amazon Web Services Region for the location.
//!   * `availablePortSpeeds` (AvailablePortSpeeds) — The available port speeds for the
//!     location.
//!   * `availableProviders` (ProviderList) — The name of the service provider for the
//!     location.
//!   * `availableMacSecPortSpeeds` (AvailableMacSecPortSpeeds) — The available MAC Security
//!     (MACsec) port speeds for the location.
//!   * `availableBillingModes` (AvailableBillingModeList) — The billing modes available at
//!     the location, including the port speeds and Amazon Web Services Regions supported by
//!     each mode.
//!
//! ## Errors
//!
//! * `DirectConnectClientException` (HTTP 400) — One or more parameters are not valid.
//! * `DirectConnectServerException` (HTTP 500) — A server-side error occurred.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeLocationsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-directconnect-overtureservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeLocationsError` alias below.
#[smithy_macros::operation_error("com.amazonaws.directconnect#DescribeLocations")]
#[derive(Debug)]
pub enum DescribeLocationsErrorModeled {
    DirectConnectClientException(DirectConnectClientException),
    DirectConnectServerException(DirectConnectServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_locations::DescribeLocationsError`).
pub type DescribeLocationsError = crate::scaffolded::Scaffolded<DescribeLocationsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the
/// handler writes `Err(SomeModeledError.into())` rather than naming the wrapper
/// variant.
impl From<DescribeLocationsErrorModeled> for DescribeLocationsError {
    fn from(modeled: DescribeLocationsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeLocations` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct DescribeLocations<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeLocations<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeLocations<Eng>
{
    type Input = DescribeLocationsInput;
    type Output = DescribeLocationsOutput;
    type Error = DescribeLocationsError;

    const NAME: &'static str = "DescribeLocations";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DESCRIBE_LOCATIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DESCRIBE_LOCATIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input is `smithy.api#Unit`, so there is nothing to read from it and no
        // request validation to do.
        let _ = &input;

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DescribeLocationsErrorModeled::DirectConnectServerException(
                DirectConnectServerException {
                    message: Some(
                        "the Direct Connect store is unavailable on this engine".to_string(),
                    ),
                },
            )
            .into());
        };

        // "Lists the Direct Connect locations in the current Amazon Web Services
        // Region": the Region is the one the request is signed for, never a member
        // of the body (there is none).
        let region = crate::state::request_region(&ctx.parts().headers);

        // A read: take the guard, render what is stored, release it. A Region the
        // catalog holds no location in answers an empty list on 200 rather than an
        // error — the operation models no "no such Region" failure.
        let store = state.lock();
        let locations = store
            .locations_in_region(&region)
            .into_iter()
            .map(|location| store.location_output(location))
            .collect();

        Ok(DescribeLocationsOutput {
            locations: Some(locations),
        })
    }
}
