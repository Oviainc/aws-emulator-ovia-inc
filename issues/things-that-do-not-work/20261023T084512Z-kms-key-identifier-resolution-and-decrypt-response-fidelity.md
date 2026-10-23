Issue-Source: trace-sweep-divergence
Issue-Trace: kms-alias-lifecycle
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kms-alias-lifecycle kms-symmetric-encrypt-decrypt
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|DescribeKey|4xx->2xx|$
Issue-Key: kms|DescribeKey|4xx->2xx|$.__type
Issue-Key: kms|DescribeKey|4xx->2xx|$.message
Issue-Key: kms|DescribeKey|4xx->2xx|status
Issue-Key: kms|Decrypt|2xx->2xx|$.KeyOrigin
Issue-Commit: 47d23180b7cc904919338b787c84ed6c6bd353ce

# KMS key-identifier resolution and Decrypt response fidelity gaps

> _Note: this issue was identified against the code base at commit `47d23180b7cc904919338b787c84ed6c6bd353ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeKey

## Description
Two net-new wire divergences in the KMS emulator, both reached through ordinary key-identifier/response paths. 2 causes in this issue; both must be closed.

Cause 1 of 2 — DescribeKey by an alias identifier that no longer exists answers 200 with the former target key's metadata instead of NotFoundException (fixture kms-alias-lifecycle, step task#11).
AWS: DescribeKey takes KeyId (KeyIdType, required, @length 1..2048), which may be a key id, key ARN, alias name or alias ARN, and models NotFoundException (Smithy: DescribeKey errors = DependencyTimeoutException, InvalidArnException, KMSInternalException, NotFoundException). In the recording, once the alias no longer resolves in the alias lifecycle, DescribeKey on the alias ARN is answered with HTTP 400 and body __type 'NotFoundException' (example message: "Alias arn:aws:kms:us-west-2:000000000000:alias/sim2real-probe-kms-probe-<suffix> is not found." — the message text is an example, the status 400 and the __type NotFoundException are the requirement). The step is a poll, so AWS is expected to settle on that error.
Emulator: answers HTTP 200 with a full KeyMetadata body for the alias's former target key (the replay shows KeyMetadata for the probe's first alias key, KeyState Enabled), and never surfaces any error code across 20 poll attempts; $.__type and $.message are absent entirely.
Why: a client cannot distinguish a removed alias from a live one. Alias teardown/rotation logic that probes DescribeKey on an alias to confirm it is gone loops or concludes the alias still points at the old key, and encrypt/decrypt calls routed via that alias silently target a key the caller believes is unreachable — a success where AWS fails closed.
Fix: belongs in the KMS alias/key-identifier resolution used by the DescribeKey handler and in the alias state store — removing an alias must invalidate the alias->key index so lookups by alias name/ARN raise NotFoundException (HTTP 400, __type NotFoundException) rather than falling back to the last known target. Hypothesis: the alias entry is dropped from the alias listing but the resolution index retains the mapping.
Traces: kms-alias-lifecycle#task#11 (DescribeKey, expected 400 / got 200).

Cause 2 of 2 — Decrypt omits the KeyOrigin response member (fixture kms-symmetric-encrypt-decrypt, step task#4).
AWS: recorded behavior only — the on-disk Smithy model's DecryptResponse declares KeyId, Plaintext, EncryptionAlgorithm, CiphertextForRecipient and KeyMaterialId and does NOT declare KeyOrigin, but the recorded AWS response for a symmetric-default CMK Decrypt carries KeyOrigin with value 'AWS_KMS' (the same origin DescribeKey reports in KeyMetadata.Origin). Status is 200 on both sides.
Emulator: returns 200 but with no KeyOrigin member at all ($.KeyOrigin missing, expected 'AWS_KMS').
Why: a client that reads Decrypt's KeyOrigin to decide how the backing key material is held (AWS_KMS vs EXTERNAL vs a CloudHSM/external store) gets a null where AWS gives a value, so origin-dependent branches take the wrong path or throw on the missing field; SDK-level round-trip and response-shape assertions over Decrypt also fail.
Fix: belongs in the Decrypt serializer (and the DecryptResponse shape in the KMS model, which lacks the member) — emit KeyOrigin for the resolved key, sourced from the same key-state field that backs KeyMetadata.Origin, so it stays consistent with DescribeKey.
Traces: kms-symmetric-encrypt-decrypt#task#4 (Decrypt, 200 vs 200, body member missing).

Neither cause is covered by an open issue: the open KMS issues are unimplemented-operation stubs (kms TagResource, kms ListGrants, kms PutKeyPolicy) and unrelated cross-service KMS-key checks (ec2-led neighbouring-service gaps, xray PutEncryptionConfig).

## Evidence
Replay of trace set simparity-wire-aws-v1 (64 fixtures, 475 steps, emulator_up=true, bootstrap_signal=false, state_reset_failures=0):

Cause 1 — fixture kms-alias-lifecycle, step task#11, operation DescribeKey, verdict mismatch:
  expected_status 400, actual_status 200
  $.__type: missing (expected 'NotFoundException', got None)
  $.message: missing (expected 'Alias arn:aws:kms:us-west-2:000000000000:alias/sim2real-probe-kms-probe-55b540c6 is not found.', got None)  [message quoted as an example]
  poll: error code None not in ['NotFoundException'] after 20 attempt(s); got body {'KeyMetadata': {'KeyId': 'cf4792db-...', 'Description': 'probe-alias-key-1', 'KeyState': 'Enabled', 'Enabled': True, 'Origin': 'AWS_KMS', 'KeySpec': 'SYMMETRIC_DEFAULT', ...}}
  divergence_key: service=kms|DescribeKey|status=400->200|keys=$,$.__type,$.message,status

Cause 2 — fixture kms-symmetric-encrypt-decrypt, step task#4, operation Decrypt, verdict mismatch:
  expected_status 200, actual_status 200
  $.KeyOrigin: missing (expected 'AWS_KMS', got None)
  divergence_key: service=kms|Decrypt|status=200->200|keys=$.KeyOrigin

Model cited (services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json):
  DescribeKey input KeyId: KeyIdType, required, @length 1..2048; errors include NotFoundException (client)
  DecryptResponse members: KeyId, Plaintext, EncryptionAlgorithm, CiphertextForRecipient, KeyMaterialId — no KeyOrigin member modelled, while the recording shows AWS returning it

## Steps to Reproduce
Cause 1: replay fixture kms-alias-lifecycle up to step task#11 — create a CMK, point an alias at it, run the alias lifecycle to the point where the alias no longer exists, then call DescribeKey with the alias ARN (arn:aws:kms:<region>:<account>:alias/<name>). Emulator answers 200 with KeyMetadata for the alias's former target key and keeps doing so for all 20 poll attempts; AWS answers 400 with __type NotFoundException.

Cause 2: replay fixture kms-symmetric-encrypt-decrypt up to step task#4 — create a SYMMETRIC_DEFAULT CMK, Encrypt a plaintext, then Decrypt the ciphertext. Inspect the 200 response body: the emulator emits no KeyOrigin member, AWS emits KeyOrigin='AWS_KMS'.

Both: re-replay the wire corpus and confirm kms-alias-lifecycle#task#11 and kms-symmetric-encrypt-decrypt#task#4 pass.
