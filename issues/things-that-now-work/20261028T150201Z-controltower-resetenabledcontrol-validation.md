Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4db25e6bf87f4906c725859fd3714c6300682a7e

# ResetEnabledControl never refuses a control implemented with an SCP, which the model says it does not work for

_Filed: 2026-10-28T15:02:01Z_
_Operation: ResetEnabledControl | Class: validation_

> _Note: this issue was identified against the code base at commit `4db25e6bf87f4906c725859fd3714c6300682a7e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The Smithy model states, as the whole of ResetEnabledControl's documentation, that the operation "Does not work for controls implemented with SCPs." The handler applies no such rule: once the `enabledControlIdentifier` resolves to a stored enablement and no operation is in flight against it, EVERY enablement is accepted and a RESET_ENABLED_CONTROL operation is started, whichever control it applies. The handler's own comment admits the gap in so many words, so the refusal the model documents simply does not exist anywhere in the crate: no branch, no catalog field, no helper. A caller that resets an SCP-implemented control gets a 200 and a fresh `operationIdentifier` that settles to SUCCEEDED, instead of the refusal the model describes.

The sibling rule shows the shape the crate already uses for exactly this kind of model sentence: `EnableControlInput$controlIdentifier`'s "Only Strongly recommended and Elective controls are permitted" is enforced from a per-control catalog flag (`CatalogControl::enablable`), and `EnableControl` refuses a control the flag excludes. ResetEnabledControl's documented restriction is the one control-level restriction in the model that is read and then dropped.

## Evidence

Model (smithy_query query="doc" shape="ResetEnabledControl"), also copied verbatim into the handler header:
  services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/reset_enabled_control.rs:48
  `//! Resets an enabled control. Does not work for controls implemented with SCPs.`

Handler, admitting it applies no such rule and starting the reset unconditionally:
  src/ops/reset_enabled_control.rs:212-214
  `// Nothing this service stores records HOW a control is implemented, so no request`
  `// is refused here for naming one implemented with a service control policy.`
  src/ops/reset_enabled_control.rs:214
  `let Some(operation_identifier) = store.record_enabled_control_reset(&control.arn, now)`

The only refusals the handler contains are the 404 for an unknown identifier (reset_enabled_control.rs:172-186) and the 409 for an operation in flight (reset_enabled_control.rs:192-203); nothing inspects the control itself.

That the crate CAN express a per-control restriction, for the one other such model sentence:
  src/state.rs:2691-2699
  `/// Whether `EnableControl` may enable this control.`
  `/// ... the MANDATORY controls AWS Control Tower applies to every governed`
  `/// account itself ... cannot be enabled by a caller, so a request naming one is refused rather`
  `/// than persisted. False for exactly those.`
  `pub enablable: bool,`
and its enforcement in src/ops/enable_control.rs:234-246.

Test coverage: tests/all/enabled_control_reset.rs enumerates the enablable catalog controls (`fn enablable_without_parameters`, line 104; `fn enablable_with_parameters`, line 111) and asserts every row's reset succeeds (`a_reset_redeploys_the_configuration_the_control_already_carries`, line 249). No test in the crate mentions SCPs at all (grep for `SCP|service control polic` matches only reset_enabled_control.rs:48 and :213), so no test pins the current accept-everything behaviour with a stated reason.

## Steps to Reproduce

1. POST /enable-control with a `controlIdentifier` the catalog publishes as enablable and an organizational-unit `targetIdentifier`; note the `arn` returned.
2. Wait for the enablement to settle (OPERATION_SETTLE_AFTER = 500 ms) so the control reports SUCCEEDED.
3. POST /reset-enabled-control with `enabledControlIdentifier` = that ARN.
4. Observe HTTP 200 with a fresh `operationIdentifier` for every control in the catalog: no control is ever refused, although the model documents ResetEnabledControl as not working for controls implemented with SCPs.
