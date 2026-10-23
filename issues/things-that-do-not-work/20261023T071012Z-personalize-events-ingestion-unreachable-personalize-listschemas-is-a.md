Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: personalize-events.put-action-interactions-max-batch-ten-success personalize-events.put-action-interactions-multiple-event-types-success personalize-events.put-action-interactions-optional-fields-success personalize-events.put-action-interactions-reserved-property-keyword-rejected personalize-events.put-action-interactions-tracker-not-found-rejected personalize-events.put-actions-success personalize-events.put-actions-wrong-dataset-type-rejected personalize-events.put-events-concurrent-writes-all-succeed personalize-events.put-events-max-batch-ten-events-success personalize-events.put-events-minimal-fields-success personalize-events.put-events-nonexistent-tracking-id-rejected personalize-events.put-events-optional-fields-round-trip-success personalize-events.put-events-reserved-property-keyword-rejected personalize-events.put-items-dataset-not-found-rejected personalize-events.put-items-max-batch-ten-items-success personalize-events.put-items-property-update-reflected-in-export personalize-events.put-items-success personalize-events.put-users-max-batch-ten-users-success personalize-events.put-users-success
Issue-Shape-Id: com.amazonaws.personalizeevents#AmazonPersonalizeEvents
Issue-Key: personalize-events|ListSchemas|NotImplemented
Issue-Key: personalize-events|PutItems|ResourceNotFoundException
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# personalize-events ingestion unreachable: personalize ListSchemas is a 501 stub, and CreateDataset never registers the dataset for PutItems

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSchemas, PutItems

## Description
All 19 personalize-events behavioral checks fail, in two distinct root causes. Every check must first stand up a real Amazon Personalize target (dataset group + schema + dataset, or an event tracker) through the `personalize` control plane and then ingest through `personalize-events`; both causes break that path, so no ingestion operation is ever reached with a valid target.

This issue bundles 2 root causes. BOTH must be closed before submitting.

=== Cause 1 of 2: `personalize` ListSchemas answers HTTP 501 NotImplemented (17 checks) ===
AWS: Smithy (personalize model, as reproduced in the handler's "Modeled contract for ListSchemas" header): ListSchemas takes optional `maxResults` (range 1..100) and `nextToken` (len 0..1500, pattern ^\p{ASCII}{0,1500}$) and returns HTTP 200 with `schemas` (Schemas, len 0..100) and an optional `nextToken`; the only modeled error is `InvalidNextTokenException` (HTTP 400). An account with no schemas returns 200 with an empty/absent list, never an error.
Emulator: the operation is an unimplemented scaffold stub and answers HTTP 501 with error code `NotImplemented` and message "operation ListSchemas is scaffolded but not implemented in this emulator" (example text). Every check that enumerates existing schemas to adopt/create one dies there before any personalize-events call is made.
Why: a client cannot discover or reuse schemas at all; any setup flow that lists schemas before CreateSchema/CreateDataset (the pattern the AWS docs and console use, and the pattern that avoids re-minting a schema on every run) fails hard with a 501 that no SDK models as retryable or recoverable, instead of a 200 with a list.
Fix: implement the handler for the operation (list the schemas held in this service's state, apply `maxResults`/`nextToken` paging, reject a malformed token with `InvalidNextTokenException` 400) and drop the SCAFFOLDED marker.
Traces: services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/list_schemas.rs lines 3-12 (SCAFFOLDED marker) and line 156 (`Err(crate::scaffolded::Scaffolded::not_implemented("ListSchemas"))`). Failing checks: personalize-events.put-action-interactions-max-batch-ten-success, personalize-events.put-action-interactions-multiple-event-types-success, personalize-events.put-action-interactions-optional-fields-success, personalize-events.put-action-interactions-reserved-property-keyword-rejected, personalize-events.put-action-interactions-tracker-not-found-rejected, personalize-events.put-actions-success, personalize-events.put-actions-wrong-dataset-type-rejected, personalize-events.put-events-concurrent-writes-all-succeed, personalize-events.put-events-max-batch-ten-events-success, personalize-events.put-events-minimal-fields-success, personalize-events.put-events-nonexistent-tracking-id-rejected, personalize-events.put-events-optional-fields-round-trip-success, personalize-events.put-events-reserved-property-keyword-rejected, personalize-events.put-items-dataset-not-found-rejected, personalize-events.put-items-success, personalize-events.put-users-max-batch-ten-users-success, personalize-events.put-users-success.

=== Cause 2 of 2: PutItems 404s on a dataset the control plane just created ACTIVE (2 checks) ===
AWS: Smithy (personalize-events): PutItems input `datasetArn` (required, len 0..256, pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$) and `items` (required, 1..10); success is HTTP 200 (empty body); `ResourceNotFoundException` is HTTP 404 and is for a dataset that does not exist. A dataset ARN minted by `personalize` CreateDataset and polled to status ACTIVE through DescribeDataset exists, so PutItems against it returns 200 and the items are ingested.
Emulator: after `personalize` CreateDataset returns a dataset ARN and DescribeDataset reports it ACTIVE, `personalize-events` PutItems against that same ARN returns HTTP 404 `ResourceNotFoundException` (recorded message example: "No dataset with the ARN arn:aws:personalize:us-west-2:000000000000:dataset/<dsg-name>/ITEMS exists."). The ingestion crate's target registry is only reachable in-process, so a dataset created over the wire is never registered for ingestion.
Why: no SDK client can ever ingest items (and by the same seam, events/users/actions via an event tracker) against the running emulator — the documented create-then-ingest sequence is permanently refused with a not-found for a resource the control plane reports as ACTIVE, so any data-plane workflow is impossible to emulate.
Fix: in the `personalize` control plane, have CreateDataset (and CreateEventTracker for trackingId) seed the personalize-events state registry from the shared engine; the ingestion handlers already read whatever is registered. NOTE: the open issue "Wire the Personalize control plane to the Personalize Events registry" (20261022T000000Z-personalize-control-plane-does-not-register-event-trackers-or-datasets-for-ingestion.md) describes this same seam; this cause is the replayed behavioral evidence for it and should be closed together with it.
Traces: services/service-com-amazonaws-personalizeevents-amazonpersonalizeevents/src/ops/put_items.rs and src/state.rs (register_dataset / register_event_tracker). Failing checks: personalize-events.put-items-max-batch-ten-items-success, personalize-events.put-items-property-update-reflected-in-export.

## Evidence
Suite: personalize-events 0/19 passed (0%), 19 errored, 0 mismatched.

Cause 1 (17 checks), e.g. personalize-events.put-events-minimal-fields-success:
  expected: HTTP 200 from ListSchemas with a `schemas` list (modeled Schemas, len 0..100), only InvalidNextTokenException (400) possible
  actual:   botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListSchemas operation: operation ListSchemas is scaffolded but not implemented in this emulator  [HTTP 501]

Cause 2 (2 checks), e.g. personalize-events.put-items-max-batch-ten-items-success:
  setup:    personalize.create_dataset_group -> ACTIVE; personalize.create_schema; personalize.create_dataset(datasetType="Items") -> datasetArn, polled ACTIVE via describe_dataset
  expected: personalize-events.put_items(datasetArn=<that ARN>, items=[10 items]) -> HTTP 200
  actual:   botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the PutItems operation: No dataset with the ARN arn:aws:personalize:us-west-2:000000000000:dataset/sim2real-pevt-dsg-30fa52c9b432/ITEMS exists.  [HTTP 404]
  (same for personalize-events.put-items-property-update-reflected-in-export, dataset group sim2real-pevt-dsg-6967412eaad4)
The generated names, account id and region above are environment artifacts, not requirements.

## Steps to Reproduce
Cause 1:
1. Boot the emulator and point an SDK client at it.
2. Call personalize ListSchemas (no parameters).
3. Observe HTTP 501 / error code NotImplemented; AWS returns HTTP 200 with a `schemas` list (empty when the account has none).

Cause 2:
1. Boot the emulator.
2. personalize CreateDatasetGroup -> poll DescribeDatasetGroup until ACTIVE.
3. personalize CreateSchema with an Items schema, then CreateDataset(datasetType="Items", schemaArn=..., datasetGroupArn=...) -> poll DescribeDataset until ACTIVE.
4. personalize-events PutItems(datasetArn=<the ARN from step 3>, items=[{itemId:"item-0", properties:"{\"price\":0.0}"}]).
5. Observe HTTP 404 ResourceNotFoundException; AWS returns HTTP 200 and the item is ingested.
