Issue-Source: trace-sweep-divergence
Issue-Trace: gamelift-build-error-paths
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gamelift-build-error-paths gamelift-build-lifecycle-and-tags
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift|CreateBuild|2xx->2xx|$.UploadCredentials.AccessKeyId
Issue-Key: gamelift|RequestUploadCredentials|2xx->2xx|$.UploadCredentials.AccessKeyId
Issue-Commit: 82a9e97b43fb811c245308aabae5044dc3bd73ae

# GameLift upload credentials AccessKeyId is not an AWS temporary access key id (EMULATOR-prefixed literal)

> _Note: this issue was identified against the code base at commit `82a9e97b43fb811c245308aabae5044dc3bd73ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBuild

## Description
Two independent causes, one shared fault: every code path that mints build-upload credentials stamps the AccessKeyId with an emulator-specific "EMULATOR" prefix instead of an AWS temporary-credential access key id, so the credential pair the emulator hands back is not shaped like an AWS credential on either operation that returns one. A fixer must close BOTH causes before submitting.

Cause 1 of 2 — CreateBuild.UploadCredentials.AccessKeyId.
AWS: when CreateBuild is called without a customer StorageLocation, GameLift mints a temporary session for its build-upload role and returns it as UploadCredentials. Smithy (com.amazonaws.gamelift#GameLift): CreateBuild output {Build, UploadCredentials: AwsCredentials (@sensitive), StorageLocation: S3Location}; AwsCredentials.AccessKeyId targets NonEmptyString (@length min 1) and is documented as "The access key ID that identifies the temporary security credentials", with the shape doc stating the credentials "are temporary and valid for a limited time span". The model does not constrain the lexical form beyond non-empty, so the form is recorded behavior only: the recording carries the value as the shape placeholder [REDACTED:aws-access-key], i.e. the harness asserts it by SHAPE, and an AWS temporary (STS-issued) access key id is 20 uppercase alphanumeric characters beginning with the ASIA prefix. Status is 200 on both the recording and the emulator; there is no error code or message involved in this divergence.
Emulator: returns HTTP 200 but with an AccessKeyId of the form "EMULATOR" + 12 uppercase alphanumeric characters — observed 'EMULATORASJHW4MMREHZ' — which fails the aws-access-key shape assertion. The 20-character length is right; the leading 8 characters are a hard-coded emulator literal where AWS puts the ASIA temporary-credential prefix followed by key material. (The specific 12 trailing characters are this run's generated value and are NOT part of the requirement; the EMULATOR prefix is.)
Why: a client that validates the returned credential before using it — a prefix/charset check, an STS-vs-long-term discrimination on the ASIA/AKIA prefix, or any SDK/credential-provider that classifies a key id by prefix — rejects or misclassifies credentials AWS would accept, and a test asserting the credentials are temporary sees a key id in no AWS family at all. It also leaks the fact that the credential is synthetic into a response field customers log and assert on.
Fix: the credential-minting helper behind the CreateBuild handler (the component that constructs the AwsCredentials returned in the upload-credentials path) must emit an access key id in the AWS temporary-credential form (ASIA prefix + 16 further uppercase alphanumeric characters, 20 total) rather than prefixing a literal.
Traces: gamelift-build-error-paths#task#2 (cluster also covers gamelift-build-lifecycle-and-tags#task#1).

Cause 2 of 2 — RequestUploadCredentials.UploadCredentials.AccessKeyId.
AWS: RequestUploadCredentials re-issues a FRESH temporary session for an existing build. Smithy: RequestUploadCredentials input {BuildId: BuildIdOrArn, required}, output {UploadCredentials: AwsCredentials (@sensitive), StorageLocation: S3Location}; same AwsCredentials shape and same ASIA-prefixed temporary access key id form as cause 1 (recorded behavior only, asserted by shape as [REDACTED:aws-access-key]). Status 200.
Emulator: HTTP 200 with the same defective form — observed 'EMULATORASYXUDS2MU4G', again "EMULATOR" + 12 characters. The re-issue path reproduces the defect independently of CreateBuild, which shows the prefix comes from the shared minting code rather than from one handler.
Why: the documented flow for resuming or retrying a build upload is to call RequestUploadCredentials and sign with what it returns; a client that validates the refreshed key id the same way it validated the original is broken on the same check, so the defect is not avoidable by using the refresh path.
Fix: same component as cause 1 — RequestUploadCredentials must obtain its credentials from the corrected shared minting helper, not re-derive a prefixed literal.
Traces: gamelift-build-lifecycle-and-tags#task#5.

RELATED (does NOT cover this): the RESOLVED issue 20261029T183132Z-gamelift-build-and-script-upload-storagelocation-and-uploadcredentials.md previously bundled the StorageLocation bucket/key naming together with this AccessKeyId shape. The bucket and key halves are now correct in the replay (they no longer diverge), but the AccessKeyId half is still EMULATOR-prefixed on both operations, so that closed issue left this cause unfixed and cannot cover it. The open issue 20261029T185309Z-gamelift-requestuploadcredentials-mints-credentials-that-are-not-upload.md is about what the minted session is PERMITTED to do (no upload-only session policy) and says nothing about the key id's form; fixing the session policy leaves these two steps diverging.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false).

Cause 1: gamelift-build-error-paths#task#2 — CreateBuild, expected status 200, actual 200. Divergence: $.UploadCredentials.AccessKeyId value_mismatch — expected '[REDACTED:aws-access-key]' (shape assertion: 20-char uppercase alphanumeric AWS temporary access key id, ASIA prefix), got 'EMULATORASJHW4MMREHZ'. Cluster affects gamelift-build-error-paths#task#2 and gamelift-build-lifecycle-and-tags#task#1 (affected_count 2).

Cause 2: gamelift-build-lifecycle-and-tags#task#5 — RequestUploadCredentials, expected status 200, actual 200. Divergence: $.UploadCredentials.AccessKeyId value_mismatch — expected '[REDACTED:aws-access-key]', got 'EMULATORASYXUDS2MU4G'.

The fixed part of each value ('EMULATOR' + a 20-character total length) is what diverges; the 12 trailing generated characters and the specific build ids in these fixtures are this run's values filling the template, not requirements. Both steps answered 200 with otherwise-matching bodies (StorageLocation.Bucket and .Key no longer diverge), so AccessKeyId is the only remaining divergent member in each.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Cause 1: call CreateBuild(Name=<unique>, Version, OperatingSystem, ServerSdkVersion) with NO StorageLocation and no ZipFile; read $.UploadCredentials.AccessKeyId from the 200 response. It is 'EMULATOR' + 12 characters; AWS returns a 20-character uppercase alphanumeric id with the ASIA temporary-credential prefix.
4. Cause 2: call RequestUploadCredentials(BuildId=<the build id from step 3>) and read $.UploadCredentials.AccessKeyId from its 200 response — same EMULATOR-prefixed form.
5. Assert the key id matches the AWS temporary access key id form (^ASIA[A-Z0-9]{16}$, length 20) on both responses.
6. Re-replay gamelift-build-error-paths#task#2 AND gamelift-build-lifecycle-and-tags#task#5 to confirm both causes are closed.
