Issue-Source: trace-sweep-divergence
Issue-Trace: signer-certificate-hashes-empty-list-rejected
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: signer-certificate-hashes-empty-list-rejected
Issue-Shape-Id: com.amazonaws.signer#WallabyService
Issue-Key: signer|GetRevocationStatus|4xx->4xx|$.message
Issue-Commit: 464a2835635950cb0afe23fa38546fc133c528e8

# GetRevocationStatus rejects an empty certificateHashes with the generic framework validation message instead of signer's ValidationException wording

> _Note: this issue was identified against the code base at commit `464a2835635950cb0afe23fa38546fc133c528e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRevocationStatus

## Description
One cause.

Cause 1 of 1: GetRevocationStatus answers an empty `certificateHashes` query with the wrong ValidationException message (fixture signer-certificate-hashes-empty-list-rejected, step task#2).

AWS: Smithy GetRevocationStatus is GET /revocations (endpoint hostPrefix `data-`), with signatureTimestamp, platformId, profileVersionArn, jobArn and certificateHashes all @required and @httpQuery-bound; certificateHashes targets CertificateHashes (list of String). Its modeled client errors are AccessDeniedException/403, TooManyRequestsException/429 and ValidationException/400 (ValidationException has members `message` and `code`). The model does not state how an EMPTY list is treated, so: recorded behavior only — a request whose certificateHashes query parameter carries no entries is rejected with HTTP 400 and the service-specific ValidationException message `certificateHashes must not be null or empty`. In that message the whole sentence is FIXED text apart from the leading member name, which is the offending input member from the request (`certificateHashes`); it carries no ids, counts, timestamps or other per-run values. AWS deliberately conflates the null and the empty case in one message, because an empty @httpQuery list is indistinguishable on the wire from an absent one.

Emulator: returns the correct HTTP 400, but serializes the generic Smithy required-member framework message instead: `1 validation error detected: Value null at 'certificateHashes' failed to satisfy constraint: Member must not be null`. The status matches and only the `$.message` member diverges (replay reports `$.message: value_mismatch`), so the request is being funnelled into the generic required-member/constraint reporter rather than the operation's own empty-list check.

Why: a client that distinguishes signer error conditions by the ValidationException message — the only discriminator available, since signer returns one ValidationException code for every input fault — never matches on the AWS text. Revocation checking is a security decision point, so callers commonly branch on "this input was empty/missing" versus other validation faults to decide whether to retry with a populated hash list or to fail the verification closed; against the emulator that branch falls through to the generic path. The generic text also asserts the value was `null`, which misreports a request that actually supplied an empty list.

Fix: the GetRevocationStatus handler's input validation in the signer service crate — the certificateHashes check must emit signer's own ValidationException message (`<member> must not be null or empty`, member taken from the request) for both the absent and the empty-list case, rather than deferring to the shared required-member/constraint validation reporter that produces the `1 validation error detected: ...` wording.

Traces: fixture signer-certificate-hashes-empty-list-rejected, step task#2 (cluster of 1; divergence_key service=signer|GetRevocationStatus|status=400->400|keys=$.message).

Related but NOT the same cause: open issue 20261023T031331Z-signer-accepts-calls-aws-rejects-and-reports-signing.md covers GetRevocationStatus ACCEPTING a malformed profileVersionArn/jobArn with HTTP 200; this divergence is a rejection that fires correctly but with the wrong message text, and is not covered there.

## Evidence
Replay of wire corpus simparity-wire-ga-prod-v1 (steps_total 2, passed 1, mismatched 1, score 50.0%; emulator_up true, bootstrap_signal false).

Cause 1 — GetRevocationStatus, fixture signer-certificate-hashes-empty-list-rejected step task#2 [mismatch]:
  request: GET /revocations with signatureTimestamp, platformId, profileVersionArn, jobArn supplied and certificateHashes bound as an EMPTY list.
  expected: HTTP 400, ValidationException, $.message = "certificateHashes must not be null or empty"
  actual:   HTTP 400, $.message = "1 validation error detected: Value null at 'certificateHashes' failed to satisfy constraint: Member must not be null"
  replay detail: $.message: value_mismatch
  status matches (400 -> 400); the only diverging key is $.message.
  Message decomposition: fixed text "must not be null or empty"; variable part = the offending request member name (certificateHashes). No per-run values (no request id, clock time, arn or generated id) are involved, so this is a wording defect, not an environment difference.
  Smithy: GetRevocationStatus GET /revocations, hostPrefix data-; certificateHashes @required @httpQuery target CertificateHashes(list<String>); errors AccessDeniedException/403, InternalServiceErrorException/500, TooManyRequestsException/429, ValidationException/400 (members message, code).

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from that checkout.
2. Replay the wire corpus simparity-wire-ga-prod-v1 (replay_wire_corpus).
3. Observe fixture signer-certificate-hashes-empty-list-rejected, step task#2, verdict mismatch on $.message.
4. Manually: call signer GetRevocationStatus with valid signatureTimestamp, platformId, profileVersionArn and jobArn, and certificateHashes=[] (empty list). The emulator answers HTTP 400 with message "1 validation error detected: Value null at 'certificateHashes' failed to satisfy constraint: Member must not be null"; AWS answers HTTP 400 ValidationException with message "certificateHashes must not be null or empty".
5. Repeat with certificateHashes omitted entirely — the same signer-specific message must be produced for that case too.
6. Confirm a request with one correctly-shaped certificateHashes entry is NOT rejected by this check.
7. Re-replay fixture signer-certificate-hashes-empty-list-rejected step task#2 after fixing.
