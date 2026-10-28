Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-images-amazon-linux-2
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-images-amazon-linux-2 ec2-catalog-describe-images-amazon-linux-2023 ec2-catalog-describe-images-deep-learning ec2-catalog-describe-images-ecs-optimized ec2-catalog-describe-images-eks-optimized ec2-catalog-describe-images-windows-server-2025-base ec2-catalog-describe-images-windows-server-base ec2-page-describe-instances-tag-filtered-two-pages ec2-page-describe-volumes-tag-filtered-two-pages ec2-vm-migration-and-image-performance-task-tracking ec2-instance-event-window-and-application-status-check-lifecycle ec2-elastic-ip-lifecycle ec2-conflict-volume-attachment ec2-t12-ebs-volume-attach-detach ec2-t12-eni-attach-and-private-ip ec2-t12-iam-profile-replace-and-stop-echo
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeImages|2xx->4xx|$.DescribeImagesResponse
Issue-Key: ec2|DescribeImages|2xx->4xx|$headers.content-type
Issue-Key: ec2|DescribeImages|2xx->4xx|status
Issue-Key: ec2|DescribeInstances|2xx->4xx|$.DescribeInstancesResponse
Issue-Key: ec2|DescribeInstances|2xx->4xx|$headers.content-type
Issue-Key: ec2|DescribeInstances|2xx->4xx|status
Issue-Key: ec2|DescribeVolumes|2xx->4xx|$.DescribeVolumesResponse
Issue-Key: ec2|DescribeVolumes|2xx->4xx|$headers.content-type
Issue-Key: ec2|DescribeVolumes|2xx->4xx|status
Issue-Key: ec2|DescribeFastLaunchImages|2xx->4xx|$.DescribeFastLaunchImagesResponse
Issue-Key: ec2|DescribeFastLaunchImages|2xx->4xx|$headers.content-type
Issue-Key: ec2|DescribeFastLaunchImages|2xx->4xx|status
Issue-Key: ec2|CreateApplicationStatusCheck|2xx->4xx|$.CreateApplicationStatusCheckResponse
Issue-Key: ec2|CreateApplicationStatusCheck|2xx->4xx|$headers.content-type
Issue-Key: ec2|CreateApplicationStatusCheck|2xx->4xx|status
Issue-Key: ec2|CreateTags|2xx->4xx|$.CreateTagsResponse
Issue-Key: ec2|CreateTags|2xx->4xx|$headers.content-type
Issue-Key: ec2|CreateTags|2xx->4xx|status
Issue-Key: ec2|RunInstances|2xx->5xx|status
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# EC2 reads and mutations that AWS answers 200 are refused 400 or 501

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
Seven operations refuse a request that AWS answers successfully. None of them is an unimplemented scaffold (every cluster reports an empty stub_path), so these are live handlers rejecting valid input. All seven must be closed before submitting.

Cause 1 of 7: DescribeImages refuses the public-AMI catalog queries. This is the largest cluster in the corpus - 33 replayed steps across the Amazon Linux 2, Amazon Linux 2023, Deep Learning, ECS-optimized and EKS-optimized catalog fixtures.
AWS: HTTP 200 with a DescribeImagesResponse whose imagesSet carries the matching AMI (recorded example: imageId ami-09179e36e2b518604, imageOwnerAlias 'amazon', with blockDeviceMapping, platformDetails, usageOperation and publicSsmParameterName). Smithy: DescribeImages returns DescribeImagesResult; errors: [].
Emulator: HTTP 400 and no DescribeImagesResponse at all.
Why: AMI discovery is the first call of almost every compute client; with it refused, nothing that resolves an image by owner/name filter can run.
Fix: filter handling in the DescribeImages handler and the AMI catalog it reads.
Traces: ec2-catalog-describe-images-amazon-linux-2#task#1 (cluster of 33 steps).

Cause 2 of 7: DescribeInstances refuses a tag-filtered paging request.
AWS: HTTP 200 with the reservationSet for the tagged instances.
Emulator: HTTP 400, no DescribeInstancesResponse.
Why: a paging client cannot enumerate its own instances.
Fix: filter/pagination argument handling in the DescribeInstances handler.
Traces: ec2-page-describe-instances-tag-filtered-two-pages#task#14.

Cause 3 of 7: DescribeVolumes refuses a tag-filtered paging request.
AWS: HTTP 200 with the volumeSet for the tagged volumes.
Emulator: HTTP 400, no DescribeVolumesResponse.
Why and Fix: as cause 2, in the DescribeVolumes handler.
Traces: ec2-page-describe-volumes-tag-filtered-two-pages#task#10.

Cause 4 of 7: DescribeFastLaunchImages refuses a request it answers elsewhere in the corpus.
AWS: HTTP 200 with a DescribeFastLaunchImagesResponse carrying an empty fastLaunchImageSet.
Emulator: HTTP 400, no response body.
Why: an empty result is reported as a failure, so a client cannot tell 'nothing configured' from 'call rejected'.
Fix: request handling in the DescribeFastLaunchImages handler (it answers 200 for the contract fixture, so the rejection is input-dependent).
Traces: ec2-vm-migration-and-image-performance-task-tracking#task#9.

Cause 5 of 7: CreateApplicationStatusCheck refuses a valid create.
AWS: HTTP 200 with a CreateApplicationStatusCheckResponse carrying the applicationStatusCheck (aggregation, deviceIndex, failureThreshold, interval, protocol, statusCodeMatcher, timeout ...).
Emulator: HTTP 400, no response body.
Fix: the CreateApplicationStatusCheck handler and its validation.
Traces: ec2-instance-event-window-and-application-status-check-lifecycle#task#5.

Cause 6 of 7: CreateTags refuses tagging an Elastic IP allocation.
AWS: HTTP 200 with a CreateTagsResponse. Smithy: CreateTags input Resources (ResourceIdList, required) and Tags (TagList, required); output_shape Unit.
Emulator: HTTP 400, no response body - yet the same operation answers 200 for VPC resources elsewhere in the corpus, so the rejection is resource-type dependent.
Why: tagging is how clients label everything they create; an allocation id that AWS accepts is refused.
Fix: resource-type dispatch in the CreateTags handler.
Traces: ec2-elastic-ip-lifecycle#task#4.

Cause 7 of 7: RunInstances answers 501 on the subnet-launch path used by four fixtures.
AWS: HTTP 200 with a RunInstancesResponse.
Emulator: HTTP 501 (RunInstances is not a scaffold stub - stub_path is empty and it answers 200 on other paths), so every fixture that needs a running instance (volume attach/detach, ENI attach, IAM profile replace) cannot set up.
Why: the single most central EC2 mutation is unavailable for this request shape, blocking whole lifecycles downstream.
Fix: the RunInstances handler path that is returning NotImplemented for this input.
Traces: ec2-conflict-volume-attachment#setup#3 (cluster also covers ec2-t12-ebs-volume-attach-detach#setup#3, ec2-t12-eni-attach-and-private-ip#setup#3, ec2-t12-iam-profile-replace-and-stop-echo#setup#8).

## Evidence
Cause 1 - 'status: expected 200, got 400\n$.DescribeImagesResponse: missing (expected {...imageId: ami-09179e36e2b518604...})', affected_count 33. Cause 2 - expected 200, got 400, $.DescribeInstancesResponse missing. Cause 3 - expected 200, got 400, $.DescribeVolumesResponse missing. Cause 4 - expected 200, got 400, $.DescribeFastLaunchImagesResponse missing (expected fastLaunchImageSet ''). Cause 5 - expected 200, got 400, $.CreateApplicationStatusCheckResponse missing. Cause 6 - expected 200, got 400, $.CreateTagsResponse missing. Cause 7 - expected 200, got 501, affected_count 4.

## Steps to Reproduce
Replay the fixture steps named under Traces. Minimal repros: (1) ec2:DescribeImages Owners=['amazon'] Filters=[{'Name':'name','Values':['amzn2-ami-kernel-5.10-hvm-*-x86_64-gp2']}]. (2) ec2:DescribeInstances Filters=[{'Name':'tag:...'}] MaxResults=N. (3) ec2:DescribeVolumes Filters=[{'Name':'tag:...'}] MaxResults=N. (4) ec2:DescribeFastLaunchImages as issued by the vm-migration fixture. (5) ec2:CreateApplicationStatusCheck on a running instance. (6) ec2:CreateTags Resources=[an eipalloc-... id] Tags=[{Key,Value}]. (7) ec2:RunInstances into a created subnet with a security group.
