Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.invoke-model-missing-permission-denied
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime.invoke-model-missing-permission-denied|_poll#18|AssertionError|raise AssertionError(f"{description} not reached within {timeout_s}s; last observation {last!r}")
Issue-Commit: e68df329f2ba7326cf40e2227a2656e4e66e8fc6

# bedrock-runtime data plane applies no authorization gate: InvokeModel/Converse keep succeeding for a principal with no bedrock:InvokeModel grant (1 cause)

> _Note: this issue was identified against the code base at commit `e68df329f2ba7326cf40e2227a2656e4e66e8fc6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeModel

## Description
ONE CAUSE on the bedrock-runtime synchronous data plane. (Related open issue: 20261023T232022Z-bedrock-runtime-synchronous-inference-guardrailconfig-and-toolconfig-silently.md describes the same authorization gap as its Cause 4, but that issue does not carry this check's current replay key — there the check died on a 60 s harness timeout inside the post-deletion poll, whereas it now runs to completion and reports the concrete observation 'UNEXPECTED_SUCCESS'. It is filed here so the surviving failure is tracked against an id a fixer can replay; if that issue is being worked, close both together.)

Cause 1 of 1: InvokeModel and Converse are served to a signing principal that holds no Allow for the action, so the emulator never produces AccessDeniedException (OVER-PERMISSIVE, security-relevant).
AWS: Smithy model services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json models AccessDeniedException (errorKind client, httpError 403) on BOTH InvokeModel (POST /model/{modelId}/invoke) and Converse (POST /model/{modelId}/converse). A sigv4-signed call by an IAM user whose only grant for bedrock:InvokeModel / bedrock:Converse has been deleted is an implicit deny and is refused 403 AccessDeniedException; re-attaching the identical inline Allow makes the same call succeed again. The refusal message is AWS's standard implicit-deny sentence for a data-plane action, whose FIXED text is "User: ", " is not authorized to perform: ", " on resource: ", " because no identity-based policy allows the ", " action" and whose VARIABLE parts come from the request and state: the signing user's ARN, the action name (bedrock:InvokeModel or bedrock:Converse), and the foundation-model ARN built from the request's region and modelId (arn:aws:bedrock:<region>::foundation-model/<modelId>). The ARN, user name, account and region in any one recording are values filling that template, never part of it. Consistently, IAM SimulatePrincipalPolicy must report EvalDecision 'implicitDeny' for those two actions once the Allow is gone.
Emulator: after the inline Allow is deleted, the real signed InvokeModel keeps returning 200 with a normal inference body. The check's bounded poll for the refusal never converges and fails with "AssertionError: InvokeModel is refused once the permission is missing not reached within 90s; last observation 'UNEXPECTED_SUCCESS'" — i.e. no 403, no AccessDeniedException, no error code at all, for the whole 90 s window. The emulator serves the data-plane call without consulting the signing principal's effective policy.
Why: authorization is unobservable on the Bedrock data plane. Every negative-path test of a client's least-privilege policy (confirming a role CANNOT invoke a model, confirming a revoked grant takes effect, confirming a scoped-down session is denied) passes against the emulator and fails in production; conversely a client wired to catch AccessDeniedException and fall back never exercises that branch. An emulator that never denies cannot be used to validate IAM posture for model invocation, which is exactly the control most Bedrock deployments rely on to fence off expensive or sensitive models.
Fix: the authorization stage of the InvokeModel and Converse handlers in services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice — resolve the sigv4 signing principal, evaluate its effective identity-based policy for bedrock:InvokeModel / bedrock:Converse on the target model resource, and on an implicit deny emit the modelled AccessDeniedException with httpError 403 and the implicit-deny message template above. The same gate belongs on InvokeModelWithResponseStream and ConverseStream so the streaming variants cannot be used to bypass it.
Traces: bedrock-runtime.invoke-model-missing-permission-denied.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, bedrock-runtime 31/84 passed. The emulator answered these calls (200s were returned by the signed client), so the service is up and answering wrongly — this is not a bootstrap gap.

Cause 1 — bedrock-runtime.invoke-model-missing-permission-denied [mismatch]:
  File "check_bedrock_runtime_invoke_model_missing_permission_denied.py", line 147, in run
    after_invoke = _poll(
  File "check_bedrock_runtime_invoke_model_missing_permission_denied.py", line 64, in _poll
    raise AssertionError(f"{description} not reached within {timeout_s}s; last observation {last!r}")
  AssertionError: InvokeModel is refused once the permission is missing not reached within 90s; last observation 'UNEXPECTED_SUCCESS'

The check's own control legs passed first: with the inline Allow attached, the signed InvokeModel and Converse both succeeded, so the credentials and the signing path work. The failure is localised to the leg after the Allow is deleted, where the expected error code is AccessDeniedException and the observed outcome is a successful 200 ('UNEXPECTED_SUCCESS' is the check's sentinel for "the call did not raise").

Expected vs actual:
  expected: HTTP 403, Error.Code 'AccessDeniedException', Error.Message = "User: <signing-user-arn> is not authorized to perform: bedrock:InvokeModel on resource: arn:aws:bedrock:<region>::foundation-model/<modelId> because no identity-based policy allows the bedrock:InvokeModel action" (fixed text as quoted; user ARN, region, modelId and action taken from the request and emulator IAM state). Same for bedrock:Converse on the Converse leg.
  actual: HTTP 200 with a normal inference response body; no error code emitted on repeated attempts over 90 s.

Model evidence: smithy_query query=operation operation=InvokeModel,Converse on services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json — both operations list {"shape": "AccessDeniedException", "errorKind": "client", "httpError": 403}.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Re-replay the check id in trace_ids (bedrock-runtime.invoke-model-missing-permission-denied).
3. Manually: iam.create_user(UserName=<u>), iam.put_user_policy(UserName=<u>, PolicyName='sim2real-behav-bedrock-invoke-allow', PolicyDocument={Allow bedrock:InvokeModel, bedrock:Converse on '*'}), iam.create_access_key(UserName=<u>).
4. Build a bedrock-runtime client signing with that access key and call invoke_model(modelId='amazon.nova-lite-v1:0', contentType='application/json', body=<nova text body>) and converse(modelId='amazon.nova-lite-v1:0', messages=[{'role':'user','content':[{'text':'Say the word yes and nothing else.'}]}]). Both should succeed (control).
5. iam.delete_user_policy(UserName=<u>, PolicyName='sim2real-behav-bedrock-invoke-allow').
6. Re-issue the same signed invoke_model and converse calls. Expected: 403 AccessDeniedException with the implicit-deny message naming the user ARN, the action and arn:aws:bedrock:<region>::foundation-model/amazon.nova-lite-v1:0. Observed: both keep returning 200 indefinitely.
7. iam.simulate_principal_policy(PolicySourceArn=<user arn>, ActionNames=['bedrock:InvokeModel','bedrock:Converse']) — expected EvalDecision 'implicitDeny' for both.
8. Re-attach the identical Allow and confirm the calls succeed again, so the refusal in step 6 is attributable to the missing grant and nothing else.
