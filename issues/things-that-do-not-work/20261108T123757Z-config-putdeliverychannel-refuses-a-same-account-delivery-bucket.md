Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.resource-config-unicode-and-binary-blob-round-trip
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.resource-config-unicode-and-binary-blob-round-trip|_poll_until#3|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last={result!r})")
Issue-Commit: a120551e8f3d3b0acb313602840003b2695dec8d

# config: PutDeliveryChannel refuses a same-account delivery bucket whose Config permissions come from the recorder role's IAM policy — a bucket with no bucket policy is denied forever (InsufficientDeliveryPolicyException)

> _Note: this issue was identified against the code base at commit `a120551e8f3d3b0acb313602840003b2695dec8d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDeliveryChannel

## Description
One net-new root cause in the AWS Config emulator, surfaced by a check the suite lists under `### config` that is marked neither ALREADY FILED nor OWNED.

Cause 1 of 1: PutDeliveryChannel's delivery-permission pre-check reads ONLY the target bucket's S3 bucket policy and treats "this bucket carries no policy" as an outright denial, so a delivery channel whose Config delivery permissions are granted through the configuration recorder's IAM role is refused permanently and no channel can ever be created.

AWS: Smithy (com.amazonaws.configservice#StarlingDoveService) models PutDeliveryChannel(DeliveryChannel required) with the client errors InsufficientDeliveryPolicyException (HTTP 400, "Your Amazon S3 bucket policy does not allow Config to write to it"), NoSuchBucketException, NoAvailableConfigurationRecorderException, InvalidDeliveryChannelNameException, InvalidS3KeyPrefixException, InvalidS3KmsKeyArnException, InvalidSNSTopicARNException and MaxNumberOfDeliveryChannelsExceededException; the model states nothing about WHICH policy document grants the permission. Recorded behavior only for that part: the recording provisions a same-account delivery bucket with NO bucket policy at all and grants the Config permissions on the recorder's IAM role instead — an inline role policy allowing s3:GetBucketAcl on arn:aws:s3:::<bucket> and s3:PutObject/s3:PutObjectAcl on arn:aws:s3:::<bucket>/AWSLogs/<accountId>/* (conditioned StringLike s3:x-amz-acl = bucket-owner-full-control), plus the AWS_ConfigRole managed policy — and PutDeliveryChannel({name,s3BucketName}) is ACCEPTED with HTTP 200 and no body. The recording retries only while IAM is still propagating (it tolerates a transient 400 InsufficientDeliveryPolicyException for up to 90 s and expects the call to succeed within that window), i.e. on AWS this setup is a sufficient delivery policy and the refusal is at most transient. StartConfigurationRecorder then succeeds instead of answering NoAvailableDeliveryChannelException.

Emulator: the identical PutDeliveryChannel is refused with 400 InsufficientDeliveryPolicyException on EVERY attempt for the whole 90 s window, so the channel is never created and the recorder can never be started. The refusal is grounded on the BUCKET POLICY alone: services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_delivery_channel.rs calls crate::state::delivery_authorization(...), and services/.../src/state/delivery_authorization.rs resolves the decision by issuing S3 GetBucketPolicy for the named bucket and mapping the `NoSuchBucketPolicy` answer to DeliveryAuthorization::Denied(DELIVERY_CHECKS[0]) (DeliveryCheck::BucketAcl) — see its doc comment "A bucket carrying no policy grants Config nothing" and its unit test `a bucket carrying no policy denies the first check Config makes`. Nothing in that path consults the configuration recorder's IAM role or its attached/inline policies, so a grant that lives on the role can never satisfy it. The message produced is the ACL-denied wording `Insufficient delivery policy to s3 bucket: <s3BucketName>, unable to get s3 bucket acl.` — fixed text `Insufficient delivery policy to s3 bucket: ` … `, unable to get s3 bucket acl.` with the only variable part the s3BucketName taken from the request; the status/code/message wording themselves are right for a genuinely policy-denied bucket, the defect is that this bucket is not denied on AWS.

Why: a client that provisions Config the way AWS's own role-based setup does — recorder role with s3:GetBucketAcl + s3:PutObject on the delivery path, no bucket policy on a same-account bucket — can never create a delivery channel, so it can never start the recorder and every downstream Config behaviour (resource discovery, configuration history, PutResourceConfig, snapshot delivery) is unreachable. Worse, the refusal is PERMANENT yet carries the same error code AWS uses for a transient IAM-propagation window, so a client's documented retry loop spins until its own timeout and the failure looks like eventual consistency rather than a rejected configuration.

Fix: PutDeliveryChannel's delivery-permission validation — the authorization decision in the Config state module that backs it (crate::state::delivery_authorization / denied_delivery_check). It must treat the bucket policy as ONE source of the grant: for a bucket in the emulated account, the permission must also be satisfiable from the configuration recorder's IAM role (its attached managed policies and inline role policies, evaluated for s3:GetBucketAcl on the bucket ARN and s3:PutObject on the writability-check key with the same condition context, including StringLike on s3:x-amz-acl), and the absence of a bucket policy must not by itself be a denial for a same-account bucket. Only when no source grants the call may InsufficientDeliveryPolicyException be raised. Context, not a duplicate: open issue 20261107T193246Z-config-4-net-new-causes-custom-lambda-and.md (Cause 3) asked for the missing bucket-policy pre-check and does not carry this check's replay key; this finding is the over-strict other side of that pre-check as it now stands, so fixing one must not re-break the other. Open issue 20261022T190340Z-config-putconfigrule-and-putdeliverychannel-are-overly-permissive-inconsistent.md covers unrelated PutDeliveryChannel permissiveness (malformed ARNs, unvalidated bucket name).

Traces: config.resource-config-unicode-and-binary-blob-round-trip (the refusal happens in its _ensure_config_recorder_running setup, so the check's PutResourceConfig / GetResourceConfigHistory / BatchGetResourceConfig assertions are never reached).

## Evidence
From the behavioral suite replay (123/177 passed). Failing check `config.resource-config-unicode-and-binary-blob-round-trip` [mismatch]:

  File "check_config_resource_config_unicode_and_binary_blob_round_trip.py", line 247, in _ensure_config_recorder_running
    _poll_until(_try_put_channel, lambda ok: ok, timeout_s=90.0, interval_s=5.0)
  File "check_config_resource_config_unicode_and_binary_blob_round_trip.py", line 92, in _poll_until
    raise AssertionError(f"condition not met within {timeout_s}s (last={result!r})")
  AssertionError: condition not met within 90.0s (last=False)

The helper is diagnostic: `_try_put_channel` returns True on an accepted PutDeliveryChannel, returns False ONLY for error code `InsufficientDeliveryPolicyException`, and re-raises any other ClientError. `last=False` therefore means every one of the ~18 attempts over 90 s came back 400 InsufficientDeliveryPolicyException.

Expected (AWS, as recorded): PutDeliveryChannel(DeliveryChannel={name:<name>, s3BucketName:<bucket>}) -> HTTP 200, channel created, and the subsequent StartConfigurationRecorder succeeds. Setup in the recording: s3:CreateBucket (no PutBucketPolicy anywhere), iam:CreateRole trusting config.amazonaws.com, iam:AttachRolePolicy arn:aws:iam::aws:policy/service-role/AWS_ConfigRole, iam:PutRolePolicy granting s3:PutObject/s3:PutObjectAcl on arn:aws:s3:::<bucket>/AWSLogs/<accountId>/* (Condition StringLike s3:x-amz-acl=bucket-owner-full-control) and s3:GetBucketAcl on arn:aws:s3:::<bucket>, then PutConfigurationRecorder with that roleARN.

Actual (emulator): HTTP 400 `InsufficientDeliveryPolicyException`, Message = "Insufficient delivery policy to s3 bucket: <s3BucketName>, unable to get s3 bucket acl." on every attempt, permanently. Code path: ops/put_delivery_channel.rs -> state::delivery_authorization(); state/delivery_authorization.rs maps S3 GetBucketPolicy's `NoSuchBucketPolicy` to Denied(DeliveryCheck::BucketAcl) and never consults the recorder role's IAM policies.

Bucket names, role names, resource ids and the account id in the message are per-run generated values and are not part of the requirement; only the status, the error code, the message's fixed text and the request-derived s3BucketName are.

Control: the suite's other Config checks that write the documented BUCKET POLICY before PutDeliveryChannel do get the channel accepted, which isolates the defect to the role-granted (no-bucket-policy) path.

## Steps to Reproduce
1. CreateBucket <bucket> in the request Region and do NOT put any bucket policy on it.
2. iam:CreateRole <role> with a trust policy allowing sts:AssumeRole to Service config.amazonaws.com; AttachRolePolicy arn:aws:iam::aws:policy/service-role/AWS_ConfigRole; PutRolePolicy with Allow s3:PutObject,s3:PutObjectAcl on arn:aws:s3:::<bucket>/AWSLogs/<accountId>/* (Condition StringLike s3:x-amz-acl = bucket-owner-full-control) and Allow s3:GetBucketAcl on arn:aws:s3:::<bucket>.
3. config:PutConfigurationRecorder(ConfigurationRecorder={name:<n>, roleARN:<role arn>, recordingGroup:{allSupported:true, includeGlobalResourceTypes:false}}) — accepted.
4. config:PutDeliveryChannel(DeliveryChannel={name:<n>, s3BucketName:<bucket>}).
   Expected (AWS): HTTP 200; DescribeDeliveryChannels then lists the channel and StartConfigurationRecorder(<n>) succeeds.
   Actual (emulator): HTTP 400 InsufficientDeliveryPolicyException "Insufficient delivery policy to s3 bucket: <bucket>, unable to get s3 bucket acl." — repeated indefinitely (retrying for 90 s never succeeds), so no delivery channel exists and StartConfigurationRecorder keeps answering NoAvailableDeliveryChannelException.
