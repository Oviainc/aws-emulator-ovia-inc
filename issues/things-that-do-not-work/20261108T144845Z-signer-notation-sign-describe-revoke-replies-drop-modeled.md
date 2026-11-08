Issue-Source: trace-sweep-divergence
Issue-Trace: signer-notation-signpayload-revoke
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: signer-notation-signpayload-revoke
Issue-Shape-Id: com.amazonaws.signer#WallabyService
Issue-Key: signer|SignPayload|2xx->2xx|$.metadata.com.amazonaws.signer.signingJob
Issue-Key: signer|SignPayload|2xx->2xx|$.metadata.com.amazonaws.signer.signingProfileVersion
Issue-Key: signer|DescribeSigningJob|2xx->2xx|$.platform
Issue-Key: signer|DescribeSigningJob|2xx->2xx|$.statusReason
Issue-Key: signer|RevokeSignature|2xx->2xx|$headers.content-length
Issue-Commit: 8c4f0f4a16330ddd0a226e1ee320cba8ce6308a8

# Signer Notation sign/describe/revoke replies drop modeled output members and the 204 Content-Length

> _Note: this issue was identified against the code base at commit `8c4f0f4a16330ddd0a226e1ee320cba8ce6308a8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SignPayload

## Description
Three net-new wire divergences, all on the signer Notation payload-signing path (fixture signer-notation-signpayload-revoke). Each is the same class of fault — the response the emulator builds is a strict subset of the one AWS returns: two success bodies omit members the caller needs, and one bodyless 204 omits its entity header. All three replay against service `signer`, which is bootstrapped. None is covered by an open issue: the neighbouring open issue 20261031T194210Z-signer-response-envelopes-diverge-empty-204-replies-omit.md covers the missing Content-Length on TagResource, UntagResource and RevokeSigningProfile only and names no RevokeSignature key, 20261024T054020Z-signer-signpayload-stub.md is about SignPayload being an unimplemented scaffold (it now answers 200, so that is a different state), and 20261108T082320Z-signer-3-net-new-causes-signpayload-is-never.md is about SignPayload authorization and ListSigningJobs/RevokeSignature validation message text, not about response members.

Cause 1 of 3: SignPayload's success body omits the Notation metadata entries.
AWS: Smithy signer#SignPayload is POST /signing-jobs/with-payload, 200, output SignPayloadResponse{jobId, jobOwner, metadata: Metadata (map<String,String>), signature: Blob}; the metadata member is documented as 'Information including the signing profile ARN and the signing job ID'. For a Notation signing request AWS returns that map populated with the signer-namespaced keys the Notation signature envelope is built from — the recording asserts the presence of the keys `com.amazonaws.signer.signingProfileVersion` and `com.amazonaws.signer.signingJob` (their values are per-run identifiers and are not value-asserted; the requirement is that each key exists and carries the corresponding profile-version / signing-job identifier from the service's own state for this call).
Emulator: answers status 200 (matching) and a body whose other asserted members match, but the metadata map has neither key — both `$.metadata.com.amazonaws.signer.signingProfileVersion` and `$.metadata.com.amazonaws.signer.signingJob` are reported missing.
Why: a Notation client assembles its signature envelope from these metadata entries, and uses them to tie a returned signature back to the profile version and job that produced it. With the keys absent the client either fails envelope construction outright or emits a signature with no provenance annotations, so a payload AWS would have signed verifiably appears unverifiable/untraceable.
Fix: the SignPayload handler's output construction (the step that populates SignPayloadResponse.metadata from the resolved profile version and the newly created job) and its serializer in the signer crate.
Traces: signer-notation-signpayload-revoke#task#3.

Cause 2 of 3: DescribeSigningJob omits the platform identifier and the status reason.
AWS: Smithy signer#DescribeSigningJob is GET /signing-jobs/{jobId}, 200, output DescribeSigningJobResponse; `statusReason` is modeled (target StatusReason, string, doc 'String value that contains the status reason') and AWS populates it for a terminal job — for a job that signed successfully the fixed text is 'Completed signing.' (no variable part). The body member `platform` is recorded behavior only: it is not in this crate's on-disk Smithy model (which carries platformId/platformDisplayName), but the recorded AWS response carries `platform` holding the signing platform id the job ran on — in the recording the Notation platform id the job was created with ('Notation-OCI-SHA384-ECDSA'); the requirement is that the field echoes the job's own platform id from state, not that one literal.
Emulator: answers status 200 (matching) with every other asserted member correct, but `$.platform` is missing (got None) and `$.statusReason` is missing (got None). This reproduces on both describes in the fixture — the one right after signing and the one after the signature is revoked — so the omission is unconditional, not a state-dependent gap.
Why: callers poll DescribeSigningJob to decide whether a job is usable: `statusReason` is the only human/diagnostic explanation of a terminal status, and `platform` is how a client confirms the signature was produced under the platform it asked for (Notation vs a FreeRTOS/Lambda platform). A client that reads either field gets a null where AWS gives a value, so verification and error reporting silently degrade and platform-conditional code paths take the wrong branch.
Fix: the DescribeSigningJob handler's response construction/serializer in the signer crate — the job record must project its platform id and its terminal status reason into the reply.
Traces: signer-notation-signpayload-revoke#task#4 (and the second describe in the same fixture, task#6).

Cause 3 of 3: RevokeSignature's empty 204 reply omits Content-Length.
AWS: Smithy signer#RevokeSignature is PUT /signing-jobs/{jobId}/revoke with output shape Unit (no members); AWS answers it bodyless and still sends the entity header Content-Length: 0. Recorded behavior: status 204 with header content-length: '0'.
Emulator: answers status 204 (matching) but emits no content-length header at all ($headers.content-length missing, got None).
Why: revocation's only success signal is the empty reply, so a client or intermediary that frames the message by Content-Length rather than by status has no terminator and must fall back on connection close or treat the reply as indeterminate-length — a revocation AWS reports as cleanly complete can be surfaced as a transport error or left unconfirmed.
Fix: the same empty-output/Unit response serializer in the signer crate that chooses status 204; RevokeSignature is not routed through whatever sets the header today (the sibling operations TagResource/UntagResource/RevokeSigningProfile show the identical gap under the open issue 20261031T194210Z-signer-response-envelopes-diverge-empty-204-replies-omit.md, which does not name RevokeSignature).
Traces: signer-notation-signpayload-revoke#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true; /__admin/health answered 200, data listener on :38597). 49 steps total, 40 passed, 9 mismatched, 8 divergence clusters; the three clusters below are the ones not already owned by an open issue and not blocked by a stub.

Cause 1 — fixture signer-notation-signpayload-revoke, step task#3, service signer, operation SignPayload, verdict mismatch:
  expected status 200, actual status 200
  detail: $.metadata.com.amazonaws.signer.signingProfileVersion: missing (expected None, got None)
           $.metadata.com.amazonaws.signer.signingJob: missing (expected None, got None)
  (i.e. the assertion is on key presence — the values are per-run and not value-asserted — and the keys are absent from the emulator's metadata map)

Cause 2 — fixture signer-notation-signpayload-revoke, step task#4, service signer, operation DescribeSigningJob, verdict mismatch (cluster also covers task#6 in the same fixture, affected_count=2):
  expected status 200, actual status 200
  detail: $.platform: missing (expected 'Notation-OCI-SHA384-ECDSA', got None)
           $.statusReason: missing (expected 'Completed signing.', got None)
  Fixed text vs values: 'Completed signing.' is AWS's fixed statusReason text for a successfully completed signing job; 'Notation-OCI-SHA384-ECDSA' is a value taken from the request/state (the platform id the job was created under), not a constant.

Cause 3 — fixture signer-notation-signpayload-revoke, step task#5, service signer, operation RevokeSignature, verdict mismatch:
  expected status 204, actual status 204
  detail: $headers.content-length: missing (expected '0', got None)

Smithy consulted (services/service-com-amazonaws-signer-wallabyservice/smithy-model/com-amazonaws-signer-wallabyservice.json):
  signer#SignPayload — POST /signing-jobs/with-payload, code 200; output SignPayloadResponse{jobId, jobOwner, metadata -> Metadata map<String,String> doc 'Information including the signing profile ARN and the signing job ID', signature -> Blob}; errors AccessDenied(403)/Validation(400)/ResourceNotFound(404)/TooManyRequests(429)/InternalServiceError(500).
  signer#DescribeSigningJob — GET /signing-jobs/{jobId}, code 200; output DescribeSigningJobResponse includes statusReason -> StatusReason (string, doc 'String value that contains the status reason'), status -> SigningStatus enum, platformId, platformDisplayName, revocationRecord. No `platform` member in the on-disk model: that body field is recorded behavior only.
  signer#RevokeSignature — PUT /signing-jobs/{jobId}/revoke, output shape Unit (no members).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus) and look at fixture signer-notation-signpayload-revoke.
3. Cause 1 — step task#3: after creating the Notation signing profile the fixture creates, POST /signing-jobs/with-payload with that profileName, the recorded payload and payloadFormat. The 200 body returns jobId/jobOwner/signature, but its metadata map contains neither com.amazonaws.signer.signingProfileVersion nor com.amazonaws.signer.signingJob; AWS returns both keys.
4. Cause 2 — step task#4: GET /signing-jobs/{jobId} for the job just created. The 200 body omits `platform` (AWS echoes the job's platform id, 'Notation-OCI-SHA384-ECDSA' in the recording) and omits `statusReason` (AWS returns the fixed text 'Completed signing.' for a successfully completed job). Repeat the same GET after the revoke (step task#6) — both fields are still absent, so the omission is unconditional.
5. Cause 3 — step task#5: PUT /signing-jobs/{jobId}/revoke with a jobOwner and a reason. The reply is 204 as AWS returns, but carries no content-length header; AWS sends content-length: 0.
