Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the securityhub emulator

_Filed: 2026-10-21T21:28:41Z_
_Operation: service-com-amazonaws-securityhub-securityhubapiservice | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-securityhub-securityhubapiservice/src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`. `src/state.rs` holds no fields (`// TODO: add service-specific state fields`), so the service holds zero state. No client workflow can succeed.

Implement a FIRST, SMALL, stateful slice: the enable-hub / import-findings / query-findings / update-findings / disable-hub core loop of Security Hub. Everything else stays 501 and is filed separately later.

### Happy path to implement (in order)

1. **EnableSecurityHub** — `POST /accounts`. Input: `Tags` (map<string,string>, 1..50), `EnableDefaultStandards` (boolean), `ControlFindingGenerator` (`SECURITY_CONTROL` | `STANDARD_CONTROL`). Output has no members. Must CREATE the hub record for the caller's account/region: synthesize `HubArn` = `arn:aws:securityhub:<region>:<account>:hub/default`, record `SubscribedAt` as the current time (ISO-8601), store `AutoEnableControls` (default true), store `ControlFindingGenerator` (default `STANDARD_CONTROL`), and store the `Tags`. If a hub already exists for the account/region, return the modelled `ResourceConflictException` (HTTP 409) — not a second success.

2. **DescribeHub** — `GET /accounts` (optional `HubArn` query param). Output: `HubArn`, `SubscribedAt`, `AutoEnableControls`, `ControlFindingGenerator`. Must be computed from the record EnableSecurityHub persisted — the same `HubArn` and the same `SubscribedAt` on every call, and the `ControlFindingGenerator` that was passed in at enable time. When no hub has been enabled (or after DisableSecurityHub), return `InvalidAccessException` (HTTP 401), which is how real Security Hub reports "account not subscribed".

3. **BatchImportFindings** — `POST /findings/import`. Input: `Findings` (list<AwsSecurityFinding>, len 1..100, required). Output: `SuccessCount` (integer, required), `FailedCount` (integer, required), `FailedFindings` (list<ImportFindingsError>). Must STORE each accepted finding keyed by its `(ProductArn, Id)` pair, preserving the full submitted finding body (`AwsAccountId`, `GeneratorId`, `Title`, `Description`, `Types`, `Severity`, `Resources`, `CreatedAt`, `UpdatedAt`, `Workflow`, `RecordState`, `UserDefinedFields`, ...). Re-importing the same `(ProductArn, Id)` UPDATES the stored finding rather than creating a duplicate. `SuccessCount`/`FailedCount` must be real counts derived from what was stored, and each rejected finding must appear in `FailedFindings` with its `Id`, `ProductArn`, `ErrorCode` and `ErrorMessage` — never a hardcoded `SuccessCount` equal to the input length.

4. **GetFindings** — `POST /findings`. Input: `Filters` (AwsSecurityFindingFilters), `SortCriteria`, `NextToken`, `MaxResults` (1..100). Output: `Findings` (list<AwsSecurityFinding>, required), `NextToken`. Must return the findings persisted by BatchImportFindings, byte-for-byte on the fields that were imported. A first slice may support a small, useful subset of `Filters` (`Id`, `ProductArn`, `AwsAccountId`, `SeverityLabel`, `RecordState`, `WorkflowStatus` string filters honouring `Comparison` = `EQUALS`/`PREFIX`) plus `MaxResults`/`NextToken` paging over a stable order (newest `UpdatedAt` first). Paging must actually truncate at `MaxResults` and resume from the opaque `NextToken`, and omit `NextToken` on the last page. An empty/absent `Filters` returns all stored findings.

5. **BatchUpdateFindings** — `PATCH /findings/batchupdate`. Input: `FindingIdentifiers` (list<AwsSecurityFindingIdentifier> = `{Id, ProductArn}`, required), `Note` (NoteUpdate), `Severity` (SeverityUpdate), `VerificationState`, `Confidence` (0..100), `Criticality` (0..100), `Types`, `UserDefinedFields`, `Workflow` (WorkflowUpdate), `RelatedFindings`. Output: `ProcessedFindings` (required), `UnprocessedFindings` (required). Must MUTATE the stored findings in place: a subsequent GetFindings must show the new `Workflow.Status`, `Severity.Label`, `Note.Text`/`Note.UpdatedBy`/`Note.UpdatedAt`, `Confidence`, `Criticality`, etc. Identifiers that match no stored finding go into `UnprocessedFindings` with an `ErrorCode`/`ErrorMessage` and are NOT reported as processed.

6. **DisableSecurityHub** — `DELETE /accounts`. No input, no output members. Must DELETE the hub record and its findings for the account/region; a following DescribeHub must then fail with `InvalidAccessException` (HTTP 401) and a following DisableSecurityHub with `ResourceNotFoundException` (HTTP 404) — proving the delete really removed state.

### NEVER STUBS

Every one of the six must compute its response from persisted state held in the shared engine (reached via `crate::state::service_state(&*self.engine)` — never a freshly constructed engine). Hardcoded counts, echoed inputs, and generated-ARN-without-storage are failures: an EnableSecurityHub followed by DescribeHub must return exactly what was enabled, and a BatchImportFindings followed by GetFindings must return exactly the findings that were imported. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker from each of the six files as it is implemented.

## State Model

In `ComAmazonawsSecurityhubSecurityhubapiserviceState`:

- `hubs: HashMap<HubKey, Hub>` where `HubKey = (account_id, region)`.
  - `Hub { hub_arn: String, subscribed_at: String /* ISO-8601 */, auto_enable_controls: bool, control_finding_generator: String /* SECURITY_CONTROL|STANDARD_CONTROL */, enable_default_standards: bool, tags: HashMap<String,String> }`
- `findings: HashMap<FindingKey, StoredFinding>` where `FindingKey = (product_arn, finding_id)` — the modelled `AwsSecurityFindingIdentifier`.
  - `StoredFinding { finding: AwsSecurityFinding /* full submitted body, mutated in place by BatchUpdateFindings */, imported_at: SystemTime, updated_at: SystemTime }`
- `finding_order: Vec<FindingKey>` (or sort on read by `updated_at`) to give GetFindings a stable, deterministic page order for `NextToken` paging.
- `next_tokens: HashMap<String, usize>` (or an encoded offset/cursor) so GetFindings' `NextToken` resumes a real position rather than restarting.

Tags stored on the hub back the later `ListTagsForResource`/`TagResource` slice; findings stored here back the later `GetFindingHistory`, `GetFindingStatisticsV2` and insight operations.

All other operations (standards, insights, members/organizations, automation rules, configuration policies, connectors, aggregators and all the *V2 surface) remain 501 for now and will be filed as individual gap issues.

## Evidence

1. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-securityhub-securityhubapiservice/src/ops/` matches every operation file in the crate (124 files), including `enable_security_hub.rs`, `describe_hub.rs`, `batch_import_findings.rs`, `get_findings.rs`, `batch_update_findings.rs` and `disable_security_hub.rs`. Each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501.
2. `services/service-com-amazonaws-securityhub-securityhubapiservice/src/state.rs` declares `ComAmazonawsSecurityhubSecurityhubapiserviceState` with no fields but a `// TODO: add service-specific state fields` comment — zero state is held, so nothing could survive between calls even if a handler answered 200.
3. Operation contracts, routes, required members and modelled errors quoted above come from the crate's `OPERATIONS.md` (generated from the Smithy model): BatchImportFindings (line 90, `POST /findings/import`, `Findings` len 1..100 required, `FailedCount`/`SuccessCount` required), BatchUpdateFindings (line 104, `PATCH /findings/batchupdate`, `FindingIdentifiers` required, `ProcessedFindings`/`UnprocessedFindings` required), DescribeHub (line 294, `GET /accounts`, outputs `HubArn`/`SubscribedAt`/`AutoEnableControls`/`ControlFindingGenerator`), DisableSecurityHub (line 361, `DELETE /accounts`, errors include `ResourceNotFoundException` 404), EnableSecurityHub (line 418, `POST /accounts`, errors include `ResourceConflictException` 409), GetFindings (line 526, `POST /findings`, `@paginated(inputToken=NextToken, outputToken=NextToken, items=Findings)`, `MaxResults` range 1..100).
4. Well-established service semantics: a finding's identity in Security Hub is the `(ProductArn, Id)` pair (the `AwsSecurityFindingIdentifier` shape used by BatchUpdateFindings), re-importing the same identifier updates the existing finding, and calling a Security Hub API on an account that has not been subscribed fails with `InvalidAccessException`.
5. No tests exercise these operations: the crate's only test is the generated `tests/state_roundtrip.rs` state-API example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI securityhub client at it.
2. `aws securityhub enable-security-hub --enable-default-standards --control-finding-generator SECURITY_CONTROL --tags Team=sec` — expect HTTP 200 with an empty body; today returns 501 NotImplemented.
3. `aws securityhub describe-hub` — expect 200 with the HubArn created in step 2, a stable SubscribedAt timestamp, AutoEnableControls and ControlFindingGenerator=SECURITY_CONTROL; today 501.
4. `aws securityhub enable-security-hub` a second time — expect ResourceConflictException (HTTP 409).
5. `aws securityhub batch-import-findings --findings file://finding.json` with one finding (ProductArn `arn:aws:securityhub:us-east-1::product/aws/securityhub`, Id `finding-1`, SeverityLabel HIGH, RecordState ACTIVE) — expect SuccessCount=1, FailedCount=0, FailedFindings empty; today 501.
6. `aws securityhub get-findings --filters '{"Id":[{"Value":"finding-1","Comparison":"EQUALS"}]}'` — expect exactly the imported finding with the Title, Description, Severity and Resources that were submitted; today 501.
7. `aws securityhub batch-update-findings --finding-identifiers Id=finding-1,ProductArn=arn:aws:securityhub:us-east-1::product/aws/securityhub --workflow Status=SUPPRESSED --note Text='triaged',UpdatedBy=analyst` — expect ProcessedFindings containing that identifier and UnprocessedFindings empty; today 501.
8. `aws securityhub get-findings` again — expect the stored finding to now report Workflow.Status=SUPPRESSED and Note.Text='triaged' (the update persisted).
9. `aws securityhub batch-update-findings --finding-identifiers Id=does-not-exist,ProductArn=arn:aws:securityhub:us-east-1::product/aws/securityhub --workflow Status=RESOLVED` — expect the identifier in UnprocessedFindings with an ErrorCode, and NOT in ProcessedFindings.
10. Import 5 findings, then `aws securityhub get-findings --max-results 2` — expect 2 findings plus a NextToken; pass that token back and expect the next 2 with no repeats, and no NextToken on the final page.
11. `aws securityhub disable-security-hub` — expect 200; then `aws securityhub describe-hub` must fail with InvalidAccessException (401) and `aws securityhub disable-security-hub` again must fail with ResourceNotFoundException (404), proving the state was really removed.
