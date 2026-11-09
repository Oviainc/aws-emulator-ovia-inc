Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__cloudformation__mediaconvert.deploy-mediaconvert-certificateassociation-round-trip-and-delete acm__cloudformation__mediaconvert.deploy-mediaconvert-certificateassociation-unknown-certificate-rejected
Issue-Key: acm__cloudformation__mediaconvert|CreateStack|ValidationError
Issue-Commit: 5386ff6290ae18fab3ce660917360d5fb6557b5e

# cloudformation refuses AWS::MediaConvert::CertificateAssociation as an unrecognized resource type, so no MediaConvert certificate association can be deployed or rejected by a template

> _Note: this issue was identified against the code base at commit `5386ff6290ae18fab3ce660917360d5fb6557b5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One cause.

Cause 1 of 1: CreateStack rejects any template that declares AWS::MediaConvert::CertificateAssociation at template-validation time, so the whole resource type is unreachable — neither its happy path nor its refusal path can be exercised. Failing checks: acm__cloudformation__mediaconvert.deploy-mediaconvert-certificateassociation-round-trip-and-delete, acm__cloudformation__mediaconvert.deploy-mediaconvert-certificateassociation-unknown-certificate-rejected.

AWS: AWS::MediaConvert::CertificateAssociation is a registered CloudFormation resource type whose only property is Arn (an ACM certificate ARN) and whose Ref is that ARN. Its provider drives the MediaConvert control plane — Smithy (com-amazonaws-mediaconvert-mediaconvert): AssociateCertificate POST /2017-08-29/certificates, required input member Arn (jsonName 'arn'), success code 201, modeled errors BadRequestException (400), NotFoundException (404), ForbiddenException (403), ConflictException (409); the delete path is DisassociateCertificate. So a well-formed template declaring the type passes validation: CreateStack answers 200 with a StackId, the stack settles CREATE_COMPLETE, and ACM's DescribeCertificate reports the certificate InUseBy the account's MediaConvert service ARN (arn:<partition>:mediaconvert:<region>:<account>:service — the partition, region and account come from the request/caller, not from any recorded run). When the named ARN is well formed but ACM never issued it, the provider's AssociateCertificate is refused NotFoundException (404) and CloudFormation fails the resource CREATE_FAILED with a registry-handler ResourceStatusReason of the fixed form 'Resource handler returned message: "<the MediaConvert error message> (Service: MediaConvert, Status Code: 404, Request ID: <request id>)" (RequestToken: <token>, HandlerErrorCode: NotFound)' — the fixed text is the wrapper and the Service/Status Code fields; the message body, request id and token are per-call values — and the stack ends ROLLBACK_COMPLETE.

Emulator: CreateStack never gets as far as a provider. It answers 400 ValidationError with the message 'Template format error: Unrecognized resource types: [AWS::MediaConvert::CertificateAssociation]' (fixed text 'Template format error: Unrecognized resource types: ' followed by the bracketed list of types taken from the submitted template). No stack is created, so there is no StackId, no CREATE_COMPLETE, no ROLLBACK_COMPLETE and no stack event to read.

Why: a client that provisions MediaConvert certificate trust from a template (or from CDK, which synthesizes this type) cannot deploy at all — CreateStack fails before any stack exists, so there is nothing to describe, update or delete. Error handling is equally wrong: code that expects a bad certificate ARN to surface as a failed resource plus ROLLBACK_COMPLETE instead sees a synchronous template-format ValidationError, and ACM's InUseBy never reflects MediaConvert usage.

Fix: the CloudFormation resource-type registry / provider dispatch table in the cloudformation service — register AWS::MediaConvert::CertificateAssociation (property Arn, Ref = the certificate ARN) and route its Create/Delete through the mediaconvert handlers for AssociateCertificate / DisassociateCertificate, propagating MediaConvert's NotFoundException into the handler-error ResourceStatusReason with HandlerErrorCode NotFound.

Note: this is the same class of gap as the already-open 'CreateStack refuses every AWS::MediaTailor::* resource type as unrecognized' and 'cloudformation__ivs__mediatailor' issues, but neither of those names the MediaConvert type, so this is net-new.

Traces: see trace_ids — both checks die on the first CreateStack call.

## Evidence
Expected (AWS): CreateStack accepts the template; stack reaches CREATE_COMPLETE and ACM DescribeCertificate(InUseBy) == ['arn:<partition>:mediaconvert:<region>:<account>:service']. For an ARN ACM never issued: resource CertAssoc goes CREATE_FAILED with ResourceStatusReason 'Resource handler returned message: "...(Service: MediaConvert, Status Code: 404, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: NotFound)' and the stack ends ROLLBACK_COMPLETE.

Actual (emulator): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::MediaConvert::CertificateAssociation]

Both checks error out at their very first CreateStack, so every later assertion (CREATE_COMPLETE, InUseBy, the handler reason, ROLLBACK_COMPLETE) is unreached.

## Steps to Reproduce
1. Import a self-signed certificate with acm ImportCertificate and keep its CertificateArn.
2. CreateStack with TemplateBody {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"CertAssoc":{"Type":"AWS::MediaConvert::CertificateAssociation","Properties":{"Arn":"<that CertificateArn>"}}},"Outputs":{"CertArn":{"Value":{"Ref":"CertAssoc"}}}}.
3. Observe the call is refused 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::MediaConvert::CertificateAssociation]' instead of returning a StackId.
4. Repeat step 2 with the same ARN prefix but the certificate id 00000000-0000-0000-0000-000000000000 (never issued): the same ValidationError is returned instead of a stack that rolls back with a MediaConvert NotFound handler reason.
