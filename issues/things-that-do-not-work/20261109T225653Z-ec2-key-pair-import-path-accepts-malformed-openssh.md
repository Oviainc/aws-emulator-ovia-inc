Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes ec2-conflict-key-pair-duplicate ec2-t12-import-key-pair-fingerprint
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|ImportKeyPair|4xx->2xx|$.Response
Issue-Key: ec2|ImportKeyPair|4xx->2xx|status
Issue-Key: ec2|ImportKeyPair|2xx->2xx|$.ImportKeyPairResponse.keyFingerprint
Issue-Key: ec2|DescribeKeyPairs|2xx->2xx|$.DescribeKeyPairsResponse.keySet.item.keyFingerprint
Issue-Key: ec2|DescribeKeyPairs|2xx->2xx|$.DescribeKeyPairsResponse.keySet.item.publicKey
Issue-Commit: 3b171590106c1f2d83787caf96ecc291950ccc13

# EC2 key-pair import path accepts malformed OpenSSH material and misrenders fingerprint and public key

> _Note: this issue was identified against the code base at commit `3b171590106c1f2d83787caf96ecc291950ccc13`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportKeyPair

## Description
This issue bundles THREE distinct causes in the EC2 key-pair import/serialize path. All THREE must be closed before submitting.

--- Cause 1 of 3: ImportKeyPair accepts public-key material that is not a valid OpenSSH public key ---
AWS: Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): ImportKeyPair input member PublicKeyMaterial targets Blob (xmlName publicKeyMaterial, required); the operation declares no modeled errors (errors: []) because EC2 is an ec2Query service whose error codes are wire-only. Recorded behavior: AWS parses the supplied material as an OpenSSH public key BEFORE creating anything and refuses HTTP 400 with Response.Errors.Error.Code 'InvalidKey.Format' and Response.Errors.Error.Message 'Key is not in valid OpenSSH public key format'. That message has NO variable part — it is fixed text for this error class, independent of the key name or the bytes supplied.
Emulator: HTTP 200 with an ImportKeyPairResponse — the key pair is created. The replay reports 'status: expected 400, got 200' and '$.Response: missing (expected {Errors:{Error:{Code:InvalidKey.Format, Message:"Key is not in valid OpenSSH public key format"}}}, got None)'.
Why: a client that feeds EC2 an arbitrary blob (a private key by mistake, a PEM body, a truncated paste) gets a success and a key-pair id back. Every later launch that references that key pair is unusable, and the failure surfaces at SSH time rather than at the API call that caused it. Validation-negative test paths that assert InvalidKey.Format never fire.
Fix: ImportKeyPair's input validation, in services/service-com-amazonaws-ec2-amazonec2/src/ops/import_key_pair.rs — decode PublicKeyMaterial as an OpenSSH public key (type token, base64 body, consistent embedded algorithm name) before any state mutation and refuse with 400 / InvalidKey.Format and the fixed message above.
Traces: ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes#task#3.

--- Cause 2 of 3: keyFingerprint is rendered in ssh-keygen form ('SHA256:' prefix, unpadded base64) instead of AWS's base64 digest ---
AWS: Smithy shape KeyPairInfo member KeyFingerprint (xmlName keyFingerprint) documents the rule verbatim: 'If you used ImportKeyPair to provide Amazon Web Services the public key: ... For ED25519 key pairs, the key fingerprint is the base64-encoded SHA-256 digest'. So the wire value is the base64 encoding of the 32-byte SHA-256 digest — standard base64 WITH padding, and with NO algorithm prefix. The recording shows exactly that form for the imported ed25519 key, and the same value is returned by both ImportKeyPair (xmlName keyFingerprint on ImportKeyPairResult) and DescribeKeyPairs (keySet item keyFingerprint). The concrete digest in the recording is a value derived from the request's key material, not part of the requirement; the FORM is the requirement.
Emulator: it emits the OpenSSH `ssh-keygen -l` rendering instead — a literal 'SHA256:' prefix followed by the base64 digest with the '=' padding stripped. Replay: '$.ImportKeyPairResponse.keyFingerprint: value_mismatch (expected "ZH0V/nWyaiuogNQvtULVPgWyzoZbhsz5mhxTx1J6fvs=", got "SHA256:ZH0V/nWyaiuogNQvtULVPgWyzoZbhsz5mhxTx1J6fvs")' and the identical mismatch on '$.DescribeKeyPairsResponse.keySet.item.keyFingerprint'.
Why: the fingerprint is the identity clients match on. Any caller that compares the fingerprint it computed locally (or recorded earlier) against the one EC2 reports never matches, so 'is this the key I think it is?' checks and idempotent-import logic both fail; the string is also 8 characters longer than AWS's, which breaks fixed-width storage and exact-match filters.
Fix: the key-pair fingerprint computation/serialization shared by both handlers — services/service-com-amazonaws-ec2-amazonec2/src/ops/import_key_pair.rs and .../describe_key_pairs.rs (and the key-pair state record that stores it) — emit padded base64 of the SHA-256 digest with no prefix for imported ed25519 material.
Traces: ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes#task#14 (cluster also covers ec2-conflict-key-pair-duplicate#task#1 and ec2-t12-import-key-pair-fingerprint#task#4) and ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes#task#12 (cluster also covers #task#15).

--- Cause 3 of 3: DescribeKeyPairs does not echo the imported public key material verbatim — the key comment is truncated ---
AWS: Smithy shape KeyPairInfo member PublicKey (xmlName publicKey) — 'The public key material'. Recorded behavior: when DescribeKeyPairs is called with IncludePublicKey, the publicKey element is the BYTE-FOR-BYTE material that was handed to ImportKeyPair, including the trailing comment field of the OpenSSH line. The requirement is 'whatever the request supplied, returned unchanged'; the concrete key body and comment in the recording are request values.
Emulator: it returns the algorithm token and base64 body unchanged but truncates the comment — replay: '$.DescribeKeyPairsResponse.keySet.item.publicKey: value_mismatch (expected "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILQS1/6CzMacTQNT+B0ftXfqbeN2n5UaZufrUqFVcyAD sim2real-probe-asgssh-kp-4c1e", got "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILQS1/6CzMacTQNT+B0ftXfqbeN2n5UaZufrUqFVcyAD sim2real-probe-asgssh")'. The same step also carries the Cause 2 fingerprint mismatch.
Why: the comment is the only human-readable provenance on a public key and tools round-trip it (authorized_keys generation, key inventories, config drift checks). A client that writes back what EC2 returns silently rewrites its own key metadata, and any byte comparison against the material it imported fails.
Hypothesis: the import path parses the OpenSSH line into fields and re-serializes it, dropping or re-deriving the comment, instead of storing the supplied bytes.
Fix: the key-pair state record plus DescribeKeyPairs' projection — services/service-com-amazonaws-ec2-amazonec2/src/ops/import_key_pair.rs must persist PublicKeyMaterial verbatim and .../describe_key_pairs.rs must emit it unmodified.
Traces: ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes#task#13.

Related but NOT covering: open issue 20261109T072050Z-ec2-key-pair-read-path-diverges-wrong-refusal.md bundles four other key-pair READ causes (malformed-id refusal code, empty keySet, case-sensitivity, deleted keys still returned). The replay reports owned_by empty and stub_path empty for all three steps above — note that its Cause 2 (empty keySet at #task#12) no longer reproduces: that step now returns an item and diverges only on the fingerprint form, which is Cause 2 here.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true, bootstrap_signal false, 2742/3942 steps passed, 1081 mismatched, 602 divergence clusters. Three net-new clusters, each with stub_path empty and owned_by empty:

1) ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes#task#3 — ImportKeyPair, status expected 400, got 200.
   $.Response: missing (expected {'Errors': {'Error': {'Code': 'InvalidKey.Format', 'Message': 'Key is not in valid OpenSSH public key format'}}, 'RequestID': '<shaped-not-asserted>'}, got None)

2) ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes#task#14 (affects ec2-conflict-key-pair-duplicate#task#1, ec2-t12-import-key-pair-fingerprint#task#4) — ImportKeyPair, 200 vs 200.
   $.ImportKeyPairResponse.keyFingerprint: value_mismatch (expected 'ZH0V/nWyaiuogNQvtULVPgWyzoZbhsz5mhxTx1J6fvs=', got 'SHA256:ZH0V/nWyaiuogNQvtULVPgWyzoZbhsz5mhxTx1J6fvs')
   ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes#task#12 (affects #task#15) — DescribeKeyPairs, 200 vs 200, same mismatch on $.DescribeKeyPairsResponse.keySet.item.keyFingerprint.

3) ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes#task#13 — DescribeKeyPairs, 200 vs 200.
   $.DescribeKeyPairsResponse.keySet.item.publicKey: value_mismatch (expected '... sim2real-probe-asgssh-kp-4c1e', got '... sim2real-probe-asgssh')
   $.DescribeKeyPairsResponse.keySet.item.keyFingerprint: same 'SHA256:'-prefixed form as above.

Model evidence: smithy_query operation=ImportKeyPair -> input {KeyName required, PublicKeyMaterial Blob required, TagSpecifications, DryRun}, output {KeyFingerprint, KeyName, KeyPairId, Tags}, errors []. smithy_query doc shape=KeyPairInfo member=KeyFingerprint -> 'If you used ImportKeyPair ... For ED25519 key pairs, the key fingerprint is the base64-encoded SHA-256 digest'. smithy_query shape=KeyPairInfo -> member PublicKey 'The public key material'.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes step task#3 — ImportKeyPair with PublicKeyMaterial that is not an OpenSSH public key. Observe HTTP 200 and a created key pair instead of HTTP 400 / InvalidKey.Format 'Key is not in valid OpenSSH public key format'.
4. Cause 2: replay the same fixture step task#14 — ImportKeyPair with ed25519 OpenSSH material. Observe keyFingerprint 'SHA256:<unpadded base64>' instead of '<padded base64>'. Then step task#12 — DescribeKeyPairs for that key name shows the same wrong form in keySet.item.keyFingerprint. Same divergence at ec2-conflict-key-pair-duplicate#task#1 and ec2-t12-import-key-pair-fingerprint#task#4.
5. Cause 3: replay the same fixture step task#13 — DescribeKeyPairs with IncludePublicKey. Observe publicKey returned with its OpenSSH comment truncated instead of the exact bytes supplied to ImportKeyPair.
