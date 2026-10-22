Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-recommended-actions-error-invalid-filter-key
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-recommended-actions-error-invalid-filter-key bcm-recommended-actions-error-invalid-match-option bcm-recommended-actions-error-malformed-next-token bcm-recommended-actions-filter-by-feature-multi-value bcm-recommended-actions-filter-by-multiple-criteria bcm-recommended-actions-filter-by-severity-not-equals
Issue-Shape-Id: com.amazonaws.bcmrecommendedactions#AWSBillingAndCostManagementRecommendedActions
Issue-Key: bcm-recommended-actions|ListRecommendedActions|4xx->4xx|$.fieldList
Issue-Key: bcm-recommended-actions|ListRecommendedActions|4xx->4xx|$.message
Issue-Key: bcm-recommended-actions|ListRecommendedActions|4xx->4xx|$.reason
Issue-Key: bcm-recommended-actions|ListRecommendedActions|4xx->4xx|$headers.content-type
Issue-Key: bcm-recommended-actions|ListRecommendedActions|2xx->2xx|$headers.content-type
Issue-Key: bcm-recommended-actions|ListRecommendedActions|2xx->2xx|$.recommendedActions[*]
Issue-Commit: 7d2cd57c0bb94f32b894957a728316a26e29b449

# ListRecommendedActions diverges on content-type, ValidationException shape and NOT_EQUALS filtering

> _Note: this issue was identified against the code base at commit `7d2cd57c0bb94f32b894957a728316a26e29b449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRecommendedActions

## Description
The emulator's only BCM Recommended Actions operation, ListRecommendedActions, answers on the wire but diverges from the recorded AWS responses in three independent ways. This bundle carries 3 CAUSES; all three must be closed before the issue is resolved.

Cause 1 of 3: ValidationException body omits the modeled `reason` and `fieldList` members.
  AWS: Smithy (com.amazonaws.bcmrecommendedactions) models ValidationException with members `message` (required), `reason` (required, enum ValidationExceptionReason = unknownOperation | cannotParse | fieldValidationFailed | other) and `fieldList` (list of ValidationExceptionField{name required, message required}). The recorded response for an ActionFilter whose `key` is outside FilterName (enum values FEATURE, SEVERITY, TYPE) is HTTP 400 ValidationException carrying reason='fieldValidationFailed' and a one-element fieldList whose `name` is the offending request path (e.g. 'filter.actions.1.member.key') and whose `message` restates the enum constraint.
  Emulator: returns HTTP 400 (status matches) but the JSON body contains only `message`: `$.reason` is absent and `$.fieldList` is absent entirely.
  Why: a client deserializing ValidationException gets null for a member the model marks required, so `reason`-based branching and per-field error reporting (which field was rejected, and why) are impossible; SDKs/validators that enforce required members on the error shape can fail to parse the error at all.
  Fix: the ListRecommendedActions input validation / ValidationException serializer for this service — populate `reason` and build one `fieldList` entry per rejected member.
  Note on the message text: the recorded and emulated `message` strings also differ (the recording shows a redacted "Value at 'filter.actions.1.member.key' failed to satisfy constraint: Member must satisfy enum value set: [FEATURE, TYPE, SEVERITY]", the emulator shows "Value 'NOT_A_REAL_FILTER_KEY' at ... [FEATURE, SEVERITY, TYPE]"). Treat the recorded text as an EXAMPLE, not a requirement; the requirement is the "N validation error detected: ... failed to satisfy constraint: Member must satisfy enum value set: [...]" shape built from the request member path, plus the presence of `reason`/`fieldList`. Traces: bcm-recommended-actions-error-invalid-filter-key#task#1 (cluster also covers bcm-recommended-actions-error-invalid-match-option#task#1, the same gap for the MatchOption enum: EQUALS | NOT_EQUALS).

Cause 2 of 3: response Content-Type header is application/x-amz-json-1.0 instead of the recorded application/x-amz-json-1.1.
  AWS: recorded behavior — every recorded response for this service, success and error alike, carries `content-type: application/x-amz-json-1.1`. Hypothesis: the service front end emits the 1.1 media type even though the Smithy model declares the awsJson1_0 protocol trait; the recording, not the trait, is what clients observe.
  Emulator: emits `content-type: application/x-amz-json-1.0` on all responses (seen on the 200 list responses and on the 400 error response).
  Why: clients and proxies that key response parsing, content negotiation or protocol assertion off the response media type see a different protocol version than AWS announces, so strict parsers and recorded-contract tests reject otherwise-correct payloads.
  Fix: the service's response serializer / HTTP response header construction (shared by success and error paths), not any single handler.
  Traces: bcm-recommended-actions-error-malformed-next-token#task#1, bcm-recommended-actions-filter-by-feature-multi-value#task#1 (cluster also covers bcm-recommended-actions-filter-by-multiple-criteria#task#1), bcm-recommended-actions-filter-by-severity-not-equals#task#1.

Cause 3 of 3: an ActionFilter with matchOption NOT_EQUALS returns nothing instead of the complement of the matched set.
  AWS: Smithy ActionFilter.matchOption (enum MatchOption) documents "Use EQUALS to include matching results or NOT_EQUALS to exclude matching results"; ListRecommendedActionsResponse.recommendedActions is required. The recording for a severity NOT_EQUALS filter returns HTTP 200 with a non-empty recommendedActions list whose entries are exactly the actions whose `severity` does NOT equal the filter value (the recorded first element is a RecommendedAction with severity 'INFO').
  Emulator: returns HTTP 200 but `$.recommendedActions[0]` is missing — the list comes back empty, i.e. NOT_EQUALS excluded every stored action rather than only those matching the filter values.
  Why: a client enumerating actions while excluding a severity/feature/type sees zero results and concludes the account has no recommended actions, so exclusion-style queries silently lose all data (and pagination over them never yields a page).
  Fix: the filter-evaluation logic in the ListRecommendedActions handler / state query — invert the predicate per filter term for NOT_EQUALS instead of dropping all candidates.
  Traces: bcm-recommended-actions-filter-by-severity-not-equals#task#1.

Not filed, deliberately: the `message` difference on the malformed-nextToken fixture (recorded "Pagination token exception in operation 'ListRecommendedActions': invalid payload wrapping schema -24949" vs the emulator's "The nextToken is not valid. Pass back the nextToken returned by the previous call, unchanged."). Both sides return HTTP 400 ValidationException and the recorded text embeds an internal, run-specific value, so the message alone is not a requirement; only Cause 2 applies to that step.

## Evidence
Replay of trace set simparity-wire-aws-v1: 6 steps replayed, 0 passed, 6 mismatched, 4 divergence clusters, all service=bcm-recommended-actions / operation=ListRecommendedActions, emulator_up=true, state_reset_failures=0, bootstrap_signal=false.

Cluster 1 — fixture bcm-recommended-actions-error-invalid-filter-key, step task#1 (also bcm-recommended-actions-error-invalid-match-option#task#1), expected 400 / actual 400:
  $.reason: missing (expected 'fieldValidationFailed', got None)
  $.fieldList: missing (expected [{'name': 'filter.actions.1.member.key', 'message': "Value [redacted] failed to satisfy constraint: Member must satisfy enum value set: [FEATURE, TYPE, SEVERITY]"}], got None)
  $.message: value_mismatch (recorded text redacted; example only)

Cluster 2 — fixture bcm-recommended-actions-error-malformed-next-token, step task#1, expected 400 / actual 400:
  $headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/x-amz-json-1.0')
  $.message: value_mismatch (recorded text carries an internal value '-24949'; example only, not filed)

Cluster 3 — fixture bcm-recommended-actions-filter-by-feature-multi-value, step task#1 (also bcm-recommended-actions-filter-by-multiple-criteria#task#1), expected 200 / actual 200:
  $headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/x-amz-json-1.0')

Cluster 4 — fixture bcm-recommended-actions-filter-by-severity-not-equals, step task#1, expected 200 / actual 200:
  $.recommendedActions[0]: missing (expected {'accountId': '000000000000', 'severity': 'INFO'}, got None)
  $headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/x-amz-json-1.0')

Model evidence (smithy_query on services/service-com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions/smithy-model/com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions.json): protocol awsJson1_0; ValidationException.reason required (enum: unknownOperation, cannotParse, fieldValidationFailed, other), ValidationException.fieldList -> ValidationExceptionFieldList; FilterName enum = [FEATURE, SEVERITY, TYPE]; MatchOption enum = [EQUALS, NOT_EQUALS] with doc "Use EQUALS to include matching results or NOT_EQUALS to exclude matching results"; ListRecommendedActionsResponse.recommendedActions required; ValidationException httpError 400.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (all 6 steps are bcm-recommended-actions ListRecommendedActions).
3. Cause 1 — replay fixture bcm-recommended-actions-error-invalid-filter-key, step task#1 (and bcm-recommended-actions-error-invalid-match-option#task#1): POST an AWSBillingAndCostManagementRecommendedActions.ListRecommendedActions request whose filter.actions[1].key is not a FilterName member (e.g. 'NOT_A_REAL_FILTER_KEY'). Response is 400 ValidationException; inspect the JSON body: `reason` and `fieldList` are absent where the recording has reason='fieldValidationFailed' and a one-entry fieldList naming 'filter.actions.1.member.key'.
4. Cause 2 — replay bcm-recommended-actions-error-malformed-next-token#task#1, bcm-recommended-actions-filter-by-feature-multi-value#task#1, bcm-recommended-actions-filter-by-multiple-criteria#task#1 and bcm-recommended-actions-filter-by-severity-not-equals#task#1 and read the response `content-type` header on both the 200 and the 400: it is application/x-amz-json-1.0 where every recording has application/x-amz-json-1.1.
5. Cause 3 — replay bcm-recommended-actions-filter-by-severity-not-equals#task#1: call ListRecommendedActions with one ActionFilter {key: SEVERITY, matchOption: NOT_EQUALS, values: [<a severity present in the seeded data>]}. Response is 200 with an empty recommendedActions list; the recording returns the complement, whose first element is a RecommendedAction with severity 'INFO'.
6. Re-run the replay after fixing: all 6 steps must pass, not just the representative step.
