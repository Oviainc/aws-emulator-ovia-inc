// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-proton-awsproton20200720 service.

/// Per-service state for com-amazonaws-proton-awsproton20200720. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsProtonAwsproton20200720State {
    // TODO: add service-specific state fields
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsProtonAwsproton20200720State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsProtonAwsproton20200720State>)
}
