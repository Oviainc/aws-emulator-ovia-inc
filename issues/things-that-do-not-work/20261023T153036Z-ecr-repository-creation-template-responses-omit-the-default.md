Issue-Source: trace-sweep-divergence
Issue-Trace: api-repository-creation-template-lifecycle
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-repository-creation-template-lifecycle
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|CreateRepositoryCreationTemplate|2xx->2xx|$.repositoryCreationTemplate.encryptionConfiguration
Issue-Key: ecr|DescribeRepositoryCreationTemplates|2xx->2xx|$.repositoryCreationTemplates[*]
Issue-Key: ecr|UpdateRepositoryCreationTemplate|2xx->2xx|$.repositoryCreationTemplate.encryptionConfiguration
Issue-Key: ecr|DeleteRepositoryCreationTemplate|2xx->2xx|$.repositoryCreationTemplate.encryptionConfiguration
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# ECR repository creation template responses omit the default encryptionConfiguration

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRepositoryCreationTemplate

## Description
This issue bundles 4 DISTINCT root causes, one per operation that returns a RepositoryCreationTemplate. They share one fault: the template's encryptionConfiguration is never materialised, so the member is absent from every response. All four must be closed before submitting; each carries its own fixture and step.

AWS (common to all four): Smithy ecr#RepositoryCreationTemplate declares encryptionConfiguration targeting ecr#EncryptionConfigurationForRepositoryCreationTemplate, whose encryptionType member is REQUIRED (enum; AES256 means SSE-S3, KMS means SSE-KMS). The recording shows that a template created WITHOUT an explicit encryptionConfiguration is stored with the default {"encryptionType": "AES256"}, and every subsequent read of that template reports it — a template always has a resolved encryption posture, the same way a repository does.
Emulator (common to all four): returns the template with encryptionConfiguration absent entirely, on create, on describe, on update and on delete, i.e. the default is never resolved and never persisted.
Why (common): repository creation templates exist to pre-declare the configuration that pull-through-cache and replication-created repositories will inherit, and encryption posture is the part auditors and compliance tooling read. A client or policy check that inspects the template to confirm repositories will be encrypted sees nothing, and cannot distinguish "defaulted to AES256" from "unset/unknown". Because encryptionType is a required member of the structure, consumers generated from the model may also fail to deserialize a partially-present configuration.
Fix (common): resolve the encryption configuration at template-create time (defaulting encryptionType to AES256 when the request omits it), persist it in repository-creation-template state, and emit it from all four templates' serializers.

Cause 1 of 4: CreateRepositoryCreationTemplate omits it in the creation response. Traces: api-repository-creation-template-lifecycle#setup#1.
Cause 2 of 4: DescribeRepositoryCreationTemplates omits it in the listed template. Traces: api-repository-creation-template-lifecycle#task#2 (representative; api-repository-creation-template-lifecycle#task#5 is in the same cluster).
Cause 3 of 4: UpdateRepositoryCreationTemplate omits it in the update response, so an update cannot confirm the posture it preserved. Traces: api-repository-creation-template-lifecycle#task#4.
Cause 4 of 4: DeleteRepositoryCreationTemplate omits it in the response describing the template it removed. Traces: api-repository-creation-template-lifecycle#task#7.

## Evidence
All four steps returned status 200 on both sides; the divergence is a missing member.
Cause 1 — fixture api-repository-creation-template-lifecycle, step setup#1: $.repositoryCreationTemplate.encryptionConfiguration missing (expected {'encryptionType': 'AES256'}, got None).
Cause 2 — same fixture, step task#2: $.repositoryCreationTemplates[0] expected {'appliedFor': ['PULL_THROUGH_CACHE'], 'encryptionConfiguration': {'encryptionType': 'AES256'}, 'imageTagMutability': 'MUTABLE', 'prefix': '<prefix>', 'createdAt': ..., 'updatedAt': ...}; got the same structure with encryptionConfiguration absent (timestamps differ per environment and are not part of this finding).
Cause 3 — same fixture, step task#4: $.repositoryCreationTemplate.encryptionConfiguration missing (expected {'encryptionType': 'AES256'}, got None).
Cause 4 — same fixture, step task#7: $.repositoryCreationTemplate.encryptionConfiguration missing (expected {'encryptionType': 'AES256'}, got None).

## Steps to Reproduce
Replay the wire corpus and re-run api-repository-creation-template-lifecycle steps setup#1, task#2, task#4 and task#7. Manually:
1. CreateRepositoryCreationTemplate(prefix=P, appliedFor=['PULL_THROUGH_CACHE']) with NO encryptionConfiguration -> assert the response's repositoryCreationTemplate.encryptionConfiguration == {'encryptionType': 'AES256'}.
2. DescribeRepositoryCreationTemplates(prefixes=[P]) -> assert the same on the listed template.
3. UpdateRepositoryCreationTemplate(prefix=P, imageTagMutability='MUTABLE') -> assert the same.
4. DeleteRepositoryCreationTemplate(prefix=P) -> assert the same.
