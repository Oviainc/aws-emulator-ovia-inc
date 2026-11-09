Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iot.cfn-iot-cacertificate-sni-only-round-trip-and-delete cloudformation__iot.cfn-iot-job-create-and-delete-with-stack cloudformation__iot.cfn-iot-job-template-create-and-delete-with-stack cloudformation__iot.cfn-iot-logging-and-resource-specific-logging-read-back-then-disabled-after-delete cloudformation__iot.cfn-iot-mitigation-action-cross-account-role-rejected cloudformation__iot.cfn-iot-mitigation-action-properties-and-getatt-read-back-then-not-found-after-delete cloudformation__iot.cfn-iot-thing-thinggroup-billinggroup-round-trip-and-delete cloudformation__iot.cfn-iot-thingtype-round-trip-and-delete cloudformation__iot.deploy-iot-command-invalid-namespace-rejected cloudformation__iot.deploy-iot-command-round-trip-then-pending-deletion cloudformation__iot.deploy-iot-softwarepackage-version-and-fleetmetric-round-trip-then-delete cloudformation__iot.cfn-iot-policy-certificate-principal-attachments-round-trip-and-delete cloudformation__iot.cfn-iot-thing-too-many-attributes-rejected-and-rolled-back
Issue-Key: cloudformation__iot|CreateStack|ValidationError
Issue-Key: cloudformation__iot|GetPolicy|ResourceNotFoundException
Issue-Key: cloudformation__iot.cfn-iot-thing-too-many-attributes-rejected-and-rolled-back|run#8|AssertionError|assert stack_status == "ROLLBACK_COMPLETE", (
Issue-Commit: ec57509239c646de823e7950d2544f6a600b68b2

# cloudformation__iot: 3 net-new causes — every AWS::IoT::* resource type is unrecognized by CreateStack, a declared AWS::IoT::Policy is never provisioned, and a >3-attribute AWS::IoT::Thing is accepted instead of rolling the stack back

> _Note: this issue was identified against the code base at commit `ec57509239c646de823e7950d2544f6a600b68b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three independent defects in the CloudFormation provisioning path for AWS IoT resources. Each is a separate cause and each must be closed before this issue is done.

Cause 1 of 4 (filed as 3 net-new; see note): CreateStack does not recognize the AWS::IoT::* resource types.
AWS: CloudFormation registers AWS::IoT::CACertificate, ::Job, ::JobTemplate, ::ThingGroup, ::BillingGroup, ::ThingType, ::Logging, ::ResourceSpecificLogging, ::MitigationAction, ::Command, ::SoftwarePackage, ::SoftwarePackageVersion and ::FleetMetric as first-class resource types; a template declaring them is accepted and the stack provisions the corresponding IoT control-plane resources (the IoT crate models all of the underlying operations). Recorded behavior only for the per-type registration list — AWS's template reference for aws-resource-iot-* is the authority.
Emulator: CreateStack refuses the template with HTTP 400, error code ValidationError and message 'Template format error: Unrecognized resource types: [<comma-separated type list>]' — the fixed text is 'Template format error: Unrecognized resource types: [' + the bracketed list + ']', the variable part being the set of declared types taken from the request template. Nothing is ever created.
Why: no IoT estate can be deployed as infrastructure-as-code. Every template, CDK app or stack-set that declares an IoT thing group, thing type, job, job template, CA certificate, mitigation action, logging configuration, command, software package/version or fleet metric fails at CreateStack, so IaC-driven IoT provisioning — and the rollback/refusal behavior layered on it — is untestable.
Fix: the CloudFormation resource-type registry and the per-type resource handlers for the AWS::IoT::* namespace (template type resolution in the cloudformation service crate, dispatching to the already-present iot operations).
Traces: cloudformation__iot.cfn-iot-cacertificate-sni-only-round-trip-and-delete, cloudformation__iot.cfn-iot-job-create-and-delete-with-stack, cloudformation__iot.cfn-iot-job-template-create-and-delete-with-stack, cloudformation__iot.cfn-iot-logging-and-resource-specific-logging-read-back-then-disabled-after-delete, cloudformation__iot.cfn-iot-mitigation-action-cross-account-role-rejected, cloudformation__iot.cfn-iot-mitigation-action-properties-and-getatt-read-back-then-not-found-after-delete, cloudformation__iot.cfn-iot-thing-thinggroup-billinggroup-round-trip-and-delete, cloudformation__iot.cfn-iot-thingtype-round-trip-and-delete, cloudformation__iot.deploy-iot-command-invalid-namespace-rejected, cloudformation__iot.deploy-iot-command-round-trip-then-pending-deletion, cloudformation__iot.deploy-iot-softwarepackage-version-and-fleetmetric-round-trip-then-delete.

Cause 2 of 3: an AWS::IoT::Policy declared by a stack that reaches CREATE_COMPLETE does not exist in IoT.
AWS: when a stack containing AWS::IoT::Policy (together with AWS::IoT::Certificate and the policy/principal attachments) reaches CREATE_COMPLETE, the policy exists and iot GetPolicy answers 200 with policyName/policyArn/policyDocument. Smithy (iot): GetPolicy errors are InvalidRequestException, ResourceNotFoundException (404), ThrottlingException, UnauthorizedException, InternalFailureException, ServiceUnavailableException — the 404 is reserved for a policy that was never created.
Emulator: the stack create path does not fail with an unrecognized-type error for this template, yet the read-back of the policy it declares is refused: GetPolicy answers 404 ResourceNotFoundException with message 'Policy <policyName> does not exist', where <policyName> is the name taken from the template. So the type is accepted but the resource is never materialized in the IoT control plane.
Why: a client that deploys certificates, policies and their attachments by template sees a successful stack and then cannot find, attach or exercise the policy; the whole mTLS authorization estate is silently empty.
Fix: the AWS::IoT::Policy (and the certificate/attachment) resource handlers in the CloudFormation provisioning path — they must call the iot control plane so the created resources are readable, and must fail the resource (not silently succeed) when they cannot.
Traces: cloudformation__iot.cfn-iot-policy-certificate-principal-attachments-round-trip-and-delete.

Cause 3 of 3: an AWS::IoT::Thing with more than three attributes and no thing type is accepted, so the stack never rolls back.
AWS: CreateThing refuses an attributePayload holding more than three attributes when no thingTypeName is given. Smithy (iot): CreateThing takes thingName, thingTypeName, attributePayload and declares InvalidRequestException (400); the refusal message is the fixed text 'To use more than 3 attributes, a thing must have a type specified.' Through CloudFormation this surfaces as the resource's CREATE_FAILED reason 'Resource handler returned message: "To use more than 3 attributes, a thing must have a type specified. (Service: Iot, Status Code: 400, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)' — fixed text as quoted, with the request id, request token and the attribute count/name values coming from the request; the stack then ends ROLLBACK_COMPLETE and the thing does not exist.
Emulator: the four-attribute thing is created and the stack ends CREATE_COMPLETE; no InvalidRequestException is raised and no rollback occurs (observed: stack ended 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE).
Why: a client relying on AWS's refusal gets a 'successful' deploy of a thing AWS would never have created, and never sees the rollback that removes it — negative IaC tests pass vacuously and drift from real AWS on redeploy.
Fix: CreateThing's input validation in the iot service (attribute-count rule when thingTypeName is absent), and the propagation of that refusal into the AWS::IoT::Thing resource handler's CREATE_FAILED reason so CloudFormation rolls back. The open issue 20261106T201842Z-iot-registry-plane-9-net-new-causes-creatething.md covers other CreateThing refusal wording but not this attribute-count rule or its CloudFormation rollback.
Traces: cloudformation__iot.cfn-iot-thing-too-many-attributes-rejected-and-rolled-back.

(Note: cloudformation__iot.cfn-iot-job-template-duplicate-id-rejected also failed in this service, on the unimplemented CreateJobTemplate stub already filed as 20261106T201842Z-iot-createjobtemplate-stub.md; it is deliberately not re-filed here.)

## Evidence
Cause 1 — expected: CreateStack accepts the template and the stack provisions the IoT resources. Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::IoT::CACertificate] / [AWS::IoT::Job] / [AWS::IoT::JobTemplate] / [AWS::IoT::ThingGroup, AWS::IoT::Logging, AWS::IoT::ResourceSpecificLogging] / [AWS::IoT::MitigationAction] / [AWS::IoT::ThingGroup, AWS::IoT::BillingGroup] / [AWS::IoT::ThingType] / [AWS::IoT::Command] / [AWS::IoT::SoftwarePackage, AWS::IoT::FleetMetric, AWS::IoT::SoftwarePackageVersion].

Cause 2 — expected: after CREATE_COMPLETE, iot GetPolicy answers 200 for the declared policy. Actual: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetPolicy operation: Policy <policyName> does not exist.

Cause 3 — expected: the AWS::IoT::Thing resource fails with 'To use more than 3 attributes, a thing must have a type specified.' (HandlerErrorCode: InvalidRequest) and the stack ends ROLLBACK_COMPLETE with the thing absent. Actual: AssertionError: stack '<name>' ended 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE — the four-attribute thing was created.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Cause 1: CreateStack with a template declaring any of AWS::IoT::{CACertificate,Job,JobTemplate,ThingGroup,BillingGroup,ThingType,Logging,ResourceSpecificLogging,MitigationAction,Command,SoftwarePackage,SoftwarePackageVersion,FleetMetric}; observe the ValidationError 'Unrecognized resource types'.
3. Cause 2: deploy a stack with AWS::IoT::Policy + AWS::IoT::Certificate + attachments, wait for CREATE_COMPLETE, then call iot GetPolicy with the declared policy name; observe 404 ResourceNotFoundException.
4. Cause 3: deploy a stack with one AWS::IoT::Thing whose AttributePayload.Attributes has four entries and no ThingTypeName; observe CREATE_COMPLETE instead of the InvalidRequest handler failure and ROLLBACK_COMPLETE.
