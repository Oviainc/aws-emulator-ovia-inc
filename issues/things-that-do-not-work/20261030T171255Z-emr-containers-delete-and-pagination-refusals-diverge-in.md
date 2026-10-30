Issue-Source: trace-sweep-divergence
Issue-Trace: emr-containers-virtual-cluster-mutation-error-paths
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-containers-virtual-cluster-mutation-error-paths emr-containers-managed-endpoint-job-run-error-paths emr-containers-error-paths
Issue-Shape-Id: com.amazonaws.emrcontainers#AwsChicagoWebService
Issue-Key: emr-containers|DeleteVirtualCluster|4xx->4xx|$.message
Issue-Key: emr-containers|DeleteManagedEndpoint|4xx->4xx|$.message
Issue-Key: emr-containers|ListJobTemplates|4xx->4xx|$.message
Issue-Key: emr-containers|CancelJobRun|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 5e13030fba3716cb0d2a1354c811e28788b6a350

# emr-containers delete and pagination refusals diverge in message text and error code

> _Note: this issue was identified against the code base at commit `5e13030fba3716cb0d2a1354c811e28788b6a350`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteVirtualCluster

## Description
Four net-new root causes in the emr-containers emulator, all the same family: the operation gets the HTTP status right (400) but the error PAYLOAD diverges - either the `message` is the emulator's own descriptive sentence built by interpolating the requested id where AWS returns a short fixed sentence that names nothing, or the `x-amzn-errortype` carries a different error code than AWS. All four causes must be fixed and all four fixture steps must replay clean before this issue is closed.

Common structural note (Hypothesis, applies to Causes 1 and 2): both delete handlers appear to build their refusal message from the service's single not-found wording helper (`crate::state::ResourceKind::not_found_message`, the helper named in open issue 20261029T185510Z for the job-template and security-configuration delete paths). That wording is correct for the Describe* paths' ResourceNotFoundException, and wrong for the mutation refusal, which AWS words with a short fixed sentence carrying no id. The two error classes need separate wording, and the fix must not disturb the Describe* wording.

Cause 1 of 4 - DeleteVirtualCluster on a virtual cluster that does not exist.
AWS: Smithy: DeleteVirtualCluster, DELETE /virtualclusters/{id}, http code 200; `id` = ResourceIdString, @required @httpLabel, @length 1..64, @pattern ^[0-9a-z]+$; errors modeled are exactly InternalServerException(500) and ValidationException(400) - no not-found error is bound, so a dead id is necessarily a ValidationException. Recorded behavior: HTTP 400 with message `VirtualCluster does not exist`. That message is entirely FIXED text: no variable parts, and in particular it does not echo the requested id.
Emulator: HTTP 400 (correct) but message `Virtual cluster <id> doesn't exist.` - a different sentence whose fixed parts are `Virtual cluster ` / ` doesn't exist.` and whose variable part is the `id` taken from the request URI.
Why: the only thing distinguishing this refusal from AWS's is the message, so idempotent teardown that tolerates `VirtualCluster does not exist` does not recognize the emulator's refusal and fails cleanup; a caller classifying refusals by message mis-buckets it.
Fix: the not-found branch of DeleteVirtualCluster's handler in the emr-containers service crate - emit AWS's fixed sentence for the ValidationException instead of the shared not-found wording helper.
Traces: fixture emr-containers-virtual-cluster-mutation-error-paths step task#2.

Cause 2 of 4 - DeleteManagedEndpoint on a managed endpoint that does not exist.
AWS: Smithy: DeleteManagedEndpoint, DELETE /virtualclusters/{virtualClusterId}/endpoints/{id}, http code 200; both `id` and `virtualClusterId` are ResourceIdString @required @httpLabel @length 1..64 @pattern ^[0-9a-z]+$; errors modeled are exactly InternalServerException(500) and ValidationException(400) - again no not-found error. Recorded behavior: HTTP 400 with message `Endpoint does not exist`, entirely FIXED text with no id.
Emulator: HTTP 400 (correct) but message `Endpoint <id> doesn't exist.` - fixed parts `Endpoint ` / ` doesn't exist.`, variable part the request's `id`.
Why: same as Cause 1 - endpoint teardown and retry logic keyed on AWS's sentence does not match, and the emulator's sentence also discloses the requested id on a path where AWS discloses nothing, so a log scrubber or assertion written against AWS behaves differently here.
Fix: the not-found branch of DeleteManagedEndpoint's handler in the emr-containers service crate - same change, same shared helper to stop calling for the mutation refusal.
Traces: fixture emr-containers-managed-endpoint-job-run-error-paths step task#3.

Cause 3 of 4 - ListJobTemplates rejects a corrupt nextToken with the emulator's own wording instead of the decoder's.
AWS: Smithy: ListJobTemplates, GET /jobtemplates; `nextToken` = NextToken @httpQuery, @length 1..1024, @pattern \S; errors InternalServerException(500) and ValidationException(400); paginated {inputToken: nextToken, outputToken: nextToken, items: templates}. The nextToken is an opaque base64 value and AWS surfaces the base64 decode failure itself: recorded behavior is HTTP 400 with message `Illegal base64 character 2d`. The fixed text is `Illegal base64 character ` and the variable part is the lower-case hex code of the FIRST character of the supplied token that is not in the base64 alphabet (here 2d, i.e. `-`), taken from the request's nextToken - so the rule is "report the offending character of the client's token", not a literal `2d`.
Emulator: HTTP 400 with message `nextToken is not a valid pagination token` - a sentence of the emulator's own composition with no variable part, which tells the caller nothing about what was wrong with its token.
Why: a client that distinguishes a structurally corrupt token (a truncated or re-encoded cursor it must discard) from other validation refusals does so on the message, since the error code is shared by every ValidationException of this operation; AWS's message also identifies the offending byte, which is the only diagnostic a caller has for an opaque cursor. Neither is available against the emulator.
Fix: the nextToken validation/deserialization of ListJobTemplates in the emr-containers service crate - decode the token as base64 and report the decoder's failure in AWS's wording, with the offending character's hex code from the supplied token; keep the ValidationException class and the 400.
Traces: fixture emr-containers-error-paths step task#5.

Cause 4 of 4 - CancelJobRun refuses with the wrong error code in x-amzn-errortype.
AWS: Smithy: CancelJobRun, DELETE /virtualclusters/{virtualClusterId}/jobruns/{id}, http code 200; both labels are ResourceIdString @required @httpLabel @length 1..64 @pattern ^[0-9a-z]+$; the operation models exactly InternalServerException(500) and ValidationException(400). Recorded behavior: HTTP 400 whose `message` the emulator already reproduces correctly, but whose `x-amzn-errortype` is asserted by shape and is NOT `ValidationException`; the recorded code is therefore one the operation's own error list does not contain. Hypothesis: it is the service's resource-not-found code, the same code the emulator (wrongly, per the sibling causes) reaches for on the tag paths - the fixture asserts the shape rather than the literal, so the exact code must be read back off the recorded response rather than guessed.
Emulator: HTTP 400 with `x-amzn-errortype: ValidationException`, message matching.
Why: SDKs dispatch on `x-amzn-errortype`, not on the message, so a caller that catches the recorded error type never catches the emulator's refusal at all and it surfaces as an unmodeled/generic service error; a caller that catches ValidationException catches a condition AWS would not have raised.
Fix: the refusal branch of CancelJobRun's handler/serializer in the emr-containers service crate - classify the dead-job-run refusal under the error type AWS records for this path (reading it from the fixture's recorded response) and serialize that code into `x-amzn-errortype`; the message and the 400 are already right.
Traces: fixture emr-containers-managed-endpoint-job-run-error-paths step task#6.

Related but not covering: open issue 20261029T185510Z-emr-containers-validationexception-refusals-carry-the-emulator-s.md is the same wording family but is scoped to DeleteJobTemplate, DeleteSecurityConfiguration and CreateJobTemplate client-token conflicts - it does not touch DeleteVirtualCluster, DeleteManagedEndpoint, ListJobTemplates pagination or CancelJobRun. Open issue 20261022T001417Z-emr-containers-error-paths-missing-input-validation-on.md owns the CreateVirtualCluster and DescribeJobRun steps of this corpus, not these four. Open issue 20261029T104910Z-emr-containers-listvirtualclusters-pagination.md concerns a nextToken honoured across Regions on a different operation, not the base64 decode wording in Cause 3.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator built from this checkout (35 steps, 24 passed, 11 mismatched, score 68.6%). Four of the mismatches are the clusters below; in all four the HTTP status is already correct and only the error payload diverges.

Cause 1 - [mismatch] emr-containers-virtual-cluster-mutation-error-paths#task#2, DeleteVirtualCluster, expected 400 / got 400:
  $.message: value_mismatch (expected 'VirtualCluster does not exist', got "Virtual cluster sim2realprobenonexistentvc00 doesn't exist.")
  (the id in the actual message is a per-run probe value; the defect is the sentence - AWS's has no variable part)

Cause 2 - [mismatch] emr-containers-managed-endpoint-job-run-error-paths#task#3, DeleteManagedEndpoint, expected 400 / got 400:
  $.message: value_mismatch (expected 'Endpoint does not exist', got "Endpoint sim2realprobenonexistentep00 doesn't exist.")

Cause 3 - [mismatch] emr-containers-error-paths#task#5, ListJobTemplates, expected 400 / got 400:
  $.message: value_mismatch (expected 'Illegal base64 character 2d', got 'nextToken is not a valid pagination token')
  (2d is the hex code of the first non-base64 character in the token the probe sent; the fixed text is 'Illegal base64 character ')

Cause 4 - [mismatch] emr-containers-managed-endpoint-job-run-error-paths#task#6, CancelJobRun, expected 400 / got 400:
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')
  no message divergence was reported for this step, so the message is already correct and only the error code is wrong.

Model evidence (services/service-com-amazonaws-emrcontainers-awschicagowebservice/smithy-model/com-amazonaws-emrcontainers-awschicagowebservice.json), via smithy_query operation=DeleteVirtualCluster,DeleteManagedEndpoint,ListJobTemplates,CancelJobRun:
  DeleteVirtualCluster: DELETE /virtualclusters/{id}, code 200; id -> ResourceIdString required httpLabel length{1,64} pattern ^[0-9a-z]+$; errors InternalServerException(500), ValidationException(400) - no not-found error modeled.
  DeleteManagedEndpoint: DELETE /virtualclusters/{virtualClusterId}/endpoints/{id}, code 200; both labels ResourceIdString required httpLabel length{1,64} pattern ^[0-9a-z]+$; errors InternalServerException(500), ValidationException(400).
  ListJobTemplates: GET /jobtemplates, code 200; nextToken -> NextToken httpQuery length{1,1024} pattern \S; errors InternalServerException(500), ValidationException(400); paginated {inputToken: nextToken, outputToken: nextToken, items: templates, pageSize: maxResults}.
  CancelJobRun: DELETE /virtualclusters/{virtualClusterId}/jobruns/{id}, code 200; both labels ResourceIdString required httpLabel length{1,64} pattern ^[0-9a-z]+$; errors InternalServerException(500), ValidationException(400) - so the recorded x-amzn-errortype for this path is a code outside the operation's modeled error list and must be taken from the recorded response.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay wire corpus simparity-wire-aws-v1, fixtures emr-containers-virtual-cluster-mutation-error-paths, emr-containers-managed-endpoint-job-run-error-paths and emr-containers-error-paths.
3. Cause 1 (emr-containers-virtual-cluster-mutation-error-paths task#2): call DeleteVirtualCluster with an `id` satisfying ResourceIdString that names no virtual cluster. Expect HTTP 400 with message exactly 'VirtualCluster does not exist'; observed "Virtual cluster <id> doesn't exist.".
4. Cause 2 (emr-containers-managed-endpoint-job-run-error-paths task#3): call DeleteManagedEndpoint with a valid `virtualClusterId` and an `id` naming no managed endpoint. Expect HTTP 400 with message exactly 'Endpoint does not exist'; observed "Endpoint <id> doesn't exist.".
5. Cause 3 (emr-containers-error-paths task#5): call ListJobTemplates with a `nextToken` that satisfies the NextToken length/pattern constraint but is not valid base64 (e.g. one containing '-'). Expect HTTP 400 with message 'Illegal base64 character <hex of the first offending character>' (e.g. 'Illegal base64 character 2d' for '-'); observed 'nextToken is not a valid pagination token'.
6. Cause 4 (emr-containers-managed-endpoint-job-run-error-paths task#6): call CancelJobRun with a valid `virtualClusterId` and an `id` naming no job run. Expect HTTP 400 carrying the error code recorded in that step's x-amzn-errortype (NOT ValidationException), with the message already produced; observed x-amzn-errortype ValidationException.
7. Confirm the Describe* wording is unchanged: DescribeVirtualCluster / DescribeManagedEndpoint on a dead id must keep their existing ResourceNotFoundException wording - only the mutation refusals change.
8. Re-replay emr-containers-virtual-cluster-mutation-error-paths#task#2, emr-containers-managed-endpoint-job-run-error-paths#task#3, emr-containers-error-paths#task#5 and emr-containers-managed-endpoint-job-run-error-paths#task#6; all four must pass.
