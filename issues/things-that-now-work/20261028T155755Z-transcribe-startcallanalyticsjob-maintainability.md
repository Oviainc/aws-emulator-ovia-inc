Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: dc8acfad318c0cd148284152a9b68532fd3f8c27

# The channel-definition distinctness rule is implemented twice, once per start that takes ChannelDefinitions

_Filed: 2026-10-28T15:57:55Z_
_Operation: StartCallAnalyticsJob | Class: maintainability_

> _Note: this issue was identified against the code base at commit `dc8acfad318c0cd148284152a9b68532fd3f8c27`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "the two channel definitions of a start must name different channels and different participant roles" is open-coded in both starts that accept `ChannelDefinitions`: each builds its own `BTreeSet`s of channel ids and roles, compares the set sizes against the list length, and returns the same refusal sentence. The two copies have already drifted in the one place they legitimately differ (`ChannelDefinition$ParticipantRole` is optional for Call Analytics, required for Medical Scribe), which is exactly the kind of divergence this crate single-homes such rules to prevent: every other cross-family request rule here lives once in `state.rs` (`co_required_setting_error`, `missing_companion_error`, `time_range_co_requirement_error`, `content_source`, `admit_start`) and is worded once.

The fix is to move the computation into `state.rs` beside those rules, taking the roles as `Option<&str>` so the optional-role case stays exactly as it is, and to have both handlers lift the single refusal text into their own error channel. Every response and every existing test stays byte-for-byte unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-transcribe-transcribe/src/ops/start_medical_scribe_job.rs:310-326:
        if let Some(definitions) = input.channel_definitions.as_deref() {
            let distinct_channels = definitions
                .iter()
                .map(|definition| definition.channel_id)
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            let distinct_roles = definitions
                .iter()
                .map(|definition| definition.participant_role.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            if distinct_channels != definitions.len() || distinct_roles != definitions.len() {
                return Err(bad_request(
                    "Each ChannelDefinition must name a different ChannelId and a different \
                     ParticipantRole. Correct your request and try again.",
                ));

Copy 2 — services/service-com-amazonaws-transcribe-transcribe/src/ops/start_call_analytics_job.rs:305-324:
        if let Some(definitions) = input.channel_definitions.as_deref() {
            let distinct_channels = definitions
                .iter()
                .map(|definition| definition.channel_id)
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            let named_roles: Vec<&str> = definitions
                .iter()
                .filter_map(|definition| definition.participant_role.as_deref())
                .collect();
            let distinct_roles = named_roles
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            if distinct_channels != definitions.len() || distinct_roles != named_roles.len() {
                return Err(bad_request(
                    "Each ChannelDefinition must name a different ChannelId and a different \
                     ParticipantRole. Correct your request and try again.",
                ));

The crate's own stated convention for such a rule — services/service-com-amazonaws-transcribe-transcribe/src/state.rs:1121-1125:
/// Single-homed here because every such pair of this service — the settings toggles and
/// the values that qualify them ([`co_required_setting_error`]), and the bounded
/// segment of a time range ([`time_range_co_requirement_error`]) — is refused the same
/// way, and because Smithy's constraint traits cannot express a conditional
/// co-requirement, so no boundary validation can refuse one.

Tests that pin both copies (and the optional-role branch), so the move is verifiable without changing any expectation:
- tests/all/medical_scribe_jobs.rs:570-597 ("two channel definitions naming the same channel", "... the same participant role"), asserted at tests/all/medical_scribe_jobs.rs:600-604 as 400 BadRequestException
- tests/all/call_analytics_jobs.rs:818-839 (the same two cases), asserted at tests/all/call_analytics_jobs.rs:842-852
- tests/all/call_analytics_jobs.rs:894-904: "`ChannelDefinition$ParticipantRole` is OPTIONAL, so two definitions that name ..." with ChannelDefinitions [{"ChannelId": 0}, {"ChannelId": 1}] accepted

## Steps to Reproduce

1. Open services/service-com-amazonaws-transcribe-transcribe/src/ops/start_medical_scribe_job.rs:310-326 and services/service-com-amazonaws-transcribe-transcribe/src/ops/start_call_analytics_job.rs:305-324 side by side: the same distinctness computation and the same refusal sentence appear twice
2. Compare with services/service-com-amazonaws-transcribe-transcribe/src/state.rs:1104-1268, where every other cross-family request rule of this crate (co_required_setting_error, missing_companion_error, time_range_co_requirement_error, rule_time_range_error) is single-homed
3. Run the crate's tests (tests/all/medical_scribe_jobs.rs, tests/all/call_analytics_jobs.rs): both copies are pinned, so folding them into one state.rs helper taking Option<&str> roles leaves every assertion unchanged
