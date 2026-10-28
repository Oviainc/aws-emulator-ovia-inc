Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 62bd53a2191a4112059965729a42c5aa0aaceb39

# An IAM user's ATTACHED managed policies are never read, so a user granted the action by AttachUserPolicy is refused 403 (the role branch next to it reads attachments)

_Filed: 2026-10-28T16:50:34Z_
_Operation: CreateExport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `62bd53a2191a4112059965729a42c5aa0aaceb39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::authorization::caller()` is the one place this crate collects the identity-based policy documents the four operations that model `AccessDeniedException` (`CreateExport`, `TagResource`, `UntagResource`, `ListTagsForResource`) are judged on. It has two branches, in one `if/else if`:

* the ASSUMED-ROLE branch reads the role's inline policies AND walks `store.role_attached_policies`, resolving each attachment's default version into `caller.documents`, and setting `caller.unreadable_attachment` for an AWS-managed attachment it cannot read (which then allows the call);
* the IAM-USER branch reads ONLY `store.user_inline_policies`, on the stated grounds that "`AttachUserPolicy` is not implemented there, so there is no attached-policy map to read for a user".

That stated reason is false: the IAM crate implements `AttachUserPolicy` and holds `user_attached_policies`, keyed and ordered "exactly as [`Self::role_attached_policies`] is". So a caller whose permission to call these operations comes from a managed policy attached to its IAM user — the ordinary way permissions are granted — contributes NO documents, `identity_decision` answers `NotStated`, and the request is refused with `AccessDeniedException` (403) and the sentence "because no policy allows the bcm-data-exports:CreateExport action". The identical grant written as an INLINE user policy is allowed, and the identical grant attached to a ROLE is allowed, so the same policy decides the same request two ways depending on which branch of this function the caller lands in.

Two symptoms of the one cause:
1. a customer-managed policy allowing `bcm-data-exports:*`, attached to the signing user, grants nothing — the call is refused;
2. an AWS-MANAGED policy attached to the user never sets `unreadable_attachment`, so the module's own rule for a grant this emulator cannot read — "A principal with such a policy attached is allowed … refusing on evidence it does not have would deny access AWS permits" — holds for a role and is broken for a user: the user is denied on evidence the service does not have.

The fix needs no new machinery: the role branch's loop, `policy_by_arn`, `default_version_document` and `is_aws_managed_policy` already exist in this file; the user branch skips helpers the crate already has.

## Evidence

This crate's user branch, with the reason it states — `services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/state/authorization.rs:235-243`:
```
    } else if let Some(user_name) = iam_user_name(&caller.arn) {
        // A user's INLINE policies are what the IAM store holds for it; `AttachUserPolicy`
        // is not implemented there, so there is no attached-policy map to read for a user
        // (unlike a role's `role_attached_policies` above).
        caller.documents.extend(inline_documents(
            &store.user_inline_policies,
            &iam_store::name_key(user_name),
        ));
    }
```

The sibling ROLE branch in the SAME function, which does read attachments — same file, `:208-234`:
```
    if let Some(role_name) = assumed_role_name(&caller.arn) {
        ...
        for attached in store
            .role_attached_policies
            .get(&role_key)
            .into_iter()
            .flatten()
        {
            match store
                .policy_by_arn(attached)
                .and_then(default_version_document)
            {
                Some(document) => caller.documents.push(document),
                // Only an AWS-MANAGED policy is a grant this emulator cannot read (see
                // the module docs).
                None if is_aws_managed_policy(attached) => {
                    caller.unreadable_attachment = true;
                }
                None => {}
            }
        }
```

The stated reason is false — the IAM store this code reads DOES hold user attachments, `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/state.rs:1479-1484`:
```
    /// The managed policies attached to each user: [`name_key`] of the user name
    /// -> the attached policies' ARNs. Keyed and ordered exactly as
    /// [`Self::role_attached_policies`] is, and for the same reasons: a policy is
    /// attached to a user at most once, and the total ARN order is what lets a
    /// reader of the set page over it.
    pub user_attached_policies: BTreeMap<String, BTreeSet<String>>,
```
and the operation that fills it exists: `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/attach_user_policy.rs`.

The module rule the user branch breaks — `.../src/state/authorization.rs:40-46`:
```
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy is owned by AWS, not by the account, so the IAM store holds no document for it
//!   ([`iam_store::AWS_MANAGED_POLICY_ACCOUNT`]). A principal with such a policy attached
//!   is allowed: this service cannot see what that document grants, and refusing on
//!   evidence it does not have would deny access AWS permits.
```

The refusal path the missing documents reach — same file, `:115-123`: `let identity_said = iam::identity_decision(&caller.documents, &request); if iam::is_allowed(identity_said, Decision::NotStated) { return Ok(()); } ... format!("because no policy allows the {action} action")`.

Test coverage: the authorization suite builds a user with an INLINE policy only — `tests/all/front_door_authorization.rs:222-224`: "Create an IAM user with `policy` as its inline policy (when one is given) and answer the access key id of a long-term key minted for it. pub(crate) async fn user_key(&self, user_name: &str, policy: Option<&str>)" — and attaches managed policies only to ROLES, `tests/all/front_door_authorization.rs:293-300`: "Attach the managed policy `policy_arn` to the role `role_name` … \"AttachRolePolicy\"". `grep -r 'AttachUserPolicy' services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/tests` returns no matches, so no test pins today's behaviour for a user attachment.

## Steps to Reproduce

1. Boot the emulator with IAM, STS and bcm-data-exports mounted on the one engine.
2. iam `CreateUser` UserName=probe; iam `CreatePolicy` PolicyName=exports-all with a document allowing Action `bcm-data-exports:*` on Resource `*`; iam `AttachUserPolicy` UserName=probe PolicyArn=<the ARN CreatePolicy returned>; iam `CreateAccessKey` UserName=probe.
3. Sign `CreateExport` (or `TagResource`/`UntagResource`/`ListTagsForResource`) with that access key id.
4. Observe HTTP 403 `AccessDeniedException` "User: arn:aws:iam::<account>:user/probe is not authorized to perform: bcm-data-exports:CreateExport on resource: … because no policy allows the bcm-data-exports:CreateExport action", although the attached policy allows the action.
5. Control 1: put the SAME document on the same user with iam `PutUserPolicy` instead and repeat — the call is allowed, because only `user_inline_policies` is read.
6. Control 2: attach the SAME managed policy to a role with iam `AttachRolePolicy`, `sts:AssumeRole` it and repeat — the call is allowed, because the role branch walks `role_attached_policies`.
7. Control 3: attach an AWS-managed policy (account segment `aws`) to the user and repeat — still 403, where the role path would set `unreadable_attachment` and allow it.
