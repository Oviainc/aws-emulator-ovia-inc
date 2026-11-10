Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.dpeer-overlapping-cidr-peering-fails-and-never-carries-traffic ec2.dpeer-route-is-blackhole-until-the-peering-is-accepted ec2.droute-deleted-peering-target-blackholes-the-route-and-drops-traffic ec2.ec2mr-byvpc-launch-with-another-regions-subnet-or-security-group-rejected ec2.ec2mr-dvpc-created-vpcs-classify-non-default-in-their-own-region-only ec2.ec2mr-inv-every-enabled-region-answers-and-holds-only-its-own-instances ec2.ec2mr-inv2-an-instance-without-a-public-ip-omits-the-key-rather-than-nulling-it ec2.ec2mr-inv2-ssh-reachability-conjunction-holds-per-region-across-all-three ec2.ec2mr-pip-one-private-ip-in-two-regions-is-two-independent-interfaces ec2.op-contract-delete-traffic-mirror-filter ec2.op-contract-delete-traffic-mirror-session ec2.op-contract-delete-traffic-mirror-target ec2.vpc-name-resolution-dhcp-black-hole-nameserver-fails-guest-lookups-while-default-resolves ec2.vscan-public-exposure-derivation-selects-only-the-host-with-a-public-ip ec2.ltver-launch-naming-a-nonexistent-template-version-rejected ec2.op-contract-delete-launch-template
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.ltver-launch-naming-a-nonexistent-template-version-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: ec2.op-contract-delete-launch-template|run#22|AssertionError|assert message == not_found, f"unexpected DescribeLaunchTemplateVersions message {message!r}"
Issue-Related: 20261110T080328Z-autoscaling-ec2-elbv2-runinstances-with-a-networkinterfaces-specification-answers-501-notimplemented.md
Issue-Commit: b4658095b9c4860859c50d7c6684da8fd2f8280e

# ec2: 3 net-new causes — RunInstances with a NetworkInterfaces specification answers 501 NotImplemented, and two launch-template not-found refusals carry the wrong message

> _Note: this issue was identified against the code base at commit `b4658095b9c4860859c50d7c6684da8fd2f8280e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
REOPENS 20261022T235108Z-ec2-sql-ha-application-status-check-fast-launch.md: its fix left the call refused. That issue closed the LaunchTemplate branch of RunInstances; cause 1 below is the NetworkInterfaces branch of the same operation, still answering NotImplemented, so it is the un-fixed remainder of that refusal rather than a new defect. Causes 2 and 3 are net-new message-fidelity defects.

Three root causes. All three must be closed before submitting.

Cause 1 of 3: RunInstances refuses any launch that carries an interface specification.
AWS: Smithy com.amazonaws.ec2#RunInstancesRequest models NetworkInterfaces as a list of InstanceNetworkInterfaceSpecification (DeviceIndex, SubnetId, Groups, AssociatePublicIpAddress, PrivateIpAddress/PrivateIpAddresses, DeleteOnTermination, Description, InterfaceType, Ipv6Addresses, …). A launch naming one eth0 interface answers HTTP 200 with a Reservation whose Instances carry that interface and its addresses; EC2 models no error for this launch shape and the recordings show a 200.
Emulator: HTTP 501, error code NotImplemented, message "operation RunInstances with a network interface specification is scaffolded but not implemented in this emulator" — wholly emulator-authored fixed text, no value taken from the request.
Why: interface-level launch is how every VPC-aware client pins a subnet, security groups and public-IP association on eth0. No guest ever boots, so VPC peering/route blackholing, multi-region inventory, DHCP name resolution, traffic-mirror lifecycles and public-exposure derivation are all unreachable.
Fix: the NetworkInterfaces branch of services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs — resolve/create the described interfaces, attach them at their DeviceIndex and return them on the Reservation's Instances.
Failing checks: ec2.dpeer-overlapping-cidr-peering-fails-and-never-carries-traffic, ec2.dpeer-route-is-blackhole-until-the-peering-is-accepted, ec2.droute-deleted-peering-target-blackholes-the-route-and-drops-traffic, ec2.ec2mr-byvpc-launch-with-another-regions-subnet-or-security-group-rejected, ec2.ec2mr-dvpc-created-vpcs-classify-non-default-in-their-own-region-only, ec2.ec2mr-inv-every-enabled-region-answers-and-holds-only-its-own-instances, ec2.ec2mr-inv2-an-instance-without-a-public-ip-omits-the-key-rather-than-nulling-it, ec2.ec2mr-inv2-ssh-reachability-conjunction-holds-per-region-across-all-three, ec2.ec2mr-pip-one-private-ip-in-two-regions-is-two-independent-interfaces, ec2.op-contract-delete-traffic-mirror-filter, ec2.op-contract-delete-traffic-mirror-session, ec2.op-contract-delete-traffic-mirror-target, ec2.vpc-name-resolution-dhcp-black-hole-nameserver-fails-guest-lookups-while-default-resolves, ec2.vscan-public-exposure-derivation-selects-only-the-host-with-a-public-ip.

Cause 2 of 3: the RunInstances refusal for a launch-template version that does not exist carries the wrong message.
AWS: launching with LaunchTemplate{LaunchTemplateId, Version} naming a version the template does not have is refused with HTTP 400 and the ec2Query error code InvalidLaunchTemplateId.VersionNotFound (EC2 error codes are documented, not modeled in Smithy) and the message "Could not find the specified version <version> for the launch template with ID <launch-template-id>." The fixed text is "Could not find the specified version" / "for the launch template with ID" plus the trailing period; the version number and the template id are values taken from the request.
Emulator: the code is right but the message is emulator-authored: "Launch template version '<version>' does not exist for launch template '<launch-template-id>'" — different wording, quoted values and no trailing period.
Why: operators and tooling match on this text, and the AWS form is the one that names the template by ID in the documented phrasing; the divergence breaks log scraping and any client that surfaces the message verbatim.
Fix: the launch-template version resolution error in services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs (its InvalidLaunchTemplateId.VersionNotFound message template).
Failing check: ec2.ltver-launch-naming-a-nonexistent-template-version-rejected.

Cause 3 of 3: DescribeLaunchTemplateVersions reuses the DescribeLaunchTemplates not-found message.
AWS: DescribeLaunchTemplateVersions(LaunchTemplateId=<deleted id>) is refused with HTTP 400, ec2Query code InvalidLaunchTemplateId.NotFound and the single-id message "The specified launch template, with template ID <launch-template-id>, does not exist." — fixed text with the template id taken from the request. The plural "At least one of the launch templates specified in the request does not exist." is the message DescribeLaunchTemplates uses for its id-list form, and the two must not be interchanged (the same single-id message is also what a double DeleteLaunchTemplate returns).
Emulator: DescribeLaunchTemplateVersions answers the plural DescribeLaunchTemplates message "At least one of the launch templates specified in the request does not exist." instead.
Why: a client cannot distinguish "this template id is gone" from "one of the ids in my list is gone", and the message no longer names the id that failed — the only identifying value in the refusal.
Fix: the not-found refusal of services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_launch_template_versions.rs — use the single-id message template and interpolate the requested LaunchTemplateId.
Failing check: ec2.op-contract-delete-launch-template.

## Evidence
Cause 1 — replay output, identical for all fourteen checks:
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RunInstances operation: operation RunInstances with a network interface specification is scaffolded but not implemented in this emulator
Expected HTTP 200 with a Reservation whose Instances carry the requested eth0 interface.

Cause 2 — ec2.ltver-launch-naming-a-nonexistent-template-version-rejected:
AssertionError: expected message 'Could not find the specified version 3 for the launch template with ID lt-...', got "Launch template version '3' does not exist for launch template 'lt-...'"
(the error code InvalidLaunchTemplateId.VersionNotFound already matched; the template id and version shown are per-run values filling the template.)

Cause 3 — ec2.op-contract-delete-launch-template:
AssertionError: unexpected DescribeLaunchTemplateVersions message 'At least one of the launch templates specified in the request does not exist.'
Expected: 'The specified launch template, with template ID <lt-id>, does not exist.' with code InvalidLaunchTemplateId.NotFound.

## Steps to Reproduce
Cause 1: CreateVpc/CreateSubnet/CreateSecurityGroup, then ec2:RunInstances(MinCount=1, MaxCount=1, NetworkInterfaces=[{DeviceIndex:0, SubnetId:<subnet>, Groups:[<sg>], AssociatePublicIpAddress:true}]) — expect 200, observe 501 NotImplemented.
Cause 2: CreateLaunchTemplate, CreateLaunchTemplateVersion (so versions 1 and 2 exist), then ec2:RunInstances(LaunchTemplate={LaunchTemplateId:<id>, Version:'3'}, MinCount=1, MaxCount=1) — expect InvalidLaunchTemplateId.VersionNotFound with 'Could not find the specified version 3 for the launch template with ID <id>.'
Cause 3: CreateLaunchTemplate, DeleteLaunchTemplate, then ec2:DescribeLaunchTemplateVersions(LaunchTemplateId=<deleted id>) — expect InvalidLaunchTemplateId.NotFound with 'The specified launch template, with template ID <id>, does not exist.'
Then re-replay every check id in trace_ids.
