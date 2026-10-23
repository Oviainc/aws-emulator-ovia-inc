Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-capacity-block-reservation-for-ml-workloads
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-capacity-block-reservation-for-ml-workloads ec2-page-create-volume-client-token-idempotency ec2-page-describe-security-groups-vpc-scoped-trailing-empty-page ec2-page-describe-subnets-vpc-scoped-two-pages ec2-run-instances-validation-and-diagnostic-interrupt
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeCapacityBlockOfferings|4xx->2xx|$.Response
Issue-Key: ec2|DescribeCapacityBlockOfferings|4xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeCapacityBlockOfferings|4xx->2xx|status
Issue-Key: ec2|CreateVolume|4xx->2xx|$.Response
Issue-Key: ec2|CreateVolume|4xx->2xx|$headers.content-type
Issue-Key: ec2|CreateVolume|4xx->2xx|status
Issue-Key: ec2|DescribeSecurityGroups|4xx->2xx|$.Response
Issue-Key: ec2|DescribeSecurityGroups|4xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeSecurityGroups|4xx->2xx|status
Issue-Key: ec2|DescribeSubnets|4xx->2xx|$.Response
Issue-Key: ec2|DescribeSubnets|4xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeSubnets|4xx->2xx|status
Issue-Key: ec2|RunInstances|4xx->2xx|$.Response
Issue-Key: ec2|RunInstances|4xx->2xx|$headers.content-type
Issue-Key: ec2|RunInstances|4xx->2xx|status
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# EC2 skips input validation and accepts requests AWS refuses with 400

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Five independent request forms that AWS rejects with HTTP 400 are answered HTTP 200 by the emulator, with no Response/Errors element at all. All five must be closed before submitting. Background: EC2 models no error shapes (Smithy com-amazonaws-ec2-amazonec2: every operation queried reports errors: []), so the status plus the ec2Query error Code is the whole contract; answering 200 means the client is told a malformed or conflicting request succeeded.

Cause 1 of 5: DescribeCapacityBlockOfferings accepts a request that sets both the ultraserver and the instance dimensions.
AWS: HTTP 400 with Code InvalidParameterValue; recorded message example 'Invalid inputs, either ultraserver-type and ultraserver-count, or instance-type and instance-count needs to be set, not both'.
Emulator: HTTP 200.
Why: the client gets an offerings list for a request AWS would never price, so capacity planning proceeds on invented data.
Fix: mutually-exclusive-parameter validation in the DescribeCapacityBlockOfferings handler.
Traces: ec2-capacity-block-reservation-for-ml-workloads#task#1.

Cause 2 of 5: CreateVolume does not detect an idempotency-token conflict.
AWS: HTTP 400 with Code IdempotentParameterMismatch when the same ClientToken is reused with different parameters; recorded message example 'Parameters on this idempotent request are inconsistent with parameters used in previous request(s)'. Smithy: CreateVolume.ClientToken carries idempotencyToken.
Emulator: HTTP 200 - a second, differently-shaped volume is created.
Why: retry-safe clients silently create extra volumes instead of being told their token is being reused inconsistently.
Fix: client-token bookkeeping in the CreateVolume handler / volume state.
Traces: ec2-page-create-volume-client-token-idempotency#task#4.

Cause 3 of 5: DescribeSecurityGroups accepts GroupIds together with MaxResults.
AWS: HTTP 400 with Code InvalidParameterCombination; recorded message example 'The parameter securityGroupIdSet cannot be used with the parameter maxResults'.
Emulator: HTTP 200.
Why: a paginating client that also filters by id is never told the combination is invalid, so its paging loop behaves differently against the emulator than against AWS.
Fix: parameter-combination validation in the DescribeSecurityGroups handler.
Traces: ec2-page-describe-security-groups-vpc-scoped-trailing-empty-page#task#10.

Cause 4 of 5: DescribeSubnets accepts SubnetIds together with MaxResults.
AWS: HTTP 400 with Code InvalidParameterCombination; recorded message example 'The parameter SubnetIds cannot be used with the parameter MaxResults'.
Emulator: HTTP 200.
Why and Fix: as cause 3, in the DescribeSubnets handler.
Traces: ec2-page-describe-subnets-vpc-scoped-two-pages#task#11.

Cause 5 of 5: RunInstances accepts a malformed AMI id.
AWS: HTTP 400 with Code InvalidAMIID.Malformed - the id syntax is validated before lookup; recorded message example 'Invalid id: "ami-1234567890abcdef0" (expecting "ami-...")'.
Emulator: HTTP 200 - the instance launches from an id that cannot exist.
Why: a typo in an AMI id produces a running instance instead of a validation error, and the client cannot distinguish a typo from a missing image.
Fix: AMI-id syntax validation in the RunInstances handler before image lookup.
Traces: ec2-run-instances-validation-and-diagnostic-interrupt#task#6. Related open issue 20261022T235108Z-ec2-misclassifies-rejections-and-skips-validation-malformed-ids.md covers malformed-AMI handling on DescribeImages from the behavioral corpus; it carries no replay key for this RunInstances step.

## Evidence
Cause 1 - expected 400 with {'Errors': {'Error': {'Code': 'InvalidParameterValue', ...}}}, got 200 and $.Response missing. Cause 2 - expected 400 / IdempotentParameterMismatch, got 200 and $.Response missing. Cause 3 - expected 400 / InvalidParameterCombination, got 200 and $.Response missing. Cause 4 - expected 400 / InvalidParameterCombination, got 200 and $.Response missing. Cause 5 - expected 400 / InvalidAMIID.Malformed, got 200 and $.Response missing.

## Steps to Reproduce
Replay the fixture steps named under Traces. Minimal repros: (1) ec2:DescribeCapacityBlockOfferings with both UltraserverType/UltraserverCount and InstanceType/InstanceCount set. (2) ec2:CreateVolume twice with the same ClientToken but different Size. (3) ec2:DescribeSecurityGroups GroupIds=[...] MaxResults=5. (4) ec2:DescribeSubnets SubnetIds=[...] MaxResults=5. (5) ec2:RunInstances ImageId='ami-1234567890abcdef0'.
