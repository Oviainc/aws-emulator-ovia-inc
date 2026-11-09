Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-dataset-not-found-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-dataset-not-found-errors bedrock-agentcore-control-dataset-s3-source-not-found
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|GetDataset|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|CreateDataset|4xx->2xx|$.message
Issue-Key: bedrock-agentcore-control|CreateDataset|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|CreateDataset|4xx->2xx|status
Issue-Commit: 15d5f672973fe406b6b38447a2fe490e83bf1181

# bedrock-agentcore-control Dataset surface diverges: GetDataset not-found message wording and CreateDataset skips S3 source validation

> _Note: this issue was identified against the code base at commit `15d5f672973fe406b6b38447a2fe490e83bf1181`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataset

## Description
Two net-new root causes in the bedrock-agentcore-control Dataset resource. Both are about the emulator's Dataset handlers not reproducing AWS's dataset contract: one on the read path's error wording, one on the create path's source validation. A fixer must close BOTH causes before submitting.

Cause 1 of 2 - GetDataset ResourceNotFoundException carries emulator-invented wording (fixture bedrock-agentcore-control-dataset-not-found-errors, step task#1).
AWS: Smithy: GetDataset (GET /datasets/{datasetId}) models ResourceNotFoundException with httpError 404. The recorded response for a datasetId that names no dataset is 404 with message 'The specified dataset does not exist. Please verify the datasetId and try again.' That text is entirely fixed for this error class - it embeds no request or state value at all (the datasetId is NOT interpolated into it).
Emulator: answers 404 (status is correct) but with the generic message "Failed to retrieve dataset because it doesn't exist. Retry the request with a different resource identifier." - a house-style not-found sentence shared across this service's resources rather than the dataset-specific text AWS returns.
Why: a client that branches on, logs, or asserts the not-found reason for a dataset (and any test or runbook matching AWS's sentence) reads an unrecognised message; the operator-facing guidance to verify the datasetId is lost, and tooling that keys off the message text behaves differently against the emulator than against AWS.
Fix: the GetDataset handler's not-found path in the bedrock-agentcore-control dataset handler/error construction - it must emit the dataset-specific ResourceNotFoundException message instead of falling through to the service-wide generic retrieve-failure template.
Traces: bedrock-agentcore-control-dataset-not-found-errors#task#1.

Cause 2 of 2 - CreateDataset accepts a dataset whose S3 source does not exist, instead of refusing it with 400 ValidationException (fixture bedrock-agentcore-control-dataset-s3-source-not-found, step task#1).
AWS: Smithy: CreateDataset (POST /datasets, success code 202) takes a required `source` of DataSourceType and models ValidationException with httpError 400. The recording shows AWS resolving the referenced S3 object before creating anything: when the S3 URI in `source` does not exist or is not readable by the caller, the call fails 400 with x-amzn-errortype ValidationException and message 'The specified S3 URI does not exist or is not accessible: <s3Uri>'. Fixed text: 'The specified S3 URI does not exist or is not accessible: '; variable part: the s3 URI taken verbatim from the request's source member (in the recording, the probe bucket/key - that value is an example, not part of the requirement).
Emulator: performs no source resolution at all and answers 202 with a created dataset; no message and no x-amzn-errortype header are emitted.
Why: the emulator reports success for a dataset that can never be populated, so a client gets a dataset id and then sees it silently never become usable, instead of the immediate, actionable validation failure AWS returns. Error-handling paths around dataset creation (retry with a corrected URI, surface the bad URI to the user) are never exercised, and a caller cannot distinguish a good source from a typo'd or inaccessible one.
Fix: CreateDataset's input validation / source-resolution step in the bedrock-agentcore-control dataset handler - resolve the `source` S3 URI against emulated S3 state (existence and readability) before creating the dataset, and raise ValidationException (400) naming the URI when it does not resolve.
Traces: bedrock-agentcore-control-dataset-s3-source-not-found#task#1.

Neither cause is covered by an open issue: the replay report attributes no owner to either cluster, and the existing bedrock-agentcore-control issues cover browsers, code interpreters, gateways, harnesses, credential providers and the policy/policy-engine surface, not the Dataset resource.

## Evidence
Cause 1 - GetDataset, expected 404 vs actual 404, fixture bedrock-agentcore-control-dataset-not-found-errors step task#1:
  $.message: value_mismatch
    expected: 'The specified dataset does not exist. Please verify the datasetId and try again.'
    actual:   "Failed to retrieve dataset because it doesn't exist. Retry the request with a different resource identifier."

Cause 2 - CreateDataset, expected 400 vs actual 202, fixture bedrock-agentcore-control-dataset-s3-source-not-found step task#1:
  status: expected 400, got 202
  $.message: missing
    expected: 'The specified S3 URI does not exist or is not accessible: s3://<bucket>/<key>' (recorded: s3://sim2real-probe-bedrock-agentcore-control-evalbucket-ccb279bb/examples/dataset.jsonl)
    actual:   None (success body returned instead)
  $headers.x-amzn-errortype: missing
    expected: 'ValidationException'
    actual:   None

Model grounding (smithy_query, services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json):
  GetDataset: GET /datasets/{datasetId}, code 200; errors include ResourceNotFoundException (404), ValidationException (400).
  CreateDataset: POST /datasets, code 202; required members datasetName, source (DataSourceType), schemaType; errors include ValidationException (400), ConflictException (409).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture bedrock-agentcore-control-dataset-not-found-errors, step task#1 - GET /datasets/<nonexistent-dataset-id>. The response is 404 ResourceNotFoundException but the $.message text is the generic "Failed to retrieve dataset because it doesn't exist. Retry the request with a different resource identifier." instead of AWS's 'The specified dataset does not exist. Please verify the datasetId and try again.'
3. Cause 2: replay fixture bedrock-agentcore-control-dataset-s3-source-not-found, step task#1 - POST /datasets with a `source` whose S3 URI does not exist in emulated S3. The emulator answers 202 with a created dataset; AWS answers 400 with x-amzn-errortype ValidationException and message 'The specified S3 URI does not exist or is not accessible: <s3Uri>'.
4. Re-replay both fixture steps after the fix to confirm both causes are closed.
