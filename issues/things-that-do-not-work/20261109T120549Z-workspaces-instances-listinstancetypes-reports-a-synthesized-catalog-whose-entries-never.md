Issue-Source: trace-sweep-divergence
Issue-Trace: workspaces-instances-catalog-list-instance-types
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: workspaces-instances-catalog-list-instance-types
Issue-Shape-Id: com.amazonaws.workspacesinstances#EUCMIFrontendAPIService
Issue-Key: workspaces-instances|ListInstanceTypes|2xx->2xx|$.InstanceTypes[*]
Issue-Commit: a0b5d56b12e90d4bdac3fa9524e8b0649f05b4d6

# ListInstanceTypes reports a synthesized catalog whose entries never match the recorded instance types

> _Note: this issue was identified against the code base at commit `a0b5d56b12e90d4bdac3fa9524e8b0649f05b4d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceTypes

## Description
ONE cause, filed from one wire divergence cluster of the workspaces-instances emulator.

=== Cause 1 of 1: the instance-type catalog ListInstanceTypes publishes is built locally and matches no recorded entry ===

AWS: Smithy (model services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/smithy-model/com-amazonaws-workspacesinstances-eucmifrontendapiservice.json): ListInstanceTypes is @readonly and @paginated (inputToken/outputToken NextToken len 1..2048, pageSize MaxResults range 1..600, items InstanceTypes); output member InstanceTypes is @required, a list of InstanceTypeInfo{InstanceType (len 1..64, pattern ^([a-z0-9-]+)\.([a-z0-9]+)$), SupportedInstanceConfigurations: list of SupportedInstanceConfiguration{BillingMode (enum MONTHLY|HOURLY), PlatformType (enum Windows|Windows BYOL|Linux/UNIX|Ubuntu Pro Linux|Red Hat Enterprise Linux|Red Hat BYOL Linux|SUSE Linux), Tenancy (enum SHARED|DEDICATED)}}. SupportedInstanceConfigurations is documented as "all valid combinations of tenancy, platform type, and billing mode supported for the specific WorkSpace Instance type ... the complete set of configuration options available for this instance type" (smithy_query doc, InstanceTypeInfo$SupportedInstanceConfigurations). WHICH instance types the catalog holds, and the order of the configuration entries inside each one, are NOT modelled, so they are recorded behavior only: the fixture's unfiltered first page carries 597 InstanceTypes entries, the first of which is InstanceType "c5.12xlarge" whose SupportedInstanceConfigurations the recording reports in a fixed, non-grouped order beginning {BillingMode MONTHLY, PlatformType "Red Hat Enterprise Linux", Tenancy DEDICATED}, then {HOURLY, "Windows BYOL", SHARED}, then {HOURLY, "Red Hat BYOL Linux", SHARED}, then {MONTHLY, "Linux/UNIX", DEDICATED}, ... Status is 200.

Emulator: status matches (200), but the response body matches NONE of the recorded entries: the replay reports every one of the 597 expected paths $.InstanceTypes[0] .. $.InstanceTypes[596] as missing from the served response (divergence_key keys = $.InstanceTypes[0]..[596]; the parent $.InstanceTypes itself is NOT reported missing, so a list is served and it is its CONTENT that diverges entry for entry). The served catalog is synthesized in-process rather than reported from recorded service data: crate::state::instance_type_catalog() (services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/state.rs:1081-1098) expands INSTANCE_TYPE_FAMILIES (src/state.rs:611-1071, family x size ladders) and sorts by name, and each row is rendered by InstanceTypeSpec::supported_configurations()/info() (src/state.rs:415-439) as the nested cross product platform_types x tenancies x CATALOG_BILLING_MODES — i.e. for every row the configuration list starts {MONTHLY, "Windows", SHARED} (PLATFORM_TYPE_ENUM_VALUES order x INSTANCE_CONFIGURATION_TENANCY_ENUM_VALUES order x BILLING_MODE_VALUES order), not the order AWS publishes. Hypothesis (computed from the source, not from the replay output): the two differences are (a) the configuration list inside every entry is emitted in the model's declaration cross-product order instead of the service's published order, which alone makes every entry unequal to its recorded counterpart, and (b) the catalog MEMBERSHIP also differs — expanding the ladders in INSTANCE_TYPE_FAMILIES yields on the order of 588 <family>.<size> rows against the 597 names the recording reports, so some instance types AWS publishes for this service are absent (and/or some present ones are not AWS's).

Why: ListInstanceTypes is the catalog a client reads BEFORE CreateWorkspaceInstance to choose an instance type and a (BillingMode, PlatformType, Tenancy) combination, and the same rows back InstanceConfigurationFilter and the create-time capability check (crate::state::instance_type_spec / supports / supports_billing_mode). A client that enumerates the catalog, indexes into it, diffs it, or picks the first supported configuration for a type gets a different answer than AWS gives: a type AWS publishes can be missing (its create is then refused here but accepted on AWS), a type AWS does not publish can be offered, and the per-type configuration list a client reads positionally is in a different order. No error code or message is involved — both sides answer 200 — so this is a pure response-content divergence.

Fix: the catalog data and the projection behind this operation, not the pagination: crate::state::INSTANCE_TYPE_FAMILIES, crate::state::instance_type_catalog() and InstanceTypeSpec::supported_configurations()/info() in services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/state.rs must report the instance-type set AWS publishes for WorkSpaces Instances and each entry's SupportedInstanceConfigurations in the order the service publishes them, with the handler services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/ops/list_instance_types.rs continuing to page that data unchanged. Note: open issue 20261029T190356Z-workspaces-instances-6-fidelity-divergences-across-createvolume-listregions.md bundles other workspaces-instances causes (CreateVolume, ListRegions, ListWorkspaceInstances, the service-linked role name) and does NOT cover ListInstanceTypes; the replay reported no owner for this cluster.

Traces: workspaces-instances-catalog-list-instance-types#task#1 (ListInstanceTypes, verdict mismatch, expected 200 / actual 200, 597 missing InstanceTypes entries).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, data listener accepting; 27/32 steps passed, 4 mismatched).

Divergence cluster: service=workspaces-instances | ListInstanceTypes | status=200->200 | fixture workspaces-instances-catalog-list-instance-types, step task#1 (affected_count 1, stub_path empty, owned_by empty).

Expected (recorded): HTTP 200 with $.InstanceTypes holding 597 entries; $.InstanceTypes[0] = {'InstanceType': 'c5.12xlarge', 'SupportedInstanceConfigurations': [{'BillingMode': 'MONTHLY', 'PlatformType': 'Red Hat Enterprise Linux', 'Tenancy': 'DEDICATED'}, {'BillingMode': 'HOURLY', 'PlatformType': 'Windows BYOL', 'Tenancy': 'SHARED'}, {'BillingMode': 'HOURLY', 'PlatformType': 'Red Hat BYOL Linux', 'Tenancy': 'SHARED'}, {'BillingMode': 'MONTHLY', 'PlatformType': 'Linux/UNIX', 'Tenancy': 'DEDICATED'}, ... ]}.

Actual: HTTP 200, but the replay reports '$.InstanceTypes[0]: missing (expected {...})' and the same for every index through $.InstanceTypes[596] — i.e. not one recorded instance-type entry is matched by the served list. The parent path $.InstanceTypes is not itself reported missing, so the divergence is in the entries, not in the member's presence or name.

Source of the served values (checkout under inspection): src/state.rs:1081 instance_type_catalog() expands src/state.rs:611-1071 INSTANCE_TYPE_FAMILIES (family x size ladders at src/state.rs:457-589) and src/state.rs:415-431 InstanceTypeSpec::supported_configurations() emits platform x tenancy x billing in generated-enum order, so the first configuration of every entry is {MONTHLY, 'Windows', SHARED} rather than the recorded {MONTHLY, 'Red Hat Enterprise Linux', DEDICATED}; src/ops/list_instance_types.rs:184-223 pages that data with DEFAULT_INSTANCE_TYPES_MAX_RESULTS = 600 (src/state.rs:244), so the whole synthesized catalog is served on the first page.

## Steps to Reproduce
1. Boot the emulator from this checkout with the workspaces-instances service mounted.
2. Call workspaces-instances ListInstanceTypes with no InstanceConfigurationFilter, no MaxResults and no NextToken (the fixture's task#1 request).
3. Compare the response's InstanceTypes list with the recorded one: AWS's first page carries 597 entries starting with InstanceType 'c5.12xlarge'; check both the set of InstanceType names served and, for 'c5.12xlarge', the order and membership of SupportedInstanceConfigurations.
4. Observe that the emulator answers 200 but no served entry equals its recorded counterpart: the configuration list of every entry begins {BillingMode 'MONTHLY', PlatformType 'Windows', Tenancy 'SHARED'} (the generated enum cross-product order) instead of AWS's published order, and the served name set is the local family x size expansion rather than AWS's 597 published names.
5. Re-replay fixture workspaces-instances-catalog-list-instance-types, step task#1 to confirm a fix.
