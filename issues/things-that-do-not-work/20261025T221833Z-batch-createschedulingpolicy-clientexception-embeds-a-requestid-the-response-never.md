Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.quota-share-scheduling-policy-idle-resource-assignment-strategy-enum-enforced
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.quota-share-scheduling-policy-idle-resource-assignment-strategy-enum-enforced|run#18|AssertionError|assert message == expected, (
Issue-Commit: 54a256303123f71c03a3ec9f8760a69174306c5f

# CreateSchedulingPolicy ClientException embeds a RequestId the response never exposes (ResponseMetadata.RequestId absent)

> _Note: this issue was identified against the code base at commit `54a256303123f71c03a3ec9f8760a69174306c5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedulingPolicy

## Description
One cause.

Cause 1 of 1: the Batch error response for a refused CreateSchedulingPolicy does not expose a request id to the client, although the ClientException message text embeds one. Everything else about this refusal is already correct — the emulator closes the enum, returns the right code and the right sentence, creates nothing, and leaves the accepted sibling policy untouched; the single divergence is the missing request-id linkage.

AWS: Smithy — com.amazonaws.batch#QuotaSharePolicy.idleResourceAssignmentStrategy is @required and targets com.amazonaws.batch#QuotaShareIdleResourceAssignmentStrategy, an enum whose only member is FIFO ("Currently, only FIFO is supported"); the refusal is com.amazonaws.batch#ClientException, errorKind client, httpError 400. The message text itself is recorded behavior only: AWS answers a strategy outside that closed set with HTTP 400, error code `ClientException`, and a message of the form `Error executing request, Exception : Idle resource assignment strategy must be one of [FIFO]., RequestId: <request-id>`. In that template the fixed text is `Error executing request, Exception : Idle resource assignment strategy must be one of [`, the closed enum's members `FIFO` rendered from the model, `]., RequestId: `, and the single variable part is `<request-id>` — the request id of THAT SAME response, i.e. the exact value the SDK surfaces as ResponseMetadata.RequestId (the `x-amzn-RequestId` response header). The two are the same string by construction, which is what lets a client quote the id it logged from a failed call.

Emulator: the status, the code and the sentence are right, and the message does end in `RequestId: <uuid>` (one recorded run showed `78205987-0861-448f-bef1-b589dafb10dd`; the uuid itself is a per-run value and is not the requirement). But on the same response `exc.response['ResponseMetadata'].get('RequestId')` is None — the response carries no request-id header/metadata at all — so the id printed in the message corresponds to nothing the client can observe. The check reconstructs the expected message from the response's own metadata and therefore renders it as `... RequestId: None`, which is how the mismatch surfaces: expected `Error executing request, Exception : Idle resource assignment strategy must be one of [FIFO]., RequestId: None`, got the same sentence ending in a uuid.

Why: a client that logs ResponseMetadata.RequestId on failure records nothing for this call, and a client that parses the id out of the message holds a value it can never match against any response metadata it captured. Correlation, support-case reporting, and any retry/diagnostic path keyed on the request id of a refused call are broken, and the message and the metadata of a single response contradict each other.

This is the same root cause as the already-open issue 20261024T014332Z-batch-ecr-lambda-logs-batch-error-responses-carry-no-request-id-so.md, which records it for RegisterJobDefinition under the batch__ecr__lambda__logs check corpus; that issue does not carry this check's replay key, so this instance on the `batch` service / CreateSchedulingPolicy path is filed separately and the two should be closed together.

Fix: the Batch error-serialization path — whatever emits the request-id response header and ResponseMetadata for error responses — together with the CreateSchedulingPolicy handler's quotaSharePolicy validation that composes the ClientException message, so both render the one id generated for the request rather than the validator minting its own.

Traces: batch.quota-share-scheduling-policy-idle-resource-assignment-strategy-enum-enforced (the enum enforcement, the error code, the positive FIFO control, the round-trip read and the effect-absent read all pass; only the request-id linkage in the message fails).

## Evidence
Check: batch.quota-share-scheduling-policy-idle-resource-assignment-strategy-enum-enforced (verdict: mismatch).

Call: CreateSchedulingPolicy(name=<unique>, quotaSharePolicy={'idleResourceAssignmentStrategy': 'STRICT'}, tags={...}).

Expected (built by the check from the refusal's OWN ResponseMetadata.RequestId): code `ClientException`, message `Error executing request, Exception : Idle resource assignment strategy must be one of [FIFO]., RequestId: <ResponseMetadata.RequestId>`.

Actual: code `ClientException` (correct), message `Error executing request, Exception : Idle resource assignment strategy must be one of [FIFO]., RequestId: 78205987-0861-448f-bef1-b589dafb10dd`, while `exc.response['ResponseMetadata'].get('RequestId')` is None — so the expected string renders as `... RequestId: None`.

AssertionError (verbatim): the refusal must name the closed set it enforced, so a generic 400 or an authorization failure cannot pass for this validation; expected 'Error executing request, Exception : Idle resource assignment strategy must be one of [FIFO]., RequestId: None', got 'Error executing request, Exception : Idle resource assignment strategy must be one of [FIFO]., RequestId: 78205987-0861-448f-bef1-b589dafb10dd'

Note the divergence is NOT the per-run uuid value: it is that the response exposes no request id at all, so the embedded id is unmatchable. Model backing: QuotaShareIdleResourceAssignmentStrategy values == ['FIFO']; ClientException httpError 400, errorKind client.

## Steps to Reproduce
1. batch.create_scheduling_policy(name=<name-A>, quotaSharePolicy={'idleResourceAssignmentStrategy': 'FIFO'}, tags={...}) — positive control; it is accepted and DescribeSchedulingPolicies echoes quotaSharePolicy with FIFO and no fairsharePolicy (this part passes).
2. batch.create_scheduling_policy(name=<name-B>, quotaSharePolicy={'idleResourceAssignmentStrategy': 'STRICT'}, tags={...}) — STRICT is outside the one-member enum, so the call is refused.
3. Catch botocore ClientError and read both exc.response['Error']['Code'], exc.response['Error']['Message'] and exc.response['ResponseMetadata'].get('RequestId').
4. Observe Code == 'ClientException' and the message ending in `RequestId: <uuid>`, while ResponseMetadata.RequestId is None — the id in the message matches no request id the client can see. On AWS the two values are identical.
