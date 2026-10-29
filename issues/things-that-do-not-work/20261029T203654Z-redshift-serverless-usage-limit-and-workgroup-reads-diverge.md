Issue-Source: trace-sweep-divergence
Issue-Trace: redshiftserverless-workgroup-resource-management-and-error-semantics
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshiftserverless-workgroup-resource-management-and-error-semantics
Issue-Shape-Id: com.amazonaws.redshiftserverless#RedshiftServerless
Issue-Key: redshift-serverless|CreateUsageLimit|2xx->2xx|$.usageLimit.featureType
Issue-Key: redshift-serverless|CreateUsageLimit|2xx->2xx|$.usageLimit.limitType
Issue-Key: redshift-serverless|CreateUsageLimit|2xx->2xx|$.usageLimit.usageLimitArn
Issue-Key: redshift-serverless|GetUsageLimit|2xx->2xx|$.usageLimit.featureType
Issue-Key: redshift-serverless|GetUsageLimit|2xx->2xx|$.usageLimit.limitType
Issue-Key: redshift-serverless|GetUsageLimit|2xx->2xx|$.usageLimit.usageLimitArn
Issue-Key: redshift-serverless|GetUsageLimit|4xx->2xx|$.__type
Issue-Key: redshift-serverless|GetUsageLimit|4xx->2xx|$.message
Issue-Key: redshift-serverless|GetUsageLimit|4xx->2xx|status
Issue-Key: redshift-serverless|GetWorkgroup|4xx->4xx|status
Issue-Commit: 482573dd66a8fdd59e0387c95d89889e67d3b0e1

# redshift-serverless usage-limit and workgroup reads diverge: response members, ARN form and not-found refusals (4 causes)

> _Note: this issue was identified against the code base at commit `482573dd66a8fdd59e0387c95d89889e67d3b0e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetUsageLimit

## Description
Four independent root causes in the redshift-serverless usage-limit and workgroup read paths, all replayed from the wire fixture `redshiftserverless-workgroup-resource-management-and-error-semantics`. Two are payload defects (modeled/recorded members missing, the usage-limit ARN built with the wrong resource-type segment), two are refusal defects (a read that succeeds where AWS refuses, and a not-found refusal sent with the wrong HTTP status). A fixer must close ALL FOUR before submitting.

No open issue covers any of them. The closest already-filed redshift-serverless entries are '20261029T191808Z-redshift-serverless-refusal-messages-diverge-from-aws-wording' (CreateSnapshot / CreateUsageLimit refusal MESSAGE text only) and the UpdateUsageLimit scaffold-stub issue; the DeleteUsageLimit scaffold stub is filed for us by the replay as a not-implemented operation. None of those touch the four causes below.

=== Cause 1 of 4: CreateUsageLimit's 200 response omits the usage-limit's featureType and limitType ===
AWS: recorded behavior only. The Smithy model's `UsageLimit` structure declares usageLimitId, usageLimitArn, resourceArn, usageType, amount, period and breachAction and declares NO featureType or limitType member (Smithy: com.amazonaws.redshiftserverless#UsageLimit; CreateUsageLimit output member `usageLimit` targets UsageLimit). The recorded CreateUsageLimit 200 response nevertheless carries both members on the usage-limit object: `featureType` with the value of the limited feature (for a `usageType=serverless-compute` request, 'serverless-compute') and `limitType` naming the metric the `amount` is counted in ('compute-used' for serverless compute). Hypothesis: featureType restates the feature being limited and limitType names the metered dimension, the same pair provisioned Redshift reports on a usage limit; the model simply does not document them.
Emulator: answers 200 with the right status and the modeled members, but both of these members are absent from `$.usageLimit` (replay: `$.usageLimit.featureType: missing (expected 'serverless-compute', got None)`, `$.usageLimit.limitType: missing (expected 'compute-used', got None)`).
Why: a client that reads featureType/limitType to decide what unit `amount` is expressed in, or that round-trips the usage-limit object it was handed, sees two null fields where AWS always supplies values, and any deserializer requiring them fails on every create.
Fix: the CreateUsageLimit handler's response construction together with the stored usage-limit record and the shared usage-limit payload type — the two members must be populated on every usage-limit payload this service emits (create, read and list alike), derived from the submitted usageType, not left unset.
Traces: redshiftserverless-workgroup-resource-management-and-error-semantics#task#1.

=== Cause 2 of 4: the usage-limit ARN uses the resource-type segment 'usage-limit/' where AWS uses 'usagelimit/' (and the read-back drops the same two members) ===
AWS: recorded behavior only — the model types `usageLimitArn` as a plain String, so the ARN form comes from the recording. A Redshift Serverless usage-limit ARN is `arn:aws:redshift-serverless:<region>:<account>:usagelimit/<usageLimitId>`: the fixed text is the partition/service prefix and the resource type `usagelimit/`, with region, account id and the usage-limit id supplied by the environment and by state (those three are per-environment values, not requirements). GetUsageLimit must report that ARN, plus the featureType/limitType of Cause 1.
Emulator: reports `...:usage-limit/<id>` — a hyphenated resource type (replay on the read: `$.usageLimit.usageLimitArn: value_mismatch (expected 'arn:aws:redshift-serverless:...:usagelimit/<id>', got 'arn:aws:redshift-serverless:...:usage-limit/<id>')`). The same read also omits featureType and limitType, because the ARN and the record are built once at create time and read straight back.
Why: the ARN is the resource's identity. An IAM policy resource pattern, a tagging call, or any client comparing the ARN it stored against the one AWS would mint never matches, and the mismatch is silent — the call succeeds with an unusable identifier.
Fix: the usage-limit ARN helper in the service's state module (`services/service-com-amazonaws-redshiftserverless-redshiftserverless/src/state.rs`, `usage_limit_arn`, lines 213-220) must emit the `usagelimit/` resource type; the GetUsageLimit handler's payload must carry the Cause 1 members.
Traces: redshiftserverless-workgroup-resource-management-and-error-semantics#task#2.

=== Cause 3 of 4: GetUsageLimit answers 200 with a usage limit that no longer exists, instead of refusing it ===
AWS: GetUsageLimit on a usageLimitId that names no usage limit is refused with HTTP 400 ValidationException (Smithy: GetUsageLimit errors are ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ValidationException 400 — the recording shows the service selects ValidationException/400 for an id that resolves to nothing, not the 404 error). The message is 'The usage limit ID is not valid. <usageLimitId> does not exist': fixed text 'The usage limit ID is not valid. ' and ' does not exist' around one variable part, the usageLimitId taken from the request. Wording is recorded behavior only (the ValidationException `message` member is a plain required String).
Emulator: answers HTTP 200 carrying the usage limit (replay: `status: expected 400, got 200`, `$.__type: missing (expected 'ValidationException', got None)`, `$.message: missing (expected 'The usage limit ID is not valid. <id> does not exist', got None)`) — no refusal at all. Hypothesis for the 200: the record is still in the store because the preceding delete never removed it (DeleteUsageLimit is an unimplemented scaffold returning 501, filed separately by the replay). Independently of that, the handler's OWN absent-id branch is also wrong: `src/ops/get_usage_limit.rs` lines 152-160 answer ResourceNotFoundException with HTTP 404 and the emulator-authored message 'no usage limit with the id <id>', so even once the record does disappear the refusal still diverges in status (404 vs 400), error code (ResourceNotFoundException vs ValidationException) and message.
Why: a client that deletes a usage limit and reads it back to confirm removal is told the limit is still live and still enforcing a cap; a client that handles the documented refusal never sees it, and when it finally does, branches on the wrong status and error code.
Fix: the GetUsageLimit handler's absent-id branch (status, error code and message) plus the usage-limit lifecycle in the service state, so a removed limit is no longer resolvable.
Traces: redshiftserverless-workgroup-resource-management-and-error-semantics#task#5.

=== Cause 4 of 4: GetWorkgroup's workgroup-not-found refusal is sent with HTTP 404 where this service sends 400 ===
AWS: recorded behavior only for the status. The model declares GetWorkgroup's ResourceNotFoundException with @httpError 404 and ValidationException with 400, but the recorded refusal for a workgroupName that resolves to nothing (here, after the workgroup was deleted) carries HTTP 400. That is the same restatement this crate already documents for the sibling resource: `src/state.rs` lines 173-189 record that recorded Redshift Serverless traffic answers an unresolvable namespace NAME with HTTP 400 carrying the modeled error shape, 'the model's @httpError 404 is the error's documented status, not the one this service sends for a namespace name'. The same holds for a workgroup name. The replay flagged only the `status` key, so the error body the recording pins is already matched — the status alone is the divergence.
Emulator: answers HTTP 404 (`src/ops/get_workgroup.rs` lines 154-162 return ResourceNotFoundException with no status restatement), i.e. `status: expected 400, got 404`.
Why: clients and SDK retry/error-classification layers branch on the HTTP status before the error code; a 404 on a name lookup is treated as a different failure class (and by some HTTP intermediaries as a routing miss) than the 400 AWS sends, so correct not-found handling never runs.
Fix: the GetWorkgroup handler's not-found branch — restate the refusal status to 400 the way the namespace-name refusals in this crate already do, rather than emitting the model's documented 404.
Traces: redshiftserverless-workgroup-resource-management-and-error-semantics#teardown#202.

## Evidence
Fixture: redshiftserverless-workgroup-resource-management-and-error-semantics (trace set simparity-wire-aws-v1). All four steps replayed against a live, bootstrapped redshift-serverless emulator (health 200, data listener accepting).

Cause 1 — step task#1, CreateUsageLimit, verdict mismatch, status 200 -> 200:
  $.usageLimit.featureType: missing (expected 'serverless-compute', got None)
  $.usageLimit.limitType:   missing (expected 'compute-used', got None)
  (usageLimitArn not asserted at this step)

Cause 2 — step task#2, GetUsageLimit, verdict mismatch, status 200 -> 200:
  $.usageLimit.featureType: missing (expected 'serverless-compute', got None)
  $.usageLimit.limitType:   missing (expected 'compute-used', got None)
  $.usageLimit.usageLimitArn: value_mismatch
      expected 'arn:aws:redshift-serverless:<region>:<account>:usagelimit/<usageLimitId>'
      actual   'arn:aws:redshift-serverless:<region>:<account>:usage-limit/<usageLimitId>'
    (region/account/id in the recording are environment values; the divergence is the resource-type token 'usagelimit' vs 'usage-limit')
  Source: src/state.rs:213-220 `usage_limit_arn` formats "usage-limit/{usage_limit_id}".

Cause 3 — step task#5, GetUsageLimit, verdict mismatch, status 400 -> 200:
  status: expected 400, got 200
  $.__type:  missing (expected 'ValidationException', got None)
  $.message: missing (expected 'The usage limit ID is not valid. <usageLimitId> does not exist', got None)
    fixed text: 'The usage limit ID is not valid. ' + ' does not exist'; variable part: the usageLimitId from the request.
  Source: src/ops/get_usage_limit.rs:152-160 — absent-id branch returns ResourceNotFoundException (HTTP 404) with format!("no usage limit with the id {}", ...); and the record is still resolvable at this step at all (DeleteUsageLimit is a not_implemented scaffold, src/ops/delete_usage_limit.rs:158).

Cause 4 — step teardown#202, GetWorkgroup, verdict mismatch, status 400 -> 404:
  status: expected 400, got 404 (only key reported — the error body matched)
  Source: src/ops/get_workgroup.rs:154-162 returns ResourceNotFoundException at the model's 404; contrast src/state.rs:173-189, which records that this service answers an unresolvable namespace NAME with HTTP 400 carrying the same modeled shape.

## Steps to Reproduce
Setup (shared): CreateNamespace(namespaceName=<ns>, dbName="dev"), then CreateWorkgroup(workgroupName=<wg>, namespaceName=<ns>) and poll GetWorkgroup until status == AVAILABLE; take workgroupArn from the response.

Cause 1:
1. CreateUsageLimit(resourceArn=<workgroupArn>, usageType="serverless-compute", amount=60, period="daily").
2. Read the 200 response's `usageLimit`: `featureType` and `limitType` are absent. AWS returns featureType='serverless-compute' and limitType='compute-used'.

Cause 2:
3. GetUsageLimit(usageLimitId=<the id from step 1>).
4. Read `usageLimit.usageLimitArn`: the resource-type segment is 'usage-limit/' where AWS mints 'usagelimit/'. featureType/limitType are absent here too.

Cause 3:
5. DeleteUsageLimit(usageLimitId=<that id>) (today a 501 scaffold — filed separately), or otherwise make the id name no usage limit, e.g. GetUsageLimit with a fresh UUID.
6. GetUsageLimit(usageLimitId=<that id>): the emulator answers 200 with the limit when the record survived, and 404 ResourceNotFoundException ('no usage limit with the id <id>') when it does not. AWS answers 400 ValidationException 'The usage limit ID is not valid. <id> does not exist' in both cases.

Cause 4:
7. DeleteWorkgroup(workgroupName=<wg>) and wait for it to be gone (or call GetWorkgroup with any well-formed name, 3-64 chars matching ^[a-z0-9-]+$, that names no workgroup).
8. GetWorkgroup(workgroupName=<that name>): the emulator answers HTTP 404; the recorded service answers HTTP 400 with the same modeled error body.

Replay: redshiftserverless-workgroup-resource-management-and-error-semantics steps task#1, task#2, task#5, teardown#202.
