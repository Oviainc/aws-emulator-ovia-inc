Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 5438a4477bf61f5d34b363a6c3dc3db11bcd3fb7

# The policy operations' "which 404" two-step is implemented twice instead of single-homed in state

_Filed: 2026-10-28T00:06:37Z_
_Operation: GetInvestigationGroupPolicy | Class: maintainability_

> _Note: this issue was identified against the code base at commit `5438a4477bf61f5d34b363a6c3dc3db11bcd3fb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate's doctrine is that a rule every operation has to state the same way lives exactly once in `crate::state`, and that even the COMPOSITION of rules is single-homed there because the order is part of the contract (that is why `write_configuration_rejection` exists and why the create/update chain duplication was already removed). The two-step rule the policy operations share — resolve `{identifier}` and report the absent GROUP when nothing resolves, then look at the stored document and report the absent POLICY when the group exists but carries none — is nevertheless written out twice, once in `GetInvestigationGroupPolicy` and once in `DeleteInvestigationGroupPolicy`, with the delete handler's comment explicitly pointing at the other copy as its source of truth. Two copies of one branch can drift: a change to which exception the first or the second step answers (or to their order) has to be made in both files, and nothing fails if only one is edited.

The rule itself is already half-factored — `state::investigation_group_not_found()` and `state::investigation_group_policy_not_found()` are the single-homed wordings — so what is missing is only the resolution step that chooses between them, the way `state::write_configuration_rejection` owns the create/update chain. A fix is a single state-level resolver the two handlers call; every status, error code and message stays byte-identical, so no response and no existing test changes.

## Evidence

The doctrine, in this crate's state module — `services/service-com-amazonaws-aiops-aiops/src/state.rs:336-340`:
```
// Two wordings, each single-homed here so every operation states the same failure
// the same way: the `ValidationException` message a bad input is refused with
// ([`validation_exception_message`], [`validation_message`]) and the
// `ResourceNotFoundException` message an absent resource is reported with
// ([`investigation_group_not_found`], [`investigation_group_policy_not_found`]).
```
and, for a COMPOSITION of rules, `src/state.rs:936-944`:
```
/// The individual rules are single-homed above; so is their composition, because the
/// chain is itself part of the contract: the rules are applied in the order
/// [`encryption_configuration_rejection`], [`cross_account_configurations_rejection`],
/// [`collection_bounds_rejection`], and the FIRST one with something to say is what
/// the request is refused with, so a request violating more than one of them is
/// refused identically whichever operation carried it.
```

Copy 1 — `src/ops/get_investigation_group_policy.rs:167-181`:
```
        // Which resource is missing decides which 404 this answers: an absent GROUP is
        // reported as the absent group — the policy operations report the group they
        // could not find, not a policy on a group that does not exist — and a group
        // that exists with no policy attached is reported as the absent policy, so a
        // client can tell the two conditions apart.
        let group = store.group(&input.identifier, &region).ok_or_else(|| {
            GetInvestigationGroupPolicyErrorModeled::ResourceNotFoundException(
                crate::state::investigation_group_not_found(),
            )
        })?;
        let policy = group.policy.clone().ok_or_else(|| {
            GetInvestigationGroupPolicyErrorModeled::ResourceNotFoundException(
                crate::state::investigation_group_policy_not_found(),
            )
        })?;
```

Copy 2 — `src/ops/delete_investigation_group_policy.rs:160-180`, whose comment names the other copy as the rule's home:
```
        // Which resource is missing decides which 404 this answers, the same way
        // `GetInvestigationGroupPolicy` reports it: an absent GROUP is reported as the
        // absent group, and a group that exists while carrying no policy is reported
        // as the absent policy, so a client can tell the two conditions apart.
        let group = store.group_mut(&input.identifier, &region).ok_or_else(|| {
            DeleteInvestigationGroupPolicyErrorModeled::ResourceNotFoundException(
                crate::state::investigation_group_not_found(),
            )
        })?;
        ...
        if group.policy.take().is_none() {
            return Err(
                DeleteInvestigationGroupPolicyErrorModeled::ResourceNotFoundException(
                    crate::state::investigation_group_policy_not_found(),
                )
                .into(),
            );
        }
```

Test coverage that must stay green and unchanged: `tests/all/investigation_group_policy_delete.rs:9-14` states the rule for the delete side ("a call that has no document to remove is a modelled `ResourceNotFoundException`, naming the GROUP when no group matches `{identifier}` and the POLICY when the group exists while carrying none"); the get side is pinned in `tests/all/investigation_groups.rs:784-860` and the message wording in `tests/all/error_message_rules.rs:528-617`.

## Steps to Reproduce

1. Read `services/service-com-amazonaws-aiops-aiops/src/ops/get_investigation_group_policy.rs:167-181`: the group-404-then-policy-404 branch, with the rule stated in its comment
2. Read `services/service-com-amazonaws-aiops-aiops/src/ops/delete_investigation_group_policy.rs:160-180`: the same branch again, its comment deferring to the Get handler for the rule
3. Note that `grep -n 'investigation_group_policy_not_found' services/service-com-amazonaws-aiops-aiops/src` shows the selection between the two wordings happening in both handlers, while `crate::state` owns only the wordings — unlike `write_configuration_rejection`, which owns the create/update chain for exactly this reason (`src/state.rs:936-944`)
4. Change which exception either step answers in one handler only: the crate still compiles and the other operation keeps the old behaviour, which is the drift this duplication allows
