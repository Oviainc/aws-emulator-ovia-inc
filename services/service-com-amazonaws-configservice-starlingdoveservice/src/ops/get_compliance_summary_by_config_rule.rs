// SPDX-License-Identifier: CC0-1.0

//! @op GetComplianceSummaryByConfigRule
//! Reports the compliance roll-up of this account's Config rules, computed from the
//! rules persisted in [`crate::state`] and the evaluation results recorded against
//! each of them.
//!
//! HAND-AUTHORED, unlike its siblings: the scaffold op-stub generator declined this
//! operation because the model binds it to the `smithy.api#Unit` input shape (see
//! `src/ops/SKIPPED_STUBS.md`), so it was modeled-but-unwired and answered by the
//! framework's unroutable-operation path. It follows the pattern a rendered sibling
//! uses; its two shapes live in `super::types`.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `GetComplianceSummaryByConfigRule`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json")
//! ```
//!
//! Returns the number of Config rules that are compliant and noncompliant, up to a
//! maximum of 25 for each.
//!
//! ## Input
//!
//! (no members: the modeled input is `smithy.api#Unit`)
//!
//! ## Output
//!
//! * `ComplianceSummary` (ComplianceSummary) — The number of Config rules that are
//!   compliant and the number that are noncompliant, up to a maximum of 25 for each.
//!   Carries `CompliantResourceCount` and `NonCompliantResourceCount` (each a
//!   `ComplianceContributorCount` of `CappedCount` and `CapExceeded`) plus
//!   `ComplianceSummaryTimestamp`, "the time that Config created the compliance
//!   summary".
//!
//! ## Errors
//!
//! (none)

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetComplianceSummaryByConfigRuleError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-configservice-starlingdoveservice.json");

// The operation's MODELED errors. `GetComplianceSummaryByConfigRule` declares none,
// so this enum is UNINHABITED, which is correct and expected: there is nothing to
// construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.configservice#GetComplianceSummaryByConfigRule")]
#[derive(Debug)]
pub enum GetComplianceSummaryByConfigRuleErrorModeled {}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_compliance_summary_by_config_rule::GetComplianceSummaryByConfigRuleError`).
/// The wrapper is also the only place an operation whose model declares no error at
/// all can refuse a request.
pub type GetComplianceSummaryByConfigRuleError =
    crate::scaffolded::Scaffolded<GetComplianceSummaryByConfigRuleErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant.
impl From<GetComplianceSummaryByConfigRuleErrorModeled>
    for GetComplianceSummaryByConfigRuleError
{
    fn from(modeled: GetComplianceSummaryByConfigRuleErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetComplianceSummaryByConfigRule` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetComplianceSummaryByConfigRule<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetComplianceSummaryByConfigRule<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetComplianceSummaryByConfigRule<Eng>
{
    type Input = GetComplianceSummaryByConfigRuleInput;
    type Output = GetComplianceSummaryByConfigRuleOutput;
    type Error = GetComplianceSummaryByConfigRuleError;

    const NAME: &'static str = "GetComplianceSummaryByConfigRule";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_COMPLIANCE_SUMMARY_BY_CONFIG_RULE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_COMPLIANCE_SUMMARY_BY_CONFIG_RULE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The modeled input has no members, and `ctx` carries only request metadata;
        // this operation reads neither.
        let _ = (&input, ctx);
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            crate::scaffolded::Scaffolded::validation(crate::state::STATE_UNAVAILABLE)
        })?;

        // A reporting operation: the roll-up is computed over the rules in state and
        // nothing is written. `ComplianceSummaryTimestamp` is the instant this summary
        // was created, which is now.
        let store = state.lock();
        let summary = crate::state::summarise_rule_compliance(
            store.config_rules.values(),
            ::std::time::SystemTime::now(),
        );

        Ok(GetComplianceSummaryByConfigRuleOutput {
            compliance_summary: Some(summary),
        })
    }
}
