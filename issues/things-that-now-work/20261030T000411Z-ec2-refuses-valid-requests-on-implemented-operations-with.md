Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-images-amazon-linux-2
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-images-amazon-linux-2 ec2-catalog-describe-images-amazon-linux-2023 ec2-catalog-describe-images-deep-learning ec2-catalog-describe-images-ecs-optimized ec2-catalog-describe-images-eks-optimized ec2-catalog-describe-images-windows-server-2025-base ec2-catalog-describe-images-windows-server-base ec2-catalog-get-image-ancestry ec2-conflict-volume-attachment ec2-create-image-copy-and-usage-report ec2-create-snapshots-from-instance ec2-report-instance-status-op-contract ec2-t12-ebs-volume-attach-detach ec2-t12-eni-attach-and-private-ip ec2-t12-iam-profile-replace-and-stop-echo ec2-client-vpn-endpoint-mutual-cert-auth-attempt ec2-on-demand-capacity-reservation-visibility ec2-instance-event-window-and-application-status-check-lifecycle ec2-conflict-elastic-ip-association
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeImages|2xx->4xx|$.DescribeImagesResponse
Issue-Key: ec2|DescribeImages|2xx->4xx|status
Issue-Key: ec2|GetImageAncestry|2xx->4xx|$.GetImageAncestryResponse
Issue-Key: ec2|GetImageAncestry|2xx->4xx|status
Issue-Key: ec2|RunInstances|2xx->4xx|status
Issue-Key: ec2|DescribeClientVpnEndpoints|2xx->4xx|$.DescribeClientVpnEndpointsResponse
Issue-Key: ec2|DescribeClientVpnEndpoints|2xx->4xx|status
Issue-Key: ec2|DescribeCapacityReservationBillingRequests|2xx->4xx|$.DescribeCapacityReservationBillingRequestsResponse
Issue-Key: ec2|DescribeCapacityReservationBillingRequests|2xx->4xx|status
Issue-Key: ec2|CreateApplicationStatusCheck|2xx->4xx|$.CreateApplicationStatusCheckResponse
Issue-Key: ec2|CreateApplicationStatusCheck|2xx->4xx|status
Issue-Key: ec2|DeleteVolume|2xx->4xx|status
Issue-Key: ec2|AssociateAddress|2xx->5xx|$.AssociateAddressResponse
Issue-Related: 20261029T200817Z-ec2-elastic-ip-and-application-status-check-state.md
Issue-Commit: 7c2160283d3342f6e10783a5c4b2d926b7b7bab3

# EC2 refuses valid requests on implemented operations with 400 or 501

> _Note: this issue was identified against the code base at commit `7c2160283d3342f6e10783a5c4b2d926b7b7bab3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
Requests that AWS answers 200 are refused by the emulator with a 4xx (or a 501 on an operation that is otherwise reachable), so the whole downstream fixture cannot run. This bundle carries 8 causes; all are requests the recording shows succeeding against AWS.

Cause 1 of 8: DescribeImages with owner/filter selection over the public AMI catalog.
AWS: returns 200 with DescribeImagesResponse.imagesSet listing the matching images and their modeled members (Smithy: DescribeImages input Owners/Filters/ImageIds/IncludeDeprecated/MaxResults/NextToken, output ImagesSet + NextToken, paginated).
Emulator: answers HTTP 400 and no DescribeImagesResponse at all, for every catalog query in the cluster (33 affected steps across the amazon-linux-2, amazon-linux-2023, deep-learning, ecs-optimized and eks-optimized fixtures).
Why: AMI lookup is the first step of nearly every instance-launch workflow; a client cannot resolve an image id and stops.
Fix: the DescribeImages handler's request parsing/validation and its backing image catalog.
Traces: ec2-catalog-describe-images-amazon-linux-2#task#1 (+32 affected steps).

Cause 2 of 8: GetImageAncestry.
AWS: returns 200 with GetImageAncestryResponse.imageAncestryEntrySet, each entry carrying imageId, imageOwnerAlias and creationDate.
Emulator: HTTP 400 with no response body, for all 11 steps of the cluster.
Why: image-provenance queries fail outright.
Fix: the GetImageAncestry handler's validation and its catalog lookup.
Traces: ec2-catalog-get-image-ancestry#task#1 (+10 affected steps).

Cause 3 of 8: RunInstances for a single t3-class instance in a fixture-created subnet.
AWS: returns 200 with a reservation describing the launched instance.
Emulator: HTTP 400, so 7 fixtures lose their instance and every later step in them is unusable.
Why: no instance-dependent workflow (volume attach, ENI attach, image creation, status reporting) can be exercised.
Fix: the RunInstances handler's request validation (the refusal is produced before any instance is created).
Traces: ec2-conflict-volume-attachment#setup#3 (+6 affected steps).

Cause 4 of 8: DescribeClientVpnEndpoints after the endpoint is created.
AWS: returns 200 with DescribeClientVpnEndpointsResponse (empty clientVpnEndpoint element when nothing matches).
Emulator: HTTP 400 with no response body.
Why: a client cannot enumerate its Client VPN endpoints at all.
Fix: the DescribeClientVpnEndpoints handler's validation/filter handling.
Traces: ec2-client-vpn-endpoint-mutual-cert-auth-attempt#task#4.

Cause 5 of 8: DescribeCapacityReservationBillingRequests.
AWS: returns 200 with DescribeCapacityReservationBillingRequestsResponse and an empty capacityReservationBillingRequestSet when there are no requests.
Emulator: HTTP 400.
Why: an empty-result read is turned into a hard error, so clients treat a healthy account as broken.
Fix: the DescribeCapacityReservationBillingRequests handler, which must serve an empty set rather than refuse. Related but not covering: 20261029T215142Z-ec2-describe-paths-refuse-or-mis-scope-requests.md owns other describe refusals, not this operation.
Traces: ec2-on-demand-capacity-reservation-visibility#task#4.

Cause 6 of 8: CreateApplicationStatusCheck.
AWS: returns 200 with CreateApplicationStatusCheckResponse.applicationStatusCheck echoing the supplied protocol/port/path/interval/timeout/thresholds plus the generated applicationStatusCheckId and creationTime.
Emulator: HTTP 400 with no response body.
Why: the status-check resource cannot be created, so the whole application-status-check workflow is unreachable.
Fix: the CreateApplicationStatusCheck handler's input validation.
Traces: ec2-instance-event-window-and-application-status-check-lifecycle#task#5.

Cause 7 of 8: DeleteVolume on an unattached volume during teardown.
AWS: returns 200.
Emulator: HTTP 400, so the volume cannot be deleted even in the state where deletion is legal (note the mirror-image defect in the conflict case is filed separately).
Why: teardown leaks volumes and a client's delete loop retries forever.
Fix: the DeleteVolume handler's validation/state lookup.
Traces: ec2-conflict-volume-attachment#teardown#19.

Cause 8 of 8: AssociateAddress for an allocated EIP and a running instance.
AWS: returns 200 with AssociateAddressResponse carrying return='true' and the generated associationId.
Emulator: answers HTTP 501 on this path, i.e. the operation reports itself unimplemented for a request AWS satisfies.
Why: no Elastic IP can be associated, so any public-addressing workflow stops.
Fix: the AssociateAddress handler. Related but not covering: 20261029T200817Z-ec2-elastic-ip-and-application-status-check-state.md owns a different AssociateAddress step cluster.
Traces: ec2-conflict-elastic-ip-association#task#8.

## Evidence
Cause 1 - status expected 200, got 400; $.DescribeImagesResponse missing entirely (cluster of 33 steps).
Cause 2 - status expected 200, got 400; $.GetImageAncestryResponse missing (cluster of 11 steps).
Cause 3 - status expected 200, got 400 on RunInstances (cluster of 7 setup steps).
Cause 4 - status expected 200, got 400; $.DescribeClientVpnEndpointsResponse missing.
Cause 5 - status expected 200, got 400; $.DescribeCapacityReservationBillingRequestsResponse missing.
Cause 6 - status expected 200, got 400; $.CreateApplicationStatusCheckResponse missing.
Cause 7 - status expected 200, got 400 on DeleteVolume teardown.
Cause 8 - status expected 200, got 501; $.AssociateAddressResponse missing (expected return='true' + associationId).

## Steps to Reproduce
Replay simparity-wire-aws-v1 and inspect ec2-catalog-describe-images-amazon-linux-2#task#1, ec2-catalog-get-image-ancestry#task#1, ec2-conflict-volume-attachment#setup#3 and #teardown#19, ec2-client-vpn-endpoint-mutual-cert-auth-attempt#task#4, ec2-on-demand-capacity-reservation-visibility#task#4, ec2-instance-event-window-and-application-status-check-lifecycle#task#5, ec2-conflict-elastic-ip-association#task#8.
