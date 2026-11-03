Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-agent-reports-container-and-task-state-for-unknown-task
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-agent-reports-container-and-task-state-for-unknown-task ecs-cluster-setup-poll-endpoint-and-teardown ecs-agent-apis-and-exec-error-paths ecs-run-task-and-task-protection
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|SubmitContainerStateChange|4xx->4xx|$.__type
Issue-Key: ecs|SubmitContainerStateChange|4xx->4xx|$.message
Issue-Key: ecs|SubmitTaskStateChange|4xx->4xx|$.message
Issue-Key: ecs|SubmitAttachmentStateChanges|4xx->4xx|$.message
Issue-Key: ecs|DiscoverPollEndpoint|4xx->4xx|$.message
Issue-Key: ecs|UpdateTaskProtection|2xx->2xx|$.failures[*]
Issue-Key: ecs|GetTaskProtection|2xx->2xx|$.failures[arn="arn
Issue-Key: ecs|DiscoverPollEndpoint|2xx->2xx|$.serviceConnectEndpoint
Issue-Commit: 20487886de73e0d49e1ee92e88e94f424cdb9e47

# ECS agent-facing operations misvalidate task ARNs instead of rejecting malformed ARNs, and rewrite the ARNs they echo

> _Note: this issue was identified against the code base at commit `20487886de73e0d49e1ee92e88e94f424cdb9e47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitContainerStateChange

## Description
Seven net-new root causes on the ECS agent-facing surface (Submit*StateChange, DiscoverPollEndpoint, Get/UpdateTaskProtection): ARN syntax is never validated before resource lookup, so malformed ARNs produce not-found errors of the wrong class, and the ARNs echoed back in failures are rewritten. All seven must be closed before submitting.

Cause 1 of 7: SubmitContainerStateChange (fixture ecs-agent-reports-container-and-task-state-for-unknown-task, step task#2).
AWS: a malformed task ARN (one whose region segment is not a valid region) is rejected on syntax before any lookup: HTTP 400, error code ClientException, message 'Invalid Region in ARN' — entirely fixed text, no variable parts. Smithy: SubmitContainerStateChange errors include ClientException (400) and InvalidParameterException (400).
Emulator: HTTP 400 with __type 'InvalidParameterException' and message 'The referenced task was not found'.
Why: an agent (or any client) distinguishing a bad request from a vanished task branches on the error code and gets the wrong branch; retry logic keyed on InvalidParameterException retries a permanently malformed ARN.
Fix: ARN parsing/validation in the Submit*StateChange handlers' validation stage, ahead of the task lookup.

Cause 2 of 7: SubmitTaskStateChange (fixture ecs-agent-reports-container-and-task-state-for-unknown-task, step task#3).
AWS: 400 with message 'Invalid Region in ARN' (fixed text) for the same malformed task ARN.
Emulator: 400 with message 'The referenced task was not found'.
Why: the refusal text does not identify the real fault, so an operator debugging an agent sees a missing-task story instead of a malformed ARN.
Fix: as cause 1.

Cause 3 of 7: SubmitAttachmentStateChanges (fixture ecs-agent-reports-container-and-task-state-for-unknown-task, step task#4).
AWS: 400 with message 'Invalid Region in ARN' — the ARN is validated before the attachment is resolved.
Emulator: 400 with message 'Attachment with id <attachment-id> does not exist', where <attachment-id> is the identifier from the request; AWS emits no such message for this class.
Why: as cause 2, and the emulator-invented message makes a client's message match fail.
Fix: ARN validation in SubmitAttachmentStateChanges' validation stage.

Cause 4 of 7: DiscoverPollEndpoint (fixture ecs-cluster-setup-poll-endpoint-and-teardown, step task#3).
AWS: a containerInstance value that is a malformed ARN is rejected with 400 and message 'Invalid Region in ARN' (fixed text). Smithy: DiscoverPollEndpoint errors include ClientException and InvalidParameterException (both 400).
Emulator: 400 with message 'ContainerInstanceId must match ^[a-f0-9-]{1,36}$' — an emulator-authored pattern message AWS never emits.
Why: a client that matches on AWS's refusal text, or an agent that treats an ARN-syntax error distinctly, cannot recognize the failure.
Fix: DiscoverPollEndpoint's validation — validate ARN syntax (region segment) rather than applying a local id regex.

Cause 5 of 7: UpdateTaskProtection (fixture ecs-agent-apis-and-exec-error-paths, step task#6).
AWS: for an unknown task the call answers 200 with a failures entry {"arn": <the task ARN exactly as supplied>, "reason": "MISSING"}; the arn is echoed, not recomposed.
Emulator: 200 but the failures entry's arn is rewritten to arn:aws:ecs:<region>:<account>:task/<cluster-name>/<task-id>, inserting a cluster segment the request did not contain.
Why: a client that matches the failures entry against the ARN it asked about finds no match and concludes the task succeeded.
Fix: UpdateTaskProtection's failure construction must echo the requested identifier verbatim.

Cause 6 of 7: GetTaskProtection (fixture ecs-agent-apis-and-exec-error-paths, step task#7).
AWS: 200 with a failures entry {"arn": <requested task ARN>, "reason": "MISSING"} for the unknown task.
Emulator: 200 with no failures entry keyed on the requested ARN at all (the entry is keyed on the rewritten, cluster-qualified ARN or absent).
Why: the caller gets neither the task nor a failure for it — a silently empty answer.
Fix: GetTaskProtection's failure construction, same root as cause 5.

Cause 7 of 7: DiscoverPollEndpoint (fixture ecs-agent-apis-and-exec-error-paths, step task#2).
AWS: recorded behavior only — the recording asserts the shape of serviceConnectEndpoint (its value is deliberately not asserted, since endpoint hostnames are environment-specific). Smithy: DiscoverPollEndpoint output {endpoint, telemetryEndpoint, serviceConnectEndpoint}, all String.
Emulator: returns 'https://ecs-sc.us-west-2.amazonaws.com/', which does not satisfy the recorded shape for that member. Hypothesis: the emulator emits a full URL with scheme and trailing slash where the recorded Service Connect endpoint is a bare endpoint address; the host name itself is environment-specific and is not the requirement.
Why: an agent that parses serviceConnectEndpoint as an address, not a URL, mis-handles the value.
Fix: DiscoverPollEndpoint's response construction for serviceConnectEndpoint.

## Evidence
Replay of simparity-wire-aws-v1:
- SubmitContainerStateChange 400/400: $.__type expected 'ClientException', got 'InvalidParameterException'; $.message expected 'Invalid Region in ARN', got 'The referenced task was not found'.
- SubmitTaskStateChange 400/400: $.message expected 'Invalid Region in ARN', got 'The referenced task was not found'.
- SubmitAttachmentStateChanges 400/400: $.message expected 'Invalid Region in ARN', got 'Attachment with id <id> does not exist'.
- DiscoverPollEndpoint 400/400: $.message expected 'Invalid Region in ARN', got 'ContainerInstanceId must match ^[a-f0-9-]{1,36}$'.
- UpdateTaskProtection 200/200: $.failures[0] expected {'arn': '...:task/<task-id>', 'reason': 'MISSING'}, got {'arn': '...:task/<cluster>/<task-id>', 'reason': 'MISSING'} (2 affected steps).
- GetTaskProtection 200/200: $.failures[arn="...:task/<task-id>"] expected {'arn': ..., 'reason': 'MISSING'}, got None.
- DiscoverPollEndpoint 200/200: $.serviceConnectEndpoint expected <shaped-not-asserted>, got 'https://ecs-sc.us-west-2.amazonaws.com/' (2 affected steps).
Smithy (ECS 2014-11-13): Submit*StateChange / DiscoverPollEndpoint errors ClientException|InvalidParameterException|ClusterNotFoundException|ServerException; DiscoverPollEndpointResponse {endpoint, telemetryEndpoint, serviceConnectEndpoint}.

## Steps to Reproduce
1. Replay the wire corpus (replay_wire_corpus) and read the ecs agent-API clusters.
2. Or by hand: call SubmitContainerStateChange / SubmitTaskStateChange / SubmitAttachmentStateChanges with a task ARN whose region segment is invalid -> expect 400 ClientException 'Invalid Region in ARN'; the emulator answers InvalidParameterException / not-found wording.
3. Call DiscoverPollEndpoint with a malformed containerInstance ARN -> expect 'Invalid Region in ARN'; the emulator answers a local regex message.
4. Call GetTaskProtection/UpdateTaskProtection with a task ARN of the form arn:aws:ecs:<region>:<account>:task/<task-id> for a task that does not exist -> the failures entry does not echo that ARN.
