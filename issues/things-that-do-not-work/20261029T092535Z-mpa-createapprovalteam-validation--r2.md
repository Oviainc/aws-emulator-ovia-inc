Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 511f9d11774345a530a86fa6799cabc26a1e1916

# A Policies reference naming a DEPRECATED policy version is attachable, although the store defines that status as whether the version may still be attached

_Filed: 2026-10-29T09:25:35Z_
_Operation: CreateApprovalTeam | Class: validation_

> _Note: this issue was identified against the code base at commit `511f9d11774345a530a86fa6799cabc26a1e1916`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateApprovalTeam` is the only path in this service that ATTACHES policies to a resource, and it holds every reference against one gate, `ComAmazonawsMpaAwsfluffycoreserviceState::unattachable_policy_reference`. That gate decides attachability purely on EXISTENCE: it resolves the qualified ARN through `policy_version` and refuses only a reference the catalog holds no version for. It never consults the version's `status`, the field this same module defines as "Whether this version may still be attached to a team".

So a catalog version the service itself reports as `DEPRECATED` (`ListPolicyVersions` / `GetPolicyVersion` both report `PolicyStatus` straight off the record) is accepted as an attachment, stored on the team's `Policies`, and reported from then on by `GetApprovalTeam$Policies`, `ListResourcePolicies` and `GetResourcePolicy` as a policy attached to that team. The store therefore records an attachment its own definition of the status field says cannot exist, and the one `DEPRECATED` token the model declares restricts nothing anywhere in the crate.

The condition needs a catalog entry holding a deprecated version — the state the crate's own fixtures construct and the two catalog reads already report — because the catalog is AWS-owned and no modelled operation mints a version. The fix is confined to the existing gate (`unattachable_policy_reference`), not to `policy_version`, which must keep resolving a deprecated version so `GetPolicyVersion` and `ListPolicyVersions` can report it.

## Evidence

The store's own definition of the field, services/service-com-amazonaws-mpa-awsfluffycoreservice/src/state.rs:2419-2421:
```
    pub version_id: i32,
    /// Whether this version may still be attached to a team.
    pub status: PolicyStatus,
```

The one gate that decides whether a reference may be attached — existence only, status never read, state.rs:3145-3162:
```
    /// The first of `references` that names no policy version this store holds, or
    /// `None` when every one of them resolves.
    ///
    /// A policy is AWS-owned — no operation of this model mints, updates or deletes one —
    /// so a reference is only attachable when the catalog already holds the policy AND
    /// the version it names. References are resolved through [`Self::policy_version`],
    /// ...
    pub fn unattachable_policy_reference<'a, I>(&self, references: I) -> Option<&'a str>
    where
        I: IntoIterator<Item = &'a str>,
    {
        references
            .into_iter()
            .find(|reference| self.policy_version(reference).is_none())
    }
```
and the resolver it calls, which answers with the version whatever its status, state.rs:3173-3181:
```
    pub fn policy_version(&self, arn: &str) -> Option<(&CatalogPolicy, &CatalogPolicyVersion)> {
        let reference = parse_policy_version_arn(arn)?;
        let policy = self.policy(&reference.policy_arn)?;
        let version_id = match reference.version {
            PolicyVersionSelector::Default => policy.default_version,
            PolicyVersionSelector::Id(version_id) => version_id,
        };
        policy.version(version_id).map(|version| (policy, version))
    }
```

The call site, src/ops/create_approval_team.rs:327-334 (its doc at create_approval_team.rs:45-51 scopes the rule to existence: "`Policies` naming a policy version the catalog does not hold earns `ValidationException` (400)"):
```
        if let Some(unattachable) = state.unattachable_policy_reference(
            input.policies.iter().map(|policy| policy.policy_arn.as_str()),
        ) {
            return Err(CreateApprovalTeamErrorModeled::ValidationException(
                crate::state::unattachable_policy(unattachable),
            )
            .into());
        }
```

The status is a value this service PUBLISHES for the same version it then accepts as an attachment — src/ops/list_policy_versions.rs:248 and src/ops/get_policy_version.rs:222:
```
        status: version.status.as_wire().to_string(),
```
and the model corroborates the distinction, `smithy_query(query="doc", shape="PolicyVersionSummary", member="Status")`:
```
"<p>Status for the policy. For example, if the policy is <a href=\"...id_groups_manage_attach-policy.html\">attachable</a> or <a href=\"...access_policies_managed-deprecated.html\">deprecated</a>.</p>"
```

Test coverage: no test attaches a deprecated version, so refusing one leaves every existing assertion unchanged. The suite that drives the attach gate records ATTACHABLE versions only — tests/all/write_path_input_validation.rs:63-68:
```
    policy.insert_version(state::CatalogPolicyVersion {
        version_id: 2,
        status: state::PolicyStatus::Attachable,
```
while the catalog READ suites are the ones that construct deprecated versions — tests/all/policy_versions.rs:82-88:
```
pub(crate) fn status_for(version_id: i32, newest: i32) -> state::PolicyStatus {
    if version_id < newest {
        state::PolicyStatus::Deprecated
    } else {
        state::PolicyStatus::Attachable
    }
}
```
(and tests/all/policy_version_detail.rs:103-105 for the single-version read).

## Steps to Reproduce

1. Record a catalog entry holding a deprecated version, the way the crate's own fixtures do because no modelled operation mints a policy (tests/all/policy_versions.rs:82-88, tests/all/write_path_input_validation.rs:55-73): insert a `CatalogPolicy` with version 1 `PolicyStatus::Deprecated` and version 2 `PolicyStatus::Attachable`, default version 2.
2. `POST /policies/{PolicyArn}/?List` and observe the entry for version 1 reporting `"Status": "DEPRECATED"`.
3. Record the caller as the organization's management account so the gated create is served (as tests/all/approval_team_lifecycle.rs:194 does).
4. `POST /approval-teams` with `Policies: [{"PolicyArn": "<PolicyArn>/1"}]` (the deprecated version) plus a conforming `Name`, `Description`, `ApprovalStrategy` and one approver.
5. Observe HTTP 200 and a stored team: expected `ValidationException` (400) from the same attach gate, since the version the reference names is one the store records as no longer attachable.
6. `GET /approval-teams/{Arn}` and `POST /resource-policies/{Arn}/?List` and observe the deprecated version reported as an attached policy of the new team.
