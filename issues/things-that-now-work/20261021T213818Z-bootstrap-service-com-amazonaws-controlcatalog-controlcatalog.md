Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the ControlCatalog emulator

_Filed: 2026-10-21T21:38:18Z_
_Operation: service-com-amazonaws-controlcatalog-controlcatalog | Class: stub_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 6 modeled ControlCatalog operations is an unmodified scaffold answering HTTP 501, so no client can browse the catalog at all. ControlCatalog is a READ-ONLY catalog service (all 6 ops are @readonly, there are no create/update/delete APIs), so the bootstrap slice is: seed a small but realistic catalog into service state at startup, then serve all reads from that persisted catalog — never from canned per-request literals.

Happy path to implement, in order (restJson1, POST routes, sigv4 signing name `controlcatalog`):

1. **ListDomains** — `POST /domains`. Input: `MaxResults` (1..100, @httpQuery maxResults), `NextToken` (<=1024, @httpQuery nextToken). Output: `Domains` (required list of `DomainSummary{Arn, Name, Description, CreateTime, LastUpdateTime}` — all required) + `NextToken` when more pages remain. Domain ARNs must match `arn:aws:controlcatalog:::domain/<lowercase-alnum>`.
2. **ListObjectives** — `POST /objectives`. Input: `MaxResults` (1..100), `NextToken`, `ObjectiveFilter{Domains:[{Arn}]}`. Output: `Objectives` (required list of `ObjectiveSummary{Arn, Name, Description, Domain:AssociatedDomainSummary{Arn,Name}, CreateTime, LastUpdateTime}`) + `NextToken`. The filter must actually restrict results to objectives whose stored domain ARN matches; an objective's `Domain` must be the real domain from state, not a fabricated one.
3. **ListCommonControls** — `POST /common-controls`. Input: `MaxResults` (1..100), `NextToken`, `CommonControlFilter{Objectives:[{Arn}]}`. Output: `CommonControls` (required list of `CommonControlSummary{Arn, Name, Description, Domain, Objective:AssociatedObjectiveSummary{Arn,Name}, CreateTime, LastUpdateTime}`) + `NextToken`. The objective filter must restrict results from state.
4. **ListControls** — `POST /list-controls`. Input: `MaxResults` (1..100), `NextToken`, `Filter{Implementations{...}, GovernedProviders[1..1]}`. Output: `Controls` (required list of `ControlSummary{Arn, Aliases, Name, Description, Behavior, Severity, ParameterRequirementSummary, Implementation:ImplementationSummary, CreateTime, GovernedResources, GovernedProviders}`) + `NextToken`. Each summary must be projected from the SAME stored control record that GetControl reads.
5. **GetControl** — `POST /get-control`. Input: `ControlArn` (required, len 34..2048, pattern `^arn:(aws(?:[-a-z]*)?):(controlcatalog|controltower):[a-zA-Z0-9-]*::control/[0-9a-zA-Z_\-]+$`). Output: `Arn`, `Name`, `Description`, `Behavior` (DETECTIVE|PREVENTIVE|PROACTIVE), `RegionConfiguration{Scope, DeployableRegions}` all required, plus `Aliases`, `Severity`, `Implementation`, `ParameterRequirementSummary`, `Parameters`, `CreateTime`, `GovernedResources`, `GovernedProviders`. A well-formed ARN that is not in the seeded catalog must return `ResourceNotFoundException` (HTTP 404); a malformed/short ARN must return `ValidationException` (HTTP 400). The returned fields must be exactly those stored for that control (so an ARN taken from ListControls round-trips identically).
6. **ListControlMappings** — `POST /list-control-mappings`. Input: `MaxResults` (1..1000), `NextToken`, `Filter{ControlArns[1..1], CommonControlArns[1..1], MappingTypes[1..1]}`. Output: `ControlMappings` (required list of `ControlMapping{ControlArn, MappingType, Mapping}`) + `NextToken`. Mappings must reference control ARNs and common-control ARNs that exist in state, and each filter dimension must narrow the result set.

Cross-cutting requirements for the slice:
- **Pagination must be real for all five list operations**: honor `MaxResults` (clamped to the modeled range — out-of-range values are `ValidationException`), return an opaque `NextToken` (<=1024 chars) only when more items remain, and resume exactly after the last returned item when the token is replayed. Iterating with `MaxResults=1` must yield every item exactly once with no duplicates and a final page with no `NextToken`.
- **No canned or echoed responses.** Every response field must be read out of the seeded catalog in state. A hardcoded list literal inside a handler, or a GetControl that echoes the requested ARN with invented Name/Description, counts as not implemented.
- Referential integrity: objective -> domain, common control -> objective/domain, and control mapping -> control/common control must all resolve to entries that the corresponding list/get operation also returns.
- Errors use the modeled shapes: `ValidationException` 400, `ResourceNotFoundException` 404 (GetControl), `AccessDeniedException` 403, `ThrottlingException` 429, `InternalServerException` 500.

## State Model

All of it lives in `ComAmazonawsControlcatalogControlcatalogState` (`src/state.rs`), reached via `crate::state::service_state(&*self.engine)` and seeded once with a deterministic catalog (stable ARNs and timestamps so repeated runs are reproducible):

- `domains: BTreeMap<String /*domain ARN*/, Domain { arn, name, description, create_time, last_update_time }>`
- `objectives: BTreeMap<String /*objective ARN*/, Objective { arn, name, description, domain_arn, create_time, last_update_time }>`
- `common_controls: BTreeMap<String /*common control ARN*/, CommonControl { arn, name, description, objective_arn, domain_arn, create_time, last_update_time }>`
- `controls: BTreeMap<String /*control ARN*/, Control { arn, aliases: Vec<String>, name, description, behavior: ControlBehavior, severity: Option<ControlSeverity>, region_configuration: RegionConfig { scope, deployable_regions: Vec<String> }, implementation: Option<ImplementationDetails { r#type, identifier, minimum_supported_region_count, regions }>, parameter_requirement_summary: Option<ParameterRequirementSummary>, parameters: Vec<ControlParameter { name }>, create_time, governed_resources: Vec<String>, governed_providers: Vec<String> }>`
- `control_mappings: Vec<ControlMappingRecord { control_arn, mapping_type: MappingType, mapping /* framework or common-control mapping details */ }>`

BTreeMap ordering (or an explicit insertion-order `Vec<String>` index per collection) gives the stable total order pagination tokens need. A `NextToken` is an opaque encoding of the collection name plus the last-returned key; decoding a token that does not belong to the requested collection/filter is a `ValidationException`.

Operations outside this slice: none — these 6 are the whole service surface, and this bootstrap is complete when all 6 serve from state.

## Evidence

`services/service-com-amazonaws-controlcatalog-controlcatalog/OPERATIONS.md` lists exactly 6 operations (ListDomains, ListObjectives, ListCommonControls, ListControls, GetControl, ListControlMappings) with the routes, input/output members, @paginated traits and error sets quoted above. `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches all 6 handler files (get_control.rs, list_domains.rs, list_objectives.rs, list_common_controls.rs, list_controls.rs, list_control_mappings.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501. Field shapes (DomainSummary, ObjectiveSummary, CommonControlSummary, ControlSummary, ControlMapping, RegionConfiguration, and the four filter structures with their 1..1 list length constraints) were taken from the Smithy model via smithy_query against `smithy-model/com-amazonaws-controlcatalog-controlcatalog.json`. README.md documents the stub marker and the shared-engine state API.

## Steps to Reproduce

1. Start the emulator and point the AWS CLI/SDK at it with signing name `controlcatalog`.
2. `aws controlcatalog list-domains` — currently HTTP 501; must return the seeded `Domains` with required Arn/Name/Description/CreateTime/LastUpdateTime.
3. `aws controlcatalog list-objectives --objective-filter Domains=[{Arn=<arn from step 2>}]` — must return only objectives of that domain, each with a `Domain` matching the step-2 entry.
4. `aws controlcatalog list-common-controls --common-control-filter Objectives=[{Arn=<arn from step 3>}]` — must return only common controls under that objective.
5. `aws controlcatalog list-controls --max-results 1` and follow `NextToken` to the end — every control appears exactly once, the final page has no NextToken.
6. `aws controlcatalog get-control --control-arn <arn from step 5>` — must return the same Name/Description/Behavior/Severity/Implementation as the list summary, plus required RegionConfiguration.
7. `aws controlcatalog get-control --control-arn arn:aws:controlcatalog:::control/doesnotexist0` — must return ResourceNotFoundException (HTTP 404), not 501 and not a fabricated control.
8. `aws controlcatalog get-control --control-arn not-an-arn` — must return ValidationException (HTTP 400).
9. `aws controlcatalog list-control-mappings --filter ControlArns=[<arn from step 6>]` — must return only mappings for that control, with ControlArn/MappingType/Mapping populated from state.
10. Repeat steps 2-9 in a second process/session against the same running emulator — identical results, proving the catalog is persisted shared state rather than per-request literals.
