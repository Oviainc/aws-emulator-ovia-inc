Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.catalog-federation-parameters-round-trip glue.get-catalog-federation-source-error-propagated
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.catalog-federation-parameters-round-trip|_error#13|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: glue|GetCatalog|EntityNotFoundException
Issue-Commit: d377c937e7b6962609af43d6e716acbe9aa40428

# glue: 3 net-new root causes — CreateCatalog accepts a native (non-federated) catalog instead of refusing it, GetCatalog refuses the account root catalog, and GetCatalog's not-found message echoes the requested CatalogId

> _Note: this issue was identified against the code base at commit `d377c937e7b6962609af43d6e716acbe9aa40428`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCatalog

## Description
Three independent root causes in the Glue catalog-entity surface (CreateCatalog / GetCatalog). The `CreateCatalog` and `GetCatalog` handlers are no longer 501 scaffolds — they answer — but what they answer diverges from AWS. Related-but-not-covering open issue: `20261025T022820Z-glue-getcatalog-stub.md` ('Implement glue GetCatalog: unimplemented scaffold stub'), which describes GetCatalog as returning HTTP 501; in this replay GetCatalog answers a modeled EntityNotFoundException instead, so that issue's NotImplemented cause does not cover causes 2 and 3 below. Likewise `20261026T073706Z-glue-createcatalog-stub.md` is already fixed and CreateCatalog now answers 200 — cause 1 is the behavior of the implemented handler, not a stub.

Cause 1 of 3: CreateCatalog accepts a plain native catalog.
AWS: Glue's CreateCatalog only creates FEDERATED catalogs. A CatalogInput that carries no `FederatedCatalog` (here only `Parameters`) is refused with HTTP 400, error code `InvalidInputException`, message `Create glue native catalog is not supported.` — that message is entirely fixed text, it has no variable parts. Smithy: `CreateCatalog` input `{Name: CatalogNameString @required @length(1,64) @pattern ^(?!(.*[./\\]|aws:)).*$, CatalogInput: CatalogInput @required, Tags}`, output is empty, and `InvalidInputException` (client, HTTP 400) is a modeled error of the operation; the `Create glue native catalog is not supported.` wording is recorded behavior.
Emulator: the call is SERVED — HTTP 200 with an empty JSON body (`content-type: application/x-amz-json-1.1`, `content-length: 2`), no error — so a native catalog is silently created under the requested name.
Why: a client that relies on Glue refusing native catalogs sees a success and believes it owns a usable catalog. Nothing federates, so every later read/write through that catalog id behaves unlike AWS, and the code path that handles `InvalidInputException` (fall back to a federated catalog, or surface the unsupported-feature error) is never exercised. Worse, the accepted name is now occupied, so the subsequent legitimate federated CreateCatalog for the same name can collide.
Fix: the CreateCatalog handler's input validation in the glue service crate — reject a CatalogInput with no `FederatedCatalog` before any state mutation, with InvalidInputException and AWS's fixed message, and persist nothing.
Traces: glue.catalog-federation-parameters-round-trip.

Cause 2 of 3: GetCatalog refuses the account's root Data Catalog.
AWS: recorded behavior only — the model documents no root-catalog rule. The recording shows `GetCatalog` with `CatalogId` set to the caller's own AWS account id (from sts:GetCallerIdentity) SUCCEEDING and returning a `Catalog` whose `CatalogId` (or `Name`) is that account id, i.e. the account's root/default Data Catalog is readable as a catalog entity and is not a tenant-created object. Smithy: `GetCatalog` input `{CatalogId: CatalogIdString @required @length(1,255)}`, output `{Catalog: Catalog}`, modeled errors include `EntityNotFoundException` (client, HTTP 400).
Emulator: the same call is refused — `EntityNotFoundException: Catalog <account-id> not found.` (observed as `Catalog 000000000000 not found.`). The emulator only knows catalogs that a CreateCatalog call created; a freshly booted account has no root catalog in state, so reading it 400s.
Why: every caller that reads its own catalog before doing catalog-scoped work — and every positive control that proves credentials/permissions are fine before asserting a specific denial — fails with a not-found. The client cannot distinguish 'my account has no Glue' from 'this specific federated catalog is missing', and a check for a real rejection downstream can never be reached.
Fix: the glue service's catalog state/seeding plus the GetCatalog handler — the account's root catalog must exist implicitly (resolve a CatalogId equal to the caller's account id to the account root catalog) rather than requiring a stored CreateCatalog record.
Traces: glue.get-catalog-federation-source-error-propagated.

Cause 3 of 3: GetCatalog's EntityNotFoundException message is request-derived instead of AWS's fixed text.
AWS: for a catalog that does not exist, GetCatalog answers HTTP 400 `EntityNotFoundException` with the message `Catalog not found.` — the whole message is fixed text; it interpolates NO value from the request (no catalog id, no account id). Smithy: `EntityNotFoundException` is a modeled client error (HTTP 400) of `GetCatalog`; the message wording is recorded behavior.
Emulator: the message is built from the request, `Catalog <CatalogId> not found.` — observed verbatim as `Catalog 000000000000 not found.`, where `000000000000` is the CatalogId taken from the request and the rest is emulator-authored template text. This is the same defect class already filed for other Glue operations (e.g. `20261025T213800Z-glue-getjobrun-s-entitynotfoundexception-message-echoes-the-request.md`, `20261027T083036Z-glue-getjob-updatejob-entitynotfoundexception-message-is-request-derived.md`), but for GetCatalog, whose handler is not covered by those.
Why: clients and test suites that match on Glue's error message see text AWS never emits; message-equality assertions and log/alarm matching on `Catalog not found.` break, and the request-derived form also leaks the requested id into the message where AWS does not.
Fix: the GetCatalog handler's error construction in the glue service crate — emit the fixed `Catalog not found.` constant (the same shared Glue not-found message helper the other catalog reads use) rather than formatting the requested CatalogId into the message.
Traces: glue.get-catalog-federation-source-error-propagated, glue.catalog-federation-parameters-round-trip (which asserts the same `Catalog not found.` text after its rejected CreateCatalog attempts).

## Evidence
Cause 1 — glue.catalog-federation-parameters-round-trip (verdict: mismatch):
  call: glue.create_catalog(Name=<name>, CatalogInput={"Parameters": {"federation.source.host": "...redshift.amazonaws.com", "federation.source.port": "5439"}})
  expected (AWS): ClientError, HTTP 400, Code=InvalidInputException, Message="Create glue native catalog is not supported."
  actual (emulator): HTTP 200, empty body —
    AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}, 'RetryAttempts': 0}}
  (the check's positive control — CreateCatalog with a FederatedCatalog on an S3 Tables bucket, then GetCatalog of it — succeeded, so CreateCatalog is not blanket-denied; the refusal above is specifically missing.)

Cause 2 — glue.get-catalog-federation-source-error-propagated (verdict: error):
  call: glue.get_catalog(CatalogId=<caller account id>)  # positive control
  expected (AWS): HTTP 200 with Catalog.CatalogId (or Catalog.Name) == the account id
  actual (emulator): botocore.errorfactory.EntityNotFoundException: An error occurred (EntityNotFoundException) when calling the GetCatalog operation: Catalog 000000000000 not found.
  The check aborts on this control, so its real target (CreateCatalog on an unregistered federation source must answer AccessDeniedException "The data source [<identifier>] is not registered.") is never reached.

Cause 3 — same response as above:
  message fixed text expected by AWS: "Catalog not found." (no interpolation)
  message emitted by emulator: "Catalog <CatalogId> not found." — observed "Catalog 000000000000 not found.", where `000000000000` is the CatalogId copied from the request and "Catalog " / " not found." is emulator template text.
  The same fixed text is asserted independently by glue.catalog-federation-parameters-round-trip: expected GetCatalog's not-found message "Catalog not found."

## Steps to Reproduce
1. Boot the emulator and point a Glue client at it.
2. Cause 1: call CreateCatalog with Name=<any valid name> and CatalogInput={"Parameters": {"federation.source.host": "<host>", "federation.source.port": "5439"}} (no FederatedCatalog member). Observe HTTP 200 and an empty body; AWS answers HTTP 400 InvalidInputException "Create glue native catalog is not supported." Then call GetCatalog on that name and observe the catalog was persisted, where AWS has nothing.
3. Cause 2: call sts:GetCallerIdentity to get the account id, then call GetCatalog with CatalogId=<that account id>. Observe EntityNotFoundException; AWS returns the account's root Data Catalog with CatalogId == that account id.
4. Cause 3: call GetCatalog with CatalogId=<a name that does not exist>. Observe the message "Catalog <CatalogId> not found." (the requested id interpolated); AWS returns the fixed message "Catalog not found."
5. Replay the two grounding checks: glue.catalog-federation-parameters-round-trip and glue.get-catalog-federation-source-error-propagated.
