Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 9e4273988d175f89e8a017aa0737ee7107d62a89

# Bootstrap the CloudSearch emulator

_Filed: 2026-10-21T23:48:08Z_
_Operation: service-com-amazonaws-cloudsearch-a9searchcloudconfigservice2013 | Class: stub_

> _Note: this issue was identified against the code base at commit `9e4273988d175f89e8a017aa0737ee7107d62a89`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in this crate is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding zero state, so no client workflow works. Implement a FIRST stateful slice covering the search-domain lifecycle plus its index configuration. Scope (in order):

1. **CreateDomain** — input `DomainName` (required, len 3..28, `^[a-z][a-z0-9\-]+$`). Allocate and persist a new domain; return `DomainStatus` computed from the stored record: `DomainId` (required, e.g. `<acct>/<name>`), `DomainName`, `ARN` (`arn:aws:cloudsearch:<region>:<acct>:domain/<name>`), `Created=true`, `Deleted=false`, `RequiresIndexDocuments=false` (required), `Processing=false`, `SearchInstanceType`, `SearchPartitionCount=1`, `SearchInstanceCount=1`, `DocService`/`SearchService` endpoints. A second CreateDomain with the same name must fail with `ResourceAlreadyExistsException` (HTTP 409), and a name violating the length/pattern constraint with `ValidationException` (HTTP 400).
2. **DefineIndexField** — input `DomainName` + `IndexField` (required: `IndexFieldName` len 1..64 pattern `^([a-z][a-z0-9_]*\*?|\*[a-z0-9_]*)$`, `IndexFieldType`, plus the per-type options struct). Store the field under the domain, keyed by name (re-defining an existing name updates it and bumps `UpdateVersion`). Return `IndexFieldStatus{Options: <the stored IndexField>, Status: OptionStatus{CreationDate, UpdateDate, UpdateVersion, State=RequiresIndexDocuments, PendingDeletion=false}}`. Defining a field must set the owning domain's `RequiresIndexDocuments=true`. Unknown domain → `ResourceNotFoundException` (409).
3. **DescribeIndexFields** — input `DomainName`, optional `FieldNames` filter, optional `Deployed`. Return `IndexFields` (required) built from persisted fields: all of them when `FieldNames` is empty, else only the named ones; when `Deployed=true` return only fields whose last IndexDocuments-committed (deployed) copy exists. Must reflect every prior DefineIndexField/DeleteIndexField.
4. **IndexDocuments** — input `DomainName`. Promote all pending field options to deployed, flip each field's `OptionStatus.State` to `Active`, purge fields marked `PendingDeletion`, set the domain's `RequiresIndexDocuments=false`, and return `FieldNames` = the sorted names of the now-indexed fields (computed from state, not echoed).
5. **DescribeDomains / ListDomainNames** — `DescribeDomains` takes an optional `DomainNames` list and returns `DomainStatusList` (required) from the store (all domains when the filter is absent, only matches otherwise, with current `RequiresIndexDocuments`/`Processing` values). `ListDomainNames` takes no input and returns `DomainNames` as a map of domain name → status string for every live domain.
6. **DeleteIndexField / DeleteDomain** — `DeleteIndexField` (input `DomainName`, `IndexFieldName`) must return the removed field's `IndexFieldStatus` with `Status.PendingDeletion=true`, set the domain's `RequiresIndexDocuments=true`, and `ResourceNotFoundException` (409) for an unknown field or domain. `DeleteDomain` must remove the domain (and its fields) and return its `DomainStatus` with `Deleted=true`; afterwards it must not appear in `ListDomainNames`/`DescribeDomains`, and `DescribeIndexFields` on it must raise `ResourceNotFoundException`.

Every one of these must compute its response from persisted state — no canned structures, no echoing of the request body, no random IDs that are never stored. A create followed by a describe must return exactly what was created, and a delete must be observable by the next read.

## State Model

- `CloudSearchState { domains: Map<String /*DomainName*/, Domain> }` stored in the single shared engine via `crate::state::service_state`.
- `Domain { domain_id: String, domain_name: String, arn: String, created: bool, deleted: bool, processing: bool, requires_index_documents: bool, search_instance_type: String, search_partition_count: i32, search_instance_count: i32, doc_service_endpoint: String, search_service_endpoint: String, created_at: Timestamp, index_fields: Map<String, FieldEntry>, expressions: Map<String, ExpressionEntry>, suggesters: Map<String, SuggesterEntry>, analysis_schemes: Map<String, AnalysisSchemeEntry>, access_policies: Option<OptionEntry<String>>, scaling_parameters: Option<OptionEntry<ScalingParameters>>, availability_options: Option<OptionEntry<bool>>, endpoint_options: Option<OptionEntry<DomainEndpointOptions>> }` (only `index_fields` is required by this first slice; the rest are placeholders for the later gap passes).
- `FieldEntry { pending: IndexField, deployed: Option<IndexField>, status: OptionEntry<()> }` where `OptionEntry<T> { value: T, creation_date: Timestamp, update_date: Timestamp, update_version: u32, state: OptionState /*RequiresIndexDocuments|Processing|Active|FailedToValidate*/, pending_deletion: bool }`.

All other operations (DefineExpression/DefineSuggester/DefineAnalysisScheme and their Describe/Delete siblings, BuildSuggesters, Update*/Describe* for availability, endpoint, scaling and access-policy options) stay stubbed at 501 for now and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-cloudsearch-a9searchcloudconfigservice2013/OPERATIONS.md` lists all 26 modeled operations with the exact input/output members and errors quoted above (awsQuery protocol, sdkId `CloudSearch`). `grep -rn '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` in that crate matches 25 handler files — every wired operation, including `create_domain.rs`, `define_index_field.rs`, `describe_index_fields.rs`, `index_documents.rs`, `describe_domains.rs`, `delete_domain.rs` — each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. Shape details (`DomainStatus` with required `DomainId`/`DomainName`/`RequiresIndexDocuments`, `IndexFieldStatus{Options,Status}`, `OptionStatus{CreationDate,UpdateDate,UpdateVersion,State,PendingDeletion}`) come from the Smithy model at `smithy-model/com-amazonaws-cloudsearch-a9searchcloudconfigservice2013.json`. CloudSearch's documented semantics ground the RequiresIndexDocuments/IndexDocuments deployment cycle. No tests assert any of this behavior (only the generated `tests/state_roundtrip.rs` exists).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `cloudsearch` client at it.
2. `CreateDomain(DomainName="test-domain")` → expect HTTP 200 with DomainStatus{DomainName=test-domain, DomainId set, ARN set, Created=true, RequiresIndexDocuments=false}; currently HTTP 501 NotImplemented.
3. `CreateDomain(DomainName="test-domain")` again → expect ResourceAlreadyExistsException (HTTP 409).
4. `ListDomainNames()` → expect the map to contain `test-domain`.
5. `DefineIndexField(DomainName="test-domain", IndexField={IndexFieldName="title", IndexFieldType="text"})` → expect IndexFieldStatus with Options.IndexFieldName=title and Status.State=RequiresIndexDocuments.
6. `DescribeDomains(DomainNames=["test-domain"])` → expect DomainStatusList[0].RequiresIndexDocuments=true.
7. `DescribeIndexFields(DomainName="test-domain")` → expect exactly the `title` field just defined; repeat with FieldNames=["missing"] and expect an empty list.
8. `IndexDocuments(DomainName="test-domain")` → expect FieldNames=["title"], and a following DescribeIndexFields(Deployed=true) to show Status.State=Active and DescribeDomains to show RequiresIndexDocuments=false.
9. `DeleteIndexField(DomainName="test-domain", IndexFieldName="title")` → expect the field's status with PendingDeletion=true; `DeleteIndexField` again → ResourceNotFoundException (409).
10. `DeleteDomain(DomainName="test-domain")` → expect DomainStatus with Deleted=true; then `ListDomainNames()` must not list it and `DescribeIndexFields(DomainName="test-domain")` must raise ResourceNotFoundException (409).
