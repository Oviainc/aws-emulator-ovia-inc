Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.create-stack-exceeding-parameter-count-limit-rejected cloudformation.hook-result-and-handler-progress-lifecycle
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.create-stack-exceeding-parameter-count-limit-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudformation.hook-result-and-handler-progress-lifecycle|run#6|AssertionError|assert code == "HookResultNotFound", (
Issue-Commit: b4d018646e260fb276f96a884ffb19d3cc9227fb

# CloudFormation: 2 net-new causes — the 200-parameter template-limit refusal message says 'Parameters count' instead of 'Parameter count', and GetHookResult answers NotImplemented (501) instead of HookResultNotFound (404)

> _Note: this issue was identified against the code base at commit `b4d018646e260fb276f96a884ffb19d3cc9227fb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetHookResult

## Description
This issue bundles 2 net-new root causes in the CloudFormation emulator, both grounded in failing behavioral checks that the replay report did not mark as already filed or owned.

Cause 1 of 2: the template-parameter-count refusal message is built with the wrong word, so the refusal text diverges from the one real CloudFormation produces.
AWS: CreateStack refuses a template declaring more than 200 Parameters synchronously with the service-common client error code `ValidationError` (HTTP 400) — Smithy: CreateStack errors are AlreadyExistsException / InsufficientCapabilitiesException / LimitExceededException / TokenAlreadyExistsException plus the service-common ValidationError; the limit itself is not modelled, so this is recorded behavior. The recording shows the message is built from the request and names the violated constraint: `Template format error: Parameter count <n> is greater than max allowed 200` (recorded example, n=201: 'Template format error: Parameter count 201 is greater than max allowed 200'). That construction rule — singular 'Parameter count', the request's own count, and the fixed max 200 — is the requirement, not the recorded sentence as a whole.
Emulator: the code and status are now correct (ValidationError), but the message is emitted with the plural noun: 'Template format error: Parameters count 201 is greater than max allowed 200'.
Why: this is the refusal surface clients and CDK/CLI wrappers pattern-match on to tell a template-content limit apart from the account/region resource-count LimitExceededException; a message that does not follow AWS's construction rule makes that discrimination and any golden-output comparison fail even though the request was correctly refused.
Fix: the message template in CreateStack's synchronous template validation (the parameter-count limit check) must use AWS's wording. Note that the open issue 20261025T015306Z-cloudformation-accepts-templates-and-requests-that-real-aws.md lists this check under its Cause 2 ('the 200-parameter template limit is not enforced'); that cause described the limit NOT being enforced at all and the check no longer fails that way — the refusal now happens and only its message diverges, so that issue's replay key no longer matches this failure and it does not cover it.
Traces: cloudformation.create-stack-exceeding-parameter-count-limit-rejected.

Cause 2 of 2: GetHookResult refuses every lookup with NotImplemented, so the hook-result read model has no not-found taxonomy at all.
AWS: Smithy: GetHookResult takes HookResultId (HookInvocationId, @length min 36 max 36, pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$) and models exactly one error, HookResultNotFoundException — errorKind client, httpError 404, awsQueryError code `HookResultNotFound`. A well-formed HookResultId that was never issued must therefore be refused with HookResultNotFound / HTTP 404. The recorded message for it is 'The hook result was not found.' (example; the model states no construction rule for the text).
Emulator: the call answers error code `NotImplemented` ('operation ... is scaffolded but not implemented in this emulator'), i.e. an error code CloudFormation does not model, with a non-404 status. ListHookResults itself now answers and returns a HookResults list, so the listing half of the hook-result read model works while the per-id read does not.
Why: a client probing a hook result cannot distinguish 'no such result' from 'service broken': the only modelled error for this operation is unreachable, so retry/branch logic keyed on HookResultNotFound never fires, and any hook-driven provisioning observability path dead-ends. The same check's second half (RecordHandlerProgress with an unrecognised BearerToken must be refused with ValidationError and the message "BearerToken '<token>' is invalid", built from the request) is never reached and so stays unverified behind this refusal.
Fix: services/service-com-amazonaws-cloudformation-cloudformation/src/ops/get_hook_result.rs — the handler and the hook-result state it reads, plus the error mapping that must emit HookResultNotFoundException (404, awsQueryError HookResultNotFound) for an unissued id. The open scaffold-stub issue 20261025T015306Z-cloudformation-gethookresult-stub.md names this operation and file; it does not carry this check's replay key, and it says nothing about the required not-found taxonomy, so close them together.
Traces: cloudformation.hook-result-and-handler-progress-lifecycle.

## Evidence
Replayed suite simparity-behavioral-ga-prod-v1 against the emulator, service cloudformation (78/242 checks passing).

Cause 1 — [mismatch] cloudformation.create-stack-exceeding-parameter-count-limit-rejected:
  check_cloudformation_create_stack_exceeding_parameter_count_limit_rejected.py line 34
  AssertionError: expected message 'Template format error: Parameter count 201 is greater than max allowed 200', got 'Template format error: Parameters count 201 is greater than max allowed 200'
  The error code assertion (ValidationError) passed, so only the message construction diverges.

Cause 2 — [mismatch] cloudformation.hook-result-and-handler-progress-lifecycle:
  check_cloudformation_hook_result_and_handler_progress_lifecycle.py line 39
  AssertionError: expected HookResultNotFound for an unissued HookResultId, got 'NotImplemented'
  Expected: GetHookResult(HookResultId='00000000-0000-0000-0000-000000000000') -> HTTP 404, Error.Code 'HookResultNotFound' (modelled HookResultNotFoundException), message e.g. 'The hook result was not found.'
  Actual:   Error.Code 'NotImplemented' — 'operation GetHookResult is scaffolded but not implemented in this emulator'.
  The preceding ListHookResults call in the same check succeeded and returned a HookResults list.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator, then replay the behavioral suite and re-run exactly the check ids in trace_ids.
2. Manual repro (cause 1): CreateStack with a TemplateBody whose Parameters section declares 201 entries ({'Param<i>': {'Type': 'String', 'Default': 'x'}}) and one AWS::SNS::Topic resource. The call is refused with ValidationError as expected, but read Error.Message: the emulator says 'Template format error: Parameters count 201 is greater than max allowed 200' where AWS says 'Template format error: Parameter count 201 is greater than max allowed 200'.
3. Manual repro (cause 2): call GetHookResult with a well-formed but never-issued HookResultId, e.g. '00000000-0000-0000-0000-000000000000'. The emulator answers Error.Code 'NotImplemented'; AWS answers HTTP 404 with Error.Code 'HookResultNotFound'. (ListHookResults in the same session answers normally, which is what makes the gap visible.)
4. Follow-on, currently blocked by step 3: RecordHandlerProgress(BearerToken='<a token no live provisioning request owns>', OperationStatus='SUCCESS') must be refused with ValidationError and the message "BearerToken '<token>' is invalid".
