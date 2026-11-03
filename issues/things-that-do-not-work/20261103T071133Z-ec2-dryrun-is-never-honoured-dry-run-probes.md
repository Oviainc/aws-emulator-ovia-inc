Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-dvpc-traj-dry-run-probe-then-real-call
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-dvpc-traj-dry-run-probe-then-real-call ec2-asgeni-traj-launch-template-ni-accepted-then-refused-at-launch ec2-asgeni-traj-run-instances-ni-device-index-and-placement ec2-asgeni-traj-run-instances-ni-param-conflicts ec2-ipam-trajwire-create-vpc-parameter-mistakes ec2-sshp-traj-instance-connect-endpoint-lookup-and-create-mistakes ec2-trajwire-lia-dry-run-permission-probe ec2-vpce-traj-iot-data-endpoint-create-refusals ec2-trajwire-deb-deb-instance-profile-named-by-role ec2-trajwire-deb-deb-instance-type-casing-and-absent-ami ec2-trajwire-deb-deb-key-pair-named-before-created ec2-trajwire-deb-deb-security-group-name-passed-with-subnet
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeVpcs|4xx->4xx|status
Issue-Key: ec2|RunInstances|4xx->5xx|$.Response.Errors.Error.Code
Issue-Key: ec2|RunInstances|4xx->5xx|$.Response.Errors.Error.Message
Issue-Key: ec2|RunInstances|4xx->5xx|status
Issue-Key: ec2|CreateVpc|4xx->5xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateVpc|4xx->5xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateVpc|4xx->5xx|status
Issue-Key: ec2|CreateInstanceConnectEndpoint|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateInstanceConnectEndpoint|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateInstanceConnectEndpoint|4xx->4xx|status
Issue-Key: ec2|DescribeInstances|4xx->4xx|status
Issue-Key: ec2|CreateVpcEndpoint|4xx->4xx|status
Issue-Key: ec2|RunInstances|4xx->4xx|status
Issue-Related: 20261029T200817Z-ec2-parameter-limit-refusals-report-emulator-authored-limits.md
Issue-Related: 20261103T071133Z-ec2-invalid-enum-and-attribute-values-are-refused.md
Issue-Related: 20261103T071133Z-ec2-pagination-and-filter-validation-refusals-use-emulator.md
Issue-Commit: ce4f28f15b4841beefbad6c596b9d2158570753e

# EC2 DryRun is never honoured: dry-run probes answer a validation refusal or 501 instead of 412 DryRunOperation

> _Note: this issue was identified against the code base at commit `ce4f28f15b4841beefbad6c596b9d2158570753e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcs

## Description
AWS's DryRun contract: when a request would have succeeded, EC2 answers HTTP 412 with Code 'DryRunOperation' and Message 'Request would have succeeded, but DryRun flag is set.' (fixed text, no variable part); when the request would NOT have succeeded, AWS answers that request's own refusal instead. The emulator gets both directions wrong on six paths — it never reaches the DryRun decision (answering a parse/validation refusal or 501), or it short-circuits to 412 for a request AWS refuses.

Cause 1 of 7: DescribeVpcs with DryRun=TRUE (upper case).
AWS: Smithy: DescribeVpcs.DryRun targets Boolean (wire name dryRun); the ec2Query wire form accepts the value case-insensitively, so DryRun=TRUE yields 412 'DryRunOperation' / 'Request would have succeeded, but DryRun flag is set.'
Emulator: 400 Code 'MalformedInput', Message 'expected `true`/`false`, got `TRUE`'.
Why: a dry-run permission probe written with an upper-case literal is reported as a malformed request, so the caller concludes its request shape is broken rather than that it has permission.
Fix: boolean parsing in the ec2Query input deserializer feeding services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpcs.rs.
Traces: ec2-dvpc-traj-dry-run-probe-then-real-call#task#1.

Cause 2 of 7: RunInstances dry run with a LaunchTemplate.
AWS: recorded behavior only — 412 'DryRunOperation' / 'Request would have succeeded, but DryRun flag is set.'
Emulator: 501 Code 'NotImplemented', Message 'operation RunInstances with a LaunchTemplate is scaffolded but not implemented in this emulator'.
Why: the caller cannot probe launch permission at all; 501 is not an answer any AWS client models.
Fix: the launch-template branch of run_instances.rs must evaluate DryRun before refusing.
Traces: ec2-asgeni-traj-launch-template-ni-accepted-then-refused-at-launch#task#12 (cluster also covers task#15, ec2-asgeni-traj-run-instances-ni-device-index-and-placement#task#11, ec2-asgeni-traj-run-instances-ni-param-conflicts#task#9/#10/#11).

Cause 3 of 7: CreateVpc dry run with an IPAM-allocated CIDR.
AWS: recorded behavior only — 412 'DryRunOperation' / 'Request would have succeeded, but DryRun flag is set.'
Emulator: 501 'NotImplemented' / 'operation CreateVpc with an IPAM-allocated CIDR block is scaffolded but not implemented in this emulator'.
Why/Fix: as cause 2, in create_vpc.rs.
Traces: ec2-ipam-trajwire-create-vpc-parameter-mistakes#task#15.

Cause 4 of 7: CreateInstanceConnectEndpoint dry run.
AWS: recorded behavior only — 412 'DryRunOperation' / 'Request would have succeeded, but DryRun flag is set.'
Emulator: 400 Code 'InvalidParameterValue', Message "The security group 'sg-00000000000000035' and the subnet 'subnet-0000000000000002b' belong to different networks" — the emulator applies a cross-VPC check AWS does not apply to this request and never reaches the DryRun decision.
Why: a dry-run probe reports a configuration error that the real call does not have.
Fix: create_instance_connect_endpoint.rs — evaluate DryRun after the checks AWS actually performs.
Traces: ec2-sshp-traj-instance-connect-endpoint-lookup-and-create-mistakes#task#10.

Cause 5 of 7: DescribeInstances dry run carrying an unsupported filter.
AWS: recorded behavior only — 412 'DryRunOperation' / 'Request would have succeeded, but DryRun flag is set.' (DryRun is answered before filter resolution for this request).
Emulator: 400 'InvalidParameterValue' / "The filter 'bogus-filter' is not one this emulator resolves" — an emulator-authored sentence that even names the emulator.
Why: permission probes fail and the message advertises the emulator rather than AWS behavior.
Fix: describe_instances.rs DryRun handling and filter-refusal text.
Traces: ec2-trajwire-lia-dry-run-permission-probe#task#3.

Cause 6 of 7: CreateVpcEndpoint dry run.
AWS: recorded behavior only — 412 'DryRunOperation' / 'Request would have succeeded, but DryRun flag is set.'
Emulator: 400 Code 'InvalidParameter', Message "Private DNS can't be enabled because the VPC vpc-00000000000000024 doesn't support DNS hostnames or DNS resolution".
Why/Fix: as cause 4, in create_vpc_endpoint.rs.
Traces: ec2-vpce-traj-iot-data-endpoint-create-refusals#task#5.

Cause 7 of 7: RunInstances dry run that AWS refuses — the opposite error.
AWS: recorded behavior only — the request names an IAM instance profile that is not valid, so DryRun does NOT win: 400 Code 'InvalidParameterValue', Message 'Value (sim2real-probe-deb-ip-1a34b480-PowerUserRole) for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name' (fixed text with the request's profile name substituted).
Emulator: 412 'DryRunOperation' / 'Request would have succeeded, but DryRun flag is set.' — it reports success for a request AWS rejects.
Why: a dry run is used precisely to discover such mistakes before launching; the emulator certifies a launch that would fail.
Fix: run_instances.rs must validate the request (instance profile, key name, security-group/subnet combination) before answering DryRunOperation.
Traces: ec2-trajwire-deb-deb-instance-profile-named-by-role#task#6 (cluster also covers ec2-trajwire-deb-deb-instance-type-casing-and-absent-ami#task#4, ec2-trajwire-deb-deb-key-pair-named-before-created#task#3, ec2-trajwire-deb-deb-security-group-name-passed-with-subnet#task#4/#5).

## Evidence
Replayed wire corpus simparity-wire-aws-v1. Representative: ec2-dvpc-traj-dry-run-probe-then-real-call task#1 — expected 412 / 'DryRunOperation' / 'Request would have succeeded, but DryRun flag is set.'; actual 400 / 'MalformedInput' / 'expected `true`/`false`, got `TRUE`'. RunInstances-with-template and CreateVpc-with-IPAM dry runs answer 501 NotImplemented; CreateInstanceConnectEndpoint, DescribeInstances and CreateVpcEndpoint dry runs answer 400 validation refusals; conversely RunInstances answers 412 DryRunOperation where the recording has 400 InvalidParameterValue for an invalid iamInstanceProfile.name (5 steps).

## Steps to Reproduce
1. Call DescribeVpcs with DryRun=TRUE (fixture ec2-dvpc-traj-dry-run-probe-then-real-call, task#1): expect 412 DryRunOperation, observe 400 MalformedInput.
2. Call RunInstances with a LaunchTemplate and DryRun=true (ec2-asgeni-traj-launch-template-ni-accepted-then-refused-at-launch, task#12): observe 501.
3. Call RunInstances with DryRun=true and an invalid IamInstanceProfile name (ec2-trajwire-deb-deb-instance-profile-named-by-role, task#6): observe 412 instead of the recorded 400 InvalidParameterValue.
4. Repeat the remaining fixture#step tokens in trace_ids.
