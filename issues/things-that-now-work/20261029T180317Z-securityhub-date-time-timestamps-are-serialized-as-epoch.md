Issue-Source: trace-sweep-divergence
Issue-Trace: securityhub-automation-rule-and-members-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityhub-automation-rule-and-members-lifecycle
Issue-Shape-Id: com.amazonaws.securityhub#SecurityHubAPIService
Issue-Key: securityhub|BatchGetAutomationRules|2xx->2xx|$.Rules[*].CreatedAt
Issue-Key: securityhub|BatchGetAutomationRules|2xx->2xx|$.Rules[*].UpdatedAt
Issue-Key: securityhub|GetMembers|2xx->2xx|$.Members[*].UpdatedAt
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# securityhub date-time timestamps are serialized as epoch numbers instead of ISO-8601 strings

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetAutomationRules

## Description
Every securityhub timestamp member the corpus observes is emitted as a bare epoch-seconds number with a fractional part, where the model binds the ISO-8601 date-time wire form. 2 causes, one serializer fault.

Cause 1 of 2 — BatchGetAutomationRules rule timestamps (fixture securityhub-automation-rule-and-members-lifecycle, step task#3).
AWS: Smithy: AutomationRulesConfig.CreatedAt and AutomationRulesConfig.UpdatedAt both target com.amazonaws.securityhub#Timestamp with @timestampFormat("date-time"), so the wire value is an RFC-3339/ISO-8601 string (e.g. a 'YYYY-MM-DDThh:mm:ss.sssZ' form), not a number. Recorded behavior: both members arrive as date-time strings (the corpus asserts their shape, not the instant, since the instant is per-run).
Emulator: emits JSON numbers — CreatedAt and UpdatedAt are epoch-seconds floats, so the asserted date-time shape does not hold.
Why: a date-time-bound member deserializes from a string; handed a number, SDK clients raise a deserialization/parse error on the whole response rather than on one field, so BatchGetAutomationRules is unusable from a typed client even when the rule data is correct. Hand-rolled callers that parse the string get a type error.
Fix: the securityhub response serializer's timestamp handling (shared by all @timestampFormat("date-time") members), not these two operations individually.

Cause 2 of 2 — GetMembers member timestamp (fixture securityhub-automation-rule-and-members-lifecycle, step task#8).
AWS: Smithy: Member.UpdatedAt (and Member.InvitedAt) target Timestamp with @timestampFormat("date-time") — same ISO-8601 string wire form. Recorded behavior: UpdatedAt arrives as a date-time string.
Emulator: emits an epoch-seconds float for Members[0].UpdatedAt.
Why: same as cause 1 — GetMembers responses fail to deserialize in typed clients, so member-account enumeration breaks.
Fix: the same shared date-time serializer path; the per-operation handlers should keep storing instants and let the serializer format them.

Note on scope: the divergence is the JSON TYPE of the member, not the instant it carries — the corpus does not assert the clock value, which differs per run.

Traces: securityhub-automation-rule-and-members-lifecycle#task#3 (BatchGetAutomationRules CreatedAt/UpdatedAt), securityhub-automation-rule-and-members-lifecycle#task#8 (GetMembers Members[0].UpdatedAt).

## Evidence
Cause 1 — securityhub-automation-rule-and-members-lifecycle#task#3 BatchGetAutomationRules (200 vs 200): $.Rules[0].CreatedAt value_mismatch (expected a date-time-shaped value, got 1793296882.3201616); $.Rules[0].UpdatedAt value_mismatch (expected a date-time-shaped value, got 1793296882.3201616).
Cause 2 — securityhub-automation-rule-and-members-lifecycle#task#8 GetMembers (200 vs 200): $.Members[0].UpdatedAt value_mismatch (expected a date-time-shaped value, got 1793296882.5226219).
Model: com.amazonaws.securityhub#Timestamp carries timestampFormat 'date-time'; AutomationRulesConfig.CreatedAt/UpdatedAt and Member.InvitedAt/UpdatedAt all target it.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 corpus (replay_wire_corpus). Cause 1: fixture securityhub-automation-rule-and-members-lifecycle step task#3 — CreateAutomationRule then BatchGetAutomationRules with its ARN and inspect the JSON type of Rules[0].CreatedAt and Rules[0].UpdatedAt. Cause 2: same fixture step task#8 — CreateMembers then GetMembers for that account and inspect the JSON type of Members[0].UpdatedAt. Both should be ISO-8601 strings, and both come back as numbers.
