Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__medical-imaging.deploy-healthimaging-datastore-provisions-active-store cloudformation__medical-imaging.deploy-healthimaging-datastore-invalid-name-rejected
Issue-Key: cloudformation__medical-imaging|CreateStack|ValidationError
Issue-Commit: 0c7f0017b5788ebb0b2bf7aa1a6523722bad5325

# cloudformation__medical-imaging: AWS::HealthImaging::Datastore is not a registered resource type — CreateStack refuses every HealthImaging template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `0c7f0017b5788ebb0b2bf7aa1a6523722bad5325`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's resource-type registry does not know AWS::HealthImaging::Datastore, so a template containing one is rejected at CreateStack time and no HealthImaging data store can ever be provisioned, referenced, or rolled back through CloudFormation. Failing checks: cloudformation__medical-imaging.deploy-healthimaging-datastore-provisions-active-store, cloudformation__medical-imaging.deploy-healthimaging-datastore-invalid-name-rejected.

AWS: CreateStack accepts a template whose Resources contain Type: AWS::HealthImaging::Datastore (the published CloudFormation type for AWS HealthImaging), returns a StackId with HTTP 200, and provisions the data store through the HealthImaging control plane. The provisioned resource is a real data store: Ref returns the 32-hex-lowercase datastoreId (Smithy: CreateDatastore output datastoreId @pattern ^[0-9a-z]{32}$, required), Fn::GetAtt exposes DatastoreArn / DatastoreStatus / CreatedAt / UpdatedAt consistent with GetDatastore.datastoreProperties, stack-level and resource-level Tags propagate so medical-imaging ListTagsForResource reports them, the store settles out of CREATING into ACTIVE (Smithy: DatastoreStatus enum), and after DeleteStack GetDatastore answers ResourceNotFoundException (404) with message "Datastore resource is not found." (recorded behavior; Smithy: CreateDatastore/GetDatastore errors include ResourceNotFoundException httpError 404).
AWS also validates the type's properties before provisioning: a DatastoreName that breaks the member's pattern (Smithy: CreateDatastore.datastoreName @length 1-256, @pattern ^[A-Za-z0-9._/#-]+$ — a trailing space is forbidden) fails early property validation, the stack ends ROLLBACK_COMPLETE, and DescribeEvents reports one VALIDATION_ERROR OperationEvent for the offending resource. The fixed text of that reason is "Property value [<value>] does not match pattern: <pattern>"; its variable parts are the submitted property value and the member's pattern taken from the model, and the row's variable parts are the template's logical id, ValidationName PROPERTY_VALIDATION, ValidationStatus FAILED, ValidationFailureMode FAIL, and ValidationPath /Resources/<LogicalId>/Properties/DatastoreName. No data store of the rejected name exists afterwards.

Emulator: CreateStack refuses the template outright with HTTP 400, error code ValidationError and message "Template format error: Unrecognized resource types: [AWS::HealthImaging::Datastore]" (fixed text "Template format error: Unrecognized resource types: [" + the unknown type names + "]"). The call never reaches provisioning, so neither the happy-path representation (Ref/GetAtt/tags/ACTIVE/NotFound-after-delete) nor the property-validation rejection path can be exercised at all.

Why: a client deploying any HealthImaging estate as infrastructure-as-code cannot create a stack at all — the template is rejected before a stack id is minted, so CloudFormation-driven provisioning, GetAtt-based wiring of a datastore ARN into other resources, and the rollback/validation behavior a client depends on for bad input are all unavailable. Code that treats ValidationError on CreateStack as "my template is malformed" will mis-diagnose a perfectly valid AWS template.

Fix: the CloudFormation resource-type registry and its HealthImaging resource provider — register AWS::HealthImaging::Datastore with its Properties (DatastoreName, Tags, KmsKeyArn, ...) and attributes (DatastoreArn, DatastoreStatus, CreatedAt, UpdatedAt), wire create/delete to the medical-imaging state so Ref yields the real datastoreId and deletion removes the store, propagate stack and resource tags, and run the modeled property constraints (datastoreName @pattern/@length) as early PROPERTY_VALIDATION producing a VALIDATION_ERROR event and ROLLBACK_COMPLETE instead of provisioning.

Traces: cloudformation__medical-imaging.deploy-healthimaging-datastore-provisions-active-store (happy path: Ref/GetAtt/tags/ACTIVE/NotFound-after-delete), cloudformation__medical-imaging.deploy-healthimaging-datastore-invalid-name-rejected (rejection path: trailing-space DatastoreName must roll the stack back with a PROPERTY_VALIDATION event).

## Evidence
Expected (AWS): CreateStack(TemplateBody=<template with Type: AWS::HealthImaging::Datastore>) -> HTTP 200 with a StackId; stack reaches CREATE_COMPLETE; Ref Store == a datastoreId matching ^[0-9a-z]{32}$ that ListDatastores reports under the template's DatastoreName; GetAtt DatastoreArn/DatastoreStatus/CreatedAt/UpdatedAt match GetDatastore.datastoreProperties (status ACTIVE); ListTagsForResource reports the stack tag and the resource tag; after DeleteStack, GetDatastore -> 404 ResourceNotFoundException "Datastore resource is not found.". For a DatastoreName with a trailing space: stack ends ROLLBACK_COMPLETE and DescribeEvents reports a VALIDATION_ERROR row ('Store', 'PROPERTY_VALIDATION', 'FAILED', 'FAIL', 'Property value [<name> ] does not match pattern: ^[A-Za-z0-9._/#-]+$', '/Resources/Store/Properties/DatastoreName'), and no data store of that name is listed.

Actual (emulator): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::HealthImaging::Datastore]. Both checks error on the very first CreateStack call, so nothing downstream is observable.

## Steps to Reproduce
1. cfn = boto3.client('cloudformation'); mi = boto3.client('medical-imaging') against the emulator.
2. template = '''AWSTemplateFormatVersion: "2010-09-09"\nResources:\n  Store:\n    Type: AWS::HealthImaging::Datastore\n    Properties:\n      DatastoreName: my-datastore\nOutputs:\n  Id:\n    Value: !Ref Store\n  Arn:\n    Value: !GetAtt Store.DatastoreArn\n'''
3. cfn.create_stack(StackName='hi-ds-stack', TemplateBody=template, Tags=[{'Key':'sim2real','Value':'hi-ds-stack'}])
   -> observe ValidationError 'Template format error: Unrecognized resource types: [AWS::HealthImaging::Datastore]' instead of a StackId.
4. Expected continuation once the type is registered: poll DescribeStacks to CREATE_COMPLETE, read Outputs, compare Ref/GetAtt against mi.list_datastores()/mi.get_datastore(), check mi.list_tags_for_resource(resourceArn=<arn>), then DeleteStack and expect mi.get_datastore to answer 404 ResourceNotFoundException.
5. Rejection path: re-submit the same template with DatastoreName ending in a space; expect ROLLBACK_COMPLETE plus a PROPERTY_VALIDATION VALIDATION_ERROR event on /Resources/Store/Properties/DatastoreName, and no datastore of that name in ListDatastores.
