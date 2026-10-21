// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codedeploy-codedeploy-20141006 service.

/// Per-service state for com-amazonaws-codedeploy-codedeploy-20141006. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCodedeployCodedeploy20141006State {
    // TODO: add service-specific state fields
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodedeployCodedeploy20141006State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodedeployCodedeploy20141006State>)
}
