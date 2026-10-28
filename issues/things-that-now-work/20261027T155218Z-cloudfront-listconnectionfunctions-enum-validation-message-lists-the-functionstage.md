Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.list-connection-functions-invalid-stage-malformed-xml-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.list-connection-functions-invalid-stage-malformed-xml-rejected|run#5|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Commit: 48b00a2ec8a92094223b124c2e7d5eacc8c9104d

# cloudfront: ListConnectionFunctions enum-validation message lists the FunctionStage value set in the wrong order ([DEVELOPMENT, LIVE] instead of AWS's [LIVE, DEVELOPMENT])

> _Note: this issue was identified against the code base at commit `48b00a2ec8a92094223b124c2e7d5eacc8c9104d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListConnectionFunctions

## Description
One net-new root cause.

Cause 1 of 1: the enum-constraint validation message for ListConnectionFunctions' `Stage` parameter renders the permitted value set in the emulator's own (alphabetical) order rather than the order AWS emits. Failing check: cloudfront.list-connection-functions-invalid-stage-malformed-xml-rejected.

AWS: ListConnectionFunctions takes `Stage`, targeting the `FunctionStage` enum (Smithy: com.amazonaws.cloudfront#FunctionStage, values DEVELOPMENT, LIVE; ListConnectionFunctions input member Stage -> FunctionStage; http POST /2020-05-31/connection-functions). When `Stage` carries a value outside that set, AWS refuses the call with HTTP 400 and error code `MalformedXML`, and the message is the standard AWS parameter-validation template:

  "1 validation error detected: Value '<supplied value>' at '<member name>' failed to satisfy constraint: Member must satisfy enum value set: [LIVE, DEVELOPMENT]"

Fixed text of that template: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]". Variable parts taken from the request: the supplied value (here 'NOT_A_VALID_STAGE') and the member name as serialized on the wire ('stage', lowercase — the query-string binding, not the Smithy member name `Stage`). The bracketed value list is NOT a per-run value: it is a property of the enum and is part of the requirement. Recorded behavior only for the ORDER of that list — the Smithy model declares FunctionStage as [DEVELOPMENT, LIVE], but the recorded AWS response renders it [LIVE, DEVELOPMENT], so the model's declaration order is not what AWS serializes and the recording is the authority. Hypothesis: AWS renders the service's internal enum ordinal order, which for FunctionStage puts LIVE first.

Emulator: the status (400) and the error code (`MalformedXML`) are correct, and the rest of the message is byte-identical, but the value set is rendered as `[DEVELOPMENT, LIVE]`. Replay detail:

  AssertionError: expected "1 validation error detected: Value 'NOT_A_VALID_STAGE' at 'stage' failed to satisfy constraint: Member must satisfy enum value set: [LIVE, DEVELOPMENT]", got "1 validation error detected: Value 'NOT_A_VALID_STAGE' at 'stage' failed to satisfy constraint: Member must satisfy enum value set: [DEVELOPMENT, LIVE]"

The positive control in the same check (ListDistributions on the same client) is unaffected, so this is specific to the enum-validation message rendering and not a broken client or transport.

Why: a client that matches on, logs, or asserts against AWS's validation message text sees a message AWS never produces. Error-message assertions in customer test suites, golden-file comparisons and operator runbooks that quote the AWS wording all diverge, and any tooling that parses the bracketed value list positionally (e.g. taking the first entry as the service's default/primary stage) reads LIVE on AWS and DEVELOPMENT against the emulator. Because the status and code are right, the divergence is invisible until the text is inspected, which makes it a silent-fidelity defect rather than a hard failure.

Note: an existing open issue (20261025T052746Z-xray-2-net-new-behavioral-root-causes-putencryptionconfig.md) reports the same class of defect — enum value set rendered in the wrong order in a validation message — for X-Ray PutEncryptionConfig. That issue is a different service and does not cover CloudFront; if the enum-constraint message is produced by shared validation plumbing, the two may share a fix, but this one must be closed on CloudFront's own replay.

Fix: belongs in the request-validation / error-message rendering component for CloudFront's ListConnectionFunctions input (the operation's input validation for the `Stage` member, and whatever shared enum-constraint message formatter it calls). The formatter must emit the enum members in AWS's serialization order for that enum rather than sorting or using the model's declaration order; the fix must not change the surrounding fixed text, the 400 status, or the `MalformedXML` code, all of which already match.

Traces: cloudfront.list-connection-functions-invalid-stage-malformed-xml-rejected (mismatch).

## Evidence
Check: cloudfront.list-connection-functions-invalid-stage-malformed-xml-rejected [mismatch]

Request: cloudfront.list_connection_functions(Stage="NOT_A_VALID_STAGE")

Expected (AWS): HTTP 400, Error.Code = "MalformedXML", Error.Message =
  1 validation error detected: Value 'NOT_A_VALID_STAGE' at 'stage' failed to satisfy constraint: Member must satisfy enum value set: [LIVE, DEVELOPMENT]

Actual (emulator): HTTP 400, Error.Code = "MalformedXML" (both correct), Error.Message =
  1 validation error detected: Value 'NOT_A_VALID_STAGE' at 'stage' failed to satisfy constraint: Member must satisfy enum value set: [DEVELOPMENT, LIVE]

Replay assertion:
  File "check_cloudfront_list_connection_functions_invalid_stage_malformed_xml_rejected.py", line 25, in run
    assert message == expected, f"expected {expected!r}, got {message!r}"
  AssertionError: expected "...enum value set: [LIVE, DEVELOPMENT]", got "...enum value set: [DEVELOPMENT, LIVE]"

Delta: only the ordering of the enum value list inside the message. Fixed template text, status and error code all match.

Model reference: smithy_query(model_path="services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json", query="shape", shape="FunctionStage") -> {"type": "enum", "values": ["DEVELOPMENT", "LIVE"]}; ListConnectionFunctions input member Stage targets FunctionStage. The model's declaration order is NOT the order AWS renders in the message, so the recorded response is the authority for the ordering.

Positive control in the same check: cf.list_distributions(MaxItems="1") returned a response containing DistributionList, so the client and transport are healthy.

## Steps to Reproduce
1. Point a botocore/AWS SDK CloudFront client at the emulator.
2. Call ListConnectionFunctions with a Stage value outside the FunctionStage enum, e.g.:
     cf.list_connection_functions(Stage="NOT_A_VALID_STAGE")
3. Catch the ClientError and read exc.response["Error"]["Code"] and ["Message"].
4. Observe Code == "MalformedXML" (correct) but the message ends with
     "Member must satisfy enum value set: [DEVELOPMENT, LIVE]"
   where AWS ends it with
     "Member must satisfy enum value set: [LIVE, DEVELOPMENT]".
5. As a control, call cf.list_distributions(MaxItems="1") on the same client — it succeeds, confirming the divergence is confined to the enum-validation message.

Replay to confirm a fix: cloudfront.list-connection-functions-invalid-stage-malformed-xml-rejected
