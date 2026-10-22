Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kendra-ranking.create-plan-invalid-name-pattern-rejected kendra-ranking.describe-nonexistent-plan-not-found-rejected kendra-ranking.delete-plan-removes-resource kendra-ranking.list-plans-includes-created-plan-with-status kendra-ranking.rescore-against-inactive-plan-conflict-rejected
Issue-Shape-Id: com.amazonaws.kendraranking#AWSKendraRerankingFrontendService
Issue-Key: kendra-ranking.create-plan-invalid-name-pattern-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: kendra-ranking.describe-nonexistent-plan-not-found-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: kendra-ranking.delete-plan-removes-resource|run#12|AssertionError|assert message == expected_message, f"expected Describe message {expected_message!r}, got {message!r}"
Issue-Key: kendra-ranking.list-plans-includes-created-plan-with-status|_assert_deleted#12|AssertionError|assert message == expected_message, (
Issue-Key: kendra-ranking.rescore-against-inactive-plan-conflict-rejected|run#7|AssertionError|raise AssertionError(
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# kendra-ranking: 3 control-plane fidelity defects (validation message rendering, ResourceNotFoundException message, missing transient CREATING plan status)

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRescoreExecutionPlan

## Description
Three distinct root causes in the Kendra Intelligent Ranking (kendra-ranking) emulator, all surfaced by the behavioral suite (6/18 passing). Each must be closed independently.

=== Cause 1 of 3: CreateRescoreExecutionPlan ValidationException message is rendered with the offending value and the anchored regex ===
AWS: Smithy: CreateRescoreExecutionPlan.Name -> RescoreExecutionPlanName, @required, @length min 1 max 1000, @pattern "^[a-zA-Z0-9][a-zA-Z0-9_-]*$"; modeled error ValidationException (client, httpError 400, doc "The input fails to satisfy the constraints set by the Amazon Kendra Intelligent Ranking service"). A Name that violates the pattern is rejected with HTTP 400 / error code ValidationException -- that part the emulator already gets right. The message is AWS's standard constraint-violation message, which IS a requirement here because its rule is known: it is built from the request member and the member's modeled constraint. Recorded behavior shows the rule AWS applies for this member: the prefix count and member path are present, the offending VALUE is NOT echoed, and the pattern is rendered WITHOUT the ^/$ anchors, i.e. "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9][a-zA-Z0-9_-]*".
Emulator: emits "1 validation error detected: Value '<the submitted name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9][a-zA-Z0-9_-]*$" -- it interpolates the rejected value into the message and prints the Smithy pattern verbatim including anchors. So two rule differences: value echo, and anchor rendering.
Why: clients and test suites that match AWS's validation-error text (a very common pattern for asserting WHICH member failed and against which constraint) do not match. Echoing the rejected value back also leaks caller-supplied input into an error string AWS deliberately withholds for this member, so a client scrubbing errors for logging sees different content than on AWS.
Fix: the shared request-validation/constraint-violation message formatter for this service (the component that renders @pattern violations into ValidationException), reached from the CreateRescoreExecutionPlan handler's input validation. The pattern must be rendered with leading ^ and trailing $ stripped, and the value must not be interpolated for this member.
Traces: kendra-ranking.create-plan-invalid-name-pattern-rejected

=== Cause 2 of 3: ResourceNotFoundException carries a request-derived message instead of the service's fixed message ===
AWS: Smithy: DescribeRescoreExecutionPlan errors include ResourceNotFoundException (client, httpError 404, doc "The resource you want to use doesn't exist. Please check you have provided the correct resource and try again."). The model does not pin the wire text, so: recorded behavior only -- across three independent checks, with three different plan ids (a never-created all-zeros uuid and two distinct freshly-deleted plan ids), AWS returned the SAME message, 'The specified Resource Execution Plan ID does not exist.'. The recording therefore establishes the rule as a fixed, request-INDEPENDENT constant for this error class, not merely one recorded example: the same string appears for unrelated ids and for both the never-existed and the deleted case.
Emulator: returns ResourceNotFoundException (code and 404 are correct) with a request-DERIVED message, 'Rescore execution plan <id> was not found.' -- the id of the request is interpolated, so the message varies per call.
Why: a client that keys off the message (or a fixture/golden comparison of the error body) sees a different and non-constant string. It also means the emulator's not-found message is structurally different in kind from AWS's (parameterised vs constant), so no single expected string can be written against both.
Fix: the ResourceNotFoundException construction used by the rescore-execution-plan lookup path (DescribeRescoreExecutionPlan handler and the shared plan-lookup/state helper it and DeleteRescoreExecutionPlan call), so it emits the service's constant not-found message rather than formatting the requested id into it.
Traces: kendra-ranking.describe-nonexistent-plan-not-found-rejected, kendra-ranking.delete-plan-removes-resource, kendra-ranking.list-plans-includes-created-plan-with-status

=== Cause 3 of 3: a newly created rescore execution plan never passes through CREATING; it is ACTIVE immediately ===
AWS: Smithy: DescribeRescoreExecutionPlan.Status -> RescoreExecutionPlanStatus, an enum whose values are CREATING, UPDATING, ACTIVE, DELETING, FAILED; Rescore's modeled errors include ValidationException (client, 400). Recorded behavior: immediately after CreateRescoreExecutionPlan returns, DescribeRescoreExecutionPlan reports Status=CREATING, and the plan only later becomes ACTIVE. While it is not ACTIVE, Rescore against that plan is rejected with HTTP 400 / ValidationException (recorded message, as an example: 'Rescore Execution Plan status is invalid, must be in ACTIVE state'). The CREATING state is therefore observable, and it gates the data plane.
Emulator: the plan is already in a terminal/serving state on the first Describe after create -- the check asserted Status==CREATING right after create and found it had already left that state, so the create-then-Rescore conflict window does not exist. Consequently Rescore on a just-created plan is accepted where AWS rejects it, and the CREATING value of the modeled enum is never produced.
Why: a client that polls DescribeRescoreExecutionPlan until ACTIVE before using the plan (the documented and SDK-waiter-shaped usage) is never exercised, and -- worse for correctness -- code paths that must handle ValidationException "must be in ACTIVE state" from Rescore are never reached, so an application that is broken against real AWS passes against the emulator. This is the over-permissive direction: the emulator accepts a data-plane call AWS refuses.
Fix: the rescore-execution-plan lifecycle in the service's state model -- CreateRescoreExecutionPlan must persist the plan in CREATING and expose a transient (ideally steppable/clock-driven) CREATING period before ACTIVE, and the Rescore handler must gate on Status==ACTIVE and raise ValidationException (400) otherwise.
Traces: kendra-ranking.rescore-against-inactive-plan-conflict-rejected

Not filed (named here for completeness, not part of this issue): kendra-ranking.rescore-tokenized-title-body-alternative-input fails only on the magnitude of a relevancy Score produced by AWS's proprietary reranking model (expected >0.3, observed on AWS 0.43-0.45; emulator returned 0.2667 for OriginalScore 0.2). That is an assertion on a value only a live run of the real ranking model produces, so it is not filed as a defect. The related UpdateRescoreExecutionPlan and TagResource failures are the already-filed unimplemented-operation clusters.

## Evidence
Suite: kendra-ranking 6/18 (33%), 12 failed.

Cause 1 -- [mismatch] kendra-ranking.create-plan-invalid-name-pattern-rejected:
  AssertionError: expected message "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9][a-zA-Z0-9_-]*", got "1 validation error detected: Value '-krank71b409600f3e' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9][a-zA-Z0-9_-]*$"
  (error code ValidationException and the rejection itself matched; only the message rendering diverged)

Cause 2 -- [mismatch] kendra-ranking.describe-nonexistent-plan-not-found-rejected:
  AssertionError: expected message 'The specified Resource Execution Plan ID does not exist.', got 'Rescore execution plan 00000000-0000-0000-0000-000000000000 was not found.'
[mismatch] kendra-ranking.delete-plan-removes-resource:
  AssertionError: expected Describe message 'The specified Resource Execution Plan ID does not exist.', got 'Rescore execution plan 01a161ca-c42a-0000-0000-000161cac42a was not found.'
[mismatch] kendra-ranking.list-plans-includes-created-plan-with-status:
  AssertionError: expected DescribeRescoreExecutionPlan message 'The specified Resource Execution Plan ID does not exist.', got 'Rescore execution plan 01a161ca-e226-0000-0000-000161cae226 was not found.'
  (three different ids, one expected constant on AWS -> request-independent on AWS, request-derived on the emulator)

Cause 3 -- [mismatch] kendra-ranking.rescore-against-inactive-plan-conflict-rejected:
  AssertionError: plan left the CREATING state before the CreateRescoreExecutionPlan-Rescore conflict window could be observed; would be closed by an emulator that exposes a slower/steppable transient CREATING period
  (the check does DescribeRescoreExecutionPlan immediately after create and requires Status=='CREATING', then requires Rescore to fail 400/ValidationException until ACTIVE)

Model facts used (smithy_query on services/service-com-amazonaws-kendraranking-awskendrarerankingfrontendservice/smithy-model/com-amazonaws-kendraranking-awskendrarerankingfrontendservice.json):
  CreateRescoreExecutionPlan.Name -> RescoreExecutionPlanName @required @length(1,1000) @pattern "^[a-zA-Z0-9][a-zA-Z0-9_-]*$"; errors include ValidationException(400)
  DescribeRescoreExecutionPlan errors include ResourceNotFoundException(404), ValidationException(400); output.Status -> RescoreExecutionPlanStatus enum [CREATING, UPDATING, ACTIVE, DELETING, FAILED]
  Rescore errors include ValidationException(400), ResourceNotFoundException(404), ConflictException(409)

## Steps to Reproduce
Boot the emulator and point a botocore 'kendra-ranking' client at it.

Cause 1:
  1. CreateRescoreExecutionPlan(Name='-krank-anything')  # leading '-' violates the Name pattern
  2. Observe the ClientError: code is ValidationException (correct), but Message interpolates the submitted value and prints the anchored pattern '^...$'. AWS omits the value and prints the pattern without anchors.

Cause 2:
  1. DescribeRescoreExecutionPlan(Id='00000000-0000-0000-0000-000000000000')
  2. Observe ResourceNotFoundException with Message 'Rescore execution plan 00000000-... was not found.' instead of the constant 'The specified Resource Execution Plan ID does not exist.'
  3. Repeat with a real plan id that was just deleted (CreateRescoreExecutionPlan -> wait ACTIVE -> DeleteRescoreExecutionPlan -> Describe): the same divergence, with that id interpolated.

Cause 3:
  1. plan_id = CreateRescoreExecutionPlan(Name='sim2real-krank-x')['Id']
  2. Immediately DescribeRescoreExecutionPlan(Id=plan_id) -> Status is already ACTIVE; on AWS it is CREATING.
  3. Immediately Rescore(RescoreExecutionPlanId=plan_id, SearchQuery='cats', Documents=[{'Id':'doc-1','Title':'cats','Body':'cats are domestic pets','OriginalScore':1.0}]) -> the emulator serves it; AWS rejects with HTTP 400 ValidationException while the plan is not ACTIVE.
