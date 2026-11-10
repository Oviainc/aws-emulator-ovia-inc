Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2a4f3385d5a5802b3fd1582cf927bdd2b48aa00d

# A default policy is created with no resource type at all, and the stored `None` is then read three different ways (and becomes settable by an update)

_Filed: 2026-10-29T03:35:10Z_
_Operation: CreateLifecyclePolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `2a4f3385d5a5802b3fd1582cf927bdd2b48aa00d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateLifecyclePolicy` accepts a DEFAULT-policy request that names the resource type nowhere — neither as the request-body `DefaultPolicy` nor as `PolicyDetails$ResourceType` — as long as the configuration structure declares `PolicyLanguage: SIMPLIFIED`. The handler decides the kind from the language (`is_default = input.default_policy.is_some() || policy_language_is_simplified(..)`) and then skips every conditional-required check for that kind: the `omitted_configuration_members` gate is guarded by `if !is_default`, and `state::omitted_configuration_members` itself declines to judge a default policy at all.

The justification the crate gives for skipping it is false for exactly one member. `omitted_configuration_members`' doc asserts that a default policy "is configured from the `[Default policies only]` members, each of which documents a value the service fills in". That holds for `CreateInterval`, `RetainInterval`, `CopyTags` and `ExtendDeletion` — each documents an "If you do not specify a value, the default is …" sentence, and the crate keeps a constant per sentence (`DEFAULT_CREATE_INTERVAL`, `DEFAULT_RETAIN_INTERVAL`, `DEFAULT_COPY_TAGS`, `DEFAULT_EXTEND_DELETION`). It does NOT hold for the resource type: `PolicyDetails$ResourceType` and `CreateLifecyclePolicyRequest$DefaultPolicy` document only "Specify the type of default policy to create" with the `VOLUME` / `INSTANCE` bullets, no fill-in value — which is why the crate has no `DEFAULT_RESOURCE_TYPE` constant to reach for. The member that decides what a default policy actually does is therefore the one member the create never requires of it.

The record that results holds `default_policy: true, default_policy_type: None`, and the crate then reads that one `None` three incompatible ways:

1. `LifecyclePolicyRecord::policy_type()` resolves it to `EBS_SNAPSHOT_MANAGEMENT` (`_ => DEFAULT_POLICY_TYPE`), i.e. reports it as the snapshot default policy — the very thing `VOLUME` means by that function's own doc ("`DefaultPolicy` `INSTANCE` creates EBS-backed AMIs … and `VOLUME` creates snapshots"). `GetLifecyclePolicy` and `GetLifecyclePolicies` both report that type.
2. `LifecyclePolicyRecord::target_resource_types()` resolves it to the EMPTY list, so `GET /policies?resourceTypes=VOLUME` does not select the policy whose `PolicyType` the read above just reported as snapshot management.
3. `GetLifecyclePolicies`' `defaultPolicyType` filter resolves it to neither type: `?defaultPolicyType=ALL` returns the policy, `?defaultPolicyType=VOLUME` and `?defaultPolicyType=INSTANCE` both exclude it. A policy the service calls a default policy belongs to no kind of default policy.

It also leaves the resource type MUTABLE afterwards, which `UpdateLifecyclePolicyRequest$PolicyDetails` rules out. `immutable_configuration_violation` only compares a stated `ResourceType` when the record already holds one (`if let Some(stored) = policy.default_policy_type.as_deref()`), so `PATCH` with `{"PolicyDetails": {"PolicyLanguage": "SIMPLIFIED", "ResourceType": "INSTANCE"}}` is accepted, `replace_default_policy_configuration` stores it, and the policy's reported `PolicyType` flips from `EBS_SNAPSHOT_MANAGEMENT` to `IMAGE_MANAGEMENT` — the policy type changed by an update.

This is NOT the already-open issue `20261027T184059Z-dlm-createlifecyclepolicy-wrong-behavior--r3`: that one is about a `[Custom snapshot policies only]` member (`PolicyDetails.ResourceTypes`) being KEPT on a default policy and accumulating into `target_resource_types()`. Stripping or refusing `ResourceTypes` would not make this create refuse a `SIMPLIFIED` configuration that states no resource type anywhere; the defect here is the absent conditional-required check for the one default-policy member that documents no fill-in value.

## Evidence

The model documents no fill-in value for the member — `smithy_query(query="doc", shape="PolicyDetails", member="ResourceType")`:

> "<b>[Default policies only]</b> Specify the type of default policy to create.</p><ul><li><p>To create a default policy for EBS snapshots, that creates snapshots of all volumes in the Region that do not have recent backups, specify <code>VOLUME</code>.</p></li><li><p>To create a default policy for EBS-backed AMIs, that creates EBS-backed AMIs from all instances in the Region that do not have recent backups, specify <code>INSTANCE</code>.</p></li></ul>"

and the body form says the same — `smithy_query(query="doc", shape="CreateLifecyclePolicyRequest", member="DefaultPolicy")`: "<b>[Default policies only]</b> Specify the type of default policy to create. … specify <code>VOLUME</code> … specify <code>INSTANCE</code>." Neither carries the "If you do not specify a value, the default is …" sentence that `CreateInterval` ("If you do not specify a value, the default is 1. Default: 1"), `RetainInterval`, `CopyTags` ("If you do not specify a value, the default is <code>false</code>. Default: false") and `ExtendDeletion` each do.

The crate nevertheless asserts that every one of them documents such a value, and skips the check — `services/service-com-amazonaws-dlm-dlm-20180112/src/state.rs:1592-1597`:
```
/// A DEFAULT policy is configured from the `[Default policies only]` members,
/// each of which documents a value the service fills in, so it requires none of
/// them and is not judged here.
///
/// Source: `smithy_query(query="shape", shape="PolicyDetails")`.
pub fn omitted_configuration_members(details: &PolicyDetails) -> Vec<String> {
```
The crate's own constants show which members really do document one — `src/state.rs:131-148`:
```
/// The creation frequency a default policy runs at when the request names none:
/// `CreateInterval` documents "If you do not specify a value, the default is 1."
pub const DEFAULT_CREATE_INTERVAL: i32 = 1;
...
pub const DEFAULT_RETAIN_INTERVAL: i32 = 7;
...
pub const DEFAULT_COPY_TAGS: bool = false;
...
pub const DEFAULT_EXTEND_DELETION: bool = false;
```
— there is no constant for the resource type.

The create gates the required-member check on the kind, so a default policy is judged by nothing — `src/ops/create_lifecycle_policy.rs:282-305`, under a comment that states the opposite intent ("a configuration that describes one leaves out nothing that kind of policy is built from"):
```
        // ... which configuration members a request must state depends on the
        // KIND of policy it describes, so the model can mark none of them
        // `@required`. A request that names neither `DefaultPolicy` nor
        // `PolicyDetails` describes no policy at all; a configuration that
        // describes one leaves out nothing that kind of policy is built from.
        if !is_default {
            let Some(details) = input.policy_details.as_ref() else {
```
and `src/ops/create_lifecycle_policy.rs:200-201` is what makes the request a default-policy request without any resource type:
```
        let is_default = input.default_policy.is_some()
            || policy_language_is_simplified(input.policy_details.as_ref());
```
with `src/ops/create_lifecycle_policy.rs:436-437` persisting the `None`:
```
            default_policy: is_default,
            default_policy_type: configuration.resource_type,
```

The three incompatible readings of that one stored `None`, inside this crate:

`src/state.rs:646-657` — absent type reported as the snapshot default policy's type:
```
    pub fn policy_type(&self) -> String {
        if self.default_policy {
            return match self.default_policy_type.as_deref() {
                Some(INSTANCE_DEFAULT_POLICY_TYPE) => IMAGE_POLICY_TYPE.to_string(),
                _ => DEFAULT_POLICY_TYPE.to_string(),
            };
        }
```
`src/state.rs:664-675` — absent type reported as targeting nothing:
```
    pub fn target_resource_types(&self) -> Vec<String> {
        let mut types: Vec<String> = Vec::new();
        if let Some(default_type) = self.default_policy_type.as_ref() {
            types.push(default_type.clone());
        }
```
`src/ops/get_lifecycle_policies.rs:228-234` — absent type matching neither `VOLUME` nor `INSTANCE`:
```
            .filter(|policy| {
                input.default_policy_type.as_ref().is_none_or(|wanted| {
                    policy.default_policy
                        && (wanted == "ALL"
                            || policy.default_policy_type.as_deref() == Some(wanted.as_str()))
                })
            })
```

And the type the model says cannot be updated becomes settable, because the immutability comparison is skipped when nothing is stored — `src/state.rs:1294-1300`:
```
    if let Some(stated) = details.resource_type.as_deref() {
        if let Some(stored) = policy.default_policy_type.as_deref() {
            if stated != stored {
                return Some(resource_type_cannot_be_updated(stated, stored));
            }
        }
    }
```
reached from `src/ops/update_lifecycle_policy.rs:284-290`, against `smithy_query(query="doc", shape="UpdateLifecyclePolicyRequest", member="PolicyDetails")`: "The configuration of the lifecycle policy. You cannot update the policy type or the resource type." `replace_default_policy_configuration` then writes it (`src/state.rs:603-605`: `if configuration.resource_type.is_some() { self.default_policy_type = configuration.resource_type; }`).

Test coverage: no test creates a default policy without a resource type. `tests/all/default_policy_resource_type.rs`, `tests/all/policy_identity_immutability.rs:270-278` (`default_policy_details` builds `"ResourceType": target`) and `tests/all/create_policy_kind_materialization.rs` always supply one of `DefaultPolicy` / `PolicyDetails.ResourceType`, so the typeless default policy is unpinned on every read path.

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with {"ExecutionRoleArn": "arn:aws:iam::<emulator account>:role/AWSDataLifecycleManagerDefaultRole", "Description": "typeless default policy", "State": "ENABLED", "PolicyDetails": {"PolicyLanguage": "SIMPLIFIED"}} — observe HTTP 200 and a PolicyId: a DEFAULT policy was created although nothing stated the type of default policy to create, in the body or in the structure
2. GET /policies/{PolicyId} — Policy.DefaultPolicy is true and Policy.PolicyDetails reports "PolicyType": "EBS_SNAPSHOT_MANAGEMENT" with NO "ResourceType" member: the read names the policy as the snapshot default policy while reporting no resource type for it
3. GET /policies?defaultPolicyType=ALL — the policy is listed; GET /policies?defaultPolicyType=VOLUME and GET /policies?defaultPolicyType=INSTANCE — the policy is listed by NEITHER, so a default policy belongs to no kind of default policy
4. GET /policies?resourceTypes=VOLUME — the policy is NOT listed, contradicting step 2's reported PolicyType EBS_SNAPSHOT_MANAGEMENT (which src/state.rs:640-645 documents as what VOLUME means)
5. PATCH /policies/{PolicyId} with {"PolicyDetails": {"PolicyLanguage": "SIMPLIFIED", "ResourceType": "INSTANCE"}} — observe HTTP 200, then GET /policies/{PolicyId} reports "PolicyType": "IMAGE_MANAGEMENT": an update changed the policy type and the resource type, which UpdateLifecyclePolicyRequest$PolicyDetails documents as impossible ("You cannot update the policy type or the resource type")
