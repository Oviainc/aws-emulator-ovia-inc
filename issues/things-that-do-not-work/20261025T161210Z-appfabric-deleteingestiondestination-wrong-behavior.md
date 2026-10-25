Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 19927c45d36f59a62bf3727b5eb74704e20f74dc

# DeleteIngestionDestination leaves an enabled ingestion enabled, on a stale premise that StartIngestion is still a scaffold

_Filed: 2026-10-25T16:12:10Z_
_Operation: DeleteIngestionDestination | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `19927c45d36f59a62bf3727b5eb74704e20f74dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents that removing a destination from an ENABLED ingestion ends with that ingestion disabled. The handler writes no ingestion-state transition at all, and its own module header justifies that by asserting a premise which is no longer true in this crate: it claims no ingestion can be enabled because `StartIngestion` is still a scaffold. `StartIngestion` is implemented today (`src/ops/start_ingestion.rs` writes `INGESTION_STATE_ENABLED` and a test pins it), so the premise the handler reasons from is false.

Consequence: after `StartIngestion` enables an ingestion and `DeleteIngestionDestination` removes its destination(s), the ingestion is left `enabled` forever with nothing to deliver to. `GetIngestion`/`ListIngestions` keep reporting `enabled`, and the sibling `DeleteIngestion` then refuses the ingestion with `ValidationException` ("is enabled and must be stopped before it can be deleted") until the caller issues an explicit `StopIngestion` — a step the documented behaviour says the service itself eventually performs. Nothing in the crate (no test, no other comment) justifies the divergence; the only justification given is the false scaffold premise.

## Evidence

Smithy (smithy_query doc, com.amazonaws.appfabric#DeleteIngestionDestination): "This deletes the association between an ingestion and it's destination. ... If the ingestion destination is deleted while the associated ingestion is enabled, the ingestion will fail and is eventually disabled." The same sentence is reproduced in the handler at services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/delete_ingestion_destination.rs:71-73.

Stale premise — src/ops/delete_ingestion_destination.rs:26-29: "//! AWS documents that deleting a destination under an ENABLED ingestion eventually\n//! disables that ingestion. No ingestion in this emulator can be enabled yet\n//! (`CreateIngestion` stores `disabled` and `StartIngestion` is still a scaffold), so\n//! the clock owes no ingestion-state transition on this path and none is written."

Contradicted by the sibling handler — src/ops/start_ingestion.rs:194-195: "ingestion.state = crate::state::INGESTION_STATE_ENABLED.to_string();\n        ingestion.updated_at = ::std::time::SystemTime::now();" (StartIngestion carries no `//! SCAFFOLDED, NOT IMPLEMENTED:` header and is pinned by tests/all/ingestion_enablement.rs:90 `starting_a_disabled_ingestion_enables_it_and_persists_it`).

The handler body confirms no state is touched — src/ops/delete_ingestion_destination.rs:239-244: "// Removing the record is the whole effect: ... its siblings, the ingestion itself and the bundle above it are left as they were ... ingestion.destinations.remove(&destination_key);".

Downstream contradiction — src/ops/delete_ingestion.rs:204-212 refuses an ingestion whose state is not `disabled`: "Ingestion {} is {} and must be stopped before it can be deleted."

Test coverage: no test asserts the ingestion state after a destination delete — tests/all/ingestion_destination_lifecycle.rs contains no occurrence of `enabled`/`/start` (the delete tests at lines 1077 and 1260 assert only the destination listing and status codes), so no existing test pins the current behaviour.

## Steps to Reproduce

1. POST /appbundles with an empty body -> note appBundle.arn
2. POST /appbundles/{bundleArn}/appauthorizations with app=SLACK, authType=apiKey, credential.apiKeyCredential.apiKey=..., tenant={tenantIdentifier:t-1,tenantDisplayName:t-1} -> 201, then POST .../appauthorizations/{authArn}/connect -> 200
3. POST /appbundles/{bundleArn}/ingestions with app=SLACK, tenantId=t-1, ingestionType=auditLog -> 201, note ingestion.arn
4. POST /appbundles/{bundleArn}/ingestions/{ingestionArn}/ingestiondestinations with a valid processingConfiguration + destinationConfiguration -> 201, note ingestionDestination.arn
5. POST /appbundles/{bundleArn}/ingestions/{ingestionArn}/start -> 200 (GET the ingestion: state == "enabled")
6. DELETE /appbundles/{bundleArn}/ingestions/{ingestionArn}/ingestiondestinations/{destinationArn} -> 204
7. GET /appbundles/{bundleArn}/ingestions/{ingestionArn} -> state is still "enabled" although the ingestion now has no destination; the documented eventual disable never happens
8. DELETE /appbundles/{bundleArn}/ingestions/{ingestionArn} -> 400 ValidationException "is enabled and must be stopped before it can be deleted"
