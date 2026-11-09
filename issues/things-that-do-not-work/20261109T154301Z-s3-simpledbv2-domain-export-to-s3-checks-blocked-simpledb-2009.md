Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__simpledbv2.export-domain-to-s3-contains-written-items s3__simpledbv2.repeated-client-token-with-different-params-rejected
Issue-Key: s3__simpledbv2|CreateDomain|InvalidAction
Issue-Commit: 5b2a9eedb8fa287cac2318fd09ae07f52de03ab6

# Domain-export-to-S3 checks blocked: SimpleDB 2009-04-15 CreateDomain refused with 400 InvalidAction on the sdb endpoint

> _Note: this issue was identified against the code base at commit `5b2a9eedb8fa287cac2318fd09ae07f52de03ab6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Both cross-service (S3 + SimpleDBv2) behavioral checks fail on their first call, before any S3 or export behavior is exercised.

Cause 1 of 1: the legacy Amazon SimpleDB 2009-04-15 AWS Query API is not served on the `sdb` wire, so the domain whose items the export is supposed to write into S3 cannot be created.

  AWS: the `sdb` endpoint (Smithy: com.amazonaws.simpledbv2#SimpleDBv2, aws.api#service endpointPrefix "sdb"/arnNamespace "sdb", aws.auth#sigv4 name "sdb") serves both the restJson1 /v2/* export operations and the Amazon SimpleDB 2009-04-15 AWS Query API on "/" (form-encoded POST with Action=<op>&Version=2009-04-15). CreateDomain answers HTTP 200 with <CreateDomainResponse xmlns="http://sdb.amazonaws.com/doc/2009-04-15/"> containing only <ResponseMetadata><RequestId>..</RequestId><BoxUsage>..</BoxUsage></ResponseMetadata>, and is idempotent on an existing name; PutAttributes and DeleteDomain answer 200 with their own envelopes in the same namespace. The items written by PutAttributes are what a later StartDomainExport serializes into the target S3 bucket under the requested key prefix (one object per export data file, each item's own attributes, byte-faithful). The repo carries no Smithy model for the 2009-04-15 surface, so that half is documented/recorded behavior only; the v2 model is cited only for the shared wire identity.
  Emulator: the 2009-04-15 request is claimed by the simpledbv2 service (its signing name is `sdb`), resolves to no modeled operation, and gets the protocol-default routing error: HTTP 400, Error Code `InvalidAction`, message `unknown action: CreateDomain` — fixed text "unknown action: " plus the request's Action value.
  Why: a client cannot populate a domain at all, so the documented export-to-S3 path is unusable: nothing can be exported and no object is ever written to the caller's bucket. The error code also misinforms the SDK — InvalidAction says the action does not exist on this service, when it does exist on this endpoint, and it is not a modeled error of any SimpleDB action so no client branch handles it.
  Fix: the simpledbv2 service's request-claiming/operation-resolution seam plus handlers and state for the 2009-04-15 query surface (dispatch a form-encoded POST on "/" on its `Action` field to CreateDomain/PutAttributes/DeleteDomain, serialize the 2009-04-15 XML envelopes, and store domains and items so StartDomainExport can both resolve the domain and write its items to S3).
  Traces: s3__simpledbv2.export-domain-to-s3-contains-written-items, s3__simpledbv2.repeated-client-token-with-different-params-rejected. Code citations: services/service-com-amazonaws-simpledbv2-simpledbv2/src/service.rs:76-96 and 112-124; services/service-com-amazonaws-simpledbv2-simpledbv2/src/state.rs:28-48 (domain registry fed only by SIMPLEDB_EXISTING_DOMAINS).

Note: this is the same root cause as the simpledbv2-only issue filed in this sweep; it is filed separately because these two checks are reported under a different service heading. The open issue 20261023T081506Z-sdb-simpledbv2-client-errors-carry-non-aws-validation-message.md concerns v2 error-message wording and does not cover it.

## Evidence
Both checks error identically on their first call:

  botocore.exceptions.ClientError: An error occurred (InvalidAction) when calling the CreateDomain operation: unknown action: CreateDomain

raised from sdb.create_domain(DomainName=<unique name>) with sdb = ctx.client("sdb") (SimpleDB 2009-04-15), a form-encoded POST to "/" on the `sdb` endpoint.

Expected (AWS): HTTP 200 with <CreateDomainResponse xmlns="http://sdb.amazonaws.com/doc/2009-04-15/"><ResponseMetadata>…</ResponseMetadata></CreateDomainResponse>, after which PutAttributes items can be written and StartDomainExport(domainName=…, s3Bucket=…, s3KeyPrefix="exports/") writes those items into the bucket (data objects under the prefix containing each item's name and its attributes).
Actual (emulator): HTTP 400, Error Code "InvalidAction", Error Message "unknown action: CreateDomain".

Consequently none of the cross-service assertions ran: no export object appeared under the S3 prefix, and the clientToken-reuse-with-different-parameters rejection was never reached.

## Steps to Reproduce
1. Boot the emulator; create an S3 bucket with the s3 client (this part works).
2. With sdb = boto3.client("sdb"), call sdb.create_domain(DomainName="behav-export-domain") — POST "/" Action=CreateDomain&Version=2009-04-15. Observe HTTP 400 / InvalidAction / "unknown action: CreateDomain".
3. The intended follow-ups are blocked: sdb.put_attributes(DomainName=…, ItemName="item-widget", Attributes=[{"Name":"color","Value":"blue","Replace":True}]), then simpledbv2 StartDomainExport(domainName=…, s3Bucket=<bucket>, s3KeyPrefix="exports/"), then list the bucket for data objects under exports/ and compare the exported items to what was written.
4. Re-replay checks: s3__simpledbv2.export-domain-to-s3-contains-written-items, s3__simpledbv2.repeated-client-token-with-different-params-rejected.
