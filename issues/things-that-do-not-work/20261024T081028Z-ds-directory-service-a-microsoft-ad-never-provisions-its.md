Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds__ec2.microsoft-ad-creates-a-controllers-security-group-and-one-eni-per-subnet-in-the-vpc ds__ec2.microsoft-ad-dns-ips-one-per-chosen-subnet-inside-its-cidr
Issue-Key: ds__ec2.microsoft-ad-creates-a-controllers-security-group-and-one-eni-per-subnet-in-the-vpc|_poll#5|Timeout|time.sleep(interval_s)
Issue-Key: ds__ec2.microsoft-ad-dns-ips-one-per-chosen-subnet-inside-its-cidr|_poll#4|Timeout|time.sleep(interval_s)
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# Directory Service: a Microsoft AD never provisions its EC2-side network resources, so the controllers security group / ENIs and DnsIpAddrs never appear and callers hang

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMicrosoftAD

## Description
Two net-new root causes in the Directory Service emulator. Both manifest as a check that makes no progress and is killed by the harness at its 60 s budget - the harness emits only "Check timed out after 60.0s" with no exception frame, which is the signature of a poll loop waiting on a state the emulator never reaches.

Cause 1 of 2: CreateMicrosoftAD does not provision the directory's EC2 resources, so EC2's view of the directory is permanently empty.
AWS: recorded behavior only (the ds Smithy model describes the envelope, not the cross-service effect). Directory Service provisions EC2 resources in the caller's VPC on the caller's behalf: after CreateMicrosoftAD, DescribeDirectories reports VpcSettings.SecurityGroupId naming a security group that EXISTS in EC2 inside the subnets' VPC and whose name carries the directory id, and EC2 holds exactly one network interface per chosen subnet naming the directory. The check polls EC2 for these.
Emulator: the poll never satisfies - the group and the one-ENI-per-subnet set never materialise in EC2 - and the check is killed at 60 s. Either the ids DescribeDirectories reports are synthetic (EC2 never heard of them) or no EC2 resources are created at all.
Why: anything that reaches into the directory's network - adding a rule to the controllers security group, resolving the controllers' ENIs, or simply reconciling the directory against EC2 - finds nothing, and a caller that waits for the ENIs (as AWS callers must, since they appear during Creating) hangs rather than being served.
Fix: the CreateMicrosoftAD handler's cross-service provisioning in services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_microsoft_ad.rs - it must mint the controllers security group and one ENI per requested subnet in the EC2 service state and report THOSE ids in VpcSettings.
Traces: ds__ec2.microsoft-ad-creates-a-controllers-security-group-and-one-eni-per-subnet-in-the-vpc.

Cause 2 of 2: DnsIpAddrs is never populated with one address derived from each chosen subnet.
AWS: recorded behavior only. Once the domain controllers are placed, DescribeDirectories reports DnsIpAddrs holding two addresses, one CONTAINED IN each of the two subnets the caller passed - the addresses are derived by the service from those subnets, not invented. The list is legitimately empty for the first seconds of Creating, so the check polls for it.
Emulator: the poll never satisfies and the check is killed at 60 s - DnsIpAddrs never reaches two addresses, one per chosen subnet CIDR.
Why: DnsIpAddrs is the only way a client learns where to point its resolvers; a client that waits for them (the documented pattern, because they appear asynchronously) never proceeds, and anything asserting containment in the requested subnets cannot be satisfied.
Fix: the directory state machine's controller placement / address derivation in the ds service state (services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/state.rs) feeding DescribeDirectories - allocate one address from each requested subnet's CIDR as the controllers are placed.
Note: an open issue 20261024T015148Z-ds-directory-service-a-new-microsoft-ad-is-instantly.md covers the directory's lifecycle STAGE being reported instantly rather than progressing, and owns other ds checks; it does not carry either replay key here and does not cover the missing EC2-side resources or DnsIpAddrs derivation. It is named so a fixer can land the directory state machine changes together. These two causes may also share a root with the already-filed unimplemented EC2 ENI operations (DescribeNetworkInterfaces / CreateNetworkInterface); if so, re-replaying the two check ids after those land will show it.
Traces: ds__ec2.microsoft-ad-dns-ips-one-per-chosen-subnet-inside-its-cidr.

## Evidence
Cause 1 - ds__ec2.microsoft-ad-creates-a-controllers-security-group-and-one-eni-per-subnet-in-the-vpc: "Error: Check timed out after 60.0s" (no frame). The check asserts, through EC2's own view, that exactly one security group tagged with the directory exists in the subnets' VPC and that one network interface per chosen subnet names the directory.
Cause 2 - ds__ec2.microsoft-ad-dns-ips-one-per-chosen-subnet-inside-its-cidr: "Error: Check timed out after 60.0s" (no frame). The check polls DescribeDirectories for DnsIpAddrs and asserts two addresses, each inside a different chosen subnet's CIDR (a containment assertion, not a literal).
Both checks build their own VPC (estate:none), so no account-specific or live-only value is involved. VPC/subnet ids and addresses differ per run and are not requirements; the requirements are the existence of the EC2-side group and per-subnet ENIs, and one DnsIpAddrs entry contained in each requested subnet.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite().
2. Cause 1: create a VPC with two subnets in different AZs, call ds.CreateMicrosoftAD naming both subnets, read VpcSettings.SecurityGroupId from ds.DescribeDirectories, then ec2.DescribeSecurityGroups on that id and ec2.DescribeNetworkInterfaces filtered to the directory -> expect the group in the subnets' VPC and one ENI per subnet; observe nothing, and the poll never completes.
3. Cause 2: with the same directory, poll ds.DescribeDirectories for DnsIpAddrs -> expect two addresses, one inside each requested subnet CIDR; observe the list never reaches that state within the budget.
