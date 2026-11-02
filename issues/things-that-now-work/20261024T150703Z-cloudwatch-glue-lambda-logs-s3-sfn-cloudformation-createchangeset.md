Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__glue__lambda__logs__s3__stepfunctions.cdk-bootstrapped-sfn-etl-workflow
Issue-Key: cloudwatch__glue__lambda__logs__s3__stepfunctions.cdk-bootstrapped-sfn-etl-workflow|_cdk_failed#8|AssertionError|raise AssertionError(f"{STATE_MARKER}: {what} rc={done.returncode}: {tail}")
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# cloudwatch__glue__lambda__logs__s3__sfn: CloudFormation CreateChangeSet with TemplateURL is unimplemented, so no CDK-bootstrapped deploy can run (1 cause)

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: CreateChangeSet does not accept TemplateURL (a template staged in S3) ===
AWS: cloudformation CreateChangeSet accepts the template either inline as TemplateBody or by reference as TemplateURL pointing at an S3 object. TemplateURL is the form the CDK always uses: `cdk deploy` stages the synthesized template (and its assets) into the bootstrap bucket and then creates a change set referring to it, so TemplateURL is on the only path a CDK application has to a deployed stack.
Emulator: CreateChangeSet with TemplateURL answers NotImplemented — 'operation CreateChangeSet with TemplateURL (fetching a template from Amazon S3) is scaffolded but not implemented in this emulator' — and `cdk deploy` exits rc=1 after synthesis and asset build. The whole Step Functions ETL workflow the check grades (Glue job, Lambda, S3, SFN state machine, CloudWatch logging) is therefore never deployed.
Why: every CDK-driven workflow is unreachable. A client that uses the CDK — the normal way these stacks are authored — cannot deploy anything, regardless of what the per-resource CloudFormation support does, and the failure appears as a generic cdk rc=1 rather than as a named gap.
Fix: the CloudFormation CreateChangeSet handler — resolve TemplateURL by fetching the object from the emulator's S3 and then proceed exactly as for TemplateBody.
Traces: cloudwatch__glue__lambda__logs__s3__sfn.cdk-bootstrapped-sfn-etl-workflow.

## Evidence
cloudwatch__glue__lambda__logs__s3__sfn.cdk-bootstrapped-sfn-etl-workflow: AssertionError: TARGET_STATE_UNIMPLEMENTED: cdk deploy sim2real-xsvc-cdkstk-<id>-main rc=1: operation CreateChangeSet with TemplateURL (fetching a template from Amazon S3) is scaffolded but not implemented in this emulator — reported after 'Synthesis time: 0.01s' and 'success: Built Extract/Code', i.e. synthesis and asset staging succeeded and only the change-set creation failed. Expected the change set to be created from the staged template and the stack to deploy.

## Steps to Reproduce
Bootstrap a CDK environment against the emulator, then `cdk deploy` any stack (the CLI uploads the template to the bootstrap bucket and calls cloudformation CreateChangeSet with TemplateURL). Equivalently: s3 PutObject a template, then cloudformation CreateChangeSet(StackName=..., ChangeSetName=..., TemplateURL='https://<bucket>.s3.<region>.amazonaws.com/<key>', ChangeSetType='CREATE') — answers NotImplemented.
