Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-stack-failure-rollback-and-cancel-recovery
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-stack-failure-rollback-and-cancel-recovery cloudformation-cfnuf-recovery-calls-on-a-wrong-stack-name cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|CancelUpdateStack|2xx->4xx|$.CancelUpdateStackResponse
Issue-Key: cloudformation|CancelUpdateStack|2xx->4xx|status
Issue-Key: cloudformation|CancelUpdateStack|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|ListStacks|2xx->2xx|$.ListStacksResponse.ListStacksResult.StackSummaries.member[*]
Issue-Related: 20261103T171935Z-cloudformation-change-set-lifecycle-export-guard-and-event.md
Issue-Commit: 62368bc9dd377bdd15bb70a03317a8f201bb87d7

# CloudFormation CancelUpdateStack refusals and ListStacks summaries diverge from AWS

> _Note: this issue was identified against the code base at commit `62368bc9dd377bdd15bb70a03317a8f201bb87d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelUpdateStack

## Description
Three net-new root causes of service `cloudformation` found by replaying the simparity-wire-aws-v1 wire corpus. All three clusters are reported with stub_path='' and owned_by=[], i.e. no open issue carries their replay key. A fixer must close all THREE causes before submitting; each cause names its own fixture#step below and every one of them is listed in this issue's replay pointer (issue_trace/issue_step plus trace_ids).

Two open issues describe adjacent shapes and are named per cause so they can be closed together, but neither owns these keys: 20261103T171935Z-cloudformation-stack-identifier-refusals-use-the-wrong-message.md (the stack-not-found wording on ten OTHER operations) and 20261103T171935Z-cloudformation-state-guards-misfire-operations-accepted-or-refused.md (operations accepted or refused in the wrong stack state).

=== Cause 1 of 3: CancelUpdateStack is refused 400 on a stack whose update is in flight, where AWS accepts it ===
AWS: CancelUpdateStack cancels an update that is in progress and answers HTTP 200 with an empty result body - the query-protocol envelope CancelUpdateStackResponse carrying only ResponseMetadata.RequestId. Smithy: CancelUpdateStack input is StackName (required) + ClientRequestToken (length 1..128, pattern ^[a-zA-Z0-9][-a-zA-Z0-9]*$), output shape is Unit, and the ONLY modelled error is TokenAlreadyExistsException (client, 400). There is no modelled refusal for a stack that is legitimately updating, and the recorded AWS response for this step is 200.
Emulator: answers HTTP 400 with no CancelUpdateStackResponse envelope at all, in the recovery leg of a stack that has just been through a failed create/update and a rollback. (The replay detail carries only the status and the missing envelope, not the emulator's Error.Code/Message, so the exact refusal text is not asserted here - what is asserted is that the call must succeed with 200 and an empty CancelUpdateStackResponse.) Hypothesis: the handler's stack-state guard does not treat the stack's current state as cancellable, so it refuses instead of starting the update rollback.
Why: a client cannot abort an in-flight update at all on this path. The standard recovery sequence (update fails -> cancel -> continue/roll back -> fix forward) dead-ends on a 400, and SDK/CLI callers surface a client error for an operation AWS performs, so the stack is left in its in-progress state with no way out.
Fix: CancelUpdateStack's handler and the stack-state guard it consults in the CloudFormation stack state machine - the operation must be admitted for a stack whose update is in progress and return the empty 200 result.
Traces: cloudformation-stack-failure-rollback-and-cancel-recovery#task#7.

=== Cause 2 of 3: CancelUpdateStack's stack-not-found message uses the bracketed wording instead of AWS's colon form ===
AWS: CancelUpdateStack against a stack name that does not exist is refused 400 with Error.Type 'Sender', Error.Code 'ValidationError' and Message 'Stack: <stackName> does not exist'. The FIXED text is 'Stack: ' and ' does not exist'; the only VARIABLE part is the stack name or stack ARN echoed verbatim from the request's StackName. Smithy: CancelUpdateStack models no not-found error (only TokenAlreadyExistsException), so CloudFormation answers with the service-common ValidationError; the recorded response confirms status 400 and the message template.
Emulator: status 400 and Error.Code are right, but the Message is the bracketed variant 'Stack [<stackName>] does not exist'.
Why: CloudFormation uses a different not-found wording per operation family, and clients, the CLI and CDK diagnostics match on that wording to tell a wrong-name mistake from another validation failure. A caller matching AWS's 'Stack: <name> does not exist' never matches here, so a typo'd stack name is reported as an unrecognised validation error.
Fix: CancelUpdateStack's stack-lookup refusal (the message template it hands to the shared ValidationError construction), not the shared template itself - sibling operations need the 'Stack with id'/'Stack [...]' forms they each record.
Traces: cloudformation-cfnuf-recovery-calls-on-a-wrong-stack-name#task#1.

=== Cause 3 of 3: ListStacks returns only one of the account's CREATE_FAILED stacks ===
AWS: ListStacks returns a StackSummary for EVERY stack in the account and region, including each stack whose creation failed, with StackStatus CREATE_FAILED and StackStatusReason 'The following resource(s) failed to create: [<logical ids>].' (fixed text 'The following resource(s) failed to create: [' / '].', variable part the failing resources' logical ids). Smithy: ListStacks takes no required input, has no modelled errors, and returns the paginated list StackSummaries - a caller that receives a 200 with one member and no NextToken is entitled to treat that as the complete set.
Emulator: with two stacks in the account whose creates both failed on different resources, ListStacks returns a single StackSummary - only the first stack - and never produces the second, so a step that polls ListStacks for both summaries exhausts its 60s budget.
Why: ListStacks is the account-wide inventory call. A failed stack that is invisible to it cannot be found, diagnosed or deleted by a client that does not already know its name, and tooling that reconciles the account against expected stacks concludes the second stack was never created.
Fix: the ListStacks handler's enumeration of the stack store (which stacks it considers listable, and the CREATE_FAILED stacks it filters out), in the CloudFormation stack state.
Traces: cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket#task#3.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout; 3 divergence clusters, all with stub_path='' and owned_by=[].

Cause 1 - fixture cloudformation-stack-failure-rollback-and-cancel-recovery, step task#7, operation CancelUpdateStack, divergence_key 'service=cloudformation|CancelUpdateStack|status=200->400|keys=$.CancelUpdateStackResponse,status': status expected 200, got 400; $.CancelUpdateStackResponse missing (expected the envelope {'ResponseMetadata': {'RequestId': <per-run>}}, got None). The RequestId itself is a per-run value and is not asserted.

Cause 2 - fixture cloudformation-cfnuf-recovery-calls-on-a-wrong-stack-name, step task#1, operation CancelUpdateStack, divergence_key 'service=cloudformation|CancelUpdateStack|status=400->400|keys=$.ErrorResponse.Error.Message': $.ErrorResponse.Error.Message value_mismatch - expected 'Stack: <stackName> does not exist', got 'Stack [<stackName>] does not exist'. Status 400 and Error.Code matched on both sides; only the message template differs. The probe stack name in the recording is a per-run value that fills the template's variable slot.

Cause 3 - fixture cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket, step task#3, operation ListStacks, divergence_key 'service=cloudformation|ListStacks|status=200->200|keys=$,$.ListStacksResponse.ListStacksResult.StackSummaries.member,$.ListStacksResponse.ListStacksResult.StackSummaries.member[1]': the step polled and was capped at 60s. Expected StackSummaries.member to be a TWO-element list, both with StackStatus CREATE_FAILED and StackStatusReason 'The following resource(s) failed to create: [<logical id>].' (one per failing stack); got a single member - member[1] missing. Stack ids/names, creation times and request ids in the recording are per-run values; the requirement is that both failed stacks appear. (Observed but not asserted by the matcher: the emulator's summary also carries a LastOperations member that the recorded AWS summary does not.)

## Steps to Reproduce
1. Clone this checkout, build the emulator and replay the simparity-wire-aws-v1 wire corpus against it.
2. Cause 1: replay fixture cloudformation-stack-failure-rollback-and-cancel-recovery step task#7 - drive the stack through its failed create/update and rollback legs, then call CancelUpdateStack while the update is in progress. Expect HTTP 200 and a CancelUpdateStackResponse envelope; observe 400 and no envelope.
3. Cause 2: replay fixture cloudformation-cfnuf-recovery-calls-on-a-wrong-stack-name step task#1 - CancelUpdateStack with a StackName that was never created. Expect 400 Sender/ValidationError with Message 'Stack: <stackName> does not exist'; observe 'Stack [<stackName>] does not exist'.
4. Cause 3: replay fixture cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket step task#3 - create two stacks that each fail on a different resource, then poll ListStacks with no filter. Expect two StackSummaries, both CREATE_FAILED with their 'The following resource(s) failed to create: [...]' reasons; observe only one summary, so the poll never satisfies and is capped at 60s.
5. Compare each response's HTTP status, Error.Code, Error.Message and body shape against the recorded AWS response.
