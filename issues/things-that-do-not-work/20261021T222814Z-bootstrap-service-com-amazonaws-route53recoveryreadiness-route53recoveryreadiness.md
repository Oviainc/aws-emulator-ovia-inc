Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 85378260976efce74d968b045ae62c2fe2a03011

# Bootstrap the Route53 Recovery Readiness emulator

_Filed: 2026-10-21T22:28:14Z_
_Operation: service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness | Class: stub_

> _Note: this issue was identified against the code base at commit `85378260976efce74d968b045ae62c2fe2a03011`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 32 operations in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, and the service holds no state. Nothing works end-to-end.

This issue scopes the FIRST buildable slice: the resource-set / readiness-check core loop, which is the primary workflow of Route 53 Application Recovery Controller readiness (a readiness check watches a named resource set). Implement these 6 operations, in this order, statefully:

1. **CreateResourceSet** — `POST /resourcesets`. Input: `ResourceSetName` (required), `ResourceSetType` (required, must match `^AWS::[A-Za-z0-9]+::[A-Za-z0-9]+$`), `Resources` (required, list<Resource> with `ResourceArn`, `ComponentId`, `ReadinessScopes`, `DnsTargetResource`), `Tags`. Store a new resource set keyed by name; mint `ResourceSetArn` as `arn:aws:route53-recovery-readiness::<account-id>:resource-set/<ResourceSetName>`. Return `ResourceSetArn`, `ResourceSetName`, `ResourceSetType`, `Resources`, `Tags` read back from the stored record. A second create with the same name must fail `ConflictException` (HTTP 409).
2. **CreateReadinessCheck** — `POST /readinesschecks`. Input: `ReadinessCheckName` (required), `ResourceSetName` (required), `Tags`. Store the check keyed by name with the resource-set name it points at; mint `ReadinessCheckArn` as `arn:aws:route53-recovery-readiness::<account-id>:readiness-check/<ReadinessCheckName>`. Output: `ReadinessCheckArn`, `ReadinessCheckName`, `ResourceSet` (the stored resource-set name), `Tags`. Duplicate name -> `ConflictException` (409).
3. **GetReadinessCheck** — `GET /readinesschecks/{ReadinessCheckName}`. Look the check up in state and return exactly what was created (`ReadinessCheckArn`, `ReadinessCheckName`, `ResourceSet`, `Tags`). Unknown name -> `ResourceNotFoundException` (HTTP 404).
4. **GetResourceSet** — `GET /resourcesets/{ResourceSetName}`. Return the stored `ResourceSetArn`, `ResourceSetName`, `ResourceSetType`, `Resources` (every member of every `Resource` preserved as submitted), `Tags`. Unknown name -> `ResourceNotFoundException` (404).
5. **ListReadinessChecks** — `GET /readinesschecks`. Return `ReadinessChecks` as a list of `ReadinessCheckOutput` (`ReadinessCheckArn`, `ReadinessCheckName`, `ResourceSet`, `Tags`) built from the stored map — every created check present, deleted ones absent. Honour `maxResults` (1..1000) by truncating and emitting an opaque `NextToken` that the next call resumes from; omit `NextToken` on the last page. (`ListResourceSets` should follow the same shape with `ResourceSetOutput`.)
6. **DeleteReadinessCheck** / **DeleteResourceSet** — `DELETE /readinesschecks/{ReadinessCheckName}` -> 204 and `DELETE /resourcesets/{ResourceSetName}` -> 204. Remove the record from state so a subsequent Get returns 404 and List no longer shows it. Deleting an unknown name -> `ResourceNotFoundException` (404).

NEVER STUBS: each of these must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the input back without storing it, no minting an ARN that is never saved. A create followed by a get/list in a later request must return what was created.

Name validation applies on the way in: `ReadinessCheckName`/`ResourceSetName` must satisfy `^\A[a-zA-Z0-9_]+\z$` and length 0..64, and `ResourceSetType` must satisfy its pattern; violations are `ValidationException` (HTTP 400).

## State Model

Store in `src/state.rs` (`ComAmazonawsRoute53recoveryreadinessRoute53recoveryreadinessState`), all behind the existing lock:

- `resource_sets: BTreeMap<String /*ResourceSetName*/, ResourceSetRecord>` where `ResourceSetRecord { arn: String, name: String, resource_set_type: String, resources: Vec<ResourceRecord>, tags: BTreeMap<String,String> }`
- `ResourceRecord { resource_arn: Option<String>, component_id: Option<String>, readiness_scopes: Vec<String>, dns_target_resource: Option<DnsTargetResourceRecord> }`
- `readiness_checks: BTreeMap<String /*ReadinessCheckName*/, ReadinessCheckRecord>` where `ReadinessCheckRecord { arn: String, name: String, resource_set_name: String, tags: BTreeMap<String,String> }`
- `arn_index: BTreeMap<String /*arn*/, ResourceKey>` so the later tag operations and ARN-addressed lookups can resolve an ARN back to the owning record (`ResourceKey::ResourceSet(name) | ResourceKey::ReadinessCheck(name) | ...`).

Use `BTreeMap` so list ordering is deterministic across calls, and derive pagination tokens from the key order.

Out of scope for this first slice (stay 501, to be filed as individual gap issues): Cells and RecoveryGroups (Create/Get/Update/Delete/List), the readiness-status and summary operations (`GetReadinessCheckStatus`, `GetReadinessCheckResourceStatus`, `GetCellReadinessSummary`, `GetRecoveryGroupReadinessSummary`), `GetArchitectureRecommendations`, `ListRules`, cross-account authorizations, the update operations, and `TagResource`/`UntagResource`/`ListTagsForResources`.

## Evidence

OPERATIONS.md (crate root) documents all 32 operations with their routes, input/output members and errors: CreateResourceSet `POST /resourcesets` (`ResourceSetName`/`ResourceSetType`/`Resources` required), CreateReadinessCheck `POST /readinesschecks` (`ReadinessCheckName`/`ResourceSetName` required), GetReadinessCheck `GET /readinesschecks/{ReadinessCheckName}` and GetResourceSet `GET /resourcesets/{ResourceSetName}` (both declare `ResourceNotFoundException` HTTP 404), ListReadinessChecks `GET /readinesschecks` (@paginated inputToken=NextToken, outputToken=NextToken, items=ReadinessChecks, `maxResults` range 1..1000), DeleteReadinessCheck/DeleteResourceSet `DELETE ... -> 204`. Creates declare `ConflictException` HTTP 409; all declare `ValidationException` HTTP 400. Smithy shapes `ResourceSetOutput`, `ReadinessCheckOutput` and `Resource` give the exact fields the state model must persist (`resourceSetArn`, `resourceSetType`, `resources`, `readinessCheckArn`, `resourceSet`, `tags`; Resource: `resourceArn`, `componentId`, `readinessScopes`, `dnsTargetResource`). Protocol is `restJson1`, signing name `route53-recovery-readiness` (README.md + OPERATIONS.md 'Wire protocol'). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 32 handler files, so there is no behavior to be wrong yet — only no behavior at all. No tests exist beyond the generated `tests/state_roundtrip.rs` state-API example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `route53-recovery-readiness` at it.
2. CreateResourceSet with ResourceSetName=my_set, ResourceSetType=AWS::AutoScaling::AutoScalingGroup, Resources=[{ResourceArn: arn:aws:autoscaling:us-east-1:111122223333:autoScalingGroup:uuid:autoScalingGroupName/asg-1, ReadinessScopes: []}], Tags={env: test} -> expect 200 with a stable ResourceSetArn; today: HTTP 501 NotImplemented.
3. CreateReadinessCheck with ReadinessCheckName=my_check, ResourceSetName=my_set -> expect 200 with ReadinessCheckArn and ResourceSet=my_set.
4. GetReadinessCheck ReadinessCheckName=my_check -> expect 200 returning the same ARN, name, ResourceSet=my_set and Tags that were stored at create time.
5. GetResourceSet ResourceSetName=my_set -> expect 200 with ResourceSetType and the full Resources list exactly as submitted.
6. ListReadinessChecks -> expect my_check present in ReadinessChecks; then ListReadinessChecks with maxResults=1 against two created checks -> expect 1 item plus a NextToken, and the second page returns the remaining item with no NextToken.
7. CreateReadinessCheck again with ReadinessCheckName=my_check -> expect ConflictException (HTTP 409).
8. DeleteReadinessCheck my_check -> expect 204; GetReadinessCheck my_check -> expect ResourceNotFoundException (HTTP 404); ListReadinessChecks -> my_check absent.
9. DeleteResourceSet my_set -> expect 204; GetResourceSet my_set -> expect ResourceNotFoundException (HTTP 404).
10. CreateResourceSet with ResourceSetName="bad name!" -> expect ValidationException (HTTP 400) for the ^\A[a-zA-Z0-9_]+\z$ pattern.
