Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.s3-object-version-pins-deployed-code lambda__s3.zipfile-and-s3-source-deploy-identical-code lambda__s3.update-code-from-missing-s3-key-rejected
Issue-Key: lambda__s3.s3-object-version-pins-deployed-code|run#11|KeyError|assert created["CodeSha256"] == sha[old_zip], "the pinned object version's bytes must be stored"
Issue-Key: lambda__s3.zipfile-and-s3-source-deploy-identical-code|run#11|KeyError|shapes = {n: (c["CodeSha256"], c["CodeSize"]) for n, c in stored.items()}
Issue-Key: lambda__s3.update-code-from-missing-s3-key-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# S3-sourced Lambda code: CodeSha256/CodeSize are absent and a missing S3 key is accepted (2 causes)

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFunctionCode

## Description
Two net-new root causes in the lambda__s3 group; the replay report marks neither ALREADY FILED nor OWNED (the group's open issues `20261027T052624Z-lambda-s3-an-s3-sourced-layer-version-omits.md` and `20261025T022656Z-lambda-s3-layer-content-from-s3-loses-codesha256.md` are about LAYER content and carry other replay keys).

Cause 1 of 2 - A function deployed from an S3 object reports no CodeSha256/CodeSize.
AWS: CreateFunction/UpdateFunctionCode with Code={S3Bucket,S3Key[,S3ObjectVersion]} stores the object's bytes and reports CodeSha256 (base64 SHA-256 of the package) and CodeSize in the function configuration - identical to the values a ZipFile deployment of the same bytes produces, and pinned to the S3ObjectVersion when one is given.
Emulator: the configuration has no CodeSha256 member at all, so reading it raises `KeyError: 'CodeSha256'`; a sibling S3 deployment also reports `CodeSize: 0`.
Why: CodeSha256 is how clients verify which artifact is deployed and whether a redeploy changed anything; its absence breaks content-addressed deploys and makes zip-vs-S3 equivalence unverifiable.
Fix: the S3 code-source path of the CreateFunction/UpdateFunctionCode handlers and the function code store (fetch the object, hash it, record size; honor S3ObjectVersion).
Traces: lambda__s3.s3-object-version-pins-deployed-code, lambda__s3.zipfile-and-s3-source-deploy-identical-code.

Cause 2 of 2 - UpdateFunctionCode naming an S3 key that does not exist succeeds.
AWS: UpdateFunctionCode whose S3Key is absent from the bucket is refused with `400 InvalidParameterValueException` ("Error occurred while GetObject. S3 Error Code: NoSuchKey...") and the function keeps its previous code.
Emulator: the call returns `200` with a full FunctionConfiguration (`'CodeSize': 0`, no CodeSha256, LastUpdateStatus=InProgress), so the function is left pointing at nothing.
Why: a deploy typo silently "succeeds" and the function is left with empty code instead of its previous working package - the opposite of AWS's fail-closed behavior.
Fix: S3 object resolution in the UpdateFunctionCode handler (refuse before mutating state).
Traces: lambda__s3.update-code-from-missing-s3-key-rejected.

## Evidence
Cause 1: `shapes = {n: (c["CodeSha256"], c["CodeSize"]) ...}` -> `KeyError: 'CodeSha256'` (lambda__s3.zipfile-and-s3-source-deploy-identical-code) and `assert created["CodeSha256"] == sha[old_zip]` -> `KeyError: 'CodeSha256'` (lambda__s3.s3-object-version-pins-deployed-code).
Cause 2: `AssertionError: expected the call to be refused, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200 ...}, 'CodeSize': 0, 'LastUpdateStatus': 'InProgress', ...}` - no CodeSha256 and no refusal.

## Steps to Reproduce
1. git_clone(); 2. run_behavioral_suite(); 3. replay the ids in trace_ids: upload a zip to a versioned bucket, CreateFunction with Code={S3Bucket,S3Key,S3ObjectVersion} and read CodeSha256/CodeSize; deploy the same bytes via ZipFile and compare; then UpdateFunctionCode with an S3Key that does not exist.
