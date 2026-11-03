Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-microvms.delete-image-with-running-microvm-rejected lambda-microvms.image-delete-rejected-while-a-microvm-is-running lambda-microvms.auth-token-expiration-over-sixty-minutes-rejected lambda-microvms.microvm-proxy-endpoint-serves-guest-http lambda-microvms.microvm-auth-token-grants-endpoint-access lambda-microvms.suspend-resume-microvm-transitions lambda-microvms.suspend-resume-preserves-guest-memory-and-process-identity lambda-microvms.idle-policy-auto-suspends-microvm lambda-microvms.update-image-version-status-on-failed-build-accepted
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms.delete-image-with-running-microvm-rejected|_error#6|AssertionError|raise AssertionError("the call unexpectedly SUCCEEDED where real AWS rejects it")
Issue-Key: lambda-microvms.image-delete-rejected-while-a-microvm-is-running|_client_error#7|AssertionError|raise AssertionError("the call unexpectedly SUCCEEDED where real AWS rejects it")
Issue-Key: lambda-microvms.auth-token-expiration-over-sixty-minutes-rejected|_error#7|AssertionError|raise AssertionError("the call unexpectedly SUCCEEDED where real AWS rejects it")
Issue-Key: lambda-microvms.microvm-proxy-endpoint-serves-guest-http|run#7|AssertionError|assert re.fullmatch(r"[A-Za-z0-9.-]+(:\d+)?", endpoint), (
Issue-Key: lambda-microvms.microvm-auth-token-grants-endpoint-access|_http_get#9|URLError|with urllib.request.urlopen(req, timeout=20) as resp:
Issue-Key: lambda-microvms.suspend-resume-microvm-transitions|_get_state#9|Timeout|time.sleep(5)
Issue-Key: lambda-microvms.suspend-resume-preserves-guest-memory-and-process-identity|_get_json#10|Timeout|time.sleep(5)
Issue-Key: lambda-microvms.idle-policy-auto-suspends-microvm|_wait_for_microvm_state#7|Timeout|time.sleep(interval_s)
Issue-Key: lambda-microvms.update-image-version-status-on-failed-build-accepted|run#4|AssertionError|assert ver["state"] == "FAILED", f"expected the broken Dockerfile build to fail, got {ver['state']}"
Issue-Commit: 2fe3407ad3a572d7c56bcafd231aa855d1c09ca1

# lambda-microvms: 6 net-new root causes — DeleteMicrovmImage has no running-MicroVM guard, CreateMicrovmAuthToken has no 60-minute expiry cap, the published endpoint is a URL not a host, the endpoint never serves the guest (501), idlePolicy never auto-suspends, and a broken build still settles SUCCESSFUL

> _Note: this issue was identified against the code base at commit `2fe3407ad3a572d7c56bcafd231aa855d1c09ca1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteMicrovmImage

## Description
Six independent root causes found by replaying the behavioral suite against the lambda-microvms emulator. They are unrelated defects in different components and each must be closed on its own; CAUSE COUNT: 6.

--- Cause 1 of 6: DeleteMicrovmImage accepts a delete while a MicroVM still runs off the image ---
AWS: DeleteMicrovmImage (Smithy: DELETE /2025-09-09/microvm-images/{imageIdentifier}; errors AccessDeniedException 403, ConflictException 409, ResourceNotFoundException 404, ValidationException 400) refuses the delete while any MicroVM created from that image is in a live state. Recorded behavior: HTTP 400, error code ValidationException, message "Cannot delete microvm image with running microvms." — that sentence is entirely fixed text, it carries no value from the request or state. The refusal has NO effect: the image stays state=CREATED and the version stays (state=SUCCESSFUL, status=ACTIVE). Once the dependent MicroVM is TERMINATED the identical call is accepted and returns state=DELETING (Smithy: DeleteMicrovmImageOutput.state, MicrovmImageState).
Emulator: the call SUCCEEDS while a MicroVM built from the image is RUNNING — the image is moved to DELETING and then removed. services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/delete_microvm_image.rs:195-223 resolves the image reference and sets delete_due_at with no scan of the MicroVM store for dependents, so there is no dependency guard at all.
Why: a client that relies on the refusal to detect "this image is still in use" is told the delete is in flight; the image record and its versions then disappear underneath a MicroVM that is still running, so later GetMicrovmImage/GetMicrovmImageVersion reads for a live workload 404 and cleanup/reconcile loops delete images they must keep.
Fix: DeleteMicrovmImage's handler (its state guard), which must refuse with ValidationException before scheduling the deletion whenever the store holds a MicroVM in a live state whose image_arn is this image.
Traces: lambda-microvms.delete-image-with-running-microvm-rejected, lambda-microvms.image-delete-rejected-while-a-microvm-is-running (both: "the call unexpectedly SUCCEEDED where real AWS rejects it").

--- Cause 2 of 6: CreateMicrovmAuthToken has no maximum on expirationInMinutes ---
AWS: Smithy models CreateMicrovmAuthToken.expirationInMinutes as PositiveInteger @range(min:1) with NO maximum, so the ceiling is recorded behavior only: AWS caps it at 60 minutes and refuses a larger value with HTTP 400, error code ValidationException, message "ExpirationMinutes 61 exceeded max allowed of 60." In that message "ExpirationMinutes " ... " exceeded max allowed of " ... "." is fixed text; the first value is the rejected expirationInMinutes from the request and the second is the service limit (60). The boundary value 60 is accepted and mints a token. Order matters and the emulator already gets it right: the microvmIdentifier lookup runs BEFORE the range check, so expirationInMinutes=61 against an absent microvm is still the 404 ResourceNotFoundException "MicroVM not found: <microvmIdentifier>".
Emulator: expirationInMinutes=61 is ACCEPTED and a token is minted. services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/create_microvm_auth_token.rs:145-225 validates only allowedPorts ranges and the MicroVM's state; the value is passed straight to state.record_auth_token, where src/state/microvm_endpoint.rs:359-380 silently clamps the lifetime to the MicroVM's own expiry instead of refusing the request.
Why: a client that asks for an over-long token gets a 200 and a token whose real lifetime is not what it asked for, instead of the 400 that tells it to re-request within the cap; code that treats the minted expiry as authoritative mis-schedules token refresh and sees connections start failing early.
Fix: CreateMicrovmAuthToken's input validation in its handler — refuse expirationInMinutes above the 60-minute service maximum with ValidationException, after the microvm lookup so the not-found verdict keeps precedence.
Traces: lambda-microvms.auth-token-expiration-over-sixty-minutes-rejected.

--- Cause 3 of 6: the published MicroVM endpoint is a full URL with scheme and path, where AWS publishes a host ---
AWS: RunMicrovmResponse.endpoint / GetMicrovmResponse.endpoint (Smithy: String, len 1..2048, required) is the address the MicroVM proxy is reached on, and the recorded value is a bare DNS host, optionally with a port — i.e. it matches [A-Za-z0-9.-]+(:\d+)? and carries NO scheme and NO path. A client builds its request URL by prefixing the scheme and appending the guest path to the host.
Emulator: endpoint is published as "https://<host>:<port>/microvms/<microvmId>" — scheme and a /microvms/<id> path baked into the value (built by src/state/microvm_endpoint.rs:175 and :193-195, stored by src/ops/run_microvm.rs:340). Replay: AssertionError: the proxy endpoint 'https://127.0.0.1:46693/microvms/mvm-0000000000000000' is not a well-formed host.
Why: every client that composes an address from this member (scheme + host + guest path, or that puts the value in a Host header / TLS SNI / connection pool key) builds a malformed address, so the data plane is unreachable for code written against AWS. The specific host, port and id above are per-environment values and are not the requirement — the SHAPE is: a host, optionally host:port, with no scheme and no path.
Fix: the MicroVM endpoint address construction in the state's endpoint module (and the RunMicrovm/GetMicrovm/ListMicrovms serializers that report it), which must publish an authority-only value and keep the per-MicroVM discriminator out of the published member.
Traces: lambda-microvms.microvm-proxy-endpoint-serves-guest-http.

--- Cause 4 of 6: the MicroVM endpoint never serves the guest — an authenticated request answers 501 ---
AWS: a GET to the MicroVM endpoint carrying the X-aws-proxy-auth header from CreateMicrovmAuthToken is proxied to the process inside the MicroVM and returns the GUEST's own HTTP response: HTTP 200 with the guest's body (the suite's guest serves a JSON document / a fixed probe payload on port 8080, which the proxy fronts). The same GET without the token is refused BY THE PROXY with HTTP 403 whose body names the missing authentication ("Request missing authentication"), and a token scoped to another port gets 403 on 8080.
Emulator: an authenticated request answers HTTP 501 with the body "This MicroVM endpoint is not implemented: the emulator runs no MicroVM guest, so the guest's own response cannot be served." — src/state/microvm_endpoint.rs:284 together with the NO_MICROVM_GUEST constant at :86-88. The guest's payload is never served, so no data-plane check can get past the first read; because the harness retries any >=500 for its retry budget, these checks burn the whole budget and are killed at the harness timeout ("Check timed out after 60.0s"). The unauthenticated refusal is the right 403 status but the wrong body: it reads "Present a valid authentication token for this MicroVM in the X-aws-proxy-auth header. Create one with CreateMicrovmAuthToken." (:80-82) instead of AWS's wording naming the missing authentication. Secondary observation on the same listener: it presents a self-signed certificate, so a verifying client fails the handshake (urllib.error.URLError: [SSL: CERTIFICATE_VERIFY_FAILED] self-signed certificate) before it can even see the 501 — the trust anchor is environmental, but the listener is the emulator's.
Why: the whole MicroVM data plane is unusable. A client that runs a MicroVM to serve traffic gets 501 for every request it proxies, and no suspend/resume, memory-identity or port-scoping behavior can be observed at all. NOTE: the resolved issue 20261027T071217Z-lambda-microvms-runmicrovm-publishes-a-microvm-endpoint-url.md claimed this path works again; a listener now exists, but it still answers no guest response, so MicroVM data-plane checks still hang to the harness timeout.
Fix: the MicroVM endpoint listener in the service state (its request handler) — it must serve a guest response for an authenticated request to a connectable MicroVM rather than 501, and align the unauthenticated 403 body with AWS's wording.
Traces: lambda-microvms.microvm-auth-token-grants-endpoint-access, lambda-microvms.suspend-resume-microvm-transitions, lambda-microvms.suspend-resume-preserves-guest-memory-and-process-identity, lambda-microvms.microvm-proxy-endpoint-serves-guest-http.

--- Cause 5 of 6: idlePolicy is recorded but never enforced — no MicroVM is ever auto-suspended ---
AWS: Smithy documents IdlePolicy as "Configuration that controls MicroVM auto-suspend and auto-resume behavior. Idle time is measured by inbound traffic through the MicroVM proxy endpoint - if no requests arrive within the configured duration, the MicroVM is suspended", with maxIdleDurationSeconds (@range min 60, required) "the maximum time in seconds that a MicroVM can remain idle before it is automatically suspended". So a MicroVM run with an idlePolicy and no inbound traffic reaches state=SUSPENDED (MicrovmState: PENDING|RUNNING|SUSPENDING|SUSPENDED|TERMINATING|TERMINATED) with no SuspendMicrovm call, no earlier than maxIdleDurationSeconds after the reported startedAt.
Emulator: the policy is stored on the record (src/ops/run_microvm.rs:354, crate::state::Microvm.idle_policy at src/state.rs:985) and nothing ever reads it: the record's clock-owed transitions are only provision_due_at / terminate_due_at / expiry_due_at / reclaim_due_at, with no idle or suspend deadline anywhere in state.rs, and materialize_due can therefore never produce SUSPENDED. The MicroVM stays RUNNING until its maximumDurationInSeconds expiry; the check polls GetMicrovm for SUSPENDED and is killed at the harness timeout ("Check timed out after 60.0s").
Why: a client that sizes its fleet on auto-suspend (idle MicroVMs stop being billed/occupying capacity and auto-resume on the next request) sees MicroVMs that never leave RUNNING, so capacity accounting and any wait-for-SUSPENDED step never converge.
Fix: the MicroVM state machinery (the record's clock-owed transitions in state.rs) — give a MicroVM carrying an idlePolicy an idle deadline refreshed by inbound endpoint traffic, and materialize RUNNING -> SUSPENDING -> SUSPENDED when it elapses. This is separate from the already-filed unimplemented SuspendMicrovm stub: that is the explicit API call, this is the platform's own timer.
Traces: lambda-microvms.idle-policy-auto-suspends-microvm.

--- Cause 6 of 6: a build that cannot succeed still settles SUCCESSFUL, so the FAILED-version activation path cannot be exercised ---
AWS: a CreateMicrovmImage whose code artifact cannot build (here a Dockerfile containing "RUN exit 1") settles the image version at state=FAILED (Smithy MicrovmImageVersionState: PENDING|IN_PROGRESS|SUCCESSFUL|FAILED|DELETING|DELETED|DELETE_FAILED) with a stateReason, and UpdateMicrovmImageVersion then ACCEPTS activating that version: the response and an independent GetMicrovmImageVersion read report status=ACTIVE while state stays FAILED — status and state are independent and activation must not rewrite the build outcome.
Emulator: GetMicrovmImageVersion reports state=SUCCESSFUL for the broken-Dockerfile build (replay: AssertionError: expected the broken Dockerfile build to fail, got SUCCESSFUL), because the build outcome is not derived from the artifact at all. The FAILED-plus-ACTIVE combination is therefore unreachable and this check cannot get past its precondition.
Why: a client cannot distinguish a built image from a broken one, and the documented independence of status (ACTIVE/INACTIVE) and state (build outcome) can never be observed; an automation that promotes versions on state==SUCCESSFUL will promote images that do not run.
Fix: the image-build settlement in the image/version state machinery, which must settle a version that cannot build at FAILED with a stateReason, and UpdateMicrovmImageVersion's handler must accept activation of a FAILED version without rewriting state. The open issues 20261025T210105Z-lambda-microvms-image-builds-never-fail-image-store.md and 20261030T024426Z-lambda-microvms-image-build-failure-is-never-modeled.md describe the same build-never-fails cause from other checks and neither carries this check's replay key — closing them should close this cause too; it is listed here so this check is re-replayed.
Traces: lambda-microvms.update-image-version-status-on-failed-build-accepted.

## Evidence
Suite: 15/41 passed. Service lambda-microvms 15/39 (38%), 24 failed. All failures below are from the replay's failing-check list and are none of the ones the report marked ALREADY FILED (CreateMicrovmShellAuthToken, DeleteMicrovmImageVersion, UpdateMicrovmImageVersion, SuspendMicrovm stubs) or OWNED by an existing issue.

Cause 1 — [mismatch] lambda-microvms.delete-image-with-running-microvm-rejected and [mismatch] lambda-microvms.image-delete-rejected-while-a-microvm-is-running:
  AssertionError: the call unexpectedly SUCCEEDED where real AWS rejects it
  expected: HTTP 400 ValidationException, Message "Cannot delete microvm image with running microvms.", image left at state=CREATED and version at (SUCCESSFUL, ACTIVE); after TERMINATE, the same delete returns state=DELETING.
  actual: HTTP 200, the delete is scheduled while the MicroVM is RUNNING.

Cause 2 — [mismatch] lambda-microvms.auth-token-expiration-over-sixty-minutes-rejected:
  AssertionError: the call unexpectedly SUCCEEDED where real AWS rejects it
  expected: CreateMicrovmAuthToken(expirationInMinutes=61) -> HTTP 400 ValidationException, Message "ExpirationMinutes 61 exceeded max allowed of 60."; expirationInMinutes=60 accepted; the same 61 against an absent microvm -> 404 ResourceNotFoundException "MicroVM not found: <id>".
  actual: HTTP 200 with a minted authToken for 61 minutes.

Cause 3 — [mismatch] lambda-microvms.microvm-proxy-endpoint-serves-guest-http:
  AssertionError: the proxy endpoint 'https://127.0.0.1:46693/microvms/mvm-0000000000000000' is not a well-formed host
  expected: RunMicrovmResponse.endpoint matching [A-Za-z0-9.-]+(:\d+)? (host, optionally host:port; no scheme, no path).
  actual: a full URL with scheme and a /microvms/<microvmId> path.

Cause 4 — [error] lambda-microvms.microvm-auth-token-grants-endpoint-access, [mismatch] lambda-microvms.suspend-resume-microvm-transitions, [mismatch] lambda-microvms.suspend-resume-preserves-guest-memory-and-process-identity, [mismatch] lambda-microvms.microvm-proxy-endpoint-serves-guest-http:
  urllib.error.URLError: <urlopen error [SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: self-signed certificate> / "Check timed out after 60.0s" (the harness retries any >=500 answer, so the 501 burns the retry budget).
  expected: authenticated GET -> HTTP 200 with the guest's own body; unauthenticated GET -> HTTP 403 whose body names the missing authentication ("Request missing authentication"); a token scoped to another port -> 403.
  actual: src/state/microvm_endpoint.rs answers HTTP 501 "This MicroVM endpoint is not implemented: the emulator runs no MicroVM guest, so the guest's own response cannot be served." to every authenticated request, and 403 "Present a valid authentication token for this MicroVM in the X-aws-proxy-auth header. Create one with CreateMicrovmAuthToken." to an unauthenticated one.

Cause 5 — [mismatch] lambda-microvms.idle-policy-auto-suspends-microvm:
  Error: Check timed out after 60.0s while polling GetMicrovm for SUSPENDED after RunMicrovm(idlePolicy={maxIdleDurationSeconds:60, suspendedDurationSeconds:300, autoResumeEnabled:true}).
  expected: state=SUSPENDED with no SuspendMicrovm call, no sooner than maxIdleDurationSeconds after the reported startedAt.
  actual: state stays RUNNING; src/state.rs stores idle_policy (line 985) and has no idle/suspend deadline at all (only provision/terminate/expiry/reclaim), so SUSPENDED is unreachable.

Cause 6 — [mismatch] lambda-microvms.update-image-version-status-on-failed-build-accepted:
  AssertionError: expected the broken Dockerfile build to fail, got SUCCESSFUL
  expected: a "FROM node:24-alpine / RUN exit 1" artifact settles the version at state=FAILED; UpdateMicrovmImageVersion(status=ACTIVE) then returns status=ACTIVE with state still FAILED, and an independent read agrees.
  actual: the version settles state=SUCCESSFUL, so the FAILED+ACTIVE combination is unreachable.

## Steps to Reproduce
All against the lambda-microvms emulator (service is bootstrapped; its bootstrap issue is resolved).

Cause 1: provision an image and wait for state=CREATED with version (SUCCESSFUL, ACTIVE); RunMicrovm(imageIdentifier=<imageArn>, maximumDurationInSeconds=600) and wait for RUNNING; call DeleteMicrovmImage(imageIdentifier=<imageArn>). Expect 400 ValidationException "Cannot delete microvm image with running microvms." and the image untouched; the emulator returns 200/DELETING. Then TerminateMicrovm, wait TERMINATED, and the same DeleteMicrovmImage must succeed with state=DELETING.

Cause 2: with a RUNNING MicroVM, call CreateMicrovmAuthToken(microvmIdentifier=<id>, expirationInMinutes=61, allowedPorts=[{allPorts:{}}]). Expect 400 ValidationException "ExpirationMinutes 61 exceeded max allowed of 60."; expirationInMinutes=60 must succeed. Repeat with microvmIdentifier=microvm-00000000-0000-0000-0000-000000000000 and expirationInMinutes=61: must be 404 ResourceNotFoundException "MicroVM not found: microvm-00000000-0000-0000-0000-000000000000" (lookup before range check).

Cause 3: RunMicrovm and read endpoint from the response (and from GetMicrovm). It must match [A-Za-z0-9.-]+(:\d+)? — no "https://" prefix, no /microvms/<id> path.

Cause 4: RunMicrovm with ingressNetworkConnectors=[...:aws-network-connector:HTTP_INGRESS] on an image whose guest serves HTTP on 8080, wait RUNNING, CreateMicrovmAuthToken(allowedPorts=[{allPorts:{}}] or [{port:8080}]), then GET <endpoint>/probe.txt with header X-aws-proxy-auth: <token>. Expect 200 and the guest's payload; the emulator answers 501. Repeat with no header: expect 403 whose body names the missing authentication.

Cause 5: RunMicrovm with idlePolicy={maxIdleDurationSeconds:60, suspendedDurationSeconds:300, autoResumeEnabled:true}, wait for RUNNING, then send NO traffic to the endpoint and poll GetMicrovm. Expect state=SUSPENDED at or after startedAt+60s with no SuspendMicrovm call; the emulator stays RUNNING indefinitely.

Cause 6: CreateMicrovmImage with a code artifact whose Dockerfile is "FROM node:24-alpine\nRUN exit 1\n", then poll GetMicrovmImageVersion until state is SUCCESSFUL or FAILED. Expect FAILED (with stateReason); the emulator reports SUCCESSFUL. Then UpdateMicrovmImageVersion(status=ACTIVE) must return status=ACTIVE with state=FAILED, and an independent GetMicrovmImageVersion must agree.
