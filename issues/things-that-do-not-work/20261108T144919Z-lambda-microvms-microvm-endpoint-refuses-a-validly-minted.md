Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-microvms.microvm-auth-token-grants-endpoint-access lambda-microvms.suspend-resume-microvm-transitions lambda-microvms.suspend-resume-preserves-guest-memory-and-process-identity
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms.microvm-auth-token-grants-endpoint-access|_http_get#9|HTTPError|with urllib.request.urlopen(req, timeout=20) as resp:
Issue-Key: lambda-microvms.suspend-resume-microvm-transitions|_get_state#9|AssertionError|raise AssertionError(
Issue-Key: lambda-microvms.suspend-resume-preserves-guest-memory-and-process-identity|_get_json#10|AssertionError|assert status == 200, f"GET {url!r} answered HTTP {status} with {body[:200]!r}"
Issue-Commit: 5bdc422e571ff089197010dd8b23a6f4d1e72235

# lambda-microvms MicroVM endpoint refuses a validly minted port-scoped auth token with HTTP 403, and serves 501 instead of the guest's response

> _Note: this issue was identified against the code base at commit `5bdc422e571ff089197010dd8b23a6f4d1e72235`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMicrovmAuthToken

## Description
Two net-new root causes found by replaying the behavioral suite against the lambda-microvms emulator. CAUSE COUNT: 2.

--- Cause 1 of 2: the published MicroVM endpoint rejects a token minted with allowedPorts=[{port: <guest port>}] with HTTP 403, because the authorization decision is taken against the image version's Hooks$port instead of the port the request addresses ---

AWS: CreateMicrovmAuthToken (Smithy: com.amazonaws.lambdamicrovms#CreateMicrovmAuthToken, POST /2025-09-09/microvms/{microvmIdentifier}/auth-token, success 200) takes `allowedPorts` (ListOfPortSpecification, @required, @length min 1) — documented as "The ports on the MicroVM endpoint the token grants access to" — whose member PortSpecification is a union of `port` (PortNumber @range 1..65535), `range` (PortRange startPort/endPort) and `allPorts` (Unit); it returns `authToken` (TokenParts, @required, @length 1..10) whose documented key is "X-aws-proxy-auth": "Use the value at key \"X-aws-proxy-auth\" as the header value when connecting to the MicroVM endpoint." The model says nothing further about the proxied exchange, so the rest is recorded behavior only: a GET to the RunMicrovm-published `endpoint` of a RUNNING MicroVM carrying that header, where the token's allowedPorts admit the port the request addresses (the port the proxy fronts by default, selectable with the X-aws-proxy-port request header), is proxied to the guest and answers HTTP 200 with the guest's own body; only a request addressing a port OUTSIDE the token's allowedPorts is refused with HTTP 403. There is no error code on this path: a granting token yields a 2xx carrying whatever the guest sent.

Emulator: a token minted for the guest's actual port is refused. services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state/microvm_endpoint.rs:261-283 computes `guest_port = store.microvm_guest_port(&microvm_id)` — which is the image VERSION's Hooks$port (same file, lines 406-415) — and then requires `grants_port(&token.allowed_ports, guest_port)` (lines 310-319), whose documented policy is that when the image declares no hooks port, `guest_port` is None and every `port`/`range`-scoped token "grants nothing that can be established and is refused; only a token for EVERY port covers such a request". The request's own X-aws-proxy-port header is never read at all, so the scope is matched against image metadata rather than against the port the caller addressed. The result is HTTP 403 with the emulator's own fixed text `Present a valid authentication token for this MicroVM in the X-aws-proxy-auth header. Create one with CreateMicrovmAuthToken.\n` — a body AWS has no counterpart for, and one that misdescribes the refusal: the token presented WAS the one CreateMicrovmAuthToken had just minted for this MicroVM and had not expired. Nothing in that sentence comes from the request. Replay: `microvm-auth-token-grants-endpoint-access` mints expirationInMinutes=10, allowedPorts=[{port: 8080}] against a RUNNING MicroVM whose guest serves HTTP on 8080 and the first authenticated GET of `/` dies on `urllib.error.HTTPError: HTTP Error 403: Forbidden`; `suspend-resume-microvm-transitions` mints expirationInMinutes=15, allowedPorts=[{port: 8080}] and gets `AssertionError: the microvm endpoint '.../microvms/<microvm id>/' answered HTTP 403: 'Present a valid authentication token for this MicroVM in the X-aws-proxy-auth header. Create one with CreateMicrovmAuthToken.\n'`. Tokens minted with allowedPorts=[{allPorts:{}}] are accepted by the same code path (they reach the 501 of Cause 2), which isolates the discriminator to the explicit `port` arm of the union.

Why: a client that scopes a token the way the API invites — least privilege, one entry naming the port its guest listens on — cannot reach its own MicroVM at all; only the broadest possible scope (allPorts) works. The 403 is indistinguishable from "your token is wrong or expired", so the client re-mints in a loop, and the one grant AWS honours is reported as no grant. It also makes the emulator's allowedPorts enforcement untestable: a client cannot show that a token for port 9999 is refused on port 8080 while a token for 8080 is admitted, because both arms answer 403.

Fix: the MicroVM endpoint listener's authorization decision in the lambda-microvms service state — the token check in `serve_request` and the `grants_port` predicate it calls — must decide against the port the REQUEST addresses (the proxy's default fronted port, overridden by X-aws-proxy-port), not against the image version's Hooks$port, and must not treat an image that declares no hooks port as a reason to reject every port-scoped token. Nothing in CreateMicrovmAuthToken's handler needs to change: it already records allowedPorts verbatim.

Traces: lambda-microvms.microvm-auth-token-grants-endpoint-access, lambda-microvms.suspend-resume-microvm-transitions.

--- Cause 2 of 2: an authenticated GET to the published endpoint of a RUNNING MicroVM answers HTTP 501 "the emulator runs no MicroVM guest" instead of the guest's own response ---

AWS: RunMicrovmResponse$endpoint (Smithy: RunMicrovm, member `endpoint` String @length 1..2048 @required) is "The HTTPS endpoint URL for communicating with the MicroVM. Include a valid authentication token in the X-aws-proxy-auth header when sending requests." Recorded behavior only: such a request is proxied into the MicroVM and the caller receives the GUEST's own HTTP response — HTTP 200 with the guest's body (here a JSON document the guest serves on port 8080 under /id, /set and /get) — and, while the MicroVM is SUSPENDED, no 200 at all. No error code is involved on the success path.

Emulator: every authenticated request to a published endpoint short-circuits to HTTP 501 with the fixed body `This MicroVM endpoint is not implemented: the emulator runs no MicroVM guest, so the guest's own response cannot be served.\n` (microvm_endpoint.rs:284, the NO_MICROVM_GUEST constant). That sentence is the emulator's own text in its entirety; nothing in it is derived from the request, and AWS never returns 501 here. Replay: `AssertionError: GET 'https://<host>:<port>/microvms/<microvm id>/id' answered HTTP 501 with "This MicroVM endpoint is not implemented: the emulator runs no MicroVM guest, so the guest's own response cannot be served.\n"` — reached after CreateMicrovmImage settled, RunMicrovm settled RUNNING with the expected imageVersion, and CreateMicrovmAuthToken minted an allPorts token, so the 501 is the emulator's answer and not an upstream failure.

Why: a client runs a MicroVM in order to serve traffic. Against this emulator it can never read its guest, so it cannot verify that suspend/resume preserves guest memory and process identity (boot_id, pid, starttime unchanged, in-memory and on-disk values retained) or that a SUSPENDED MicroVM serves nothing — the data plane is wholly unobservable and every arm of such a test gets the identical 501.

Fix: the MicroVM endpoint listener's request handler in the lambda-microvms service state (`serve_request`, which currently returns the NO_MICROVM_GUEST 501 for every authenticated request) must serve the guest's response for the addressed path, and must reflect the MicroVM's lifecycle state (a SUSPENDED MicroVM serves no 200). Related open issues, neither of which carries this check's replay key: 20261103T220031Z-lambda-microvms-6-net-new-root-causes-deletemicrovmimage.md, whose Cause 4 describes the same 501 from four other lambda-microvms checks, and 20261108T130022Z-lambda-core-lambda-microvms-lambda-core-vpc-egress-connector-data-plane-is.md, which describes it from the two lambda-core connector-egress checks; 20261107T201003Z-lambda-microvms-microvm-endpoint-listener-presents-an-untrusted.md concerns the listener's TLS material, not its HTTP answer.

Traces: lambda-microvms.suspend-resume-preserves-guest-memory-and-process-identity.

## Evidence
Suite: 20/41 passed (48.8%); 17 mismatched, 4 errored. Per-service: lambda-microvms 20/39 (51%), lambda-core__lambda-microvms 0/2. Failures marked ALREADY FILED (UpdateMicrovmImageVersion, DeleteMicrovmImageVersion, CreateMicrovmShellAuthToken stubs) or OWNED by an open issue were skipped; this issue is grounded in the three remaining lambda-microvms failures.

Cause 1 — port-scoped token refused 403:

[error] lambda-microvms.microvm-auth-token-grants-endpoint-access
  good_token = create_microvm_auth_token(microvmIdentifier=<id>, expirationInMinutes=10, allowedPorts=[{"port": 8080}])["authToken"]
  _http_get(ctx, endpoint, good_token) -> request.py line 639, http_error_default:
  urllib.error.HTTPError: HTTP Error 403: Forbidden
  expected: HTTP 200 whose body is the guest app's JSON, {"status": "ok", "marker": "<this run's marker>"}; then a token scoped to [{"port": 9999}] sent with X-aws-proxy-port: 8080 refused with 403; then the correctly-scoped token working again.
  actual: the FIRST, correctly-scoped GET is already 403, so allowedPorts enforcement is never exercised.

[mismatch] lambda-microvms.suspend-resume-microvm-transitions
  token = create_microvm_auth_token(microvmIdentifier=<id>, expirationInMinutes=15, allowedPorts=[{"port": 8080}])["authToken"]
  check_lambda_microvms_suspend_resume_microvm_transitions.py line 36 -> line 114:
  AssertionError: the microvm endpoint 'https://127.0.0.1:40029/microvms/mvm-0000000000000000/' answered HTTP 403: 'Present a valid authentication token for this MicroVM in the X-aws-proxy-auth header. Create one with CreateMicrovmAuthToken.\n'
  (host, port and mvm-0000000000000000 are per-run values and are not requirements; the whole quoted sentence is the emulator's fixed text.)

  Contrast, same code path, same suite run: every check that mints allowedPorts=[{"allPorts": {}}] passes authorization and reaches the 501 of Cause 2 — so the explicit `port` arm of PortSpecification is the discriminator.
  Source: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state/microvm_endpoint.rs:261-268 (guest_port from microvm_guest_port), :282 (403 AUTH_TOKEN_REQUIRED), :310-319 (grants_port), :406-415 (microvm_guest_port = image version Hooks$port).

Cause 2 — 501 instead of the guest's response:

[mismatch] lambda-microvms.suspend-resume-preserves-guest-memory-and-process-identity
  token minted with expirationInMinutes=30, allowedPorts=[{"allPorts": {}}]; headers = {"X-aws-proxy-auth": <token>}
  check_..._suspend_resume_preserves_guest_memory_and_process_identity.py line 129 -> line 232:
  AssertionError: GET 'https://127.0.0.1:37789/microvms/mvm-0000000000000000/id' answered HTTP 501 with "This MicroVM endpoint is not implemented: the emulator runs no MicroVM guest, so the guest's own response cannot be served.\n"
  expected: HTTP 200 with the guest's JSON from /id ({"boot_id": ..., "pid": ..., "starttime": ...}), unchanged across SuspendMicrovm + ResumeMicrovm, and /get returning the values written through /set before the suspend.
  actual: HTTP 501 with that fixed emulator sentence for every guest path.
  Source: microvm_endpoint.rs:284 (NO_MICROVM_GUEST 501).

## Steps to Reproduce
Against the emulator (lambda-microvms stood up):

Cause 1:
1. CreateMicrovmImage from an artifact whose guest serves HTTP on port 8080; poll until image state=CREATED and version state=SUCCESSFUL.
2. RunMicrovm(imageIdentifier=<imageArn>, ingressNetworkConnectors=[...:aws-network-connector:HTTP_INGRESS]); poll GetMicrovm until state=RUNNING and read `endpoint`.
3. CreateMicrovmAuthToken(microvmIdentifier=<id>, expirationInMinutes=10, allowedPorts=[{port: 8080}]) and GET <endpoint>/ with the returned map as request headers (X-aws-proxy-auth: <value>).
   Expected: HTTP 200 carrying the guest's own body.
   Observed: HTTP 403 'Present a valid authentication token for this MicroVM in the X-aws-proxy-auth header. Create one with CreateMicrovmAuthToken.'
4. Repeat step 3 with allowedPorts=[{allPorts:{}}]: authorization now passes (the request gets as far as the 501 of Cause 2), showing the refusal is the port-scope decision, not the token.
5. Also repeat step 3 with allowedPorts=[{range:{startPort:8000,endPort:9000}}]: same 403.
6. Then, with a granting token, send X-aws-proxy-port: 9999 while the token allows only 8080: AWS refuses that request with 403; the emulator ignores the header entirely.

Cause 2:
7. With the allPorts token from step 4, GET <endpoint>/id (or any guest path).
   Expected: HTTP 200 with the guest's response for that path; after SuspendMicrovm the endpoint serves no 200, and after ResumeMicrovm /id reports the same boot_id/pid/starttime and /get still returns what /set stored before the suspend.
   Observed: HTTP 501 'This MicroVM endpoint is not implemented: the emulator runs no MicroVM guest, so the guest's own response cannot be served.' for every path and every lifecycle state.
