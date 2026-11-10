Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-microvms-run-suspend-resume-terminate-microvm
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-microvms-run-suspend-resume-terminate-microvm lambdamicrovms-image-version-status-and-update
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms|RunMicrovm|2xx->2xx|$.egressNetworkConnectors
Issue-Key: lambda-microvms|RunMicrovm|2xx->2xx|$.endpoint
Issue-Key: lambda-microvms|RunMicrovm|2xx->2xx|$.ingressNetworkConnectors
Issue-Key: lambda-microvms|RunMicrovm|2xx->2xx|$.microvmId
Issue-Key: lambda-microvms|RunMicrovm|2xx->2xx|$.startedAt
Issue-Key: lambda-microvms|GetMicrovm|2xx->2xx|$.egressNetworkConnectors
Issue-Key: lambda-microvms|GetMicrovm|2xx->2xx|$.endpoint
Issue-Key: lambda-microvms|GetMicrovm|2xx->2xx|$.ingressNetworkConnectors
Issue-Key: lambda-microvms|GetMicrovm|2xx->2xx|$.startedAt
Issue-Key: lambda-microvms|GetMicrovm|2xx->2xx|$.stateReason
Issue-Key: lambda-microvms|GetMicrovm|2xx->2xx|$.terminatedAt
Issue-Key: lambda-microvms|UpdateMicrovmImageVersion|2xx->2xx|$.versionStateTimeBucket
Issue-Commit: d9137e240d6d86ce2d0eba5736851daf32b7e9eb

# lambda-microvms MicroVM responses drop the attached network connectors and mis-set the termination stateReason

> _Note: this issue was identified against the code base at commit `d9137e240d6d86ce2d0eba5736851daf32b7e9eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunMicrovm

## Description
Four net-new root causes found by replaying the wire corpus against the emulator, all of them lambda-microvms response records that omit a member AWS reports or report a member with the wrong value. CAUSE COUNT: 4. All four must be closed before this issue is submitted, and all four bindings must be re-replayed: lambda-microvms-run-suspend-resume-terminate-microvm#task#7, #task#8 (whose cluster also covers #task#10 and #task#12), #task#14, and lambdamicrovms-image-version-status-and-update#task#4.

--- Cause 1 of 4: RunMicrovm's response omits ingressNetworkConnectors and egressNetworkConnectors when the request names none ---

AWS: Smithy com.amazonaws.lambdamicrovms#RunMicrovm (POST /2025-09-09/microvms) -> RunMicrovmResponse members `ingressNetworkConnectors` and `egressNetworkConnectors` (NetworkConnectorList, member NetworkConnector len 1..2048, list @length max 10), documented as "The list of ingress/egress network connectors configured for the MicroVM". The model does not state what a MicroVM is attached to when the request names no connector, so for that default this is recorded behavior only: the recorded 200 response carries ingressNetworkConnectors = ['arn:aws:lambda:<region>:aws:network-connector:aws-network-connector:HTTP_INGRESS'] and egressNetworkConnectors = ['arn:aws:lambda:<region>:aws:network-connector:aws-network-connector:INTERNET_EGRESS'] — the two AWS-managed connectors, in the AWS-owned ARN namespace (account field 'aws'). Hypothesis: AWS attaches those managed connectors by default and reports them on the MicroVM, which is why the members are present with no connector in the request.

Emulator: both members are absent from the response (replay: '$.ingressNetworkConnectors: missing', '$.egressNetworkConnectors: missing', got None). The handler stores only what the request carried — services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/run_microvm.rs:377-382 stores `input.ingress_network_connectors.unwrap_or_default()` / `input.egress_network_connectors.unwrap_or_default()`, i.e. empty lists, and line 516-517 renders them through `crate::state::wire_list`, which drops an empty list from the wire — so a MicroVM run with no explicit connector reports no connectors at all.

Why: the connector lists are how a client learns what network paths its MicroVM actually has. Against this emulator a MicroVM that AWS reports as reachable over the managed HTTP ingress connector and able to egress to the internet reports neither, so a client that asserts on, logs, or diffs its MicroVM's attachments sees an unattached MicroVM and cannot distinguish the managed default from "no connectivity".

Fix: RunMicrovm's handler (the connector defaulting when the input lists are absent/empty) and the Microvm record in the lambda-microvms service state that must hold those defaults.

Traces: lambda-microvms-run-suspend-resume-terminate-microvm#task#7.

--- Cause 2 of 4: GetMicrovm omits the same two connector lists for the MicroVM's whole lifecycle ---

AWS: Smithy com.amazonaws.lambdamicrovms#GetMicrovm (GET /2025-09-09/microvms/{microvmIdentifier}, @readonly) -> GetMicrovmResponse carries the same `ingressNetworkConnectors` / `egressNetworkConnectors` members, and the recording reports the managed HTTP_INGRESS / INTERNET_EGRESS ARNs on every read of the MicroVM — after it reaches RUNNING, after SuspendMicrovm and after ResumeMicrovm alike. A connector attachment is a property of the MicroVM and does not change across suspend/resume.

Emulator: every GetMicrovm in the lifecycle answers 200 with both members missing. One cluster covers three steps of the fixture (task#8 RUNNING, task#10 SUSPENDED, task#12 RUNNING after resume), so the read path never reports connectors regardless of state. Same root as cause 1 on the read side: the stored lists are empty and `wire_list` omits them.

Why: a client polling GetMicrovm to confirm its MicroVM's network attachments after a suspend/resume cycle never sees them, so it cannot verify that suspension preserved the MicroVM's connectivity.

Fix: GetMicrovm's response projection plus the same stored-connector defaulting in the lambda-microvms service state. NOTE FOR THE FIXER: open issues 20261108T144919Z-lambda-core-lambda-microvms-lambda-core-vpc-egress-connector-reachability-cannot-be.md and 20261108T130022Z-lambda-core-lambda-microvms-lambda-core-vpc-egress-connector-data-plane-is.md are about the MicroVM endpoint answering 501 instead of the guest's response (whether a connector is ENFORCED as a network path); this cause is that the connector list is not REPORTED at all, and the replay does not mark these steps owned by either.

Traces: lambda-microvms-run-suspend-resume-terminate-microvm#task#8 (cluster also covers #task#10 and #task#12).

--- Cause 3 of 4: after TerminateMicrovm, GetMicrovm reports stateReason 'Client.UserInitiatedTermination' instead of AWS's 'Success.' ---

AWS: Smithy com.amazonaws.lambdamicrovms#GetMicrovm output `stateReason` (NonBlankString, len 1..2048, pattern ^[^\s]+$), "The reason for why the MicroVM is in the current state"; TerminateMicrovm is DELETE /2025-09-09/microvms/{microvmIdentifier}, @idempotent, with an empty output. The model does not enumerate the reason tokens, so the value is recorded behavior only: for a MicroVM that reached TERMINATED because the customer called TerminateMicrovm, the recorded GetMicrovm response reports stateReason exactly 'Success.' — whole fixed text, no variable part. That is the value for a clean, successful termination; a reason that diagnoses a failure is a different value.

Emulator: the same read answers 200 with stateReason 'Client.UserInitiatedTermination', a token AWS has no counterpart for here. TerminateMicrovm writes it: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/terminate_microvm.rs:181 sets `vm.state_reason = Some(crate::state::TERMINATION_REASON.to_string())`.

Why: stateReason is a requirement of the response, and a client that branches on it (success vs. a platform-initiated or failed termination) classifies a normal customer termination as something else, and any log or assertion comparing the reason against AWS's text fails.

Fix: TerminateMicrovm's handler / the TERMINATION_REASON constant in the lambda-microvms service state must record the reason AWS reports for a successful user-initiated termination.

Traces: lambda-microvms-run-suspend-resume-terminate-microvm#task#14.

--- Cause 4 of 4: UpdateMicrovmImageVersion's 200 response omits versionStateTimeBucket ---

AWS: recorded behavior only — `versionStateTimeBucket` is NOT a member of UpdateMicrovmImageVersionResponse in the Smithy model (its modeled output members are baseImageArn, baseImageVersion, buildRoleArn, description, codeArtifact, logging, egressNetworkConnectors, cpuConfigurations, resources, additionalOsCapabilities, hooks, environmentVariables, imageArn, imageVersion, state, status, createdAt, updatedAt, stateReason, tags), yet the recorded 200 response carries it. Its value is the composite key '<version state>#<time bucket>' (observed elsewhere in the corpus as 'FAILED#26110509': the MicrovmImageVersionState token, a literal '#', then a compact bucket derived from the state-transition time). The bucket digits are a per-run value; the requirement is the member's presence and that composite form.

Emulator: the member is absent from the response (replay: '$.versionStateTimeBucket: missing (expected <shaped-not-asserted>, got None)'). The projection emits only modeled members: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/update_microvm_image_version.rs:294-322 (`rendered`).

Why: a client that reads the bucket key back from the status update to index or sort versions by state-and-time gets nothing, and must re-read the version — where this emulator reports the key in a different format again.

Fix: the UpdateMicrovmImageVersion response projection/serializer, plus the shared version projection in the lambda-microvms service state that computes '<STATE>#<bucket>' from the stored version. NOTE FOR THE FIXER: the read paths do emit versionStateTimeBucket but as an ISO-8601 instant ('2026-11-10T01:00:00Z'); those steps are owned by 20261103T192714Z-lambda-microvms-getmicrovmimageversion-reports-a-failed-build-as.md and 20261108T125129Z-lambda-microvms-image-version-and-build-read-paths.md, so one correct value computation serves them and this cause. Open issue 20261103T220031Z-lambda-microvms-updatemicrovmimageversion-stub.md is a stub-implementation issue; the operation now answers 200, and the replay reports no stub for it.

Traces: lambdamicrovms-image-version-status-and-update#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, 104 steps, 78 passed, 26 mismatched, score 75.0%.

[mismatch] fixture lambda-microvms-run-suspend-resume-terminate-microvm step task#7 — RunMicrovm, expected HTTP 200, got HTTP 200
  $.ingressNetworkConnectors: missing (expected ['arn:aws:lambda:us-west-2:aws:network-connector:aws-network-connector:HTTP_INGRESS'], got None)
  $.egressNetworkConnectors: missing (expected ['arn:aws:lambda:us-west-2:aws:network-connector:aws-network-connector:INTERNET_EGRESS'], got None)
  Source: src/ops/run_microvm.rs:377-382 (stores unwrap_or_default() of the absent input lists) and :516-517 (wire_list drops the empty lists).
  The same cluster also reported $.microvmId, $.endpoint and $.startedAt as diverging, but the fixture records them as '<shaped-not-asserted>': the generated MicroVM id, the emulator's own host:port endpoint and the run timestamp are per-run values and are NOT filed as requirements here.

[mismatch] fixture lambda-microvms-run-suspend-resume-terminate-microvm step task#8 — GetMicrovm, expected HTTP 200, got HTTP 200 (cluster of 3: task#8, task#10, task#12 — RUNNING, SUSPENDED, RUNNING-after-resume)
  $.ingressNetworkConnectors: missing (expected ['...:aws-network-connector:HTTP_INGRESS'], got None)
  $.egressNetworkConnectors: missing (expected ['...:aws-network-connector:INTERNET_EGRESS'], got None)
  ($.endpoint and $.startedAt in the same cluster are '<shaped-not-asserted>' per-run values — not filed.)

[mismatch] fixture lambda-microvms-run-suspend-resume-terminate-microvm step task#14 — GetMicrovm after TerminateMicrovm, expected HTTP 200, got HTTP 200
  $.stateReason: value_mismatch (expected 'Success.', got 'Client.UserInitiatedTermination')
  $.ingressNetworkConnectors / $.egressNetworkConnectors: missing (as above)
  Source: src/ops/terminate_microvm.rs:181 (vm.state_reason = crate::state::TERMINATION_REASON).
  ($.terminatedAt / $.endpoint / $.startedAt are '<shaped-not-asserted>' per-run values — not filed.)

[mismatch] fixture lambdamicrovms-image-version-status-and-update step task#4 — UpdateMicrovmImageVersion, expected HTTP 200, got HTTP 200
  $.versionStateTimeBucket: missing (expected '<shaped-not-asserted>', got None)
  Elsewhere in the corpus the recorded value of this member is 'FAILED#26110509', i.e. '<state>#<bucket>'; the emulator's read paths instead report '2026-11-10T01:00:00Z'.
  Source: src/ops/update_microvm_image_version.rs:294-322 (`rendered`).

## Steps to Reproduce
Against the emulator (lambda-microvms stood up), base path /2025-09-09:

Causes 1-3 (MicroVM lifecycle):
1. CreateMicrovmImage from a buildable code artifact and poll GetMicrovmImage / GetMicrovmImageVersion until the image is CREATED with an ACTIVE, SUCCESSFUL version.
2. RunMicrovm(imageIdentifier=<imageArn>) with NO ingressNetworkConnectors and NO egressNetworkConnectors in the request.
   Expected: HTTP 200 whose body includes ingressNetworkConnectors = ['arn:aws:lambda:<region>:aws:network-connector:aws-network-connector:HTTP_INGRESS'] and egressNetworkConnectors = ['arn:aws:lambda:<region>:aws:network-connector:aws-network-connector:INTERNET_EGRESS'].
   Observed: HTTP 200 with neither member present.
3. Poll GetMicrovm(<microvmId>) until state=RUNNING, then SuspendMicrovm and poll until SUSPENDED, then ResumeMicrovm and poll until RUNNING.
   Expected: every read reports the same two connector lists.
   Observed: both members absent on every read, in every state.
4. TerminateMicrovm(<microvmId>), then poll GetMicrovm until state=TERMINATED and read stateReason.
   Expected: stateReason == 'Success.'
   Observed: stateReason == 'Client.UserInitiatedTermination'.

Cause 4 (image version status update):
5. CreateMicrovmImage, then UpdateMicrovmImageVersion(imageIdentifier=<imageArn>, imageVersion=<version>, status='INACTIVE') — PATCH /2025-09-09/microvm-images/{imageIdentifier}/versions/{imageVersion}.
   Expected: HTTP 200 whose body includes versionStateTimeBucket of the form '<version state>#<time bucket>' (e.g. 'FAILED#<bucket>' for a failed build, 'SUCCESSFUL#<bucket>' for a successful one).
   Observed: HTTP 200 with no versionStateTimeBucket member.
