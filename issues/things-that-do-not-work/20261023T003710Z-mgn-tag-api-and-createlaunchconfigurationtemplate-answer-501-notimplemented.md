Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mgn.tag-resource-round-trip mgn.create-launch-configuration-template-access-denied-rejected mgn.update-application-fields-persist
Issue-Shape-Id: com.amazonaws.mgn#ApplicationMigrationService
Issue-Key: mgn.tag-resource-round-trip|run#5|AssertionError|assert code == "NotFoundException", f"TagResource on a nonexistent ARN got {code!r}"
Issue-Key: mgn.create-launch-configuration-template-access-denied-rejected|_poll#18|AssertionError|raise AssertionError(
Issue-Key: mgn.update-application-fields-persist|run#5|AssertionError|assert init_code == "ValidationException", (
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# mgn tag API and CreateLaunchConfigurationTemplate answer 501 NotImplemented, and the account-initialization gate is not modeled (3 causes)

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Three net-new root causes in the mgn (ApplicationMigrationService) crate. Each must be closed separately.

Cause 1 of 3: the Tag* family answers 501 NotImplemented instead of a not-found refusal.
AWS: Smithy (com.amazonaws.mgn#TagResource) binds POST /tags/{resourceArn} with resourceArn (ARN, @length 20..2048, httpLabel, required) and models ResourceNotFoundException (httpError 404) among its errors; ListTagsForResource and UntagResource model the same not-found error. Recorded behavior only for the exact wire code: against a syntactically well-formed but nonexistent application ARN (arn:aws:mgn:<region>:<account>:application/app-00000000000000000) real MGN answers a 404 not-found refusal with error code NotFoundException and message 'Resource was not found' (message quoted as the recorded example; the requirement is a 404 not-found refusal, not a 501 and not a success). Notably the Tag* family does NOT fall back to UninitializedAccountException for a resource that was never created — that precedence is what the check asserts across all three operations.
Emulator: HTTP 501 with error code NotImplemented. Replay detail: "AssertionError: TagResource on a nonexistent ARN got 'NotImplemented'". The handlers are scaffolded stubs: services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/tag_resource.rs:183, .../list_tags_for_resource.rs and .../untag_resource.rs all return crate::scaffolded::Scaffolded::not_implemented(..) and still carry the SCAFFOLDED marker.
Why: tagging is how callers label and discover MGN resources. A 501 is not a modeled error, so SDK retry/exception mapping does not classify it: client code never sees ResourceNotFoundException/NotFoundException for a bad ARN, and every tag read/write against the emulator fails with a transport-level error instead of the service's own contract.
Fix: implement the three tag operations in their own handler files (tag_resource.rs, list_tags_for_resource.rs, untag_resource.rs) over the mgn tag state, returning the 404 not-found refusal for an ARN that names no existing resource and 200/empty (TagResource/UntagResource) or the tag map (ListTagsForResource) for one that does; drop the SCAFFOLDED markers.
Traces: mgn.tag-resource-round-trip (first assertion; the ListTagsForResource and UntagResource assertions in the same check were never reached because TagResource aborted it).

Cause 2 of 3: CreateLaunchConfigurationTemplate answers 501 NotImplemented, so no business outcome and no IAM authorization decision is ever produced.
AWS: Smithy (com.amazonaws.mgn#CreateLaunchConfigurationTemplate) binds POST /CreateLaunchConfigurationTemplate with success code 201 and models AccessDeniedException (httpError 403), UninitializedAccountException (400) and ValidationException (400). Recorded behavior: an identity with no mgn:* grant is refused with AccessDeniedException ahead of any account-level business state, and granting mgn:* collapses its outcome back onto whatever the authorized caller sees; an authorized caller gets either a 201 template or the account's own business exception — never a 501.
Emulator: every call, authorized or not, answers error code NotImplemented. Replay detail: "an unauthorized identity being rejected by IAM not reached within 45.0s; last observation 'NotImplemented'". The handler is a scaffolded stub: services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/create_launch_configuration_template.rs:202 returns Scaffolded::not_implemented("CreateLaunchConfigurationTemplate").
Why: launch configuration templates are the entry point of MGN launch configuration, and the 501 also masks the authorization decision — an unauthorized identity gets the SAME answer as an authorized one, so a client cannot distinguish 'denied' from 'allowed', and nothing proves rejected calls leave the template list unchanged.
Fix: implement create_launch_configuration_template.rs over mgn launch-configuration-template state (mint an lct-[0-9a-zA-Z]{17} id, answer 201 with the modeled output) and return AccessDeniedException (403) for an identity with no mgn:CreateLaunchConfigurationTemplate grant; drop the SCAFFOLDED marker.
Traces: mgn.create-launch-configuration-template-access-denied-rejected.

Cause 3 of 3: InitializeService always succeeds, so MGN's account-initialization gate does not exist.
AWS: Smithy (com.amazonaws.mgn#InitializeService) models errors AccessDeniedException (403) and ValidationException (400); UpdateApplication and ListApplications both model UninitializedAccountException (httpError 400) and ListApplications models it as its ONLY error, i.e. the account gate is part of their contract. Recorded behavior only: in the recorded environment InitializeService is refused with HTTP 400 ValidationException (message example 'Failed to create SLR or instance profiles', produced because the execution role cannot create the service-linked role), and the account therefore stays uninitialized so gated calls answer UninitializedAccountException.
Emulator: InitializeService returns success — the check observed error code None where a 400 ValidationException was recorded. Replay detail: "AssertionError: expected InitializeService to fail with ValidationException on an account whose execution role lacks iam:CreateServiceLinkedRole, got None". Because the check aborted there, the emulator's UpdateApplication/ListApplications gate behavior was not observed. NOTE: the already-open issue 20261022T132055Z-mgn-initializeservice-returns-204-success-instead-of-modeled.md reports the same InitializeService 204-vs-400 divergence from the WIRE corpus; this cause is filed to carry the behavioral check's replay key and the account-gate consequence, and the two should be closed together rather than independently.
Why: a client that bootstraps an account through InitializeService is told initialization succeeded when it did not, and no gated operation can ever report UninitializedAccountException — so error handling that keys on the gate (initialize, then retry) never runs and every later migration call proceeds against an account that was never initialized.
Fix: the mgn InitializeService handler plus the account-initialization state it should establish, and the gate check in the handlers of the operations that model UninitializedAccountException (UpdateApplication, ListApplications and the other gated control-plane ops). InitializeService must be able to conclude in failure with the modeled ValidationException (400), and gated operations must refuse with UninitializedAccountException (400) while the account is uninitialized — ahead of any not-found check.
Traces: mgn.update-application-fields-persist.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service mgn (0/20 passed).

[mismatch] mgn.tag-resource-round-trip
  AssertionError: TagResource on a nonexistent ARN got 'NotImplemented'
  Expected (recorded AWS): HTTP 404 not-found refusal, error code NotFoundException, message 'Resource was not found' (message = example).
  Actual (emulator):       HTTP 501, error code NotImplemented.
  Request: mgn.tag_resource(resourceArn='arn:aws:mgn:<region>:<account>:application/app-00000000000000000', tags={...})
  Stub: services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/tag_resource.rs:183 (also list_tags_for_resource.rs, untag_resource.rs — all carry the SCAFFOLDED marker).

[mismatch] mgn.create-launch-configuration-template-access-denied-rejected
  AssertionError: an unauthorized identity being rejected by IAM not reached within 45.0s; last observation 'NotImplemented'
  Expected (recorded AWS): unauthorized identity -> AccessDeniedException (403); authorized caller -> 201 template or the account's business exception; granting mgn:* collapses the outcome onto the authorized baseline.
  Actual (emulator):       error code NotImplemented for both the authorized baseline call and the unauthorized identity, for 45s of polling.
  Stub: services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/create_launch_configuration_template.rs:202.

[mismatch] mgn.update-application-fields-persist
  AssertionError: expected InitializeService to fail with ValidationException on an account whose execution role lacks iam:CreateServiceLinkedRole, got None
  Expected (recorded AWS): InitializeService -> HTTP 400 ValidationException (message example 'Failed to create SLR or instance profiles'); UpdateApplication / ListApplications on the uninitialized account -> UninitializedAccountException (400).
  Actual (emulator):       InitializeService succeeded (no error raised); the gate assertions were never reached.
  Related open issue: 20261022T132055Z-mgn-initializeservice-returns-204-success-instead-of-modeled.md (same InitializeService divergence seen on the wire corpus).

Model support (services/service-com-amazonaws-mgn-applicationmigrationservice/smithy-model/com-amazonaws-mgn-applicationmigrationservice.json):
  TagResource: POST /tags/{resourceArn}; errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400).
  CreateLaunchConfigurationTemplate: POST /CreateLaunchConfigurationTemplate, success 201; errors AccessDeniedException(403), UninitializedAccountException(400), ValidationException(400).
  UpdateApplication: errors ConflictException(409), ResourceNotFoundException(404), UninitializedAccountException(400).
  ListApplications: errors [UninitializedAccountException(400)] only.

## Steps to Reproduce
1. git clone the mockforge-aws-emulator repo (branch mainline) and boot the emulator built from that checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1, or just the three checks below.
3. mgn.tag-resource-round-trip: call TagResource with a well-formed but nonexistent application ARN. The emulator answers HTTP 501 NotImplemented; AWS answers a 404 not-found refusal (NotFoundException / modeled ResourceNotFoundException).
4. mgn.create-launch-configuration-template-access-denied-rejected: call CreateLaunchConfigurationTemplate as the authorized caller, then as a freshly created IAM user with no mgn:* grant. Both answer NotImplemented; AWS answers the account's business outcome for the authorized caller and AccessDeniedException for the unauthorized one.
5. mgn.update-application-fields-persist: call InitializeService; it succeeds instead of failing with ValidationException, so no UninitializedAccountException gate can be observed on UpdateApplication / ListApplications.
6. Re-replay mgn.tag-resource-round-trip, mgn.create-launch-configuration-template-access-denied-rejected and mgn.update-application-fields-persist after the fix; ALL THREE must pass before closing.
