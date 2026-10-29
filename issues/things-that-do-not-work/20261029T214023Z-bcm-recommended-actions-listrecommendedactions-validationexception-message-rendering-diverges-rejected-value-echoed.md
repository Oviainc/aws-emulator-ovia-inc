Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-recommended-actions.empty-filter-values-list-rejected bcm-recommended-actions.max-results-out-of-range-rejected bcm-recommended-actions.filter-invalid-key-rejected bcm-recommended-actions.invalid-filter-key-rejected bcm-recommended-actions.filter-invalid-match-option-rejected bcm-recommended-actions.invalid-match-option-rejected bcm-recommended-actions.filter-match-option-wrong-case-rejected bcm-recommended-actions.next-token-invalid-rejected
Issue-Shape-Id: com.amazonaws.bcmrecommendedactions#AWSBillingAndCostManagementRecommendedActions
Issue-Key: bcm-recommended-actions.empty-filter-values-list-rejected|run#6|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: bcm-recommended-actions.max-results-out-of-range-rejected|run#5|AssertionError|assert error_low.get("Message") == expected_low, (
Issue-Key: bcm-recommended-actions.filter-invalid-key-rejected|run#7|AssertionError|assert error["Error"].get("Message") == expected_message, (
Issue-Key: bcm-recommended-actions.invalid-filter-key-rejected|run#6|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: bcm-recommended-actions.filter-invalid-match-option-rejected|run#7|AssertionError|assert error["Error"].get("Message") == expected_message, (
Issue-Key: bcm-recommended-actions.invalid-match-option-rejected|run#6|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: bcm-recommended-actions.filter-match-option-wrong-case-rejected|run#5|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: bcm-recommended-actions.next-token-invalid-rejected|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 7240e32f2500d20910730c71d4ad0714ff1f3217

# ListRecommendedActions ValidationException message rendering diverges: rejected value echoed in the summary clause, enum value-set order, and nextToken wording

> _Note: this issue was identified against the code base at commit `7240e32f2500d20910730c71d4ad0714ff1f3217`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRecommendedActions

## Description
ListRecommendedActions refuses every malformed input with the right HTTP status (400), the right error shape (ValidationException with required `message`/`reason` and a `fieldList`) and the right `reason` (fieldValidationFailed) — but the human-readable `message`, which is part of the error contract, is worded differently from AWS in three independent ways. Three causes, all in this one operation's refusal rendering, bundled because they share one service and one operation.

RELATED OPEN ISSUE: 20261029T211113Z-bcm-recommended-actions-listrecommendedactions-error-messages-diverge-enum-value-set-order describes the enum-order and nextToken wording for the WIRE corpus and carries wire replay keys only (fixture#step bindings for bcm-recommended-actions-error-invalid-filter-key / -invalid-match-option / -malformed-next-token). It does NOT carry the behavioral check ids below, and its cause 1/2 are scoped to the two enum members only — it says nothing about the @length and @range constraint clauses, which diverge the same way. A fixer closing that issue would not re-replay these checks, so they are filed here with their own replay list. The other open issues on this service are unrelated (…-maintainability is about the unrouted-operation 404/501 default; …-wrong-behavior--r4 is about the boot inventory's lastUpdatedTimeStamp).

Cause 1 of 3: the top-level summary `message` interpolates the rejected value, where AWS's summary clause names only the member path. This is NOT limited to enum members — it is how this service renders EVERY constraint violation, and it diverges for the enum, @length and @range classes alike.
  AWS: Smithy (model com-amazonaws-bcmrecommendedactions): ListRecommendedActions$maxResults targets MaxResults with @range min 1 max 1000; ActionFilter$values targets FilterValues with @length min 1, @required; ActionFilter$key targets enum FilterName, ActionFilter$matchOption targets enum MatchOption, both @required. All of these are refused with ValidationException (errorKind client, httpError 400). The model does not state the message text, so the wording is recorded behavior only. The recording shows, for a single refused member, the summary message `N validation error(s) detected: Value at '<member path>' failed to satisfy constraint: <constraint clause>` — with NO rejected value in it. Fixed text of this message class: the `N validation error detected: ` envelope, the literal frame `Value at '` / `' failed to satisfy constraint: `, and the constraint clause itself (`Member must have length greater than or equal to <min>`, `Member must have value greater than or equal to <min>`, `Member must have value less than or equal to <max>`, `Member must satisfy enum value set: [...]`). Variable parts taken from the request and the model: the error count, the dotted member path (for a list element, the `filter.actions.1.member.<field>` form), and the bound or value set from the model. The rejected value appears only in the per-field `fieldList` entry, never in the summary.
  Emulator: emits the same frame but splices the rejected value into the summary: `1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1`; `… Value '0 items' at 'filter.actions.1.member.values' failed to satisfy constraint: Member must have length greater than or equal to 1` (note it synthesises a descriptor, `'0 items'`, that is not even the submitted value); `… Value 'BOGUS_KEY' at 'filter.actions.1.member.key' …`; `… Value 'CONTAINS' at 'filter.actions.1.member.matchOption' …`; `… Value 'equals' at 'filter.actions.1.member.matchOption' …`.
  Why: a client, SDK test or operator that matches or displays AWS's validation summary sees a different sentence for every rejected request, so assertions pinned to AWS's wording fail and a diff of emulator vs AWS output shows a spurious difference on every refusal. It also leaks the caller's submitted value into a field AWS keeps value-free.
  Fix: the operation's refusal rendering — `field_validation_failed` in the ListRecommendedActions handler and the framework violation clause it interpolates (`constraints::envelope` for the summary, `Violation::message()` for the per-field clause) reached through the ModelledValidation seam. The summary clause must drop the value interpolation while the fieldList clause keeps it.
  Traces: bcm-recommended-actions.max-results-out-of-range-rejected, bcm-recommended-actions.empty-filter-values-list-rejected, bcm-recommended-actions.filter-invalid-key-rejected, bcm-recommended-actions.invalid-filter-key-rejected, bcm-recommended-actions.filter-invalid-match-option-rejected, bcm-recommended-actions.invalid-match-option-rejected, bcm-recommended-actions.filter-match-option-wrong-case-rejected. Code: services/service-com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions/src/ops/list_recommended_actions.rs:219-241.

Cause 2 of 3: the enum value-set listing is emitted in the model's declaration order instead of the per-member order AWS uses.
  AWS: Smithy: FilterName values FEATURE, SEVERITY, TYPE; MatchOption values EQUALS, NOT_EQUALS. The message text is recorded behavior only, and the recording shows AWS listing these sets as `[FEATURE, TYPE, SEVERITY]` for ActionFilter$key and `[NOT_EQUALS, EQUALS]` for ActionFilter$matchOption. The listing order is therefore a fixed, per-member property of AWS's message — it is neither the Smithy declaration order nor a sort, so it must be carried explicitly per member and cannot be derived.
  Emulator: lists `[FEATURE, SEVERITY, TYPE]` and `[EQUALS, NOT_EQUALS]` — i.e. the `ALL` constant in src/state.rs, which is documented as "every member the model declares, in model order".
  Why: the message tells a caller which values the service accepts; a caller or test comparing that listing against AWS's sees a different set spelling on every out-of-enum refusal. It also demonstrates that a fix which sorts or reuses model order cannot satisfy both members at once.
  Fix: same component as cause 1, plus the per-member value listing the enum clause interpolates (`member_strings` over `FilterName::ALL` / `MatchOption::ALL` in src/state.rs) — the order must be AWS's for each member, not the enum's declaration order.
  Traces: bcm-recommended-actions.filter-invalid-key-rejected, bcm-recommended-actions.invalid-filter-key-rejected, bcm-recommended-actions.filter-invalid-match-option-rejected, bcm-recommended-actions.invalid-match-option-rejected, bcm-recommended-actions.filter-match-option-wrong-case-rejected. Code: src/state.rs:119-150 (ALL / member_strings) and src/ops/list_recommended_actions.rs:219-241.

Cause 3 of 3: a nextToken this service never minted is refused with the emulator's own sentence instead of AWS's pagination-token message.
  AWS: Smithy: ListRecommendedActions is @paginated (inputToken/outputToken nextToken); NextToken is a string with @length 1..8192 and pattern ^[\S\s]*$, so a token that satisfies those constraints but was not minted by the service is refused by the handler rather than by constraint validation, with ValidationException (400). Message text is recorded behavior only: `Pagination token exception in operation '<OperationName>': invalid payload wrapping schema <n>`. Fixed text: the prefix `Pagination token exception in operation '` and the separator `': invalid payload wrapping schema `. Variable parts: the operation name from the request's routing key, and a small integer derived from the supplied token's decoded payload — the recorded run shows -24949 for its token; that number is this run's derived value and must be computed from the token, never hard-coded.
  Emulator: answers `The nextToken is not valid. Pass back the nextToken returned by the previous call, unchanged.` (status 400 and the error shape are otherwise correct; only the message diverges).
  Why: the two sentences are not interchangeable — a client or operator keying on AWS's pagination-token message class, or parsing the operation name out of it, does not recognise the emulator's wording, and cannot tell from it that the token failed payload decoding rather than, say, naming a different selection.
  Fix: the ListRecommendedActions handler's token parsing — the `malformed` refusal in `parse_token` must word its message the way AWS words a pagination-token decode failure, naming the operation and the decoded-payload discriminator.
  Traces: bcm-recommended-actions.next-token-invalid-rejected. Code: src/ops/list_recommended_actions.rs (parse_token / malformed refusal).

## Evidence
Behavioral replay simparity-behavioral-aws-v1: 35/47 passed; all 12 failures are service bcm-recommended-actions, operation ListRecommendedActions. Eight of them are this issue; every one of the eight got HTTP 400 and error Code ValidationException as expected — only the message text diverged.

Cause 1 (value echoed into the summary clause):
- bcm-recommended-actions.max-results-out-of-range-rejected (maxResults=0)
  expected: "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1"
  actual:   "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1"
  (the same check also asserts maxResults=1001 -> "... Value at 'maxResults' ... Member must have value less than or equal to 1000")
- bcm-recommended-actions.empty-filter-values-list-rejected (filter.actions[0].values = [])
  expected: "1 validation error detected: Value at 'filter.actions.1.member.values' failed to satisfy constraint: Member must have length greater than or equal to 1"
  actual:   "1 validation error detected: Value '0 items' at 'filter.actions.1.member.values' failed to satisfy constraint: Member must have length greater than or equal to 1"
- bcm-recommended-actions.filter-invalid-key-rejected / .invalid-filter-key-rejected
  actual summaries carry "Value 'BOGUS_KEY' at '…'" and "Value 'NOT_A_REAL_KEY' at '…'" where AWS carries "Value at '…'".
- bcm-recommended-actions.filter-invalid-match-option-rejected / .invalid-match-option-rejected / .filter-match-option-wrong-case-rejected
  actual summaries carry "Value 'CONTAINS' at '…'" and "Value 'equals' at '…'".
Fixed text vs values: the envelope, the `Value at '`/`' failed to satisfy constraint: ` frame and the constraint clause are fixed; the error count, the member path and the bound come from the request and the model. 'BOGUS_KEY', 'NOT_A_REAL_KEY', 'CONTAINS', 'equals', '0', '0 items' and 'filter.actions.1.member.key' are these recordings' request values, not requirements.

Cause 2 (enum value-set order):
- filter key: expected listing "[FEATURE, TYPE, SEVERITY]", actual "[FEATURE, SEVERITY, TYPE]" (bcm-recommended-actions.filter-invalid-key-rejected, .invalid-filter-key-rejected).
- matchOption: expected listing "[NOT_EQUALS, EQUALS]", actual "[EQUALS, NOT_EQUALS]" (bcm-recommended-actions.filter-invalid-match-option-rejected, .invalid-match-option-rejected, .filter-match-option-wrong-case-rejected).
Model cross-check (smithy_query shape FilterName/MatchOption): FEATURE, SEVERITY, TYPE and EQUALS, NOT_EQUALS — i.e. the emulator reproduces model order, AWS does not.
The wrong-case check also confirms MatchOption is case-sensitive: 'equals' IS refused with ValidationException (correct), only the message diverges.

Cause 3 (nextToken wording), bcm-recommended-actions.next-token-invalid-rejected with nextToken="not-a-real-continuation-token-0123456789":
  expected: "Pagination token exception in operation 'ListRecommendedActions': invalid payload wrapping schema -24949"
  actual:   "The nextToken is not valid. Pass back the nextToken returned by the previous call, unchanged."
  (Fixed text: "Pagination token exception in operation '" … "': invalid payload wrapping schema "; variables: the operation name and an integer derived from the supplied token. -24949 is that token's derived value.)

Every one of the eight checks also runs a positive control on the same client (a well-formed filter / in-range maxResults / no token) and the control succeeds, so the refusal path, not the happy path, is what diverges.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite (simparity-behavioral-aws-v1).
2. Cause 1, @range: call ListRecommendedActions with maxResults=0 using a client with botocore parameter_validation disabled (otherwise the SDK refuses locally). Compare Error.Message against AWS's "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1" — the emulator interpolates the rejected value. Repeat with maxResults=1001 for the upper bound clause.
3. Cause 1, @length: call ListRecommendedActions with filter={"actions":[{"key":"FEATURE","matchOption":"EQUALS","values":[]}]} (parameter_validation disabled). The emulator's summary reads "Value '0 items' at 'filter.actions.1.member.values' …" where AWS reads "Value at 'filter.actions.1.member.values' …".
4. Causes 1+2, enum key: call ListRecommendedActions with filter.actions[0].key outside {FEATURE, SEVERITY, TYPE}. The summary echoes the rejected value and lists the set as [FEATURE, SEVERITY, TYPE] instead of AWS's [FEATURE, TYPE, SEVERITY].
5. Causes 1+2, enum matchOption: same request with filter.actions[0].matchOption = "CONTAINS", and again with the wrong-case "equals". Both are refused (correct) but the listing is [EQUALS, NOT_EQUALS] instead of AWS's [NOT_EQUALS, EQUALS] and the summary echoes the value.
6. Cause 3: call ListRecommendedActions with nextToken="not-a-real-continuation-token-0123456789" (satisfies NextToken's length/pattern but was never minted). The 400 ValidationException message is the emulator's own sentence instead of AWS's "Pagination token exception in operation 'ListRecommendedActions': invalid payload wrapping schema <n>".
7. In each case the positive control on the same client still succeeds, confirming only the refusal rendering diverges.
