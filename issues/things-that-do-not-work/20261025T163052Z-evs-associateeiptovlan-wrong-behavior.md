Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a4b61295d097ae4da0c45de1d4f9e67ffc2c86fd

# The authorization gate reads managed policies attached to a role but not those attached to an IAM user, so an explicit Deny on a user is ignored

_Filed: 2026-10-25T16:30:52Z_
_Operation: AssociateEipToVlan | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a4b61295d097ae4da0c45de1d4f9e67ffc2c86fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::authorization::caller` — the gate every implemented operation of this crate calls first (`AssociateEipToVlan` at associate_eip_to_vlan.rs:209, and the same call in all 22 other implemented handlers) — gathers the identity-based documents in force on the caller. For an assumed-role principal it reads BOTH the role's inline policies and the default version of every managed policy attached to the role. For an IAM user principal it reads ONLY the user's inline policies: `store.user_attached_policies` is never consulted, even though the IAM store exposes it as a public field that `iam:AttachUserPolicy` writes. The consequence is that an explicit `Deny` of `evs:*` in a managed policy attached to an IAM user is invisible to this service, so every EVS operation is performed for a principal IAM says is denied — while the very same policy document, attached to a role or embedded inline on the user, is refused with `AccessDeniedException`. The user branch needs the lookup the role branch two lines above it already performs, through the `default_version_document` helper this file already owns.

## Evidence

The stated principle the code is measured against — src/state/authorization.rs:20-23:
```
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal (a role's inline policies and the default versions of the managed
//!    policies attached to it, a user's inline policies), read out of the IAM service's
//!    OWN store so this service sees exactly what IAM's handlers wrote;
```
and authorization.rs:36-37: "An EXPLICIT `Deny` about the action is the one IAM decision this emulator can make soundly, and it is final whatever else the account holds, so it is what refuses here."

The role branch, which reads attached managed policies — src/state/authorization.rs:168-189:
```
    if let Some(role_name) = assumed_role_name(&caller.arn) {
        let role_key = iam_store::name_key(role_name);
        caller.documents.extend(inline_documents(&store.role_inline_policies, &role_key));
        for attached in store.role_attached_policies.get(&role_key).into_iter().flatten() {
            if let Some(document) = store.policy_by_arn(attached).and_then(default_version_document) {
                caller.documents.push(document);
            }
        }
```
The user branch, which does not — src/state/authorization.rs:190-195:
```
    } else if let Some(user_name) = iam_user_name(&caller.arn) {
        caller.documents.extend(inline_documents(
            &store.user_inline_policies,
            &iam_store::name_key(user_name),
        ));
    }
```
The store field that is never read, written by IAM's own `AttachUserPolicy` — services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/state.rs:861: `pub user_attached_policies: BTreeMap<String, BTreeSet<String>>,` and services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/attach_user_policy.rs:254: `let attached = store.user_attached_policies.entry(user_key).or_default();`.

Test coverage: tests/all/authorization.rs only drives the role path — authorization.rs:72-78 ("Embed an inline identity policy on a role in IAM's OWN store", writing `role_inline_policies`) and :236 ("The recorded case: a role whose inline identity policy denies the whole service"). Its header lists what deliberately does NOT refuse — "group policies, permissions boundaries, service control policies" (tests/all/authorization.rs:12-15) — and a managed policy attached to a user is not among them, so no test pins this behaviour.

## Steps to Reproduce

1. In the IAM emulator, `CreatePolicy` with the document `{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"evs:*","Resource":"*"}]}` and `AttachUserPolicy` it to IAM user `evs-user`.
2. Sign an EVS request as `arn:aws:iam::<account>:user/evs-user` (the ARN form `iam_user_name` resolves, authorization.rs:213-217).
3. Call any implemented EVS operation, e.g. `AssociateEipToVlan` or `GetEnvironment`: the call is performed, because `caller.documents` is empty (only `user_inline_policies` was read) and `explicit_deny` returns `None` at authorization.rs:107-109.
4. Attach the identical document to a role instead and call as an assumed-role session of that role: the request is now refused with `AccessDeniedException`, showing the two principal kinds are judged against different document sets.
