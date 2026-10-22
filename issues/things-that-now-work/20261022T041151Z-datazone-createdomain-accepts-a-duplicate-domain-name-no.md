Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: datazone.domain-duplicate-name-conflict-rejected datazone.project-cross-domain-isolation-not-found
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone.domain-duplicate-name-conflict-rejected|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: datazone.project-cross-domain-isolation-not-found|run#7|AssertionError|assert message == "Project not found", (
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# DataZone: CreateDomain accepts a duplicate domain name (no ConflictException), and GetProject's not-found message echoes the identifier

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Two distinct root causes in the DataZone emulator, both grounded in failing behavioral checks. This issue bundles 2 causes; both must be closed before submitting.

Cause 1 of 2: CreateDomain does not enforce domain-name uniqueness within the account.
AWS: CreateDomain (Smithy: POST /v2/domains, success 201, errors include ConflictException httpError 409 — "There is a conflict while performing this action", member message required). Recorded behavior: a second CreateDomain with a name already in use in the account is REJECTED with HTTP 409 ConflictException and the account is left with exactly one domain of that name; the recorded message is, as an example, "Domain name already exists under this account, please use another one". The requirement is the 409 / ConflictException refusal plus the invariant that no duplicate is created; the exact message text is an example.
Emulator: the second CreateDomain SUCCEEDS with HTTP 201 and mints a second, independent domain. Replay output: "expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'id': 'dzd_mvj0uqib3', 'name': 'sim2real-dz-dup-...', 'status': 'CREATING', ...}". The handler only short-circuits on the @idempotencyToken clientToken; there is no name-uniqueness check before the new Domain is inserted into the store.
Why: a client that relies on AWS's uniqueness guarantee (create-if-absent patterns, tests asserting a ConflictException, reconciliation loops that retry CreateDomain) silently creates duplicate domains against the emulator. Subsequent name-based lookups/ListDomains filtering become ambiguous, and error-handling paths for ConflictException are never exercised.
Fix: CreateDomain's handler/validation for this service — reject before insert when a live domain with the same name already exists in the account, returning the modeled ConflictException (409).
Traces: datazone.domain-duplicate-name-conflict-rejected.

Cause 2 of 2: ResourceNotFoundException messages for DataZone project lookups are built from the request identifier instead of the service's fixed resource-not-found text.
AWS: GetProject (Smithy: GET /v2/domains/{domainIdentifier}/projects/{identifier}, errors include ResourceNotFoundException httpError 404, member message required — the model does not state the text). Recorded behavior only: when the project is not found in the scoped domain (here, a project owned by domain A looked up under unrelated domain B), DataZone answers 404 ResourceNotFoundException with a terse resource-kind message that does NOT echo the requested id — "Project not found". The same rule shows up for sibling resources in this suite ("Account Pool not found"), so the rule is: the message names the resource kind only, never the caller-supplied identifier.
Emulator: status and error code are correct (ResourceNotFoundException), but the message is formatted from the request: "Project mvj0stme5 was not found" (and the domain branch likewise formats "Domain <id> was not found").
Why: clients and test suites that match DataZone's not-found text (or that treat echoed identifiers as a signal the resource exists but is inaccessible) behave differently against the emulator; cross-domain isolation assertions fail even though the isolation itself is correct.
Fix: GetProject's handler error construction (and the sibling not-found paths in the same service's handlers) — emit the service's fixed resource-kind not-found message rather than interpolating the requested identifier.
Traces: datazone.project-cross-domain-isolation-not-found.

## Evidence
Cause 1 — check datazone.domain-duplicate-name-conflict-rejected [mismatch]:
  expected: second CreateDomain with an in-use name -> HTTP 409, Error.Code=ConflictException, and exactly 1 domain with that name afterwards (recorded message example: "Domain name already exists under this account, please use another one").
  actual:   HTTP 201 and a NEW domain is returned — AssertionError: "expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'id': 'dzd_mvj0uqib3', 'rootDomainUnitId': 'mvj0uqib4', 'name': 'sim2real-dz-dup-a42edf7b1acb', 'status': 'CREATING', ...}".
  code path: services/service-com-amazonaws-datazone-datazone/src/ops/create_domain.rs lines 219-309 — only the clientToken idempotency lookup (lines 240-246) and the MAX_DOMAINS quota check (lines 251-261) guard the insert at line 303; no name-uniqueness check exists.

Cause 2 — check datazone.project-cross-domain-isolation-not-found [mismatch]:
  expected: Error.Code=ResourceNotFoundException (HTTP 404) with the resource-kind message "Project not found".
  actual:   Error.Code=ResourceNotFoundException with message "Project mvj0stme5 was not found" — AssertionError: "GetProject scoped to the unrelated domain 'dzd_mvj0spr53' must say 'Project not found', got 'Project mvj0stme5 was not found'".
  code path: services/service-com-amazonaws-datazone-datazone/src/ops/get_project.rs lines 181-194 — message built with format!("Project {} was not found", input.identifier) (and format!("Domain {} was not found", ...) on the domain branch).

## Steps to Reproduce
Cause 1 (datazone.domain-duplicate-name-conflict-rejected):
1. Create an IAM role usable as a DataZone domain execution role.
2. dz.create_domain(name=N, domainExecutionRole=<role arn>) and wait for the domain to reach AVAILABLE.
3. dz.create_domain(name=N, domainExecutionRole=<role arn>) again with the SAME name.
4. Observe: the call returns HTTP 201 with a new domain id. Expected: HTTP 409 ConflictException, and ListDomains shows exactly one domain named N.

Cause 2 (datazone.project-cross-domain-isolation-not-found):
1. Create two DataZone domains A and B.
2. Create a project P in domain A.
3. dz.get_project(domainIdentifier=B.id, identifier=P.id).
4. Observe: 404 ResourceNotFoundException with message "Project <P.id> was not found". Expected: ResourceNotFoundException with the resource-kind message "Project not found" (no identifier echoed).
