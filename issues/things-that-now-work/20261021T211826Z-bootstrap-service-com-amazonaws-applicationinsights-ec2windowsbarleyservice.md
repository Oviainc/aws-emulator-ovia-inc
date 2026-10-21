Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the applicationinsights-ec2windowsbarleyservice emulator

_Filed: 2026-10-21T21:18:26Z_
_Operation: service-com-amazonaws-applicationinsights-ec2windowsbarleyservice | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 33 operations in `src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the service holds no state. Nothing works end to end.

This issue scopes the FIRST, small slice: the application + component lifecycle of Amazon CloudWatch Application Insights, implemented statefully over the `awsJson1_1` wire protocol (`POST /`, `X-Amz-Target: EC2WindowsBarleyService.<OperationName>`).

### Happy path to implement (in order)

1. **CreateApplication** — input `ResourceGroupName` (1..256, `^[a-zA-Z0-9\.\-_]*$`), `OpsCenterEnabled`, `CWEMonitorEnabled`, `OpsItemSNSTopicArn`, `SNSNotificationArn`, `AutoConfigEnabled`, `AttachMissingPermission`, `Tags`, `GroupingType`. Must persist a new application keyed by `ResourceGroupName` and return `ApplicationInfo` populated from what was stored: `ResourceGroupName`, `AccountId` (the caller's 12-digit account), `LifeCycle` (e.g. `ACTIVE`), `OpsCenterEnabled`, `CWEMonitorEnabled`, `AutoConfigEnabled`, `OpsItemSNSTopicArn`, `SNSNotificationArn`, `AttachMissingPermission`, `DiscoveryType`. Creating the same `ResourceGroupName` twice must fail with `ResourceInUseException` (HTTP 400).
2. **DescribeApplication** — input `ResourceGroupName` (required), optional `AccountId`. Must look the application up in state and return the SAME `ApplicationInfo` that CreateApplication returned (including any later UpdateApplication edits). Unknown group name → `ResourceNotFoundException` (HTTP 404).
3. **ListApplications** — input `MaxResults` (1..40), `NextToken` (1..1024). Must return `ApplicationInfoList` built from the stored applications, honouring `MaxResults` as a page size and emitting/consuming an opaque `NextToken` so a second call continues where the first stopped (operation is `@paginated`). Must NOT return a fixed/empty list.
4. **CreateComponent** — input `ResourceGroupName`, `ComponentName` (1..128, `^[\d\w\-_\.+]*$`), `ResourceList` (required). Must store the component (with its resource list) under the parent application; empty output body. Unknown `ResourceGroupName` → `ResourceNotFoundException`; duplicate `ComponentName` → `ResourceInUseException`.
5. **DescribeComponent** — input `ResourceGroupName`, `ComponentName`. Must return `ApplicationComponent` { `ComponentName`, `Tier` (default `DEFAULT` until configured), `Monitor`, `ResourceType`, `OsType`, `ComponentRemarks` } plus `ResourceList` exactly as supplied to CreateComponent. Unknown component → `ResourceNotFoundException`. (Stretch, same data structure: **ListComponents** returning `ApplicationComponentList` with the same pagination rules as ListApplications.)
6. **DeleteApplication** — input `ResourceGroupName` (required). Must remove the application AND its components from state; empty output body. A following DescribeApplication must then fail with `ResourceNotFoundException` (HTTP 404), and ListApplications must no longer include it. Deleting an unknown group → `ResourceNotFoundException`.

### Hard requirements

- NEVER STUBS: each of the six must compute its response from persisted state. No canned `ApplicationInfo`, no echoing the request back without storing it, no generated IDs that are not written to the store.
- Required-member and constraint validation (length/pattern on `ResourceGroupName`, `ComponentName`, the ARNs; `MaxResults` range 1..40) must produce `ValidationException` (HTTP 400), not a 500 or a silent success.
- Errors must use the modeled shapes and their documented HTTP statuses (`ResourceNotFoundException` 404, `ResourceInUseException` 400, `ValidationException` 400).
- Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from each implemented handler file.
- Done = an integration test runs the full workflow below and asserts response BODIES (not just 2xx): the described application matches the created one field-for-field, the component's `ResourceList` round-trips, and the application is gone after delete.

## State Model

In `ComAmazonawsApplicationinsightsEc2windowsbarleyserviceState` (src/state.rs), reached via `crate::state::service_state(&*self.engine)`:

- `applications: BTreeMap<String /* ResourceGroupName */, Application>` — BTreeMap so list order is stable for pagination.
- `Application { resource_group_name: String, account_id: String, life_cycle: String /* "ACTIVE" */, ops_center_enabled: Option<bool>, cwe_monitor_enabled: Option<bool>, auto_config_enabled: Option<bool>, ops_item_sns_topic_arn: Option<String>, sns_notification_arn: Option<String>, attach_missing_permission: Option<bool>, discovery_type: Option<DiscoveryType>, remarks: Option<String>, grouping_type: Option<GroupingType>, tags: Vec<Tag>, components: BTreeMap<String /* ComponentName */, Component>, created_at: SystemTime }`
- `Component { component_name: String, resource_list: Vec<String>, resource_type: Option<String>, os_type: Option<OsType>, tier: Tier /* DEFAULT */, monitor: bool, component_configuration: Option<String>, component_remarks: Option<String>, workloads: BTreeMap<String /* WorkloadId */, WorkloadConfiguration> }`
- Pagination: `NextToken` is an opaque encoding of the last key returned (e.g. base64 of the `ResourceGroupName`/`ComponentName`), resolved against the BTreeMap on the next call — no server-side cursor table needed.

Later slices (log patterns, workloads, problems/observations, configuration history, tagging, the Update* and remaining Describe* calls) stay 501 and are filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-applicationinsights-ec2windowsbarleyservice/OPERATIONS.md` lists all 33 operations with their input/output members and errors (CreateApplication → `ApplicationInfo`; DescribeApplication requires `ResourceGroupName` and returns `ApplicationInfo` with `ResourceNotFoundException` 404; ListApplications is `@paginated(inputToken=NextToken, outputToken=NextToken)` with `MaxResults` range 1..40; CreateComponent requires `ResourceList` and declares `ResourceInUseException`; DescribeComponent returns `ApplicationComponent` + `ResourceList`; DeleteApplication requires `ResourceGroupName`). `smithy_query` on `ApplicationInfo` and `ApplicationComponent` confirms the member names used in the State Model above. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 33 handler files, including `create_application.rs`, `describe_application.rs`, `list_applications.rs`, `create_component.rs`, `describe_component.rs` and `delete_application.rs`, proving the service is all-stubs with zero state. README.md documents the `awsJson1_1` / `X-Amz-Target: EC2WindowsBarleyService.<Op>` routing and the shared-engine state API.

## Steps to Reproduce

1. Start the emulator and point an Application Insights client at it (awsJson1_1, POST / with X-Amz-Target: EC2WindowsBarleyService.<OperationName>).
2. CreateApplication { ResourceGroupName: "my-rg", OpsCenterEnabled: true, CWEMonitorEnabled: true, AutoConfigEnabled: true } — expect 200 with ApplicationInfo.ResourceGroupName == "my-rg" and the three flags echoed from stored state; today: HTTP 501 NotImplemented.
3. CreateApplication with the same ResourceGroupName again — expect ResourceInUseException (HTTP 400).
4. DescribeApplication { ResourceGroupName: "my-rg" } — expect the identical ApplicationInfo returned by step 2.
5. ListApplications { MaxResults: 1 } — expect ApplicationInfoList containing "my-rg"; with several applications created, expect a NextToken that pages through the rest without repeats.
6. CreateComponent { ResourceGroupName: "my-rg", ComponentName: "web-tier", ResourceList: ["arn:aws:ec2:us-east-1:123456789012:instance/i-0123456789abcdef0"] } — expect 200 empty body.
7. DescribeComponent { ResourceGroupName: "my-rg", ComponentName: "web-tier" } — expect ApplicationComponent.ComponentName == "web-tier" and ResourceList equal to what was supplied.
8. DescribeComponent for an unknown component name — expect ResourceNotFoundException (HTTP 404).
9. DeleteApplication { ResourceGroupName: "my-rg" } — expect 200 empty body.
10. DescribeApplication { ResourceGroupName: "my-rg" } again — expect ResourceNotFoundException (HTTP 404), and ListApplications no longer lists it.
