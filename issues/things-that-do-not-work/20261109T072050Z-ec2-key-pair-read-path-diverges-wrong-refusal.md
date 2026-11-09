Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes ec2-t12-import-key-pair-fingerprint ec2-delete-key-pair-dryrun-idempotency-contract
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeKeyPairs|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeKeyPairs|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeKeyPairs|2xx->2xx|$.DescribeKeyPairsResponse.keySet
Issue-Key: ec2|DescribeKeyPairs|2xx->4xx|$.DescribeKeyPairsResponse
Issue-Key: ec2|DescribeKeyPairs|2xx->4xx|status
Issue-Key: ec2|DescribeKeyPairs|4xx->2xx|$
Issue-Key: ec2|DescribeKeyPairs|4xx->2xx|$.Response
Issue-Key: ec2|DescribeKeyPairs|4xx->2xx|status
Issue-Commit: 70f3a3a7604d630d6ba1835b99fbb61e8ce58784

# EC2 key-pair read path diverges: wrong refusal code, missing imported keys, and deleted keys still returned

> _Note: this issue was identified against the code base at commit `70f3a3a7604d630d6ba1835b99fbb61e8ce58784`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeKeyPairs

## Description
This issue bundles FOUR distinct causes, all in the EC2 key-pair read path (DescribeKeyPairs and the key-pair state it projects). All four must be closed before submitting.

--- Cause 1 of 4: a syntactically invalid keyPairId is refused as a lookup miss instead of a parameter-format error ---
AWS: Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): DescribeKeyPairs input member KeyPairIds targets KeyPairIdStringList (xmlName KeyPairId); the operation declares no modeled errors (errors: []), because EC2 is an ec2Query service whose error codes are wire-only. Recorded behavior: AWS validates the SHAPE of each supplied key-pair id before any lookup and answers HTTP 400 with Response.Errors.Error.Code 'InvalidParameterValue' and Response.Errors.Error.Message "Invalid value '<keyPairId>' for keyPairId.". Message template: the fixed text is "Invalid value '" ... "' for keyPairId."; the only variable part is the KeyPairIds entry copied verbatim from the request (the recording's 'key-123' is one such request value, not part of the template).
Emulator: HTTP 400 with Code 'InvalidKeyPair.NotFound' and Message "The key pair ID '<value>' does not exist" - it skips format validation and reports a lookup miss.
Why: a client cannot tell a typo'd/ill-formed id from a valid id that was deleted or belongs to another account. SDK retry and error-mapping logic keys on the code: InvalidParameterValue is a non-retryable client input error, while InvalidKeyPair.NotFound is treated as a resource-state condition, so callers that branch on it (e.g. 'create the key pair if it is not found') will take the wrong branch and try to create a key pair for a malformed id.
Fix: the DescribeKeyPairs handler's input validation, in services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_key_pairs.rs - validate every KeyPairIds entry against the key-pair id format first and refuse with InvalidParameterValue; reserve InvalidKeyPair.NotFound for well-formed ids that are genuinely absent.
Traces: ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes#task#10 (cluster also covers #task#11).

--- Cause 2 of 4: a key pair that exists in the fixture's state is absent from the DescribeKeyPairs projection ---
AWS: Smithy: DescribeKeyPairs output member KeyPairs (xmlName keySet, ec2QueryName KeySet) is a KeyPairList of KeyPairInfo, whose members are keyName, keyPairId, keyFingerprint, keyType (enum KeyType), createTime (MillisecondDateTime) and tagSet. After a key pair has been imported, a DescribeKeyPairs scoped to that key name returns exactly one KeyPairInfo item for it, with keyType reflecting the imported material (ed25519 for an ed25519 public key) and the fingerprint AWS computed for it.
Emulator: HTTP 200, but keySet is serialized as an empty string - no item element at all (replay reports type_mismatch: expected an object with an 'item', got '').
Why: every client that confirms a key pair exists before launching an instance, or that enumerates key pairs to pick one, sees no key pair even though the preceding step succeeded. Read-after-write on the key-pair resource is broken.
Fix: the key-pair state store plus the DescribeKeyPairs projection in services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_key_pairs.rs - the imported key pair must be persisted and projected as a KeyPairInfo item.
Hypothesis: the import path never writes the key pair into state, so the read path has nothing to project. Related but NOT covering: open issue 20261022T235108Z-ec2-importkeypair-stub.md reports ImportKeyPair as an unimplemented scaffold; the replay reports owned_by empty and stub_path empty for this DescribeKeyPairs step, so that issue does not carry this step's replay key.
Traces: ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes#task#12.

--- Cause 3 of 4: a key pair whose name differs from another only in letter case is not found at all ---
AWS: recorded behavior only (the model constrains neither key-name case nor the refusal). The recording shows DescribeKeyPairs scoped to a key name that differs from an already-present key pair only in letter case answering HTTP 200 with that key pair's own KeyPairInfo (its own keyPairId, keyType ed25519, keyFingerprint, createTime, empty tagSet) - i.e. EC2 key-pair names are case-sensitive and the two names denote two distinct key pairs.
Emulator: HTTP 400 with no DescribeKeyPairsResponse body at all (replay: status expected 200, got 400; $.DescribeKeyPairsResponse missing).
Why: a client that uses mixed-case key-pair names cannot read back a key pair it just created, and any workflow keyed on the exact name it supplied fails with a hard 400 instead of getting its resource.
Fix: the key-pair state keying and lookup used by DescribeKeyPairs (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_key_pairs.rs and the key-pair store) - key-name comparison must be byte-exact/case-sensitive so case-distinct names are distinct key pairs.
Traces: ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes#task#15 (cluster also covers ec2-t12-import-key-pair-fingerprint#task#2).

--- Cause 4 of 4: a deleted key pair is still returned by DescribeKeyPairs ---
AWS: recorded behavior (DescribeKeyPairs declares no modeled errors). After the key pair is deleted, DescribeKeyPairs scoped to that name answers HTTP 400 with Response.Errors.Error.Code 'InvalidKeyPair.NotFound' and Response.Errors.Error.Message "The key pair '<keyName>' does not exist". Message template: fixed text "The key pair '" ... "' does not exist"; the variable part is the key NAME taken from the request (note this is the name form, distinct from the id form "The key pair ID '<id>' does not exist"). The recording's concrete name and the emulator's generated keyPairId/createTime/fingerprint are per-run values, not requirements.
Emulator: HTTP 200 with the deleted key pair still present in keySet (keyName, keyPairId, keyType rsa, keyFingerprint, createTime, empty tagSet), and it keeps returning it - the fixture's poll gave up after 12 attempts with no error code ever appearing.
Why: deletion is not observable. A client that deletes a key pair and polls until it is gone never terminates; a client that re-creates the same name sees a stale record; idempotent teardown logic believes the resource still exists. This is the most severe of the four because state is wrong, not merely the rendering of an error.
Fix: the DeleteKeyPair handler's state mutation plus the DescribeKeyPairs lookup (services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_key_pair.rs and .../describe_key_pairs.rs) - deletion must remove the key pair from state, and a name-scoped describe of an absent key pair must refuse with 400 InvalidKeyPair.NotFound.
Traces: ec2-delete-key-pair-dryrun-idempotency-contract#task#6 (cluster also covers #task#8).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, bootstrap_signal false, 2630/3817 steps passed, 1074 mismatched). Four net-new DescribeKeyPairs clusters, each with stub_path empty and owned_by empty:

1) ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes#task#10 (affects #task#11) - status 400 vs 400.
   $.Response.Errors.Error.Code: expected 'InvalidParameterValue', got 'InvalidKeyPair.NotFound'
   $.Response.Errors.Error.Message: expected "Invalid value 'key-123' for keyPairId.", got "The key pair ID 'key-123' does not exist"

2) ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes#task#12 - status 200 vs 200.
   $.DescribeKeyPairsResponse.keySet: type_mismatch - expected {'item': {'keyName': ..., 'keyPairId': ..., 'keyFingerprint': ..., 'keyType': 'ed25519', 'createTime': <shaped-not-asserted>, 'tagSet': ''}}, got ''

3) ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes#task#15 (affects ec2-t12-import-key-pair-fingerprint#task#2) - status expected 200, got 400.
   $.DescribeKeyPairsResponse: missing - expected the KeyPairInfo item for the case-distinct key name (keyType 'ed25519'), got None

4) ec2-delete-key-pair-dryrun-idempotency-contract#task#6 (affects #task#8) - status expected 400, got 200.
   $.Response: missing - expected {'Errors': {'Error': {'Code': 'InvalidKeyPair.NotFound', 'Message': "The key pair '<name>' does not exist"}}}, got None
   $: poll: error code None not in ['InvalidKeyPair.NotFound'] after 12 attempt(s); the emulator kept answering DescribeKeyPairsResponse with the deleted key pair still in keySet.

Model evidence: smithy_query operation=DescribeKeyPairs -> input {KeyNames, KeyPairIds, IncludePublicKey, DryRun, Filters}, output {KeyPairs xmlName keySet}, errors []. smithy_query shape=KeyPairInfo -> {keyPairId, keyType, tagSet, publicKey, createTime, keyName, keyFingerprint}.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes step task#10 - DescribeKeyPairs with a KeyPairId that is not in EC2's key-pair id format. Observe 400 / InvalidKeyPair.NotFound instead of 400 / InvalidParameterValue "Invalid value '<keyPairId>' for keyPairId.".
4. Cause 2: replay the same fixture through task#12 - import a key pair with ed25519 material, then DescribeKeyPairs for its name. Observe HTTP 200 with an empty keySet instead of one KeyPairInfo item.
5. Cause 3: replay the same fixture through task#15 - DescribeKeyPairs for a key name that differs from an existing key pair only in letter case. Observe HTTP 400 with no response body instead of HTTP 200 with that key pair's KeyPairInfo. Same divergence at ec2-t12-import-key-pair-fingerprint#task#2.
6. Cause 4: replay ec2-delete-key-pair-dryrun-idempotency-contract through task#6 (and task#8) - create a key pair, DeleteKeyPair it, then DescribeKeyPairs for its name. Observe HTTP 200 with the deleted key pair still in keySet on every one of the 12 poll attempts, instead of HTTP 400 / InvalidKeyPair.NotFound "The key pair '<keyName>' does not exist".
