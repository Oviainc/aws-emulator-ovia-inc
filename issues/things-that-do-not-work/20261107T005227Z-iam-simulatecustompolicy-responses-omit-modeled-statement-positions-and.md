Issue-Source: trace-sweep-divergence
Issue-Trace: iam-permissions-boundary-and-policy-simulation
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-permissions-boundary-and-policy-simulation iam-policy-simulation-and-context-keys
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|SimulateCustomPolicy|2xx->2xx|$.SimulateCustomPolicyResponse.SimulateCustomPolicyResult.EvaluationResults.member.MatchedStatements.member.EndPosition
Issue-Key: iam|SimulateCustomPolicy|2xx->2xx|$.SimulateCustomPolicyResponse.SimulateCustomPolicyResult.EvaluationResults.member.MatchedStatements.member.StartPosition
Issue-Key: iam|SimulateCustomPolicy|2xx->2xx|$.SimulateCustomPolicyResponse.SimulateCustomPolicyResult.EvaluationResults.member.MissingContextValues
Issue-Key: iam|SimulateCustomPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|SimulateCustomPolicy|2xx->2xx|$.SimulateCustomPolicyResponse.SimulateCustomPolicyResult.EvaluationResults.member[*].MatchedStatements.member.EndPosition
Issue-Key: iam|SimulateCustomPolicy|2xx->2xx|$.SimulateCustomPolicyResponse.SimulateCustomPolicyResult.EvaluationResults.member[*].MatchedStatements.member.StartPosition
Issue-Key: iam|SimulateCustomPolicy|2xx->2xx|$.SimulateCustomPolicyResponse.SimulateCustomPolicyResult.EvaluationResults.member[*].MissingContextValues
Issue-Commit: 1b064f119d6a2bdadd353b8b6f400830e73a40d7

# iam SimulateCustomPolicy responses omit modeled statement positions and MissingContextValues, and its InvalidInput refusal names no policy entry

> _Note: this issue was identified against the code base at commit `1b064f119d6a2bdadd353b8b6f400830e73a40d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SimulateCustomPolicy

## Description
Three net-new divergences on IAM's SimulateCustomPolicy wire surface: the success response drops modeled members of EvaluationResult (and of the Statement entries inside MatchedStatements), and the InvalidInput refusal of an unparseable policy document carries emulator prose instead of AWS's message. Each cause must be closed before this issue is submitted.

Cause 1 of 3: a successful simulation's MatchedStatements entries omit StartPosition and EndPosition, and EvaluationResult omits MissingContextValues.
AWS: Smithy — SimulateCustomPolicy output SimulatePolicyResponse.EvaluationResults is a list of com.amazonaws.iam#EvaluationResult; EvaluationResult.MatchedStatements targets StatementListType of com.amazonaws.iam#Statement, whose members are SourcePolicyId, SourcePolicyType, StartPosition and EndPosition — StartPosition is documented as "The row and column of the beginning of the Statement in an IAM policy" and EndPosition as "The row and column of the end of a Statement in an IAM policy", both targeting the Position structure (Line, Column). EvaluationResult.MissingContextValues targets ContextKeyNamesResultListType and is documented as "A list of context keys that are required by the included input policies but that were not provided by one of the input parameters"; when no key is missing AWS still emits the member as an empty list (the query/XML protocol serializes it as an empty element, which the fixture records as ''). So for a simulation whose statement matches, AWS returns each matched statement with Line/Column for both the start and the end of that statement within the submitted document, and returns MissingContextValues present-but-empty.
Emulator: both Position members are absent from every matched statement and MissingContextValues is absent from the EvaluationResult entirely — the replay reports `MatchedStatements.member.StartPosition: missing`, `...EndPosition: missing` and `MissingContextValues: missing (expected '', got None)` on an otherwise-200 response.
Why: StartPosition/EndPosition are how a caller points a user at the exact statement in the submitted document that produced the decision (the whole purpose of MatchedStatements); without them a policy-linter or console cannot highlight the deciding statement. And because MissingContextValues is absent rather than empty, a client cannot distinguish "no condition keys were missing" from "the field was not returned" — SDKs that treat an absent list as unknown, and callers that branch on len(MissingContextValues), both read the wrong answer; AWS's documented discrimination between "condition unsatisfied" and "condition key not supplied" depends on this member.
Fix: the SimulateCustomPolicy handler's response serializer and its policy-evaluation path — the evaluator must record the character/line span of each matching statement in the submitted document and populate Statement.StartPosition/EndPosition, and the EvaluationResult projection must always emit MissingContextValues (empty when nothing is missing) rather than eliding it.
Traces: iam-permissions-boundary-and-policy-simulation#task#11.

Cause 2 of 3: the InvalidInput refusal for an unparseable PolicyInputList entry uses emulator prose that identifies no entry.
AWS: Smithy — SimulateCustomPolicy models InvalidInputException (errorKind client, httpError 400, awsQueryError code InvalidInput, HTTP 400); PolicyInputList is required and targets SimulationPolicyListType of policyDocumentType. An entry that is not a parseable policy document is refused HTTP 400 with Error.Code InvalidInput and the message 'Policy input list item <n> has invalid content' — the fixed text is "Policy input list item " ... " has invalid content" and the only variable part is <n>, the 1-BASED index of the offending entry within the request's PolicyInputList.
Emulator: the status 400 and the code InvalidInput are correct, but Error.Message is the fixed sentence 'Syntax errors in policy.', which names no entry and is not AWS's wording.
Why: a caller that submits several documents in one simulation is told only that something was unparseable, not which entry — so it cannot report the fault back to the user or retry with the offending document removed; any client or test matching AWS's message text diverges.
Fix: the InvalidInput message construction in the SimulateCustomPolicy handler's input validation — it must carry the 1-based index of the failing PolicyInputList entry. Note: the SAME wording defect on this operation is already described as Cause 2 of the OPEN behavioral issue 20261106T201244Z-iam-3-net-new-root-causes-simulatecustompolicy-never.md, but that issue carries no wire replay key for this fixture step, so this cluster is filed here and must be re-replayed against this fixture after the fix.
Traces: iam-permissions-boundary-and-policy-simulation#task#18.

Cause 3 of 3: the same omission as Cause 1 on a multi-action simulation, confirming it affects every EvaluationResults member, not only a single-result response.
AWS: as Cause 1 — every EvaluationResults entry carries its own MissingContextValues (present, empty when nothing is missing), and every Statement inside its MatchedStatements carries StartPosition and EndPosition with Line and Column.
Emulator: on a simulation returning several EvaluationResults, the first entry's MatchedStatements member omits StartPosition and EndPosition and the entry omits MissingContextValues, and the SECOND entry omits MissingContextValues as well — so the elision is in the shared projection, applied per result.
Why: as Cause 1; a caller iterating EvaluationResults to find which of several simulated actions lacked context keys gets None for every entry.
Fix: same component as Cause 1 — the per-result EvaluationResult/Statement projection in the SimulateCustomPolicy serializer.
Traces: iam-policy-simulation-and-context-keys#task#4.

Related already-filed issues: 20261103T132715Z-iam-simulateprincipalpolicy-evaluation-results-omit-modeled-detail-members.md describes an analogous omission on SimulatePrincipalPolicy and 20261106T201244Z-iam-3-net-new-root-causes-simulatecustompolicy-never.md describes the condition-evaluation and message faults behaviorally; neither owns the three wire replay keys above, so closing them does not close this issue.

## Evidence
Cause 1 — fixture iam-permissions-boundary-and-policy-simulation step task#11, SimulateCustomPolicy, expected HTTP 200 / actual HTTP 200. Replay detail: $.SimulateCustomPolicyResponse.SimulateCustomPolicyResult.EvaluationResults.member.MatchedStatements.member.EndPosition: missing (expected a Position structure {'Line': ..., 'Column': ...}, recorded as Line 1 / Column 112, got None); ...MatchedStatements.member.StartPosition: missing (expected {'Line': 1, 'Column': 41}, got None); ...MissingContextValues: missing (expected '' — the empty-list serialization — got None). The Line/Column numbers are offsets into the policy document submitted in that request, so they are request-derived values; the requirement is that both Position members be present and locate the matched statement.

Cause 2 — fixture iam-permissions-boundary-and-policy-simulation step task#18, SimulateCustomPolicy, expected HTTP 400 / actual HTTP 400, Error.Code InvalidInput on both sides. Replay detail: $.ErrorResponse.Error.Message: value_mismatch (expected 'Policy input list item 1 has invalid content', got 'Syntax errors in policy.'). Fixed text: 'Policy input list item ' + ' has invalid content'; variable part: the 1-based index of the bad PolicyInputList entry (here 1, because the request submitted a single unparseable document).

Cause 3 — fixture iam-policy-simulation-and-context-keys step task#4, SimulateCustomPolicy, 200 vs 200. Replay detail: EvaluationResults.member[0].MatchedStatements.member.EndPosition: missing (expected {'Line': 1, 'Column': 121}, got None); member[0].MatchedStatements.member.StartPosition: missing (expected {'Line': 1, 'Column': 41}, got None); member[0].MissingContextValues: missing (expected '', got None); member[1].MissingContextValues: missing (expected '', got None).

## Steps to Reproduce
Cause 1: replay fixture iam-permissions-boundary-and-policy-simulation step task#11 — or call SimulateCustomPolicy with PolicyInputList=[a document whose single statement allows the simulated action] and ActionNames=[that action], then read EvaluationResults[0]: MissingContextValues must be present (empty when no key is missing) and MatchedStatements[0] must carry StartPosition{Line,Column} and EndPosition{Line,Column} locating that statement in the submitted document.

Cause 2: replay fixture iam-permissions-boundary-and-policy-simulation step task#18 — or call SimulateCustomPolicy with a single unparseable entry in PolicyInputList (e.g. '{"Version": "2012-10-17", "Statement": [') and any ActionNames; inspect Error.Code (InvalidInput, correct) and Error.Message, which must read 'Policy input list item 1 has invalid content'. Repeat with the bad document as the second of two entries and confirm the index becomes 2.

Cause 3: replay fixture iam-policy-simulation-and-context-keys step task#4 — or call SimulateCustomPolicy with two ActionNames so several EvaluationResults are returned, and check that EVERY entry carries MissingContextValues and that each matched statement carries both Position members.
