// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemakerruntime-amazonsagemakerruntime service.
//!
//! This service is SageMaker's inference DATA plane: the endpoints it invokes are owned
//! by the SageMaker control plane, so an endpoint is resolved in that service's store
//! (`service_com_amazonaws_sagemaker_sagemaker::state`) through the one shared engine,
//! never copied into a second map here. What this store holds is what the data plane
//! itself produces: the log of the invocations it served, which is the emulator's
//! counterpart of the per-endpoint invocation metrics the real service publishes.

use std::collections::VecDeque;
use std::time::SystemTime;

/// The message a handler refuses with when the service is mounted on an engine that
/// holds no store for it. Unreachable in the assembled emulator (the router mounts
/// every service on the one shared `InMemoryEngine`); stated once so no handler
/// invents its own wording — and never by building a private engine, which would drop
/// the request's writes.
pub const NO_STATE: &str = "SageMaker Runtime state is unavailable: this service is not \
                            mounted on the shared in-memory engine";

/// The message a handler refuses with when the SageMaker CONTROL PLANE's store is not
/// reachable on this engine, so the endpoint a request names can be neither resolved
/// nor refused as missing.
pub const NO_CONTROL_PLANE_STATE: &str =
    "SageMaker's endpoint store is unavailable: the SageMaker control plane is not \
     mounted on the shared in-memory engine";

/// How many invocations the log below keeps.
///
/// The log is bounded so a client invoking an endpoint in a loop cannot grow this
/// service's state without end; the oldest entry is dropped when a new one arrives at
/// the cap.
pub const MAX_RECORDED_INVOCATIONS: usize = 1024;

/// What the service did with one `InvokeEndpoint` call.
///
/// Every variant but [`InvocationOutcome::NoInferenceContainer`] is a refusal of the
/// REQUEST, decided from the control plane's own record of the endpoint. The last one is
/// the honest answer to a request this emulator cannot serve: a model container is
/// customer-provided code in a container image, which a generated emulator cannot run,
/// so no inference is performed and the operation's modelled `ModelError` says so rather
/// than a synthesised prediction body claiming work that never ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationOutcome {
    /// No endpoint of that name exists in the account.
    EndpointNotFound,
    /// The endpoint exists but is not `InService`, so it serves no traffic yet.
    EndpointNotInService,
    /// `TargetVariant` named a production variant the endpoint does not host.
    VariantNotFound,
    /// `EnableExplanations` was set on an endpoint carrying no explainer configuration.
    ExplanationsNotConfigured,
    /// `SessionId` named a stateful session that does not exist.
    SessionNotFound,
    /// Everything about the request resolved; no container ran the inference.
    NoInferenceContainer,
}

impl InvocationOutcome {
    /// A stable short name for the outcome, for logs and test assertions.
    pub fn label(self) -> &'static str {
        match self {
            Self::EndpointNotFound => "EndpointNotFound",
            Self::EndpointNotInService => "EndpointNotInService",
            Self::VariantNotFound => "VariantNotFound",
            Self::ExplanationsNotConfigured => "ExplanationsNotConfigured",
            Self::SessionNotFound => "SessionNotFound",
            Self::NoInferenceContainer => "NoInferenceContainer",
        }
    }
}

/// One invocation this service served, as the log below replays it.
///
/// The request BODY and `CustomAttributes` are `@sensitive` in the model, so neither is
/// stored: the record keeps the body's SIZE, which is what a metric would report, and
/// the non-sensitive routing members of the request.
#[derive(Debug, Clone)]
pub struct EndpointInvocation {
    pub endpoint_name: String,
    /// The production variant the request TARGETED, or `None` when it named none —
    /// without `TargetVariant` the real service distributes the request across the
    /// endpoint's variants by their weights, and no container served this one, so no
    /// variant is recorded for it.
    pub target_variant: Option<String>,
    /// `Body`'s length in bytes; the body itself is `@sensitive` and is not stored.
    pub request_bytes: usize,
    pub content_type: Option<String>,
    pub accept: Option<String>,
    pub inference_id: Option<String>,
    pub target_model: Option<String>,
    pub session_id: Option<String>,
    pub outcome: InvocationOutcome,
    pub at: SystemTime,
}

/// Per-service state for com-amazonaws-sagemakerruntime-amazonsagemakerruntime. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSagemakerruntimeAmazonsagemakerruntimeState {
    /// Every invocation this service served, oldest first, capped at
    /// [`MAX_RECORDED_INVOCATIONS`].
    invocations: VecDeque<EndpointInvocation>,
}

impl ComAmazonawsSagemakerruntimeAmazonsagemakerruntimeState {
    /// Append `invocation` to the log, dropping the oldest entry at the cap.
    pub fn record_invocation(&mut self, invocation: EndpointInvocation) {
        while self.invocations.len() >= MAX_RECORDED_INVOCATIONS {
            self.invocations.pop_front();
        }
        self.invocations.push_back(invocation);
    }

    /// Every logged invocation, oldest first.
    pub fn invocations(&self) -> impl ExactSizeIterator<Item = &EndpointInvocation> {
        self.invocations.iter()
    }

    /// The logged invocations of one endpoint, oldest first.
    pub fn invocations_of<'log>(
        &'log self,
        endpoint_name: &'log str,
    ) -> impl Iterator<Item = &'log EndpointInvocation> {
        self.invocations
            .iter()
            .filter(move |invocation| invocation.endpoint_name == endpoint_name)
    }

    /// The most recently logged invocation.
    pub fn last_invocation(&self) -> Option<&EndpointInvocation> {
        self.invocations.back()
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSagemakerruntimeAmazonsagemakerruntimeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSagemakerruntimeAmazonsagemakerruntimeState>)
}
