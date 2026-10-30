Issue-Source: trace-sweep-divergence
Issue-Trace: license-manager-linux-subscriptions-list-linux-subscriptions-invalid-filter
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: license-manager-linux-subscriptions-list-linux-subscriptions-invalid-filter
Issue-Shape-Id: com.amazonaws.licensemanagerlinuxsubscriptions#LicenseManagerLinuxSubscriptions
Issue-Key: license-manager-linux-subscriptions|ListLinuxSubscriptions|4xx->4xx|$.message
Issue-Key: license-manager-linux-subscriptions|ListLinuxSubscriptionInstances|4xx->4xx|$.message
Issue-Commit: 5be2d973ad1b703e2cd0a56e2c9e00ba2fe90954

# Filter Operator enum ValidationException message lists the enum value set in the wrong order

> _Note: this issue was identified against the code base at commit `5be2d973ad1b703e2cd0a56e2c9e00ba2fe90954`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListLinuxSubscriptions

## Description
Both discovery-list operations refuse an unmodelled `Filters[].Operator` value with the right class, status and message template, but they render the "Member must satisfy enum value set" clause in the enum's Smithy declaration order instead of the order AWS emits. The emulator's filter-validation refusal formatter is shared by the two operations, so the single formatting rule produces both causes.

Cause 1 of 2: ListLinuxSubscriptions with an invalid Filters[1].Operator.
AWS: refuses with HTTP 400 and error ValidationException (Smithy: ListLinuxSubscriptions errors ValidationException, httpError 400 defaulted; member Filter.Operator targets the `Operator` enum, @length min 1 max 20). The constraint-violation message follows the standard AWS validation template: fixed text "1 validation error detected: Value '", then the rejected value from the request, then "' at '", then the member path, then "' failed to satisfy constraint: Member must satisfy enum value set: [", then the comma-separated accepted values, then "]". Recorded behavior only for the ORDER of that value list: the recording shows "[NotEqual, Equal, Contains]". Variable parts taken from the request are the rejected value and the member path (the recording's are 'NotAnOperator' and 'filters.1.member.operator'); the fixed parts are the surrounding template text and the accepted-value list itself.
Emulator: same status (400) and same template, but the accepted-value list is emitted as "[Equal, NotEqual, Contains]" — the order in which the members are declared in the Smithy `Operator` enum (values: Equal, NotEqual, Contains). Replay reports `$.message: value_mismatch` with everything but the bracketed list identical.
Why: a client or test that asserts on the refusal message — the usual way this error class is pinned, since the error body carries nothing but `message` — sees a non-AWS string and fails, and a human reading the refusal is told a different value set than AWS reports. The permitted values themselves are correct, so only the text diverges.
Fix: the shared filter-validation refusal formatter used by the discovery-list handlers (this crate's enum-constraint message builder, reached from the generated handlers under services/service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions/src/ops/). It must emit the enum value set in AWS's order for this member rather than the model's declaration order.
Traces: license-manager-linux-subscriptions-list-linux-subscriptions-invalid-filter#task#1.

Cause 2 of 2: ListLinuxSubscriptionInstances with an invalid Filters[1].Operator.
AWS: identical contract — HTTP 400 ValidationException (Smithy: ListLinuxSubscriptionInstances errors ValidationException) with the same message template and the same value list "[NotEqual, Equal, Contains]" (recorded behavior only for the ordering).
Emulator: again 400 with "[Equal, NotEqual, Contains]"; only `$.message` diverges.
Why: same as cause 1, on the second of the two filterable discovery lists, so a client that validated its filter handling against either list is misled.
Fix: the same shared refusal formatter — fixing it once must settle both operations; do not patch one handler's string.
Traces: license-manager-linux-subscriptions-list-linux-subscriptions-invalid-filter#task#2.

## Evidence
Fixture license-manager-linux-subscriptions-list-linux-subscriptions-invalid-filter.

Step task#1 (ListLinuxSubscriptions): expected status 400, actual 400.
  expected $.message = "1 validation error detected: Value 'NotAnOperator' at 'filters.1.member.operator' failed to satisfy constraint: Member must satisfy enum value set: [NotEqual, Equal, Contains]"
  actual   $.message = "1 validation error detected: Value 'NotAnOperator' at 'filters.1.member.operator' failed to satisfy constraint: Member must satisfy enum value set: [Equal, NotEqual, Contains]"

Step task#2 (ListLinuxSubscriptionInstances): expected status 400, actual 400; identical expected/actual $.message pair as above.

Smithy: Operator is an enum with values [Equal, NotEqual, Contains] (declaration order), @length min 1 max 20; Filter.Operator targets it; both operations model ValidationException (httpError 400).
Only the bracketed accepted-value list diverges — status, error class and the rest of the message text match.

## Steps to Reproduce
1. Boot the emulator and replay fixture license-manager-linux-subscriptions-list-linux-subscriptions-invalid-filter.
2. Step task#1: POST /subscription/ListLinuxSubscriptions with a Filters list whose second element carries an Operator value outside the enum (e.g. "NotAnOperator").
3. Observe HTTP 400 ValidationException whose message ends "Member must satisfy enum value set: [Equal, NotEqual, Contains]" instead of AWS's "[NotEqual, Equal, Contains]".
4. Step task#2: POST /subscription/ListLinuxSubscriptionInstances with the same invalid filter and observe the same reworded value list.
