Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53-recovery-readiness.delete-cell-nonexistent-not-found route53-recovery-readiness.get-cell-readiness-summary-nonexistent-not-found route53-recovery-readiness.get-readiness-check-resource-status-nonexistent-check-not-found route53-recovery-readiness.readiness-check-status-starts-unknown route53-recovery-readiness.list-tags-for-resources-nonexistent-arn-not-found route53-recovery-readiness.tag-and-untag-resource-nonexistent-arn-rejected route53-recovery-readiness.list-rules-invalid-resource-type-filter-rejected route53-recovery-readiness.list-resource-sets-empty-account-well-formed-page route53-recovery-readiness.readiness-check-repoint-resource-set route53-recovery-readiness.get-readiness-check-nonexistent-not-found route53-recovery-readiness.list-readiness-checks-malformed-next-token-rejected
Issue-Shape-Id: com.amazonaws.route53recoveryreadiness#Route53RecoveryReadiness
Issue-Key: route53-recovery-readiness.delete-cell-nonexistent-not-found|run#4|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: route53-recovery-readiness.get-cell-readiness-summary-nonexistent-not-found|run#4|AssertionError|assert code == "ResourceNotFoundException", code
Issue-Key: route53-recovery-readiness.get-readiness-check-resource-status-nonexistent-check-not-found|run#6|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: route53-recovery-readiness.readiness-check-status-starts-unknown|run#6|AssertionError|assert first == "ResourceNotFoundException", first
Issue-Key: route53-recovery-readiness.list-tags-for-resources-nonexistent-arn-not-found|run#5|AssertionError|assert code == "NotFoundException", code
Issue-Key: route53-recovery-readiness.tag-and-untag-resource-nonexistent-arn-rejected|run#7|AssertionError|assert tag_code == "NotFoundException", f"expected NotFoundException, got {tag_code}"
Issue-Key: route53-recovery-readiness.list-rules-invalid-resource-type-filter-rejected|run#3|AssertionError|assert code == "ValidationException", code
Issue-Key: route53-recovery-readiness.list-resource-sets-empty-account-well-formed-page|_error#8|AssertionError|raise AssertionError(f"expected a ClientError, the call succeeded with {result!r}")
Issue-Key: route53-recovery-readiness.readiness-check-repoint-resource-set|_error#6|AssertionError|raise AssertionError(f"expected a ClientError, the call succeeded with {result!r}")
Issue-Key: route53-recovery-readiness.get-readiness-check-nonexistent-not-found|run#5|AssertionError|assert message == _NOT_FOUND_MESSAGE, message
Issue-Key: route53-recovery-readiness.list-readiness-checks-malformed-next-token-rejected|run#6|AssertionError|assert message == _BAD_TOKEN_MESSAGE, message
Issue-Commit: 6136e2766a973f12a1067e5538f7a2dced441244

# route53-recovery-readiness: 9 divergences — six not-found/tagging operations answer 501 NotImplemented, Create* is not account-gated, ListRules filter unvalidated, and two error messages diverge

> _Note: this issue was identified against the code base at commit `6136e2766a973f12a1067e5538f7a2dced441244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCell

## Description
Nine distinct root causes in the Route53 Recovery Readiness emulator, all grounded in failing behavioral checks of this one service. Every cause below is independent and must be closed before this issue is submitted (Cause 1 of 9 … Cause 9 of 9). Service identity: Smithy sdkId "Route53 Recovery Readiness", protocol restJson1; the per-operation modeled errors and HTTP codes quoted below come from this service's on-disk Smithy model (services/service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness/smithy-model/com-amazonaws-route53recoveryreadiness-route53recoveryreadiness.json).

NOTE ON ALREADY-FILED WORK: separate scaffold issues are already filed for UNIMPLEMENTED CreateCrossAccountAuthorization, DeleteCrossAccountAuthorization, ListCells, ListRecoveryGroups and ListRules. Causes 1–6 below are for operations that NO such issue names, and Cause 7 is a validation rule on ListRules that implementing the stub alone does not satisfy. Do not close any cause here by pointing at those scaffold issues.

=== Cause 1 of 9: DeleteCell answers 501 NotImplemented instead of 404 ResourceNotFoundException ===
AWS: Smithy DeleteCell — DELETE /cells/{CellName}, success 204, errors AccessDeniedException(403), ResourceNotFoundException(404, "The requested resource does not exist"), ThrottlingException(429), ValidationException(400). For a well-formed CellName that was never created AWS returns ResourceNotFoundException; recorded message (example, constant and request-independent in the recording): "Resource not found".
Emulator: the handler is a scaffold stub and answers HTTP 501 with error code NotImplemented for every request.
Why: a client probing for absence (or writing idempotent delete-if-exists logic) sees a 501 server-side failure instead of the modeled 404, so it retries or aborts instead of treating the cell as absent. 501/NotImplemented is also not in the operation's modeled error set, so SDK error handling cannot classify it.
Fix: implement the handler, services/service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness/src/ops/delete_cell.rs (the Err(Scaffolded::not_implemented("DeleteCell")) at the end of handle), resolving CellName against the service store and returning the modeled ResourceNotFoundException on a miss.
Traces: route53-recovery-readiness.delete-cell-nonexistent-not-found ("expected ResourceNotFoundException for a well-formed but never-created CellName, got NotImplemented"). The same check then independently re-confirms absence through GetCell, which it never reached.

=== Cause 2 of 9: GetCellReadinessSummary answers 501 NotImplemented instead of 404 ResourceNotFoundException ===
AWS: Smithy GetCellReadinessSummary — GET /cellreadiness/{CellName}, errors AccessDeniedException(403), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). An unknown CellName yields ResourceNotFoundException; recorded message example "Resource not found".
Emulator: scaffold stub, HTTP 501 / NotImplemented.
Why: readiness dashboards that poll a cell summary cannot distinguish "cell does not exist" from "service malfunction".
Fix: services/service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness/src/ops/get_cell_readiness_summary.rs.
Traces: route53-recovery-readiness.get-cell-readiness-summary-nonexistent-not-found ("AssertionError: NotImplemented" on `assert code == "ResourceNotFoundException"`).

=== Cause 3 of 9: GetReadinessCheckResourceStatus answers 501 NotImplemented instead of 404 ResourceNotFoundException ===
AWS: Smithy GetReadinessCheckResourceStatus — GET /readinesschecks/{ReadinessCheckName}/resource/{ResourceIdentifier}/status, errors AccessDeniedException(403), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). For a never-created ReadinessCheckName (with a well-formed ResourceIdentifier) AWS returns ResourceNotFoundException; recorded message example "Resource not found".
Emulator: scaffold stub, HTTP 501 / NotImplemented.
Why: per-resource readiness polling gets an unmodeled 501 for a missing check and cannot tell a typo'd check name from an outage.
Fix: services/service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness/src/ops/get_readiness_check_resource_status.rs.
Traces: route53-recovery-readiness.get-readiness-check-resource-status-nonexistent-check-not-found ("expected ResourceNotFoundException for a never-created check with ResourceIdentifier=…, got NotImplemented").

=== Cause 4 of 9: GetReadinessCheckStatus answers 501 NotImplemented instead of 404 ResourceNotFoundException ===
AWS: Smithy GetReadinessCheckStatus — GET /readinesschecks/{ReadinessCheckName}/status, errors AccessDeniedException(403), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). An unknown ReadinessCheckName yields ResourceNotFoundException on EVERY call (the check asserts it twice in a row, so the behavior must be stable/repeatable, not first-call-only); recorded message example "Resource not found".
Emulator: scaffold stub, HTTP 501 / NotImplemented.
Why: callers that poll readiness status cannot detect a deleted/absent check and will treat a 501 as a transient server fault forever.
Fix: services/service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness/src/ops/get_readiness_check_status.rs.
Traces: route53-recovery-readiness.readiness-check-status-starts-unknown ("AssertionError: NotImplemented" on `assert first == "ResourceNotFoundException"`).

=== Cause 5 of 9: ListTagsForResources answers 501 NotImplemented instead of a 404 not-found for an unknown ARN ===
AWS: Smithy ListTagsForResources — GET /tags/{ResourceArn}, modeled error ResourceNotFoundException (plus the service-common InternalServerException/ValidationException). Recorded behavior for a well-formed but never-created resource-set ARN: a 404 whose wire error code is "NotFoundException" (the tagging APIs of this service use NotFoundException, NOT the ResourceNotFoundException used by the resource-specific Get/Delete operations), recorded message example "Resource was not found". The recorded code is the requirement; where it differs from the model name, follow the recording.
Emulator: scaffold stub, HTTP 501 / NotImplemented.
Why: tag-reading tooling (cost/inventory sweeps) cannot distinguish a stale ARN from a broken service, and a 501 is not in the operation's error set.
Fix: services/service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness/src/ops/list_tags_for_resources.rs — resolve ResourceArn through the store's ARN index and emit the not-found error on a miss.
Traces: route53-recovery-readiness.list-tags-for-resources-nonexistent-arn-not-found ("AssertionError: NotImplemented" on `assert code == "NotFoundException"`).

=== Cause 6 of 9: TagResource (and UntagResource) answer 501 NotImplemented instead of rejecting an unknown ARN ===
AWS: Smithy TagResource — POST /tags/{ResourceArn}, modeled error ResourceNotFoundException; UntagResource — DELETE /tags/{ResourceArn}, success 204, modeled error ResourceNotFoundException. Recorded behavior for a well-formed but never-created resource-set ARN: both are rejected with wire error code "NotFoundException" (404), recorded message example "Resource was not found". A tag must not be attachable to, or removable from, a resource that was never created.
Emulator: TagResource is a scaffold stub answering HTTP 501 / NotImplemented; UntagResource was never reached by the check and must be fixed with it (same handler family, same stub pattern).
Why: this is an overly-permissive/unclassifiable write path — a client cannot learn that its tag target does not exist, and once implemented naively it could silently accept tags for ghost ARNs.
Fix: services/service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness/src/ops/tag_resource.rs and .../src/ops/untag_resource.rs — look ResourceArn up in the store's arn_index (see state.rs ResourceKey) before mutating.
Traces: route53-recovery-readiness.tag-and-untag-resource-nonexistent-arn-rejected ("expected NotFoundException, got NotImplemented").

=== Cause 7 of 9: ListRules does not reject an unknown ResourceType filter with ValidationException ===
AWS: Smithy ListRules — GET /rules, errors AccessDeniedException(403), ThrottlingException(429), plus the service-common ValidationException(400). Recorded behavior: ListRules(ResourceType="not_a_valid_type") is rejected with ValidationException (400) — an unsupported resource-type filter is refused, it does not return an empty rule list; and a supported filter (e.g. ResourceType="AWS::SQS::Queue") returns only rules whose ResourceType equals that value.
Emulator: the operation is a scaffold stub answering HTTP 501 / NotImplemented, so neither the rejection nor the filter is exercised.
Why: callers that enumerate the built-in readiness-rule catalog by type cannot tell a bad filter value from a working query, and a filter that is honored for valid types but silently ignored for invalid ones would hand back the whole catalog.
Fix: services/service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness/src/ops/list_rules.rs — the handler plus its ResourceType validation. A separate already-filed scaffold issue names the same file for the bare "ListRules is unimplemented" fact (grounded in route53-recovery-readiness.list-rules-returns-builtin-types); THIS cause additionally requires the invalid-ResourceType rejection, which implementing the stub does not imply.
Traces: route53-recovery-readiness.list-rules-invalid-resource-type-filter-rejected ("AssertionError: NotImplemented" on `assert code == "ValidationException"`).

=== Cause 8 of 9: Create* readiness resources succeed where AWS denies them account-wide (overly permissive) ===
AWS: recorded behavior only (the Smithy model does not express the account-eligibility gate, it only lists AccessDeniedException(403) on CreateCell/CreateReadinessCheck/CreateRecoveryGroup/CreateResourceSet). Recorded: the ARC readiness-check feature is closed to new accounts, so a syntactically valid CreateResourceSet / CreateReadinessCheck / CreateCell / CreateRecoveryGroup request passes input validation and is then DENIED at the account-eligibility gate with HTTP 403; the recorded error code is "AccessDeniedException" on CreateResourceSet and "AccessDenied" on CreateReadinessCheck, with recorded message example "This operation is currently unavailable for new accounts." No resource is created: a subsequent ListResourceSets still returns [] and GetReadinessCheck/UpdateReadinessCheck for that name return ResourceNotFoundException.
Emulator: the create handlers are fully implemented and store-backed, so they return HTTP 200 with a minted ARN and persist the record. Evidence from replay: CreateResourceSet returned 200 with ResourceSetArn arn:aws:route53-recovery-readiness::<acct>:resource-set/<name>; CreateReadinessCheck returned 200 with ReadinessCheckArn …:readiness-check/<name>.
Why: this is the overly-permissive class — a client (or a conformance test) that expects the current AWS denial gate instead gets a resource that exists only in the emulator, so code paths handling the denial are never exercised, and downstream list/get assertions that must stay empty are polluted.
Fix: the account-eligibility gate belongs ahead of the store writes in each create handler: .../src/ops/create_resource_set.rs, .../src/ops/create_readiness_check.rs, .../src/ops/create_cell.rs, .../src/ops/create_recovery_group.rs (ordering matters — input validation first, then the gate, per the recording), with the error classes quoted above. Only the status and error code are requirements; the quoted message is the recorded example.
Traces: route53-recovery-readiness.list-resource-sets-empty-account-well-formed-page ("expected a ClientError, the call succeeded with {…'HTTPStatusCode': 200…'ResourceSetArn': 'arn:aws:route53-recovery-readiness::…:resource-set/…'}"), route53-recovery-readiness.readiness-check-repoint-resource-set ("expected a ClientError, the call succeeded with {…'HTTPStatusCode': 200…'ReadinessCheckArn': …}"). The same gate is documented by route53-recovery-readiness.create-operations-blocked-for-new-account, which is blocked by an unrelated already-filed stub.

=== Cause 9 of 9: not-found and invalid-pagination-token error TEXT diverges from this service's constant messages (LOW) ===
AWS: recorded behavior only. This service's not-found responses carry a fixed, request-independent message "Resource not found" — recorded consistently across DeleteCell, GetCell, GetCellReadinessSummary, GetReadinessCheck, GetReadinessCheckStatus, GetReadinessCheckResourceStatus and GetRecoveryGroupReadinessSummary — and ListReadinessChecks with a malformed NextToken carries the fixed message "Invalid pagination token." The RULE here is "a constant, resource-agnostic text", which is what diverges; the exact strings are the recorded examples of it.
Emulator: GetReadinessCheck returns the correct code (ResourceNotFoundException) but a synthesized per-resource message: "Readiness check sim2real_ghost_check_9afcd64c320e not found." ListReadinessChecks returns the correct code (ValidationException) but an AWS-style constraint message built from the request: "1 validation error detected: Value 'not-a-valid-token!!' at 'nextToken' failed to satisfy constraint: Member must be a pagination token returned by a previous call". Status and error code are correct in both cases — this cause is TEXT only, which is why it is LOW; it is recorded here because the same convention must be followed by the handlers written for Causes 1–6.
Why: a client that keys on the message (log matchers, message-equality conformance assertions) sees a different string, and the emulator additionally leaks the requested resource name into a message AWS keeps constant.
Fix: centralize the not-found and invalid-token message constants in services/service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness/src/state.rs and use them from .../src/ops/get_readiness_check.rs and .../src/ops/list_readiness_checks.rs (and from the handlers implemented for Causes 1–6).
Traces: route53-recovery-readiness.get-readiness-check-nonexistent-not-found ("AssertionError: Readiness check sim2real_ghost_check_9afcd64c320e not found."), route53-recovery-readiness.list-readiness-checks-malformed-next-token-rejected ("AssertionError: 1 validation error detected: Value 'not-a-valid-token!!' at 'nextToken' …").

## Evidence
Suite: simparity-behavioral-aws-v1, route53-recovery-readiness 0/21 passed. The 11 mismatched checks below are the grounding (the 10 errored checks were all marked ALREADY FILED against unimplemented CreateCrossAccountAuthorization / DeleteCrossAccountAuthorization / ListCells / ListRecoveryGroups / ListRules and are not re-filed here).

Cause 1 — delete-cell-nonexistent-not-found:
  expected: ResourceNotFoundException (HTTP 404), message example "Resource not found"
  actual:   NotImplemented (HTTP 501) — "expected ResourceNotFoundException for a well-formed but never-created CellName, got NotImplemented"

Cause 2 — get-cell-readiness-summary-nonexistent-not-found:
  expected: ResourceNotFoundException (404)
  actual:   NotImplemented (501) — AssertionError: NotImplemented

Cause 3 — get-readiness-check-resource-status-nonexistent-check-not-found:
  expected: ResourceNotFoundException (404)
  actual:   NotImplemented (501) — "expected ResourceNotFoundException for a never-created check with ResourceIdentifier='arn:aws:sqs:us-west-2:<acct>:<queue>', got NotImplemented"

Cause 4 — readiness-check-status-starts-unknown:
  expected: ResourceNotFoundException (404) on two consecutive calls
  actual:   NotImplemented (501) — AssertionError: NotImplemented

Cause 5 — list-tags-for-resources-nonexistent-arn-not-found:
  expected: NotFoundException (404), message example "Resource was not found"
  actual:   NotImplemented (501) — AssertionError: NotImplemented

Cause 6 — tag-and-untag-resource-nonexistent-arn-rejected:
  expected: TagResource and UntagResource both NotFoundException (404)
  actual:   NotImplemented (501) — "expected NotFoundException, got NotImplemented"

Cause 7 — list-rules-invalid-resource-type-filter-rejected:
  expected: ValidationException (400) for ResourceType='not_a_valid_type', and a supported filter returning only matching rules
  actual:   NotImplemented (501) — AssertionError: NotImplemented

Cause 8 — list-resource-sets-empty-account-well-formed-page:
  expected: CreateResourceSet denied with AccessDeniedException (403), message example "This operation is currently unavailable for new accounts.", and ListResourceSets still []
  actual:   HTTP 200 with a persisted record — "expected a ClientError, the call succeeded with {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'ResourceSetArn': 'arn:aws:route53-recovery-readiness::<acct>:resource-set/<name>', 'ResourceSetName': '<name>', 'ResourceSetType': 'AWS::SQS::Queue', 'Resources': [...]}"
Cause 8 — readiness-check-repoint-resource-set:
  expected: CreateReadinessCheck denied with code AccessDenied (403), then GetReadinessCheck/UpdateReadinessCheck -> ResourceNotFoundException
  actual:   HTTP 200 — "expected a ClientError, the call succeeded with {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'ReadinessCheckArn': 'arn:aws:route53-recovery-readiness::<acct>:readiness-check/<name>', 'ReadinessCheckName': '<name>', 'ResourceSet': '<rs>'}"

Cause 9 — get-readiness-check-nonexistent-not-found (code OK, text only):
  expected message: constant "Resource not found"
  actual message:   "Readiness check sim2real_ghost_check_9afcd64c320e not found."
Cause 9 — list-readiness-checks-malformed-next-token-rejected (code OK, text only):
  expected message: constant "Invalid pagination token."
  actual message:   "1 validation error detected: Value 'not-a-valid-token!!' at 'nextToken' failed to satisfy constraint: Member must be a pagination token returned by a previous call"

Source confirmation in the checkout: services/.../src/ops/delete_cell.rs still carries the "//! SCAFFOLDED, NOT IMPLEMENTED" marker and ends in Err(crate::scaffolded::Scaffolded::not_implemented("DeleteCell")); services/.../src/ops/create_resource_set.rs is fully implemented and writes to crate::state (no account-eligibility gate anywhere in handle).

## Steps to Reproduce
Boot the emulator from this checkout and point a botocore client at it with region us-west-2 (service name 'route53-recovery-readiness'), then:

Cause 1: client.delete_cell(CellName='ghost_cell_1')  -> expect ResourceNotFoundException (404); observe 501 NotImplemented. Repeat with client.get_cell(CellName='ghost_cell_1').
Cause 2: client.get_cell_readiness_summary(CellName='ghost_cell_1') -> expect ResourceNotFoundException (404).
Cause 3: client.get_readiness_check_resource_status(ReadinessCheckName='ghost_check_1', ResourceIdentifier='arn:aws:sqs:us-west-2:000000000000:q1') -> expect ResourceNotFoundException (404).
Cause 4: client.get_readiness_check_status(ReadinessCheckName='ghost_check_1') twice -> expect ResourceNotFoundException (404) both times.
Cause 5: client.list_tags_for_resources(ResourceArn='arn:aws:route53-recovery-readiness::000000000000:resource-set/ghost_rs_1') -> expect NotFoundException (404).
Cause 6: client.tag_resource(ResourceArn=<that ghost ARN>, Tags={'k':'v'}) and client.untag_resource(ResourceArn=<that ghost ARN>, TagKeys=['k']) -> expect NotFoundException (404) for both.
Cause 7: client.list_rules(ResourceType='not_a_valid_type') -> expect ValidationException (400); then client.list_rules(ResourceType='AWS::SQS::Queue') -> expect a non-empty list whose every rule has ResourceType == 'AWS::SQS::Queue'.
Cause 8: client.create_resource_set(ResourceSetName='rs_1', ResourceSetType='AWS::SQS::Queue', Resources=[{'ResourceArn':'arn:aws:sqs:us-west-2:000000000000:q1'}]) -> expect AccessDeniedException (403) and ListResourceSets still []; observe HTTP 200 and a persisted resource set. Same for client.create_readiness_check(ReadinessCheckName='c_1', ResourceSetName='rs_1') -> expect code AccessDenied (403).
Cause 9: client.get_readiness_check(ReadinessCheckName='ghost_check_1') -> code is right, message must be the constant "Resource not found"; client.list_readiness_checks(NextToken='not-a-valid-token!!') -> code is right, message must be the constant "Invalid pagination token."

Or simply re-replay the 11 checks named in trace_ids.
