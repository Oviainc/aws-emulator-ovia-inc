Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.op-contract-getAccessKeyLastUsed
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.op-contract-getAccessKeyLastUsed|run#16|AssertionError|assert code == "AccessDenied", f"expected AccessDenied, got {code}"
Issue-Commit: dc4158ff05b54d9987477de08887f7f0381de207

# iam: GetAccessKeyLastUsed confirms a never-issued access key as absent (NoSuchEntity) instead of refusing it AccessDenied

> _Note: this issue was identified against the code base at commit `dc4158ff05b54d9987477de08887f7f0381de207`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessKeyLastUsed

## Description
Cause 1 of 1: a well-formed AccessKeyId that this account never issued is answered with the not-found error of the access-key family instead of IAM's authorization refusal. Failing check: iam.op-contract-getAccessKeyLastUsed.

AWS: Smithy iam GetAccessKeyLastUsed — input AccessKeyId (accessKeyIdType, @required, @length min 16 max 128, @pattern ^[\w]+$); output UserName (existingUserNameType) and AccessKeyLastUsed; the operation declares NO errors of its own (errors: [] in the model), so NoSuchEntity is not among this operation's modeled errors. Recorded behavior only for the refusal itself: an AccessKeyId that satisfies the modeled length and pattern but was never issued by any account does reach IAM, and IAM — rather than confirming that no key with that id exists — refuses the call with error code AccessDenied and the message "User: <caller-arn> is not authorized to perform iam:GetAccessKeyLastUsed on resource: access key <AccessKeyId>". In that message the fixed text is "User: ", " is not authorized to perform iam:GetAccessKeyLastUsed on resource: access key "; the variable parts are the caller's principal ARN (the signing identity, as GetCallerIdentity reports it) and the AccessKeyId exactly as the request carried it. The same check's happy path — GetAccessKeyLastUsed on a real, unused key of a user this account holds resolving UserName and reporting the "N/A" sentinel for ServiceName and Region — passes against the emulator, so only the unknown-key branch diverges.

Emulator: the call is authorized successfully and then answered with error code NoSuchEntity (message "The Access Key with id <AccessKeyId> cannot be found."). Replay detail: `AssertionError: expected AccessDenied, got NoSuchEntity`. In services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/get_access_key_last_used.rs the handler already builds the account-wide authorization target and the refusal subject `format!("access key {}", input.access_key_id)` for an id it holds no key under, but then, when authorization passes, falls through to `GetAccessKeyLastUsedErrorModeled::NoSuchEntityException` for that same id.

Why: a client learns that the id does not exist where AWS deliberately refuses to say so, so the emulator leaks the non-existence of an access key id to any caller; and credential-audit and key-ownership tooling that branches on AccessDenied versus NoSuchEntity (retry/alarm vs. "key is gone, stop tracking it") takes the wrong branch, with a message naming a different failure class than AWS's.

Fix: the unknown-key branch of the GetAccessKeyLastUsed handler (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/get_access_key_last_used.rs) — an AccessKeyId the account holds no key under must be refused as AccessDenied, worded with the subject the handler already computes for it, instead of being reported as NoSuchEntity after a successful authorization.

Note on prior issues: 20261025T000319Z-iam-getaccesskeylastused-stub.md is resolved and asked only that the operation stop answering 501; the operation is implemented and its happy-path assertions pass here, so this is not that stub refusal and not a reopening of it. No open issue names this divergence.

Traces: iam.op-contract-getAccessKeyLastUsed.

## Evidence
Replay verdict: [mismatch] iam.op-contract-getAccessKeyLastUsed
Detail, verbatim: `File "check_iam_op_contract_getAccessKeyLastUsed.py", line 42, in run / assert code == "AccessDenied", f"expected AccessDenied, got {code}" / AssertionError: expected AccessDenied, got NoSuchEntity`

Expected (AWS, as the recorded check grades it):
- error code: AccessDenied
- error message: "User: {caller_arn} is not authorized to perform iam:GetAccessKeyLastUsed on resource: access key {never_issued}", where {caller_arn} is the ARN sts:GetCallerIdentity reports for the signing identity and {never_issued} is the AccessKeyId from the request (the check uses a 20-character id of the form "AKIA" + 16 identical word characters, which satisfies accessKeyIdType's @length 16..128 and @pattern ^[\w]+$).

Actual (emulator):
- error code: NoSuchEntity, with the access-key-family not-found wording "The Access Key with id <AccessKeyId> cannot be found." (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/get_access_key_last_used.rs lines 276-288), so the message assertion was never reached.

Unaffected in the same check (passing): GetAccessKeyLastUsed on a real key returns UserName equal to the owning user and AccessKeyLastUsed with ServiceName and Region both "N/A" for a key that has never signed a call.

## Steps to Reproduce
1. Boot the emulator and sign as any principal allowed iam:GetAccessKeyLastUsed.
2. CreateUser, then CreateAccessKey for that user; call GetAccessKeyLastUsed with the returned AccessKeyId and observe UserName plus AccessKeyLastUsed ServiceName/Region = "N/A" (this part already matches AWS).
3. Call GetAccessKeyLastUsed with a well-formed AccessKeyId no account ever issued, e.g. "AKIA" followed by 16 word characters (20 chars total, within accessKeyIdType @length 16..128 and matching @pattern ^[\w]+$).
4. Observe the response error code is NoSuchEntity with message "The Access Key with id <AccessKeyId> cannot be found." AWS instead refuses with code AccessDenied and message "User: <caller-arn> is not authorized to perform iam:GetAccessKeyLastUsed on resource: access key <AccessKeyId>".
5. Or replay iam.op-contract-getAccessKeyLastUsed.
