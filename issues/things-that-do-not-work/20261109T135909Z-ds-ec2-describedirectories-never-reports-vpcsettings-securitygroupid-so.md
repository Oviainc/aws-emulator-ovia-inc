Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds__ec2.delete-directory-releases-its-controllers-security-group-and-enis-from-the-vpc
Issue-Key: ds__ec2.delete-directory-releases-its-controllers-security-group-and-enis-from-the-vpc|_poll#5|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s; last observed {last!r}")
Issue-Commit: eaa123f65a60b02b9ee1461dec055a780335fce9

# ds__ec2: DescribeDirectories never reports VpcSettings.SecurityGroupId, so the DeleteDirectory network-release flow cannot even start

> _Note: this issue was identified against the code base at commit `eaa123f65a60b02b9ee1461dec055a780335fce9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDirectories

## Description
ONE net-new cause, grounded in the single failing check the suite lists under `### ds__ec2` that is neither ALREADY FILED nor OWNED by an open issue.

Cause 1 of 1: a VPC-placed Managed Microsoft AD never publishes the identifier of the domain-controller security group the service places in the caller's VPC, so DescribeDirectories' VpcSettings has no SecurityGroupId and no caller can reach, inspect or clean up the directory's network footprint.
AWS: Smithy — DirectoryDescription$VpcSettings targets DirectoryVpcSettingsDescription{VpcId, SubnetIds, AvailabilityZones, SecurityGroupId}; SecurityGroupId has @pattern `^(sg-[0-9a-f]{8}|sg-[0-9a-f]{17})$` and is documented "The domain controller security group identifier for the directory." Recorded behavior: once the directory has left Requested, DescribeDirectories reports VpcSettings.SecurityGroupId naming the AWS-managed controllers security group that EC2 DescribeSecurityGroups then resolves to exactly one group in the caller's VPC; DeleteDirectory later releases that group and the per-subnet directory ENIs, so the caller's subnets and VPC become deletable. The id VALUE differs per run and is not a requirement — its presence, its `sg-` shape and its resolvability in the directory's VPC are.
Emulator: the member is absent from the serialized VpcSettings. The replayed DescribeDirectories description of an Active MicrosoftAD carries VpcSettings = {VpcId, SubnetIds, AvailabilityZones} only, so a client polling for the controllers security group never sees one and the whole delete-releases-the-network sequence (EC2 DescribeSecurityGroups on the group, the two directory ENIs, DeleteDirectory, then the ENIs' InvalidNetworkInterfaceID.NotFound) is unreachable.
Why: a client cannot authorize traffic to the directory, cannot reconcile the directory against EC2, and cannot verify (or safely order) teardown of its own VPC: it fails on a missing modeled field rather than on any modeled refusal, and the documented two-phase teardown — directory deleted, then network resources released — cannot be observed at all.
Fix: the directory's network provisioning in the CreateMicrosoftAD/CreateDirectory handler (it must mint and persist a controllers security group id for the directory, and release it on DeleteDirectory) plus the single rendering point that builds DirectoryVpcSettingsDescription for every reporting operation (`Directory::described_vpc_settings` in services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/state.rs, around lines 2793-2815, which clones the create's settings and fills only AvailabilityZones).
Related-but-not-covering open issues: 20261108T155909Z-ds-2-net-new-causes-describedirectories-omits-vpcsettings.md (cause 1) reports the same absent member for a ds-side trust check, and 20261024T081028Z-ds-directory-service-a-microsoft-ad-never-provisions-its.md covers the group/ENIs not EXISTING in EC2 for other ds__ec2 checks; neither carries this check's replay key, and this check additionally needs the group and ENIs to be released on DeleteDirectory. A fixer should land them together.
Traces: ds__ec2.delete-directory-releases-its-controllers-security-group-and-enis-from-the-vpc.

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against this checkout: Total 130/180 passed; ds__ec2 7/21 (14 failed) — the services answer, so this is a fidelity divergence, not a bootstrap gap.

ds__ec2.delete-directory-releases-its-controllers-security-group-and-enis-from-the-vpc [mismatch]:
  AssertionError: the controllers security group is reported not met within 300.0s; last observed {'DirectoryId': '<id>', 'Name': '<generated>.test', 'ShortName': '<generated>', 'Size': 'Small', 'Edition': 'Standard', 'Alias': '<id>', 'AccessUrl': '<id>.awsapps.com', 'DnsIpAddrs': ['10.192.0.4', '10.192.1.4'], 'Stage': 'Active', 'LaunchTime': <ts>, 'StageLastUpdatedDateTime': <ts>, 'Type': 'MicrosoftAD', 'VpcSettings': {'VpcId': '<vpc>', 'SubnetIds': ['<subnet-a>', '<subnet-b>'], 'AvailabilityZones': ['us-west-2a', 'us-west-2b']}, 'SsoEnabled': False, 'DesiredNumberOfDomainControllers': 2, 'OsVersion': 'SERVER_2019'}
  The check polls DescribeDirectories for `VpcSettings.SecurityGroupId` to be present (300 s budget) before it can run its EC2 controls; the member never appears, so EC2 DescribeSecurityGroups(GroupIds=[sg]), the two per-subnet ENI assertions, DeleteDirectory and the post-delete ENI release assertions are never reached. Ids, names, hosts and timestamps above are per-run values; the requirement is that VpcSettings carries a SecurityGroupId matching `^(sg-[0-9a-f]{8}|sg-[0-9a-f]{17})$` for a VPC-placed directory that has left Requested.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite(); ds__ec2 answers 7/21 checks, so this is a fidelity divergence rather than a bootstrap gap.
2. ec2:CreateVpc + two subnets in two AZs, then ds:CreateMicrosoftAD(Name=<x>.test, Password=<compliant>, Edition=Standard, VpcSettings={VpcId, SubnetIds:[a,b]}).
3. Poll ds:DescribeDirectories(DirectoryIds=[<id>]) and read DirectoryDescriptions[0].VpcSettings — expect a SecurityGroupId (sg-…) once the directory has left Requested; observe the key absent for the whole 300 s budget.
4. With the group id AWS would report, ec2:DescribeSecurityGroups(GroupIds=[<sg>]) must list exactly one group in the check's VPC, ec2:DescribeNetworkInterfaces must show one directory ENI per chosen subnet, and after ds:DeleteDirectory both the ENIs and the group must be released (each ENI then answering InvalidNetworkInterfaceID.NotFound "The networkInterface ID '<eni>' does not exist") — none of this is reachable today.
5. Re-replay ds__ec2.delete-directory-releases-its-controllers-security-group-and-enis-from-the-vpc.
