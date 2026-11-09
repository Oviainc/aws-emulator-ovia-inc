Issue-Source: trace-sweep-divergence
Issue-Trace: efs-file-system-policy-workflow
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: efs-file-system-policy-workflow
Issue-Shape-Id: com.amazonaws.efs#MagnolioAPIService_v20150201
Issue-Key: elasticfilesystem|PutFileSystemPolicy|4xx->4xx|$.ErrorCode
Issue-Commit: e3ea9c3d18c5ac215ea88ebad8a0c22355767e25

# EFS PutFileSystemPolicy serializes InvalidPolicyException with an ErrorCode member AWS omits

> _Note: this issue was identified against the code base at commit `e3ea9c3d18c5ac215ea88ebad8a0c22355767e25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutFileSystemPolicy

## Description
PutFileSystemPolicy's InvalidPolicyException refusal carries an extra body member: the emulator populates the optional ErrorCode member, where AWS serializes the exception without it. One cause.

Cause 1 of 1: the InvalidPolicyException body carries ErrorCode where AWS sends none (fixture efs-file-system-policy-workflow, step task#6).

AWS: Smithy (com.amazonaws.efs#MagnolioAPIService_v20150201): PutFileSystemPolicy is PUT /2015-02-01/file-systems/{FileSystemId}/policy, success code 200; its modeled refusals are BadRequest (400), FileSystemNotFound (404), IncorrectFileSystemLifeCycleState (409), InternalServerError (500) and InvalidPolicyException (400). InvalidPolicyException is documented "Returned if the FileSystemPolicy is malformed or contains an error such as a parameter value that is not valid or a missing required parameter. Returned in the case of a policy lockout safety check error." Its members are ErrorCode (target ErrorCode, @length min 1) and Message (target ErrorMessage) — and unlike sibling BadRequest, whose ErrorCode is @required, InvalidPolicyException's ErrorCode is NOT required. The recorded AWS response for a malformed/lockout-failing policy is HTTP 400 with the error shape named in the x-amzn-errortype header and a body whose ErrorCode member is absent; the Message member carries AWS's sentence for the policy-validation failure (its fixed text is AWS's, with the variable parts — the policy element or parameter at fault — taken from the submitted Policy document) and the emulator already matches it, so Message is not part of this divergence. The requirement here is the member set of the serialized error, not any one recorded value.

Emulator: answers the right class and status (400, same error shape) but serializes an additional body member ErrorCode with the string "InvalidPolicyException", i.e. it echoes the exception's shape name into an optional member AWS leaves out. Replay reports $.ErrorCode type_mismatch (expected None, got 'InvalidPolicyException') on an otherwise matching 400 response.

Why: a client that distinguishes EFS refusals by the presence and value of the body's ErrorCode — the pattern EFS forces, since several of its errors share HTTP 400 — sees an ErrorCode on a response AWS ships without one. Code written against AWS that treats "ErrorCode present" as the signal for a BadRequest-class refusal (where ErrorCode is required) misroutes this policy-validation failure, and any assertion that the InvalidPolicyException body contains only Message fails against the emulator. The same defect family was already fixed for CreateAccessPoint (20261023T023939Z-efs-createaccesspoint-accesspointalreadyexists-is-serialized-with-an-empty) — an optional error member populated where AWS omits it — so this is that serializer habit surviving in the file-system-policy path.

Fix: the error serializer for the PutFileSystemPolicy handler in the efs service crate — emit InvalidPolicyException with Message only and omit the optional ErrorCode member, instead of defaulting it to the exception's shape name. Do not blanket-apply the omission: BadRequest and the other EFS errors whose ErrorCode is @required must keep carrying it.

No open issue covers this. The open efs issue 20261026T232755Z-efs-lambda-putfilesystempolicy-answers-501-notimplemented-aborting-the concerns PutFileSystemPolicy answering 501 NotImplemented (the operation now answers 400 with the right class, so that is a different, earlier state), and 20261029T195311Z-elasticfilesystem-efs-mount-targets-ignore-ec2-subnet-state owns the CreateMountTarget/DescribeMountTargets/DeleteFileSystem clusters in this same replay, not this one.

Traces: efs-file-system-policy-workflow#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 99 steps, 85 passed, score 85.9%, 5 divergence clusters — all elasticfilesystem; the other four are owned by open issue 20261029T195311Z-elasticfilesystem-efs-mount-targets-ignore-ec2-subnet-state).

Cause 1 — fixture efs-file-system-policy-workflow, step task#6, PutFileSystemPolicy:
  expected_status 400, actual_status 400 (status and error class agree)
  $.ErrorCode: type_mismatch (expected None, got 'InvalidPolicyException')
  divergence_key: service=elasticfilesystem|PutFileSystemPolicy|status=400->400|keys=$.ErrorCode
  verdict: mismatch; stub_path: (none); owned_by: (none)

Expected (AWS, recorded): HTTP 400, x-amzn-errortype naming InvalidPolicyException, body with Message and NO ErrorCode member.
Actual (emulator): HTTP 400, same error shape, body additionally carrying ErrorCode="InvalidPolicyException".

Smithy evidence: InvalidPolicyException members = {ErrorCode (@length min 1, optional), Message}; sibling BadRequest members = {ErrorCode @required, Message}. The optionality difference is modeled, and the recording exercises it.

## Steps to Reproduce
1. git clone the repo (branch under test) and build/boot the emulator from that checkout.
2. Replay trace set simparity-wire-aws-v1 against it.
3. Replay fixture efs-file-system-policy-workflow, step task#6: create a file system, then PUT /2015-02-01/file-systems/{FileSystemId}/policy with a Policy document that fails EFS policy validation (malformed/lockout-failing policy, BypassPolicyLockoutSafetyCheck left at its default false).
4. Expect HTTP 400 with the x-amzn-errortype header naming InvalidPolicyException and a body containing only Message (AWS's policy-validation sentence) — no ErrorCode member.
5. Observe HTTP 400 whose body additionally contains "ErrorCode": "InvalidPolicyException".
6. Confirm the fix by re-replaying efs-file-system-policy-workflow#task#6 and checking that $.ErrorCode is absent while the status, error shape and Message are unchanged, and that a BadRequest-class refusal on the same operation still carries its @required ErrorCode.
