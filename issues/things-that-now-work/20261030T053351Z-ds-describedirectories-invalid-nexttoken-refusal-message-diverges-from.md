Issue-Source: trace-sweep-divergence
Issue-Trace: ds-directory-error-taxonomy
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ds-directory-error-taxonomy
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds|DescribeDirectories|4xx->4xx|$.Message
Issue-Commit: 2d11c019ca89f17fce399c4a7a8c59c866e2a978

# ds DescribeDirectories invalid-NextToken refusal Message diverges from the recorded ds error body

> _Note: this issue was identified against the code base at commit `2d11c019ca89f17fce399c4a7a8c59c866e2a978`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDirectories

## Description
One net-new wire divergence in the Directory Service (ds) control plane, found by replaying the recorded corpus against the emulator. The HTTP status (400) and the error class (__type InvalidNextTokenException) both match; what diverges is the text the emulator puts in the error body's Message member. This is the ONE cause this issue carries (Cause 1 of 1).

Cause 1 of 1 - DescribeDirectories' invalid-NextToken refusal carries an emulator-authored Message instead of the sentence ds words for InvalidNextTokenException.

AWS: Smithy (services/service-com-amazonaws-directoryservice-directoryservice-20150416): DescribeDirectories is @paginated (inputToken/outputToken NextToken, items DirectoryDescriptions, pageSize Limit) and models InvalidNextTokenException among its errors - errorKind client, HTTP 400 - whose structure declares exactly two members, Message (ExceptionMessage) and RequestId (@pattern ^([A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12})$). The shape's own documentation is "The NextToken value is not valid." Beyond the class, the recording asserts $.Message as SHAPED and not as a literal (the replay's expected value is the sentinel '<shaped-not-asserted>'), so for this step the exact sentence is recorded behavior only: the requirement is that Message be the ds-worded sentence for this error class with the per-run request id supplied as the body's own RequestId value, not emulator prose. Hypothesis (grounded in the sibling ds recordings in this same corpus, e.g. the trust not-found bodies recorded as "Trust: <trust id> does not exist : RequestId: <request id>"): ds's Message template for a client refusal is "<error sentence> : RequestId: <request id>" with NO full stop terminating the sentence immediately before the " : RequestId: " separator. Fixed text of the template: the error sentence for the class and the " : RequestId: " separator; variable parts: only the request id, which is per-run.

Emulator: answers HTTP 400 with the right __type but $.Message = "The NextToken value is not valid. : RequestId: 00000000-0000-4000-8000-000000000001", which the replay flags as "$.Message: value_mismatch (expected '<shaped-not-asserted>', got 'The NextToken value is not valid. : RequestId: 00000000-0000-4000-8000-000000000001')" - i.e. the Message the emulator builds is not the shaped Message the recording holds for this refusal. The diverging part is FIXED text (the sentence as the emulator terminates and concatenates it, specifically the full stop placed directly before the " : RequestId: " separator); the trailing identifier itself is a per-run value and is explicitly NOT part of the requirement being filed here.

Why: a client that distinguishes an invalid continuation token from the other 400 classes ds can raise on a listing (ClientException, EntityDoesNotExistException, InvalidParameterException) by matching the error text - as SDK retry/paging wrappers, log scrapers and recorded-response assertions do - fails to match, and the sentence surfaced to the user is not the sentence AWS surfaces. The step is a pagination-taxonomy step, so the whole invalid-token branch of a paging client is validated against text AWS never sends.

Fix: the ds error-message builder for the invalid-NextToken refusal, reached from DescribeDirectories' NextToken validation in its handler before the page is cut - it must emit the class's ds-worded sentence and append the request id with ds's separator, without re-terminating the sentence. The same builder is shared by ds's other paginated listings, so fixing it in DescribeDirectories alone leaves the siblings diverging.

Traces: ds-directory-error-taxonomy#task#1.

Related open issues that do NOT cover this step (none carries its replay key, so none owns it): 20261029T180805Z-ds-directory-service-response-fidelity-input-validation-skipped-or.md owns the six other ds divergences in this replay (including the VerifyTrust/DeleteTrust $.Message wording causes) but says nothing about DescribeDirectories or InvalidNextTokenException; 20261029T200700Z-ds-10-net-new-refusal-classification-and-message.md and 20261029T200700Z-ds-7-net-new-root-causes-connectdirectory-describesettings.md discuss other ds refusal wording and DescribeDirectories members (Size) respectively.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, 54 steps, 35 passed, 19 mismatched, 7 divergence clusters - all service "ds"; six clusters are owned by 20261029T180805Z-ds-directory-service-response-fidelity-input-validation-skipped-or.md, this one has no owner):

fixture ds-directory-error-taxonomy, step task#1 - DescribeDirectories, verdict mismatch, expected status 400 / actual status 400.
  divergence_key: service=ds|DescribeDirectories|status=400->400|keys=$.Message
  detail: "$.Message: value_mismatch (expected '<shaped-not-asserted>', got 'The NextToken value is not valid. : RequestId: 00000000-0000-4000-8000-000000000001')"
  affected: ds-directory-error-taxonomy#task#1 (cluster size 1); stub_path empty (not an unimplemented-operation block); owned_by empty.

Matching keys: HTTP status 400 matches and $.__type is not reported, so the error CLASS (InvalidNextTokenException) is already right; only the Message text diverges.

Model evidence (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json): DescribeDirectories errors = ClientException, EntityDoesNotExistException, InvalidNextTokenException, InvalidParameterException (all client, HTTP 400, defaulted) and ServiceException (server, 500); paginated {inputToken: NextToken, outputToken: NextToken, items: DirectoryDescriptions, pageSize: Limit}. InvalidNextTokenException = structure{Message: ExceptionMessage, RequestId: RequestId @pattern uuid} with documentation "The NextToken value is not valid." The request id in the observed body is a per-run value and is not asserted as a requirement.

Cross-recording evidence for the ds Message template: in the same corpus the recorded ds client-refusal bodies are shaped "<sentence> : RequestId: <uuid>" with no full stop before the separator (expected $.Message "Trust: t-1234567890 does not exist : RequestId: <uuid>" for VerifyTrust/DeleteTrust in ds-trust-not-found-errors#task#2 and #task#4).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1 of 1: fixture ds-directory-error-taxonomy, step task#1 - call DescribeDirectories with a NextToken the service never issued.
3. Observe HTTP 400 with __type InvalidNextTokenException (correct) but a body whose Message is the emulator-built string "The NextToken value is not valid. : RequestId: <sequential id>", which the replay reports as a $.Message value_mismatch against the recorded (shaped) Message for that refusal.
4. Compare with the ds refusal bodies recorded in ds-trust-not-found-errors (steps task#2/task#4) to see the ds Message template this refusal must follow: the class's sentence, then " : RequestId: " and the response's own request id, with no full stop closing the sentence before the separator.
