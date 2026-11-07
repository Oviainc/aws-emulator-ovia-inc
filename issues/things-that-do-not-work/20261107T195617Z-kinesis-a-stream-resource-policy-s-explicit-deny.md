Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.resource-policy-denies-cross-principal-get-records
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis.resource-policy-denies-cross-principal-get-records|run#27|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Commit: d58fa0759c246b06d8eee65f217a3be5842074ab

# kinesis: a stream resource policy's explicit Deny is not enforced on GetRecords — an assumed role the policy denies still reads records with HTTP 200

> _Note: this issue was identified against the code base at commit `d58fa0759c246b06d8eee65f217a3be5842074ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRecords

## Description
One net-new root cause, grounded in a failing behavioral check whose replay key no open issue carries.

=== Cause 1 of 1: a resource-policy Deny is inert on the kinesis data plane ===
Failing check: kinesis.resource-policy-denies-cross-principal-get-records.

AWS: Smithy models PutResourcePolicy { ResourceARN (required, @length 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$), StreamId, Policy (required) } as the operation that attaches a resource-based policy to a stream ARN, and models GetRecords { ShardIterator (required), Limit, StreamARN, StreamId, DryRun } with AccessDeniedException (client error, HTTP 400) among its errors. Recorded behavior: with a resource policy on the stream carrying { "Effect": "Deny", "Principal": {"AWS": [<role arn>]}, "Action": "kinesis:GetRecords", "Resource": <stream arn> }, a caller using credentials assumed into that role — whose IDENTITY policy allows kinesis:GetRecords, GetShardIterator, ListShards and DescribeStreamSummary on * — is refused on GetRecords with HTTP 400, error code AccessDeniedException and the message `User: arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name> is not authorized to perform: kinesis:GetRecords on resource: <stream-arn> with an explicit deny in a resource-based policy`. Its FIXED text is `User: `, ` is not authorized to perform: `, ` on resource: ` and ` with an explicit deny in a resource-based policy`; its VARIABLE parts are the caller's assumed-role ARN (account id, role name and session name from the STS session), the action name, and the stream ARN — all taken from the request and the emulator's state. GetShardIterator, which the Deny does not name, still succeeds; the stream owner's own principal, not targeted by the Deny, keeps reading; and after DeleteResourcePolicy the identical GetRecords call by the same role succeeds again.

Emulator: the Deny has no effect at all. PutResourcePolicy accepts and stores the document (GetResourcePolicy reads it back), the owner reads normally, and the assumed-role client's GetShardIterator + GetRecords keep returning HTTP 200 with the record. The check polls for the refusal for 60 s and every attempt is allowed — `AssertionError: expected AccessDeniedException, got allowed` (check line 130). A stored kinesis resource policy is inert: persisted and readable, never consulted by the data plane.

Why: this is the over-permissive direction on a security contract. A client that verifies a Deny actually blocks a principal passes against the emulator and gets the opposite answer in production; conversely code written against the emulator can be built on the assumption that a resource policy is decorative, so a stream believed to be fenced off is readable by any principal whose identity policy allows the action.

Fix: resource-policy evaluation in the kinesis authorization path — the data-plane handlers (get_records / get_shard_iterator / put_record / put_records in services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/) must consult the policy stored for the stream ARN against the calling principal and raise the modeled AccessDeniedException with AWS's explicit-deny wording before serving records.

This is the SAME defect as Cause 1 of open issue 20261029T200552Z-kinesis-9-net-new-root-causes-resource-policy.md, but that issue's Issue-Key for this check is a Timeout signature (`_retry_until_denied#26|Timeout|time.sleep(interval_s)`) while the check now fails with an AssertionError at run line 130, so the replay report does not treat it as owned and that fixer would never re-replay it. This issue exists to carry that replay key; one fix closes both.

Traces: kinesis.resource-policy-denies-cross-principal-get-records.

## Evidence
From the replayed behavioral suite (simparity-behavioral-aws-v1), 197/325 passed; kinesis 138/210.

kinesis.resource-policy-denies-cross-principal-get-records (check line 130):
  expected: the assumed-role client's GetRecords is refused HTTP 400, Error.Code AccessDeniedException, Error.Message 'User: arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name> is not authorized to perform: kinesis:GetRecords on resource: <stream-arn> with an explicit deny in a resource-based policy' (fixed text as quoted; account id, role name, session name, action and stream ARN come from the request and state)
  actual:   every attempt is ALLOWED — HTTP 200 with the record — for the whole 60 s poll:
            AssertionError: expected AccessDeniedException, got allowed

Positive controls in the same run all behaved as AWS does, so the divergence is isolated to policy evaluation: PutResourcePolicy(Deny) returned success, the stream owner's GetRecords returned the single record it had just put, and the assumed role's GetShardIterator (allowed by its identity policy and not named by the Deny) succeeded.

## Steps to Reproduce
1. kinesis.create_stream(StreamName=<n>, ShardCount=1, StreamModeDetails={'StreamMode':'PROVISIONED'}); wait for StreamStatus ACTIVE and take StreamARN from DescribeStreamSummary.
2. iam.create_role(RoleName=<r>, AssumeRolePolicyDocument=trust on arn:aws:iam::<account>:root for sts:AssumeRole) and iam.put_role_policy(RoleName=<r>, PolicyName='AllowKinesisRead', PolicyDocument=Allow kinesis:GetRecords, kinesis:GetShardIterator, kinesis:DescribeStreamSummary, kinesis:ListShards on '*').
3. kinesis.put_resource_policy(ResourceARN=<stream arn>, Policy={'Version':'2012-10-17','Statement':[{'Sid':'DenyRoleGetRecords','Effect':'Deny','Principal':{'AWS':[<role arn>]},'Action':'kinesis:GetRecords','Resource':<stream arn>}]}).
4. kinesis.put_record(StreamARN=<stream arn>, Data=b'owner-visible-record', PartitionKey='pk'); as the owner, get_shard_iterator(TRIM_HORIZON) + get_records -> the record is returned (as it must be).
5. sts.assume_role(RoleArn=<role arn>, RoleSessionName=<session>); with those credentials call get_shard_iterator(TRIM_HORIZON) (succeeds) then get_records(ShardIterator=...) -> observe HTTP 200 with the record instead of HTTP 400 AccessDeniedException carrying the explicit-deny message above. Retrying for 60 s never produces a refusal.
6. kinesis.delete_resource_policy(ResourceARN=<stream arn>); the identical assumed-role get_records must succeed — proving the earlier refusal was the policy's doing (unreachable today because step 5 never refuses).
