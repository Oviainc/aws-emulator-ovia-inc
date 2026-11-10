Issue-Source: trace-sweep-divergence
Issue-Trace: simpledbv2-export-domain-to-s3-succeeds
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: simpledbv2-export-domain-to-s3-succeeds
Issue-Shape-Id: com.amazonaws.simpledbv2#SimpleDBv2
Issue-Key: sdb|CreateDomain|2xx->4xx|status
Issue-Key: sdb|PutAttributes|2xx->4xx|status
Issue-Key: sdb|StartDomainExport|2xx->4xx|$.clientToken
Issue-Key: sdb|StartDomainExport|2xx->4xx|$.exportArn
Issue-Key: sdb|StartDomainExport|2xx->4xx|$.requestedAt
Issue-Key: sdb|StartDomainExport|2xx->4xx|status
Issue-Key: sdb|ListExports|2xx->4xx|$.exportSummaries
Issue-Key: sdb|ListExports|2xx->4xx|status
Issue-Key: sdb|DeleteDomain|2xx->4xx|status
Issue-Commit: cad593586d705c3641fe11f59f14efd9baecb330

# Wire replay: domain-export lifecycle fixture refused end-to-end on sdb

> _Note: this issue was identified against the code base at commit `cad593586d705c3641fe11f59f14efd9baecb330`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Replaying the recorded success fixture for the SimpleDB domain-export-to-S3 lifecycle (`simpledbv2-export-domain-to-s3-succeeds`) diverges at EVERY one of its five recorded sdb steps: each recorded a 200 and the emulator answered 400. The shared fault is that the `sdb` endpoint serves only the three restJson1 `/v2/*` export operations and nothing of the Amazon SimpleDB 2009-04-15 AWS Query API that creates, writes and deletes the domain those export operations resolve `domainName` against. The two export steps then fail as a consequence, because no domain a wire client can create ever enters the registry. This issue bundles FIVE causes; all five must be closed before submitting.

Cause 1 of 5: CreateDomain on the 2009-04-15 query wire is not served (fixture simpledbv2-export-domain-to-s3-succeeds, step setup#1).
  AWS: the `sdb` endpoint (Smithy: com.amazonaws.simpledbv2#SimpleDBv2, aws.api#service endpointPrefix/arnNamespace `sdb`, aws.auth#sigv4 name `sdb`) serves, alongside the v2 `/v2/*` export operations, the Amazon SimpleDB 2009-04-15 AWS Query API on "/": a form-encoded POST carrying `Action=CreateDomain&DomainName=<name>&Version=2009-04-15`. It answers HTTP 200 with `<CreateDomainResponse xmlns="http://sdb.amazonaws.com/doc/2009-04-15/"><ResponseMetadata><RequestId>..</RequestId><BoxUsage>..</BoxUsage></ResponseMetadata></CreateDomainResponse>` and is idempotent on a name that already exists. The repo carries no Smithy model for the 2009-04-15 surface, so for that half this is recorded behavior only; the v2 model is cited for the shared wire identity. The recorded step shows status 200.
  Emulator: the request reaches the simpledbv2 service (its resolved route key equals the signing name `sdb`), names no modeled operation, and falls through to the protocol-default routing error: HTTP 400 with error code `InvalidAction`. Its message is the framework's `unknown action: <Action>` — fixed text "unknown action: " plus the request's `Action` value — not AWS's InvalidAction wording "The action <name> is not valid for this web service."
  Why: a client doing what AWS requires — create a domain, write items, export it — is refused at its first call with a code telling it the action does not exist on this service, which is false. `InvalidAction` is not a modeled error of any SimpleDB action, so SDK error branching cannot handle it, and the domain registry can then only be populated out of band (SIMPLEDB_EXISTING_DOMAINS), which no AWS client can set.
  Fix: the simpledbv2 service's request-claiming/operation-resolution seam plus new handlers, serializer and state for the 2009-04-15 query surface — route a form-encoded POST on "/" by its `Action` field, and hold created domains in this service's own state so the export operations resolve a just-created domain.
  Traces: simpledbv2-export-domain-to-s3-succeeds#setup#1.

Cause 2 of 5: PutAttributes on the 2009-04-15 query wire is not served (fixture simpledbv2-export-domain-to-s3-succeeds, step setup#2).
  AWS: `Action=PutAttributes` with `DomainName`, `ItemName` and indexed `Attribute.N.Name`/`Attribute.N.Value` parameters answers HTTP 200 with `<PutAttributesResponse xmlns="http://sdb.amazonaws.com/doc/2009-04-15/">` carrying only `ResponseMetadata`; the item it writes is part of what a later export of that domain reports (itemsCount). Recorded behavior only (no model for the 2009-04-15 surface in this repo); the recorded step shows status 200.
  Emulator: HTTP 400, error code `InvalidAction`, message "unknown action: PutAttributes" (fixed text "unknown action: " plus the request's `Action` value), from the same routing-error fallthrough as Cause 1.
  Why: a client cannot put any data into a domain, so even if the export surface worked the exported content is always empty and itemsCount can never be anything but 0 — the lifecycle this service exists to emulate is unobservable.
  Fix: as Cause 1 — a PutAttributes handler on the query surface plus item storage in this service's state.
  Traces: simpledbv2-export-domain-to-s3-succeeds#setup#2.

Cause 3 of 5: StartDomainExport refuses a domain the client just created, instead of accepting the export (fixture simpledbv2-export-domain-to-s3-succeeds, step task#4).
  AWS: Smithy: StartDomainExport POST /v2/StartDomainExport, required input members `domainName` (@length min 1) and `s3Bucket` (@length 3..255, pattern ^[a-z0-9A-Z]+[\.\-\w]*[a-z0-9A-Z]+$), optional `clientToken` (@idempotencyToken), `s3KeyPrefix`, `s3SseAlgorithm`, `s3SseKmsKeyId`, `s3BucketOwner`. On success it answers HTTP 200 with all three required output members present: `clientToken`, `exportArn`, `requestedAt`. Its modeled error `NoSuchDomainException` (client, httpError 400, doc "The specified domain does not exist.") is the answer only for a domain name that was never created. The recorded step shows status 200 with clientToken/exportArn/requestedAt present.
  Emulator: HTTP 400 with no `clientToken`, `exportArn` or `requestedAt` in the body. Hypothesis (grounded in services/service-com-amazonaws-simpledbv2-simpledbv2/src/state.rs:28-48,296-307): the handler resolves `domainName` against a registry fed only by the SIMPLEDB_EXISTING_DOMAINS environment variable plus the domains of already-stored exports, so the domain created in setup#1 is absent and the call is refused with the modeled NoSuchDomainException, message "The specified domain does not exist." (fixed text, echoing no request value).
  Why: the client holds a domain it legitimately created and is told it does not exist; no export can ever be started for a client-created domain, so the whole export lifecycle (PENDING -> IN_PROGRESS -> SUCCEEDED, clientToken idempotency, manifest reporting) is unreachable over the wire.
  Fix: the StartDomainExport handler's domain resolution and this service's state — the registry a `domainName` is resolved against must include domains created on the 2009-04-15 wire, not only operator-declared ones.
  Traces: simpledbv2-export-domain-to-s3-succeeds#task#4.

Cause 4 of 5: ListExports refuses the enumeration of a client-created domain's exports instead of returning a page (fixture simpledbv2-export-domain-to-s3-succeeds, step task#6).
  AWS: Smithy: ListExports POST /v2/ListExports, optional input `domainName` (@length min 1), `maxResults` (@range min 1), `nextToken` (@length 1..2048); output has required `exportSummaries` (list, @length min 0) and optional `nextToken`; modeled errors InvalidNextTokenException / InvalidParameterValueException / NoSuchDomainException, all client/400. A successful call answers HTTP 200 with `exportSummaries` present — possibly empty, never absent — each summary carrying `domainName`, `exportArn`, `exportStatus` and `requestedAt`. The recorded step shows status 200 and one summary for the exported domain with exportStatus SUCCEEDED.
  Emulator: HTTP 400 with `exportSummaries` absent from the body. Hypothesis (same registry defect as Cause 3, state.rs:300-307): the `domainName` filter is resolved against the same registry and refused with NoSuchDomainException, message "The specified domain does not exist."
  Why: a client cannot enumerate its own exports: a filter on a domain it created is rejected as nonexistent, and because `exportSummaries` is a required output member its absence also breaks SDK response deserialization rather than merely returning an empty page.
  Fix: the ListExports handler's domain-filter resolution, against the same shared domain registry named in Cause 3.
  Traces: simpledbv2-export-domain-to-s3-succeeds#task#6.

Cause 5 of 5: DeleteDomain on the 2009-04-15 query wire is not served (fixture simpledbv2-export-domain-to-s3-succeeds, step teardown#9).
  AWS: `Action=DeleteDomain&DomainName=<name>` answers HTTP 200 with `<DeleteDomainResponse xmlns="http://sdb.amazonaws.com/doc/2009-04-15/">` carrying only `ResponseMetadata`, and is a 200 no-op on a domain that does not exist. Recorded behavior only; the recorded step shows status 200.
  Emulator: HTTP 400, error code `InvalidAction`, message "unknown action: DeleteDomain" (fixed text plus the request's `Action` value), from the routing-error fallthrough.
  Why: a client cannot clean up; a test or script that tears down its own domains fails on teardown and leaks state, and the refusal's code wrongly tells the client the action is not part of this service.
  Fix: as Cause 1 — a DeleteDomain handler on the query surface that removes the domain from this service's state.
  Traces: simpledbv2-export-domain-to-s3-succeeds#teardown#9.

Related open issues that do NOT cover these wire steps: 20261109T154301Z-simpledbv2-simpledb-2009-04-15-query-api-unserved-on.md and 20261109T154301Z-s3-simpledbv2-domain-export-to-s3-checks-blocked-simpledb-2009.md are behavioral-corpus issues (check ids, no fixture replay key), and 20261023T081506Z-sdb-simpledbv2-client-errors-carry-non-aws-validation-message.md covers only the wording of three v2 client-error messages. None of them carries the replay key for this fixture's five steps.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture `simpledbv2-export-domain-to-s3-succeeds` (5 of the 8 reported divergence clusters, all service `sdb`):

- setup#1 CreateDomain — expected status 200, got 400. Emulator body: error code `InvalidAction`, message "unknown action: CreateDomain". AWS: 200 + <CreateDomainResponse xmlns="http://sdb.amazonaws.com/doc/2009-04-15/"> with only ResponseMetadata (RequestId/BoxUsage are per-run values, not requirements).
- setup#2 PutAttributes — expected status 200, got 400. Emulator: `InvalidAction`, "unknown action: PutAttributes". AWS: 200 + <PutAttributesResponse ...>.
- task#4 StartDomainExport — expected status 200, got 400; `$.clientToken`, `$.exportArn`, `$.requestedAt` all missing (each is a REQUIRED output member in the model). AWS: 200 with all three present.
- task#6 ListExports — expected status 200, got 400; `$.exportSummaries` missing (REQUIRED output member). Recorded 200 body held one summary: {domainName: <the exported domain>, exportArn: <the export's ARN>, exportStatus: "SUCCEEDED", requestedAt: <timestamp>} — the domain name, ARN and timestamp are per-run values; the requirement is one summary for the started export with exportStatus SUCCEEDED.
- teardown#9 DeleteDomain — expected status 200, got 400. Emulator: `InvalidAction`, "unknown action: DeleteDomain". AWS: 200 + <DeleteDomainResponse ...>.

Fixture score for the service: 7 of 19 steps passed, 10 mismatched, 2 blocked. Code evidence for the shared root cause: services/service-com-amazonaws-simpledbv2-simpledbv2/src/service.rs:76-96 (`can_handle` claims any request whose route key is `sdb`; `override_operation` returns None) and src/service.rs:112-124 (an unmatched request renders the protocol-default routing error, query -> 400 InvalidAction); src/state.rs:28-48 and 288-307 (the domain registry is written only by `create_domain`, reachable from SIMPLEDB_EXISTING_DOMAINS and from `insert_export`, with the comment that "NOTHING on this wire creates a domain").

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Observe fixture `simpledbv2-export-domain-to-s3-succeeds` diverge at steps setup#1, setup#2, task#4, task#6 and teardown#9 — every step expected 200 and got 400.
3. By hand: point a botocore `sdb` client (API version 2009-04-15) at the emulator and call create_domain(DomainName="wire-domain-1"), i.e. POST "/" with Action=CreateDomain&DomainName=wire-domain-1&Version=2009-04-15. Observe HTTP 400 / InvalidAction / "unknown action: CreateDomain" instead of a 200 CreateDomainResponse. Same for put_attributes(...) and delete_domain(...).
4. Then POST /v2/StartDomainExport with {"domainName":"wire-domain-1","s3Bucket":"wire-export-bucket"} and observe HTTP 400 (NoSuchDomainException, "The specified domain does not exist.") with no clientToken/exportArn/requestedAt, instead of a 200 carrying all three.
5. Then POST /v2/ListExports with {"domainName":"wire-domain-1"} and observe HTTP 400 with no `exportSummaries`, instead of a 200 whose required exportSummaries list holds the started export.
6. Re-replay to confirm the fix: simpledbv2-export-domain-to-s3-succeeds#setup#1, #setup#2, #task#4, #task#6, #teardown#9.
