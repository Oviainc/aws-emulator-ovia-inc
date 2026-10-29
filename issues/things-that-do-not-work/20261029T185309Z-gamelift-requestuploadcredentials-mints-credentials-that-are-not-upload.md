Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: gamelift.create-build-stays-initialized-until-upload
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift.create-build-stays-initialized-until-upload|_error#23|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# GameLift RequestUploadCredentials mints credentials that are not upload-scoped: GetObject on the build key succeeds instead of AccessDenied

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RequestUploadCredentials

## Description
One cause.

Cause 1 of 1 — the temporary credentials RequestUploadCredentials hands back are not scoped to upload-only, so a read with them is permitted where AWS refuses it.
AWS: RequestUploadCredentials returns a StorageLocation (the GameLift-managed build bucket and key for that build) together with an AwsCredentials session for GameLift's TemporaryS3AccessRole. That session is scoped to the upload: PutObject at the named key succeeds, and GetObject on the SAME key with the SAME credentials is refused with HTTP 403 and error code AccessDenied, whose Message is S3's session-policy denial — fixed text "User: ", " is not authorized to perform: s3:GetObject on resource: ", and " because no session policy allows the s3:GetObject action", with the variable parts being the caller principal ARN of the minted session (as GetCallerIdentity reports it) and the quoted "arn:aws:s3:::<bucket>/<key>" built from the request's bucket and key. Smithy (com.amazonaws.gamelift#GameLift): RequestUploadCredentials output {UploadCredentials: AwsCredentials (sensitive), StorageLocation: S3Location}; the upload-only scoping is recorded behavior only (the model does not describe the session policy) and the recording shows the 403 AccessDenied above.
Emulator: the StorageLocation and the credential fields are returned and PutObject with them succeeds, but GetObject on the very same bucket/key with the very same credentials returns HTTP 200 with the object body (observed: 200, content-length 31, the payload just written) instead of being refused. No session policy is enforced on the minted credentials at all, so no AccessDenied is ever produced.
Why: the emulator is overly permissive on a credential-scoping boundary. A client (or a test of least-privilege behavior) that verifies the build-upload credentials cannot read back, or that handles AccessDenied from a read-back attempt, sees success instead; code written against the emulator would ship assuming these credentials grant read access to the build bucket, and on real AWS every such read fails. It also hides the distinction between the build-upload principal and the caller's own principal.
Fix: the credential minting behind RequestUploadCredentials (and the CreateBuild upload-credentials path that shares it) must attach the upload-only session policy to the minted session, and the S3 authorization path must enforce that policy for sessions minted this way — refusing non-upload actions on the build key with 403 AccessDenied and S3's session-policy message template above.
Traces: gamelift.create-build-stays-initialized-until-upload (the build-status assertions in the same check passed; the failure is the read-back refusal).

RELATED (does not cover this): open issue 20261029T183132Z-gamelift-build-and-script-upload-storagelocation-and-uploadcredentials.md files the wrong bucket/key naming and the non-AWS AccessKeyId SHAPE for builds and scripts. This issue is about what the minted session is PERMITTED to do, which that issue does not address; fixing the bucket name or key format alone leaves this check failing.

## Evidence
gamelift.create-build-stays-initialized-until-upload (mismatch).
Flow: CreateBuild (no ZipFile/StorageLocation) → Build INITIALIZED, SizeOnDisk 0, StorageLocation present; DescribeBuild still INITIALIZED; RequestUploadCredentials returns the same Bucket/Key plus UploadCredentials with an AccessKeyId. PutObject(Bucket, Key, 31-byte payload) with the minted credentials → 200 (as AWS).
Divergence: GetObject(Bucket, Key) with those same minted credentials was expected to raise ClientError with Error.Code 'AccessDenied' and Error.Message 'User: <minted-principal-arn> is not authorized to perform: s3:GetObject on resource: "arn:aws:s3:::<bucket>/<key>" because no session policy allows the s3:GetObject action'. The emulator instead returned HTTP 200 with the object: {'HTTPStatusCode': 200, 'ContentLength': 31, 'ETag': '"6d518b22bfd070803df38a16c794e336"', 'ContentType': 'binary/octet-stream', ...} and a readable Body. The check failed at the read-back step with "expected a ClientError, but the call succeeded".
(The principal ARN, bucket, key, etag and request ids above are this run's values filling the message/response template, not part of the requirement.)

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (simparity-behavioral-aws-v1), gamelift checks.
2. CreateBuild(Name=<unique>, Version, OperatingSystem, ServerSdkVersion) with no ZipFile and no StorageLocation; note StorageLocation.Bucket/Key.
3. RequestUploadCredentials(BuildId=<the new build id>); build an S3 client signing with the returned UploadCredentials (AccessKeyId/SecretAccessKey/SessionToken).
4. With that client, PutObject to the exact returned Bucket/Key — expect 200.
5. With the SAME client, GetObject on the same Bucket/Key — AWS refuses with 403 AccessDenied and the session-policy message; the emulator returns 200 with the object body.
6. Re-replay gamelift.create-build-stays-initialized-until-upload to confirm the read-back is refused with exactly AccessDenied and AWS's message text.
