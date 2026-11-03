Issue-Source: trace-sweep-divergence
Issue-Trace: ram-permission-error-paths
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ram-permission-error-paths
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram|CreatePermission|4xx->4xx|$.message
Issue-Key: ram|CreatePermission|4xx->4xx|$.__type
Issue-Key: ram|CreatePermission|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: d36bb525c2331ae923943e9e5ae02c5704a8c1d0

# RAM CreatePermission refusals use the wrong exception class and emulator-authored wording

> _Note: this issue was identified against the code base at commit `d36bb525c2331ae923943e9e5ae02c5704a8c1d0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePermission

## Description
Both CreatePermission refusal paths exercised by the corpus answer with text (and in one case a class) the emulator invented rather than RAM's. 2 causes.

Cause 1 of 2 — duplicate permission name: right class and status, wrong message (fixture ram-permission-error-paths, step task#2).
AWS: CreatePermission with a name that already exists in the Region fails with PermissionAlreadyExistsException (Smithy: CreatePermission errors include PermissionAlreadyExistsException, httpError 409; shape doc: 'The operation failed because a permission with the specified name already exists in the requested Amazon Web Services Region. Choose a different name.'). The recorded wire message identifies the clashing permission by ARN: fixed text 'The permission ' + <permission ARN> + ' already exists.' The variable part is the ARN of the existing permission, built from the request's name plus the region and account of the caller (in the recording arn:aws:ram:us-west-2:000000000000:permission/sim2real-probe-ramperm-erro-92756ec5); that ARN value is from one run and is not itself the requirement.
Emulator: status 409 and the right error class, but message "A permission named '<name>' already exists in Region <region>. Choose a different name." — a paraphrase of the shape's documentation, naming the bare name and region instead of the existing permission's ARN.
Why: clients and tests that match on the refusal text, or parse the ARN out of it to fetch the conflicting permission, break; the message is the only place the existing resource is identified.
Fix: the CreatePermission handler's already-exists refusal message construction (render the ARN of the stored conflicting permission, not the request's name and region).

Cause 2 of 2 — malformed policy template: wrong exception class, wrong errortype header and wrong message (fixture ram-permission-error-paths, step task#3).
AWS: CreatePermission models both InvalidPolicyException (Smithy: httpError 400, awsQueryError code InvalidPolicy, doc 'The operation failed because a policy you specified isn't valid.') and MalformedPolicyTemplateException (httpError 400, doc 'The operation failed because the policy template that you provided isn't valid.'). For the policyTemplate the recording submits — a template whose Effect element is not a string — RAM answers 400 InvalidPolicyException with message 'InvalidPolicy: Invalid policy format.' and the matching x-amzn-errortype header. The message is fixed text for this error class (no request-derived values in it).
Emulator: 400 with __type 'MalformedPolicyTemplateException', x-amzn-errortype 'MalformedPolicyTemplateException' and message 'policyTemplate must carry a string Effect element' — a validator-authored sentence under the wrong one of the two modeled policy errors.
Why: both errors are 400s but distinct classes, so a client that catches InvalidPolicyException (or dispatches on x-amzn-errortype) sees an unexpected exception type and either escapes the handler or retries a non-retryable request; the diagnostic text also does not match anything RAM emits.
Fix: the CreatePermission policy-template validation — which modeled error it raises for an ill-formed policy document, the x-amzn-errortype/__type it serializes, and the message text.

Traces: ram-permission-error-paths#task#2; ram-permission-error-paths#task#3.

## Evidence
Cause 1 — ram-permission-error-paths#task#2, CreatePermission, expected 409 / actual 409: $.message expected 'The permission arn:aws:ram:us-west-2:000000000000:permission/sim2real-probe-ramperm-erro-92756ec5 already exists.', got "A permission named 'sim2real-probe-ramperm-erro-92756ec5' already exists in Region us-west-2. Choose a different name."
Cause 2 — ram-permission-error-paths#task#3, CreatePermission, expected 400 / actual 400: $.__type expected 'InvalidPolicyException', got 'MalformedPolicyTemplateException'; $.message expected 'InvalidPolicy: Invalid policy format.', got 'policyTemplate must carry a string Effect element'; $headers.x-amzn-errortype got 'MalformedPolicyTemplateException'.

## Steps to Reproduce
1. CreatePermission with name N, a valid resourceType and policyTemplate -> 200.
2. CreatePermission again with the same name N -> emulator answers 409 PermissionAlreadyExistsException with 'A permission named ... already exists in Region ...' instead of 'The permission <arn> already exists.' (ram-permission-error-paths#task#2).
3. CreatePermission with a policyTemplate whose Effect element is not a string -> emulator answers 400 MalformedPolicyTemplateException / 'policyTemplate must carry a string Effect element' instead of 400 InvalidPolicyException / 'InvalidPolicy: Invalid policy format.' with x-amzn-errortype InvalidPolicyException (ram-permission-error-paths#task#3).
