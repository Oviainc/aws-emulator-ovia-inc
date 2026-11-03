Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-cfnuf-recovery-calls-on-a-wrong-stack-name
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-cfnuf-recovery-calls-on-a-wrong-stack-name cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward stack-rollback-operations-wrong-state cloudformation-kbdeps-discovery-request-mistakes cloudformation-stack-query-parameter-mistakes cloudformation-cfns3cnt-listing-request-mistakes cloudformation-tsrb-list-stack-resources-bad-stack-references cloudformation-dcsr-stackid-spelling-mistakes cloudformation-refarch-s3ev-stack-lookup-from-bucket cloudformation-tsrb-describe-resources-filter-mistakes cloudformation-dcsr-single-resource-param-mistakes cloudformation-tsrb-active-status-filter-mistakes cloudformation-aorigin-physical-id-candidate-walk
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|ContinueUpdateRollback|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|GetStackPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|DetectStackResourceDrift|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|ListStackResources|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|DescribeStackResources|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|DescribeStackResource|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudformation|ListImports|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudformation|ListStackResources|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudformation|ListStacks|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|DescribeStackResources|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|DescribeStackResources|4xx->2xx|status
Issue-Commit: 2d97abf0bacad2832802b49de9c950da1a09c669

# CloudFormation stack-identifier refusals use the wrong message text and error code

> _Note: this issue was identified against the code base at commit `2d97abf0bacad2832802b49de9c950da1a09c669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ContinueUpdateRollback

## Description
The emulator answers CloudFormation's stack-identifier refusals with a single generic message template and, for malformed input, with a non-CloudFormation error code. This bundle carries 10 distinct causes. Related open issue 20261029T214321Z-cloudformation-the-stack-not-found-refusal-message-is.md describes the 'Stack with id <name> does not exist' wording on five operations; it does NOT own any of these replay keys (the replay reports owned_by empty for all ten), so each must be closed here.

Cause 1 of 10 - ContinueUpdateRollback on a nonexistent stack.
AWS: 400 with Error.Type 'Sender', Error.Code 'ValidationError' and Message 'Stack: <stackNameOrId> does not exist'. Fixed text is 'Stack: ' + ' does not exist'; the variable part is the StackName/stack ARN taken verbatim from the request. Smithy: ContinueUpdateRollback declares no operation-specific not-found error, so CloudFormation uses the service-common ValidationError (Sender/400); recorded response confirms the code and text.
Emulator: 400 ValidationError but Message 'Stack with id <stackNameOrId> does not exist'.
Why: callers and the CLI/CDK diagnostics that match on CloudFormation's wording see a message AWS never emits, so recovery flows cannot distinguish a wrong-name mistake from another validation failure.
Fix: the shared stack-lookup error construction used by the stack operation handlers.
Traces: cloudformation-cfnuf-recovery-calls-on-a-wrong-stack-name#task#2 (cluster also covers cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward#task#5, stack-rollback-operations-wrong-state#task#4).

Cause 2 of 10 - GetStackPolicy on a nonexistent stack.
AWS: 400 Sender/ValidationError, Message 'Stack: <stackNameOrId> does not exist' (same template as cause 1; the identifier is echoed with the request's own casing).
Emulator: 'Stack with id <stackNameOrId> does not exist'.
Why: same as cause 1, on the stack-policy read path.
Fix: GetStackPolicy's stack-lookup refusal.
Traces: cloudformation-cfnuf-recovery-calls-on-a-wrong-stack-name#task#3.

Cause 3 of 10 - DetectStackResourceDrift on a nonexistent stack.
AWS: 400 Sender/ValidationError with the BRACKETED form 'Stack [<stackName>] does not exist' - fixed text 'Stack [' / '] does not exist', variable part the request's StackName.
Emulator: 'Stack with id <stackName> does not exist'.
Why: CloudFormation uses different not-found wordings per operation family; collapsing them all to one template means no caller can rely on any of them.
Fix: DetectStackResourceDrift's stack-lookup refusal.
Traces: cloudformation-stack-query-parameter-mistakes#task#6.

Cause 4 of 10 - ListStackResources with an empty StackName.
AWS: the modeled constraints are checked BEFORE the stack lookup, producing 400 Sender/ValidationError with the aggregated message '<n> validation errors detected: Value '<value>' at 'stackNameOrId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z][-a-zA-Z0-9]*|arn:[-a-zA-Z0-9:/._+]*; Value '<value>' at 'stackNameOrId' failed to satisfy constraint: Member must have length greater than or equal to 1'. Fixed text is the 'N validation errors detected: ' prefix and the per-constraint clauses; the variable parts are the offending value, the member path and the pattern/length from the model. Smithy: StackNameOrId carries length{min:1} and pattern ^([a-zA-Z][-a-zA-Z0-9]*)|(arn:...)$.
Emulator: skips constraint validation and answers the lookup failure 'Stack with id  does not exist'.
Why: a client that sends an empty identifier is told the stack is missing rather than that its request is malformed, so it retries instead of fixing the call.
Fix: ListStackResources' input validation, ahead of its handler's state lookup.
Traces: cloudformation-cfns3cnt-listing-request-mistakes#task#3.

Cause 5 of 10 - DescribeStackResources with an empty StackName.
AWS: same aggregated constraint message as cause 4, with the member path 'stackName'.
Emulator: 'Stack with id  does not exist'.
Why/Fix: as cause 4, in DescribeStackResources' input validation.
Traces: cloudformation-cfns3cnt-listing-request-mistakes#task#5.

Cause 6 of 10 - DescribeStackResource with a malformed parameter.
AWS: 400 with Error.Code 'ValidationError' (Type Sender). CloudFormation's query protocol has no 'MalformedInput' code.
Emulator: 400 with Error.Code 'MalformedInput'.
Why: SDK error classes are keyed on the code, so a caller catching ValidationError sees an unmodeled error instead.
Fix: DescribeStackResource's request validation error mapping.
Traces: cloudformation-dcsr-single-resource-param-mistakes#task#4.

Cause 7 of 10 - ListImports with a malformed parameter: same 'MalformedInput' instead of 'ValidationError'.
Fix: ListImports' request validation error mapping.
Traces: cloudformation-kbdeps-discovery-request-mistakes#task#1.

Cause 8 of 10 - ListStackResources with a malformed parameter: same 'MalformedInput' instead of 'ValidationError'.
Fix: ListStackResources' request validation error mapping.
Traces: cloudformation-tsrb-list-stack-resources-bad-stack-references#task#4.

Cause 9 of 10 - ListStacks with an unmodeled StackStatusFilter value.
AWS: ONE aggregate validation error naming the whole list value at the list member path: '1 validation error detected: Value '[<value>]' at 'stackStatusFilter' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [<the StackStatus set, in the order AWS emits it>]]'. The doubly-nested 'Member must satisfy constraint: [...]' clause and the bracketed list value are fixed text.
Emulator: reports a per-element error instead - 'Value '<value>' at 'stackStatusFilter.1.member' failed to satisfy constraint: Member must satisfy enum value set: [...]' - with the enum printed in Smithy declaration order rather than the order AWS emits.
Why: the message structure (aggregate vs per-member path) is what tells a caller whether one element or the whole list was rejected.
Fix: ListStacks' input validation and its constraint-message formatter.
Traces: cloudformation-cfns3cnt-listing-request-mistakes#task#1.

Cause 10 of 10 - DescribeStackResources by PhysicalResourceId after the owning stack is gone.
AWS: 400 Sender/ValidationError with Message 'Stack for <physicalResourceId> does not exist' - fixed text 'Stack for ' / ' does not exist', variable part the PhysicalResourceId from the request.
Emulator: 200 with a resource list.
Why: the physical-id reverse lookup is how callers find a resource's owning stack; answering 200 for a deleted stack makes a deleted resource look live.
Fix: DescribeStackResources' PhysicalResourceId lookup path in its handler.
Traces: cloudformation-aorigin-physical-id-candidate-walk#teardown#13.

## Evidence
Replay of simparity-wire-aws-v1, 10 divergence clusters, all with stub_path='' and owned_by=[].
Cause 1: expected 400 Message 'Stack: arn:...:stack/<name>/<uuid> does not exist', got 'Stack with id arn:...:stack/<name>/<uuid> does not exist'.
Cause 2: expected 'Stack: <NAME> does not exist', got 'Stack with id <NAME> does not exist'.
Cause 3: expected 'Stack [<name>] does not exist', got 'Stack with id <name> does not exist'.
Cause 4: expected "2 validation errors detected: Value '' at 'stackNameOrId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z][-a-zA-Z0-9]*|arn:[-a-zA-Z0-9:/._+]*; Value '' at 'stackNameOrId' failed to satisfy constraint: Member must have length greater than or equal to 1", got 'Stack with id  does not exist'.
Cause 5: same, member path 'stackName'.
Causes 6-8: expected Error.Code 'ValidationError', got 'MalformedInput' (DescribeStackResource, ListImports, ListStackResources).
Cause 9: expected "1 validation error detected: Value '[ACTIVE]' at 'stackStatusFilter' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [...]]", got "1 validation error detected: Value 'ACTIVE' at 'stackStatusFilter.1.member' failed to satisfy constraint: Member must satisfy enum value set: [...]".
Cause 10: expected 400 ValidationError 'Stack for <physical-id> does not exist', got 200.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. Cause 1: fixture cloudformation-cfnuf-recovery-calls-on-a-wrong-stack-name, step task#2 (ContinueUpdateRollback on a stack ARN that was never created).
3. Causes 2-10: replay the fixture#step bindings listed in trace_ids - GetStackPolicy / DetectStackResourceDrift on a missing stack, ListStackResources and DescribeStackResources with StackName='', DescribeStackResource / ListImports / ListStackResources with a malformed parameter, ListStacks with StackStatusFilter=[ACTIVE], and DescribeStackResources by PhysicalResourceId after teardown.
4. Compare each response's HTTP status, Error.Code and Error.Message against the recorded AWS response.
