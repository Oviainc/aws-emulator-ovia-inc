Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.deploy-cfn-description-update-keeps-the-generated-value-and-a-generator-change-regenerates secretsmanager.deploy-cfn-dynamic-reference-resolves-the-json-key-of-the-named-version secretsmanager.deploy-cfn-secret-generate-secret-string-matches-template secretsmanager.deploy-secret-cfn-stack-behaves secretsmanager.deploy-cfn-dynamic-reference-to-a-missing-json-key-rejected-and-rolled-back secretsmanager.deploy-cfn-public-resource-policy-rejected-by-block-public-policy-and-rolled-back
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secrets-manager|GetSecretValue|ResourceNotFoundException
Issue-Key: secretsmanager.deploy-cfn-dynamic-reference-to-a-missing-json-key-rejected-and-rolled-back|run#12|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: secretsmanager.deploy-cfn-public-resource-policy-rejected-by-block-public-policy-and-rolled-back|run#11|AssertionError|assert status == "ROLLBACK_COMPLETE", f"a public policy under BlockPublicPolicy must roll back; it ended {status!r}"
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# CloudFormation-managed secrets: AWS::SecretsManager::Secret deploys but no secret exists, and failed dynamic references never roll back (2 causes)

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two root causes. Cause count: 2.

=== Cause 1 of 2: a stack carrying AWS::SecretsManager::Secret reaches CREATE_COMPLETE without the secret existing ===
AWS: CloudFormation creates the secret as part of the stack: after CREATE_COMPLETE the Ref/GetAtt ARN resolves, DescribeSecret reports it, GetSecretValue serves the value (either the template's SecretString or one produced by GenerateSecretString matching the template's constraints), a stack UPDATE that only changes Description keeps the generated value while a change to the generator regenerates it, and a dynamic reference {{resolve:secretsmanager:<arn>:SecretString:<key>}} resolves against the deployed secret.
Emulator: the stack settles but the secret is not in the store - DescribeSecret and GetSecretValue on the ARN the stack reports answer 400 ResourceNotFoundException ('Secrets Manager can\'t find the specified secret.'). Sibling deploy checks of the same family are already carried by open issue 20261024T024248Z-redshift-secretsmanager-createcluster-with-managemasterpassword-does-not-create-the-admin.md; the four checks listed here are not on its replay key.
Why: every IaC path to a secret goes through CloudFormation. A client deploys a stack that reports success and then cannot read the credential it just provisioned, and template-side generation/update semantics (keep the generated value, regenerate on a generator change) are unobservable.
Fix: the CloudFormation resource provider for AWS::SecretsManager::Secret - create/update the secret in the Secrets Manager store during the stack operation (including GenerateSecretString and the update rules) rather than only recording the logical resource.
Traces: secretsmanager.deploy-cfn-description-update-keeps-the-generated-value-and-a-generator-change-regenerates, secretsmanager.deploy-cfn-dynamic-reference-resolves-the-json-key-of-the-named-version, secretsmanager.deploy-cfn-secret-generate-secret-string-matches-template, secretsmanager.deploy-secret-cfn-stack-behaves.

=== Cause 2 of 2: a resource whose Secrets Manager reference or policy must fail still reports CREATE_COMPLETE ===
AWS: a dynamic reference {{resolve:secretsmanager:<arn>:SecretString:<key>}} naming a JSON key the secret does not hold fails the resource's creation, and a stack attaching a public (wildcard-principal) resource policy under BlockPublicPolicy is refused; in both cases the stack rolls back and ends ROLLBACK_COMPLETE with the failure reason on the resource event.
Emulator: both stacks end CREATE_COMPLETE - the unresolvable reference is accepted and the public policy is not refused, so no rollback occurs. Related: open issue 20261023T114649Z-cloudformation-outputs-carry-no-outputvalue-failed-resources-never.md carries the cfn__lambda variant of the missing-key case.
Why: the rollback is the safety property. A deployment that should have failed loudly leaves a half-configured resource (a Lambda with an unresolved API key, a secret with a public policy) and the pipeline that gates on the stack status proceeds.
Fix: the CloudFormation dynamic-reference resolver (fail the resource when the key is absent) and the resource provider's refusal path, plus the stack's rollback transition on resource failure.
Traces: secretsmanager.deploy-cfn-dynamic-reference-to-a-missing-json-key-rejected-and-rolled-back, secretsmanager.deploy-cfn-public-resource-policy-rejected-by-block-public-policy-and-rolled-back.

## Evidence
Cause 1: 'botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeSecret operation: Secrets Manager can\'t find the specified secret.' and the same on GetSecretValue, raised after the stack reached a terminal state, in all four checks.
Cause 2: 'a reference to a missing JSON key must roll the stack back; it ended \'CREATE_COMPLETE\'' and 'a public policy under BlockPublicPolicy must roll back; it ended \'CREATE_COMPLETE\''.

## Steps to Reproduce
Cause 1: cloudformation CreateStack with a template containing AWS::SecretsManager::Secret (SecretString or GenerateSecretString) and an Output of its Ref; wait for CREATE_COMPLETE; secretsmanager DescribeSecret/GetSecretValue on the reported ARN -> ResourceNotFoundException. Then UpdateStack changing only Description and re-read the value.
Cause 2: CreateStack with a resource whose property is {{resolve:secretsmanager:<arn>:SecretString:no_such_key}}, and separately a stack attaching a wildcard-principal resource policy with BlockPublicPolicy in force; poll DescribeStacks for the terminal status - both report CREATE_COMPLETE instead of ROLLBACK_COMPLETE.
