Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.cfn-deploy-lambda-sesv2-identity-resource-positive lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email lambda__sesv2.cli-lambda-sesv2-missing-policy-negative lambda__sesv2.cli-lambda-sesv2-update-role-policy-positive lambda__sesv2.cfn-lambda-sesv2-trust-policy-positive lambda__sesv2.lambda-sesv2-send-email-response-format lambda__sesv2.lambda-sesv2-get-suppressed-destination-positive lambda__sesv2.sesv2-eventbridge-bounce-event-data-format lambda__sesv2.sesv2-eventbridge-send-event-lambda-invocation-positive lambda__sesv2.sesv2-eventbridge-rule-missing-resource-policy-negative lambda__sesv2.sesv2-eventbridge-wrong-event-pattern-negative
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-identity-resource-positive|_deploy_cfn#5|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email|_run_shell#10|AssertionError|raise AssertionError(
Issue-Key: lambda__sesv2.cli-lambda-sesv2-missing-policy-negative|_run_shell#4|AssertionError|raise AssertionError(
Issue-Key: lambda__sesv2.cli-lambda-sesv2-update-role-policy-positive|_run_shell#10|AssertionError|raise AssertionError(
Issue-Key: lambda__sesv2.cfn-lambda-sesv2-trust-policy-positive|_find_verified_sender#1|AssertionError|raise AssertionError(
Issue-Key: lambda__sesv2.lambda-sesv2-send-email-response-format|_verified_sender#1|AssertionError|raise AssertionError(
Issue-Key: lambda__sesv2.lambda-sesv2-get-suppressed-destination-positive|_wait_suppressed#10|Timeout|time.sleep(5)
Issue-Key: lambda__sesv2.sesv2-eventbridge-bounce-event-data-format|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sesv2.sesv2-eventbridge-send-event-lambda-invocation-positive|_drain_relay_events#12|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__sesv2.sesv2-eventbridge-rule-missing-resource-policy-negative|_drain_for#10|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__sesv2.sesv2-eventbridge-wrong-event-pattern-negative|_drain_for#11|Timeout|resp = sqs.receive_message(
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# lambda__sesv2: 3 net-new root causes — a CFN stack declaring AWS::SES::EmailIdentity rolls back, lambda Invoke through the CLI is refused NotImplemented, and no SES sending identity ever reaches verified so seven checks stall out

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Three distinct net-new root causes behind the surviving lambda__sesv2 failures (the other lambda__sesv2 failures in this run are already owned by 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md, 20261023T004328Z-bedrock-agent-runtime-lambda-s3vectors-... and 20261023T183615Z-lambda-sesv2-lambda-driven-sesv2-scenarios-all-fail.md and are not re-filed here).

Cause 1 of 3 — a CloudFormation stack that declares an AWS::SES::EmailIdentity (with ConfigurationSetAttributes referencing an AWS::SES::ConfigurationSet in the same stack) never reaches CREATE_COMPLETE: it ends in ROLLBACK_COMPLETE.
AWS: recorded behavior only (the CFN resource-provider coverage is not in the Smithy model). Smithy (com.amazonaws.cloudformation): CreateStack POST / (awsQuery) -> {StackId}; DescribeStacks reports Stacks[].StackStatus. Recorded: a stack holding AWS::SES::ConfigurationSet + AWS::SES::EmailIdentity (Properties EmailIdentity, ConfigurationSetAttributes.ConfigurationSetName = {"Ref": "ConfigSet"}, Tags) + AWS::IAM::Role + AWS::Lambda::Function reaches CREATE_COMPLETE, `Ref` on the AWS::SES::EmailIdentity resolves to the identity name (consumed here as a Lambda environment variable), and sesv2 GetEmailIdentity afterwards reports ConfigurationSetName equal to the created configuration set. The requirement is general: every resource type the template declares is provisioned, or the failure is reported as a resource-level reason — not a whole-stack rollback.
Emulator: DescribeStacks settles on StackStatus `ROLLBACK_COMPLETE`, so the check aborts with `AssertionError: Stack <name> ended in ROLLBACK_COMPLETE` before any Lambda invoke. Hypothesis: the AWS::SES::EmailIdentity resource type (or its ConfigurationSetAttributes property / its Ref resolution) is unsupported by the stack engine, and the resource failure rolls the whole stack back. Note the sibling CFN checks in this suite fail one call later and are owned by 20261023T004328Z-... (AWS::Lambda::Function reaching CREATE_FAILED); that issue does not carry this check and does not name the SES identity resource.
Why: a client cannot deploy the ordinary SES+Lambda stack shape at all — the identity/configuration-set association CFN is supposed to create never exists, and the stack reports a blanket rollback that names no cause.
Fix: the cloudformation stack engine's resource-provider coverage for AWS::SES::EmailIdentity (creation, ConfigurationSetAttributes handling, Ref/GetAtt resolution) and its per-resource failure reporting.
Traces: lambda__sesv2.cfn-deploy-lambda-sesv2-identity-resource-positive.

Cause 2 of 3 — lambda Invoke is refused NotImplemented for a python3.12 deployment package, reached here through the AWS CLI, so every CLI round-trip check dies with rc=254.
AWS: Smithy (com.amazonaws.lambda): Invoke POST /2015-03-31/functions/{FunctionName}/invocations -> 200 with the function's response Payload (FunctionError set only when the handler itself failed); modeled refusals are the real ones (ResourceNotFoundException(404), InvalidRequestContentException(400), ...) — NotImplemented is not among them. Recorded behavior: `aws lambda invoke --function-name <fn> --payload <json> out.json` exits 0 and out.json holds the handler's JSON.
Emulator: the call is refused with code `NotImplemented` and the message `Invoke resolved arn:aws:lambda:<region>:<account>:function:<name> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce. InvocationType=DryRun validates an invocation without running the function.` (the ARN/name are per-run values); the CLI surfaces it as rc=254. The open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md reports this same refusal but does not carry these three CLI check ids, so they are filed here and should be closed together with it.
Why: no code-driven scenario works — a client that deploys a function and invokes it (the core Lambda contract, and the only way these SESv2 behaviours are reachable) gets an error code AWS never returns, and anything that branches on real error codes mis-handles it.
Fix: the lambda Invoke handler's execution path (an in-process runtime for the packaged handler, or at minimum a faithful 200/Payload contract) in the lambda service.
Traces: lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email, lambda__sesv2.cli-lambda-sesv2-missing-policy-negative, lambda__sesv2.cli-lambda-sesv2-update-role-policy-positive.

Cause 3 of 3 — no SES sending identity can ever be obtained: enumeration reports none verified, and a domain the check verifies for itself (its Easy DKIM CNAMEs published in its own Route 53 hosted zone) never reaches verified, so seven checks either abort immediately or burn their whole 60 s budget in that wait.
AWS: Smithy (com.amazonaws.sesv2): ListEmailIdentities POST /v2/email/list-identities -> IdentityInfo[] with VerificationStatus (PENDING|SUCCESS|FAILED|TEMPORARY_FAILURE|NOT_STARTED) and SendingEnabled; CreateEmailIdentity returns DkimAttributes.Tokens; GetEmailIdentity GET /v2/email/identities/{EmailIdentity} -> {VerifiedForSendingStatus, DkimAttributes{Status,...}}. Smithy (com.amazonaws.route53): CreateHostedZone, ChangeResourceRecordSets, ListHostedZones/ListTagsForResource. Recorded behavior: the account exposes a verified sending identity — either the tag-discoverable DNS-baseline zone's apex (GetEmailIdentity 200 with VerifiedForSendingStatus true, DkimAttributes.Status 'SUCCESS') or a fresh domain whose three Easy DKIM CNAMEs (`<token>._domainkey.<domain>` -> `<token>.dkim.amazonses.com`) were published in its own public hosted zone, which SES then reports verified within the wait.
Emulator: ListEmailIdentities enumerates no identity with VerificationStatus 'SUCCESS' (two checks abort at their first statement with `AssertionError: no verified SES sending identity ... exists in this account`), and the self-verification fallback never converges — GetEmailIdentity keeps reporting VerifiedForSendingStatus false / DkimAttributes.Status not 'SUCCESS', so the five remaining checks are killed by the harness with `Check timed out after 60.0s` while still polling that identity (the stand-in wait is the first statement of each). No call is refused; the divergence is purely in the identity's verification state never advancing. Related open issues 20261029T085623Z-sesv2-two-causes-the-dns-baseline-hosted-zone.md and 20261029T160438Z-sesv2-sns-sqs-the-ses-sending-surface-is.md report the route53 tag-discovery half for other services' check ids; neither carries these lambda__sesv2 checks.
Why: every Lambda+SES data-plane behaviour is unreachable — SendEmail response shape, the account/tenant suppression reads, and the whole SESv2 -> EventBridge -> Lambda event path can never be exercised, and a client that publishes the DKIM records SES asked for is left with a permanently unverified domain.
Fix: the sesv2 email-identity verification state machine (DkimAttributes.Status / VerifiedForSendingStatus transition once the identity's DKIM CNAMEs exist in the account's route53 zone) and the ListEmailIdentities projection of VerificationStatus/SendingEnabled; the route53 hosted-zone/record state it must consult.
Traces: lambda__sesv2.cfn-lambda-sesv2-trust-policy-positive, lambda__sesv2.lambda-sesv2-send-email-response-format (immediate abort); lambda__sesv2.lambda-sesv2-get-suppressed-destination-positive, lambda__sesv2.sesv2-eventbridge-bounce-event-data-format, lambda__sesv2.sesv2-eventbridge-send-event-lambda-invocation-positive, lambda__sesv2.sesv2-eventbridge-rule-missing-resource-policy-negative, lambda__sesv2.sesv2-eventbridge-wrong-event-pattern-negative (60 s timeout in the verification wait).

## Evidence
Cause 1 — expected (AWS, recorded): the four-resource stack (AWS::SES::ConfigurationSet, AWS::SES::EmailIdentity with ConfigurationSetAttributes.ConfigurationSetName={"Ref":"ConfigSet"}, AWS::IAM::Role, AWS::Lambda::Function) reaches CREATE_COMPLETE. Actual (emulator):
[mismatch] lambda__sesv2.cfn-deploy-lambda-sesv2-identity-resource-positive — _deploy_cfn: AssertionError: Stack sim2real-xsvc-stk-94c9b90c9b91 ended in ROLLBACK_COMPLETE (stack name is a per-run generated value). create_stack itself returned a StackId; DescribeStacks then settles on ROLLBACK_COMPLETE.

Cause 2 — expected (AWS): `aws lambda invoke --function-name <fn> --cli-binary-format raw-in-base64-out --payload <json> out.json` exits 0 with the handler's JSON in out.json. Actual (emulator), rc=254 on all three checks:
[mismatch] lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email — aws: [ERROR]: An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved arn:aws:lambda:us-west-2:000000000000:function:sim2real-sesv2-fn-32b1bfd060db at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator ...
[mismatch] lambda__sesv2.cli-lambda-sesv2-missing-policy-negative — identical NotImplemented refusal for ...:function:sim2real-sesv2-fn-broken-6829c43b01d7
[mismatch] lambda__sesv2.cli-lambda-sesv2-update-role-policy-positive — identical NotImplemented refusal for ...:function:sim2real-sesv2-fn-e453f3639cb7
(The ARNs/function names are per-run values; the diverging part is the code `NotImplemented`, which Invoke does not model, instead of a 200 with the function's Payload.)

Cause 3 — expected (AWS): a verified sender is available, either enumerated (VerificationStatus 'SUCCESS') or self-verified (GetEmailIdentity -> VerifiedForSendingStatus true, DkimAttributes.Status 'SUCCESS' after the Easy DKIM CNAMEs are published). Actual (emulator):
[mismatch] lambda__sesv2.cfn-lambda-sesv2-trust-policy-positive — _find_verified_sender: AssertionError: no verified SES sending identity (email address or domain) exists in this account/region
[mismatch] lambda__sesv2.lambda-sesv2-send-email-response-format — _verified_sender: AssertionError: no verified SES sending identity exists in this account; SES requires a verified FROM identity to send email
[mismatch] lambda__sesv2.lambda-sesv2-get-suppressed-destination-positive — Check timed out after 60.0s (first statement is _dns_baseline_domain -> _verified_stand_in, which polls get_email_identity every 10 s)
[mismatch] lambda__sesv2.sesv2-eventbridge-bounce-event-data-format — Check timed out after 60.0s (first statement is _ensure_verified_sender_identity -> _verified_stand_in)
[mismatch] lambda__sesv2.sesv2-eventbridge-send-event-lambda-invocation-positive — Check timed out after 60.0s (same first statement)
[mismatch] lambda__sesv2.sesv2-eventbridge-rule-missing-resource-policy-negative — Check timed out after 60.0s (same first statement)
[mismatch] lambda__sesv2.sesv2-eventbridge-wrong-event-pattern-negative — Check timed out after 60.0s (same first statement)

## Steps to Reproduce
Cause 1:
1. cloudformation.create_stack with a template whose Resources are: AWS::SES::ConfigurationSet{Name}, AWS::SES::EmailIdentity{EmailIdentity, ConfigurationSetAttributes:{ConfigurationSetName:{Ref: ConfigSet}}}, AWS::IAM::Role (lambda.amazonaws.com trust), AWS::Lambda::Function (python3.12, Environment IDENTITY_NAME={Ref: Identity}), Capabilities CAPABILITY_IAM/CAPABILITY_NAMED_IAM.
2. Poll describe_stacks: the stack settles on ROLLBACK_COMPLETE instead of CREATE_COMPLETE; inspect describe_stack_events for the resource that failed.
3. Re-replay lambda__sesv2.cfn-deploy-lambda-sesv2-identity-resource-positive.

Cause 2:
1. lambda.create_function(Runtime='python3.12', Handler='handler.handler', Code={'ZipFile': <zip>}, Role=<role>) and wait for State Active.
2. `aws lambda invoke --function-name <fn> --cli-binary-format raw-in-base64-out --payload '{}' out.json` — rc=254, NotImplemented.
3. Re-replay lambda__sesv2.cli-lambda-sesv2-deploy-and-send-email, lambda__sesv2.cli-lambda-sesv2-missing-policy-negative, lambda__sesv2.cli-lambda-sesv2-update-role-policy-positive.

Cause 3:
1. Page sesv2.list_email_identities(): no entry has VerificationStatus 'SUCCESS'.
2. route53.create_hosted_zone(Name='<rand>.example.net'); sesv2.create_email_identity(EmailIdentity='<rand>.example.net'); UPSERT each returned DKIM token as a CNAME '<token>._domainkey.<domain>' -> '<token>.dkim.amazonses.com' in that zone.
3. Poll sesv2.get_email_identity(EmailIdentity='<rand>.example.net'): VerifiedForSendingStatus stays false and DkimAttributes.Status never becomes 'SUCCESS'.
4. Re-replay the seven checks listed in trace_ids for this cause.
