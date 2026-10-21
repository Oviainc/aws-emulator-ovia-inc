// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-s3vectors-s3vectors service.

/// Per-service state for com-amazonaws-s3vectors-s3vectors. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsS3vectorsS3vectorsState {
    // TODO: add service-specific state fields
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsS3vectorsS3vectorsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsS3vectorsS3vectorsState>)
}
