Issue-Source: trace-sweep-divergence
Issue-Trace: cfnerr-capability-auto-expand-transform
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfnerr-capability-auto-expand-transform cfnerr-capability-named-iam-create-and-update cfnrep-get-template-processed-transform cloudformation-stack-recovery-operation-errors cfnerr-client-request-token-reuse cloudformation-stack-policy-and-termination-protection
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|CreateStack|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|CreateStack|4xx->2xx|status
Issue-Key: cloudformation|UpdateStack|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|UpdateStack|4xx->2xx|status
Issue-Key: cloudformation|CreateStack|2xx->4xx|$.CreateStackResponse
Issue-Key: cloudformation|CreateStack|2xx->4xx|status
Issue-Key: cloudformation|UpdateStack|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudformation|UpdateStack|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|DeleteStack|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|DeleteStack|4xx->2xx|status
Issue-Related: 20261022T043551Z-cloudformation-query-error-envelope-omits-the-requestid-element.md
Issue-Commit: bffba5cba440022693dbdf16c8135b8d2c175737

# CloudFormation stack mutation guards not enforced: capabilities, client request tokens, termination protection

> _Note: this issue was identified against the code base at commit `bffba5cba440022693dbdf16c8135b8d2c175737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
FIVE net-new causes: the mutating stack operations accept requests AWS rejects, and reject one AWS accepts — the pre-conditions AWS enforces before mutating a stack (required Capabilities, client-request-token uniqueness, termination protection) are not implemented.

Cause 1 of 5 — CreateStack does not require CAPABILITY_AUTO_EXPAND (fixture cfnerr-capability-auto-expand-transform, task#1; cluster also covers cfnerr-capability-named-iam-create-and-update#task#3, cfnrep-get-template-processed-transform#task#1, cloudformation-stack-recovery-operation-errors#task#6).
AWS: a CreateStack whose template uses a macro/Transform and whose Capabilities omit CAPABILITY_AUTO_EXPAND is rejected with HTTP 400 and error Code InsufficientCapabilitiesException. Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json): CreateStack errors include InsufficientCapabilitiesException, errorKind client, httpError 400, awsQueryError code InsufficientCapabilitiesException, doc 'The template contains resources with capabilities that weren't specified in the Capabilities parameter.' Recorded message 'Requires capabilities : [CAPABILITY_AUTO_EXPAND]' is an example of the text; the requirement is 400 + that Code.
Emulator: returns HTTP 200 and creates the stack — no ErrorResponse at all.
Why: the capability acknowledgement is a safety gate. A client (or a human review step) that relies on CloudFormation refusing an unacknowledged macro/IAM template sees it silently succeed, so tests that assert the guard pass against the emulator and fail against AWS, and negative-path handling is never exercised.
Fix: capability validation in the CreateStack handler's request validation (services/service-com-amazonaws-cloudformation-cloudformation): compute the capabilities the template requires and raise InsufficientCapabilitiesException (400) when Capabilities does not contain them.
Traces: cfnerr-capability-auto-expand-transform#task#1.

Cause 2 of 5 — UpdateStack does not require CAPABILITY_NAMED_IAM. AWS: an UpdateStack introducing a named IAM resource without CAPABILITY_NAMED_IAM is rejected 400 InsufficientCapabilitiesException (Smithy: UpdateStack errors include InsufficientCapabilitiesException, httpError 400; recorded message 'Requires capabilities : [CAPABILITY_NAMED_IAM]' is an example). Emulator: 200, update applied. Why/Fix: as cause 1, in the UpdateStack handler. Traces: cfnerr-capability-named-iam-create-and-update#task#5.

Cause 3 of 5 — CreateStack with a ClientRequestToken is rejected outright. AWS: CreateStack(StackName=<new>, ClientRequestToken=<token>) returns 200 with CreateStackResult.StackId. Smithy: CreateStack.ClientRequestToken is a normal optional member (length 1..128, pattern ^[a-zA-Z0-9][-a-zA-Z0-9]*$) and the op's modeled errors do not apply to a valid first-use create. Emulator: HTTP 400, no CreateStackResponse. Hypothesis: the token-bearing create path rejects the request instead of recording the token. Why: a client that passes a client request token for idempotency/traceability — the documented way to correlate stack events — cannot create a stack at all. Fix: the CreateStack handler must accept and persist ClientRequestToken and return 200 with the StackId. Traces: cfnerr-client-request-token-reuse#task#2.

Cause 4 of 5 — reused ClientRequestToken yields the wrong error class. AWS: an UpdateStack reusing a ClientRequestToken already used for that stack is rejected 400 with Code TokenAlreadyExistsException (Smithy: UpdateStack errors include TokenAlreadyExistsException, errorKind client, httpError 400, awsQueryError code TokenAlreadyExistsException, doc 'A client request token already exists.'). Recorded message ('The client request token <tok> has already been used for this stack...') is an example. Emulator: 400 but Code ValidationError, with a message about the stack being in CREATE_IN_PROGRESS — i.e. it never checks token reuse, and the envelope also omits RequestId. Why: SDK retry/idempotency logic branches on the error code; TokenAlreadyExistsException is a distinct, non-retryable duplicate-submission signal, while ValidationError reads as a malformed request, so a client retries or aborts incorrectly. Fix: record used client request tokens per stack in stack state and raise TokenAlreadyExistsException from the CreateStack/UpdateStack/DeleteStack handlers on reuse. Traces: cfnerr-client-request-token-reuse#task#3.

Cause 5 of 5 — DeleteStack ignores termination protection. AWS: DeleteStack against a stack with EnableTerminationProtection=true is rejected 400 with Code ValidationError (recorded message 'Stack [<name>] cannot be deleted while TerminationProtection is enabled' — an example of the text; the requirement is 400 + ValidationError and that the stack survives). Smithy: DeleteStack models only TokenAlreadyExistsException, so the protection refusal is the service-common ValidationError seen in the recording. Emulator: returns 200 and proceeds with deletion. Why: termination protection exists precisely to make accidental deletion impossible; an emulator that deletes a protected stack validates the opposite of AWS's behavior and lets destructive flows pass review. Fix: the DeleteStack handler must read the stack's termination-protection flag and refuse (400 ValidationError) while it is set — see the companion DescribeStacks issue, where that flag is also not persisted. Traces: cloudformation-stack-policy-and-termination-protection#task#12.

## Evidence
Replay of simparity-wire-aws-v1.

Cause 1 — cfnerr-capability-auto-expand-transform#task#1, CreateStack:
  status: expected 400, got 200
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'InsufficientCapabilitiesException', 'Message': 'Requires capabilities : [CAPABILITY_AUTO_EXPAND]'}, 'RequestId': ...}, got None)
  cluster: cfnerr-capability-named-iam-create-and-update#task#3, cfnrep-get-template-processed-transform#task#1, cloudformation-stack-recovery-operation-errors#task#6

Cause 2 — cfnerr-capability-named-iam-create-and-update#task#5, UpdateStack:
  status: expected 400, got 200
  $.ErrorResponse: missing (expected Code 'InsufficientCapabilitiesException', Message 'Requires capabilities : [CAPABILITY_NAMED_IAM]')

Cause 3 — cfnerr-client-request-token-reuse#task#2, CreateStack:
  status: expected 200, got 400
  $.CreateStackResponse: missing (expected CreateStackResult.StackId 'arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-probe-cfnerr-tok-stack-.../...')

Cause 4 — cfnerr-client-request-token-reuse#task#3, UpdateStack (status 400 both sides):
  $.ErrorResponse.Error.Code: value_mismatch (expected 'TokenAlreadyExistsException', got 'ValidationError')
  $.ErrorResponse.Error.Message: expected 'The client request token ... has already been used for this stack. Retry the request with a different client request token', got 'Stack:arn:... is in CREATE_IN_PROGRESS state and can not be updated.' (message quoted as an example; the Code is the requirement)
  $.ErrorResponse.RequestId: missing

Cause 5 — cloudformation-stack-policy-and-termination-protection#task#12, DeleteStack:
  status: expected 400, got 200
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'ValidationError', 'Message': 'Stack [...] cannot be deleted while TerminationProtection is enabled'}})

Stack names/ARNs and request ids are environment-specific examples.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateStack with a template declaring a Transform/macro and Capabilities=[] (cfnerr-capability-auto-expand-transform#task#1): emulator returns 200 and creates the stack; AWS returns 400 InsufficientCapabilitiesException.
3. UpdateStack introducing a named IAM resource with Capabilities=[] (cfnerr-capability-named-iam-create-and-update#task#5): emulator returns 200; AWS returns 400 InsufficientCapabilitiesException.
4. CreateStack(StackName=<new>, ClientRequestToken=<token>) (cfnerr-client-request-token-reuse#task#2): emulator returns 400 with no CreateStackResult; AWS returns 200 with a StackId.
5. Reuse the same ClientRequestToken on UpdateStack for that stack (cfnerr-client-request-token-reuse#task#3): emulator returns 400 Code ValidationError; AWS returns 400 Code TokenAlreadyExistsException.
6. UpdateTerminationProtection(EnableTerminationProtection=true) then DeleteStack (cloudformation-stack-policy-and-termination-protection#task#12): emulator returns 200 and deletes; AWS returns 400 ValidationError and keeps the stack.
7. Re-replay the representative plus the four bindings in Issue-Trace-Ids to confirm the fix.
