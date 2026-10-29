Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr-public.put-registry-catalog-data-unsupported-command-rejected
Issue-Shape-Id: com.amazonaws.ecrpublic#SpencerFrontendService
Issue-Key: ecr-public.put-registry-catalog-data-unsupported-command-rejected|_error#10|AssertionError|raise AssertionError(
Issue-Commit: e0c940385d2021579b054d05faa052f4cbf83854

# ECR Public: PutRegistryCatalogData is served 200 for a non-us-east-1-signed caller instead of UnsupportedCommandException, and mutates account-global registry catalog data

> _Note: this issue was identified against the code base at commit `e0c940385d2021579b054d05faa052f4cbf83854`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRegistryCatalogData

## Description
Cause 1 of 1: the Region gate on PutRegistryCatalogData is absent — a request whose SigV4 credential scope names a Region other than us-east-1 is accepted, answered HTTP 200, and allowed to overwrite the account-singleton registry catalog record, where AWS refuses it with the modelled UnsupportedCommandException and leaves the record untouched.

AWS: Smithy (services/service-com-amazonaws-ecrpublic-spencerfrontendservice/smithy-model/com-amazonaws-ecrpublic-spencerfrontendservice.json) models PutRegistryCatalogData with input member `displayName` -> RegistryDisplayName @length {min 0, max 100}, @required output member `registryCatalogData` -> RegistryCatalogData, and three errors: InvalidParameterException (client, httpError 400), ServerException (server, 500) and UnsupportedCommandException (client, httpError 400), documented "The action isn't supported in this Region." ECR Public's registry catalog write is an account-global, us-east-1-only control action (https://docs.aws.amazon.com/general/latest/gr/ecr-public.html): the same call issued from us-east-1 succeeds 200 with registryCatalogData.displayName echoing the submitted string, while the identical call signed for any other Region is refused HTTP 400 with error code `UnsupportedCommandException` and message `PutRegistryCatalogData command is only supported in us-east-1.` — in that message the fixed text is `<operation> command is only supported in us-east-1.` with the operation name taken from the request (here `PutRegistryCatalogData`) and the Region being the single Region that serves the action; there are no per-run values in it. The refusal must be pre-mutation: a subsequent GetRegistryCatalogData from us-east-1 must still report the value written by the last accepted call (recorded behavior for the no-mutation guarantee; the model constrains only displayName's length).

Emulator: the us-west-2-signed call `put_registry_catalog_data(displayName="should-not-apply")` returns HTTPStatusCode 200 with body `{"registryCatalogData": {"displayName": "should-not-apply"}}` (content-type application/x-amz-json-1.1). No UnsupportedCommandException is produced for any signing Region, and the accepted write replaces the displayName the check had just stored through the us-east-1 client, so the account-global record is mutated by a call AWS never serves.

Why: a client relying on AWS's behavior gets two things wrong. First, the Region constraint disappears: code that is supposed to pin this control call to us-east-1 (or to catch the terminal 400 UnsupportedCommandException and re-issue against us-east-1) is never exercised and ships believing a regional endpoint accepts it, then fails in production on the real service. Second, it is an over-permissive accept of an account-global mutation: a regional caller silently rebrands the public registry's display name on the emulator, so tests of "wrong-region writes are inert" pass wrongly and any cross-region reconciliation logic is validated against state AWS would never have changed.

History (not a new defect report, context for the fixer): this behavior has oscillated across already-resolved issues. 20261022T192707Z-ecr-public-putregistrycatalogdata-has-no-us-east-1.md reported the missing gate; 20261025T010839Z-ecr-public-the-new-us-east-1-gate.md reported the gate refusing the legitimate us-east-1 caller; 20261025T192924Z-ecr-public-the-putregistrycatalogdata-region-gate-no-longer.md reported this same non-us-east-1 200; and 20261027T223608Z-ecr-public-putregistrycatalogdata-is-unconditionally-refused-with-unsupportedcommandexception.md reported an unconditional refusal. The latest fix removed the refusal altogether rather than making it conditional on the caller's signing Region, which is the state replayed here. This is an over-permissive accept, not a refusal left in place by a fix.

Fix: the PutRegistryCatalogData handler in the ecr-public service crate, specifically its Region/support gate, must resolve the Region from the request's SigV4 credential scope and return 400 UnsupportedCommandException with the message above when that Region is not us-east-1, before touching the registry-catalog state; a us-east-1-signed request must continue to be served 200 and persist displayName (with the existing @length max 100 InvalidParameterException validation unchanged). GetRegistryCatalogData must remain served in every Region, as it is today.

Traces: ecr-public.put-registry-catalog-data-unsupported-command-rejected.

## Evidence
Check: ecr-public.put-registry-catalog-data-unsupported-command-rejected [mismatch] (suite simparity-behavioral-ga-prod-v1; ecr-public 67/74, and the service is bootstrapped — service-com-amazonaws-ecrpublic-spencerfrontendservice is in the bootstrapped set — and answering, so this is not a bootstrap gap).

Sequence the check runs (both clients built via ctx.client("ecr-public", region=...), so they differ only in the signing Region of their SigV4 credential scope):
  1. ecrp_east.get_registry_catalog_data() -> saved for restore (succeeds).
  2. ecrp_east.put_registry_catalog_data(displayName=<unique control name, <=100 chars>) -> accepted; the read-back control assertion passes, so the us-east-1 write path works.
  3. ecrp_west.put_registry_catalog_data(displayName="should-not-apply").

Expected at step 3 (AWS): HTTP 400, Error.Code == "UnsupportedCommandException", Error.Message == "PutRegistryCatalogData command is only supported in us-east-1.", and step 4's ecrp_east.get_registry_catalog_data() still reporting the control name from step 2.

Actual at step 3 (emulator): no ClientError at all. Harness assertion: "expected the us-west-2 endpoint to reject with UnsupportedCommandException, but the call SUCCEEDED and returned {'registryCatalogData': {'displayName': 'should-not-apply'}, 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '58', ...}, 'RetryAttempts': 0}}" (check_ecr_public_put_registry_catalog_data_unsupported_command_rejected.py line 61, raised from run() line 38). Because the call was served, the account-global displayName now holds the value the wrong-Region call supplied, so the check's no-mutation assertion at step 4 is also unreachable.

Smithy consulted: PutRegistryCatalogData input {displayName -> RegistryDisplayName @length 0..100}; output {registryCatalogData -> RegistryCatalogData, required}; errors InvalidParameterException (client/400), ServerException (server/500), UnsupportedCommandException (client/400, "The action isn't supported in this Region").

## Steps to Reproduce
1. git_clone() the repo, then run_behavioral_suite() to build and boot the emulator.
2. Build two boto3 'ecr-public' clients against the emulator endpoint that differ only in signing Region: one for us-east-1, one for us-west-2.
3. With the us-east-1 client, record the current displayName (get_registry_catalog_data) and then write a control value: put_registry_catalog_data(displayName="ctl-<unique>"); confirm get_registry_catalog_data reads it back. This must succeed (it does today).
4. With the us-west-2 client, call put_registry_catalog_data(displayName="should-not-apply").
   Expected: HTTP 400, Code="UnsupportedCommandException", Message="PutRegistryCatalogData command is only supported in us-east-1."
   Actual:   HTTP 200 with {'registryCatalogData': {'displayName': 'should-not-apply'}}.
5. With the us-east-1 client, call get_registry_catalog_data(): it must still return the control value from step 3 (the wrong-Region write must not have mutated account-global state). Today it returns 'should-not-apply'.
6. Confirm GetRegistryCatalogData still answers 200 from BOTH Regions (the gate applies to the write only), and that the us-east-1 write path and its over-length displayName InvalidParameterException are unaffected.
7. Replay to confirm: ecr-public.put-registry-catalog-data-unsupported-command-rejected.
