Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-api-key-usage-plan-workflow
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-api-key-usage-plan-workflow apigateway-usage-plan-key-association-and-usage-tracking apigateway-tag-usageplan-bulk-tags-and-pagination apigateway-tag-vpc-link-opportunistic apigateway-import-api-keys-csv apigateway-import-api-keys-csv-bulk-and-failonwarnings-validation apigateway-import-api-keys-workflow
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|CreateUsagePlan|4xx->4xx|$.message
Issue-Key: apigateway|DeleteUsagePlan|4xx->2xx|$
Issue-Key: apigateway|DeleteUsagePlan|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|DeleteUsagePlan|4xx->2xx|status
Issue-Key: apigateway|UpdateUsagePlan|2xx->4xx|$.apiStages
Issue-Key: apigateway|UpdateUsagePlan|2xx->4xx|$.id
Issue-Key: apigateway|UpdateUsagePlan|2xx->4xx|$.name
Issue-Key: apigateway|UpdateUsagePlan|2xx->4xx|$.quota
Issue-Key: apigateway|UpdateUsagePlan|2xx->4xx|$.tags
Issue-Key: apigateway|UpdateUsagePlan|2xx->4xx|$.throttle
Issue-Key: apigateway|UpdateUsagePlan|2xx->4xx|status
Issue-Key: apigateway|DeleteUsagePlan|2xx->4xx|$
Issue-Key: apigateway|DeleteUsagePlan|2xx->4xx|status
Issue-Key: apigateway|GetUsagePlanKey|4xx->2xx|$.message
Issue-Key: apigateway|GetUsagePlanKey|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|GetUsagePlanKey|4xx->2xx|status
Issue-Key: apigateway|GetUsagePlanKeys|2xx->2xx|$.item[*]
Issue-Key: apigateway|CreateUsagePlanKey|4xx->4xx|$.message
Issue-Key: apigateway|TagResource|4xx->4xx|$.message
Issue-Key: apigateway|GetApiKey|2xx->2xx|$.enabled
Issue-Key: apigateway|UpdateApiKey|2xx->2xx|$.enabled
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# API Gateway usage plans and API keys: deletes, listings and refusals (10 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUsagePlan

## Description
Usage-plan and API-key state is not maintained: an in-use usage plan can be deleted, deleted plans and keys keep answering, and the refusal messages are emulator-authored. Smithy: ApiKey.enabled is a Boolean; DeleteUsagePlan/DeleteUsagePlanKey succeed with 202 and model BadRequestException 400 / NotFoundException 404; GetUsagePlanKey models NotFoundException 404.

Cause 1 of 10 - CreateUsagePlan with an apiStages entry naming a stage that does not exist. AWS: 404 NotFoundException `API Stage not found: <apiId>:<stageName>` (fixed prefix, the apiId:stageName pair taken from the request). Emulator: `Invalid stage identifier specified`. Fix: CreateUsagePlan's apiStages validation message. Trace: apigateway-api-key-usage-plan-workflow#task#11.

Cause 2 of 10 - DeleteUsagePlan while API stages are still associated. AWS: 400 BadRequestException `Cannot delete Usage Plan <usagePlanId> because there are API Stages associated with it, including <apiId>:<stageName>` (fixed text; the plan id and one offending apiId:stageName come from state). Emulator: 202 - it deletes the plan. Why: a client's protection against removing a plan that still gates traffic is gone, and the emulator's state diverges irrecoverably. Fix: DeleteUsagePlan's pre-delete check. Trace: apigateway-api-key-usage-plan-workflow#task#27.

Cause 3 of 10 - consequence in the same fixture: UpdateUsagePlan on that plan answers 404 instead of 200 with the plan's representation. Closes with cause 2. Trace: apigateway-api-key-usage-plan-workflow#task#28.

Cause 4 of 10 - consequence: the later DeleteUsagePlan answers 404 instead of 202. Closes with cause 2. Trace: apigateway-api-key-usage-plan-workflow#task#29.

Cause 5 of 10 - GetUsagePlanKey for a plan/key that no longer exists. AWS: 404 NotFoundException `Invalid Usage Plan ID specified`. Emulator: 200 with a key representation. Why: a client sees a usage-plan key that was deleted. Fix: GetUsagePlanKey's existence check. Trace: apigateway-api-key-usage-plan-workflow#task#24 (2 steps).

Cause 6 of 10 - GetUsagePlanKeys still lists a key after DeleteUsagePlanKey. AWS returns the collection without it. Fix: the usage-plan-key state on delete. Trace: apigateway-usage-plan-key-association-and-usage-tracking#task#12.

Cause 7 of 10 - CreateUsagePlanKey with a keyType other than API_KEY. AWS: 400 BadRequestException `Invalid Usage Plan key type specified. Must be one of [API_KEY]` (fixed text). Emulator: `Invalid key type specified: NOT_API_KEY, expected API_KEY`. Fix: CreateUsagePlanKey's validation message. Trace: apigateway-usage-plan-key-association-and-usage-tracking#task#3.

Cause 8 of 10 - TagResource against an absent usage-plan ARN. AWS: 404 NotFoundException `UsagePlan <usagePlanId> was not found` (fixed text around the id from the request ARN). Emulator: `Invalid Usage Plan ID specified`. Fix: the tagging layer's usage-plan not-found message. Trace: apigateway-tag-usageplan-bulk-tags-and-pagination#task#8 (2 steps).

Cause 9 of 10 - ImportApiKeys ignores the CSV `enabled` column. AWS creates the key with enabled=true when the CSV says so; the emulator stores false, so GetApiKey reports enabled=false. Why: an imported key is unusable until a client patches it. Fix: ImportApiKeys' CSV column mapping. Trace: apigateway-import-api-keys-csv#task#2 (5 steps).

Cause 10 of 10 - the same defect surfaced through UpdateApiKey's response (enabled=false where AWS reports true). Closes with cause 9. Trace: apigateway-import-api-keys-csv-bulk-and-failonwarnings-validation#task#5.

Ids and names in the evidence are that recording's instances of the message templates, not requirements. No open issue owns these clusters; the open 'API Gateway: a usage plan's DAY quota is never enforced...' issue is about data-plane metering, not this control-plane state.

## Evidence
Cause 1: 404 both; $.message expected 'API Stage not found: 8sh3tqaaaa:no-such-stage', got 'Invalid stage identifier specified'.
Cause 2: status expected 400 with x-amzn-errortype BadRequestException and the quoted message, got 202 with an empty body.
Cause 3: status expected 200 (apiStages, id, name, quota, tags, throttle), got 404.
Cause 4: status expected 202, got 404 with NotFoundException 'Invalid Usage Plan ID specified'.
Cause 5: status expected 404 with 'Invalid Usage Plan ID specified', got 200.
Cause 6: $.item[0] extra - a deleted usage-plan key is still listed.
Cause 7: $.message expected 'Invalid Usage Plan key type specified. Must be one of [API_KEY]', got 'Invalid key type specified: NOT_API_KEY, expected API_KEY'.
Cause 8: $.message expected 'UsagePlan nonexistent12345 was not found', got 'Invalid Usage Plan ID specified'.
Causes 9/10: $.enabled expected True, got False.

## Steps to Reproduce
1. Replay the corpus (replay_wire_corpus).
2. Cause 2-4: create a usage plan with an apiStages entry, then DELETE /usageplans/{id} and observe 202 instead of the BadRequestException (apigateway-api-key-usage-plan-workflow#task#27).
3. Cause 5/6: delete a usage-plan key, then GET /usageplans/{id}/keys/{keyId} and GET /usageplans/{id}/keys.
4. Causes 9/10: POST /apikeys?mode=import with a CSV whose enabled column is TRUE, then GET /apikeys/{id}.
