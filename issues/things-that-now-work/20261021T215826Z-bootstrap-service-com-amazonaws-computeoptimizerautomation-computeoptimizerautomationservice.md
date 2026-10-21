Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the Compute Optimizer Automation emulator

_Filed: 2026-10-21T21:58:26Z_
_Operation: service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice | Class: stub_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 23 handlers under `src/ops/` is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding zero state, so no client can do anything against this service. Implement a FIRST, SMALL stateful slice: the enrollment + automation-rule lifecycle, which is the service's primary resource.

Happy path to implement, in order (wire protocol: awsJson1_0, `POST /` with `X-Amz-Target: ComputeOptimizerAutomationService.<OperationName>`):

1. **UpdateEnrollmentConfiguration** — input `status` (EnrollmentStatus: Active|Failed|Inactive|Pending, required), `clientToken`. Must persist the enrollment status for the calling account and return `status`, optional `statusReason`, and `lastUpdatedTimestamp` (both `status` and `lastUpdatedTimestamp` are @required in the output) read back from what was just stored — the timestamp must be the actual store time, not a constant.
2. **GetEnrollmentConfiguration** — no input. Must return the status previously persisted by UpdateEnrollmentConfiguration (`status`, `statusReason`, `organizationRuleMode`, `lastUpdatedTimestamp`). Before any enrollment call it must report the default (`Inactive`), never a hardcoded `Active`.
3. **CreateAutomationRule** — input `name` (required, len 0..128, `^[a-zA-Z0-9_-]*$`), `ruleType` (required, AccountRule|OrganizationRule), `recommendedActionTypes` (required), `schedule` (required), `status` (required, Active|Inactive), plus optional `description`, `organizationConfiguration`, `priority`, `criteria`, `tags` (<=200), `clientToken`. Must mint and store a new rule: generate `ruleId` matching `^[0-9A-Za-z]{16}$`, `ruleArn` matching `^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$` (use the request's account id), set `ruleRevision` to its initial value and `createdTimestamp` to now, and echo back the stored values (not the raw input struct).
4. **GetAutomationRule** — input `ruleArn` (required). Must look the rule up in the store and return every stored field (`ruleArn`, `ruleId`, `name`, `description`, `ruleType`, `ruleRevision`, `accountId`, `organizationConfiguration`, `priority`, `recommendedActionTypes`, `criteria`, `schedule`, `status`, `tags`, `createdTimestamp`, `lastUpdatedTimestamp`). An unknown or deleted ARN must be `ResourceNotFoundException` (HTTP 404), and a syntactically invalid ARN `InvalidParameterValueException` (HTTP 400).
5. **UpdateAutomationRule** — input `ruleArn` + `ruleRevision` (both required) plus the mutable fields. Must apply only the supplied fields to the stored rule, bump `ruleRevision`, set `lastUpdatedTimestamp`, and return the merged result. A stale/mismatched `ruleRevision` must fail (InvalidParameterValueException) rather than silently overwrite — `ruleRevision` is the service's optimistic-concurrency token (it is @required on Update/Delete/Tag/Untag).
6. **ListAutomationRules** — input `filters`, `maxResults`, `nextToken`. Must return the rules actually in the store (@paginated items=automationRules), honour `maxResults` by truncating and emitting an opaque `nextToken` matching `^[A-Za-z0-9+/=]+$` that resumes exactly where the page stopped, and return no `nextToken` on the final page. An empty store must yield an empty list, not a canned sample rule.
7. **DeleteAutomationRule** — input `ruleArn` + `ruleRevision` (required). Must remove the rule from the store (revision-checked) and return an empty body; a second Delete and any later Get of the same ARN must then be `ResourceNotFoundException` (404).

Every one of these must compute its response from persisted state. No canned responses, no echoing the input back without storing it, and no generating a ruleId/ruleArn that is never recorded — a Create followed by a Get/List must return exactly what was created, and a Delete must be observable by the next reader.

## State Model

In `src/state.rs` (reached via `crate::state::service_state(&*self.engine)`, one shared store per engine):

- `enrollment: EnrollmentConfig { status: EnrollmentStatus (default Inactive), status_reason: Option<String>, organization_rule_mode: Option<OrganizationRuleMode>, last_updated_timestamp: Option<DateTime> }` — one per account, keyed by the caller's account id.
- `rules: Map<String /* ruleArn */, AutomationRule { rule_arn: String, rule_id: String /* 16 alnum */, account_id: String, name: String, description: Option<String>, rule_type: RuleType, rule_revision: i64, organization_configuration: Option<OrganizationConfiguration>, priority: Option<String>, recommended_action_types: Vec<RecommendedActionType>, criteria: Option<Criteria>, schedule: Schedule, status: RuleStatus, tags: Vec<Tag>, created_timestamp: DateTime, last_updated_timestamp: Option<DateTime> }>` — insertion order preserved so List pagination is stable.
- `rule_ids: Map<String /* ruleId */, String /* ruleArn */>` — secondary index so ruleId-keyed lookups (events reference `ruleId`) resolve without a scan.
- `idempotency: Map<String /* clientToken */, (OperationName, RequestHash, StoredResponse)>` — the @idempotent/@idempotencyToken operations replay the stored response for a repeated token and raise `IdempotentParameterMismatchException` (409) when the same token arrives with different parameters.
- `next_token_cursors: Map<String /* opaque nextToken */, Cursor { offset: usize, filter_fingerprint: String }>` — backs real pagination for ListAutomationRules (and is reused by the other paginated list ops later).

All 16 other operations (AssociateAccounts, DisassociateAccounts, ListAccounts, Start/Rollback/Get AutomationEvent, ListAutomationEvents*, ListRecommendedActions*, ListAutomationRulePreview*, TagResource, UntagResource, ListTagsForResource) stay at 501 for now and will be filed as individual gap issues — keep this first slice small.

Done = an integration test that enrolls, creates a rule, gets it back field-for-field, updates it and observes the bumped revision + new lastUpdatedTimestamp, lists it (including a maxResults=1 paged walk over two rules), deletes it, and gets a 404 ResourceNotFoundException afterwards.

## Evidence

OPERATIONS.md for this crate documents all 23 operations with the exact input/output members cited above (CreateAutomationRule required members and the ruleArn/ruleId patterns, GetAutomationRule's full output set, ListAutomationRules' @paginated(inputToken=nextToken, outputToken=nextToken, items=automationRules), Delete/Update/Tag/Untag's required `ruleRevision`, Get/UpdateEnrollmentConfiguration's status fields and required output `lastUpdatedTimestamp`, and the ResourceNotFoundException (404) / InvalidParameterValueException (400) / IdempotentParameterMismatchException (409) error sets). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:'` over `src/ops/` matches all 23 handler files, each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). README.md gives the wire protocol (awsJson1_0, X-Amz-Target) and the shared-engine state API. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `aco-automation` (awsJson1_0, POST / with X-Amz-Target: ComputeOptimizerAutomationService.<Op>) at it.
2. Call UpdateEnrollmentConfiguration with status=Active, then GetEnrollmentConfiguration — expect status=Active with the lastUpdatedTimestamp from the update; today both return HTTP 501 NotImplemented.
3. Call CreateAutomationRule with name="my-rule", ruleType=AccountRule, recommendedActionTypes=[...], schedule={...}, status=Active — expect a stored rule with a 16-char ruleId and an arn:aws:compute-optimizer::<acct>:automation-rule/... ruleArn.
4. Call GetAutomationRule with that ruleArn — expect every field created in step 3 echoed from state.
5. Call UpdateAutomationRule with the returned ruleArn and ruleRevision, changing status=Inactive — expect a bumped ruleRevision and a lastUpdatedTimestamp; re-sending the old ruleRevision must fail rather than apply.
6. Create a second rule, then call ListAutomationRules with maxResults=1 and follow nextToken — expect exactly the two created rules across two pages and no nextToken on the last page.
7. Call DeleteAutomationRule with ruleArn + current ruleRevision, then GetAutomationRule on the same ARN — expect ResourceNotFoundException (HTTP 404).
