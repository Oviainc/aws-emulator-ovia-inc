Issue-Source: trace-sweep-divergence
Issue-Trace: gamelift-build-error-paths
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gamelift-build-error-paths gamelift-build-lifecycle-and-tags gamelift-script-lifecycle gamelift-script-tagging-roundtrip
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift|CreateBuild|2xx->2xx|$.StorageLocation.Bucket
Issue-Key: gamelift|CreateBuild|2xx->2xx|$.StorageLocation.Key
Issue-Key: gamelift|CreateBuild|2xx->2xx|$.UploadCredentials.AccessKeyId
Issue-Key: gamelift|RequestUploadCredentials|2xx->2xx|$.StorageLocation.Bucket
Issue-Key: gamelift|RequestUploadCredentials|2xx->2xx|$.StorageLocation.Key
Issue-Key: gamelift|RequestUploadCredentials|2xx->2xx|$.UploadCredentials.AccessKeyId
Issue-Key: gamelift|CreateScript|2xx->2xx|$.Script.StorageLocation.Bucket
Issue-Key: gamelift|CreateScript|2xx->2xx|$.Script.StorageLocation.Key
Issue-Related: 20261029T183132Z-gamelift-update-operations-return-success-but-do-not.md
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# GameLift build and script upload StorageLocation and UploadCredentials are wrong

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBuild

## Description
Five independent causes, one shared fault: the service-managed S3 upload location the emulator reports for builds and scripts does not follow the AWS naming rule for bucket or key, and the temporary upload credentials it mints are not shaped like AWS credentials. A fixer must close all FIVE before submitting.

Cause 1 of 5 - CreateBuild StorageLocation and UploadCredentials.
AWS: when CreateBuild is called without a customer StorageLocation, the response's StorageLocation names the GameLift-managed build bucket for the request's region, whose name is "prod-gamescale-builds-<region>", and the Key is "<awsAccountId>/<buildId>" - the account id of the caller, a slash, then the generated build id, with no file name appended. UploadCredentials is an AwsCredentials (sensitive) whose AccessKeyId is a temporary-credential access key id (the recording carries it as [REDACTED:aws-access-key], i.e. the value is asserted by shape, and an AWS temporary access key id is a 20-character uppercase key beginning with the ASIA prefix). Smithy: CreateBuild output {Build, UploadCredentials: AwsCredentials (sensitive), StorageLocation: S3Location}; S3Location.Bucket/Key are NonEmptyString; AwsCredentials.AccessKeyId NonEmptyString.
Emulator: Bucket is "gamelift-build-uploads-<accountId>" (own invented naming, account-qualified instead of region-qualified and not the prod-gamescale-builds family), Key is "<buildId>/build.zip" (resource id first, plus a hard-coded file name, and no account id), and AccessKeyId is "EMULATORAS..." - a 20-character literal starting with EMULATOR, which no AWS access key id does.
Why: a client that takes StorageLocation+UploadCredentials and uploads the build zip to the named bucket/key - the documented CreateBuild upload flow - writes to a path AWS would never hand out, and credential-shape checks (prefix/length) on the returned AccessKeyId fail.
Fix: the CreateBuild handler's upload-location derivation and its credential minting (bucket = prod-gamescale-builds-<region>, key = <accountId>/<buildId>, access key id in the AWS temporary-credential form). Region and account id come from the request context, not from a literal.
Traces: gamelift-build-error-paths#task#2 (also gamelift-build-lifecycle-and-tags#task#1).

Cause 2 of 5 - RequestUploadCredentials returns the same wrong location and credentials.
AWS: RequestUploadCredentials re-issues credentials for an existing build and returns the SAME StorageLocation as CreateBuild for that build - bucket "prod-gamescale-builds-<region>", key "<awsAccountId>/<buildId>" - plus a fresh AwsCredentials. Smithy: RequestUploadCredentials output {UploadCredentials: AwsCredentials (sensitive), StorageLocation: S3Location}.
Emulator: repeats the cause-1 defects - account-named bucket, "<buildId>/build.zip" key, EMULATOR-prefixed AccessKeyId.
Why: resuming an interrupted upload targets the wrong bucket/key.
Fix: the RequestUploadCredentials handler must derive the location from the same helper as CreateBuild.
Traces: gamelift-build-lifecycle-and-tags#task#5.

Cause 3 of 5 - CreateScript StorageLocation.
AWS: for a script created by uploading a ZipFile, Script.StorageLocation.Bucket is the GameLift-managed script bucket for the region, named "prod-gamescale-scripts-<region>", and the Key identifies the stored script object (the recording asserts it by shape).
Emulator: Bucket is "gamelift-script-uploads-<region>-<accountId>" and Key is "<scriptId>/script.zip".
Why: clients that read back where GameLift stored the script see a bucket that does not exist in AWS.
Fix: the CreateScript handler's storage-location derivation (same rule as builds, scripts family).
Traces: gamelift-script-lifecycle#task#1.

Cause 4 of 5 - DescribeScript reports the same wrong StorageLocation.
AWS: DescribeScript echoes the stored location - bucket "prod-gamescale-scripts-<region>", key "<awsAccountId>/<scriptId>".
Emulator: returns the cause-3 values (gamelift-script-uploads-<region>-<accountId> and <scriptId>/script.zip).
Why: the wrong location is persisted, not just mis-serialised on create, so every later read is wrong too.
Fix: whatever the CreateScript handler writes into state must be the corrected location; DescribeScript's serializer must not re-derive it differently.
Traces: gamelift-script-lifecycle#task#2.

Cause 5 of 5 - CreateScript in the tagging fixture shows the bucket defect independently.
AWS: as cause 3 - bucket "prod-gamescale-scripts-<region>".
Emulator: "gamelift-script-uploads-<region>-<accountId>".
Why: same as cause 3; confirms the bucket name is a fixed emulator literal rather than a per-request derivation.
Fix: same component as cause 3.
Traces: gamelift-script-tagging-roundtrip#setup#1.

## Evidence
Cause 1: gamelift-build-error-paths#task#2 CreateBuild 200/200 - $.StorageLocation.Bucket expected 'prod-gamescale-builds-us-west-2', got 'gamelift-build-uploads-000000000000'; $.StorageLocation.Key expected '<accountId>/<buildId>', got '<buildId>/build.zip'; $.UploadCredentials.AccessKeyId expected a redacted AWS access key id, got 'EMULATORASEGXT2SMKVA'.
Cause 2: gamelift-build-lifecycle-and-tags#task#5 RequestUploadCredentials 200/200 - same three members diverge the same way.
Cause 3: gamelift-script-lifecycle#task#1 CreateScript 200/200 - $.Script.StorageLocation.Bucket expected 'prod-gamescale-scripts-us-west-2', got 'gamelift-script-uploads-us-west-2-000000000000'; Key shape-asserted and not matched.
Cause 4: gamelift-script-lifecycle#task#2 DescribeScript 200/200 - Bucket as above; Key expected '<accountId>/<scriptId>', got '<scriptId>/script.zip'.
Cause 5: gamelift-script-tagging-roundtrip#setup#1 CreateScript 200/200 - Bucket as above.
(The region in the expected bucket names is the request's region; the account id in the expected keys is the caller's account - those are template slots, not literals from this recording.)

## Steps to Reproduce
1. Replay the wire corpus (simparity-wire-aws-v1).
2. Call CreateBuild with no StorageLocation and inspect StorageLocation.Bucket/Key and UploadCredentials.AccessKeyId; then call RequestUploadCredentials for the same BuildId.
3. Call CreateScript with a ZipFile and inspect Script.StorageLocation, then DescribeScript for the same ScriptId.
4. Compare bucket (prod-gamescale-builds-<region> / prod-gamescale-scripts-<region>), key (<accountId>/<resourceId>) and access key id shape.
5. Re-replay gamelift-build-error-paths#task#2 and every fixture#step in trace_ids to confirm all five causes are closed.
