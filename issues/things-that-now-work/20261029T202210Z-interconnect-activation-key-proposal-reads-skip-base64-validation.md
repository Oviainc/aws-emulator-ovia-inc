Issue-Source: trace-sweep-divergence
Issue-Trace: interconnect-connection-proposal-describe-and-accept-invalid-key
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: interconnect-connection-proposal-describe-and-accept-invalid-key
Issue-Shape-Id: com.amazonaws.interconnect#Interconnect
Issue-Key: interconnect|DescribeConnectionProposal|4xx->4xx|$.__type
Issue-Key: interconnect|DescribeConnectionProposal|4xx->4xx|$.message
Issue-Key: interconnect|DescribeConnectionProposal|4xx->4xx|status
Issue-Key: interconnect|AcceptConnectionProposal|4xx->4xx|$.__type
Issue-Key: interconnect|AcceptConnectionProposal|4xx->4xx|$.message
Issue-Key: interconnect|AcceptConnectionProposal|4xx->4xx|status
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# Interconnect activation-key proposal reads skip base64 validation, answering 404 not-found for an invalid key

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConnectionProposal, AcceptConnectionProposal

## Description
Replay of simparity-wire-aws-v1 against this checkout scored 40.9% (9 passed / 13 mismatched of 22 steps, 12 divergence clusters, nothing blocked or errored, emulator up, bootstrap_signal false; service interconnect is bootstrapped). Seven of the twelve clusters are marked OWNED by 20261023T095130Z-interconnect-wire-fidelity-seeded-environment-attach-point-catalog.md and are skipped here. This issue carries TWO net-new root causes, numbered Cause 1 of 2 .. Cause 2 of 2; BOTH must be closed before it is submitted, and the representative step in Issue-Trace/Issue-Step plus every fixture#step in Issue-Trace-Ids must be re-replayed.

Shared fault: an `activationKey` whose text is not valid base64 is a fault in the REQUEST, and neither operation that takes that member judges it. Both handlers go straight to the store's activationKey index and report the resource verdict (404 not-found) for a value that could never name a proposal at all.

Related but NOT covering: 20261029T102122Z-interconnect-acceptconnectionproposal-error-response.md. That issue is about AcceptConnectionProposal judging resource verdicts before the request's own `tags` and `attachPoint` faults; it says nothing about the `activationKey` member's own well-formedness and carries no wire replay key, and it does not touch DescribeConnectionProposal at all. The replay report marks neither cluster as owned.

=== Cause 1 of 2: DescribeConnectionProposal answers 404 ResourceNotFoundException for an activationKey that is not valid base64, where AWS answers 400 InterconnectValidationException ===
AWS: Smithy (services/service-com-amazonaws-interconnect-interconnect/smithy-model/com-amazonaws-interconnect-interconnect.json): DescribeConnectionProposal is @readonly, input {activationKey: ActivationKey, @required, @length 1..2048, @sensitive}, and models both InterconnectValidationException (@httpError 400) and ResourceNotFoundException (@httpError 404). The doc for the member says the key "was generated on a supported partner's portal" and "captures the desired parameters from the initial creation request", i.e. it is an encoded capture, not an opaque identifier. Recorded behavior for the encoding rule: a key whose text is not valid base64 is refused with HTTP status 400, error code InterconnectValidationException and the message
  "Unable to process request: The provided activation key is not valid base64"
That message is ENTIRELY fixed text — "Unable to process request: " is this service's refusal envelope and the rest is the constant sentence; it has NO variable part, and in particular it does not echo the submitted key (ActivationKey is @sensitive). The decision is made on the request alone, so it does not depend on which keys the account holds.
Emulator: answers HTTP 404 with code ResourceNotFoundException and the message "Unable to process request: Unable to retrieve a connection proposal for the supplied activationKey, as it does not exist" — the store-lookup verdict. The handler reads `store.proposal_in(&region, &input.activation_key)` as its first act and has no decoding step before it, so an undecodable key is indistinguishable from a key that was simply never issued.
Why: a caller cannot tell "the string I passed is not an activation key" from "that proposal does not exist (yet, or in this Region)". The two have opposite remedies — fix/re-copy the key versus wait for or re-request the proposal — and a client that retries or polls on 404 will poll forever on a value that can never resolve. The status class is also wrong (404 where AWS sends 400), which SDK retry policy, ALB/API-Gateway status routing and per-status dashboards split on before they read the body.
Fix: the DescribeConnectionProposal handler's input validation — decode/validate the activationKey's base64 encoding BEFORE resolving it against the store, and return the modelled InterconnectValidationException with the fixed sentence above; put the wording beside the crate's other refusal messages in src/state.rs so the sibling operation (Cause 2) shares it.
Traces: interconnect-connection-proposal-describe-and-accept-invalid-key#task#1. Source: services/service-com-amazonaws-interconnect-interconnect/src/ops/describe_connection_proposal.rs lines 154-166 (the store lookup as the first act) and lines 201-215 (the 404 ResourceNotFoundException built from crate::state::proposal_not_retrievable_message()).

=== Cause 2 of 2: AcceptConnectionProposal answers the same 404 for the same undecodable activationKey, where AWS answers the same 400 input refusal ===
AWS: Smithy: AcceptConnectionProposal is @idempotent, input {activationKey @required ActivationKey @length 1..2048 @sensitive, attachPoint @required, description, tags, clientToken}, errors include InterconnectValidationException (400) and ResourceNotFoundException (404). Recorded behavior: the identical key text is refused on the accept path exactly as on the describe path — HTTP 400, code InterconnectValidationException, message "Unable to process request: The provided activation key is not valid base64" (same fixed text, no variable part, key not echoed). The encoding of the member is judged before any proposal is resolved, so the write never reaches the store.
Emulator: answers HTTP 404 ResourceNotFoundException "Unable to process request: Unable to retrieve a connection proposal for the supplied activationKey, as it does not exist" — `judge_against_store` resolves `store.activation_keys` first and returns the not-found verdict, so the request-level fault is masked.
Why: this is the mutating half of the pair, so the misclassification also changes what a client does with a failed acceptance: a 404 reads as "the proposal is gone / not yet visible" and invites a retry of the same unusable key, while the real fault is in the key the user pasted. Keeping the two operations' verdicts identical for the same key is part of the contract — describe is the probe a client runs before accept.
Fix: the AcceptConnectionProposal handler's request-level validation — judge the activationKey's encoding (together with the other request-level faults that handler currently judges after the lookup) before `judge_against_store` resolves it, returning the shared InterconnectValidationException wording from Cause 1.
Traces: interconnect-connection-proposal-describe-and-accept-invalid-key#task#2. Source: services/service-com-amazonaws-interconnect-interconnect/src/ops/accept_connection_proposal.rs lines 202-215 (the activation-key resolution and its 404, the handler's first store verdict).

## Evidence
replay_wire_corpus, trace_set simparity-wire-aws-v1: emulator_up true, bootstrap_signal false, steps_total 22, steps_passed 9, steps_mismatched 13, steps_blocked 0, steps_errored 0, score_pct 40.9, divergence_count 12. Both clusters below are service=interconnect with empty stub_path and empty owned_by.

Cause 1 — fixture interconnect-connection-proposal-describe-and-accept-invalid-key, step task#1, operation DescribeConnectionProposal:
  status: expected 400, got 404
  $.__type: expected 'com.amazon.awsairspanpassportservice#InterconnectValidationException' (error code InterconnectValidationException), got 'ResourceNotFoundException'
  $.message: expected 'Unable to process request: The provided activation key is not valid base64', got 'Unable to process request: Unable to retrieve a connection proposal for the supplied activationKey, as it does not exist'

Cause 2 — same fixture, step task#2, operation AcceptConnectionProposal:
  status: expected 400, got 404
  $.__type: expected 'com.amazon.awsairspanpassportservice#InterconnectValidationException' (error code InterconnectValidationException), got 'ResourceNotFoundException'
  $.message: expected 'Unable to process request: The provided activation key is not valid base64', got 'Unable to process request: Unable to retrieve a connection proposal for the supplied activationKey, as it does not exist'

Model evidence (smithy_query, model_path services/service-com-amazonaws-interconnect-interconnect/smithy-model/com-amazonaws-interconnect-interconnect.json): DescribeConnectionProposal input {activationKey: ActivationKey required, @length 1..2048, @sensitive}, output {bandwidth, environmentId, provider, location}, @readonly; AcceptConnectionProposal input {attachPoint required, activationKey required @length 1..2048 @sensitive, description, tags, clientToken}, output {connection}, @idempotent. Both model AccessDeniedException(403), InterconnectClientException(400), InterconnectServerException(500), InterconnectValidationException(400), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429).

Source corroboration in the checkout: src/ops/describe_connection_proposal.rs:154-166 (proposal_in lookup, no decode step before it), :201-215 (ResourceNotFoundException + proposal_not_retrievable_message); src/ops/accept_connection_proposal.rs:202-215 (activation_keys lookup -> ResourceNotFoundException as the handler's first verdict).

## Steps to Reproduce
Boot the emulator from this checkout and either replay the corpus (replay_wire_corpus) or drive a botocore/AWS SDK interconnect client at the data listener.

1. Cause 1: call ic.describe_connection_proposal(activationKey='not!valid!base64!') (any text that is not valid base64; client-side validation does not reject it, the member is only @length 1..2048). Expected: HTTP 400 (e.response['ResponseMetadata']['HTTPStatusCode']), error code InterconnectValidationException, message exactly 'Unable to process request: The provided activation key is not valid base64' — fixed text, with the key NOT echoed. Observe HTTP 404 ResourceNotFoundException 'Unable to process request: Unable to retrieve a connection proposal for the supplied activationKey, as it does not exist'. Re-replay interconnect-connection-proposal-describe-and-accept-invalid-key#task#1.
2. Cause 2: call ic.accept_connection_proposal(activationKey='not!valid!base64!', attachPoint={'directConnectGateway': '<any uuid>'}) with the SAME key text. Expected: the identical 400 / InterconnectValidationException / 'Unable to process request: The provided activation key is not valid base64', judged before any proposal lookup, so no connection is created or mutated. Observe the same 404 ResourceNotFoundException. Re-replay interconnect-connection-proposal-describe-and-accept-invalid-key#task#2.
3. Regression guard: a WELL-FORMED base64 activationKey that names no proposal must keep its current answer on both operations (code ResourceNotFoundException with 'Unable to process request: Unable to retrieve a connection proposal for the supplied activationKey, as it does not exist'), and a valid key issued by CreateConnection must still describe and accept successfully.

Both causes must be closed, then re-replay the representative step together with every fixture#step in Issue-Trace-Ids.
