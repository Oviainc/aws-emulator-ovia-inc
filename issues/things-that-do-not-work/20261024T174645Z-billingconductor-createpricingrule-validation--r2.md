Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0de7718ade8f368be0e47aca52d0dd18a585e082

# CreatePricingRule accepts (and persists) members the rule's Scope does not key on, e.g. Service on a GLOBAL rule

_Filed: 2026-10-24T17:46:45Z_
_Operation: CreatePricingRule | Class: validation_

> _Note: this issue was identified against the code base at commit `0de7718ade8f368be0e47aca52d0dd18a585e082`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreatePricingRule` enforces the model's Scope-conditional member rules in ONE direction only: it refuses a request that OMITS a member the scope needs (`Service` for `SERVICE`/`SKU`, `UsageType`+`Operation` for `SKU`, `BillingEntity` for `BILLING_ENTITY`), but it happily accepts the same members when the scope gives them no meaning.

Expected: a pricing rule describes exactly one scope, and the members that scope does not key on are not part of that rule — a request that names one is refused with the modelled 400 `ValidationException` under the reason the model enumerates for that member (`ILLEGAL_SERVICE` / `ILLEGAL_USAGE_TYPE` / `ILLEGAL_OPERATION` / `ILLEGAL_BILLING_ENTITY`, or `ILLEGAL_SCOPE` for the scope that cannot carry it), and nothing is stored.

Actual: HTTP 200 with a minted ARN, the irrelevant member is persisted on the record, and every later read echoes it — `ListPricingRules`/`UpdatePricingRule` report, for instance, a rule with `Scope: "GLOBAL"` **and** `Service: "AmazonEC2"`, or a `MARKUP` `SERVICE`-scoped rule carrying `UsageType`/`Operation` that only a `SKU` rule can act on. The resulting record describes a rule the service's own pricing semantics cannot apply, and it is indistinguishable (to a client) from a rule AWS would have refused to create.

Symptoms of this one class, to be fixed together:
- `Scope: GLOBAL` (or `BILLING_ENTITY`) + `Service`
- `Scope: GLOBAL` / `SERVICE` / `BILLING_ENTITY` + `UsageType` and/or `Operation`
- `Scope: GLOBAL` / `SERVICE` / `SKU` + `BillingEntity`

## Evidence

Spec: the member documentation in the model conditions each of these members on the scope — `Service`: "If the Scope attribute is set to SERVICE or SKU, the attribute indicates which service the PricingRule is applicable for"; `UsageType`/`Operation`: "If the Scope attribute is set to SKU, this attribute indicates which usage type / operation the PricingRule is modifying" (smithy_query doc, CreatePricingRuleInput members). `ValidationExceptionReason` enumerates `ILLEGAL_SCOPE`, `ILLEGAL_SERVICE`, `ILLEGAL_USAGE_TYPE`, `ILLEGAL_OPERATION`, `ILLEGAL_BILLING_ENTITY` and `INVALID_SKU_COMBO` — a per-member vocabulary that only exists because the service judges which members a scope may carry.

Handler: `src/ops/create_pricing_rule.rs` lines ~240-275 — the only Scope-keyed checks are `matches!(input.scope.as_str(), "SERVICE" | "SKU") && input.service.is_none()`, `input.scope == "SKU"` with `usage_type.is_none()` / `operation.is_none()`, and `input.scope == "BILLING_ENTITY" && input.billing_entity.is_none()`. There is no check that a member the scope does NOT key on is absent; lines ~334-350 then store `service`, `usage_type`, `operation` and `billing_entity` verbatim, and `src/ops/list_pricing_rules.rs` `element()` (lines ~117-141) reports them all back.

Tests: `tests/all/pricing_rule_core_loop.rs::a_combination_missing_a_member_it_keys_on_is_refused` (lines 339-449) covers only the omission direction and its own doc comment says so: "The model states no condition that REFUSES one of these members on another combination, so this suite asserts only the direction the documentation states." So nothing pins the current acceptance, and no test asserts the refusal. (The earlier, already-fixed CreatePricingRule validation issue was about a rule carrying BOTH `ModifierPercentage` and `Tiering` — a different member pair and a different rule.)

## Steps to Reproduce

1. Make the emulated account the payer (the crate's payer seam) so the create is reachable rather than 403.
2. POST /create-pricing-rule with {"Name":"GlobalWithService","Scope":"GLOBAL","Type":"MARKUP","ModifierPercentage":5.0,"Service":"AmazonEC2"}.
3. Observe HTTP 200 and a minted Arn instead of a 400 ValidationException naming Service (reason ILLEGAL_SERVICE / ILLEGAL_SCOPE).
4. POST /list-pricing-rules with {} and read the element for that ARN: it reports Scope=GLOBAL together with Service=AmazonEC2 — a rule state AWS does not create.
5. Repeat with {"Name":"ServiceWithSku","Scope":"SERVICE","Type":"DISCOUNT","ModifierPercentage":5.0,"Service":"AmazonEC2","UsageType":"USW2-BoxUsage:m2.2xlarge","Operation":"RunInstances:0202"} and with a GLOBAL rule carrying "BillingEntity":"AWS": both are stored and echoed.
