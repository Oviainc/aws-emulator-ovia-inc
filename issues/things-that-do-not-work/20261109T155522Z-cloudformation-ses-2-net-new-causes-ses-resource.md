Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ses.deploy-ses-receipt-rule-nonexistent-ruleset-rejected cloudformation__ses.deploy-ses-receipt-ruleset-rule-and-filter-functional
Issue-Key: cloudformation__ses.deploy-ses-receipt-rule-nonexistent-ruleset-rejected|run#11|AssertionError|assert got == rejected, f"expected masked reason {rejected!r}, got {got!r}"
Issue-Key: cloudformation__ses|CreateStack|ValidationError
Issue-Commit: 5b2a9eedb8fa287cac2318fd09ae07f52de03ab6

# cloudformation__ses: 2 net-new causes — SES resource-handler failure reasons omit the AWS SDK exception suffix, and AWS::SES::ReceiptFilter is an unrecognized resource type

> _Note: this issue was identified against the code base at commit `5b2a9eedb8fa287cac2318fd09ae07f52de03ab6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes in the CloudFormation handling of AWS::SES::* resources, grounded in the two failing checks of the cloudformation__ses group. Neither check is marked "ALREADY FILED" nor "OWNED by" in this replay, so both are filed here. Related open issues describe the same SHAPE of defect in other services' CFN resource registries and handler reasons but carry different replay keys and a different service, so they do not cover these: 20261109T141609Z-cloudformation-ecr-8-net-new-causes-two-aws.md (ECR types unrecognized + handler reasons ECR-API-worded) and 20261109T134927Z-cloudformation-sagemaker-stacks-createstack-refuses-15-aws-sagemaker.md.

Cause 1 of 2: a failed AWS::SES::ReceiptRule resource reports a ResourceStatusReason whose handler message is only the bare SES error sentence — the AWS SDK exception suffix that CloudFormation always carries is missing.
Failing check id: cloudformation__ses.deploy-ses-receipt-rule-nonexistent-ruleset-rejected.
AWS: the stack-event envelope is recorded behavior only (CloudFormation's ResourceStatusReason is not modelled); the underlying refusal IS modelled. Smithy (com.amazonaws.ses): CreateReceiptRule — input {RuleSetName @required (ReceiptRuleSetName), After, Rule @required (ReceiptRule)}, empty output; modeled error RuleSetDoesNotExistException, errorKind client, httpError 400, awsQueryError code RuleSetDoesNotExist ("Indicates that the provided receipt rule set does not exist."). Recorded behavior: when the AWS::SES::ReceiptRule handler calls CreateReceiptRule for a RuleSetName that does not exist, the stack rolls back (ROLLBACK_COMPLETE) and the resource's DELETE/CREATE_FAILED event reason is the fixed CloudFormation envelope `Resource handler returned message: "<downstream SDK exception toString>" (RequestToken: <uuid>, HandlerErrorCode: NotFound)`, where the quoted downstream text is the SDK exception's full toString: the SES sentence `Rule set does not exist: <RuleSetName>` followed by ` (Service: Ses, Status Code: 400, Request ID: <request id>) (SDK Attempt Count: <attempt count>)`. Fixed text of that suffix: the literal ` (Service: Ses, Status Code: 400, Request ID: ` ... `) (SDK Attempt Count: ` ... `)`, including the service label `Ses` and the status 400 that the modeled httpError gives. Variable parts: <RuleSetName> comes from the template/request (here the RuleSetNameParam value), the request id is the per-call id and the attempt count the per-call retry count — the check masks exactly those two to <id> and <n>, so only the fixed frame and the service/status are requirements.
Emulator: the stack does roll back and the HandlerErrorCode is already the correct `NotFound`, and the SES sentence `Rule set does not exist: <RuleSetName>` is already correct — but the reason stops there. The check's masked comparison yields got = ('Rule set does not exist: sim2real-cfn-ses-missingrs-<hex>', 'NotFound') against the expected ('Rule set does not exist: sim2real-cfn-ses-missingrs-<hex> (Service: Ses, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'NotFound'). No status or error code diverges on the CloudFormation API itself; the divergence is the truncated handler message.
Why: the SDK suffix is how a client (and every CFN troubleshooting runbook, log scraper and CI gate) learns WHICH service refused and with WHAT HTTP status a rollback was caused by. A reason without it cannot be attributed to SES or to a 400-class refusal, so automation that parses `(Service: X, Status Code: N, ...)` out of ResourceStatusReason — the documented shape of every CFN handler failure — finds nothing and misclassifies a deterministic client error as an unknown provisioning failure.
Fix: the CloudFormation resource-handler error path that renders a downstream service refusal into ResourceStatusReason (the AWS::SES::ReceiptRule handler's error mapping in the cloudformation crate, services/service-com-amazonaws-cloudformation-cloudformation): the quoted message must be the downstream SDK exception's full toString — SES's sentence plus the ` (Service: <sdkId>, Status Code: <httpError>, Request ID: <id>) (SDK Attempt Count: <n>)` frame — not just the service's message string.
Traces: cloudformation__ses.deploy-ses-receipt-rule-nonexistent-ruleset-rejected.

Cause 2 of 2: AWS::SES::ReceiptFilter is not a registered CloudFormation resource type, so CreateStack refuses at template-validation time any template that declares one and no SES receipt filter can be provisioned through CloudFormation.
Failing check id: cloudformation__ses.deploy-ses-receipt-ruleset-rule-and-filter-functional.
AWS: the CFN resource-type registry is recorded behavior only (resource types are not Smithy shapes); the provisioned SES surface is modelled. Smithy (com.amazonaws.ses): CreateReceiptFilter/DeleteReceiptFilter/ListReceiptFilters exist and model the IpFilter (Policy Allow|Block, Cidr) a filter carries, so the receipt-filter resource the template asks for is a real, supported SES resource. Recorded behavior: AWS::SES::ReceiptFilter is a recognized AWS::SES::* resource type — CreateStack ACCEPTS a template declaring it alongside AWS::SES::ReceiptRuleSet and AWS::SES::ReceiptRule, the stack reaches CREATE_COMPLETE, and the filter is then visible through ses ListReceiptFilters; deleting the stack removes it.
Emulator: CreateStack is refused before any provisioning with HTTP 400, error code `ValidationError`, message `Template format error: Unrecognized resource types: [AWS::SES::ReceiptFilter]`. Fixed text of that emulator message is CloudFormation's own "Template format error: Unrecognized resource types: [...]" frame and the bracketed list is the set of types the template declared that the registry does not know — AWS emits that message only for types it genuinely does not support, which AWS::SES::ReceiptFilter is not. Because the refusal happens at CreateStack, the whole round-trip the check exercises (create rule set + rule + filter, read them back through SES, then delete the stack) is unreachable.
Why: a client deploying SES email-receiving infrastructure as code cannot express the IP allow/block filter at all — the stack is rejected outright, not partially provisioned — so any template that mixes receipt rule sets with receipt filters (the documented way to build an inbound mail pipeline) fails on this emulator while succeeding on AWS, and the whole stack's other SES resources are never created either.
Fix: the CloudFormation resource-type registry and its SES resource handlers (the cloudformation crate, services/service-com-amazonaws-cloudformation-cloudformation): register AWS::SES::ReceiptFilter with a handler whose create/delete call through to the ses crate's CreateReceiptFilter/DeleteReceiptFilter and whose Ref returns the filter name, so a template declaring it validates and provisions.
Traces: cloudformation__ses.deploy-ses-receipt-ruleset-rule-and-filter-functional.

## Evidence
Replayed behavioral suite simparity-behavioral-aws-v1: 95/132 passed (72.0%); per-service line `cloudformation__ses  0/2 (0%) 2 failed`. Both checks are in the failing list and NEITHER is marked "ALREADY FILED" or "OWNED by".

Cause 1 — [mismatch] cloudformation__ses.deploy-ses-receipt-rule-nonexistent-ruleset-rejected (verbatim tail):
  File "check_cloudformation__ses_deploy_ses_receipt_rule_nonexistent_ruleset_rejected.py", line 223, in run
    assert got == rejected, f"expected masked reason {rejected!r}, got {got!r}"
  AssertionError: expected masked reason ('Rule set does not exist: sim2real-cfn-ses-missingrs-bff9bcf43d98 (Service: Ses, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'NotFound'), got ('Rule set does not exist: sim2real-cfn-ses-missingrs-bff9bcf43d98', 'NotFound')
Expected (AWS): the "Rule" resource's failed stack event carries ResourceStatusReason matching `Resource handler returned message: "Rule set does not exist: <RuleSetName> (Service: Ses, Status Code: 400, Request ID: <uuid>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: NotFound)`.
Actual (emulator): the same envelope but the quoted message is only `Rule set does not exist: <RuleSetName>` — the ` (Service: Ses, Status Code: 400, Request ID: ...) (SDK Attempt Count: ...)` suffix is absent. The stack status (ROLLBACK_COMPLETE) and HandlerErrorCode (NotFound) both already match. The `sim2real-cfn-ses-missingrs-bff9bcf43d98` token is the per-run minted rule-set name from the request, not part of the requirement; the request id and attempt count are per-call values the check masks to <id>/<n>.

Cause 2 — [error] cloudformation__ses.deploy-ses-receipt-ruleset-rule-and-filter-functional (verbatim tail):
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::SES::ReceiptFilter]
Expected (AWS): CreateStack returns 200 with a StackId for a template declaring AWS::SES::ReceiptRuleSet + AWS::SES::ReceiptRule + AWS::SES::ReceiptFilter; the stack reaches CREATE_COMPLETE and the filter is readable through ses ListReceiptFilters.
Actual (emulator): CreateStack answers HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::SES::ReceiptFilter]"; nothing is provisioned, so the rule-set/rule/filter round-trip and the stack delete are never exercised.

## Steps to Reproduce
Cause 1 (handler reason suffix):
1. Build and boot the emulator; point cloudformation and ses clients at it.
2. cloudformation CreateStack with a template whose ONLY resource is `Rule` of Type AWS::SES::ReceiptRule, Properties.RuleSetName = <a rule-set name that was never created>, Properties.Rule = {Name: <rule>, Enabled: false, ScanEnabled: false, Actions: []}.
3. Poll DescribeStacks(StackName=<StackId>) until terminal. It reaches ROLLBACK_COMPLETE (matches AWS).
4. DescribeStackEvents(StackName=<StackId>) and read the failed "Rule" event's ResourceStatusReason.
   Expected: `Resource handler returned message: "Rule set does not exist: <rule-set name> (Service: Ses, Status Code: 400, Request ID: <uuid>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: NotFound)`.
   Observed: the quoted message is only `Rule set does not exist: <rule-set name>` — no `(Service: Ses, Status Code: 400, Request ID: ...) (SDK Attempt Count: ...)` suffix.
5. Or re-replay cloudformation__ses.deploy-ses-receipt-rule-nonexistent-ruleset-rejected.

Cause 2 (unrecognized resource type):
1. cloudformation CreateStack with a template declaring Resources: a RuleSet of Type AWS::SES::ReceiptRuleSet, a Rule of Type AWS::SES::ReceiptRule (DependsOn the rule set), and a Filter of Type AWS::SES::ReceiptFilter with Properties.Filter = {Name: <filter>, IpFilter: {Policy: Block, Cidr: <cidr>}}.
   Expected: 200 with a StackId; the stack reaches CREATE_COMPLETE and ses ListReceiptFilters lists <filter>.
   Observed: HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::SES::ReceiptFilter]"; no stack is created.
2. Or re-replay cloudformation__ses.deploy-ses-receipt-ruleset-rule-and-filter-functional.
