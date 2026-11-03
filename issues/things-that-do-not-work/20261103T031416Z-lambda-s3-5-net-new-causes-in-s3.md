Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.bucket-notification-filter-rule-names-echo-capitalised lambda__s3.bucket-notification-rejected-when-lambda-grants-s3-no-invoke lambda__s3.s3-notification-config-rejects-permission-scoped-to-a-different-bucket lambda__s3.overlapping-lambda-notification-filters-rejected lambda__s3.unsupported-notification-event-name-rejected lambda__s3.armed-notification-converts-csv-and-suppresses-uppercase-suffix
Issue-Key: lambda__s3.bucket-notification-filter-rule-names-echo-capitalised|run#11|AssertionError|assert names == ["Prefix", "Suffix"], (
Issue-Key: lambda__s3.bucket-notification-rejected-when-lambda-grants-s3-no-invoke|_err#12|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error")
Issue-Key: lambda__s3.s3-notification-config-rejects-permission-scoped-to-a-different-bucket|_err#15|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it, got {result!r}")
Issue-Key: lambda__s3.overlapping-lambda-notification-filters-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: lambda__s3.unsupported-notification-event-name-rejected|run#8|AssertionError|assert (code, message) == ("InvalidArgument", _UNSUPPORTED_EVENT_MESSAGE), (
Issue-Key: lambda__s3.armed-notification-converts-csv-and-suppresses-uppercase-suffix|run#14|Timeout|time.sleep(5.0)
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# lambda__s3: 5 net-new causes in S3 bucket-notification configuration — destination-permission and overlap validation missing, an unsupported event name answers MalformedXML, filter-rule names are not re-cased, and delivery never happens (check hangs)

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Five distinct root causes under the suite heading `lambda__s3`, all in S3's bucket-notification surface. A fixer must close all five before submitting. (The Lambda layer-from-S3 causes in the same service are filed separately.)

Cause 1 of 5: filter-rule `Name` is echoed in the caller's case instead of S3's canonical capitalisation.
AWS: S3 normalises `Filter.Key.FilterRules[].Name` to `Prefix` / `Suffix` whatever case the caller sent, so GetBucketNotificationConfiguration always reports the capitalised form.
Emulator: rules sent as `prefix`/`suffix` read back as `['prefix', 'suffix']`.
Why: every caller that matches rule names against `Prefix`/`Suffix` — including CloudFormation drift readers and the SDK examples — behaves differently against the emulator.
Fix: s3's notification-configuration state/serializer — canonicalise the name on write and emit the capitalised form on read.
Traces: lambda__s3.bucket-notification-filter-rule-names-echo-capitalised

Cause 2 of 5: destination-permission validation is missing — a configuration naming a Lambda that grants `s3.amazonaws.com` no invoke permission, or whose grant is scoped by SourceArn to a DIFFERENT bucket, is accepted.
AWS: S3 validates the destination list as a UNIT before storing anything. If any named Lambda does not grant `lambda:InvokeFunction` to `s3.amazonaws.com` for this bucket, the whole call is refused with HTTP 400 `InvalidArgument`, message `Unable to validate the following destination configurations` (fixed text, no variable parts), and the bucket's prior configuration is left exactly as it was (the operation is a full replace, so its survival is the effect-absence proof).
Emulator: the call SUCCEEDS — HTTP 200 with an empty body — in both shapes: a two-destination list where one leg has no grant, and a single destination whose grant names another bucket's ARN.
Why: a misconfigured grant is accepted at configure time, so the failure degrades into silently undelivered events instead of the API refusal AWS gives; and S3's "validate the list as a unit" contract is lost, so a partially valid list can be stored.
Fix: s3's PutBucketNotificationConfiguration destination validation — evaluate each destination's resource policy (principal, SourceArn and SourceAccount conditions) and refuse the whole request.
Traces: lambda__s3.bucket-notification-rejected-when-lambda-grants-s3-no-invoke, lambda__s3.s3-notification-config-rejects-permission-scoped-to-a-different-bucket

Cause 3 of 5: overlapping filter rules for one event type are accepted instead of refused as ambiguous.
AWS: two `LambdaFunctionConfigurations` whose prefix AND suffix overlap for the same event type are refused with HTTP 400 `InvalidArgument`, message `Configuration is ambiguously defined. Cannot have overlapping suffixes in two rules if the prefixes are overlapping for the same event type.` (fixed text, no variable parts).
Emulator: the call SUCCEEDS (`AssertionError: the call succeeded; real AWS rejects it, so there is no error code`).
Why: S3 refuses the ambiguity rather than choosing; an emulator that accepts it lets a client store a configuration whose delivery behaviour is undefined.
Fix: s3's notification-configuration validator — the per-event-type prefix/suffix overlap check.
Traces: lambda__s3.overlapping-lambda-notification-filters-rejected

Cause 4 of 5: an event type S3 does not publish is answered with `MalformedXML` instead of `InvalidArgument`.
AWS: an `Events` entry that is not a supported S3 event type is refused with HTTP 400 `InvalidArgument`, message `The event is not supported for notifications` (fixed text), and this happens regardless of whether the destination holds a grant.
Emulator: HTTP 400 `MalformedXML` / `The XML you provided was not well-formed or did not validate against our published schema` — the request is misclassified as a syntax error.
Why: a client branching on `InvalidArgument` (a semantic refusal it can report to the user as "unsupported event") instead sees a schema error and reports a malformed request it did not send.
Fix: s3's notification-configuration parser/validator — accept the XML, then refuse the unknown event value as InvalidArgument with AWS's message.
Traces: lambda__s3.unsupported-notification-event-name-rejected

Cause 5 of 5: an armed notification never invokes its Lambda target, so the delivery check hangs to the 60 s harness cap.
AWS: a PutObject matching a stored, granted `LambdaFunctionConfigurations` rule invokes the function (which here converts the CSV), and an uppercase suffix is NOT delivered because the suffix filter is case-sensitive — the negative is only meaningful because the positive control fires.
Emulator: nothing is delivered and no error is produced; replay reports `Error: Check timed out after 60.0s`.
Why: the object-driven pipeline is inert, and with no positive control a client cannot distinguish "filtered out" from "never delivered".
Fix: s3's object-write notification dispatcher (match stored rules, then invoke the Lambda destination). Related open issues `20261029T180026Z-dynamodb-lambda-s3-s3-notifications-never-invoke-lambda-are-accepted-without.md` and `20261102T093047Z-lambda-s3-sns-sqs-6-net-new-root.md` describe the same inertness under other headings and do not carry this check's replay key.
Traces: lambda__s3.armed-notification-converts-csv-and-suppresses-uppercase-suffix

## Evidence
Cause 1 — expected `[r['Name'] for r in FilterRules] == ['Prefix', 'Suffix']`; actual `['prefix', 'suffix']`.
Cause 2 — expected ('InvalidArgument', 'Unable to validate the following destination configurations') with the prior configuration untouched; actual: HTTP 200 with an empty body (`{'ResponseMetadata': {... 'HTTPStatusCode': 200 ...}}`), configuration stored.
Cause 3 — expected ('InvalidArgument', 'Configuration is ambiguously defined. Cannot have overlapping suffixes in two rules if the prefixes are overlapping for the same event type.'); actual: the call succeeded.
Cause 4 — expected ('InvalidArgument', 'The event is not supported for notifications'); actual ('MalformedXML', 'The XML you provided was not well-formed or did not validate against our published schema').
Cause 5 — expected the matching PutObject to invoke the function and the uppercase-suffix key not to; actual: no delivery at all, `Error: Check timed out after 60.0s`.

## Steps to Reproduce
1. Replay the behavioral suite and re-run every check id in trace_ids.
2. Cause 1: PUT a configuration with `FilterRules=[{'Name':'prefix',...},{'Name':'suffix',...}]`, then GetBucketNotificationConfiguration and read the names.
3. Cause 2: PUT a two-destination configuration where one Lambda has an s3.amazonaws.com invoke grant and the other has none; separately PUT one whose grant's SourceArn names a different bucket.
4. Cause 3: PUT two LambdaFunctionConfigurations with overlapping prefix+suffix for `s3:ObjectCreated:*`.
5. Cause 4: PUT a configuration whose `Events` contains an unsupported event name.
6. Cause 5: with a granted, suffix-filtered configuration installed, PutObject a matching key and an uppercase-suffix key and watch for invocations.
