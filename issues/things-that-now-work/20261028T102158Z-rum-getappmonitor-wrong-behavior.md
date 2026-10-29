Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2290109583dfdd2361f9898cc3723bf653f523ee

# The caller's IAM policies are read from attached managed policies only for a ROLE: an IAM user's attached policies are invisible, so every RUM operation refuses it

_Filed: 2026-10-28T10:21:58Z_
_Operation: GetAppMonitor | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2290109583dfdd2361f9898cc3723bf653f523ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::authorization::caller` is the single place this crate assembles the identity-based documents every operation's authorization decision is made from (`authorize` -> `caller`), and it is reached by GetAppMonitor, GetAppMonitorData, UpdateAppMonitor, DeleteAppMonitor, ListAppMonitors, PutRumEvents, the three resource-policy calls, the three metrics-destination calls, the four metric-definition calls and the three tagging calls.

It has two branches. The ASSUMED-ROLE branch collects both the role's inline policies and the documents of the managed policies attached to it, and — when an attachment is an AWS-managed ARN whose document the account does not hold — sets `unreadable_attachment`, which `Caller::identity_is_unbounded` turns into an allow. The IAM-USER branch collects `user_inline_policies` and nothing else: it never looks at `store.user_attached_policies`, never reads an attached policy's default-version document, and never sets `unreadable_attachment`.

Consequences, for input the model allows (any request signed with an IAM user's access key):

1. A user whose only grant is a CUSTOMER-managed policy attached with `iam:AttachUserPolicy` (allowing `rum:*` on the monitor's ARN) is handed `documents == []`, so `identity_decision` answers `NotStated` and every RUM operation refuses with `AccessDeniedException` 403 — the authorization decision produces the wrong verdict for a grant the IAM store holds.
2. A user with an AWS-MANAGED policy attached (e.g. the kind `AttachUserPolicy` accepts without the account holding its document) is ALSO refused, which directly contradicts the rule this module's own header states for such a principal.

The fix is to give the user branch the same two steps the role branch already has: extend `caller.documents` with the default-version documents of `store.user_attached_policies` for the user key, and set `unreadable_attachment` for an attachment that `is_aws_managed_policy` recognises. Nothing about the role branch, the decision itself, or any response of an authorized call changes.

## Evidence

The rule the module states for a principal whose attachment cannot be read — `services/service-com-amazonaws-rum-rum/src/state/authorization.rs:34-42`:
```
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy is owned by AWS, not by the account, so the IAM store holds no document for it
//!   ([`iam_store::AWS_MANAGED_POLICY_ACCOUNT`]) ...
//!   A principal with such a policy attached is allowed: RUM cannot see what that document grants, and refusing on evidence it
//!   does not have would deny access AWS permits.
```
and `authorization.rs:19-22`:
```
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal ... read out of the IAM service's
//!    OWN store so RUM sees exactly what IAM's handlers wrote;
```

The ROLE branch, which honours both — `authorization.rs:307-335`:
```
    if let Some(role_name) = assumed_role_name(&caller.arn) {
        let role_key = iam_store::name_key(role_name);
        ...
        for attached in store
            .role_attached_policies
            .get(&role_key)
            ...
            match store
                .policy_by_arn(attached)
                .and_then(default_version_document)
            {
                Some(document) => caller.documents.push(document),
                None if is_aws_managed_policy(attached) => {
                    caller.unreadable_attachment = true;
                }
                None => {}
            }
        }
```

The USER branch, which honours neither, in the same function — `authorization.rs:336-341`:
```
    } else if let Some(user_name) = iam_user_name(&caller.arn) {
        caller.documents.extend(inline_documents(
            &store.user_inline_policies,
            &iam_store::name_key(user_name),
        ));
    }
    caller
```

The map the user branch skips exists and is written by IAM's own handler, so "RUM sees exactly what IAM's handlers wrote" is false for a user: `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/state.rs:1447` declares `pub user_attached_policies: BTreeMap<String, BTreeSet<String>>,` and `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/attach_user_policy.rs:249` writes it (`let attached = store.user_attached_policies.entry(user_key).or_default();`).

The refusal a wrongly-denied caller gets, built from the empty document set — `authorization.rs:226-230`:
```
    let identity_said = iam::identity_decision(&caller.documents, &request);
    if iam::is_allowed(identity_said, resource_said) {
        return Ok(());
    }
    Err(refusal(identity_said == Decision::Denied))
```
rendered as `AccessDeniedException` by every handler, e.g. `services/service-com-amazonaws-rum-rum/src/ops/get_app_monitor.rs:166-178`.

Test coverage: `services/service-com-amazonaws-rum-rum/tests/all/authorization.rs` exercises attachments for ROLES only — its helper is `pub(crate) async fn attach_role_policy(&self, role_name: &str, policy_arn: &str)` (line 341) and the attachment table is `an_attached_managed_policy_is_read_as_the_document_it_names` (line 1077) whose rows all go through `emulator.attach_role_policy(&role, &policy_arn).await` (line 1116). No test in the crate attaches a policy to an IAM USER, so no existing test states a reason for the asymmetry.

## Steps to Reproduce

1. Mount RUM beside the emulated IAM and STS services (as tests/all/authorization.rs does).
2. With an administrative credential: CreateAppMonitor(Name="m", Domain="example.com").
3. Through IAM: CreateUser(UserName="reader"), CreatePolicy(PolicyName="rum-read", PolicyDocument={"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"rum:GetAppMonitor","Resource":"arn:aws:rum:us-east-1:<account>:appmonitor/m"}]}), then AttachUserPolicy(UserName="reader", PolicyArn=<the new policy ARN>), and CreateAccessKey(UserName="reader").
4. Signed with that user's access key: GET /appmonitor/m -> observed AccessDeniedException 403 ("because no policy allows the rum:GetAppMonitor action"), because `caller` collected no document for the attached policy; expected 200 with the monitor, since the grant is in the IAM store.
5. Repeat step 3 with an AWS-managed policy ARN (account segment `aws`) instead of a customer-managed one, so the account holds no document for it: GET /appmonitor/m -> observed 403; expected allowed, per authorization.rs:34-42 ("A principal with such a policy attached is allowed").
6. For contrast, attach the SAME customer-managed policy to a role and call GET /appmonitor/m with an AssumeRole session credential -> 200, which is the branch that reads `role_attached_policies`.
