Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-trajwire-deb-deb-al2023-ami-lookup-ssm-and-describe-images
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-trajwire-deb-deb-al2023-ami-lookup-ssm-and-describe-images
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ssm|GetParameter|2xx->4xx|$.Parameter
Issue-Related: 20261030T030533Z-ssm-getparameter-cannot-resolve-aws-published-aws-parameters.md
Issue-Commit: 16c881ed26ef4e30669bd1a48293a88cc368ef87

# SSM GetParameter cannot resolve AWS-published public /aws/service AMI parameters

> _Note: this issue was identified against the code base at commit `16c881ed26ef4e30669bd1a48293a88cc368ef87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameter

## Description
Cause 1 of 1: a GetParameter read of an AWS-published public parameter under /aws/service/... is refused instead of answered.
AWS: Smithy (com.amazonaws.ssm) GetParameter - input Name is @required with @length min 1 max 2048, optional WithDecryption; output carries Parameter; modelled errors InternalServerError (500), InvalidKeyId (400), ParameterNotFound (client, 400), ParameterVersionNotFound (400). AWS publishes read-only public parameters in the /aws/service/... namespace of every commercial region (the Amazon Linux AMI alias namespace /aws/service/ami-amazon-linux-latest/... among them), and GetParameter on such a name answers 200 with Parameter.Name echoing the requested name, Type 'String', DataType 'text', Value the current AMI id for that alias in the called region, a Version (integer, server-assigned), a LastModifiedDate, and ARN in the public-parameter form arn:aws:ssm:<region>::parameter<name> (empty account-id segment). ParameterNotFound is only correct for a name that exists in neither the caller's store nor the published namespace.
Emulator: the same request is answered 400 and the body carries no Parameter member at all (the replay records 'status: expected 200, got 400' and '$.Parameter: missing'), i.e. the published namespace is simply not present in the emulator's parameter store, so an existing public name is treated as a not-found read.
Why: the standard AMI-lookup idiom - resolve an AMI alias through SSM, then pass the returned id to DescribeImages/RunInstances - cannot run: the client gets a 400 for a read AWS always answers 200, and every downstream step that depends on the resolved AMI id is skipped or fails. A client cannot distinguish 'this alias does not exist' from 'this emulator has no published parameters'.
Fix: the ssm GetParameter handler and the parameter-store state it reads (the ssm service crate) - seed/resolve the AWS-published /aws/service/... namespace and return it with Type String, DataType text, a monotonic Version, LastModifiedDate and the public-parameter ARN form; reserve the 400 ParameterNotFound refusal (message 'Parameter <name> not found.', where <name> is the requested Name and the rest is fixed text) for names that genuinely do not exist.
Traces: ec2-trajwire-deb-deb-al2023-ami-lookup-ssm-and-describe-images#task#1.
Related open issues that do NOT carry this replay key (the replay reports owned_by empty for this cluster), named so a fixer can join them up: 20261030T030533Z-ssm-getparameter-cannot-resolve-aws-published-aws-parameters.md, 20261029T204746Z-pcs-the-aws-published-public-ssm-parameter-namespace-aws.md, 20261022T093447Z-fis-4-fidelity-divergences-cloudformation-aws-fis-experimenttemplate.md.

## Evidence
Replay verdict mismatch, service ssm, operation GetParameter, fixture ec2-trajwire-deb-deb-al2023-ami-lookup-ssm-and-describe-images step task#1.
expected_status 200, actual_status 400.
detail: 'status: expected 200, got 400' and '$.Parameter: missing (expected {ARN: arn:aws:ssm:us-west-2::parameter/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64, DataType: text, LastModifiedDate: <timestamp>, Name: /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64, Type: String, Value: <ami-id>, Version: <integer>}, got None)'.
The recorded ARN, AMI id, version number and timestamp are per-environment values that fill the template; the requirements are the 200 status, the presence of Parameter, Type 'String', DataType 'text', Name echoing the request, and the public-parameter ARN shape arn:aws:ssm:<region>::parameter<name>.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture ec2-trajwire-deb-deb-al2023-ami-lookup-ssm-and-describe-images, step task#1: call ssm GetParameter with Name=/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64 (no WithDecryption).
3. Observe HTTP 400 with no Parameter member, where the recording has HTTP 200 and a Parameter whose Value is the resolved AMI id.
4. The later steps of the same fixture feed that id to DescribeImages, so they cannot be exercised until the read succeeds.
