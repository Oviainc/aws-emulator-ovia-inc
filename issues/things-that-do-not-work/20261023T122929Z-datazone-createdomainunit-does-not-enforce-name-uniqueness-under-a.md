Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: datazone.domain-unit-concurrent-create-same-name-one-wins
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone.domain-unit-concurrent-create-same-name-one-wins|run#10|AssertionError|assert len(successes) == 1, f"expected exactly 1 winner, got {successes}"
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# CreateDomainUnit does not enforce name uniqueness under a parent: concurrent same-name creates all succeed instead of exactly one winner plus ConflictException losers

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainUnit

## Description
Cause 1 of 1: CreateDomainUnit never checks whether a domain unit with the requested `name` already exists under the requested `parentDomainUnitIdentifier`, so N creates of the same name under the same parent all succeed and the domain-unit hierarchy ends up holding N siblings with identical names.

AWS: a domain unit's name is unique among the children of one parent domain unit. A CreateDomainUnit whose (parentDomainUnitIdentifier, name) pair is already taken is refused with HTTP 409 `ConflictException`. Smithy (services/service-com-amazonaws-datazone-datazone/smithy-model/com-amazonaws-datazone-datazone.json): `CreateDomainUnit` models `ConflictException` (httpError 409, "There is a conflict while performing this action") alongside AccessDenied/Validation/ServiceQuotaExceeded/Throttling/InternalServer; `CreateDomainUnitInput.name` is `DomainUnitName` (@length 1..128, pattern ^[\w -]+$, @sensitive) and `parentDomainUnitIdentifier` is required. The model itself does not spell out the uniqueness scope, so for the collision scope this is recorded behavior only: the recorded run of 5 concurrent CreateDomainUnit calls with one shared name under the same root domain unit yields exactly one 201 and four 409 ConflictException responses, and ListDomainUnitsForParent then shows exactly one unit with that name. The loser's message is NOT a requirement — the recording shows it varies between a "Conflict with domainUnit <winner id>" form and a storage transaction-cancelled form depending on interleaving — only the 409 status and the `ConflictException` code are.

Emulator: every racer is accepted. The replay recorded 5 successful creates returning 5 distinct ids (e.g. ['mvky1bqd3', 'mvky1bqx4', 'mvky1br07', 'mvky1br05', 'mvky1br06']) and zero ConflictException responses; the check fails at `assert len(successes) == 1`. The handler validates the domain id, the domain status, the parent's existence and a per-domain unit-count cap, then mints an id and inserts unconditionally — there is no lookup of existing siblings by name. Because the whole handler runs under one store lock, the same gap applies to sequential duplicate creates, not just the concurrent case: a second same-name create is also accepted.

Why: clients treat domain-unit names as addressable, unique labels within a parent and rely on the 409 to detect that another actor already created the unit (the standard create-or-adopt pattern: attempt the create, catch ConflictException, then look the existing unit up). Against this emulator the conflict never arrives, so every racer believes it owns the unit, each holds a different id for the same logical node, and subsequent lookups by name are ambiguous — ListDomainUnitsForParent returns several identically named siblings and name-based resolution picks an arbitrary one. Idempotent provisioning (CDK/CloudFormation-style converge loops, retried deploys) silently creates duplicate hierarchy nodes instead of converging, and the duplicates are not reachable-by-name for cleanup.

Fix: the CreateDomainUnit handler's validation step, before id minting and insertion — after the existing domain-status and parent-existence checks, reject with the already-modeled `ConflictException` when the parent already has a child whose name equals the requested name. The uniqueness decision must be made under the same store lock that performs the insert so concurrent racers are serialized and exactly one wins. The sibling lookup belongs with the domain-unit hierarchy in the service's state module (where `ancestor_domain_unit_ids` and the per-domain `domain_units` map already live), since the same constraint is needed by UpdateDomainUnit renames.

Traces: datazone.domain-unit-concurrent-create-same-name-one-wins (handler: services/service-com-amazonaws-datazone-datazone/src/ops/create_domain_unit.rs, insert at lines 272-304 with no name-collision check among the checks at lines 222-270).

## Evidence
Check datazone.domain-unit-concurrent-create-same-name-one-wins, [mismatch].

Expected (AWS, recorded): 5 concurrent CreateDomainUnit(domainIdentifier=<domain>, name=<shared name>, parentDomainUnitIdentifier=<rootDomainUnitId>) -> exactly 1 success (HTTP 201) and 4 failures, every failure carrying error code `ConflictException` (HTTP 409); then ListDomainUnitsForParent(parentDomainUnitIdentifier=<rootDomainUnitId>) contains exactly one item with that name, and its id is the winner's id. (Loser message text varies and is not asserted.)

Actual (emulator): all 5 calls returned HTTP 201 with distinct ids and no error was raised:
  AssertionError: expected exactly 1 winner, got ['mvky1bqd3', 'mvky1bqx4', 'mvky1br07', 'mvky1br05', 'mvky1br06']

Code evidence: services/service-com-amazonaws-datazone-datazone/src/ops/create_domain_unit.rs `handle` checks, in order, the idempotency token replay (l.208), domain existence (l.222-239), domain status != AVAILABLE -> ConflictException (l.242-250), parent existence (l.251-257), per-domain unit cap -> ServiceQuotaExceededException (l.260-270), then mints an id and inserts (l.272-304). No step consults the existing children's names, so `ConflictException` is never produced for a duplicate name.

## Steps to Reproduce
1. Create an IAM role trusted by datazone.amazonaws.com and CreateDomain with it; poll GetDomain until status == AVAILABLE. Note rootDomainUnitId.
2. Pick one name matching ^[\w -]+$ (e.g. "sim2real-dz-race-1").
3. From 5 threads concurrently call CreateDomainUnit(domainIdentifier=<domain id>, name=<that same name>, parentDomainUnitIdentifier=<rootDomainUnitId>) with no clientToken.
4. Observe: all 5 calls return 201 with distinct `id` values. Expected exactly 1 success and 4 failures with error code ConflictException / HTTP 409.
5. Sequential variant (same gap, no concurrency needed): call CreateDomainUnit twice with the same name and parent. The second call is accepted (201, new id) instead of being refused with ConflictException.
6. (Blocked separately) ListDomainUnitsForParent would show the duplicate siblings, but that operation is an unimplemented stub in this emulator and is covered by its own filed issue.
