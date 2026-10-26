Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6f4880dda1aa7f89588445b4a5ba9a411741294f

# Reject Tags whose keys use the reserved aws: prefix on CreateConfigurationManager

_Filed: 2026-10-24T17:05:59Z_
_Operation: CreateConfigurationManager | Class: validation_

> _Note: this issue was identified against the code base at commit `6f4880dda1aa7f89588445b4a5ba9a411741294f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateConfigurationManager` is the only entry point through which this emulator can attach tags to a configuration manager (`TagResource` is still a scaffold), and it stores whatever `Tags` map the request carries without checking AWS's reserved tag-key namespace.

Expected: a request whose `Tags` map carries a key in the `aws:` namespace (any case, e.g. `aws:createdBy`, `AWS:owner`) is refused with 400 `ValidationException` and NO configuration manager is created — the `aws:` prefix is reserved for the tags AWS itself applies, and a caller may neither assign nor edit one.

Actual: the handler runs only `definitions_rejection(&input)` (empty-list, type-catalog, size caps and the IAM-role-ARN form) and then stores the map verbatim via `crate::state::to_btree_map(input.tags)`. The call answers 200 with a fresh `ManagerArn`, and `GetConfigurationManager` then reports the reserved tag back in its `Tags` member, so the emulator persists a tag the real service would never have accepted (and would never let a caller remove through `UntagResource` either).

The modelled `TagsMap` key pattern `^[A-Za-z0-9 _=@:.+-/]+$` admits `:`, so the boundary validator cannot express this rule — it has to live in the handler, exactly as the service's other model-inexpressible rules (type catalog, `IAMRoleArn` form) already do in `crate::state`.

## Evidence

Spec/well-known grounding: the Smithy model defines `CreateConfigurationManagerInput$Tags` as `TagsMap` (sensitive map, key `@length(1,128)` + `@pattern ^[A-Za-z0-9 _=@:.+-/]+$`) — the pattern permits `aws:`-prefixed keys, so nothing at the modelled boundary refuses them. AWS-wide tagging rule (AWS General Reference, "Tag naming limits and requirements"): the `aws:` prefix is reserved for AWS use and a caller can neither create nor edit such a tag; AWS tagging APIs answer a client validation error. This repo already treats that rule as the service-independent one it is — e.g. `service-com-amazonaws-acm-certificatemanager/src/state.rs:1849` (`RESERVED_TAG_PREFIX`), `service-com-amazonaws-secretsmanager-secretsmanager/src/ops/create_secret.rs:293`, `service-com-amazonaws-networkmanager-networkmanager/src/state.rs:2882`, `service-com-amazonaws-licensemanagerusersubscriptions/src/state.rs:276` (case-insensitive comparison).

Handler evidence: `services/service-com-amazonaws-ssmquicksetup-quicksetup/src/ops/create_configuration_manager.rs` — `definitions_rejection()` (lines 128-196) checks the definitions list, per-definition `Parameters` count, `Type` catalog membership, `Tags` COUNT only (`too_many("Tags", ...MAX_TAGS)`) and `LocalDeploymentAdministrationRoleArn` form; no key inspection. The map is then stored unexamined at line 326 (`tags: crate::state::to_btree_map(input.tags)`). `crate::state` defines `MAX_TAGS` but no reserved-prefix helper (grep for `RESERVED_TAG_PREFIX` / `aws:` in this crate's `src/` returns nothing).

Test coverage: none. `tests/all/resource_tags.rs` exercises create-with-tags, `UntagResource` removal, idempotency and an oversized `TagKeys` list, but no row uses an `aws:`-prefixed key (grep for `aws:` in this crate's tests matches only IAM/ssm-quicksetup ARNs). This gap is not covered by any open issue: the existing ssm-quicksetup error-paths issue covers the missing TargetAccounts/TargetOrganizationalUnits cross-member rule, the anchored-pattern message rendering and the 401 front-door paths, and says nothing about tag keys.

## Steps to Reproduce

1. Boot the emulator and sign requests for us-west-2.
2. POST /configurationManager with body {"Name":"reserved-tags","ConfigurationDefinitions":[{"Type":"AWSQuickSetupType-SSMHostMgmt","Parameters":{"TargetOrganizationalUnits":"ou-1","TargetRegions":"us-west-2"}}],"Tags":{"aws:createdBy":"quicksetup"}}.
3. Observe 200 and a minted ManagerArn; the real service refuses the reserved key with 400 ValidationException and creates nothing.
4. GET /configurationManager/<ManagerArn>: the response's Tags member reports {"aws:createdBy":"quicksetup"} — a reserved tag now persisted in emulator state.
5. Repeat with an upper-case variant ("AWS:Owner") to confirm the check must be case-insensitive.
