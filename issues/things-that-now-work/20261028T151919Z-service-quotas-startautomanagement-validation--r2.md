Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7e4efc65cc687607a3c1322cd06c2fb91d881b32

# Automatic Management accepts and reports the OptInType NotifyAndAdjust, which the model documents as not available

_Filed: 2026-10-28T15:19:19Z_
_Operation: StartAutoManagement | Class: validation_

> _Note: this issue was identified against the code base at commit `7e4efc65cc687607a3c1322cd06c2fb91d881b32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartAutoManagement` stores whatever `OptInType` member the boundary validator lets through, and `GetAutoManagementConfiguration` reports it back. So a caller can configure — and then read back — `OptInType: "NotifyAndAdjust"`, even though the model's documentation for that member says in so many words that only `NotifyOnly` is available. The same sentence is attached to the RESPONSE member too, so the emulator emits a value its own model says the service does not report.

The crate's justification for accepting both members is that "the member carries no constraint narrowing it to a subset" — but the member's own documentation is exactly such a narrowing, stated in prose rather than as a trait. `UpdateAutoManagement` shares the member and the same gap (one fix covers both).

As a result a client that sets `NotifyAndAdjust` against the emulator is told the auto-adjust mode is in force, when nothing in the crate ever auto-adjusts a quota: `request_view` hard-codes `request_type: None` with the comment that `RequestType::AutomaticManagement` "marks a request the Auto Management feature raised; a caller's own request carries none", so the mode the emulator accepts has no behaviour behind it.

## Evidence

The model sentence, from `smithy_query(query="shape", shape="StartAutoManagementRequest")`, member `OptInType`:

    "Sets the opt-in type for Automatic Management. There are two modes: Notify only and Notify and Auto-Adjust. Currently, only NotifyOnly is available."

and on the response, `smithy_query(query="shape", shape="GetAutoManagementConfigurationResponse")`, member `OptInType`:

    "Information on the opt-in type for Automatic Management. There are two modes: Notify only and Notify and Auto-Adjust. Currently, only NotifyOnly is available."

The same sentence is already copied into the handlers' own docs — services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/ops/start_auto_management.rs:62-64:

    //! * `OptInType` (OptInType, one of: NotifyAndAdjust | NotifyOnly) **required** — Sets the
    //!   ... Auto-Adjust. Currently, only NotifyOnly is available.

The handler nonetheless stores the member unchecked — src/ops/start_auto_management.rs:159-165:

        quotas_state.set_auto_management(crate::state::AutoManagementConfig {
            opt_in_level: input.opt_in_level,
            opt_in_type: input.opt_in_type,
            opt_in_status: crate::state::OPT_IN_STATUS_ENABLED.to_string(),
            notification_arn: input.notification_arn,
            exclusion_list,
        });

and the read reports it — src/ops/get_auto_management_configuration.rs:144-150: `opt_in_type: Some(config.opt_in_type.clone()),`.

The validator the crate relies on admits both members — src/ops/types/shared_types.rs:213:

    pub const OPT_IN_TYPE_VALUES: &[&str] = &["NotifyAndAdjust", "NotifyOnly"];

The stated reason the model sentence contradicts — src/state.rs:482-486:

    /// `OptInType` — an `OptInType` member, as the opt-in named it. Every member the
    /// enum declares is stored as given: the member carries no constraint narrowing it
    /// to a subset, so the boundary validator's enum check is the whole of its
    /// validation (see [`crate::ops::types::OPT_IN_TYPE_VALUES`]).

and the test that pins it on that same reason — tests/all/auto_management.rs:814-817 + :825-827:

    /// EVERY `OptInType` member the enum declares is stored as given by both writers of the
    /// member: the member carries no constraint narrowing it to a subset of the enum, so a
    /// value the enum declares is never invalid input, ...
    async fn every_modeled_opt_in_type_is_stored_by_both_writers() {
        for requested in OPT_IN_TYPE_VALUES {

No auto-adjust behaviour exists behind the accepted mode — src/state.rs:1305-1308 (`request_view`): `// `RequestType` has one member, `AutomaticManagement`, ... a caller's own request carries none.` followed by `request_type: None,`.

## Steps to Reproduce

1. Start the emulator and sign requests with a us-east-1 SigV4 credential scope.
2. POST / with X-Amz-Target: ServiceQuotasV20190624.StartAutoManagement and body {"OptInLevel":"ACCOUNT","OptInType":"NotifyAndAdjust"} — observe HTTP 200 with an empty body instead of the IllegalArgumentException the model's "Currently, only NotifyOnly is available" implies.
3. POST / with X-Amz-Target: ServiceQuotasV20190624.GetAutoManagementConfiguration and body {} — observe {"OptInLevel":"ACCOUNT","OptInType":"NotifyAndAdjust","OptInStatus":"ENABLED"}, i.e. the response reports a mode its own modelled member documents as unavailable.
4. Repeat with UpdateAutoManagement and body {"OptInType":"NotifyAndAdjust"} over a running configuration — same acceptance.
5. Confirm nothing acts on the mode: make a RequestServiceQuotaIncrease and read it back with GetRequestedServiceQuotaChange; RequestType is always absent, so no request is ever attributed to Automatic Management.
