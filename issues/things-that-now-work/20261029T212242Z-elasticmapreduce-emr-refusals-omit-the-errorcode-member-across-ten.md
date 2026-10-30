Issue-Source: trace-sweep-divergence
Issue-Trace: elasticmapreduce-cluster-inventory-and-unknown-cluster-reads
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticmapreduce-cluster-inventory-and-unknown-cluster-reads elasticmapreduce-cluster-mutations-on-unknown-cluster elasticmapreduce-cluster-scaling-and-policy-errors
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: elasticmapreduce|DescribeCluster|4xx->4xx|$.ErrorCode
Issue-Key: elasticmapreduce|ListInstanceGroups|4xx->4xx|$.ErrorCode
Issue-Key: elasticmapreduce|ListInstanceFleets|4xx->4xx|$.ErrorCode
Issue-Key: elasticmapreduce|ListSteps|4xx->4xx|$.ErrorCode
Issue-Key: elasticmapreduce|DescribeStep|4xx->4xx|$.ErrorCode
Issue-Key: elasticmapreduce|CancelSteps|4xx->4xx|$.ErrorCode
Issue-Key: elasticmapreduce|AddTags|4xx->4xx|$.ErrorCode
Issue-Key: elasticmapreduce|RemoveTags|4xx->4xx|$.ErrorCode
Issue-Key: elasticmapreduce|ModifyCluster|4xx->4xx|$.ErrorCode
Issue-Key: elasticmapreduce|AddInstanceFleet|4xx->4xx|$.ErrorCode
Issue-Commit: 14bf84ae7f57c35d094a91bae03d265103651cef

# EMR refusals omit the ErrorCode member across ten cluster-id validation paths

> _Note: this issue was identified against the code base at commit `14bf84ae7f57c35d094a91bae03d265103651cef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
This issue bundles 10 net-new root causes in the EMR (elasticmapreduce) crate that share ONE fault: when an operation refuses a malformed / nonexistent cluster or job-flow id, the emulator serialises the modelled `InvalidRequestException` with only its `Message` member and NEVER populates `ErrorCode`. The HTTP status (400) and the exception type are correct on every one of these paths; the `ErrorCode` member is simply absent from the JSON body.

AWS (shared by all 10 causes): Smithy: `com.amazonaws.emr#InvalidRequestException` is a client error, HTTP 400, with TWO members — `ErrorCode` (target `ErrorCode`, @length min 1 max 256, documented "The error code associated with the exception.") and `Message`. Real EMR populates `ErrorCode` on these refusals with a machine-readable discriminator; the recordings show the discriminator is per-operation-family, not a single constant: the cluster/step/tag/instance-group read and mutate paths use `MalformedClusterId`, while `AddInstanceFleet` uses `INVALID_JOB_ID_MALFORMED`. `ErrorCode` is the field a client branches on, which is why AWS sets it even though `Message` already carries prose.

Emulator (shared by all 10 causes): the error body contains `Message` only; `$.ErrorCode` is missing entirely (`got None` in every cluster). Whatever helper builds `InvalidRequestException` in this crate constructs it from a message string alone and leaves `ErrorCode` as `None`, so it is never serialised.

Why (shared by all 10 causes): `ErrorCode` is the only stable, non-prose discriminator on EMR's `InvalidRequestException` — the same exception type and the same 400 cover unknown clusters, unknown release labels, malformed security configurations and more. With the member absent, a client cannot tell a malformed-cluster-id refusal from any other `InvalidRequestException`; code that reads `error.ErrorCode` gets null and falls through to a generic failure branch, and retry/classification logic that keys on `MalformedClusterId` (do not retry, fix the id) never fires.

Fix (shared by all 10 causes): the error-construction path these handlers share in the EMR crate — the `InvalidRequestException` builder/helper must take and set `ErrorCode` alongside `Message`, and each of the handlers below must pass its operation-family's code. Per-operation handlers live under the EMR crate's `src/ops/` (see Traces for the exact failing step of each).

--- Cause 1 of 10: DescribeCluster omits ErrorCode=MalformedClusterId ---
AWS: Smithy: `DescribeCluster(ClusterId required, @length 0..256)`, errors `InternalServerException` (500) and `InvalidRequestException` (client, 400). Recorded behavior: a malformed cluster id is refused 400 `InvalidRequestException` with `ErrorCode` = the constant `MalformedClusterId` (no variable part) and a `Message` naming the id.
Emulator: 400 with the right message but `$.ErrorCode` missing.
Why / Fix: as shared above, in the DescribeCluster handler.
Traces: elasticmapreduce-cluster-inventory-and-unknown-cluster-reads#task#2.

--- Cause 2 of 10: ListInstanceGroups omits ErrorCode=MalformedClusterId ---
AWS: same `InvalidRequestException` shape; `ErrorCode` is the constant `MalformedClusterId`.
Emulator: `$.ErrorCode` missing.
Why / Fix: as shared above, in the ListInstanceGroups handler.
Traces: elasticmapreduce-cluster-inventory-and-unknown-cluster-reads#task#4.

--- Cause 3 of 10: ListInstanceFleets omits ErrorCode=MalformedClusterId ---
AWS: `ErrorCode` = constant `MalformedClusterId`.
Emulator: `$.ErrorCode` missing.
Why / Fix: as shared above, in the ListInstanceFleets handler.
Traces: elasticmapreduce-cluster-inventory-and-unknown-cluster-reads#task#5.

--- Cause 4 of 10: ListSteps omits ErrorCode=MalformedClusterId ---
AWS: `ErrorCode` = constant `MalformedClusterId`.
Emulator: `$.ErrorCode` missing.
Why / Fix: as shared above, in the ListSteps handler.
Traces: elasticmapreduce-cluster-inventory-and-unknown-cluster-reads#task#7.

--- Cause 5 of 10: DescribeStep omits ErrorCode=MalformedClusterId ---
AWS: `ErrorCode` = constant `MalformedClusterId`.
Emulator: `$.ErrorCode` missing.
Why / Fix: as shared above, in the DescribeStep handler.
Traces: elasticmapreduce-cluster-inventory-and-unknown-cluster-reads#task#8.

--- Cause 6 of 10: CancelSteps omits ErrorCode=MalformedClusterId ---
AWS: `ErrorCode` = constant `MalformedClusterId`.
Emulator: `$.ErrorCode` missing.
Why / Fix: as shared above, in the CancelSteps handler.
Traces: elasticmapreduce-cluster-mutations-on-unknown-cluster#task#2.

--- Cause 7 of 10: AddTags omits ErrorCode=MalformedClusterId ---
AWS: `ErrorCode` = constant `MalformedClusterId`.
Emulator: `$.ErrorCode` missing.
Why / Fix: as shared above, in the AddTags handler.
Traces: elasticmapreduce-cluster-mutations-on-unknown-cluster#task#3.

--- Cause 8 of 10: RemoveTags omits ErrorCode=MalformedClusterId ---
AWS: `ErrorCode` = constant `MalformedClusterId`.
Emulator: `$.ErrorCode` missing.
Why / Fix: as shared above, in the RemoveTags handler.
Traces: elasticmapreduce-cluster-mutations-on-unknown-cluster#task#4.

--- Cause 9 of 10: ModifyCluster omits ErrorCode=MalformedClusterId ---
AWS: `ErrorCode` = constant `MalformedClusterId`.
Emulator: `$.ErrorCode` missing.
Why / Fix: as shared above, in the ModifyCluster handler.
Traces: elasticmapreduce-cluster-mutations-on-unknown-cluster#task#5.

--- Cause 10 of 10: AddInstanceFleet omits ErrorCode=INVALID_JOB_ID_MALFORMED ---
AWS: `AddInstanceFleet` refuses a malformed cluster id with the SAME 400 `InvalidRequestException`, but `ErrorCode` is the DIFFERENT constant `INVALID_JOB_ID_MALFORMED` — so the fix is not one global constant; the per-operation code must be preserved.
Emulator: `$.ErrorCode` missing.
Why / Fix: as shared above, in the AddInstanceFleet handler; it must emit `INVALID_JOB_ID_MALFORMED`, not the cluster-read code.
Traces: elasticmapreduce-cluster-scaling-and-policy-errors#task#3.

Note: the open behavioral issue "EMR control-plane refusals use emulator-authored messages (and one wrong error code) for 10 distinct validation paths" (20261029T175551Z-emr-control-plane-refusals-use-emulator-authored-messages.md) covers EMR refusal MESSAGE text and one wrong error TYPE, but it does not cover the missing `ErrorCode` MEMBER on any of these ten paths, and it carries no wire replay key for them.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator (service token `elasticmapreduce`; 124/202 steps passed, 74 mismatched). Each cause below is one divergence cluster; status matched (400 expected, 400 actual) and only the ErrorCode member diverged.

Cause 1 — fixture elasticmapreduce-cluster-inventory-and-unknown-cluster-reads step task#2, DescribeCluster: `$.ErrorCode: missing (expected 'MalformedClusterId', got None)`. divergence_key `service=elasticmapreduce|DescribeCluster|status=400->400|keys=$.ErrorCode`.
Cause 2 — same fixture, task#4, ListInstanceGroups: `$.ErrorCode: missing (expected 'MalformedClusterId', got None)`.
Cause 3 — same fixture, task#5, ListInstanceFleets: `$.ErrorCode: missing (expected 'MalformedClusterId', got None)`.
Cause 4 — same fixture, task#7, ListSteps: `$.ErrorCode: missing (expected 'MalformedClusterId', got None)`.
Cause 5 — same fixture, task#8, DescribeStep: `$.ErrorCode: missing (expected 'MalformedClusterId', got None)`.
Cause 6 — fixture elasticmapreduce-cluster-mutations-on-unknown-cluster step task#2, CancelSteps: `$.ErrorCode: missing (expected 'MalformedClusterId', got None)`.
Cause 7 — same fixture, task#3, AddTags: `$.ErrorCode: missing (expected 'MalformedClusterId', got None)`.
Cause 8 — same fixture, task#4, RemoveTags: `$.ErrorCode: missing (expected 'MalformedClusterId', got None)`.
Cause 9 — same fixture, task#5, ModifyCluster: `$.ErrorCode: missing (expected 'MalformedClusterId', got None)`.
Cause 10 — fixture elasticmapreduce-cluster-scaling-and-policy-errors step task#3, AddInstanceFleet: `$.ErrorCode: missing (expected 'INVALID_JOB_ID_MALFORMED', got None)`. divergence_key `service=elasticmapreduce|AddInstanceFleet|status=400->400|keys=$.ErrorCode`.

Smithy grounding: `com.amazonaws.emr#InvalidRequestException` = {ErrorCode: ErrorCode @length(1,256) "The error code associated with the exception.", Message: ErrorMessage}, errorKind client, httpError 400.

## Steps to Reproduce
Boot the emulator and replay these steps from wire trace set simparity-wire-aws-v1 (service token `elasticmapreduce`):
1. Replay fixture `elasticmapreduce-cluster-inventory-and-unknown-cluster-reads` and inspect steps task#2 (DescribeCluster), task#4 (ListInstanceGroups), task#5 (ListInstanceFleets), task#7 (ListSteps), task#8 (DescribeStep) — each calls the operation with a well-formed-looking but nonexistent/malformed ClusterId and asserts `Error.ErrorCode == 'MalformedClusterId'` on the 400 body.
2. Replay fixture `elasticmapreduce-cluster-mutations-on-unknown-cluster` and inspect steps task#2 (CancelSteps), task#3 (AddTags), task#4 (RemoveTags), task#5 (ModifyCluster) — same assertion.
3. Replay fixture `elasticmapreduce-cluster-scaling-and-policy-errors` step task#3 (AddInstanceFleet) — asserts `Error.ErrorCode == 'INVALID_JOB_ID_MALFORMED'`.
In every case the response is HTTP 400 with an `InvalidRequestException` whose body has a `Message` but no `ErrorCode` key at all.
