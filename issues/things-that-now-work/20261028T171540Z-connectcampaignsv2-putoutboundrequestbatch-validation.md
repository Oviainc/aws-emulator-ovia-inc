Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 68e4a9e742a6835063083b9abc795c05ac8a482e

# Attributes/templateParameters map keys are never checked against AttributeName's pattern, unlike every other unreachable map key this crate compensates for

_Filed: 2026-10-28T17:15:40Z_
_Operation: PutOutboundRequestBatch | Class: validation_

> _Note: this issue was identified against the code base at commit `68e4a9e742a6835063083b9abc795c05ac8a482e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every `channelSubtypeParameters` variant `PutOutboundRequestBatch` accepts carries an `Attributes` map: `telephony.attributes` (`@required`) and `sms`/`email`/`whatsApp`'s `templateParameters` (`@required`). The model declares that map's KEY shape as `AttributeName`, with `@pattern "^[a-zA-Z0-9\\-_]+$"` and `@length(min: 0, max: 32767)`.

Nothing checks those keys:

* this crate states in its own words that the framework's centralized constraint walk never validates a map key — "a map key is captured as a path segment, never validated as a leaf" — which is exactly why it hand-rolls a key check for every OTHER constrained map key it accepts: `TagMap`'s keys go through `crate::state::tag_keys_violation` (from `TagResource`, `UntagResource` and `CreateCampaign`), and the `DayOfWeek`-keyed `dailyHours` map behind the `OpenHours` union goes through `crate::state::communication_time_violation` (from `UpdateCampaignCommunicationTime` and `CreateCampaign`);
* the generated input type holds the map as `HashMap<String, String>`, so there is no typed key to constrain it;
* `PutOutboundRequestBatch`'s handler touches the union only to classify it into one of four channel names (`requested_channel_subtype`) and adds no check of its own.

So a request carrying `attributes: {"Customer Id!": "c-1"}` — a key the modelled `@pattern` forbids, as is the empty key `""` (the pattern's `+` requires at least one character) — is accepted: the handler mints a `DialRequestId`, reports the request in `successfulRequests` and answers HTTP 200, where the modelled `ValidationException` (HTTP 400) is what the key shape's constraints call for and what the two sibling map-key checks answer for their own off-shape keys.

This is a different gap from the open `PutOutboundRequestBatch` parameter-ignored issue, which asserts these values are "validated by the boundary walk" and then dropped: the map's VALUES (`AttributeValue`) are indeed validated as leaves, but its KEYS are not validated at all, so the refusal is missing whether or not the parameters are ever persisted.

## Evidence

The model's key constraints — `smithy_query(query="shape", shape="Attributes")`:

```
"Attributes": {"type": "map", "sensitive": true,
  "key": {"target": "AttributeName", "length": {"min": 0, "max": 32767},
          "pattern": "^[a-zA-Z0-9\\-_]+$"},
  "value": {"target": "AttributeValue", "length": {"min": 0, "max": 32767}, "pattern": "^.*$"}}
```
and `smithy_query(query="shape", shape="AttributeName")`: `{"type": "string", "length": {"min": 0, "max": 32767}, "pattern": "^[a-zA-Z0-9\\-_]+$", "doc": "The key of the attribute. Attribute keys can include only alphanumeric, dash, and underscore characters."}`

The four `@required` maps that reach this operation — services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/ops/types/put_outbound_request_batch_types.rs:204-210, :214-219, :223-228, :232-237:
```
pub struct TelephonyChannelSubtypeParameters {
    pub destination_phone_number: String,
    pub attributes: std::collections::HashMap<String, String>,
...
pub struct SmsChannelSubtypeParameters {
...
    pub template_parameters: std::collections::HashMap<String, String>,
```
(no typed key: `HashMap<String, String>`).

This crate's OWN statement that a constrained map key reaches the handler unchecked — src/ops/create_campaign.rs:216-222:
```
        // A tag map's KEYS are a constrained shape like any other input member, and
        // the centralized walk reaches neither their `@length` (a map key is captured
        // as a path segment, never validated as a leaf) nor their `@pattern` (whose
        // negative lookahead the pattern engine cannot compile), so they are checked
        // here.
```
and src/state.rs:205-213 (doc of `tag_keys_violation`):
```
/// Tag keys reach a handler UNCHECKED, for two reasons that compound: the boundary
/// walk captures a map's key as the path segment of that entry's value rather than
/// validating it as a leaf of its own, ... So this is the one place the key shape's `@length` and `@pattern` are
/// enforced, shared by every operation that takes a tag map
```

The sibling compensations that DO exist for the only two other constrained map keys in this service — src/ops/tag_resource.rs:186-188:
```
        if let Some(message) =
            crate::state::tag_keys_violation("tags", input.tags.keys().map(String::as_str))
```
and src/ops/update_campaign_communication_time.rs:178-185:
```
        // A `dailyHours` key names a day of the week, the model's own `DayOfWeek`
        // catalog, and that map sits behind the `OpenHours` union, which the
        // centralized constraint walk does not reach: a key naming no day is refused
        // here as a bad request, before anything is looked up or stored.
        if let Some(message) = crate::state::communication_time_violation(
```

The handler that adds no such check — the ONLY use it makes of the union, src/ops/put_outbound_request_batch.rs:156-163:
```
fn requested_channel_subtype(parameters: &ChannelSubtypeParameters) -> &'static str {
    match parameters {
        ChannelSubtypeParameters::Telephony(_) => "TELEPHONY",
```
and the whole per-request walk, src/ops/put_outbound_request_batch.rs:240-268, which reads only `request.channel_subtype_parameters` (for the variant name), `request.client_token` and `request.expiration_time`:
```
        for request in &input.outbound_requests {
            let channel_subtype = requested_channel_subtype(&request.channel_subtype_parameters);
            ...
            let outcome = store.take_outbound_request(
```
No `*_violation` helper is called anywhere in put_outbound_request_batch.rs (`grep -n "_violation" src/ops/put_outbound_request_batch.rs` matches nothing).

Test coverage: every case in the suite sends only in-shape keys or an empty map, so nothing pins the current acceptance — tests/all/instance_onboarding_tags_and_outbound_requests.rs:114-120 (`fn telephony_request(client_token: &str, phone: &str, attributes: Value)` → `"attributes": attributes`) is called only with `json!({})` and `json!({"CustomerId": "c-1"})`-style maps, tests/all/instance_onboarding_tags_and_outbound_requests.rs:932 and :1135-1153 send `"templateParameters": {}`, and tests/all/outbound_request_expiration.rs:68 / tests/all/region_scoping.rs:267 send `"attributes": {}`.

## Steps to Reproduce

1. Create an Amazon Connect instance, then `PUT /v2/connect-instance/{instance}/onboarding` and wait out `crate::state::ONBOARDING_JOB_DURATION` so the job reports SUCCEEDED.
2. `PUT /v2/campaigns` with that `connectInstanceId`, a name, and a `channelSubtypeConfig` holding a `telephony` block; keep the returned campaign `id`.
3. `POST /v2/campaigns/{id}/start` so the campaign is `Running`.
4. `PUT /v2/campaigns/{id}/outbound-requests` with `{"outboundRequests": [{"clientToken": "t1", "expirationTime": "<future RFC 3339>", "channelSubtypeParameters": {"telephony": {"destinationPhoneNumber": "+15555550100", "attributes": {"Customer Id!": "c-1"}}}}]}` — the key `Customer Id!` contains a space and a `!`, neither admitted by `AttributeName`'s `@pattern ^[a-zA-Z0-9\-_]+$`.
5. Observe HTTP 200 with the request listed in `successfulRequests` and a minted `id`, instead of the modelled `ValidationException` (HTTP 400).
6. Repeat with the empty key `{"": "c-1"}` (the pattern's `+` admits no empty string): also 200.
7. Repeat for the other three variants, e.g. `{"sms": {"destinationPhoneNumber": "+15555550101", "templateParameters": {"Customer Id!": "v"}}}` against a campaign configured for SMS: also 200.
8. For contrast, send the analogous off-shape key to a sibling map — `POST /v2/tags/{campaignArn}` with `{"tags": {"bad key!": "v"}}`, or `POST /v2/campaigns/{id}/communication-time` with a `dailyHours` key of `"FUNDAY"` — both are refused HTTP 400 `ValidationException` by the hand-rolled key checks in src/ops/tag_resource.rs:186 and src/ops/update_campaign_communication_time.rs:182.
