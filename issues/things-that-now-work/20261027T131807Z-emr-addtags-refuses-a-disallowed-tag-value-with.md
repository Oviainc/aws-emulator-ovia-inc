Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.unicode-tag-value-round-trip
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr|AddTags|InvalidRequestException
Issue-Commit: 550533946ef1f98b21f2c5680405da1eae2993a6

# EMR AddTags refuses a disallowed tag value with its own invented sentence instead of AWS's documented character-class message

> _Note: this issue was identified against the code base at commit `550533946ef1f98b21f2c5680405da1eae2993a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddTags

## Description
One cause.

Cause 1 of 1: `AddTags` correctly refuses a tag value containing a character outside EMR's tag character class with HTTP 400 `InvalidRequestException`, but the `Message` is this emulator's own wording rather than the sentence Amazon EMR answers, so a client that matches or surfaces the refusal text sees text AWS never sends.

AWS: Smithy (services/service-com-amazonaws-emr-elasticmapreduce/smithy-model/com-amazonaws-emr-elasticmapreduce.json) models `AddTags` with errors `InvalidRequestException` (client, HTTP 400) and `InternalServerException` (HTTP 500); `AddTags$Tags` is documented as "a required key string with a maximum of 128 characters, and an optional value string with a maximum of 256 characters" and `Tag$Value` documentation points at the Tag Clusters guide. The model states NO pattern for the key/value character class, so the character class and the refusal wording are recorded behavior only. The recorded behavior for a value carrying a character outside the class (here an emoji, Unicode category So, which is a symbol and not a letter/number/separator/listed punctuation) is HTTP 400, error code `InvalidRequestException`, message:

  Invalid tag value: '<value>'. Tag values may only contain unicode letters, digits, whitespace, or one of these symbols: '_ . : / = + \ -'.

Message template: the fixed text is `Invalid tag value: '` … `'. Tag values may only contain unicode letters, digits, whitespace, or one of these symbols: '_ . : / = + \ -'.` — including the literal symbol enumeration `_ . : / = + \ -` with its spacing and surrounding single quotes. The ONLY variable part is the rejected tag value, taken verbatim from the request's `Tags[].Value` and single-quoted (the recorded run's `no-\U0001f680-go` is a request value filling that slot, not part of the template). The key half of the pair has the corresponding value-free form naming the key member.

Emulator: the same request is refused HTTP 400 `InvalidRequestException` but with message `Tag value 'no-🚀-go' is not valid.` — i.e. the template `Tag <member> '<value>' is not valid.` built by `tag_not_valid_message`/`not_valid_message`, which the code's own doc comment concedes is "this service's own sentence" because "AWS publishes no wording for this class". The emulator's message names no character class at all, so it tells the caller nothing about what a tag value may contain, and its text does not begin with `Invalid tag value:`.

Why: a client (SDK retry/branching logic, a CLI user, or an IaC tool that pattern-matches refusal text) relying on AWS's wording cannot recognise the refusal class, and cannot learn the allowed character set from the response the way a real EMR call teaches it. Tooling that asserts on AWS's documented sentence fails against the emulator even though the status and error code are right.

Fix: the tag validation that produces this refusal's message — `validate_tags` and the message builders it calls in the EMR service state — must emit EMR's documented sentence for an out-of-class tag key/value, with the rejected value interpolated from the request, instead of the generic `'<value>' is not valid.` tail shared with the unknown-resource refusals. The character class the message enumerates should be the one actually enforced (note the enforced class currently also admits `@`, which the documented enumeration does not list, and excludes `\`, which it does).

No existing open issue covers EMR tag-value refusal wording (the nearest filed items — e.g. 20261024T131534Z-appconfig-refusal-messages-…, 20261027T105755Z-elbv2-removetags-… — are other services).

Traces: behavioral check `emr.unicode-tag-value-round-trip` (the multi-script Unicode value round-trips byte-identical and the error CODE is correct; only the refusal message diverges).

## Evidence
Check: emr.unicode-tag-value-round-trip (verdict: mismatch)

Assertion:
  File "check_emr_unicode_tag_value_round_trip.py", line 53, in run
    assert msg == _DISALLOWED_MESSAGE, f"unexpected refusal message: {msg!r}"
  AssertionError: unexpected refusal message: "Tag value 'no-🚀-go' is not valid."

Expected (AWS), HTTP 400 / Error.Code = InvalidRequestException:
  Message = "Invalid tag value: '<Tags[].Value>'. Tag values may only contain unicode letters, digits, whitespace, or one of these symbols: '_ . : / = + \\ -'."
  (fixed text as quoted; '<Tags[].Value>' is the request-supplied value)

Actual (emulator), HTTP 400 / Error.Code = InvalidRequestException:
  Message = "Tag value '<Tags[].Value>' is not valid."

Source of the emulator wording: services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs — validate_tags() -> tag_not_valid_message(member, value) -> not_valid_message(value) = format!("'{value}' is not valid."); the character class is is_tag_char()/TAG_CHARACTER_PATTERN. The handler that calls it: services/service-com-amazonaws-emr-elasticmapreduce/src/ops/add_tags.rs line 150 (validate_tags(&input.tags).map_err(refuse)?) with refuse() at lines 228-234.

What is NOT diverging (so the fix is message-only): the multi-script Unicode value "héllo 日本語_2026.v1:path/to=ok+end-now" round-trips byte-identical through AddTags -> DescribeCluster, the emoji value is refused rather than accepted, the error code is InvalidRequestException, and the rejected key never appears in the cluster's tag map.

## Steps to Reproduce
1. RunJobFlow a single-master cluster (ReleaseLabel emr-6.15.0, Applications [{Name: Hadoop}], one MASTER m5.xlarge instance group, KeepJobFlowAliveWhenNoSteps=true) and take its JobFlowId.
2. AddTags(ResourceId=<cluster id>, Tags=[{Key: "sim2real-unicode", Value: "héllo 日本語_2026.v1:path/to=ok+end-now"}]) — succeeds, and DescribeCluster reports the value byte-identical (this part passes).
3. AddTags(ResourceId=<cluster id>, Tags=[{Key: "sim2real-unicode-bad", Value: "no-\U0001f680-go"}]) — a value containing an emoji, i.e. a character outside EMR's tag character class.
4. Inspect the ClientError: Error.Code is InvalidRequestException (correct) but Error.Message is "Tag value 'no-🚀-go' is not valid." instead of AWS's "Invalid tag value: 'no-🚀-go'. Tag values may only contain unicode letters, digits, whitespace, or one of these symbols: '_ . : / = + \\ -'."
5. Re-replay with: run_behavioral_suite and check emr.unicode-tag-value-round-trip.
