Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__rds__secretsmanager.deploy-secret-target-attachment-rds-instance cloudformation__rds__secretsmanager.secret-target-attachment-update-nonexistent-target-rejected
Issue-Key: cloudformation__rds__secretsmanager.deploy-secret-target-attachment-rds-instance|run#14|AssertionError|assert status == "CREATE_COMPLETE", (
Issue-Key: cloudformation__rds__secretsmanager.secret-target-attachment-update-nonexistent-target-rejected|run#12|AssertionError|assert status == "CREATE_COMPLETE", (
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# cloudformation__rds__secretsmanager: an AWS::RDS::DBInstance whose MasterUsername/MasterUserPassword are {{resolve:secretsmanager:...}} dynamic references fails to create, rolling the stack back

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the secretsmanager dynamic reference in an RDS instance's master credentials is not resolved, so the DBInstance create fails.

AWS: CloudFormation resolves a '{{resolve:secretsmanager:<secret-id>:SecretString:<json-key>}}' dynamic reference at resource-create time and passes the retrieved plaintext to RDS, so a template that generates a secret with AWS::SecretsManager::Secret and then sets MasterUsername/MasterUserPassword from that secret creates the DBInstance and the stack reaches CREATE_COMPLETE; DescribeDBInstances then reads back MasterUsername equal to the username stored in the secret. Recorded behavior only for the resolution order (secret first, instance second, both in one stack).

Emulator: the AppSecret resource reaches CREATE_COMPLETE, then DbInstance goes CREATE_FAILED, is deleted, and the stack settles ROLLBACK_COMPLETE - identically with and without the AWS::SecretsManager::SecretTargetAttachment that the same template later adds. Nothing downstream of the instance (the attachment merging host/port/dbname into the secret, the update that removes it) is ever reached.

Why: the canonical pattern for RDS credentials in CloudFormation - generate a secret, consume it by dynamic reference - cannot deploy at all. A client gets a rolled-back stack instead of a running instance, and the SecretTargetAttachment contract (the secret gaining engine/host/port/dbInstanceIdentifier) is unobservable.

Fix: the CloudFormation template-resolution component that expands {{resolve:secretsmanager:...}} dynamic references before handing properties to a resource provider (and, if it already expands them, the AWS::RDS::DBInstance provider's handling of the resolved MasterUsername/MasterUserPassword).

Traces: cloudformation__rds__secretsmanager.deploy-secret-target-attachment-rds-instance, cloudformation__rds__secretsmanager.secret-target-attachment-update-nonexistent-target-rejected.

## Evidence
Expected: stack CREATE_COMPLETE with an available mysql db.t3.micro instance whose MasterUsername comes from the secret.
Actual: stack ended 'ROLLBACK_COMPLETE': [('AppSecret','CREATE_COMPLETE'), ('DbInstance','CREATE_FAILED'), ('DbInstance','DELETE_COMPLETE'), ('AppSecret','DELETE_SKIPPED'|'DELETE_COMPLETE'), ('<stack>','ROLLBACK_COMPLETE')].
Template properties involved: MasterUsername: !Sub "{{resolve:secretsmanager:${AppSecret}:SecretString:username}}", MasterUserPassword: !Sub "{{resolve:secretsmanager:${AppSecret}:SecretString:password}}".

## Steps to Reproduce
1. CreateStack with an AWS::SecretsManager::Secret using GenerateSecretString and an AWS::RDS::DBInstance whose MasterUsername/MasterUserPassword are {{resolve:secretsmanager:<that secret>:SecretString:username|password}}.
2. Wait for the stack's terminal status and read its events: the DBInstance goes CREATE_FAILED and the stack rolls back.
