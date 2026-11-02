Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns.snstrig-e2e-find-sns-triggered-lambda-functions-lifecycle lambda__sns.snstrig-explicit-latest-qualifier-mints-a-second-subscription-that-can-never-be-granted lambda__sns.snstrig-filter-policy-rejected-when-not-json
Issue-Key: lambda__sns|GetPolicy|ResourceNotFoundException
Issue-Key: lambda__sns.snstrig-explicit-latest-qualifier-mints-a-second-subscription-that-can-never-be-granted|_error#14|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: lambda__sns.snstrig-filter-policy-rejected-when-not-json|run#9|AssertionError|assert message == expected_message, f"expected {expected_message!r}, got {message!r}"
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# lambda__sns: 3 net-new root causes — a CFN-deployed Lambda function does not exist, GetPolicy on a :$LATEST-qualified ARN returns the unqualified policy instead of refusing, and the SNS FilterPolicy parse refusal carries an extra "Attributes Reason:" segment

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPolicy

## Description
This issue bundles THREE net-new root causes of service `lambda__sns`. All 3 must be closed before submitting. None of the three grounding checks was marked ALREADY FILED or OWNED by the replay report, and there is no open issue for this service token.

=== Cause 1 of 3: a function declared by a CloudFormation stack that reaches CREATE_COMPLETE does not exist in Lambda ===
AWS: a stack that reaches CREATE_COMPLETE with an AWS::Lambda::Function has registered that function with Lambda, so a read of it answers. Smithy (lambda) GetPolicy takes FunctionName as an HTTP label (NamespacedFunctionName, length 1-256, the ARN pattern) and declares ResourceNotFoundException at HTTP 404 only for a function that is genuinely absent; for a deployed function GetPolicy answers 200 with the Policy document (or the resource-policy-absent refusal), never "Function not found".
Emulator: during the deploy phase, GetPolicy on the function ARN the stack reports is refused 404 `ResourceNotFoundException` with `Function not found: arn:aws:lambda:<region>:<account>:function:<name>` — the message's fixed text is "Function not found: " followed by the ARN from the request — so the whole four-phase lifecycle aborts at phase 1.
Why: a green deploy is immediately followed by not-found on the resource the stack claims to own, so no estate-introspection workflow ("which functions does SNS trigger?"), no permission read and no teardown verification can run against CloudFormation-managed functions. The open issues 20261023T004328Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-stacks-declaring-aws-lambda-function-reach-create.md and 20261025T022656Z-lambda-sns-sqs-sns-sqs-to-lambda-delivery.md (its Cause 2) record the same provisioning gap for other service tokens and neither carries this check's id; they should be closed together.
Fix: the CloudFormation provisioner for AWS::Lambda::Function — create the function through the same path lambda:CreateFunction takes, so the ARN the stack reports resolves in the Lambda service (and the subscriptions/grants the same template declares resolve against it).
Traces: lambda__sns.snstrig-e2e-find-sns-triggered-lambda-functions-lifecycle.

=== Cause 2 of 3: GetPolicy on `<function-arn>:$LATEST` answers the unqualified function's policy instead of refusing ===
AWS: `$LATEST` is a distinct, explicitly-qualified identity. Smithy (lambda) GetPolicy accepts a qualifier inside FunctionName (NamespacedFunctionName pattern `…(:(\$LATEST(\.PUBLISHED)?|[a-zA-Z0-9-_]+))?$`) and declares ResourceNotFoundException at HTTP 404. A grant added with AddPermission against the UNQUALIFIED function does NOT cover the `:$LATEST` identity: GetPolicy on `<arn>:$LATEST` is refused 404 ResourceNotFoundException with the message `The resource you requested does not exist.` — entirely fixed text, no values interpolated. (And `$LATEST` is the one qualifier AddPermission refuses outright, with InvalidParameterValueException and `We currently do not support adding policies for $LATEST.`, which is why an SNS subscription minted against the `:$LATEST` endpoint can never be granted.)
Emulator: GetPolicy on `<arn>:$LATEST` SUCCEEDS, returning the unqualified function's policy — the qualifier is dropped and the two identities are conflated, so the grant appears to cover a scope AWS says it does not.
Why: a client reading GetPolicy to decide whether a subscription endpoint is authorized gets the opposite answer from the account: here the `:$LATEST` endpoint looks granted, in AWS it can never be granted at all, so an SNS→Lambda trigger that silently never fires in production reads as correctly permissioned in test.
Fix: the GetPolicy handler's function resolution — keep the qualifier from the FunctionName label (and the Qualifier query member), look the policy up for that exact qualified identity, and refuse an unknown one with 404 ResourceNotFoundException / "The resource you requested does not exist."; the same resolution must make AddPermission refuse the $LATEST qualifier with InvalidParameterValueException and "We currently do not support adding policies for $LATEST."
Traces: lambda__sns.snstrig-explicit-latest-qualifier-mints-a-second-subscription-that-can-never-be-granted.

=== Cause 3 of 3: the SNS SetSubscriptionAttributes FilterPolicy parse refusal interpolates an extra "Attributes Reason: " segment ===
AWS: Smithy (sns) SetSubscriptionAttributes declares InvalidParameterException (errorKind client, httpError 400, awsQueryError code `InvalidParameter`). A FilterPolicy value that is not JSON is refused with that code and the message `Invalid parameter: FilterPolicy: failed to parse JSON. <the Jackson parse diagnostic>` — the fixed text is `Invalid parameter: ` + the attribute name + `: failed to parse JSON. `, and the variable part is the parser's own diagnostic naming the offending character, its code and the line/column, taken from the submitted value. The prefix `Attributes Reason: ` belongs to refusals that name the ATTRIBUTES map as the offending parameter (for example a bad RawMessageDelivery value), not to a parse failure attributed to the named attribute itself. Nothing is stored: GetSubscriptionAttributes reports no FilterPolicy afterwards, and a valid policy then round-trips.
Emulator: the code is right and the diagnostic text is right, but the message carries the extra segment — `Invalid parameter: Attributes Reason: FilterPolicy: failed to parse JSON. Unexpected character ('k' (code 107)): was expecting double-quote to start field name…` instead of `Invalid parameter: FilterPolicy: failed to parse JSON. …`.
Why: the refusal message is the only machine-readable identification of WHICH parameter SNS rejected; clients and test suites match on the `Invalid parameter: <Name>:` prefix, so the inserted segment makes a parse failure on FilterPolicy look like a failure attributed to the Attributes map and breaks that matching.
Fix: the SetSubscriptionAttributes (and Subscribe Attributes) validation path that formats InvalidParameterException messages — emit the `Attributes Reason: ` segment only for refusals whose offending parameter IS the attributes map, and attribute a per-attribute parse failure directly to the attribute name.
Traces: lambda__sns.snstrig-filter-policy-rejected-when-not-json.

## Evidence
Cause 1 — lambda__sns.snstrig-e2e-find-sns-triggered-lambda-functions-lifecycle [error]:
  _PhaseError: [phase 1/4 deploy] ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetPolicy operation: Function not found: arn:aws:lambda:us-west-2:<account>:function:sim2real-scn-snstrig-e2e-<hex>-app-fn
  expected: after the stack (one role, three inline functions, two topics) reaches CREATE_COMPLETE, GetPolicy on each declared function answers.

Cause 2 — lambda__sns.snstrig-explicit-latest-qualifier-mints-a-second-subscription-that-can-never-be-granted [mismatch]:
  AssertionError: the call succeeded; real AWS rejects it, so there is no error code
  (raised from `_error(lambda: lam.get_policy(FunctionName=f"{fn_arn}:$LATEST"))`)
  expected: 404 ResourceNotFoundException with Message 'The resource you requested does not exist.' after AddPermission was granted only on the unqualified ARN; and AddPermission(Qualifier='$LATEST') refused InvalidParameterValueException 'We currently do not support adding policies for $LATEST.'
  actual: GetPolicy on the ':$LATEST' ARN returned 200 with the unqualified function's policy.

Cause 3 — lambda__sns.snstrig-filter-policy-rejected-when-not-json [mismatch]:
  AssertionError: expected "Invalid parameter: FilterPolicy: failed to parse JSON. Unexpected character ('k' (code 107)): was expecting double-quote to start field name\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 2]", got "Invalid parameter: Attributes Reason: FilterPolicy: failed to parse JSON. Unexpected character ('k' (code 107)): was expecting double-quote to start field name\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 2]"
  (error code InvalidParameter matched; only the message's leading segment diverges)

## Steps to Reproduce
Boot the emulator and replay the three check ids in trace_ids from suite simparity-behavioral-aws-v1.

By hand:
1. Cause 1: CreateStack with a template declaring an AWS::IAM::Role and AWS::Lambda::Function (inline code) plus AWS::SNS::Topic/Subscription resources; wait for CREATE_COMPLETE; call lambda GetPolicy(FunctionName=<the function ARN from the stack>) — observe 404 ResourceNotFoundException "Function not found: <arn>".
2. Cause 2: create a function and a topic; Subscribe(Protocol='lambda', Endpoint=<fn arn>) and again with Endpoint=<fn arn>:$LATEST (two distinct subscriptions); lambda AddPermission on the UNQUALIFIED function for sns.amazonaws.com; then GetPolicy(FunctionName=<fn arn>) (expect the grant) and GetPolicy(FunctionName=<fn arn>:$LATEST) — expect 404 'The resource you requested does not exist.', observe 200 with the unqualified policy. Also call AddPermission(..., Qualifier='$LATEST') — expect InvalidParameterValueException 'We currently do not support adding policies for $LATEST.'
3. Cause 3: Subscribe a function to a topic, then sns SetSubscriptionAttributes(SubscriptionArn=<sub>, AttributeName='FilterPolicy', AttributeValue='{kind: order') — the code is InvalidParameter but the message carries the extra 'Attributes Reason: ' segment. Confirm nothing was stored (GetSubscriptionAttributes has no FilterPolicy) and that a valid '{"kind": ["order"]}' then round-trips.
