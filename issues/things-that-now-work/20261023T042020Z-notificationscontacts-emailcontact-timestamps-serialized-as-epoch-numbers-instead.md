Issue-Source: trace-sweep-divergence
Issue-Trace: notificationscontacts-new-email-contact-starts-inactive
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: notificationscontacts-new-email-contact-starts-inactive
Issue-Shape-Id: com.amazonaws.notificationscontacts#NotificationsContacts
Issue-Key: notificationscontacts|GetEmailContact|2xx->2xx|$.emailContact.creationTime
Issue-Key: notificationscontacts|GetEmailContact|2xx->2xx|$.emailContact.updateTime
Issue-Commit: 6288a0cc473439e64f80da5d6e6b2c53557408ab

# notificationscontacts EmailContact timestamps serialized as epoch numbers instead of date-time strings

> _Note: this issue was identified against the code base at commit `6288a0cc473439e64f80da5d6e6b2c53557408ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEmailContact

## Description
Cause 1 of 1: GetEmailContact serializes the EmailContact timestamp members with the wrong wire shape.

AWS: Smithy model services/service-com-amazonaws-notificationscontacts-notificationscontacts/smithy-model/com-amazonaws-notificationscontacts-notificationscontacts.json declares EmailContact.creationTime (target CreationTime) and EmailContact.updateTime (target UpdateTime) as `timestamp` members each carrying @timestampFormat("date-time"), and both are @required. GetEmailContact is `GET /emailcontacts/{arn}` returning HTTP 200 with a required `emailContact` member. With an explicit date-time timestampFormat, AWS emits these two members as RFC3339/ISO-8601 date-time STRINGS in the JSON body (e.g. "2026-01-20T12:19:38.653Z"), not as numeric values. The recorded response for this fixture asserts the shape of both members rather than a literal instant (the recording's expectation is '<shaped-not-asserted>'), because the instant itself is environment-specific; the required wire form is still a date-time string.

Emulator: the replayed GetEmailContact answers 200 (status matches) but renders both members as JSON numbers of epoch seconds with a fractional part -- $.emailContact.creationTime = 1792729178.653 and $.emailContact.updateTime = 1792729178.653 -- so each member diverges on type/shape, not merely on value.

Why: a client decoding this response with a generated AWS SDK parses these members under @timestampFormat("date-time"), i.e. it expects a string to hand to an RFC3339 parser. Receiving a JSON number makes the SDK raise a deserialization/type error on a @required member, so GetEmailContact fails outright for the caller rather than returning a contact whose timestamps are merely imprecise. Anything that reads a contact -- polling until an email contact leaves the inactive state, auditing creation/update times -- cannot complete against the emulator.

Fix: the GetEmailContact response serializer for notificationscontacts (the EmailContact structure serialization shared by the contact read/list paths) must honour the member-level @timestampFormat("date-time") and emit an RFC3339 date-time string for creationTime and updateTime instead of defaulting to the protocol's epoch-seconds numeric timestamp encoding. The same EmailContact serializer is reached from the other contact operations, so fixing it at the structure level covers them rather than patching GetEmailContact alone.

Traces: fixture notificationscontacts-new-email-contact-starts-inactive, step task#2 (GetEmailContact, expected 200 / actual 200, diverging keys $.emailContact.creationTime and $.emailContact.updateTime). Model evidence: EmailContact.creationTime / EmailContact.updateTime, timestampFormat date-time, both required.

## Evidence
Expected (AWS / Smithy EmailContact.creationTime, EmailContact.updateTime with @timestampFormat("date-time"), both @required): HTTP 200 with each member as an RFC3339 date-time string, e.g. "emailContact": {"creationTime": "2026-01-20T12:19:38.653Z", "updateTime": "2026-01-20T12:19:38.653Z", ...}. The recorded fixture asserts the shape of these members, not a literal instant.

Actual (emulator, replay of notificationscontacts-new-email-contact-starts-inactive step task#2): HTTP 200 with numeric epoch-seconds values -- detail from replay: "$.emailContact.creationTime: value_mismatch (expected '<shaped-not-asserted>', got 1792729178.653)" and "$.emailContact.updateTime: value_mismatch (expected '<shaped-not-asserted>', got 1792729178.653)". Verdict: mismatch; status matched 200->200, so the divergence is purely the timestamp wire encoding.

## Steps to Reproduce
1. Boot the emulator from this checkout (notificationscontacts is bootstrapped per get_bootstrap_status).
2. Replay wire fixture notificationscontacts-new-email-contact-starts-inactive: it creates an email contact and then, at step task#2, calls GetEmailContact with GET /emailcontacts/{arn} for the new contact's ARN.
3. Inspect the 200 response body: $.emailContact.creationTime and $.emailContact.updateTime come back as JSON numbers (epoch seconds with a fraction, e.g. 1792729178.653).
4. Expected: both members are RFC3339 date-time strings as required by @timestampFormat("date-time") on EmailContact.creationTime / EmailContact.updateTime. Equivalently, calling GetEmailContact through an AWS SDK client fails to deserialize the response instead of returning the contact.
