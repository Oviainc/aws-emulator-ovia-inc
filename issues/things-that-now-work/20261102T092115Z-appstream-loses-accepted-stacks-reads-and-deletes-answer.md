Issue-Source: trace-sweep-divergence
Issue-Trace: appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected appstream-asvpce-traj-endpoint-deleted-under-stack appstream-asvpce-traj-reference-stack-on-pending-endpoint
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream|DescribeStacks|2xx->4xx|$.Stacks
Issue-Key: appstream|DescribeStacks|2xx->4xx|status
Issue-Key: appstream|DeleteStack|2xx->4xx|status
Issue-Commit: 798fb32aa8eb1e75369deb4b48c94746eb1ca248

# appstream loses accepted stacks: reads and deletes answer 404 for an existing stack

> _Note: this issue was identified against the code base at commit `798fb32aa8eb1e75369deb4b48c94746eb1ca248`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Two net-new causes sharing one fault: a stack that the trajectory created is not present in the emulator's stack registry when a later step addresses it, so operations that must succeed are refused as not-found. This is distinct from the refusal-status fault filed separately ("appstream stack not-found refusals diverge: wrong HTTP status and missing refusal"): here the stack DOES exist as far as the recorded service is concerned and the correct answer is a success, not a refusal.

Cause 1 of 2: DescribeStacks for a stack created earlier in the same trajectory answers HTTP 404 with no body instead of HTTP 200 carrying the stack.
AWS: Smithy: DescribeStacks -> DescribeStacksResult { Stacks: StackList, NextToken }. A stack created by CreateStack is immediately readable by name; the recording shows HTTP 200 whose `Stacks[0]` is the full Stack projection (Arn, Name, CreatedTime, State, StackErrors, AccessEndpoints with the endpoint registered at create time, ApplicationSettings, UserSettings, StreamingExperienceSettings). Error ResourceNotFoundException is reserved for names that name no stack.
Emulator: answers HTTP 404 and omits `Stacks` entirely (replay detail: "status: expected 200, got 404; $.Stacks: missing"). The stack the earlier CreateStack in the trajectory accepted is not retrievable.
Why: the whole create-then-read contract is broken for these stacks — a client that creates a stack and then polls DescribeStacks to observe its state or its AccessEndpoints never sees it, so no stack can be driven to readiness or referenced by a dependent resource.
Fix: the stack registry write/lookup path shared by CreateStack and DescribeStacks (the service's state layer plus the DescribeStacks handler's name resolution) — a stack accepted by CreateStack must be keyed so the subsequent by-name read in the same account/region resolves it.
Traces: appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected#task#10 (cluster also covers appstream-asvpce-traj-endpoint-deleted-under-stack#task#9 and appstream-asvpce-traj-reference-stack-on-pending-endpoint#task#7).

Cause 2 of 2: DeleteStack on a stack created earlier in the same trajectory answers HTTP 404 instead of deleting it with HTTP 200.
AWS: Smithy: DeleteStack -> DeleteStackResult (empty output); a successful delete of an existing stack answers HTTP 200 with an empty JSON body. ResourceNotFoundException applies only when the name names no stack, and ResourceInUseException when it is still associated with a fleet.
Emulator: answers HTTP 200 -> 404 (replay detail: "status: expected 200, got 404"), i.e. the stack under its own recorded name cannot be deleted because the registry does not hold it.
Why: teardown cannot complete; a client's cleanup step fails hard, and a re-create of the same name afterwards races against whatever residual state the emulator does hold.
Fix: the same stack-registry lookup used by the DeleteStack handler.
Traces: appstream-asvpce-traj-reference-stack-on-pending-endpoint#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator.

Cause 1 — DescribeStacks, fixture appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected step task#10:
  expected status 200, actual 404.
  $.Stacks: missing. Expected one Stack element shaped as:
    AccessEndpoints [{EndpointType: 'STREAMING', VpceId: <the vpce created in setup>}],
    ApplicationSettings {Enabled: false}, Arn arn:aws:appstream:<region>:<account>:stack/<name>,
    CreatedTime <timestamp>, Name <name>, StackErrors [], State 'ACTIVE',
    StreamingExperienceSettings {PreferredProtocol: 'TCP'},
    UserSettings [the 8 default action/permission entries].
  divergence_key service=appstream|DescribeStacks|status=200->404|keys=$.Stacks,status; affected_count 3.

Cause 2 — DeleteStack, fixture appstream-asvpce-traj-reference-stack-on-pending-endpoint step task#8:
  expected status 200, actual 404.
  divergence_key service=appstream|DeleteStack|status=200->404|keys=status; affected_count 1.

Smithy evidence: DescribeStacks output {Stacks: StackList, NextToken}; DeleteStack output {} with errors [ConcurrentModificationException, OperationNotPermittedException, ResourceInUseException, ResourceNotFoundException].

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: replay fixture appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected through step task#10 — the trajectory creates a VPC endpoint and a stack referencing it, then reads the stack back with DescribeStacks; observe HTTP 404 with no Stacks member where the fixture expects HTTP 200 carrying the stack.
4. Cause 2: replay fixture appstream-asvpce-traj-reference-stack-on-pending-endpoint through step task#8 — DeleteStack on the stack the same trajectory created; observe HTTP 404 instead of an HTTP 200 empty-body success.
