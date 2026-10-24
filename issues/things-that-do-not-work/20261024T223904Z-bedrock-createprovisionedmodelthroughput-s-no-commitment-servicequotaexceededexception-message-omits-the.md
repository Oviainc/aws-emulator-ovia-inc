Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.concurrent-delete-provisioned-throughput-race-one-wins
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.concurrent-delete-provisioned-throughput-race-one-wins|run#10|AssertionError|assert message == expected_message, (
Issue-Commit: 27d60cb41c89860474fb4eac178b022e94c750b0

# CreateProvisionedModelThroughput's no-commitment ServiceQuotaExceededException message omits the documented service-limit-increase support URL

> _Note: this issue was identified against the code base at commit `27d60cb41c89860474fb4eac178b022e94c750b0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProvisionedModelThroughput

## Description
Cause 1 of 1: the quota refusal CreateProvisionedModelThroughput raises when the account has no granted no-commitment model units carries the wrong support-center link and the wrong closing clause.

AWS: CreateProvisionedModelThroughput refuses a purchase the account has no granted no-commitment model-unit quota for with 400 ServiceQuotaExceededException. Smithy: CreateProvisionedModelThroughputRequest.modelUnits (PositiveInteger, range 1.., required) documents the remedy explicitly — "By default, your account has no model units for purchasing Provisioned Throughputs with commitment. You must first visit the <a href=\"https://console.aws.amazon.com/support/home#/case/create?issueType=service-limit-increase\">Amazon Web Services support center</a> to request MUs." Smithy: ServiceQuotaExceededException (HTTP 400) — "The number of requests exceeds the service quota. Resubmit your request later." The recorded refusal message is built from the request and the model doc: it names the requested base model and the request's region, and directs the caller at the support-center case form in the issueType=service-limit-increase form the model documents, closing with "to request a service quota increase." Recorded example: "Your account does not currently have any no commitment model units reserved for amazon.nova-lite-v1:0. Please see https://support.console.aws.amazon.com/support/home?region=us-west-2#/case/create?issueType=service-limit-increase to request a service quota increase."

Emulator: the HTTP status and the error code are CORRECT (400 ServiceQuotaExceededException), and the request-derived parts of the message are correct too — the base model id and the region are interpolated as recorded. What diverges is the remedy clause: the emulator emits the support URL WITHOUT the documented `?issueType=service-limit-increase` selector and closes with "to request model units." instead of "to request a service quota increase." Observed: "... Please see https://support.console.aws.amazon.com/support/home?region=us-west-2#/case/create to request model units."

Why: the message's remedy link is the only actionable content in this refusal — a Provisioned Throughput purchase cannot be retried into success, it can only be escalated. A caller or operator following the emulator-shaped link lands on the generic case-creation form rather than the service-limit-increase form the model's own modelUnits documentation points at, so the escalation path the refusal is supposed to hand over is lost. Tooling or runbooks that match on the documented issueType selector (or on the quota-increase wording) to classify a refusal as "needs a quota escalation" will not match.

Fix: the shared message builder behind this refusal — `crate::state::no_commitment_model_units_message`, called from the no-commitment branch of the CreateProvisionedModelThroughput handler. The remedy clause belongs in that builder; the status and code paths around it are already right and need no change.

Traces: bedrock.concurrent-delete-provisioned-throughput-race-one-wins. The check takes ListProvisionedModelThroughputs as a positive control (passes), asserts the refusal code is ServiceQuotaExceededException (passes), then asserts the refusal message exactly (fails only on the remedy clause), then asserts no Provisioned Throughput leaked from the refused call. Handler: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/create_provisioned_model_throughput.rs lines 292-311 (the `granted_model_units` branch); builder in services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs.

Note: the open issue 20261024T175106Z-bedrock-createprovisionedmodelthroughput-stub.md ('Implement bedrock CreateProvisionedModelThroughput: unimplemented scaffold stub') does NOT cover this — that operation is now fully implemented and answered this replay with a real modeled refusal rather than NotImplemented; that issue reads as stale against this checkout.

## Evidence
Check bedrock.concurrent-delete-provisioned-throughput-race-one-wins, mismatch.

Request: CreateProvisionedModelThroughput(modelUnits=1, provisionedModelName=<unique>, modelId=arn:aws:bedrock:us-west-2::foundation-model/amazon.nova-lite-v1:0, tags=[{key:sim2real,...}]) against an account with no granted no-commitment model units.

Status: 400 (matches AWS)
Error code: ServiceQuotaExceededException (matches AWS)

Expected message (recorded AWS, with the model-doc'd support URL form):
  'Your account does not currently have any no commitment model units reserved for amazon.nova-lite-v1:0. Please see https://support.console.aws.amazon.com/support/home?region=us-west-2#/case/create?issueType=service-limit-increase to request a service quota increase.'

Actual message (emulator):
  'Your account does not currently have any no commitment model units reserved for amazon.nova-lite-v1:0. Please see https://support.console.aws.amazon.com/support/home?region=us-west-2#/case/create to request model units.'

Diff is confined to the remedy clause: the '?issueType=service-limit-increase' selector is dropped from the case-create URL, and the trailing clause reads 'to request model units.' instead of 'to request a service quota increase.' The model-name and region interpolation is correct.

AssertionError raised at check_bedrock_concurrent_delete_provisioned_throughput_race_one_wins.py line 47:
  'expected the quota refusal to name model units for 'amazon.nova-lite-v1:0' exactly: expected ... got ...'

Smithy grounding for the URL form: smithy_query(query='doc', shape='CreateProvisionedModelThroughputRequest', member='modelUnits', model_path='services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json') returns a doc string containing 'https://console.aws.amazon.com/support/home#/case/create?issueType=service-limit-increase'.

## Steps to Reproduce
1. Boot the emulator and point a bedrock (control-plane) client at it in a region, e.g. us-west-2.
2. Confirm the positive control answers: bedrock.list_provisioned_model_throughputs() -> response contains 'provisionedModelSummaries'.
3. Pick an ACTIVE foundation model offered with the PROVISIONED inference type:
   bedrock.list_foundation_models(byInferenceType='PROVISIONED') and take modelArn of the first summary whose modelLifecycle.status == 'ACTIVE' (e.g. arn:aws:bedrock:us-west-2::foundation-model/amazon.nova-lite-v1:0).
4. Purchase with NO commitmentDuration, so the no-commitment granted quota (zero) applies:
   bedrock.create_provisioned_model_throughput(modelUnits=1, provisionedModelName='sim2real-pt-xxxx', modelId=<that arn>)
5. Catch the ClientError and inspect response['Error']:
   - Code is 'ServiceQuotaExceededException' and the status is 400 — both correct.
   - Message ends with '#/case/create to request model units.'
   AWS instead ends it with '#/case/create?issueType=service-limit-increase to request a service quota increase.'
6. Re-replay bedrock.concurrent-delete-provisioned-throughput-race-one-wins to confirm the fix; the other legs of that check (positive control, error code, no-leaked-resource) already pass.
