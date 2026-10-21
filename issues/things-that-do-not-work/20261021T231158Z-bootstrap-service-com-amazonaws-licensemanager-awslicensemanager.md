Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 0761ec9a912f1ad457725ac2bfa14369ca92ad04

# Bootstrap the License Manager emulator

_Filed: 2026-10-21T23:11:58Z_
_Operation: service-com-amazonaws-licensemanager-awslicensemanager | Class: missing_

> _Note: this issue was identified against the code base at commit `0761ec9a912f1ad457725ac2bfa14369ca92ad04`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 62 operations in `services/service-com-amazonaws-licensemanager-awslicensemanager/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST, SMALL stateful slice: the self-managed license-configuration lifecycle, which is the primary resource of License Manager (awsJson1_1, `POST /` with `X-Amz-Target: AWSLicenseManager.<Operation>`).

Happy path to implement, in order (fields from OPERATIONS.md):

1. **CreateLicenseConfiguration** — input `Name` (required), `LicenseCountingType` (required, one of Core | Instance | Socket | vCPU), optional `Description`, `LicenseCount`, `LicenseCountHardLimit`, `LicenseRules`, `Tags`, `DisassociateWhenNotFound`, `ProductInformationList`, `LicenseExpiry`. Must mint and PERSIST a new record keyed by a generated id `lic-<32 hex>` and ARN `arn:aws:license-manager:<region>:<account>:license-configuration:lic-<32 hex>`, returning that ARN in `LicenseConfigurationArn`. Two creates must yield two distinct stored configurations.
2. **GetLicenseConfiguration** — input `LicenseConfigurationArn` (required). Must look the record up in state and echo back exactly what was stored: `LicenseConfigurationId`, `LicenseConfigurationArn`, `Name`, `Description`, `LicenseCountingType`, `LicenseCount`, `LicenseCountHardLimit`, `LicenseRules`, `Tags`, `ProductInformationList`, `DisassociateWhenNotFound`, `LicenseExpiry`, plus computed `Status` ("AVAILABLE" for a fresh config), `OwnerAccountId`, `ConsumedLicenses` (0 when nothing is associated) and empty `ConsumedLicenseSummaryList` / `ManagedResourceSummaryList`. An unknown ARN must fail with `InvalidParameterValueException` (HTTP 400), not a 200 with empty fields.
3. **ListLicenseConfigurations** — input `LicenseConfigurationArns`, `MaxResults`, `NextToken`, `Filters`. Must enumerate the stored configurations as `LicenseConfigurations` (list<LicenseConfiguration>, same per-item fields as Get), honour `LicenseConfigurationArns` as an ARN filter, truncate at `MaxResults` and return an opaque `NextToken` that resumes exactly where the page ended (and no `NextToken` on the last page).
4. **UpdateLicenseConfiguration** — input `LicenseConfigurationArn` (required) plus `Name`, `Description`, `LicenseCount`, `LicenseCountHardLimit`, `LicenseRules`, `ProductInformationList`, `DisassociateWhenNotFound`, `LicenseExpiry`, `LicenseConfigurationStatus` (AVAILABLE | DISABLED). Must mutate the stored record in place so the next Get/List reflects the change; omitted members are left unchanged. Unknown ARN -> `InvalidParameterValueException` (400).
5. **TagResource / ListTagsForResource** — `TagResource` stores `Tags` (list<Tag> of Key/Value) against the configuration ARN; `ListTagsForResource` returns exactly the tags stored for that ARN (merging tags supplied at create time). Unknown ARN -> `InvalidParameterValueException` (400).
6. **DeleteLicenseConfiguration** — input `LicenseConfigurationArn` (required), empty output. Must remove the record from state; a subsequent `GetLicenseConfiguration` must fail with `InvalidParameterValueException` (400) and `ListLicenseConfigurations` must no longer include it. Deleting an unknown ARN must also error, not succeed silently.

NEVER STUBS: each of these must compute its response from the persisted store — no hardcoded ARNs, no echoing the request back without storing it, no random id that is never written. A create followed by a get/list in a separate request must return what was created, and a delete must be observable by later reads.

## State Model

In `src/state.rs` (`ComAmazonawsLicensemanagerAwslicensemanagerState`, reached via `crate::state::service_state(&*self.engine)`):

- `license_configurations: HashMap<String /* LicenseConfigurationArn */, LicenseConfiguration>` where
  `LicenseConfiguration { id: String /* lic-<hex> */, arn: String, name: String, description: Option<String>, license_counting_type: LicenseCountingType, license_count: Option<i64>, license_count_hard_limit: Option<bool>, license_rules: Vec<String>, product_information_list: Vec<ProductInformation>, disassociate_when_not_found: Option<bool>, license_expiry: Option<i64>, status: String /* AVAILABLE | DISABLED */, owner_account_id: String, consumed_licenses: i64, consumed_license_summary_list: Vec<ConsumedLicenseSummary>, managed_resource_summary_list: Vec<ManagedResourceSummary>, created_at: SystemTime }`
- `tags: HashMap<String /* resource ARN */, BTreeMap<String /* Key */, String /* Value */>>` — shared by TagResource / UntagResource / ListTagsForResource and seeded from the `Tags` passed to CreateLicenseConfiguration.
- `list_tokens: HashMap<String /* NextToken */, usize /* offset into the ordered ARN list */>` (or an encoded offset token) so ListLicenseConfigurations paginates deterministically.
- Insertion order preserved (e.g. an auxiliary `config_order: Vec<String>`) so list paging is stable across calls.

Out of scope for this first slice (stay 501, filed separately later): granted licenses (CreateLicense/GetLicense/ListLicenses/CreateLicenseVersion), grants (CreateGrant/AcceptGrant/RejectGrant/DeleteGrant), checkout/check-in (CheckoutLicense/CheckoutBorrowLicense/CheckInLicense/ExtendLicenseConsumption), tokens (CreateToken/GetAccessToken/ListTokens/DeleteToken), report generators, license asset groups/rulesets, conversion tasks, resource inventory, usage/association listings and service settings.

Done = a client script that creates a license configuration, gets it back with identical fields, lists it (including a MaxResults=1 page + NextToken follow-up with two configs), updates its LicenseCount and Name and observes the change, tags it and lists the tags, then deletes it and observes a 400 `InvalidParameterValueException` on the follow-up get — all against a single running emulator process.

## Evidence

`OPERATIONS.md` (crate root, generated from `smithy-model/com-amazonaws-licensemanager-awslicensemanager.json`) documents the wire protocol (awsJson1_1, target `AWSLicenseManager.<Op>`) and these contracts: CreateLicenseConfiguration lines 81-85 (required `Name`, `LicenseCountingType`; output `LicenseConfigurationArn`), GetLicenseConfiguration lines 189-193 (required `LicenseConfigurationArn`; full output field list incl. `ConsumedLicenses`, `Status`, `OwnerAccountId`), ListLicenseConfigurations lines 255-259 (`LicenseConfigurationArns`/`MaxResults`/`NextToken`/`Filters` -> `LicenseConfigurations`, `NextToken`), UpdateLicenseConfiguration lines 375-379 (required ARN, mutable members, empty output), DeleteLicenseConfiguration lines 135-139 (required ARN, empty output). All of these declare `InvalidParameterValueException (HTTP 400)` for a bad/unknown ARN. Handlers are unmodified scaffolds: e.g. `src/ops/create_license_configuration.rs` line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 189 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateLicenseConfiguration"))`; the same marker is present in all 62 files under `src/ops/`. No tests exercise any of these operations (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with service `license-manager`.
2. `aws license-manager create-license-configuration --name demo-config --license-counting-type vCPU --license-count 10 --license-count-hard-limit --tags Key=env,Value=dev` -> expect 200 with a `LicenseConfigurationArn`; today returns HTTP 501 NotImplemented.
3. `aws license-manager get-license-configuration --license-configuration-arn <arn>` -> expect the stored Name/LicenseCountingType/LicenseCount/LicenseCountHardLimit/Tags, `Status=AVAILABLE`, `ConsumedLicenses=0`.
4. Create a second configuration, then `aws license-manager list-license-configurations --max-results 1` -> expect exactly one item plus a `NextToken`; pass that token to get the second item and no token.
5. `aws license-manager update-license-configuration --license-configuration-arn <arn> --name demo-renamed --license-count 20`, then get again -> expect the new Name and LicenseCount persisted.
6. `aws license-manager tag-resource --resource-arn <arn> --tags Key=team,Value=plat` then `aws license-manager list-tags-for-resource --resource-arn <arn>` -> expect both `env=dev` and `team=plat`.
7. `aws license-manager delete-license-configuration --license-configuration-arn <arn>`, then get it again -> expect HTTP 400 `InvalidParameterValueException`, and `list-license-configurations` must no longer include it.
