Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__config__s3.deploy-config-delivery-channel-by-cloudformation-delivers-a-snapshot-containing-the-recorded-item
Issue-Key: cloudformation__config__s3.deploy-config-delivery-channel-by-cloudformation-delivers-a-snapshot-containing-the-recorded-item|run#25|KeyError|observed_arn = items[-1]["arn"]
Issue-Commit: c7b32a251ff44cc6c28a05b1a335573ec2c90dc2

# config GetResourceConfigHistory: every configuration item omits `arn`, so a recorded resource's ARN is never reported (1 cause)

> _Note: this issue was identified against the code base at commit `c7b32a251ff44cc6c28a05b1a335573ec2c90dc2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceConfigHistory

## Description
ONE root cause. ONE CAUSE must be closed.

Cause 1 of 1: GetResourceConfigHistory returns configuration items whose `arn` member is always absent, so the ARN of the resource Config recorded is never observable.

AWS: Smithy (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): GetResourceConfigHistory -> GetResourceConfigHistoryResponse{configurationItems: ConfigurationItemList, nextToken}; ConfigurationItem member `arn` targets `ARN` and is documented "Amazon Resource Name (ARN) associated with the resource." For a resource a customer-managed recorder has discovered, Config populates that member on every configuration item it reports: for `resourceType=AWS::S3::Bucket` with `resourceId=<bucket>` the value is the bucket's own ARN, i.e. the ARN whose resource segment is the recorded `resourceId` (`arn:<partition>:s3:::<bucket>`), which is why a client can assert `item["arn"].endswith(resourceId)`. The member is the only place in the configuration item that carries the resource's ARN — `resourceId`/`resourceName` carry only the bare id/name. ARN partition, region and account are environment values and are not part of the requirement; the requirement is that `arn` is present and is the ARN of the recorded resource.

Emulator: `arn` is hardcoded absent. `DiscoveredResourceEntry::to_base_configuration_item` sets `arn: None` (services/service-com-amazonaws-configservice-starlingdoveservice/src/state.rs:2260, with the doc comment at :2233-2238 stating the member is deliberately left unset), and `to_configuration_item` copies it through as `arn: base.arn` (src/state.rs:2319), which is what `GetResourceConfigHistory` serializes via `entry.configuration_items(...)` (src/ops/get_resource_config_history.rs:313-314, 352-355). The replayed check reaches a populated history — the recorder is running, the bucket IS discovered, the call returns 200 with a non-empty `configurationItems` — and then dies on the member itself: `observed_arn = items[-1]["arn"]` -> `KeyError: 'arn'`. The same projection feeds the aggregate configuration-item read paths, so they report no ARN either.

Why: a client that resolves a recorded resource to its ARN — to tag it, to evaluate a resource-based policy against it, to correlate a configuration item with a CloudTrail/EventBridge record, or simply to render Config history — gets a KeyError/absent field instead of the ARN. Every such consumer breaks, and because the member is simply missing rather than wrong, there is no fallback: nothing else in the item yields the resource's ARN.

Fix: the configuration-item projection in Config's state — `DiscoveredResourceEntry::to_base_configuration_item` (and therefore `to_configuration_item` and `configuration_items`), which must derive and report the recorded resource's ARN from its `resourceType`/`resourceId` (and the request's partition/region/account scope) instead of hardcoding `None`; `GetResourceConfigHistory`'s handler itself needs no change beyond consuming the populated member.

Related open issue, named for overlap but NOT relied on as cover (it does not carry this check id): 20261025T014224Z-cloudformation-config-s3-snapshot-delivery-never-reaches-a.md covers DescribeDeliveryChannelStatus never reaching a terminal `configSnapshotDeliveryInfo.lastStatus` for the OTHER check in this group. This cause is in the configuration-item read path and is reached and fails BEFORE any delivery polling.

Traces: cloudformation__config__s3.deploy-config-delivery-channel-by-cloudformation-delivers-a-snapshot-containing-the-recorded-item.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (emulator serving; bootstrap_signal false; 177/749 checks passed).

[error] cloudformation__config__s3.deploy-config-delivery-channel-by-cloudformation-delivers-a-snapshot-containing-the-recorded-item
  Error: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
  File "check_cloudformation__config__s3_deploy_config_delivery_channel_by_cloudformation_delivers_a_snapshot_containing_the_recorded_item.py", line 502, in run
    observed_arn = items[-1]["arn"]
  KeyError: 'arn'

Expected (AWS): the last configuration item for the discovered bucket carries `arn`, and `arn.endswith(<bucket name>)` holds — the check's next line is `assert observed_arn.endswith(observed)`.
Actual (emulator): the response's configuration items contain no `arn` key at all; botocore raises KeyError because the member was never serialized.

How the check gets there (from the replayed source):
  - deploys the Config delivery stack by CloudFormation (bucket + bucket policy granting config.amazonaws.com GetBucketAcl/ListBucket/PutObject + recorder role), asserts CREATE_COMPLETE and that the stack's ConfigBucket physical id matches the output;
  - PutConfigurationRecorder, PutDeliveryChannel at that bucket, StartConfigurationRecorder (polled until `recording=True`);
  - creates an observed bucket, then `_await_history_items(...)` which pages `get_resource_config_history(resourceType="AWS::S3::Bucket", resourceId=<observed>)` until `configurationItems` is non-empty — this SUCCEEDS, so the resource is discovered and items are returned;
  - `items[-1]["arn"]` — this is where it dies.

Code grounding in the checkout:
  - services/service-com-amazonaws-configservice-starlingdoveservice/src/state.rs:2260 — `arn: None,` in `DiscoveredResourceEntry::to_base_configuration_item`.
  - services/service-com-amazonaws-configservice-starlingdoveservice/src/state.rs:2319 — `arn: base.arn,` in `to_configuration_item` (so the full item inherits the absent value).
  - services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/get_resource_config_history.rs:313-314 and 352-355 — the handler projects items via `entry.configuration_items(&region, ...)` and returns them unchanged.

Model grounding: ConfigurationItem.arn -> target `ARN`, doc "Amazon Resource Name (ARN) associated with the resource." (optional in the model, but populated by AWS for every recorded resource).

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline.
2. Re-replay cloudformation__config__s3.deploy-config-delivery-channel-by-cloudformation-delivers-a-snapshot-containing-the-recorded-item.
3. Manually, against the emulator:
   a. aws s3 create-bucket --bucket <delivery-bucket>; put a bucket policy granting config.amazonaws.com s3:GetBucketAcl, s3:ListBucket and s3:PutObject.
   b. aws configservice put-configuration-recorder --configuration-recorder name=<rec>,roleARN=<role trusting config.amazonaws.com>
   c. aws configservice put-delivery-channel --delivery-channel name=<chan>,s3BucketName=<delivery-bucket>
   d. aws configservice start-configuration-recorder --configuration-recorder-name <rec>
   e. aws s3 create-bucket --bucket <observed-bucket>
   f. aws configservice get-resource-config-history --resource-type AWS::S3::Bucket --resource-id <observed-bucket>
4. Observe: configurationItems is non-empty, but no item has an `arn` field; AWS returns `arn` on every item, equal to the observed bucket's ARN.
