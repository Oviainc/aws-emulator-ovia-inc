Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ssm.e2e-count-windows-server-managed-nodes-lifecycle ec2__ssm.e2e-trace-ec2-ssh-access-path-lifecycle ec2__ssm.vscan-e2e-ec-instance-security-vulnerability-scan-lifecycle
Issue-Key: ec2__ssm.e2e-count-windows-server-managed-nodes-lifecycle|phase#4|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: ec2__ssm.e2e-trace-ec2-ssh-access-path-lifecycle|phase#5|_PhaseError|raise _PhaseError(
Issue-Key: ec2__ssm.vscan-e2e-ec-instance-security-vulnerability-scan-lifecycle|phase#4|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 74acd3b5911f8cc01604f18ec0c28de145582648

# CloudFormation-deployed EC2/SSM estates are unusable: instance Refs resolve to name-form ids, stack Outputs omit OutputValue, and a valid stack settles in ROLLBACK_FAILED (3 causes)

> _Note: this issue was identified against the code base at commit `74acd3b5911f8cc01604f18ec0c28de145582648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Three end-to-end checks that deploy an EC2+SSM estate with CloudFormation all fail in their first (deploy) phase, each on a different defect in the CloudFormation surface they read the estate through. 3 CAUSES.

Cause 1 of 3 - an AWS::EC2::Instance Ref resolves to a synthesized name instead of the instance id.
AWS: for AWS::EC2::Instance the physical resource id IS the EC2 instance id, and Ref returns it, so a stack Output whose Value is {Ref: <instance>} has an OutputValue of the form i-<17 hex chars>. Recorded behavior only - the resource-provider contract is not in the Smithy model; the recording shows every node output reading back as an i- id that EC2 and SSM then accept.
Emulator: the Outputs carry stack-scoped name-form ids of the shape <stack-name>-<LogicalId>-<uppercase hash> (observed: four outputs, each like '<stack>-Windows2022Node-<HASH>', '<stack>-Ubuntu2204Node-<HASH>'), i.e. the generic 'name' physical id is minted for a resource type whose physical id is an id.
Why: a client that feeds a stack output into any instance-scoped API (DescribeInstances, SendCommand, DescribeInstanceInformation) is refused for a malformed instance id, so the entire deploy-then-operate flow a stack exists for cannot be driven from its own outputs.
Fix: the CloudFormation resource provider for AWS::EC2::Instance - its physical resource id must be the instance id returned by RunInstances, and Ref/Outputs resolution must use it. Related open issues (different resource types, so they do not cover this): 20261103T171935Z-cloudformation-synthesizes-physical-resource-ids-of-the-wrong.md and 20261103T171935Z-cloudformation-reports-resource-names-where-aws-reports-resource.md.
Traces: ec2__ssm.e2e-count-windows-server-managed-nodes-lifecycle.

Cause 2 of 3 - stack Outputs entries are served without OutputValue.
AWS: every resolved output of a stack in a COMPLETE state is reported as an Output structure carrying both OutputKey and OutputValue (Smithy: cloudformation#Output has members OutputKey, OutputValue, Description, ExportName; the service sets OutputValue for a resolved output).
Emulator: the Output entries omit the OutputValue member entirely, so a client indexing the documented member raises KeyError: 'OutputValue'.
Why: any consumer of stack outputs - the only supported way to hand a stack's created ids to the next step - crashes or sees an output with no value, and cannot distinguish 'not yet resolved' from 'absent'.
Fix: the DescribeStacks output serializer / the stack-output resolution that populates it. Related open issues naming the same member for other scenarios: 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md, 20261029T175114Z-cloudformation-stack-outputs-carry-no-outputvalue-an-asg.md.
Traces: ec2__ssm.e2e-trace-ec2-ssh-access-path-lifecycle.

Cause 3 of 3 - CreateStack on a valid EC2+SSM template settles in ROLLBACK_FAILED.
AWS: a CreateStack whose template and parameters are valid reaches CREATE_COMPLETE; ROLLBACK_FAILED is reached only after a resource failure AND a rollback that itself could not complete. Recorded behavior only (stack status values are the modeled StackStatus enum; which status a given template reaches is behavior).
Emulator: the stack reaches the terminal status 'ROLLBACK_FAILED', so the check never gets an estate at all. Hypothesis: one resource type in the template fails to create and the rollback of the already-created resources also fails, leaving the stack in the un-retryable state rather than surfacing the failing resource.
Why: the scenario is unrunnable, and ROLLBACK_FAILED is a status a client cannot recover from without ContinueUpdateRollback/delete, so a caller is pushed into an error path AWS would not have put it in.
Fix: stack creation and rollback in the CloudFormation engine for the resource types in this template (EC2 instance + SSM estate); the stack-event history must also name the resource that failed.
Traces: ec2__ssm.vscan-e2e-ec-instance-security-vulnerability-scan-lifecycle.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout; all three checks are listed under the suite's `### ec2__ssm` heading, and all three fail in phase 1 of 4 (deploy).

Cause 1 - ec2__ssm.e2e-count-windows-server-managed-nodes-lifecycle [mismatch]: "[phase 1/4 deploy] {'amazon-linux-2023': 'sim2real-w22n-e2e-65a24142bca9-AmazonLinux2023Node-F561BFF4D39A', 'ubuntu-22-04': 'sim2real-w22n-e2e-65a24142bca9-Ubuntu2204Node-BFAFEC0A4EEC', 'windows-2022-full': 'sim2real-w22n-e2e-65a24142bca9-Windows2022Node-5D6344F4A5FE', 'windows-2016-full': 'sim2real-w22n-e2e-65a24142bca9-Windows2016Node-788E53AF6AA7'}" - the check's assertion is `all(v.startswith("i-") for v in nodes.values())` over the four stack outputs; the dict shown is the diverging value. (Stack names, logical ids and hashes vary per run; the requirement is the i- id form, not these literals.)

Cause 2 - ec2__ssm.e2e-trace-ec2-ssh-access-path-lifecycle [error]: "[phase 1/4 deploy] KeyError: 'OutputValue'" raised while reading the deployed stack's outputs.

Cause 3 - ec2__ssm.vscan-e2e-ec-instance-security-vulnerability-scan-lifecycle [mismatch]: "[phase 1/4 deploy] stack sim2real-scn-vscan-706db5e810ff reached 'ROLLBACK_FAILED', not CREATE_COMPLETE - read its stack events for the resource that failed".

Smithy model consulted: services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json (Output structure: OutputKey, OutputValue, Description, ExportName).

## Steps to Reproduce
1. Clone this checkout and replay the behavioral suite (simparity-behavioral-aws-v1) against the booted emulator.
2. Cause 1: run check ec2__ssm.e2e-count-windows-server-managed-nodes-lifecycle - it deploys deployments/w22n-managed-node-fleet/template.json and asserts every stack Output that is a {Ref: <AWS::EC2::Instance>} reads back as an i- instance id.
3. Cause 2: run check ec2__ssm.e2e-trace-ec2-ssh-access-path-lifecycle - deploy its stack and read DescribeStacks Outputs; each entry must carry OutputValue alongside OutputKey.
4. Cause 3: run check ec2__ssm.vscan-e2e-ec-instance-security-vulnerability-scan-lifecycle - CreateStack on its EC2+SSM template must reach CREATE_COMPLETE; the emulator settles at ROLLBACK_FAILED. Read DescribeStackEvents for the failing resource.
