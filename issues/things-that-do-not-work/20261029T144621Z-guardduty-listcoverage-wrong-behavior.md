Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 58d3cdd8a54717975c7a5e45d2c60ee5b89f816e

# An EKS coverage row drops the agent add-on's addonStatus whenever the add-on reports no addonVersion, even though the row holds that status and computed its coverageStatus/issue from it

_Filed: 2026-10-29T14:46:21Z_
_Operation: ListCoverage | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `58d3cdd8a54717975c7a5e45d2c60ee5b89f816e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/state/coverage.rs` models the GuardDuty security agent add-on's VERSION and its STATUS as two independent facts: `CoverageRow` carries `addon_version: Option<String>` and `addon_status: Option<String>` side by side, `AgentAddon::Installed` carries `version: Option<String>` and `status: Option<String>` side by side, and `agent_addon` fills each from its own independent accessor on the EKS answer (`addon.addon_version()` / `addon.status()`, both `Option`).

The row's COVERAGE VERDICT is then computed from the STATUS alone: an add-on reported `ACTIVE` makes the row `HEALTHY`, any other reported status makes it `UNHEALTHY` with an issue naming that status.

But the projection onto the wire (`CoverageRow::reported`) conditions the whole `AddonDetails` structure on the VERSION:

    addon_details: self.addon_version.as_ref().map(|version| AddonDetails { ... addon_status: self.addon_status.clone() })

So when EKS reports the add-on with a status but no version — the exact combination the module's own `Option`/`Option` pair says is possible — the row keeps `addon_status`, uses it to decide `coverageStatus` and to word `issue`, and then reports `addonDetails` as ABSENT. The answer contradicts itself: it says the cluster is UNHEALTHY *because* the add-on is in some named state, while reporting no add-on detail at all, i.e. exactly what a cluster that carries no add-on reports. The one fact the model has a member for (`AddonDetails.AddonStatus`, an optional member in its own right) is discarded on account of a different, independent member being absent.

The module's own stated rule for an absent fact is per-member — "A member this emulator holds no fact for is left ABSENT rather than filled with a stand-in" — not per-structure, and the module demonstrates that elsewhere: `cluster_name`, `covered_nodes` and `compatible_nodes` are each set/left absent on their own. Only `addon_status` is gated on a sibling member.

The same coupling also shows in the criterion lookup: `KEY_ADDON_VERSION => self.addon_version.clone()` resolves to nothing for such a row, which is correct for ADDON_VERSION — but the status the row does hold is then unreadable anywhere in the response.

## Evidence

THE TWO FACTS ARE INDEPENDENT — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state/coverage.rs:322-325:
```
    /// `AddonDetails.AddonVersion` of the agent add-on, when the cluster carries it.
    addon_version: Option<String>,
    /// `AddonDetails.AddonStatus` of the agent add-on, when the cluster carries it.
    addon_status: Option<String>,
```
and src/state/coverage.rs:856-865:
```
enum AgentAddon {
    Installed {
        version: Option<String>,
        status: Option<String>,
        modified_at_millis: Option<i64>,
    },
    Absent,
    Unknown,
}
```
filled from two independent accessors — src/state/coverage.rs:881-886:
```
            Some(addon) => AgentAddon::Installed {
                version: addon.addon_version().map(str::to_string),
                status: addon.status().map(|status| status.as_str().to_string()),
                modified_at_millis: addon.modified_at().map(millis_of),
            },
```

THE VERDICT IS COMPUTED FROM THE STATUS ALONE — src/state/coverage.rs:789-803:
```
        let addon_status = match &addon {
            AgentAddon::Installed { status, .. } => status.clone(),
            AgentAddon::Absent | AgentAddon::Unknown => None,
        };
        // Three ANSWERS, not two: ...
        let (coverage_status, issue) = match (&addon, addon_status.as_deref()) {
            (AgentAddon::Installed { .. }, Some(ADDON_STATUS_ACTIVE)) => (HEALTHY, None),
            (AgentAddon::Installed { .. }, Some(status)) => (
                UNHEALTHY,
                Some(format!(
                    "The GuardDuty security agent add-on {GUARDDUTY_AGENT_ADDON} is {status} on cluster {cluster_name}."
                )),
            ),
```

THE PROJECTION GATES IT ON THE VERSION — src/state/coverage.rs:358-367:
```
                eks_cluster_details: Some(CoverageEksClusterDetails {
                    cluster_name: self.cluster_name.clone(),
                    covered_nodes: None,
                    compatible_nodes: None,
                    addon_details: self.addon_version.as_ref().map(|version| AddonDetails {
                        addon_version: Some(version.clone()),
                        addon_status: self.addon_status.clone(),
                    }),
                    management_type: Some(self.management_type.to_string()),
                }),
```
against the module's own per-member absence rule at src/state/coverage.rs:333-334: "A member this emulator holds no fact for is left ABSENT rather than filled with a stand-in".

THE MODEL MAKES THE TWO MEMBERS INDEPENDENT — `smithy_query(shape="AddonDetails")`:
```
"AddonVersion": {"target": "String", "jsonName": "addonVersion", "doc": "<p>Version of the installed EKS add-on.</p>"},
"AddonStatus": {"target": "String", "jsonName": "addonStatus", "doc": "<p>Status of the installed EKS add-on.</p>"}
```
Neither is `@required`, so `AddonDetails` carrying only `addonStatus` is a value the shape admits.

TEST COVERAGE: none. A repo-wide grep of services/service-com-amazonaws-guardduty-guarddutyapiservice/tests for `addonDetails|addon_details|addonStatus|addonVersion` returns no matches, and tests/all/coverage_listing.rs contains no `addon` assertion at all — so nothing pins the current coupling.

## Steps to Reproduce

1. Create a GuardDuty detector in us-east-1 and enable RUNTIME_MONITORING (or EKS_RUNTIME_MONITORING) on it, so `CoverageScope::of` sets `covers_eks`.
2. Have the in-process Amazon EKS emulator hold a cluster whose `aws-guardduty-agent` add-on `DescribeAddon` answers with a `status` (e.g. anything other than `ACTIVE`) and no `addonVersion` — the combination `Addon::addon_version()`/`Addon::status()` both being `Option` admits.
3. Call `GET /detector/{detectorId}/coverage` (ListCoverage).
4. Observe the row for that cluster: `coverageStatus` is `UNHEALTHY` and `issue` reads "The GuardDuty security agent add-on aws-guardduty-agent is <status> on cluster <name>." — both derived from `addon_status` at src/state/coverage.rs:796-803.
5. Observe that `resourceDetails.eksClusterDetails.addonDetails` is ABSENT, because src/state/coverage.rs:362 gates the whole structure on `addon_version`. The status the issue text names is reported nowhere in the response, and the row is indistinguishable from one for a cluster that carries no add-on at all.
6. Contrast with the same add-on reported WITH a version: `addonDetails.addonStatus` is then present, from the identical `addon_status` field.
