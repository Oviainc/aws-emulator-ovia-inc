Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: detective.list-datasource-packages-max-results-rejected detective.update-datasource-packages-missing-mandatory-core-rejected detective.start-investigation-malformed-entity-arn-validation-rejected detective.start-investigation-nonexistent-graph-not-found-rejected detective.reject-invitation-nonexistent-graph-not-found-rejected detective.start-monitoring-member-non-member-not-found-rejected detective.delete-members-removes-member-record
Issue-Shape-Id: com.amazonaws.detective#AmazonDetective
Issue-Key: detective.list-datasource-packages-max-results-rejected|_error#6|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: detective.update-datasource-packages-missing-mandatory-core-rejected|run#7|AssertionError|assert message == (
Issue-Key: detective.start-investigation-malformed-entity-arn-validation-rejected|run#7|AssertionError|assert message == (
Issue-Key: detective.start-investigation-nonexistent-graph-not-found-rejected|run#9|AssertionError|assert message == "Graph Not Found. Please check the graphArn provided", f"unexpected message: {message!r}"
Issue-Key: detective.reject-invitation-nonexistent-graph-not-found-rejected|run#6|AssertionError|assert message == "No such membership", f"unexpected message: {message!r}"
Issue-Key: detective.start-monitoring-member-non-member-not-found-rejected|run#6|AssertionError|assert message == "No such membership", f"unexpected message: {message!r}"
Issue-Key: detective.delete-members-removes-member-record|run#14|AssertionError|assert unprocessed[fake_account] == "No such membership", (
Issue-Commit: 128b971c27980fd8fd6bfdfd4c5ec95349a617e4

# Detective refusal texts diverge from AWS and MaxResults on ListDatasourcePackages is accepted instead of refused

> _Note: this issue was identified against the code base at commit `128b971c27980fd8fd6bfdfd4c5ec95349a617e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDatasourcePackages

## Description
Seven net-new root causes in Detective's refusal surface, found by replaying the behavioral suite against the emulator. One is an overly-permissive accept (input AWS refuses is served a 200 with a synthesised pagination token); the other six are refusal/partial-success texts the emulator words in its own prose instead of Detective's. Each cause below is distinct and MUST be closed before submitting; they are seven gaps, not one.

Related open issue (does NOT cover these; it is grounded in the wire corpus and in different steps): 20261029T172317Z-detective-client-error-bodies-omit-the-message-field.md reports that some Detective 4xx bodies carry NO message member at all and that DeleteMembers' OWN response body drops AccountIds and rewords its reason. Here the message member IS present on every refusal below — its text is wrong — and Cause 3 is GetMembers' UnprocessedAccounts reason, a different response body. Fixing the text in a single shared place would close both.

Cause 1 of 7: ListDatasourcePackages accepts MaxResults, which Detective refuses, and invents a NextToken.
AWS: Smithy: ListDatasourcePackages input has MaxResults -> MemberResultsLimit @range min 1 max 200 and NextToken -> PaginationToken, and the operation carries a @paginated trait; errors include ValidationException (errorKind client, httpError 400). Despite the modelled member, this endpoint does not paginate: recorded behavior shows a request carrying any MaxResults value refused with HTTP 400, error code ValidationException and the fixed message 'Max results must be null' (no variable part — nothing from the request is interpolated), while the same call without MaxResults returns the full package map in one shot and NO NextToken.
Emulator: the call with MaxResults=1 SUCCEEDS with HTTP 200, returns a single-entry DatasourcePackages map and a NextToken (a hex-encoded package name, e.g. '4445544543544956455f434f5245' = 'DETECTIVE_CORE'). No refusal at all.
Why: a client that mistakenly passes MaxResults is told its request is valid and then silently receives a TRUNCATED view of the graph's datasource packages, and is handed a continuation token that real Detective never issues; code written and tested against the emulator will page a non-paginated API and, against real AWS, break outright with a 400 it was never taught to expect. Truncation of a security-ingest state map is a correctness problem, not only a wording one.
Fix: the ListDatasourcePackages handler's input validation — reject any present MaxResults with ValidationException/400 and that fixed message — and its response builder, which must never emit NextToken for this operation.
Traces: detective.list-datasource-packages-max-results-rejected.

Cause 2 of 7: UpdateDatasourcePackages' mandatory-package refusal message has extra wording.
AWS: Smithy: UpdateDatasourcePackages.DatasourcePackages -> DatasourcePackageList, required, @length min 1 max 25; errors include ValidationException (client, 400). Recorded behavior: omitting the mandatory DETECTIVE_CORE package is refused with 400 / ValidationException and the message 'UpdateDatasourcePackages request missing mandatory datasource package(s): [DETECTIVE_CORE]'. Fixed text: 'UpdateDatasourcePackages request missing mandatory datasource package(s): ['...']'; the only variable part is the comma-separated list of mandatory package names missing from the request.
Emulator: same status (400) and code (ValidationException) but the message is 'UpdateDatasourcePackages request is missing mandatory datasource package(s): [DETECTIVE_CORE]' — an interpolated 'is' that AWS's fixed text does not contain.
Why: the message is the only machine-readable detail for this class (ValidationException's ErrorCode/ErrorCodeReason are unset), so clients, runbooks and tests that match Detective's documented wording never match.
Fix: the UpdateDatasourcePackages handler's mandatory-package validation message template.
Traces: detective.update-datasource-packages-missing-mandatory-core-rejected.

Cause 3 of 7: GetMembers reports a removed account's UnprocessedAccount reason in the emulator's own prose.
AWS: Smithy: GetMembers output is {MemberDetails -> MemberDetailList, UnprocessedAccounts -> UnprocessedAccountList}; UnprocessedAccount is {AccountId, Reason -> UnprocessedReason}, documented as 'The reason that the member account request could not be processed.' Recorded behavior: after DeleteMembers removes an account, GetMembers for that account returns 200 with MemberDetails empty and one UnprocessedAccounts entry whose Reason is the fixed missing-membership text 'No such membership' — entirely fixed text, with the account id carried in AccountId, not in the reason.
Emulator: 200 with MemberDetails empty (correct) but UnprocessedAccounts[0].Reason is 'The account is not a member account in the behavior graph.'
Why: clients branch on this reason string to distinguish 'never a member / removed' from other unprocessed causes; a reworded reason makes every such branch fall through, and it is inconsistent with the 404 text Detective uses for the identical condition (Causes 4 and 6), so one condition surfaces under three different strings in this emulator.
Fix: the GetMembers handler's UnprocessedAccount builder, sourcing the missing-membership text from the single shared place that also produces the 404 message, so the two can never drift.
Traces: detective.delete-members-removes-member-record.

Cause 4 of 7: RejectInvitation's not-found message is a graph-existence message instead of Detective's membership text.
AWS: Smithy: RejectInvitation input is {GraphArn required, @pattern ^arn:aws[-\\w]{0,10}?:detective:[-\\w]{2,20}?:\\d{12}?:graph:[abcdef\\d]{32}?$}; errors include ResourceNotFoundException (client, 404), documented 'The request refers to a nonexistent resource.' Recorded behavior: a well-formed GraphArn that was never created is refused with 404 / ResourceNotFoundException and the fixed message 'No such membership' — no variable part, and notably membership-centric: Detective does not disclose whether the graph exists.
Emulator: 404 / ResourceNotFoundException with message 'Graph not found'.
Why: the client sees an unexpected string for the standard 'you hold no invitation' case, and the emulator's phrasing additionally asserts a different fact (the graph is absent) than AWS's refusal, teaching callers a distinction real Detective does not make. Matching on the documented wording fails.
Fix: the RejectInvitation handler's not-found path — evaluate it as a membership lookup and emit the shared missing-membership text rather than a graph-lookup message.
Traces: detective.reject-invitation-nonexistent-graph-not-found-rejected.

Cause 5 of 7: StartMonitoringMember's non-member refusal message is reworded.
AWS: Smithy: StartMonitoringMember input is {GraphArn required, AccountId required -> AccountId @length min 12 max 12 @pattern ^[0-9]+$}; errors include ResourceNotFoundException (client, 404). Recorded behavior: naming an account that is not a member of the graph is refused with 404 / ResourceNotFoundException and the fixed text 'No such membership' (no request value interpolated).
Emulator: 404 / ResourceNotFoundException with message 'The specified account is not a member account of the behavior graph'.
Why: same as Cause 4 — clients enabling members cannot match the documented text, and this condition now reads differently here than in GetMembers' unprocessed reason (Cause 3) for the very same state.
Fix: the StartMonitoringMember handler's member-lookup refusal, through the shared missing-membership text.
Traces: detective.start-monitoring-member-non-member-not-found-rejected.

Cause 6 of 7: StartInvestigation's nonexistent-graph message is not Detective's wording.
AWS: Smithy: StartInvestigation input is {GraphArn required (graph ARN pattern), EntityArn required, ScopeStartTime, ScopeEndTime}; errors include ResourceNotFoundException (client, 404). Recorded behavior: a well-formed GraphArn that was never created is refused with 404 / ResourceNotFoundException and the fixed message 'Graph Not Found. Please check the graphArn provided' — fixed text in full, with no value echoed; note this operation's wording differs from the membership text of Causes 4/5, so the two are not interchangeable.
Emulator: 404 / ResourceNotFoundException with message 'Graph not found'.
Why: the exception text is the only user-facing detail; clients and tests asserting Detective's wording fail, and the operation-specific guidance ('check the graphArn provided') that tells an operator which input to inspect is lost.
Fix: the StartInvestigation handler's graph-lookup refusal message.
Traces: detective.start-investigation-nonexistent-graph-not-found-rejected.

Cause 7 of 7: the regex echoed in StartInvestigation's entityArn validation message is not the pattern AWS reports.
AWS: Smithy: StartInvestigation.EntityArn is required with @pattern ^arn: and no length constraint, so the input is never intercepted client-side and is refused by the service with ValidationException (client, 400). Recorded behavior: the message is the standard AWS validation template "1 validation error detected: Value '<the rejected value>' at 'entityArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:.*" — the fixed parts are '<n> validation error(s) detected: Value ', " at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: " and the reported pattern '^arn:.*'; the variable parts are the rejected value and the member path, both taken from the request.
Emulator: 400 / ValidationException with the same template but the trailing pattern rendered as '^arn:' — i.e. "...regular expression pattern: ^arn:" — the raw model pattern rather than the '^arn:.*' form Detective reports.
Why: this message is what SDK users and automated assertions read; a client that compares or parses the reported constraint gets a different pattern than AWS states, and tooling that re-validates input against the echoed regex derives a stricter rule (anchored prefix only) than the one AWS publishes.
Fix: the ValidationException message builder for @pattern failures in this service's input validation — report Detective's pattern rendering for EntityArn.
Traces: detective.start-investigation-malformed-entity-arn-validation-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 27/38 passed (71.1%); 7 mismatched, 4 errored. All failures are service=detective. The 4 errored checks are stub-blocked (ListInvestigations, UpdateInvestigationState) and already filed — excluded here. The 7 mismatches are the 7 causes above.

Cause 1 — detective.list-datasource-packages-max-results-rejected [mismatch]:
  AssertionError: expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'DatasourcePackages': {'ASFF_SECURITYHUB_FINDING': {'DatasourcePackageIngestState': 'STARTED', ...}}, 'NextToken': '4445544543544956455f434f5245'}
  expected: 400 / ValidationException / 'Max results must be null'. (The single returned package and the hex token are per-run values; the requirement is the refusal and the absence of NextToken.)

Cause 2 — detective.update-datasource-packages-missing-mandatory-core-rejected [mismatch]:
  expected message: 'UpdateDatasourcePackages request missing mandatory datasource package(s): [DETECTIVE_CORE]'
  got:              'UpdateDatasourcePackages request is missing mandatory datasource package(s): [DETECTIVE_CORE]'
  code ValidationException and status 400 agree.

Cause 3 — detective.delete-members-removes-member-record [mismatch]:
  GetMembers UnprocessedAccounts[<removed account>].Reason:
  expected 'No such membership', got 'The account is not a member account in the behavior graph.'
  (MemberDetails == [] as recorded; status 200 agrees. The account id is generated per run.)

Cause 4 — detective.reject-invitation-nonexistent-graph-not-found-rejected [mismatch]:
  code ResourceNotFoundException agrees; message expected 'No such membership', got 'Graph not found'.

Cause 5 — detective.start-monitoring-member-non-member-not-found-rejected [mismatch]:
  code ResourceNotFoundException agrees; message expected 'No such membership',
  got 'The specified account is not a member account of the behavior graph'.

Cause 6 — detective.start-investigation-nonexistent-graph-not-found-rejected [mismatch]:
  code ResourceNotFoundException agrees; message expected 'Graph Not Found. Please check the graphArn provided', got 'Graph not found'.

Cause 7 — detective.start-investigation-malformed-entity-arn-validation-rejected [mismatch]:
  code ValidationException agrees; message expected
    "1 validation error detected: Value 'not-an-arn-at-all' at 'entityArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:.*"
  got
    "1 validation error detected: Value 'not-an-arn-at-all' at 'entityArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:"
  ('not-an-arn-at-all' is the value the check sends — a request value filling the template; the diverging part is the reported pattern.)

Status codes and error codes agree on every refusal above; except for Cause 1 the divergence is the message/body text.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it, then replay the simparity-behavioral-aws-v1 behavioral suite (service detective).
2. Cause 1: CreateGraph, UpdateDatasourcePackages with all three packages, then call ListDatasourcePackages(GraphArn=arn, MaxResults=1). Observe HTTP 200 with a truncated DatasourcePackages map and a NextToken, instead of 400 / ValidationException / 'Max results must be null'. Re-run check detective.list-datasource-packages-max-results-rejected.
3. Cause 2: call UpdateDatasourcePackages(GraphArn=arn, DatasourcePackages=['EKS_AUDIT']) — omitting DETECTIVE_CORE. Status 400 / ValidationException as recorded, but the message contains the extra word 'is'. Check detective.update-datasource-packages-missing-mandatory-core-rejected.
4. Cause 3: CreateMembers for a fabricated account id, DeleteMembers it, then poll GetMembers for that account until MemberDetails is empty and inspect UnprocessedAccounts[0].Reason — it is 'The account is not a member account in the behavior graph.' instead of 'No such membership'. Check detective.delete-members-removes-member-record.
5. Cause 4: call RejectInvitation with a well-formed but never-created graph ARN (arn:aws:detective:<region>:<account>:graph:<32 hex>). Status 404 / ResourceNotFoundException, message 'Graph not found' instead of 'No such membership'. Check detective.reject-invitation-nonexistent-graph-not-found-rejected.
6. Cause 5: call StartMonitoringMember against a real graph naming an account that is not a member. Status 404 / ResourceNotFoundException, message reworded. Check detective.start-monitoring-member-non-member-not-found-rejected.
7. Cause 6: call StartInvestigation with a never-created graph ARN and a valid EntityArn/scope window. Status 404 / ResourceNotFoundException, message 'Graph not found' instead of 'Graph Not Found. Please check the graphArn provided'. Check detective.start-investigation-nonexistent-graph-not-found-rejected.
8. Cause 7: call StartInvestigation against a real graph with EntityArn='not-an-arn-at-all'. Status 400 / ValidationException, but the message's reported regular expression is '^arn:' instead of '^arn:.*'. Check detective.start-investigation-malformed-entity-arn-validation-rejected.
