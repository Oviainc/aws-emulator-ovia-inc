Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__config__s3.config-delivery-denied-when-bucket-policy-is-removed
Issue-Key: cloudformation__config__s3.config-delivery-denied-when-bucket-policy-is-removed|run#20|AssertionError|assert bad_info.get("lastStatus") == "FAILURE", (
Issue-Commit: 2d6e0284801305ae3aa8ad9d3ca49cea550d31a4

# cloudformation__config__s3: snapshot delivery reports SUCCESS after the delivery bucket's policy is deleted — the S3 write is never authorized, so a denied delivery is indistinguishable from a successful one

> _Note: this issue was identified against the code base at commit `2d6e0284801305ae3aa8ad9d3ca49cea550d31a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDeliveryChannelStatus

## Description
ONE root cause. ONE CAUSE must be closed.

Cause 1 of 1: Config's snapshot-delivery pipeline never consults the delivery bucket's authorization, so configSnapshotDeliveryInfo settles on lastStatus=SUCCESS even when the bucket policy that grants config.amazonaws.com was deleted out from under the established channel.

AWS: recorded behavior only, with model grounding. Recorded flow: with a running configuration recorder and a delivery channel whose bucket policy grants config.amazonaws.com s3:GetBucketAcl, s3:ListBucket and s3:PutObject (bucket-owner-full-control), DeliverConfigSnapshot is accepted and returns a configSnapshotId; DescribeDeliveryChannelStatus's DeliveryChannelsStatus[0].configSnapshotDeliveryInfo then settles on a TERMINAL outcome that reflects whether the S3 write was actually permitted. After DeleteBucketPolicy on that bucket (GetBucketPolicy reporting NoSuchBucketPolicy), the delivery named by the returned configSnapshotId settles on lastStatus=FAILURE with a non-empty lastErrorCode and NO object written under the bucket whose key contains that snapshot id. Restoring only that bucket policy and issuing one further DeliverConfigSnapshot settles on lastStatus=SUCCESS with lastSuccessfulTime strictly after the failed attempt's lastAttemptTime and a readable object whose key contains the new snapshot id; the denied snapshot's id never appears as a key. Smithy (com-amazonaws-configservice-starlingdoveservice.json): DeliverConfigSnapshot models only client/400 errors (NoSuchDeliveryChannelException, NoAvailableConfigurationRecorderException, NoRunningConfigurationRecorderException), i.e. the delivery outcome is never raised to the caller — configSnapshotDeliveryInfo's lastStatus/lastErrorCode is the only place it is reported.

Emulator: delivery now reaches a terminal state, but it is always SUCCESS. With the bucket policy deleted and GetBucketPolicy confirming NoSuchBucketPolicy, the polled configSnapshotDeliveryInfo is {'lastStatus': 'SUCCESS', 'lastAttemptTime': <ts>, 'lastSuccessfulTime': <ts>, 'nextDeliveryTime': <ts>} — lastStatus is SUCCESS instead of FAILURE and no lastErrorCode member is present at all. The emulator therefore reports a successful delivery for a write that S3 would refuse: the delivery path either does not attempt the PutObject against the bucket's effective policy, or ignores its authorization result when deciding lastStatus. (Timestamps shown are per-run values, not requirements.)

Why: a client that verifies its Config delivery wiring, or that alarms on a delivery denied by an S3 policy change, is told everything is fine. An operator who removes or mis-edits the delivery bucket policy sees lastStatus=SUCCESS and no lastErrorCode and concludes snapshots are still landing, while on AWS delivery would be failing and reporting an error code. Any runbook keyed on FAILURE/lastErrorCode is dead code against the emulator.

Fix: the snapshot-delivery pipeline and the status surface that feeds it in Config — the delivery task behind the channel state (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/deliver_config_snapshot.rs and the channel state written by src/ops/put_delivery_channel.rs) must perform/authorize the S3 write against the target bucket's effective bucket policy and propagate the refusal into src/ops/describe_delivery_channel_status.rs as lastStatus=FAILURE plus a lastErrorCode, with no object written.

Related open issue, named for overlap and NOT relied on as cover: 20261025T014224Z-cloudformation-config-s3-snapshot-delivery-never-reaches-a.md described the earlier shape of this check's failure (delivery never reached a terminal lastStatus at all). Delivery is now terminal, so that symptom is gone; what remains is that the terminal verdict ignores the bucket policy. 20261029T065729Z-cloudformation-config-s3-a-cloudformation-provisioned-config-delivery-dcb3f1.md and 20261107T193246Z-config-4-net-new-causes-custom-lambda-and.md (PutDeliveryChannel's bucket-policy pre-check) are adjacent but cover different legs; neither carries this check id as a current replay key.

Traces: cloudformation__config__s3.config-delivery-denied-when-bucket-policy-is-removed.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (emulator serving, bootstrap_signal false; 117/177 checks passed).

[mismatch] cloudformation__config__s3.config-delivery-denied-when-bucket-policy-is-removed
  File "check_cloudformation__config__s3_config_delivery_denied_when_bucket_policy_is_removed.py", line 684, in run
    assert bad_info.get("lastStatus") == "FAILURE", (
  AssertionError: deleting the delivery bucket's policy must make snapshot delivery FAIL, got {'lastStatus': 'SUCCESS', 'lastAttemptTime': datetime.datetime(2026, 11, 7, 21, 33, 6, 690297, tzinfo=tzlocal()), 'lastSuccessfulTime': datetime.datetime(2026, 11, 7, 21, 33, 7, 191134, tzinfo=tzlocal()), 'nextDeliveryTime': datetime.datetime(2026, 11, 8, 21, 33, 7, 191134, tzinfo=tzlocal())}

What the check establishes before that assertion (from the replayed source):
  - creates a delivery bucket carrying the AWSConfigBucketPermissionsCheck (s3:GetBucketAcl), AWSConfigBucketExistenceCheck (s3:ListBucket) and AWSConfigBucketDelivery (s3:PutObject with s3:x-amz-acl=bucket-owner-full-control) statements, a configuration recorder and a delivery channel at that bucket, then starts the recorder — PutDeliveryChannel would have been refused InsufficientDeliveryPolicyException without the policy, so the policy is provably in place;
  - s3.delete_bucket_policy(Bucket=bucket) and polls GetBucketPolicy until it reports NoSuchBucketPolicy (confirmed: the poll returns before the failing assertion);
  - cfg.deliver_config_snapshot(deliveryChannelName=channel) -> configSnapshotId (accepted, as AWS does);
  - polls describe_delivery_channel_status(...)['DeliveryChannelsStatus'][0]['configSnapshotDeliveryInfo'] until lastStatus is SUCCESS or FAILURE — this now returns, with SUCCESS.

The assertions beyond it that the emulator cannot satisfy in this state: a non-empty lastErrorCode on the failed delivery, no bucket key containing the denied snapshot id, and (after PutBucketPolicy restores only that policy) a second delivery reaching SUCCESS with lastSuccessfulTime > the failed attempt's lastAttemptTime and a readable object naming the new snapshot id while the denied id is still absent.

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): DeliverConfigSnapshot -> DeliverConfigSnapshotResponse{configSnapshotId}; its modeled errors are all client/400 pre-conditions, so the delivery result is observable only through DescribeDeliveryChannelStatus.configSnapshotDeliveryInfo.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator.
2. Replay simparity-behavioral-aws-v1 and re-replay cloudformation__config__s3.config-delivery-denied-when-bucket-policy-is-removed.
3. Manually: s3 create-bucket; s3 put-bucket-policy granting config.amazonaws.com s3:GetBucketAcl + s3:ListBucket on the bucket and s3:PutObject on <bucket>/* with condition s3:x-amz-acl=bucket-owner-full-control; config put-configuration-recorder with a role trusting config.amazonaws.com; config put-delivery-channel pointing at that bucket; config start-configuration-recorder.
4. s3 delete-bucket-policy --bucket <bucket>; confirm s3 get-bucket-policy now fails with NoSuchBucketPolicy.
5. config deliver-config-snapshot --delivery-channel-name <channel>; note the returned configSnapshotId.
6. Poll config describe-delivery-channel-status --delivery-channel-names <channel>: configSnapshotDeliveryInfo reports lastStatus=SUCCESS with no lastErrorCode. AWS reports lastStatus=FAILURE with a non-empty lastErrorCode and writes no object naming that snapshot id.
7. s3 put-bucket-policy to restore the same policy and repeat steps 5-6: the emulator's verdict is identical to the denied leg, so the two legs are indistinguishable.
