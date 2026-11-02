Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: serverlessrepo.sar-prepared-template-serves-the-published-body serverlessrepo.cloudformation-template-expiry-window serverlessrepo.rejects-unshareable-policy-statement
Issue-Shape-Id: com.amazonaws.serverlessapplicationrepository#ServerlessApplicationRepository
Issue-Key: serverlessrepo.sar-prepared-template-serves-the-published-body|_fetch#16|HTTPError|with urllib.request.urlopen(urllib.request.Request(target_url, headers=route_headers), timeout=_FETCH_TIMEOUT_S) as response: # noqa: S310
Issue-Key: serverlessrepo.cloudformation-template-expiry-window|run#18|AssertionError|assert window == datetime.timedelta(hours=6), (
Issue-Key: serverlessrepo.rejects-unshareable-policy-statement|_error#13|AssertionError|raise AssertionError("PutApplicationPolicy succeeded; real AWS refuses this statement")
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# serverlessrepo prepared templates: TemplateUrl serves nothing, expiry window is 1h not 6h, and PutApplicationPolicy accepts an owner principal

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCloudFormationTemplate

## Description
Four net-new root causes in the serverlessapplicationrepository emulator, all grounded in failing behavioral checks. Causes 1-2 are in the prepared-CloudFormation-template path; causes 3-4 are in PutApplicationPolicy statement validation. (Note: the open issue 20261022T182143Z-serverlessapplicationrepository-putapplicationpolicy-stub.md describes PutApplicationPolicy as an unimplemented 501 scaffold; it is now implemented and answers 200, so it does NOT cover causes 3-4 — those are validation defects in the implemented handler.)

=== Cause 1 of 4: the TemplateUrl of a prepared template is not fetchable ===
AWS: CreateCloudFormationTemplate/GetCloudFormationTemplate return `TemplateUrl`, documented in the Smithy model (CreateCloudFormationTemplateResponse$TemplateUrl: "A link to the template that can be used to deploy the application using AWS CloudFormation"). Recorded behavior only for the fetch itself: the recorded unit performs a plain HTTPS GET of that URL with no AWS signature of its own and receives HTTP 200 whose body is the published SAM template (SAR re-renders the submitted JSON as YAML, so the parsed document equals the published document). This is the artifact a CloudFormation client actually deploys from.
Emulator: the URL is announced but nothing serves it. `state::minted_artifact_url` builds a cosmetic locator `https://awsserverlessrepo-<region>.s3.<region>.amazonaws.com/<account>/<app>/<version>/template.yaml` and its own doc comment says it is "an identifier of stored state, not a fetchable endpoint". The replay routes that URL back at the emulator's own endpoint and the emulator answers `urllib.error.HTTPError: HTTP Error 404: Not Found`, so the check dies before it can grade the body, the TemplateId-scoped read or re-preparation.
Why: a client that does what CloudFormation does — GET the TemplateUrl and deploy the bytes — gets a 404 instead of a template, so no SAR-published application can be deployed through the emulator even though the control-plane envelope looks correct.
Fix: the template/artifact data plane. Either serve the stored body at the path `minted_artifact_url` announces (the body is already retained on the version record as `AppVersion::template_body`, rendered as YAML), or announce a URL on the emulator's own endpoint that is served. The announced address and the served address must be the same one.
Traces: serverlessapplicationrepository.sar-prepared-template-serves-the-published-body

=== Cause 2 of 4: a prepared template expires 1 hour after creation instead of 6 ===
AWS: recorded behavior only — the recorded unit asserts `ExpirationTime - CreationTime == exactly 6 hours` on an ACTIVE prepared template. (The Smithy doc on CreateCloudFormationTemplateResponse$ExpirationTime says "Templates expire 1 hour after creation", which the recorded service contradicts; the recording is the behavior clients observe.)
Emulator: the window is one hour. `TEMPLATE_LIFETIME = Duration::from_secs(60 * 60)` and `CloudFormationTemplate::prepared` sets `expires_at = now + TEMPLATE_LIFETIME`, so the replay saw ExpirationTime 1:00:00 after CreationTime (e.g. CreationTime '2026-10-23T02:27:33.449545Z', ExpirationTime '2026-10-23T03:27:33.449545Z').
Why: a client that caches a prepared template, or schedules a re-prepare from the reported ExpirationTime, treats the artifact as dead five hours early, and any test that asserts the documented window fails.
Fix: the prepared-template lifetime constant in the service state, and the `expires_at`/`status_at` derivation that reads it, so the PREPARING -> ACTIVE -> EXPIRED transitions all follow the corrected window.
Traces: serverlessapplicationrepository.cloudformation-template-expiry-window

=== Cause 3 of 4: PutApplicationPolicy accepts a statement naming the application's own owner as principal ===
AWS: PutApplicationPolicy models BadRequestException (HTTP 400, "One of the parameters in the request is invalid"). Recorded behavior: a statement whose `Principals` is the application owner's own account id is refused with code `BadRequestException`, HTTP 400, message `You cannot set principal to resource owner`; the previously granted policy is left exactly as it was (PutApplicationPolicy otherwise REPLACES the whole statement list).
Emulator: the put SUCCEEDS. `state::validate_policy_statements` checks only that each principal is `*` or twelve digits (`is_principal`), so the owner's own 12-digit account id passes and the statement is stored, replacing the previously granted policy. The replay reports `AssertionError: PutApplicationPolicy succeeded; real AWS refuses this statement` (the positive control, a well-formed Deploy grant to another account, was accepted first, so the divergence is attributable to this one varied field).
Why: a publisher can store a self-grant that means nothing, and because the accepted put replaces the whole statement list it silently destroys the real grants the policy carried — the client believes it shared the application when it has instead un-shared it.
Fix: the policy-statement validation in the service state, which needs the caller's own account id (the request's credential scope) to refuse an owner principal before anything is written; the pre-write ordering is already correct.
Traces: serverlessapplicationrepository.rejects-unshareable-policy-statement

=== Cause 4 of 4: the refusal wording for an invalid action or principal is not the request-built message AWS produces ===
AWS: the three unshareable-statement classes share one code (BadRequestException, HTTP 400) and are distinguished only by a message built from the request, naming the offending value: `Action Undeploy is invalid` for an action outside SAR's vocabulary and `Principal not-an-account is invalid` for a principal that is not an account id (recorded behavior; the rule is "name the rejected member value", so it is a requirement rather than incidental text).
Emulator: the same two classes are refused, but with catalogue-dump wording of its own: `Actions must each be one of the application permissions [CreateCloudFormationChangeSet, CreateCloudFormationTemplate, Deploy, ...], got Undeploy` and `Principals must each be a 12-digit AWS account ID or *, got not-an-account`. The replay stops at cause 3 before reaching these asserts, so they are the next failures this check hits once cause 3 is fixed.
Why: BadRequestException is one code for every refusal reason, so a client that classifies which field it must correct can only do so from the message; the emulator's wording makes those three refusals indistinguishable from any other 400 this service returns.
Fix: the refusal messages emitted by the policy-statement validation, alongside cause 3, in the same place that builds them today.
Traces: serverlessapplicationrepository.rejects-unshareable-policy-statement

## Evidence
Suite: simparity-behavioral-aws-v1 — serverlessapplicationrepository 3/9 passed (6 failed).

1) [error] serverlessapplicationrepository.sar-prepared-template-serves-the-published-body
  File "request.py", line 613, in http_error_default
    raise HTTPError(req.full_url, code, msg, hdrs, fp)
  urllib.error.HTTPError: HTTP Error 404: Not Found
  (raised by the plain GET of active["TemplateUrl"]; expected HTTP 200 serving the published SAM template)

2) [mismatch] serverlessapplicationrepository.cloudformation-template-expiry-window
  assert window == datetime.timedelta(hours=6)
  AssertionError: ExpirationTime '2026-10-23T03:27:33.449545Z' is 1:00:00 after CreationTime '2026-10-23T02:27:33.449545Z'; SAR expires a generated template exactly 6 hours after it is created
  Emulator source: state.rs — `pub const TEMPLATE_LIFETIME: Duration = Duration::from_secs(60 * 60);` (~line 1375) and `expires_at: now + TEMPLATE_LIFETIME` in `CloudFormationTemplate::prepared` (~line 1450).

3) [mismatch] serverlessapplicationrepository.rejects-unshareable-policy-statement
  File "check_serverlessapplicationrepository_rejects_unshareable_policy_statement.py", line 124, in run
    code, message = _error(lambda s=statements: sar.put_application_policy(ApplicationId=app_id, Statements=s))
  AssertionError: PutApplicationPolicy succeeded; real AWS refuses this statement
  Expected: BadRequestException / HTTP 400 / "You cannot set principal to resource owner".
  Emulator source: src/ops/put_application_policy.rs calls `crate::state::validate_policy_statements`; that function's principal rule is `fn is_principal(p) { p == "*" || (p.len() == 12 && all ascii digits) }` (state.rs ~line 771) — no owner check.

4) Wording, same check: state.rs ~lines 699-713 emit "Actions must each be one of the application permissions [...], got <action>" and "Principals must each be a 12-digit AWS account ID or *, got <principal>" where the recording shows "Action <value> is invalid" / "Principal <value> is invalid".

Smithy model consulted (services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json): PutApplicationPolicy PUT /applications/{ApplicationId}/policy, errors BadRequestException(400)/ForbiddenException(403)/NotFoundException(404)/TooManyRequestsException(429)/InternalServerErrorException(500); CreateCloudFormationTemplate POST /applications/{ApplicationId}/templates (201) and GetCloudFormationTemplate GET /applications/{ApplicationId}/templates/{TemplateId} (200), both returning TemplateUrl/CreationTime/ExpirationTime/Status; doc CreateCloudFormationTemplateResponse$ExpirationTime = "The date and time this template expires. Templates expire 1 hour after creation."

## Steps to Reproduce
Boot the emulator and point a serverlessrepo client at it.

Causes 1 and 2:
1. CreateApplication with a SAM TemplateBody (Transform: AWS::Serverless-2016-10-31) and SemanticVersion 1.0.0.
2. Poll GetApplication until version 1.0.0 is readable.
3. CreateCloudFormationTemplate(ApplicationId, SemanticVersion="1.0.0"); poll GetCloudFormationTemplate until Status == ACTIVE.
4. Compute ExpirationTime - CreationTime -> observe 1:00:00, expected exactly 6:00:00 (cause 2).
5. Perform an unsigned HTTPS GET of the returned TemplateUrl -> observe HTTP 404, expected HTTP 200 whose YAML-parsed body equals the published template (cause 1).

Causes 3 and 4:
6. On the same application, PutApplicationPolicy with one well-formed statement {StatementId: "control", Actions: ["Deploy"], Principals: ["<some other account>"]} -> accepted (control).
7. PutApplicationPolicy with {Actions: ["Deploy"], Principals: ["<the caller's own account from sts:GetCallerIdentity>"]} -> observe HTTP 200 and the control policy replaced; expected HTTP 400 BadRequestException "You cannot set principal to resource owner" with the control policy untouched (cause 3).
8. PutApplicationPolicy with Actions: ["Undeploy"], then with Principals: ["not-an-account"] -> both are refused 400 BadRequestException but with catalogue-dump messages instead of "Action Undeploy is invalid" / "Principal not-an-account is invalid" (cause 4).

Replay: serverlessapplicationrepository.sar-prepared-template-serves-the-published-body, serverlessapplicationrepository.cloudformation-template-expiry-window, serverlessapplicationrepository.rejects-unshareable-policy-statement
