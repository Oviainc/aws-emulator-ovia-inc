Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-microvms.get-microvm-image-access-denied lambda-microvms.get-microvm-image-build-access-denied lambda-microvms.idle-policy-auto-suspends-microvm
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms.get-microvm-image-access-denied|run#11|AssertionError|assert bogus_msg == (
Issue-Key: lambda-microvms.get-microvm-image-build-access-denied|run#19|AssertionError|assert message == (
Issue-Key: lambda-microvms.idle-policy-auto-suspends-microvm|_wait_for_microvm_state#8|AssertionError|raise AssertionError(f"microvm {microvm_id} never reached {target_states}, last state={last}")
Issue-Commit: 8d55b39ab66a28bf4265bc0ae130d6078a9d6cb6

# lambda-microvms: 2 net-new root causes — the resource-scoped AccessDeniedException message form is never produced, and a MicroVM's idlePolicy never auto-suspends it

> _Note: this issue was identified against the code base at commit `8d55b39ab66a28bf4265bc0ae130d6078a9d6cb6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMicrovmImage

## Description
Two net-new root causes, grounded in 3 failing behavioral checks of this suite.

=== Cause 1 of 2: every denial answers the caller-only IAM sentence; the resource-scoped form naming the IAM action and the requested identifier is never produced ===
Failing checks: lambda-microvms.get-microvm-image-access-denied, lambda-microvms.get-microvm-image-build-access-denied

AWS: Smithy (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json) declares AccessDeniedException as a service-common error, errorKind client, httpError 403, with one optional member `message`; its doc string ("You do not have sufficient access to perform this action.") describes the error CLASS, not the wire message, so the message text is recorded behavior only. The recordings show AWS uses two forms, chosen by what the identifier names:
  (a) an identifier naming a resource the principal is simply not permitted on (an AWS-owned managed base-image ARN, account segment `aws`): HTTP 403, code AccessDeniedException, message `User: <caller-arn> is not authorized to perform this operation`. Fixed text: `User: ` and ` is not authorized to perform this operation`; the only variable part is <caller-arn>, the signing principal's ARN (the value STS GetCallerIdentity answers in `Arn`).
  (b) an identifier that is well-formed for this resource type but names nothing the principal was granted (e.g. `arn:aws:lambda:<region>:<account>:microvm-image/<name>`): HTTP 403, code AccessDeniedException, message `User: <caller-arn> is not authorized to perform: lambda:<OperationName> on resource: <identifier>`. Fixed text: `User: `, ` is not authorized to perform: `, `lambda:`, ` on resource: `. Variable parts: <caller-arn> (caller identity), <OperationName> (the refused operation, giving the IAM action name — GetMicrovmImage, GetMicrovmImageBuild, …) and <identifier> (the identifier string the request carried, verbatim).
Both forms confirm authorization is decided before existence, which the emulator already does.

Emulator: status 403 and code AccessDeniedException are correct, and form (a) is now correct, but form (b) does not exist. `crate::state::access_denied_message()` takes NO arguments and always builds form (a) (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state.rs:798-806, whose own comment records the decision: "the sentence carries nothing of the request and a denial says nothing about what the identifier it refused names"), and every denial branch of every customer-facing read calls exactly that (get_microvm_image.rs:200-207, list_microvm_image_builds.rs, get_microvm_image_build.rs, run_microvm.rs:239-246). So a request carrying a well-formed-but-ungranted identifier is answered with the caller-only sentence where AWS names the action and the resource. Observed on GetMicrovmImage and on GetMicrovmImageBuild; ListMicrovmImageVersions/ListMicrovmImageBuilds take the identical path through the same argument-less builder.

Why: the message is the only part of a 403 that tells a client WHICH action on WHICH resource was refused. IAM-diagnosis tooling, operator runbooks and test assertions parse form (b) to turn a denial into an actionable grant ("add lambda:GetMicrovmImage on arn:…"). Against the emulator every denial is one sentence that names neither the action nor the resource, so a caller cannot distinguish a permission boundary it may never cross (a managed base image) from a missing grant on a specific resource, and cannot learn which identifier was refused when one call touches several.

Fix: this service's denial-message construction in the service state — replace the single argument-less builder with per-form builders (one caller-only, one taking the IAM action name and the requested identifier) alongside the other message templates there — plus each customer-facing read/launch handler's denial branch, so each passes its own operation name and the identifier the request carried, and the branch picks form (b) for a well-formed identifier of this account's own namespace that names nothing granted. The caller principal must keep coming from the request's signing identity.
Related open issue, which does NOT cover this: 20261103T022935Z-lambda-microvms-accessdeniedexception-message-is-the-model-s.md described the message as one cause and is still open, but only its form (a) has landed — the emulator now emits form (a) correctly and these checks fail one assertion later, on the form (b) message. Its replay keys name the form (a) assertions, so the form (b) requirement is not carried by any open issue.

=== Cause 2 of 2: a MicroVM's idlePolicy never arms an idle timer, so it stays RUNNING forever instead of auto-suspending ===
Failing check: lambda-microvms.idle-policy-auto-suspends-microvm

AWS: Smithy IdlePolicy is documented "Configuration that controls MicroVM auto-suspend and auto-resume behavior. Idle time is measured by inbound traffic through the MicroVM proxy endpoint — if no requests arrive within the configured duration, the MicroVM is suspended." Its members are all required: maxIdleDurationSeconds (Integer, @range min 60) "The maximum time in seconds that a MicroVM can remain idle before it is automatically suspended", suspendedDurationSeconds (Integer, @range min 0) "The maximum time in seconds that a MicroVM can remain suspended before it is automatically terminated", and autoResumeEnabled (Boolean). IdlePolicy is accepted on RunMicrovm input and reported on RunMicrovm/GetMicrovm output. So a MicroVM launched with an idlePolicy and then left without any inbound request through its proxy endpoint transitions out of RUNNING on its own, with no SuspendMicrovm call: GetMicrovm reports state SUSPENDED (via SUSPENDING, per MicrovmState PENDING|RUNNING|SUSPENDING|SUSPENDED|TERMINATING|TERMINATED) once maxIdleDurationSeconds has elapsed since the last inbound request, and no earlier than that — the timer must actually run, not fire immediately on launch.

Emulator: the idle policy is stored and echoed but drives nothing. RunMicrovm persists `idle_policy: input.idle_policy` and explicitly sets `suspend_due_at: None` (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/run_microvm.rs:371, 386), and the clock-owed transition pass only ever sets suspend_due_at from an EXPLICIT SuspendMicrovm (state.rs:1035-1046 start_microvm_transition, applied in materialize_due at state.rs:1607-1612). Nothing derives a due instant from maxIdleDurationSeconds and nothing records an inbound-request timestamp on the MicroVM, so the MicroVM remains RUNNING indefinitely until maximumDurationInSeconds expiry terminates it. The check polled for 300 s for {SUSPENDED, TERMINATED} with maxIdleDurationSeconds=60 and read state=RUNNING throughout.

Why: auto-suspend is the whole point of an idle policy — it is how a client caps the cost of an idle MicroVM and how autoResumeEnabled/suspendedDurationSeconds become reachable at all. A client that configures an idle policy and waits for SUSPENDED (to assert its own cost controls, or to then exercise auto-resume or the suspended-duration auto-termination) waits forever against the emulator and cannot observe any part of the idle lifecycle; a scheduler that treats SUSPENDED as "safe to reclaim" never fires.

Fix: the MicroVM state's clock-owed transition machinery plus RunMicrovm's record construction — RunMicrovm must arm an idle-suspend due instant from idlePolicy.maxIdleDurationSeconds when an idlePolicy is present, the MicroVM endpoint's request path must reset that instant on every inbound request (idle time is measured by inbound traffic), and materialize_due must take a MicroVM whose idle instant has come due from RUNNING through SUSPENDING to SUSPENDED (and, per suspendedDurationSeconds, on to TERMINATED), leaving the explicit SuspendMicrovm path unchanged.

Traces: lambda-microvms.get-microvm-image-access-denied (GetMicrovmImage on a managed ARN then on a well-formed ungranted ARN — form (a) passes, form (b) fails); lambda-microvms.get-microvm-image-build-access-denied (ListMicrovmImageBuilds and GetMicrovmImageBuild on a managed image version, then GetMicrovmImageBuild on an ungranted image ARN — fails on form (b)); lambda-microvms.idle-policy-auto-suspends-microvm (RunMicrovm with idlePolicy {maxIdleDurationSeconds: 60, suspendedDurationSeconds: 300, autoResumeEnabled: true}, then poll GetMicrovm).

## Evidence
Cause 1 — resource-scoped denial message never produced:

  [mismatch] lambda-microvms.get-microvm-image-access-denied
    check_lambda_microvms_get_microvm_image_access_denied.py:34
    assert bogus_msg == (f"User: {caller} is not authorized to perform: lambda:GetMicrovmImage on resource: {bogus_identifier}")
    AssertionError: User: arn:aws:iam::000000000000:user/emulator is not authorized to perform this operation
    (the preceding form-(a) assertion on the managed ARN and both code assertions passed; bogus_identifier = arn:aws:lambda:<region>:<account>:microvm-image/<a-name-nobody-created>)

  [mismatch] lambda-microvms.get-microvm-image-build-access-denied
    check_lambda_microvms_get_microvm_image_build_access_denied.py:54
    assert message == (f"User: {caller} is not authorized to perform: lambda:GetMicrovmImageBuild on resource: {bogus_image}")
    AssertionError: User: arn:aws:iam::000000000000:user/emulator is not authorized to perform this operation
    (ListMicrovmImageBuilds and GetMicrovmImageBuild on the managed image version returned form (a) as required and passed)

  Expected (AWS, recorded): 403 AccessDeniedException, Message = "User: <caller-arn> is not authorized to perform: lambda:<OperationName> on resource: <identifier>" for a well-formed identifier naming nothing granted. <caller-arn> is the signing principal (STS GetCallerIdentity Arn), <OperationName> the refused operation, <identifier> the request's own imageIdentifier.
  Actual (emulator): 403 AccessDeniedException, Message = "User: <caller-arn> is not authorized to perform this operation" for every denial of every operation.

  Source grounding:
    services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state.rs:798-806
      pub fn access_denied_message() -> String { format!("User: {} is not authorized to perform this operation", ::service_common::caller_principal::ambient_user_arn()) }  // no identifier, no action name
    services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/get_microvm_image.rs:200-207
      ImageAccess::Denied => AccessDeniedException { message: Some(crate::state::access_denied_message()) }
    services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/run_microvm.rs:239-246 (the same builder on the managed-ARN denial)
  Model grounding: smithy_query(query="shape", shape="AccessDeniedException") -> {errorKind: client, httpError: 403, members: {message: String}} — the model constrains neither the text nor its parts.

Cause 2 — idlePolicy never auto-suspends:

  [mismatch] lambda-microvms.idle-policy-auto-suspends-microvm
    check_lambda_microvms_idle_policy_auto_suspends_microvm.py:36 -> _wait_for_microvm_state (line 285)
    AssertionError: microvm mvm-0000000000000000 never reached {'SUSPENDED', 'TERMINATED'}, last state=RUNNING
    (RunMicrovm with idlePolicy {maxIdleDurationSeconds: 60, suspendedDurationSeconds: 300, autoResumeEnabled: true}; RUNNING was observed, then no traffic was sent to the endpoint and GetMicrovm was polled every 5 s for 300 s)

  Expected (AWS, recorded + modeled): GetMicrovm reports state SUSPENDED once maxIdleDurationSeconds has elapsed with no inbound request through the MicroVM proxy endpoint, with no SuspendMicrovm call — and not before that, so the elapsed time from the first RUNNING read to SUSPENDED is at least maxIdleDurationSeconds (minus one poll interval).
  Actual (emulator): state stays RUNNING for the whole 300 s window.

  Source grounding:
    services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/run_microvm.rs:371 (idle_policy: input.idle_policy — stored) and :386 (suspend_due_at: None — no idle timer armed from maxIdleDurationSeconds)
    services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state.rs:1035-1046 (start_microvm_transition — suspend_due_at is only ever set by MicrovmTransition::Suspend, i.e. an explicit SuspendMicrovm)
    services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state.rs:1607-1612 (materialize_due applies SUSPENDING -> SUSPENDED only; no idle-derived transition exists, and no last-inbound-request instant is recorded on the MicroVM record at state.rs:1318-1340)
  Model grounding: smithy_query(query="shape", shape="IdlePolicy") -> required maxIdleDurationSeconds (@range min 60), suspendedDurationSeconds (@range min 0), autoResumeEnabled; doc: "Idle time is measured by inbound traffic through the MicroVM proxy endpoint — if no requests arrive within the configured duration, the MicroVM is suspended." smithy_query(query="shape", shape="MicrovmState") -> PENDING|RUNNING|SUSPENDING|SUSPENDED|TERMINATING|TERMINATED.

## Steps to Reproduce
Cause 1:
1. mvm = client('lambda-microvms'); caller = client('sts').get_caller_identity()['Arn']; account = get_caller_identity()['Account'].
2. bogus = f"arn:aws:lambda:{region}:{account}:microvm-image/<a-name-nobody-created>"; mvm.get_microvm_image(imageIdentifier=bogus).
   Emulator: 403 AccessDeniedException, Message="User: <caller> is not authorized to perform this operation".
   AWS:      403 AccessDeniedException, Message="User: <caller> is not authorized to perform: lambda:GetMicrovmImage on resource: " + bogus.
3. mvm.get_microvm_image_build(imageIdentifier=bogus, imageVersion="1.0", buildId="build-does-not-exist").
   Emulator: the same caller-only sentence.
   AWS:      "User: <caller> is not authorized to perform: lambda:GetMicrovmImageBuild on resource: " + bogus.
4. For contrast, repeat step 2 with mvm.list_managed_microvm_images()['items'][0]['imageArn'] — the caller-only form (a) sentence is correct there and already matches.
5. Replay: lambda-microvms.get-microvm-image-access-denied, lambda-microvms.get-microvm-image-build-access-denied.

Cause 2:
1. Provision a MicroVM image whose latest version built SUCCESSFUL (S3 build context + IAM build role + CreateMicrovmImage), then mvm.run_microvm(imageIdentifier=<image arn>, idlePolicy={'maxIdleDurationSeconds': 60, 'suspendedDurationSeconds': 300, 'autoResumeEnabled': True}).
2. Poll mvm.get_microvm(microvmIdentifier=<microvmId>) until state == 'RUNNING'; note the time.
3. Send NO request to the published endpoint and keep polling GetMicrovm for 300 s.
   Emulator: state stays 'RUNNING' for the whole window; no SUSPENDING/SUSPENDED is ever reported.
   AWS:      state becomes 'SUSPENDED' (through 'SUSPENDING') once ~60 s of idleness has elapsed, and not before.
4. Replay: lambda-microvms.idle-policy-auto-suspends-microvm.
