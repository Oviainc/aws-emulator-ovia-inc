Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 95be350e2e64bcff4ad89a5a8e5d1ae15d5b4a08

# The bandwidth-schedule refusal numbers list elements from 0, where this crate's one element-position convention counts from 1

_Filed: 2026-10-29T15:35:19Z_
_Operation: PutBandwidthRateLimitSchedule | Class: error-response_

> _Note: this issue was identified against the code base at commit `95be350e2e64bcff4ad89a5a8e5d1ae15d5b4a08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutBandwidthRateLimitSchedule` is the only operation of this crate that refuses a prose rule about a LIST ELEMENT and names the offending element with the raw 0-based iterator index, while calling it a "position". Every other element-level refusal this crate makes converts the index to the crate's own element position first (`let position = index + 1;`) and hands it to `crate::service::element_member_path`, whose doc states the rule outright: "`position` counts from 1, as a validation path counts a list's elements."

The effect is that one service answers two conventions for the same question ("which element of the list did you refuse?"): a client that sends two `Tags` and gets `tags.2.member.key` is told the SECOND pair, while a client that sends two `BandwidthRateLimitIntervals` and gets "the bandwidth rate limit intervals at positions 0 and 1" is told about positions that do not exist in the crate's own numbering — the second interval is `1` here and `2` everywhere else, and there is no interval `0` in the numbering the sibling refusals use. The refusal's own doc says the sentence exists so "the caller can fix it from the refusal alone", which is exactly what a second, undocumented numbering defeats.

This is not about the custom sentence itself (the crate states a reason for wording these refusals in prose rather than through the constraint envelope, because AWS publishes no wording for them) — it is about the element numbering inside it contradicting the numbering the same crate defines and applies elsewhere.

## Evidence

The 0-based side — `src/state.rs:434-467` (`bandwidth_rate_limit_schedule_violation`), the raw `enumerate()` index printed as a "position":

    for (index, interval) in intervals.iter().enumerate() {
        if minute_of_day(interval.end_hour_of_day, interval.end_minute_of_hour)
            < minute_of_day(interval.start_hour_of_day, interval.start_minute_of_hour)
        {
            return Some(format!(
                "The bandwidth rate limit interval at position {index} ends at \
                 {:02}:{:02}, before it starts at {:02}:{:02}: ...

    ... "The bandwidth rate limit interval at position {index} lists day \
         {day} of the week more than once: ..." (src/state.rs:452-454)

    for (index, interval) in intervals.iter().enumerate() {
        for (other_index, other) in intervals.iter().enumerate().skip(index + 1) {
            if let Some((day, from, through)) = overlapping_day_of_week(interval, other) {
                return Some(format!(
                    "The bandwidth rate limit intervals at positions {index} and \
                     {other_index} both apply on day {day} of the week, ..."   (src/state.rs:462-468)

Its own stated purpose — `src/state.rs:425-428`:

    /// example: the requirement is the error shape, its status and the `ErrorCode`, and the
    /// sentence names the intervals and the values that clash — taken from the request — so the
    /// caller can fix it from the refusal alone.

The rule the crate defines for naming a list element in a refusal — `src/service.rs:162-172`:

    /// The validation path of `member` inside the element at `position` of the list member
    /// `list_member` ...
    ///
    /// `position` counts from 1, as a validation path counts a list's elements.
    pub(crate) fn element_member_path(list_member: &str, position: usize, member: &str) -> String {

and the two sibling prose-rule refusals that apply it — `src/state.rs:2596-2604` (`validate_tag_mappings`):

        // The element's place in the list, counted as a validation path counts it, so a
        // request carrying several mappings is told WHICH one was refused.
        let position = index + 1;
        validate_tag_pair(
            ...
            &crate::service::element_member_path(TAG_MAPPINGS_MEMBER, position, "AwsTagKey"),

and `src/state.rs:2625-2632` (`merged_tags`):

        // As in `validate_tag_mappings`: the position names the pair that was refused.
        let position = index + 1;

Test coverage: the bandwidth suite asserts only that a message exists, so nothing pins the 0-based wording — `tests/all/bandwidth_rate_limit_schedule_consistency.rs:90-98`:

    assert!(
        reply.body.get("Message").and_then(Value::as_str).is_some_and(|message| !message.is_empty()),
        "case {case:?}: the refusal must state why the schedule was refused, got {:?}", reply.body
    );

while the 1-based convention IS pinned for the tag lists — `tests/all/refusal_names_the_member_one_way.rs:79-88` and `:399`:

    /// The validation path of a member inside a list: the list member, the element's position
    /// counted from 1, ...
    fn element_path(list_member: &str, position: usize, member: &str) -> String {
        format!("{list_member}.{position}.member.{member}")
    }
    ...
        for position in [1usize, 2] {

## Steps to Reproduce

1. Start the emulator with the backup-gateway service mounted, perform the activation handshake and CreateGateway to obtain a GatewayArn.
2. POST / with `X-Amz-Target: BackupOnPremises_v20210101.PutBandwidthRateLimitSchedule` and two intervals that clash, e.g. {"GatewayArn":"<arn>","BandwidthRateLimitIntervals":[{"StartHourOfDay":9,"StartMinuteOfHour":0,"EndHourOfDay":17,"EndMinuteOfHour":59,"DaysOfWeek":[1],"AverageUploadRateLimitInBitsPerSec":51200},{"StartHourOfDay":10,"StartMinuteOfHour":0,"EndHourOfDay":12,"EndMinuteOfHour":0,"DaysOfWeek":[1],"AverageUploadRateLimitInBitsPerSec":8000000}]}
3. Observe 400 ValidationException whose Message names "the bandwidth rate limit intervals at positions 0 and 1" — the first and second elements numbered from 0.
4. POST / with `X-Amz-Target: BackupOnPremises_v20210101.TagResource` for the same gateway and two tag pairs whose SECOND value contains a space, e.g. {"ResourceARN":"<arn>","Tags":[{"Key":"fine","Value":"ok"},{"Key":"env","Value":"two words"}]}.
5. Observe 400 ValidationException whose Message names the same (second) element as `tags.2.member.value` — numbered from 1. One service, two numberings for "which element of the list was refused".
