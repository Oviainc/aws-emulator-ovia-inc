Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__opensearch.xsvc-opensearch-3-subnets-across-two-vpcs-rejected
Issue-Key: ec2__opensearch.xsvc-opensearch-3-subnets-across-two-vpcs-rejected|run#20|AssertionError|raise AssertionError(
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# OpenSearch CreateDomain accepts VPCOptions subnets that span two different VPCs

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Cause 1 of 1: CreateDomain with VPCOptions.SubnetIds naming subnets from two different VPCs is accepted.
AWS: refused with ValidationException (HTTP 400), message 'The subnets must be in the same VPC.' — OpenSearch resolves every subnet id against EC2 and compares their parent VPCs; nothing is created, and replacing the foreign subnet with a second subnet of the first VPC is accepted. Smithy (opensearch): CreateDomain models ValidationException; the cross-VPC rule itself is recorded behavior only.
Emulator: the call is accepted and DomainStatus.VPCOptions reports the two foreign subnets {'SubnetIds': ['subnet-...024', 'subnet-...025'], 'SecurityGroupIds': ['sg-...027']}.
Why: a domain is created with an impossible network placement; the subnets each exist and are individually valid, so only the cross-VPC comparison catches it, and a client that branches on ValidationException to repair its subnet list never does.
Fix: resolve VPCOptions.SubnetIds against EC2 and compare parent VPCs in the CreateDomain handler's validation.
Traces: ec2__opensearch.xsvc-opensearch-3-subnets-across-two-vpcs-rejected.

## Evidence
AssertionError: CreateDomain with subnets in 'vpc-0000000000000001c' and 'vpc-00000000000000020' was accepted: {'SubnetIds': ['subnet-00000000000000024', 'subnet-00000000000000025'], 'SecurityGroupIds': ['sg-00000000000000027']} — the check asserts code == 'ValidationException' and message == 'The subnets must be in the same VPC.'

## Steps to Reproduce
Replay ec2__opensearch.xsvc-opensearch-3-subnets-across-two-vpcs-rejected. Minimal repro: create two VPCs each with a subnet, then opensearch:CreateDomain DomainName=<n> VPCOptions={'SubnetIds':[<subnet in vpc A>, <subnet in vpc B>],'SecurityGroupIds':[<sg in vpc A>]}.
