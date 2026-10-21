// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sts-awssecuritytokenservicev20110615 service.

/// Per-service state for com-amazonaws-sts-awssecuritytokenservicev20110615. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsStsAwssecuritytokenservicev20110615State {
    // TODO: add service-specific state fields
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsStsAwssecuritytokenservicev20110615State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsStsAwssecuritytokenservicev20110615State>)
}
