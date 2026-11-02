Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: serverlessrepo.cloudformation-template-expiry-window serverlessrepo.sar-deploy-denied-for-role-scoped-to-another-application
Issue-Shape-Id: com.amazonaws.serverlessapplicationrepository#ServerlessApplicationRepository
Issue-Key: serverlessrepo.cloudformation-template-expiry-window|run#24|AssertionError|assert message == expected, (
Issue-Key: serverlessrepo.sar-deploy-denied-for-role-scoped-to-another-application|run#20|AssertionError|raise AssertionError(
Issue-Commit: a9006feb831fdd789c02a15722f93d2f2e52852a

# SAR: GetCloudFormationTemplate not-found message is reworded, and CreateCloudFormationChangeSet/GetApplication ignore the caller's identity-policy resource scope

> _Note: this issue was identified against the code base at commit `a9006feb831fdd789c02a15722f93d2f2e52852a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCloudFormationTemplate

## Description
Two distinct net-new root causes in the serverlessapplicationrepository emulator. Both must be closed before submitting.

Cause 1 of 2: GetCloudFormationTemplate's NotFoundException message does not use AWS's wording for an unissued TemplateId.
AWS: Smithy (com-amazonaws-serverlessapplicationrepository): GetCloudFormationTemplate GET /applications/{ApplicationId}/templates/{TemplateId}, errors include NotFoundException @httpError 404. The message wording itself is recorded behavior only: for a well-formed UUID the service never issued, SAR answers 404 NotFoundException whose message is the fixed text "Template with id " + <the TemplateId taken verbatim from the request path> + " can not be found." — the only variable part is the requested TemplateId; no application identifier appears in it.
Emulator: returns the correct code (NotFoundException) but the message "Template with id <TemplateId> could not be found for application <application ARN>." Replay saw: 'Template with id 00000000-0000-4000-8000-000000000000 could not be found for application arn:aws:serverlessrepo:us-west-2:000000000000:applications/sim2real-behav-sar-e3153f44503e.' where AWS answers 'Template with id 00000000-0000-4000-8000-000000000000 can not be found.' (the UUID and the ARN are per-run values; the divergence is the fixed text "could not be found for application" replacing "can not be found." and the extra state-derived ARN segment).
Why: clients and test suites that match on SAR's not-found text — and anything normalising SAR errors into a user-facing message — see a message AWS never emits, and the leaked application ARN makes the message depend on emulator state that AWS's message does not carry.
Fix: the not-found error construction in the GetCloudFormationTemplate handler of the serverlessapplicationrepository service (the template-lookup path that builds NotFoundException), not the serializer — the code and status are already right.
Traces: serverlessapplicationrepository.cloudformation-template-expiry-window (all other assertions in that check — UUID shape, PREPARING->ACTIVE, the exact 6-hour CreationTime/ExpirationTime window, identity echo — passed; only the refusal message diverged).

Cause 2 of 2: CreateCloudFormationChangeSet (and GetApplication) are not authorized against the caller's identity-policy resource scope, so an application the caller is not permitted to touch is deployed instead of refused.
AWS: Smithy models CreateCloudFormationChangeSet POST /applications/{ApplicationId}/changesets (success 201) with ForbiddenException @httpError 403 among its errors; the IAM-denial behavior is recorded behavior only. Authorization for serverlessrepo is decided per APPLICATION ARN: when an assumed role's identity policy allows serverlessrepo:CreateCloudFormationChangeSet and serverlessrepo:GetApplication only on application A's ARN, the same identity calling for application B in the same account is refused with HTTP 403 and error code AccessDeniedException, message fixed text "User: " + <the caller's assumed-role ARN> + " is not authorized to perform: serverlessrepo:CreateCloudFormationChangeSet on resource: " + <the requested application's ARN> + " because no identity-based policy allows the serverlessrepo:CreateCloudFormationChangeSet action". The variable parts are the caller ARN, the requested application ARN and the action name; the rest is fixed. The same rule applies to GetApplication on the unnamed application, with serverlessrepo:GetApplication substituted for the action in both places. The refusal has no effect: no stack named serverlessrepo-<requested StackName> exists afterwards, while the application the policy DOES name stays usable for both calls.
Emulator: the call succeeds. Replay saw CreateCloudFormationChangeSet return 201 with a ChangeSetId ('arn:aws:cloudformation:us-west-2:000000000000:changeSet/serverlessrepo-changeset-<uuid>/<uuid>') and a StackId for the application the role's policy never names, under credentials from an assumed role whose only serverlessrepo statement is scoped to the other application's ARN — i.e. the identity's policy scope is not consulted at all, and a stack is cut as a side effect. The positive control (same identity, the allowed application) succeeded, so this is not propagation or a missing CloudFormation/S3 permission.
Why: a client cannot test or rely on SAR deploy authorization against the emulator. Least-privilege policies appear to work while in fact nothing is enforced: code that should get a 403 AccessDeniedException instead gets a 201 and a real stack, so authorization regressions pass locally and only fail against AWS, and an unintended deploy leaves state behind.
Fix: authorization of the serverlessapplicationrepository service's CreateCloudFormationChangeSet and GetApplication handlers — the per-application-ARN identity-policy evaluation that must run before the handler does any work and must emit 403 AccessDeniedException with the message template above while leaving no stack or change set behind.
Traces: serverlessapplicationrepository.sar-deploy-denied-for-role-scoped-to-another-application.

Related but not covering: the open issue 20261022T182143Z-serverlessapplicationrepository-serverlessrepo-template-handling-is-json-only-a-yaml.md covers YAML template parsing (ParameterDefinitions / missing Transform) and the open issue 20261022T182143Z-cloudformation-serverlessapplicationrepo-sar-deploy-path-requiredcapabilities-is-always-empty-never.md covers RequiredCapabilities; neither covers the not-found wording or the missing authorization.

## Evidence
Cause 1 — serverlessapplicationrepository.cloudformation-template-expiry-window:
  request: GetCloudFormationTemplate(ApplicationId=<published app>, TemplateId="00000000-0000-4000-8000-000000000000") (a well-formed UUID SAR never issued)
  expected: 404, Code=NotFoundException, Message='Template with id 00000000-0000-4000-8000-000000000000 can not be found.'
  actual:   Code=NotFoundException, Message='Template with id 00000000-0000-4000-8000-000000000000 could not be found for application arn:aws:serverlessrepo:us-west-2:000000000000:applications/sim2real-behav-sar-e3153f44503e.'
  AssertionError: reading an unissued TemplateId was refused with '... could not be found for application ...', expected '... can not be found.'

Cause 2 — serverlessapplicationrepository.sar-deploy-denied-for-role-scoped-to-another-application:
  setup: two applications published in one account (A = allowed, B = denied); an assumed role whose inline policy allows serverlessrepo:CreateCloudFormationChangeSet + serverlessrepo:GetApplication on A's ARN only, plus cloudformation:* and s3:GetObject on *; positive control on A SUCCEEDED.
  request: CreateCloudFormationChangeSet(ApplicationId=B, StackName=<unique>, SemanticVersion="1.0.0", ParameterOverrides=[{Name:QueueNameParam,Value:<unique>}]) under that role's credentials
  expected: HTTP 403, Code=AccessDeniedException, Message='User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: serverlessrepo:CreateCloudFormationChangeSet on resource: <B's application ARN> because no identity-based policy allows the serverlessrepo:CreateCloudFormationChangeSet action'; and no stack serverlessrepo-<StackName>
  actual:   HTTP 201 with ChangeSetId 'arn:aws:cloudformation:us-west-2:000000000000:changeSet/serverlessrepo-changeset-0c595985-3dee-4736-88dc-a593928219e3/cda37c13-dcbb-b6ff-1ea6-781fa29e20e4' and a StackId — the change set was cut for B.
  AssertionError: a role scoped to '...applications/sim2real-behav-sarok-a25d0e4ab7ff' cut a change set for '...applications/sim2real-behav-sarno-5f159e34fffb'.
  (The same check also asserts GetApplication(B) under that identity is refused with the analogous AccessDeniedException message; the run failed before reaching it.)

## Steps to Reproduce
Cause 1:
1. serverlessrepo.CreateApplication(Name=<unique>, Author=.., Description=.., SemanticVersion="1.0.0", TemplateBody=<SAM template with one AWS::SQS::Queue>).
2. Poll GetApplication(ApplicationId, SemanticVersion="1.0.0") until the version is served.
3. CreateCloudFormationTemplate(ApplicationId, SemanticVersion="1.0.0"); poll GetCloudFormationTemplate until Status=ACTIVE.
4. GetCloudFormationTemplate(ApplicationId, TemplateId="00000000-0000-4000-8000-000000000000").
5. Observe Error.Message: it reads 'Template with id <id> could not be found for application <ARN>.' instead of AWS's 'Template with id <id> can not be found.'

Cause 2:
1. Publish two SAR applications A and B (version 1.0.0) from a SAM template with a QueueNameParam parameter.
2. iam.CreateRole assumable by the account root; iam.PutRolePolicy allowing serverlessrepo:CreateCloudFormationChangeSet and serverlessrepo:GetApplication on A's application ARN only, plus cloudformation:* and s3:GetObject on *.
3. sts.AssumeRole into that role and build a serverlessrepo client from the returned credentials.
4. CreateCloudFormationChangeSet for A (positive control) — succeeds.
5. CreateCloudFormationChangeSet for B with the same identity and the same shape of request.
6. Observe a 201 response carrying ChangeSetId/StackId instead of a 403 AccessDeniedException; also GetApplication(B) under that identity is answered instead of refused.
