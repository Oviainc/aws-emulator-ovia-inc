Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehub-assessment-preconditions
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehub-assessment-preconditions
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub|StartAppAssessment|4xx->4xx|$.message
Issue-Commit: 3157dd4d1552792a0982de1a1298281fbb744f2b

# resiliencehub StartAppAssessment unreleased-version refusal message diverges from AWS wording

> _Note: this issue was identified against the code base at commit `3157dd4d1552792a0982de1a1298281fbb744f2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAppAssessment

## Description
One cause: StartAppAssessment refuses an assessment against an app-version label the application does not carry with the right HTTP status and error type, but with its own paraphrased `message` instead of AWS's. The message is a requirement of the error class, so this is a client-visible divergence.

=== Cause 1 of 1: StartAppAssessment invalid/unavailable app-version ResourceNotFoundException message ===
AWS: StartAppAssessment takes a required `appVersion` (Smithy: StartAppAssessmentRequest.appVersion -> EntityVersion, required, pattern ^\\S{1,50}$; appArn -> Arn, required; assessmentName -> EntityName, required) and models ResourceNotFoundException with httpError 404 (Smithy: StartAppAssessment errors = AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ThrottlingException 429, ValidationException 400). Starting an assessment for an app-version label that is neither the app's draft nor a published release is refused with ResourceNotFoundException / HTTP 404 carrying (recorded behavior; the model documents the error but not its text) the message `Invalid app version <appVersion> for App <appName> (<appArn>). App version has to be draft or release.` The FIXED text is `Invalid app version `, ` for App `, ` (`, `). App version has to be draft or release.` — note the capitalised `App`, the ARN in parentheses after the name, and the closing remediation sentence. The VARIABLE parts are taken from the request and the emulator's state: the request's `appVersion` label, the application's `name` from state, and the request's `appArn`.
Emulator: HTTP 404 with `App version release not found for appArn <appArn>` — different fixed text (`App version `, ` not found for appArn `), it drops the application NAME entirely, it drops the parenthesised-ARN framing, and it omits the `App version has to be draft or release.` remediation sentence altogether.
Why: a client that submits an assessment before publishing cannot tell from the emulator's text WHY the version was rejected — AWS's message states the precondition (the label must be `draft` or a published release) and names the application, which is the only thing tying the failure to one app when a caller fans assessments out over many applications. Any retry wrapper or test that matches AWS's `Invalid app version ... App version has to be draft or release.` wording sees an unmatched message, classifies the 404 as a missing-application error instead of a missing-release one, and retries or aborts instead of calling PublishAppVersion first.
Fix: the app-version resolution / precondition error construction in the StartAppAssessment handler — it must raise the invalid-app-version refusal (interpolating the requested version label, the stored app name and the request's appArn) rather than reusing the generic app-version-lookup not-found message.
Traces: resiliencehub-assessment-preconditions#task#5.

Related open issues that do NOT cover this cause: `20261029T195027Z-resiliencehub-refusal-messages-diverge-from-aws-wording-across.md` bundles seven message-wording causes on CreateApp, DescribeApp, CreateAppVersionResource, DescribeResiliencyPolicy and DeleteResiliencyPolicy, none of them StartAppAssessment; `20261030T040357Z-resiliencehub-startappassessment-stub.md` covers StartAppAssessment being an unimplemented scaffold (the operation now answers, so this wording defect is a separate, live divergence); `20261104T205441Z-resiliencehub-startappassessment-omits-resiliencyscore-componentscore-so-a-finished.md` covers the success-path response shape, not this refusal path.

## Evidence
Replay of resiliencehub-assessment-preconditions#task#5 (StartAppAssessment). The step returned the EXPECTED status 404 (and the expected error type); only `$.message` diverges.

  expected: 'Invalid app version <appVersion> for App <appName> (<appArn>). App version has to be draft or release.'
  actual:   'App version <appVersion> not found for appArn <appArn>'

Verbatim replay detail for the step:
  $.message: value_mismatch (expected 'Invalid app version release for App sim2real-probe-rh-asmt-app-1ea4d3b7 (arn:aws:resiliencehub:us-west-2:000000000000:app/b1b6b7fc-25b1-4521-9155-ff104afb700f). App version has to be draft or release.', got 'App version release not found for appArn arn:aws:resiliencehub:us-west-2:000000000000:app/b1b6b7fc-25b1-4521-9155-ff104afb700f')
  divergence_key: service=resiliencehub|StartAppAssessment|status=404->404|keys=$.message
  expected_status 404, actual_status 404; verdict mismatch; owned_by: (none); stub_path: (none)

Requirement vs per-run value: the app name `sim2real-probe-rh-asmt-app-1ea4d3b7`, the app id in the ARN `b1b6b7fc-25b1-4521-9155-ff104afb700f`, the region `us-west-2` and the account `000000000000` are per-run values filling the template; the requirement is the fixed text `Invalid app version `, ` for App `, ` (`, `). App version has to be draft or release.` and the identity of each variable slot (requested appVersion label, stored app name, request appArn).

Smithy citations (services/service-com-amazonaws-resiliencehub-awsresiliencehub/smithy-model/com-amazonaws-resiliencehub-awsresiliencehub.json): StartAppAssessment POST /start-app-assessment, success code 200; input appArn (Arn, required), appVersion (EntityVersion, required, pattern ^\S{1,50}$), assessmentName (EntityName, required); error ResourceNotFoundException with errorKind client / httpError 404.

## Steps to Reproduce
1. CreateApp(name=<appName>) — leave the application with only a draft app version (never call PublishAppVersion).
2. StartAppAssessment(appArn=<the new app's arn>, appVersion="release", assessmentName=<name>).
3. Observe HTTP 404 with message 'App version release not found for appArn <appArn>' instead of AWS's 'Invalid app version release for App <appName> (<appArn>). App version has to be draft or release.' — the application name, the parenthesised ARN and the 'App version has to be draft or release.' remediation sentence are all missing.
4. Full replay: call replay_wire_corpus() and re-check resiliencehub-assessment-preconditions#task#5.
