// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-simpledbv2-simpledbv2 service.

/// Per-service state for com-amazonaws-simpledbv2-simpledbv2. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSimpledbv2Simpledbv2State {
    // TODO: add service-specific state fields
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSimpledbv2Simpledbv2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSimpledbv2Simpledbv2State>)
}
