Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.deploy-codepipeline-cfn-execution-behaves codepipeline.deploy-codepipeline-cloudformation-executes codepipeline.deploy-codepipeline-pipeline-executes-via-cloudformation codepipeline.deploy-pipeline-via-cloudformation-behaves
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: codepipeline|GetPipeline|PipelineNotFoundException
Issue-Key: codepipeline|PutObject|NoSuchBucket

# CloudFormation stacks report CREATE_COMPLETE but provision none of the resources their template declares

> _Note: this record is the cause 8 of
> `issues/things-that-now-work/20261022T173520Z-codepipeline-executions-never-progress-structural-validation-absent-error.md`,
> filed on its own because the defect is in the CloudFormation crate rather than in
> CodePipeline, and the other eight causes of that issue are closed. The four traces
> above keep failing until this one is fixed. The code base may have drifted since, so
> trace it back before editing._

## Operation
CreateStack

## Description
AWS: a `CreateStack` whose template declares `AWS::CodePipeline::Pipeline`,
`AWS::S3::Bucket` (with a `VersioningConfiguration`), `AWS::IAM::Role` and the like
reaches `CREATE_COMPLETE` only once those resources EXIST and are addressable by their
physical ids, and `DescribeStacks` then carries an `OutputValue` per declared `Output`.
A caller that deploys a stack and then calls the owning service — `GetPipeline` on the
declared pipeline, `PutObject` into the declared bucket — reaches the resources the
stack made.

Emulator: the stack reports completion, but nothing was provisioned.
`services/service-com-amazonaws-cloudformation-cloudformation/src/state/template.rs::provision`
turns each declared resource into a `StoredResource` with the physical id
CloudFormation's naming rules would give it and the attributes its "Return values"
section publishes — and stops there: no call is made into the service that owns the
resource type, so no bucket, role or pipeline comes into existence. Recorded follow-up
calls fail as a genuinely absent resource: `GetPipeline` answers
`PipelineNotFoundException` for the template's pipeline, and S3 `PutObject` answers
`NoSuchBucket` for the template's bucket.

Why: every infrastructure-as-code path is broken — a client deploys a stack, sees
success, and then every reference to a stack resource 404s. The error codes are correct
for "absent", so the divergence is the provisioning, not the serializer.

Fix: the CloudFormation stack-resource provisioner — a per-resource-type handler that
CREATES the underlying resource in the service that owns it (the framework's in-process
callback is the seam: `ctx.callback().aws_credentials_provider()` /
`ctx.callback().http_client()` with an `aws-sdk-<service>` client, as
`services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/state/driver.rs`
and `services/service-com-amazonaws-b2bi-b2bi/src/ops/create_profile.rs` do), records
the physical id the owning service minted, and populates the stack's `Outputs` from it.
A resource type with no such handler must not be reported as created: the stack reaches
the modelled failure state naming the type it could not provision, rather than
`CREATE_COMPLETE` over resources that do not exist.

## Evidence
`.../src/state/template.rs::provision` maps each declared resource to
`StoredResource { logical_id, resource_type, physical_resource_id, attributes }` and
makes no cross-service call; `DescribeStacks` then reports `CREATE_COMPLETE`.
Recorded follow-ups from the codepipeline behavioral sweep:
`PipelineNotFoundException ... pipeline sim2real-cp-cfn-0b98157ee88c does not exist`
and `NoSuchBucket ... when calling the PutObject operation`.

## Steps to Reproduce
1. Boot the emulator.
2. `CreateStack` with a template declaring `AWS::S3::Bucket` (with
   `VersioningConfiguration: {Status: Enabled}`), `AWS::IAM::Role` and
   `AWS::CodePipeline::Pipeline`, and an `Outputs` section naming them.
3. Wait for `DescribeStacks` to report `CREATE_COMPLETE`.
4. Call `codepipeline:GetPipeline` on the declared pipeline name — observe
   `PipelineNotFoundException` — and `s3:PutObject` into the declared bucket — observe
   `NoSuchBucket`.
