Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds__ec2.e2e-managed-ad-with-ldaps-and-resets-lifecycle
Issue-Key: ds__ec2.e2e-managed-ad-with-ldaps-and-resets-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 52a79640a1ceceb858c6fc834fcbcb6a886924f9

# ds: an ACTIVE Microsoft AD serves no DNS addresses — DescribeDirectories reports an empty DnsIpAddrs, so the end-to-end LDAPS/reset lifecycle cannot resolve the domain (1 cause)

> _Note: this issue was identified against the code base at commit `52a79640a1ceceb858c6fc834fcbcb6a886924f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDirectories

## Description
Cause 1 of 1: a Managed Microsoft AD that has reached Stage=Active is reported with NO DNS server addresses. DescribeDirectories answers `DnsIpAddrs` as an empty list instead of one address per subnet the directory was created in, drawn from that subnet's CIDR.

AWS: `CreateMicrosoftAD` places one domain controller in each of the two subnets named in `VpcSettings.SubnetIds`, and each controller's address is a DNS server of the directory. Smithy: `DirectoryDescription$DnsIpAddrs` — "The IP addresses of the DNS servers for the directory. For a Simple AD or Microsoft AD directory, these are the IP addresses of the Simple AD or Microsoft AD directory servers." (`smithy_query(query="doc", shape="DirectoryDescription", member="DnsIpAddrs", model_path="services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json")`). So once `Stage` is `Active`, the member is populated: one address per chosen subnet, each inside that subnet's own CIDR block. The number of addresses and the CIDRs they must fall in are derived from the request's subnets and the VPC's state — the concrete addresses themselves are environment values, never fixed text.

Emulator: the directory reaches `Active` but `DnsIpAddrs` stays empty. The replay of the four-phase lifecycle fails in its solve phase with `AssertionError: [phase 2/4 solve] an Active directory serves DNS addresses [] that should sit in [IPv4Network('10.168.0.0/24'), IPv4Network('10.168.1.0/24')]` — i.e. the member is present but holds zero entries while two were required, one per subnet of the deployed three-subnet VPC.

Why: `DnsIpAddrs` is the only thing a client can point a resolver, a DHCP option set, an `ldaps://` endpoint or a domain-join at. With it empty, every consumer of a newly created directory is blocked: a caller cannot build a Route 53 Resolver rule or VPC DHCP options for the domain, cannot reach LDAPS after RegisterCertificate/EnableLDAPS, and cannot verify a password reset against a controller. It also reads as a successfully provisioned directory (Stage=Active), so the failure surfaces far from its cause.

Note: the open issue 20261024T081028Z-ds-directory-service-a-microsoft-ad-never-provisions-its.md carries the single-operation check for this member (ds__ec2.microsoft-ad-dns-ips-one-per-chosen-subnet-inside-its-cidr); this lifecycle check is NOT recorded against it, so it is filed here and that issue is named rather than assumed to cover it.

Fix: the ds directory state/provisioning path — whatever moves a created directory to Stage=Active must, at the same time, allocate one controller address per subnet of `VpcSettings.SubnetIds` from that subnet's CIDR and store it so `DescribeDirectories` projects it into `DirectoryDescription.DnsIpAddrs` (and so the directory's ENIs/controllers agree with it).

Traces: ds__ec2.e2e-managed-ad-with-ldaps-and-resets-lifecycle (phase 2/4, solve).

## Evidence
Expected (AWS): after CreateMicrosoftAD with VpcSettings.SubnetIds naming two subnets of a VPC (here 10.168.0.0/24 and 10.168.1.0/24) and once DescribeDirectories reports Stage='Active', DirectoryDescription.DnsIpAddrs holds exactly one address per named subnet, each inside that subnet's CIDR block.

Actual (emulator): DnsIpAddrs == [] for the Active directory. Replay detail:
  File "check_ds__ec2_e2e_managed_ad_with_ldaps_and_resets_lifecycle.py", line 266, in phase
  AssertionError: [phase 2/4 solve] an Active directory serves DNS addresses [] that should sit in [IPv4Network('10.168.0.0/24'), IPv4Network('10.168.1.0/24')]

The CIDRs above are this run's VPC values and are NOT the requirement; the requirement is one address per subnet named in the request, each within that subnet's CIDR. No status code or error code diverges — the call succeeds and under-reports a modeled member.

## Steps to Reproduce
1. Deploy the three-subnet VPC the check uses (CloudFormation template deployments/ldaps-directory-network/template.json) and read its subnets with ec2 DescribeSubnets.
2. ds CreateMicrosoftAD(Name=<fqdn>, Password=<throwaway>, Edition='Standard', VpcSettings={'VpcId': <vpc>, 'SubnetIds': [<subnet-a>, <subnet-b>]}).
3. Poll ds DescribeDirectories(DirectoryIds=[<id>]) until DirectoryDescriptions[0].Stage == 'Active'.
4. Read DirectoryDescriptions[0].DnsIpAddrs: AWS returns two addresses, one inside each of <subnet-a>'s and <subnet-b>'s CidrBlock. The emulator returns [].
Replay: ds__ec2.e2e-managed-ad-with-ldaps-and-resets-lifecycle
