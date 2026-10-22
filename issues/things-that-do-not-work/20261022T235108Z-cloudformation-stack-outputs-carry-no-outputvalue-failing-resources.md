Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.cfn-cidr-computed-blocks-accepted-by-ec2-and-bad-count-rejected cloudformation__ec2.getazs-resolves-to-and-tracks-the-default-subnet-set cloudformation__ec2__sqs__ssm.cfn-yaml-short-form-intrinsics-shape-resources cloudformation__ec2__ssm.deploy-ec2-3-group-rename-replaces-the-group-and-source-group-rule-still-drops-an-unrelated-instance cloudformation__ec2.deploy-ec2-1-bad-ami-instance-rolls-back-the-network-in-reverse-order cloudformation__ec2.deploy-ec2-1-overlapping-subnet-cidr-rejected-and-rolled-back cloudformation__ec2__rds.cfn-rds-db-subnet-group-single-az-rejected-negative cloudformation__ec2.deploy-ec2-1-volume-deletion-policy-snapshot-vs-retain cloudformation__ec2__ssm.cli-script-cfn-stack-delete-removes-ssm-association-positive cloudformation__ec2__ssm.sdk-ssm-encrypted-parameter-used-in-ec2-launch-positive cloudformation__ec2__ssm.sdk-ssm-parameter-policy-restricts-ec2-cross-account-read-negative
Issue-Key: cloudformation__ec2.cfn-cidr-computed-blocks-accepted-by-ec2-and-bad-count-rejected|run#10|KeyError|out = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__ec2.getazs-resolves-to-and-tracks-the-default-subnet-set|_evaluate#10|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in described.get("Outputs") or []}
Issue-Key: cloudformation__ec2__sqs__ssm.cfn-yaml-short-form-intrinsics-shape-resources|_deploy_cfn#10|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__ec2__ssm.deploy-ec2-3-group-rename-replaces-the-group-and-source-group-rule-still-drops-an-unrelated-instance|_outputs#13|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in _stack(cfn, stack_id).get("Outputs", [])}
Issue-Key: cloudformation__ec2.deploy-ec2-1-bad-ami-instance-rolls-back-the-network-in-reverse-order|run#8|AssertionError|assert stack["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__ec2.deploy-ec2-1-overlapping-subnet-cidr-rejected-and-rolled-back|run#7|AssertionError|assert stack["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__ec2__rds.cfn-rds-db-subnet-group-single-az-rejected-negative|run#8|AssertionError|assert failed["status"] == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__ec2|DescribeVolumes|InvalidVolumeID.Malformed
Issue-Key: ssm|DescribeAssociation|ValidationException
Issue-Key: cloudformation__ec2__ssm.sdk-ssm-encrypted-parameter-used-in-ec2-launch-positive|run#9|AssertionError|assert ciphertext["Value"] != secret_value, "SecureString value was not encrypted at rest"
Issue-Key: cloudformation__ec2__ssm.sdk-ssm-parameter-policy-restricts-ec2-cross-account-read-negative|_error#14|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# CloudFormation: stack Outputs carry no OutputValue, failing resources never roll the stack back, and physical resource ids are synthesized names (5 causes)

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Five distinct root causes on the CloudFormation-led behavioral surface. A fixer must close all five before submitting.

Cause 1 of 5: DescribeStacks returns Outputs entries without OutputValue.
AWS: DescribeStacks on a CREATE_COMPLETE stack returns Stacks[].Outputs[] where each Output carries OutputKey and the RESOLVED OutputValue (Smithy com-amazonaws-cloudformation-cloudformation: Output members OutputKey/OutputValue/Description/ExportName).
Emulator: every consumer of a stack output raises KeyError: 'OutputValue' — the Output entry exists but the value member is absent.
Why: any client that deploys a stack and then reads an output to drive the next call (subnet id, volume id, instance id) cannot proceed at all; the whole deploy-then-use pattern is dead.
Fix: the DescribeStacks handler/serializer and the stack-output resolution in CloudFormation state.
Traces: cloudformation__ec2.cfn-cidr-computed-blocks-accepted-by-ec2-and-bad-count-rejected; cloudformation__ec2.getazs-resolves-to-and-tracks-the-default-subnet-set; cloudformation__ec2__sqs__ssm.cfn-yaml-short-form-intrinsics-shape-resources; cloudformation__ec2__ssm.deploy-ec2-3-group-rename-replaces-the-group-and-source-group-rule-still-drops-an-unrelated-instance. Open issue 20261022T202249Z-autoscaling-cloudformation-ec2-cloudformation-describestacks-returns-stack-outputs-entries-without-outputvalue.md describes the same symptom for the autoscaling-led checks and owns those; the four checks above are NOT carried by it.

Cause 2 of 5: a stack whose resource cannot be created still reaches CREATE_COMPLETE instead of rolling back.
AWS: when a resource creation fails, CloudFormation moves the stack CREATE_FAILED -> ROLLBACK_IN_PROGRESS -> ROLLBACK_COMPLETE and deletes the resources it created, in reverse dependency order (recorded behavior only for the specific templates; the terminal state ROLLBACK_COMPLETE is the modeled StackStatus).
Emulator: three stacks that must fail are reported CREATE_COMPLETE — one declaring an EC2 instance with a nonexistent AMI, one declaring subnets 10.72.0.0/25 and 10.72.0.0/24 inside one VPC, and one declaring an RDS DB subnet group with a single AZ.
Why: a client cannot distinguish a good deployment from a broken one, and negative-path infrastructure tests pass against invalid templates.
Fix: resource provisioning in the CloudFormation stack engine must propagate per-resource create failures into stack state and run the rollback path.
Traces: cloudformation__ec2.deploy-ec2-1-bad-ami-instance-rolls-back-the-network-in-reverse-order ('a stack whose instance names a nonexistent AMI ended CREATE_COMPLETE, expected ROLLBACK_COMPLETE'); cloudformation__ec2.deploy-ec2-1-overlapping-subnet-cidr-rejected-and-rolled-back; cloudformation__ec2__rds.cfn-rds-db-subnet-group-single-az-rejected-negative.

Cause 3 of 5: physical resource ids of CFN-created resources are synthesized logical names, not service-issued ids.
AWS: Ref/GetAtt on AWS::EC2::Volume resolves to a vol-<17 hex> id and on AWS::SSM::Association to a UUID associationId; those ids are accepted by the owning service.
Emulator: the volume resolves to 'sim2real-xsvc-ec2vol-27af355a7e43-SnapVol-DE69CB08BE1B', and EC2 DescribeVolumes refuses it with InvalidVolumeID.Malformed ('Invalid id: ...'); the SSM association resolves to the document name 'AWS-RunShellScript', and SSM DescribeAssociation refuses it with ValidationException for failing the associationId UUID pattern.
Why: every stack-then-inspect flow breaks: the id a stack hands back cannot be used against the service that supposedly owns the resource.
Fix: physical-id assignment in the CloudFormation resource provisioners must come from the real create call of the owning service.
Traces: cloudformation__ec2.deploy-ec2-1-volume-deletion-policy-snapshot-vs-retain; cloudformation__ec2__ssm.cli-script-cfn-stack-delete-removes-ssm-association-positive. Related open issues naming the same family for other resource types: 20261022T154846Z-ec2-emr-cloudformation-stack-outputs-return-synthesized-logical-names-instead.md, 20261022T175927Z-ds-ec2-e2e-cloudformation-reports-create-complete-but.md.

Cause 4 of 5: SSM SecureString parameters are not encrypted at rest.
AWS: PutParameter with Type=SecureString stores the value encrypted under a KMS key; GetParameter without WithDecryption returns the ciphertext, so the plaintext is never recoverable from the stored form.
Emulator: the check reads the stored value and finds the plaintext — 'SecureString value was not encrypted at rest'.
Why: a client relying on SecureString for secret material gets no confidentiality boundary, and the WithDecryption distinction is unobservable.
Fix: SSM parameter state/PutParameter handling for Type=SecureString.
Traces: cloudformation__ec2__ssm.sdk-ssm-encrypted-parameter-used-in-ec2-launch-positive.

Cause 5 of 5: a restricted SSM parameter is readable when AWS refuses the read.
AWS: the scoped read in this scenario is refused with a ClientError (recorded behavior only for the exact code).
Emulator: GetParameter returns HTTP 200 with the full parameter ({'Name': '/sim2real/restricted-...', 'Value': 'sim2real-value-...'}) where a ClientError was required.
Why: authorization/parameter-policy scoping is not enforced, so a client cannot test or rely on a deny boundary.
Fix: SSM GetParameter authorization/parameter-policy evaluation.
Traces: cloudformation__ec2__ssm.sdk-ssm-parameter-policy-restricts-ec2-cross-account-read-negative.

## Evidence
Cause 1 — expected: Outputs[].OutputValue present on a CREATE_COMPLETE stack; actual: KeyError: 'OutputValue' raised by every output consumer (4 checks).
Cause 2 — expected: ROLLBACK_COMPLETE; actual: "a stack whose instance names a nonexistent AMI ended 'CREATE_COMPLETE' (None), expected 'ROLLBACK_COMPLETE'"; "a stack declaring 10.72.0.0/25 beside 10.72.0.0/24 in one VPC ended 'CREATE_COMPLETE'"; "stack ended in CREATE_COMPLETE; a resource that cannot be created must roll the stack back to ROLLBACK_COMPLETE".
Cause 3 — actual: botocore ClientError (InvalidVolumeID.Malformed) from DescribeVolumes: "Invalid id: 'sim2real-xsvc-ec2vol-27af355a7e43-SnapVol-DE69CB08BE1B'"; botocore ValidationException from DescribeAssociation: "Value 'AWS-RunShellScript' at 'associationId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-fA-F]{8}-...".
Cause 4 — actual: AssertionError: SecureString value was not encrypted at rest.
Cause 5 — actual: AssertionError: expected a ClientError, but the call succeeded and returned {'Parameter': {...'Value': 'sim2real-value-962929ef4db5'...}, 'ResponseMetadata': {'HTTPStatusCode': 200}}.

## Steps to Reproduce
Replay the behavioral suite and re-run the check ids in trace_ids. Minimal repros: (1) CreateStack with any template declaring an Output, wait for CREATE_COMPLETE, DescribeStacks and read Stacks[0].Outputs[0].OutputValue. (2) CreateStack with an AWS::EC2::Instance whose ImageId does not exist and poll StackStatus. (3) CreateStack with an AWS::EC2::Volume, read its Ref, and pass it to ec2:DescribeVolumes. (4) ssm:PutParameter Type=SecureString then read the stored value without WithDecryption. (5) ssm:GetParameter on the restricted parameter from the scoped principal.
