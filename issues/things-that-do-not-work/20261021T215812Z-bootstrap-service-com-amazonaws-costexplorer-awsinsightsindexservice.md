Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the service-com-amazonaws-costexplorer-awsinsightsindexservice emulator

_Filed: 2026-10-21T21:58:12Z_
_Operation: service-com-amazonaws-costexplorer-awsinsightsindexservice | Class: stub_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 47 operations in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the service holds no state at all. This issue defines the FIRST buildable slice: the stateful Cost Category definition lifecycle, which is the only create -> read -> list -> update -> delete resource in Cost Explorer (every other operation is a read-only cost/usage analytic or a recommendation generator).

Wire protocol: `awsJson1_1`, `POST /` with `X-Amz-Target: AWSInsightsIndexService.<OperationName>`, sigv4 signing name `ce`.

Implement these 5 operations, in this order, each computing its response from persisted state (NO canned responses, NO echoing the request, NO minting an ARN without storing the resource behind it):

1. **CreateCostCategoryDefinition** — input `Name` (required, 1..50), `RuleVersion` (required, enum `CostCategoryExpression.v1`), `Rules` (required, 1..500), optional `EffectiveStart`, `DefaultValue`, `SplitChargeRules`, `ResourceTags`. Must mint and STORE a new definition under a generated ARN matching `^arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$` (e.g. `arn:aws:ce::123456789012:costcategory/<uuid>`), default `EffectiveStart` to the first day of the current month in `YYYY-MM-DDTHH:MM:SSZ` form when absent, and return `CostCategoryArn` + the stored `EffectiveStart`.
2. **DescribeCostCategoryDefinition** — input `CostCategoryArn` (required), optional `EffectiveOn`. Must look the ARN up in state and return the full `CostCategory` structure built from what was stored: `CostCategoryArn`, `Name`, `EffectiveStart`, `EffectiveEnd` (only once deleted), `RuleVersion`, `Rules`, `SplitChargeRules`, `DefaultValue`. An unknown or deleted ARN must return the modeled `ResourceNotFoundException` (HTTP 404), never 200 with an empty body.
3. **ListCostCategoryDefinitions** — input optional `EffectiveOn`, `MaxResults` (1..100), `NextToken`, `SupportedResourceTypes`. Must return one `CostCategoryReference` per stored, non-deleted definition (`CostCategoryArn`, `Name`, `EffectiveStart`, `EffectiveEnd`, `NumberOfRules` = `Rules.len()`, `DefaultValue`, `Values`), honour `MaxResults` and emit/consume `NextToken` per the `@paginated(inputToken=NextToken, outputToken=NextToken, items=CostCategoryReferences)` trait.
4. **UpdateCostCategoryDefinition** — input `CostCategoryArn` (required), `RuleVersion` (required), `Rules` (required), optional `EffectiveStart`, `DefaultValue`, `SplitChargeRules`. Must mutate the STORED definition so a following Describe/List reflects the new rules and default value, return `CostCategoryArn` + effective `EffectiveStart`, and return `ResourceNotFoundException` (404) for an unknown ARN.
5. **DeleteCostCategoryDefinition** — input `CostCategoryArn` (required). Must remove (or tombstone with `EffectiveEnd` = now) the stored definition, return `CostCategoryArn` + `EffectiveEnd`, and `ResourceNotFoundException` (404) when the ARN was never created or was already deleted. After it, Describe must 404 and List must not include the ARN.

Done = a client script can run the sequence above against a single running emulator and observe its own data back at every step: the Name/Rules it created, the updated Rules after Update, the count shrinking in List after Delete, and a 404 on Describe afterwards. 2xx alone is not done.

## State Model

In `src/state.rs` on `ComAmazonawsCostexplorerAwsinsightsindexserviceState` (reached only via `crate::state::service_state(&*self.engine)`; never construct an engine in a handler):

- `cost_categories: BTreeMap<String /* CostCategoryArn */, CostCategoryDefinition>` — insertion-ordered-by-ARN registry, the single source of truth for all five operations.
- `CostCategoryDefinition { arn: String, name: String, rule_version: CostCategoryRuleVersion, rules: Vec<CostCategoryRule>, split_charge_rules: Option<Vec<CostCategorySplitChargeRule>>, default_value: Option<String>, resource_tags: Vec<ResourceTag>, effective_start: String /* ZonedDateTime, 1st of month, `Z`-suffixed */, effective_end: Option<String>, supported_resource_types: Vec<String> }`
- `names: BTreeSet<String>` (or a derived lookup over `cost_categories`) so a duplicate `Name` can be refused and so List/Describe need no re-derivation of identity.
- `list_tokens: BTreeMap<String /* NextToken */, usize /* offset into the ARN ordering */>` — opaque pagination cursors for ListCostCategoryDefinitions.

All other 42 operations (GetCostAndUsage, anomaly monitors/subscriptions, savings-plans and reservation analytics, tagging, backfills) stay at 501 in this slice and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md (crate root) lines 39-43 (CreateCostCategoryDefinition input/output/errors), 57-61 (DeleteCostCategoryDefinition), 63-67 (DescribeCostCategoryDefinition), 237-242 (ListCostCategoryDefinitions incl. the @paginated trait), 318-322 (UpdateCostCategoryDefinition); lines 8-15 give the awsJson1_1 / X-Amz-Target: AWSInsightsIndexService.* wire contract. The Smithy model's `CostCategory` shape requires `CostCategoryArn`, `EffectiveStart`, `Name`, `RuleVersion`, `Rules`, and `CostCategoryReference` carries `NumberOfRules`/`Values`/`EffectiveEnd` — so Describe/List cannot be answered without stored state. Proof of the all-stub state: `src/ops/create_cost_category_definition.rs` line 4 is `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 181 is `Err(crate::scaffolded::Scaffolded::not_implemented("CreateCostCategoryDefinition"))`; every other file in `src/ops/` is the same generated stub. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `ce` client at it.
2. CreateCostCategoryDefinition with Name="demo-cc", RuleVersion="CostCategoryExpression.v1" and one rule — expect 200 with a syntactically valid CostCategoryArn and an EffectiveStart; today: HTTP 501 NotImplemented.
3. DescribeCostCategoryDefinition with that ARN — expect 200 whose CostCategory has Name="demo-cc", the rule just submitted, and the same EffectiveStart.
4. ListCostCategoryDefinitions — expect the new ARN present once with NumberOfRules=1; repeat with MaxResults=1 and follow NextToken to confirm pagination.
5. UpdateCostCategoryDefinition on that ARN with a second rule and DefaultValue="Other" — expect 200, then DescribeCostCategoryDefinition must show 2 rules and DefaultValue="Other".
6. DeleteCostCategoryDefinition on that ARN — expect 200 with CostCategoryArn + EffectiveEnd.
7. DescribeCostCategoryDefinition on the deleted ARN — expect ResourceNotFoundException (HTTP 404); ListCostCategoryDefinitions must no longer include it.
8. DescribeCostCategoryDefinition with a never-created ARN — expect ResourceNotFoundException (HTTP 404), not 501 and not an empty 200.
