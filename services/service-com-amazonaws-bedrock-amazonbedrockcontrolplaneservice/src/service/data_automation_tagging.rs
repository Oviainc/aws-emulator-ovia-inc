// SPDX-License-Identifier: CC0-1.0

//! The Bedrock Data Automation half of the three tagging paths this service binds.
//!
//! WHY THIS LAYER EXISTS. `POST /tagResource`, `POST /untagResource` and
//! `POST /listTagsForResource` are bound IDENTICALLY by this control plane and by
//! `bedrock-data-automation` (`smithy_query(query="digest")` on either model), both sign
//! as `bedrock`, and both are served on one local endpoint — AWS tells the two apart by
//! endpoint HOST, which an emulator cannot read. The family's claim rule
//! (`service_common::bedrock`) therefore gives the whole segment to ONE claimant, this
//! service, so a request about a Bedrock Data Automation resource arrives here.
//!
//! WHAT DECIDES. The TARGET is named in the request's own body: `resourceARN`. Each of the
//! two models admits only its OWN resources there — this model's `TaggableResourcesArn`
//! admits a custom model, a guardrail, an inference profile and the rest of this plane's
//! resources, while the other admits a blueprint, a data-automation project, a
//! blueprint-optimization invocation and a data-automation library — so the ARN names
//! which of the two planes holds the resource the call is about, and the planes' sets do
//! not overlap. An ARN of a Bedrock Data Automation resource is therefore served out of
//! THAT model: by its own `TaggableResourceArn` constraint, its own handler, its own
//! refusal wording and its own store, exactly as it is under its own endpoint. Everything
//! else stays this service's, as it was.
//!
//! WHY NOT EARLIER. `Service::can_handle` is handed the request's parts WITHOUT its body,
//! so the ARN is not knowable where the claim is made; `Service::override_operation` is the
//! first seam handed the whole request, which is why the decision is taken here. This is
//! the same shape the Elastic Load Balancing front door takes for the two API versions
//! that share its endpoint
//! (`services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/service/api_version.rs`).
//!
//! The Bedrock Data Automation service is built over the SAME shared engine, so a
//! blueprint tagged through this front door is the blueprint its own `GetBlueprint`
//! reports: one store, reached through that model's own handlers.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice as bda;
use ::smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};
use std::sync::Arc;

/// The operations this front door answers for: the three tagging operations both models
/// bind on identical routes (`smithy_query(query="digest")` on each), named by the routing
/// key the models name them by — which is the same key in both, because both models
/// declare these operations under these names.
const TAGGING_OPERATIONS: &[&str] = &["TagResource", "UntagResource", "ListTagsForResource"];

/// The Bedrock Data Automation half of this service's tagging paths: that model's own
/// emulated service, served for a request whose `resourceARN` names one of ITS resources.
///
/// Held as a field on this service and handed back from `Service::override_operation`,
/// which is the one seam that sees the request body before the model's routing table
/// resolves it — and the ARN it decides on lives in that body.
pub struct DataAutomationTagging<Eng: Send + Sync + 'static> {
    data_automation: bda::EmulatedService<Eng>,
}

impl<Eng: Send + Sync + 'static> DataAutomationTagging<Eng> {
    /// Build the Bedrock Data Automation service this front door serves, from the shared
    /// engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            data_automation: bda::EmulatedService::new(engine),
        }
    }

    /// Whether the request under the routing key `key` is a tagging call about a Bedrock
    /// Data Automation resource, and so belongs to this front door.
    ///
    /// Both halves have to hold: the operation is one of the three tagging operations, and
    /// the `resourceARN` the body carries addresses a resource of that service
    /// (`bda::state::tagging::addresses_a_resource_of_this_service`, read off THAT model's
    /// own taggable-resource alternation rather than from a list restated here). A request
    /// whose body carries no readable `resourceARN` is NOT claimed: it is a malformed
    /// request to this service's own operation, which answers the absent member itself.
    pub fn serves(&self, key: &str, req: &RtRequest) -> bool {
        TAGGING_OPERATIONS.contains(&key)
            && target_resource_arn(req)
                .is_some_and(|arn| bda::state::tagging::addresses_a_resource_of_this_service(&arn))
    }

    /// The Bedrock Data Automation operation the request names, or `None` when that model
    /// declares none for it.
    ///
    /// The framework's own lookup, in the order and with the data the router uses
    /// (`MultiServiceRouter::dispatch` STEP 2): that service's own `override_operation` seam
    /// first — which is where its authorization gate stands — then its wire protocol reads
    /// the routing key off the request and its model-derived routing table must hold an
    /// operation under it. So nothing about which operation serves the request is restated
    /// here.
    fn data_automation_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        if let Some(op) = self.data_automation.override_operation(req) {
            return Some(op);
        }
        let definition = self.data_automation.definition();
        let key = definition.protocol().routing_key(req).ok()?;
        definition.routing_table().get(key)
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for DataAutomationTagging<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match self.data_automation_operation(req) {
                // The Bedrock Data Automation service — not the one that claimed the
                // request — is handed back as the SERVING service, which is what the
                // framework's centralized sequence reads the wire protocol, the modelled
                // input constraints and the refusal wording off. So the answer is that
                // model's own, down to its `TaggableResourceArn` and its date-time
                // timestamps, exactly as it is under that model's own routing key.
                Some(op) => op.run(&self.data_automation, req, ctx).await,
                // Unreachable in practice: this front door is only reached under a key both
                // models declare. Answer that service's own no-such-operation response
                // rather than inventing one.
                None => self.data_automation.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` defers to the modelled operation the request's routing key names, so the
        // request is verified under exactly the auth the model declares for it and this
        // front door can neither weaken nor widen it. Both models declare the service's
        // `aws.auth#sigv4` for these three operations and no `@auth` override on any of
        // them.
        None
    }
}

/// The ARN the request's body names as the target of a tagging call.
///
/// The body member every one of the three carries its target in is read through the Bedrock
/// Data Automation crate's own reader ([`bda::service::tagging_target`]), which is also what
/// that service's authorization gate reads it with — so the two cannot disagree about which
/// member names a tagging call's target or how it is read. It is read BEFORE either model's
/// input schema is applied, because it decides WHICH model's schema the request is then
/// validated against.
fn target_resource_arn(req: &RtRequest) -> Option<String> {
    bda::service::tagging_target::resource_arn_in_body(req)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::aws_smithy_types::body::SdkBody;

    /// A `POST` to `path` carrying `body`, as a client sends a tagging call.
    fn request(path: &str, body: &str) -> RtRequest {
        let mut req = RtRequest::new(SdkBody::from(body.to_string()));
        req.set_uri(path).expect("the test URI is valid");
        req.set_method("POST").expect("POST is a valid method");
        req
    }

    /// RULE: both halves decide — the request names one of the three tagging operations AND
    /// its body's `resourceARN` addresses a resource of the Bedrock Data Automation model.
    ///
    /// The rows span a blueprint and a project ARN (both that model's), this control
    /// plane's own guardrail ARN and a bare resource name (neither), a body naming no
    /// target at all, and an operation outside the three.
    #[test]
    fn only_a_tagging_call_naming_that_services_resource_is_served_there() {
        let blueprint = "arn:aws:bedrock:us-west-2:000000000000:blueprint/83f78afe19be";
        let project =
            "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/37ccb70b0000";
        let guardrail = "arn:aws:bedrock:us-west-2:000000000000:guardrail/abcdefabcdef";
        // (case, the routing key, the body, whether the front door serves it)
        let rows: [(&str, &str, String, bool); 7] = [
            (
                "TagResource naming a blueprint",
                "TagResource",
                format!("{{\"resourceARN\":\"{blueprint}\",\"tags\":[]}}"),
                true,
            ),
            (
                "UntagResource naming a project",
                "UntagResource",
                format!("{{\"resourceARN\":\"{project}\",\"tagKeys\":[]}}"),
                true,
            ),
            (
                "ListTagsForResource naming a blueprint",
                "ListTagsForResource",
                format!("{{\"resourceARN\":\"{blueprint}\"}}"),
                true,
            ),
            (
                "TagResource naming this plane's own guardrail",
                "TagResource",
                format!("{{\"resourceARN\":\"{guardrail}\",\"tags\":[]}}"),
                false,
            ),
            (
                "TagResource naming a bare resource name",
                "TagResource",
                "{\"resourceARN\":\"my-custom-model\",\"tags\":[]}".to_string(),
                false,
            ),
            (
                "TagResource naming no target at all",
                "TagResource",
                "{\"tags\":[]}".to_string(),
                false,
            ),
            (
                "another operation entirely, with that model's ARN in its body",
                "CreateGuardrail",
                format!("{{\"resourceARN\":\"{blueprint}\"}}"),
                false,
            ),
        ];

        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let front_door = DataAutomationTagging::new(engine);
        for (case, key, body, expected) in rows {
            let actual = front_door.serves(key, &request("/tagResource", &body));
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected}, got {actual}"
            );
        }
    }
}
