Issue-Source: trace-sweep-divergence
Issue-Trace: uxc-configure-visible-regions-roundtrip
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: uxc-configure-visible-regions-roundtrip uxc-partial-update-preserves-other-fields
Issue-Shape-Id: com.amazonaws.uxc#AWSAccountUXSetting
Issue-Key: uxc|UpdateAccountCustomizations|2xx->2xx|$.visibleRegions[*]
Issue-Key: uxc|UpdateAccountCustomizations|2xx->2xx|$.visibleServices[*]
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# UpdateAccountCustomizations reports list settings in the wrong member order

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAccountCustomizations

## Description
TWO causes, both on uxc UpdateAccountCustomizations, both the same shared fault: the sequence of the members inside the response's visibleRegions / visibleServices arrays does not match the recording. Both must be closed before submitting, and each names its own fixture and step so each can be re-replayed independently.

Cause 1 of 2: visibleRegions member order on a configure roundtrip (fixture uxc-configure-visible-regions-roundtrip, step task#4).
AWS: recorded behavior only. Smithy: UpdateAccountCustomizations is PATCH /v1/account-customizations, success 200; output UpdateAccountCustomizationsOutput.visibleRegions targets RegionsList (@length max 100, member Region @pattern ^[a-z]{2}(-[a-z]{1,10}){1,2}-[1-9]$), documented as "The current list of visible Region codes after the update"; the operation is @idempotent and its errors are AccessDeniedException/403, ThrottlingException/429, ValidationException/400, InternalServerException/500. RegionsList is a plain Smithy list — ordered, with no @uniqueItems and no sort trait — so the member order IS part of the response document, but the model states no rule that fixes it. The recording shows HTTP 200 whose visibleRegions reads eu-west-1, us-east-1, us-west-2.
Emulator: HTTP 200 with the same three Region codes in a different order — us-east-1, us-west-2, eu-west-1 — because the handler stores and reports a list setting as the exact sequence the request supplied and never normalizes it. Status, error-free outcome and member SET all agree; only the ordering diverges.
Hypothesis: the recorded order for this step is the ascending lexicographic order of the three codes, which is consistent with the service canonicalizing a list setting (sorting it) before persisting and echoing it. That is a hypothesis, not something the model or the doc states.
Why: a client that compares the response array positionally — asserting visibleRegions[0], diffing the echoed array against what it sent, or storing the echo as the canonical record — reads a different document than AWS returns, and the subsequent GetAccountCustomizations echo inherits the same order, so the whole roundtrip reads differently even though the configured set is right.
Fix: the state layer that persists a list setting — AccountUxCustomization::set_visible_regions / set_visible_services and the handler path that reads the record back out — not the serializer and not validation; whatever ordering rule is adopted must be applied on the write so the update's own echo and the later read agree.

Cause 2 of 2: visibleServices member order on a partial update (fixture uxc-partial-update-preserves-other-fields, step task#2).
AWS: recorded behavior only. Smithy: output member visibleServices targets ServiceList (@length max 500, member Service @length 1..64, @pattern ^[a-z0-9]+(-[a-z0-9]+)*$), documented as "The current list of visible service identifiers after the update"; same ordered-plain-list situation as RegionsList. The recording shows HTTP 200 whose visibleServices reads s3, ec2.
Emulator: HTTP 200 with the same two identifiers in the opposite order — ec2, s3 — again the request's supplied sequence read straight back out of state.
Note this cause is NOT explained by sorting: the recorded order (s3 before ec2) is not ascending, so Cause 1's hypothesis does not cover it, and the rule the service actually applies to a list setting is not established by either the model or the docs. The checkout already records awareness of this step: services/service-com-amazonaws-uxc-awsaccountuxsetting/src/state.rs lines 44-55 argues that this fixture's task#2 and task#4 echo the same two members in opposite sequences with no write between them, and deliberately chose the supplied-sequence rule, which makes task#4 agree and leaves task#2 diverging as replayed. That choice is the emulator's, so the divergence stands and needs resolving rather than asserting.
Why: same as Cause 1 — a positional comparison of the echoed visibleServices array, or a client treating the update's echo as the account's canonical record, disagrees with AWS. A partial update is exactly where clients check the echo, since the point of the call is that untouched settings come back unchanged.
Fix: same component as Cause 1 — the list-setting write/read-back path in the service state, which must produce one ordering rule that satisfies both the update echo and the following GetAccountCustomizations echo for this fixture.

Not covered elsewhere: the one open uxc issue, 20261022T030944Z-uxc-listservices-returns-wrong-service-catalog-and-validation-message.md, is about the ListServices catalog contents and the pattern-violation message rendering; it carries no replay key for either step below and does not cover UpdateAccountCustomizations.

Traces: uxc-configure-visible-regions-roundtrip#task#4 (Cause 1), uxc-partial-update-preserves-other-fields#task#2 (Cause 2).

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout: emulator up (/__admin/health 200), 9 fixtures replayed, 36 steps, 27 passed, 9 mismatched, 0 errored, 0 blocked, 0 state-reset failures, score 75.0%, bootstrap_signal false. 8 divergence clusters, all service=uxc; 6 of them are owned by the existing open ListServices issue and are excluded here. The two clusters below are unowned and carry no stub_path.

Cause 1 -- fixture uxc-configure-visible-regions-roundtrip, step task#4, operation UpdateAccountCustomizations, expected_status 200 / actual_status 200, verdict mismatch, affected_count 1:
  "$.visibleRegions[0]: value_mismatch (expected 'eu-west-1', got 'us-east-1')"
  "$.visibleRegions[1]: value_mismatch (expected 'us-east-1', got 'us-west-2')"
  "$.visibleRegions[2]: value_mismatch (expected 'us-west-2', got 'eu-west-1')"
  divergence_key: service=uxc|UpdateAccountCustomizations|status=200->200|keys=$.visibleRegions[0],$.visibleRegions[1],$.visibleRegions[2]
The member multiset is identical; only the positions differ, and the expected sequence is the ascending order of the three codes.

Cause 2 -- fixture uxc-partial-update-preserves-other-fields, step task#2, operation UpdateAccountCustomizations, expected_status 200 / actual_status 200, verdict mismatch, affected_count 1:
  "$.visibleServices[0]: value_mismatch (expected 's3', got 'ec2')"
  "$.visibleServices[1]: value_mismatch (expected 'ec2', got 's3')"
  divergence_key: service=uxc|UpdateAccountCustomizations|status=200->200|keys=$.visibleServices[0],$.visibleServices[1]
Again the member multiset is identical and only the positions differ; the expected sequence here is NOT ascending, so the two causes are not both explained by a plain sort.

Emulator source behind both: services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/update_account_customizations.rs lines 217-222 pass the supplied list straight into record.set_visible_services / set_visible_regions and lines 231-239 answer from the record read back, while services/service-com-amazonaws-uxc-awsaccountuxsetting/src/state.rs lines 56-65 store the supplied Vec unchanged (no ordering applied); the same file's lines 44-55 document the deliberate choice that leaves Cause 2 diverging.

Model facts cited: smithy_query(query="operation", operation="UpdateAccountCustomizations,GetAccountCustomizations") and smithy_query(query="shape", shape="ServiceList,RegionsList,UpdateAccountCustomizationsOutput") on services/service-com-amazonaws-uxc-awsaccountuxsetting/smithy-model/com-amazonaws-uxc-awsaccountuxsetting.json.

## Steps to Reproduce
1. Clone the repo, boot the emulator built from this checkout, and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture uxc-configure-visible-regions-roundtrip up to step task#4 -- PATCH /v1/account-customizations carrying the fixture's visibleRegions -- and compare the 200 body's $.visibleRegions positionally with the recording. The emulator answers us-east-1, us-west-2, eu-west-1; the recording is eu-west-1, us-east-1, us-west-2.
3. Cause 2: replay fixture uxc-partial-update-preserves-other-fields up to step task#2 -- PATCH /v1/account-customizations -- and compare the 200 body's $.visibleServices positionally. The emulator answers ec2, s3; the recording is s3, ec2. Then continue to that fixture's task#4 GetAccountCustomizations, which currently passes, to see that any ordering rule adopted must satisfy both echoes.
4. Both steps return HTTP 200 with the correct member sets, so the divergence is visible only in array element positions -- diff the arrays index by index rather than as sets.
