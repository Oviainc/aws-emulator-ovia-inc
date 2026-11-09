Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__redshift-serverless.deploy-redshiftserverless-namespace-round-trip-then-delete cloudformation__redshift-serverless.deploy-redshiftserverless-namespace-invalid-name-rejected
Issue-Key: cloudformation__redshift-serverless|CreateStack|ValidationError
Issue-Commit: ab58585e96332c13ed2b8d30023e29e5d03b1c39

# cloudformation__redshift-serverless: AWS::RedshiftServerless::Namespace is not a registered resource type — CreateStack refuses every namespace template with "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `ab58585e96332c13ed2b8d30023e29e5d03b1c39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's template resource-type registry does not contain AWS::RedshiftServerless::Namespace, so a template that declares one is refused at CreateStack and no Redshift Serverless namespace can ever be provisioned, read back, validated or deleted through a stack. Failing checks: cloudformation__redshift-serverless.deploy-redshiftserverless-namespace-round-trip-then-delete, cloudformation__redshift-serverless.deploy-redshiftserverless-namespace-invalid-name-rejected.

AWS: AWS::RedshiftServerless::Namespace is a first-class, registry-backed CloudFormation resource type. CreateStack with a well-formed template declaring it is ACCEPTED (HTTP 200, a StackId is returned) and the stack proceeds to CREATE_COMPLETE; the type is never reported as unrecognized. Its handler creates the namespace through Redshift Serverless CreateNamespace (Smithy: CreateNamespace.namespaceName required, @length min 3 max 64, @pattern ^[a-z0-9-]+$; members adminUsername, dbName, manageAdminPassword, tags; errors ConflictException/TooManyTagsException/ValidationException). Ref resolves to the NamespaceName; Fn::GetAtt Namespace.NamespaceArn / Namespace.NamespaceId / Namespace.AdminUsername / Namespace.DbName resolve to the service's own values, where the ARN has the form arn:<partition>:redshift-serverless:<region>:<account>:namespace/<namespaceId> (the id and account are per-run values, the shape is the requirement). With ManageAdminPassword true the namespace reports an adminPasswordSecretArn of the form arn:<partition>:secretsmanager:<region>:<account>:secret:<name> for a secret that exists in this account and region. Stack-level Tags propagate onto the namespace and are readable through redshift-serverless ListTagsForResource alongside CloudFormation's aws:cloudformation:* system tags. A NamespaceName that violates the modeled pattern is caught by CloudFormation's EARLY property validation, not by the service call: the stack ends ROLLBACK_COMPLETE and the resource records a validation row with Type PROPERTY_VALIDATION, Status FAILED, verdict FAIL, property path /Resources/Namespace/Properties/NamespaceName and the message "Property value [<value>] does not match pattern: ^[a-z0-9-]+$" — fixed text "Property value [", "] does not match pattern: " plus the modeled pattern, with <value> taken verbatim from the request. Deleting the stack deletes the namespace, after which GetNamespace answers ResourceNotFoundException (HTTP 404) with "Serverless namespace <name> not found." — fixed text "Serverless namespace " / " not found." around the requested name — and ListNamespaces no longer lists it.

Emulator: CreateStack is REFUSED with HTTP 400 ValidationError and the message "Template format error: Unrecognized resource types: [AWS::RedshiftServerless::Namespace]" (fixed text "Template format error: Unrecognized resource types: [" ... "]", the bracketed list being the unrecognized types from the submitted template). No stack is created, so every downstream assertion — Ref/GetAtt resolution, the service round trip, ManageAdminPassword secret, tag propagation, the pattern-validation rollback row and the delete — is unreachable.

Why: a client deploying Redshift Serverless by template (console, CLI, CDK, Terraform's CFN resources) cannot create or manage a namespace at all; the refusal arrives as a template-format error, which tells the client its template is malformed rather than that the provider is missing, so retries and template fixes can never succeed. Negative tests also invert: a template AWS rejects with a per-property PROPERTY_VALIDATION row after a rollback is instead rejected before the stack exists, with a different code, status and message.

Fix: the CloudFormation service's template resource-type registry (the table that template validation consults to decide a type is recognized) must register AWS::RedshiftServerless::Namespace, and a provisioning handler for it must be added that calls redshift-serverless CreateNamespace/GetNamespace/DeleteNamespace, publishes Ref and the Namespace.* GetAtt attributes from the service's state, propagates stack tags, honours ManageAdminPassword via Secrets Manager, and routes the NamespaceName pattern through CloudFormation's early property-validation path so the failure is recorded as a PROPERTY_VALIDATION row and the stack rolls back.

Traces: see trace_ids; both checks fail identically on the CreateStack call, before any Redshift Serverless call is made.

## Evidence
Expected (AWS): cfn.create_stack(TemplateBody=<template declaring Resources.Namespace of Type AWS::RedshiftServerless::Namespace>) -> HTTP 200 with a StackId; DescribeStacks reaches CREATE_COMPLETE; ListNamespaces/GetNamespace show the namespace with the templated adminUsername and dbName; Outputs carry Ref == NamespaceName and GetAtt Namespace.NamespaceArn == arn:<partition>:redshift-serverless:<region>:<account>:namespace/<namespaceId>. For the invalid-name template the stack ends ROLLBACK_COMPLETE with exactly one validation row ("Namespace", "PROPERTY_VALIDATION", "FAILED", "FAIL", "Property value [<bad-name>] does not match pattern: ^[a-z0-9-]+$", "/Resources/Namespace/Properties/NamespaceName").

Actual (emulator): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::RedshiftServerless::Namespace]

No StackId is returned and no stack exists, so neither the happy-path round trip nor the rejection shape can be observed.

## Steps to Reproduce
1. Build and boot the emulator.
2. aws cloudformation create-stack --stack-name rss-ns --template-body '{"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Namespace":{"Type":"AWS::RedshiftServerless::Namespace","Properties":{"NamespaceName":"rss-ns-demo","AdminUsername":"admin1","DbName":"db1","ManageAdminPassword":true}}},"Outputs":{"NamespaceRef":{"Value":{"Ref":"Namespace"}},"NamespaceArn":{"Value":{"Fn::GetAtt":["Namespace","Namespace.NamespaceArn"]}}}}'
3. Observe HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::RedshiftServerless::Namespace]" instead of an accepted CreateStack.
4. Repeat with NamespaceName set to a value containing uppercase characters: AWS rolls the stack back with a PROPERTY_VALIDATION row on /Resources/Namespace/Properties/NamespaceName; the emulator refuses the CreateStack with the same unrecognized-type error instead.
