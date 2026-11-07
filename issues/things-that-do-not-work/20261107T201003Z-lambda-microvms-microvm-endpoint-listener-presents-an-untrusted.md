Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-microvms.suspend-resume-microvm-transitions
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms.suspend-resume-microvm-transitions|_get_state#9|AssertionError|raise AssertionError(f"the microvm endpoint {url!r} never answered: {exc!r}") from exc
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# lambda-microvms MicroVM endpoint listener presents an untrusted self-signed TLS certificate, so a verifying HTTPS client cannot complete a handshake with the published endpoint at all

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunMicrovm

## Description
One net-new root cause found by replaying the behavioral suite against the lambda-microvms emulator. CAUSE COUNT: 1.

--- Cause 1 of 1: the published MicroVM endpoint terminates TLS with a self-signed certificate that chains to no anchor a client trusts ---

AWS: RunMicrovmResponse$endpoint / GetMicrovmResponse$endpoint (Smithy: `endpoint`, target String, @length 1..2048, @required on both RunMicrovmResponse and GetMicrovmResponse of com.amazonaws.lambdamicrovms#LambdaMicrovms; RunMicrovm POST /2025-09-09/microvms, GetMicrovm GET /2025-09-09/microvms/{microvmIdentifier}) is documented as "The HTTPS endpoint URL for communicating with the MicroVM. Include a valid authentication token in the X-aws-proxy-auth header when sending requests." The model says nothing about the TLS material, so the requirement here is recorded behavior only: a plain HTTPS client with DEFAULT certificate verification (no custom CA bundle, no verification disabled — the suite uses urllib.request.urlopen with stock ssl defaults) completes the handshake against the published endpoint and then receives an HTTP response from it. Against real AWS the endpoint is an AWS-operated HTTPS address whose certificate chains to a publicly trusted anchor, so the handshake is never the thing that fails; whatever the service then answers (200 with the guest's body for an authenticated request, 403 for a missing/ill-scoped token, 404 for an address naming no MicroVM) is an HTTP-level answer the client can read. The requirement is "the address a client is told to use is usable by a verifying HTTPS client", not any particular certificate, hostname, port or issuer — the host, port and MicroVM id in the replay below are per-environment values and are not part of it.

Emulator: the handshake itself fails and NO HTTP exchange ever happens. Replay of lambda-microvms.suspend-resume-microvm-transitions, on the very first read of the endpoint published by RunMicrovm/GetMicrovm for a MicroVM in state RUNNING, with a token freshly minted by CreateMicrovmAuthToken(expirationInMinutes=15, allowedPorts=[{port:8080}]):
  AssertionError: the microvm endpoint 'https://<host>:<port>/microvms/<microvmId>/' never answered: URLError(SSLCertVerificationError(1, '[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: self-signed certificate (_ssl.c:1010)'))
The check retried for its whole 90 s budget and every attempt died in the same handshake. The endpoint is this emulator's own loopback HTTPS listener: the state owns it and the core owns its TLS (see Traces for the file and lines), and that listener serves a self-signed certificate. The failure is the emulator's listener, not the runner's TLS stack: the runner's same stock trust configuration reaches every control-plane operation of this service in the same run (19 of 41 checks pass, all of them control-plane calls through botocore), and only this data-plane listener is unverifiable.

Why: a client written against AWS takes `endpoint`, prefixes https:// and appends its guest path, and makes an ordinary verified request. Against this emulator that request never reaches the HTTP layer, so the client cannot observe ANY data-plane behavior — not the guest's response, not the 403 for a missing token, not the 404 for an unknown address. It sees a certificate-verification error, which is indistinguishable from an interception/misconfiguration failure rather than from a service answer, and no amount of HTTP-level retrying or error handling can get past it. Every suspend/resume, memory-identity, token-scoping and proxy-reachability flow is therefore unobservable, and a client whose only correct workaround is to disable certificate verification is being asked to do something it would never do against AWS.

Fix: the MicroVM endpoint listener's TLS material in the service state's endpoint module (the loopback HTTPS listener the state opens per store and publishes as `endpoint`), together with the core listener facility that owns that socket's TLS — a published endpoint must be reachable by a client using default verification, i.e. the listener's certificate must chain to an anchor the emulator makes available to its clients (or the published address must not demand verification the emulator cannot satisfy). The handler's HTTP-level answers are a separate concern from this handshake failure.

Related open issue, which does NOT carry this check's replay key: 20261103T220031Z-lambda-microvms-6-net-new-root-causes-deletemicrovmimage.md, whose Cause 4 is about the endpoint handler answering HTTP 501 for an authenticated request and mentions the self-signed certificate only as a secondary observation while treating the trust anchor as environmental. That cause recorded this check under a Timeout key (`suspend-resume-microvm-transitions|_get_state#9|Timeout`); the check now fails strictly earlier, in the TLS handshake, so the 501 is never reached and fixing the 501 alone leaves this check failing exactly as it does now. The handshake is filed here as its own cause so it is closed and re-replayed on its own.

Traces: lambda-microvms.suspend-resume-microvm-transitions (the only failure in this run not marked ALREADY FILED or OWNED). Code citations: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state/microvm_endpoint.rs:3-14 (the module header stating the published endpoint is "the core's own loopback HTTPS listener (`engine.listeners().http(..)`, which owns the socket, the bind interface and the TLS)"), :116-120 (`MicrovmProxy.base_url` = "https://<loopback host>:<port>" and the held `::emulator_server::ResourceListener`).

## Evidence
Suite: 19/41 passed (46.3%). Service lambda-microvms 19/39 (49%), 20 failed; lambda-core__lambda-microvms 0/2, 2 failed. Of those 22 failures, 21 are marked ALREADY FILED (unimplemented CreateMicrovmShellAuthToken, DeleteMicrovmImageVersion, UpdateMicrovmImageVersion stubs) or OWNED by an existing open issue. This issue is grounded in the one remaining failure.

[mismatch] lambda-microvms.suspend-resume-microvm-transitions
  check_lambda_microvms_suspend_resume_microvm_transitions.py, line 36, in run:
    first = _get_state(ctx, endpoint, token, retry_budget_s=90.0)
  line 120, in _get_state:
    raise AssertionError(f"the microvm endpoint {url!r} never answered: {exc!r}") from exc
  AssertionError: the microvm endpoint 'https://127.0.0.1:35591/microvms/mvm-0000000000000000/' never answered: URLError(SSLCertVerificationError(1, '[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: self-signed certificate (_ssl.c:1010)'))
  (host 127.0.0.1, port 35591 and id mvm-0000000000000000 are per-run values, not requirements.)

  expected: an HTTPS GET to the published `endpoint` using default certificate verification completes its TLS handshake and returns an HTTP response the client can read — HTTP 200 with the guest's JSON body for a request carrying X-aws-proxy-auth from CreateMicrovmAuthToken, HTTP 403 for one that does not, HTTP 404 for an address naming no MicroVM.
  actual: the TLS handshake fails with CERTIFICATE_VERIFY_FAILED / "self-signed certificate" on every attempt for the full 90 s retry budget; no HTTP request is ever sent and no HTTP status is ever returned.

  Corroboration that this is the emulator's listener and not the runner's TLS stack: in the same run, with the same stock trust configuration, every control-plane call of this service succeeds (CreateMicrovmImage, GetMicrovmImage, GetMicrovmImageVersion, RunMicrovm, GetMicrovm, CreateMicrovmAuthToken all complete in this very check before the data-plane read), and 19 checks pass. Only the MicroVM endpoint listener is unverifiable.

## Steps to Reproduce
Against the lambda-microvms emulator (the service is bootstrapped):

1. Provision an image: CreateMicrovmImage with a code artifact whose guest serves HTTP JSON on port 8080, then poll GetMicrovmImage until state=CREATED and GetMicrovmImageVersion until state=SUCCESSFUL.
2. RunMicrovm(imageIdentifier=<imageArn>, ingressNetworkConnectors=[arn:aws:lambda:<region>:aws:network-connector:aws-network-connector:ALL_INGRESS], egressNetworkConnectors=[...:INTERNET_EGRESS]); poll GetMicrovm until state=RUNNING and read `endpoint` from the response.
3. CreateMicrovmAuthToken(microvmIdentifier=<microvmId>, expirationInMinutes=15, allowedPorts=[{port:8080}]) and keep the returned authToken map as request headers.
4. Make an HTTPS GET to the published endpoint with those headers, using a client with DEFAULT certificate verification — e.g. python3 -c "import urllib.request; urllib.request.urlopen(urllib.request.Request('<endpoint>/', headers=<authToken>), timeout=20)" with no ssl context override, or `curl <endpoint>/` WITHOUT -k.

Expected: the handshake completes and the client reads an HTTP response (200 with the guest's body for this authenticated request). Observed: ssl.SSLCertVerificationError / URLError "[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: self-signed certificate" — no HTTP exchange occurs. Repeating with verification disabled (curl -k) is what currently reaches the listener at all, which is the divergence: AWS never requires that.

5. Confirm the same for the GetMicrovm-reported endpoint, and for a request with NO X-aws-proxy-auth header (which must reach the listener and be refused with 403, not fail in the handshake).
