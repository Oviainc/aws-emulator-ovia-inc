Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: fffc92c5e8ca342c0e1949ba8f9a858d2cea7932

# Behind the OpenHours/RestrictedPeriods unions only the dailyHours KEYS are checked: TimeRange and RestrictedPeriod patterns are accepted and stored unvalidated

_Filed: 2026-10-28T22:29:11Z_
_Operation: UpdateCampaignCommunicationTime | Class: validation_

> _Note: this issue was identified against the code base at commit `fffc92c5e8ca342c0e1949ba8f9a858d2cea7932`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::communication_time_violation` is the one check this crate performs on a `CommunicationTimeConfig`, and it exists precisely because the centralized constraint walk does not descend into the `OpenHours` union. But it only inspects the `dailyHours` MAP KEYS. Everything else behind that same union — `TimeRange$startTime` / `TimeRange$endTime` (`@pattern ^T\d{2}:\d{2}$`, both `@required`) and, behind the sibling `RestrictedPeriods` union, `RestrictedPeriod$startDate` / `$endDate` (`@pattern ^\d{4}-\d{2}-\d{2}$`) — is unreached by the same walk and unchecked by the handler, so `UpdateCampaignCommunicationTime` (and `CreateCampaign`, which calls the same helper) answers 200 and PERSISTS values the model's own patterns reject. `DescribeCampaign` then echoes them back inside `Campaign$communicationTimeConfig`, so the stored record and every later response carry a `TimeRange` outside the shape's declared form.

The handler's own comment names the exact reason a hand check is needed there, and then applies it to one of the three constrained things behind the union.

## Evidence

Smithy, `TimeRange`: `"startTime": {"target": "Iso8601Time", "required": true, "pattern": "^T\\d{2}:\\d{2}$"}, "endTime": {... "pattern": "^T\\d{2}:\\d{2}$"}` and `RestrictedPeriod`: `"startDate": {"target": "Iso8601Date", "required": true, "pattern": "^\\d{4}-\\d{2}-\\d{2}$"}`; `OpenHours` is `{"type": "union", "members": {"dailyHours": {"target": "DailyHours", "type": "map"}}}` whose value is `TimeRangeList`.

The crate states the unreachability itself — services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/state.rs:367-370: "Those keys sit behind the `OpenHours` union, which the centralized constraint walk does not reach, so without this check a campaign could be stored opening a day the model does not model."

And the helper under that doc checks ONLY the keys — state.rs:387-394:
```
        let OpenHours::dailyHours(daily_hours) = &window.open_hours;
        // Sorted so a config carrying more than one unmodelled day is always refused
        let mut days: Vec<&str> = daily_hours.keys().map(String::as_str).collect();
        days.sort_unstable();
        if let Some(day) = days
            .into_iter()
            .find(|day| !DAY_OF_WEEK_VALUES.contains(day))
```
No branch reads `TimeRange` or `window.restricted_periods` at all.

Handler side — src/ops/update_campaign_communication_time.rs:178-185: "A `dailyHours` key names a day of the week, the model's own `DayOfWeek` catalog, and that map sits behind the `OpenHours` union, which the centralized constraint walk does not reach: a key naming no day is refused here as a bad request" followed by the single `communication_time_violation` call, then `campaign.communication_time_config = Some(input.communication_time_config);` (line 263) storing the config verbatim.

Test coverage: tests/all/campaign_communication_time_update.rs and tests/all/campaign_configuration_is_one_rule.rs only ever send well-formed `{"startTime": "T08:00", "endTime": "T18:00"}` rows (campaign_configuration_is_one_rule.rs:111), so no test pins the unvalidated form.

## Steps to Reproduce

1. Onboard a Connect instance and `CreateCampaign` to get a campaign id.
2. `POST /v2/campaigns/{id}/communication-time` with `{"communicationTimeConfig": {"localTimeZoneConfig": {"defaultTimeZone": "America/New_York"}, "telephony": {"openHours": {"dailyHours": {"MONDAY": [{"startTime": "not-a-time", "endTime": "99:99"}]}}}}}`.
3. Observe HTTP 200 instead of the modelled `ValidationException` (HTTP 400) the `^T\d{2}:\d{2}$` pattern calls for.
4. `GET /v2/campaigns/{id}` and observe `campaign.communicationTimeConfig.telephony.openHours.dailyHours.MONDAY[0].startTime == "not-a-time"` — the pattern-violating value was persisted and is now served back.
5. Repeat with `"restrictedPeriods": {"restrictedPeriodList": [{"startDate": "01/01/2024", "endDate": "nope"}]}` for the same result.
