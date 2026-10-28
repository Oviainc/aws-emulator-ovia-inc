Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# The "resolve the IAM role this ARN names" lookup is implemented twice, once per half of StartExperiment's role gate

_Filed: 2026-10-28T11:53:14Z_
_Operation: StartExperiment | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartExperiment asks the account's IAM store two questions about the SAME execution role, in the same block, under one IAM guard: (a) does the role's trust policy let FIS assume it (`crate::state::role_trust_refusal`), and (b) which policy documents are in force on it (`crate::state::action_permissions::role_documents`). Each of the two helpers opens with its OWN copy of the lookup that answers "which stored role does this ARN name" — the identical expression `iam.roles.values().find(|role| role.arn == role_arn)?` — rather than both going through one helper.

That is one computation written twice in this crate, and the two copies can drift: if one of them later learns anything about matching an ARN (case folding, a path or partition difference, resolving through IAM's own ARN parser the way `action_permissions::is_aws_managed_policy` already does with `iam::parse_policy_arn`), the other keeps the old rule. Because StartExperiment spends both answers on what it believes is one role, a drift makes the start read the trust policy of one role and the permissions of another (or of none) for a single `template.roleArn` — the trust half would refuse/allow on one record while the privileges half attributes `insufficientPrivileges` from a different one.

The fix is confined to the crate: give `state.rs` one `role_by_arn(iam, role_arn) -> Option<&Role>` (next to the other IAM-store readings it already owns) and have both helpers call it. No response changes and no test changes: both call sites keep their current semantics (`None` = nothing can be shown about the role, so the start goes ahead).

## Evidence

Copy 1 — the trust half, in this crate's state module:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs:4482-4483`
```
pub fn role_trust_refusal(role_arn: &str, iam: &IamState) -> Option<ValidationException> {
    let role = iam.roles.values().find(|role| role.arn == role_arn)?;
```

Copy 2 — the permissions half, in this crate's `state/action_permissions.rs`:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/state/action_permissions.rs:117-118`
```
pub fn role_documents(role_arn: &str, iam: &IamState) -> Option<Vec<String>> {
    let role = iam.roles.values().find(|role| role.arn == role_arn)?;
```

Both are called on the SAME `template.role_arn`, from the same handler, inside one IAM guard — which is what makes a drift between them observable as one role answering two different questions:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/start_experiment.rs:302-308`
```
        let (trust_refusal, role_documents) = {
            let iam = iam.lock();
            (
                crate::state::role_trust_refusal(&template.role_arn, &iam),
                crate::state::action_permissions::role_documents(&template.role_arn, &iam),
            )
        };
```

The crate already states that this kind of rule should have one home — the same file hoists every other shared decision (`template_refusal` for the admissibility sequence, `ordered_page` for the page walk) and `action_permissions` explicitly reads IAM's own constants rather than restating them:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/state/action_permissions.rs:308-315`
```
/// Whether `arn` names an AWS-MANAGED policy: one whose account segment is IAM's own
/// `aws`, so AWS owns the document and this account's store holds none for it. Read
/// through IAM's OWN ARN parser and its own constant, so this service and IAM agree on
/// what an AWS-managed policy ARN is.
fn is_aws_managed_policy(arn: &str) -> bool {
    iam::parse_policy_arn(arn)
        .is_some_and(|policy| policy.account == iam::AWS_MANAGED_POLICY_ACCOUNT)
}
```

Test coverage that must stay unchanged by the de-duplication: `tests/all/control_plane_rules.rs` (the trust-policy reading — "runs under: with no trust policy to read, `StartExperiment` has no evidence the role …" at `tests/all/control_plane_rules.rs:70` — plus the `ExperimentFailure::insufficient_privileges` expectations at `tests/all/control_plane_rules.rs:948` and `:1233`) and `tests/all/experiment_options_decide_the_run.rs:515` (`code, "insufficientPrivileges"`), which exercise both halves of the gate through the router.

## Steps to Reproduce

1. Open services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs at line 4483 and services/service-com-amazonaws-fis-faultinjectionsimulator/src/state/action_permissions.rs at line 118: the same expression `iam.roles.values().find(|role| role.arn == role_arn)?` appears in both, each being the entry point of one half of StartExperiment's execution-role gate.
2. Confirm both are invoked on the same value in one place: services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/start_experiment.rs:302-308 calls `role_trust_refusal(&template.role_arn, &iam)` and `action_permissions::role_documents(&template.role_arn, &iam)` under a single IAM guard.
3. Change ONE copy only (e.g. make the trust half compare through IAM's ARN parser, or ignore a trailing path difference) and re-run `cargo test -p service-com-amazonaws-fis-faultinjectionsimulator`: a start can now resolve a role for the trust question and no role for the privileges question (or vice versa) from one `roleArn`, because nothing ties the two readings together.
4. The de-duplicated form: add one `role_by_arn(iam, role_arn)` helper in state.rs and call it from both helpers; every existing test (tests/all/control_plane_rules.rs, tests/all/experiment_options_decide_the_run.rs) must still pass with identical responses.
