Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 743fc0a4792edb6d196f368afdefaae152d33ff0

# CreateFHIRDatastore stores SseConfiguration/PreloadDataConfig but materialises neither documented effect (no AWS-owned-key default, no Synthea import)

_Filed: 2026-10-24T02:19:41Z_
_Operation: CreateFHIRDatastore | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `743fc0a4792edb6d196f368afdefaae152d33ff0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateFHIRDatastore persists its configuration members verbatim and reports them back, but two documented EFFECTS of those members never happen, so later reads report a data store that differs from the one the real service creates:

1. Default server-side encryption is not materialised. The model documents KmsEncryptionConfig as "The customer-managed-key (CMK) used when creating a data store. If a customer-owned key is not specified, an Amazon Web Services-owned key is used for encryption", and CmkType models AWS_OWNED_KMS_KEY for exactly that case. Expected: a data store created WITHOUT SseConfiguration is encrypted with the service-owned key, and DescribeFHIRDatastore / ListFHIRDatastores report DatastoreProperties.SseConfiguration = {KmsEncryptionConfig:{CmkType:"AWS_OWNED_KMS_KEY"}}. Actual: the handler stores `input.sse_configuration` as-is (None), `Datastore::properties()` clones that None, and the member is omitted from every Describe/List body — a client reading the data store's encryption posture sees no configuration at all.

2. PreloadDataConfig is inert. The member is documented as "An optional parameter to preload (import) open source Synthea FHIR data upon creation of the data store". Expected: a data store created with PreloadDataConfig={PreloadDataType:"SYNTHEA"} comes up holding the preloaded Synthea FHIR resources, reachable through the data store's FHIR REST endpoint. Actual: the value is only echoed back by Describe/List — the new record's `fhir_resources` map is initialised empty and nothing in the crate ever imports anything (grep for SYNTHEA/preload finds only the stored field), so a SYNTHEA data store is byte-for-byte an empty data store.

## Evidence

Smithy model: KmsEncryptionConfig doc ("If a customer-owned key is not specified, an Amazon Web Services-owned key is used for encryption") and CmkType values [CUSTOMER_MANAGED_KMS_KEY, AWS_OWNED_KMS_KEY]; CreateFHIRDatastoreRequest$PreloadDataConfig doc ("An optional parameter to preload (import) open source Synthea FHIR data upon creation of the data store"); DatastoreProperties carries both SseConfiguration and PreloadDataConfig. Handler: src/ops/create_fhir_datastore.rs lines ~289-319 — `sse_configuration: input.sse_configuration` and `preload_data_config: input.preload_data_config` are stored unchanged, `fhir_resources: BTreeMap::new()` with the comment "A new data store holds no FHIR resources". Reporting: src/state.rs `Datastore::properties()` (lines ~342-372) clones both Options straight through. Tests: tests/all/fhir_datastore_lifecycle.rs lines ~265-285 assert the opposite of symptom 1 — "one that was not supplied is absent rather than invented" — i.e. the current no-default behaviour is pinned by the suite; no test asserts any preloaded resource exists.

## Steps to Reproduce

1. CreateFHIRDatastore with only DatastoreTypeVersion=R4 (no SseConfiguration), wait for ACTIVE, then DescribeFHIRDatastore: DatastoreProperties has no SseConfiguration member, where the service reports KmsEncryptionConfig.CmkType=AWS_OWNED_KMS_KEY for the service-owned key
2. Repeat the check through ListFHIRDatastores: the same data store's entry also omits SseConfiguration
3. CreateFHIRDatastore with PreloadDataConfig={"PreloadDataType":"SYNTHEA"} and wait for ACTIVE
4. DescribeFHIRDatastore echoes PreloadDataConfig, but the data store holds no resources: no Synthea Patient/Observation resource is readable or searchable on the reported DatastoreEndpoint, whereas a real SYNTHEA data store comes up populated
