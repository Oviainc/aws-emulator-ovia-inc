Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 804fb9e39527c7d8ad6b71682b2c9d408f11768a

# StopAutoManagement keeps the whole Automatic Management configuration although the model says it removes all associated configurations

_Filed: 2026-10-28T16:22:14Z_
_Operation: StopAutoManagement | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `804fb9e39527c7d8ad6b71682b2c9d408f11768a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model's own documentation for `StopAutoManagement` states that the operation stops Automatic Management AND removes all associated configurations. The handler only flips `OptInStatus` to `DISABLED` and leaves the stored record in place, so every preference the stopped opt-in carried — `OptInLevel`, `OptInType`, `NotificationArn` and the whole `ExclusionList` — survives the stop and `GetAutoManagementConfiguration` keeps reporting it.

The handler's own module doc reproduces the model sentence ("removes all associated configurations") a few lines above code that does the opposite, and `crate::state::AutoManagementConfig` documents the retention as deliberate ("Stopping keeps the record, so the preferences a later start would otherwise have to be given again are still reported"). That reason is exactly the one the modelled sentence contradicts.

The crate already has the shape of the correct answer: when no configuration is stored at all, `GetAutoManagementConfiguration` answers `OptInStatus: DISABLED` with no other member set (the `None` branch), and both `StopAutoManagement` and `UpdateAutoManagement` already refuse with `NoSuchResourceException` when nothing is running — so clearing the record on stop needs no new state or new error path, only `self.auto_management = None`-style removal instead of a status flip.

## Evidence

Model text (smithy_query(query="doc", shape="StopAutoManagement")):

    "Stops Service Quotas Automatic Management for an Amazon Web Services account and removes all associated\n            configurations. Automatic Management monitors your Service Quotas utilization and notifies you before you\n            run out of your allocated quotas."

The handler quotes that same sentence in its own doc — services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/ops/stop_auto_management.rs:25-27:

    //! Stops Service Quotas Automatic Management for an Amazon Web Services account and removes
    //! all associated configurations. Automatic Management monitors your Service Quotas
    //! utilization and notifies you before you run out of your allocated quotas.

and then removes nothing — src/ops/stop_auto_management.rs:126-127:

            config.opt_in_status = crate::state::OPT_IN_STATUS_DISABLED.to_string();
            quotas_state.set_auto_management(config);

with the retention stated as intended at src/ops/stop_auto_management.rs:5-10:

    //! Stops Automatic Management for the account: the opt-in moves to
    //! `OptInStatus = DISABLED`, and the record stays so the preferences the account
    //! chose are still reported (see [`crate::state::AutoManagementConfig`]).

and at src/state.rs:487-490:

        /// `OptInStatus` — [`OPT_IN_STATUS_ENABLED`] while Automatic Management is
        /// started, [`OPT_IN_STATUS_DISABLED`] once it is stopped. Stopping keeps the
        /// record, so the preferences a later start would otherwise have to be given
        /// again are still reported.

The read then publishes the configuration of a stopped account — src/ops/get_auto_management_configuration.rs:144-150:

        Ok(GetAutoManagementConfigurationOutput {
            opt_in_level: Some(config.opt_in_level.clone()),
            opt_in_type: Some(config.opt_in_type.clone()),
            opt_in_status: Some(config.opt_in_status.clone()),
            notification_arn: config.notification_arn.clone(),
            exclusion_list: (!exclusion_list.is_empty()).then_some(exclusion_list),
        })

while the same handler already has the "nothing stored" answer a removed configuration would produce — src/ops/get_auto_management_configuration.rs:132-139:

        let Some(config) = quotas_state.auto_management() else {
            // Never opted in: nothing is monitored and no preference was ever given,
            // so the only thing to report is that Automatic Management is not running.
            return Ok(GetAutoManagementConfigurationOutput {
                opt_in_status: Some(crate::state::OPT_IN_STATUS_DISABLED.to_string()),
                ..GetAutoManagementConfigurationOutput::default()
            });

Test coverage: tests/all/auto_management.rs:447 `stopping_disables_the_opt_in_and_keeps_the_preferences` pins the current behaviour — tests/all/auto_management.rs:517-521:

        assert_eq!(
            exclusion_codes(&read),
            expected_codes(exclusions),
            "case {case}: stopping must keep the exclusion list on record, got {read}"
        );

and tests/all/auto_management.rs:505-516 asserts the stopped read still reports the `OptInLevel`, `OptInType` and `NotificationArn` of the stopped configuration. That suite's expectation is the one the quoted model sentence contradicts; it also already asserts (auto_management.rs:525-537) that `StopAutoManagement`/`UpdateAutoManagement` are refused `NoSuchResourceException` after a stop, which stays true when the record is removed.

## Steps to Reproduce

1. Boot the emulator and sign requests with a us-east-1 SigV4 credential scope.
2. POST / with X-Amz-Target: ServiceQuotasV20190624.StartAutoManagement and body {"OptInLevel":"ACCOUNT","OptInType":"NotifyOnly","NotificationArn":"arn:aws:notifications::<account>:configuration/quota-alerts","ExclusionList":{"lambda":["L-B99A9384"]}} — accepted.
3. POST / with X-Amz-Target: ServiceQuotasV20190624.StopAutoManagement and body {} — answers 200 with an empty body.
4. POST / with X-Amz-Target: ServiceQuotasV20190624.GetAutoManagementConfiguration and body {} — observe OptInLevel ACCOUNT, OptInType NotifyOnly, NotificationArn and the lambda ExclusionList all still reported alongside OptInStatus DISABLED, although the stop is documented to have removed all associated configurations (expected: OptInStatus DISABLED with no other member, the same answer an account that never opted in gets).
