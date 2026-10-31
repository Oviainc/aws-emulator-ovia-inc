Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.unit-is-part-of-a-datapoints-read-identity
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.unit-is-part-of-a-datapoints-read-identity|run#37|AssertionError|assert gms_message == (
Issue-Commit: 004a13bc6b62a404600c0a09686b1d7dd87c7440

# cloudwatch: GetMetricStatistics' InvalidParameterValue for an out-of-enum Unit renders the allowed-unit set in Smithy declaration order and without AWS's bracket padding

> _Note: this issue was identified against the code base at commit `004a13bc6b62a404600c0a09686b1d7dd87c7440`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
One net-new root cause. The failing check was reported neither ALREADY FILED nor OWNED by an open issue, so no existing work item carries it.

=== Cause 1 of 1: the InvalidParameterValue message for a non-enum Unit on the GetMetricStatistics read path is built by iterating the Smithy enum instead of reproducing AWS's sentence ===

AWS:      Smithy: GetMetricStatistics input member `Unit` targets com.amazonaws.cloudwatch#StandardUnit, an enum declaring, in this order, [Seconds, Microseconds, Milliseconds, Bytes, Kilobytes, Megabytes, Gigabytes, Terabytes, Bits, Kilobits, Megabits, Gigabits, Terabits, Percent, Count, Bytes/Second, Kilobytes/Second, Megabytes/Second, Gigabytes/Second, Terabytes/Second, Bits/Second, Kilobits/Second, Megabits/Second, Gigabits/Second, Terabits/Second, Count/Second, None]. `Unit` carries no modeled per-member error, so a string outside the enum surfaces on this path as the query-protocol error `InvalidParameterValue` at HTTP 400 — and the emulator already gets that class right. The MESSAGE is recorded behavior only; AWS returns exactly:
            "The parameter Unit must be a value in the set [ Megabits, Terabits, Gigabits, Count, Bytes, Gigabytes, Gigabytes/Second, Kilobytes, Kilobits/Second, Terabytes, Terabits/Second, Bytes/Second, Percent, Megabytes, Megabits/Second, Milliseconds, Microseconds, Kilobytes/Second, Gigabits/Second, Megabytes/Second, Bits, Bits/Second, Count/Second, Seconds, Kilobits, Terabytes/Second, None ]."
          Breaking that template down: the FIXED text is `The parameter Unit must be a value in the set [ <set> ].` — note the single space immediately inside each bracket and the terminating period — where `Unit` is the member name taken from the request and `<set>` is the comma-and-space-joined list of the member's allowed values. The only variable part is the member name; the rejected value is NOT echoed anywhere, and no count or position is reported. Two properties of AWS's rendering matter and neither is derivable from the model: (a) the brackets are PADDED (`[ ` ... ` ]`), unlike the unpadded `[...]` the sibling `ValidationError` enum template uses; (b) the member order AWS emits is a fixed server-side constant that is NOT the Smithy declaration order (AWS leads with Megabits/Terabits/Gigabits; the model leads with Seconds/Microseconds/Milliseconds), so iterating the Smithy enum can never reproduce it.

Emulator: HTTP status and error code are correct — GetMetricStatistics with Unit='Furlongs' is refused `InvalidParameterValue` (the check's code assertion passes). The message is:
            "The parameter Unit must be a value in the set [Seconds, Microseconds, Milliseconds, Bytes, Kilobytes, Megabytes, Gigabytes, Terabytes, Bits, Kilobits, Megabits, Gigabits, Terabits, Percent, Count, Bytes/Second, Kilobytes/Second, Megabytes/Second, Gigabytes/Second, Terabytes/Second, Bits/Second, Kilobits/Second, Megabits/Second, Gigabits/Second, Terabits/Second, Count/Second, None]."
          Two deltas: the brackets carry no inner padding (`[Seconds` / `None]` instead of `[ Megabits` / `None ]`), and the member list is byte-for-byte the StandardUnit declaration order from the Smithy model rather than AWS's fixed order. The surrounding fixed text and the member name are right.

Why:      A client that asserts on, logs, surfaces to a user, or pattern-matches this refusal sees a sentence CloudWatch never returns. Because the divergence is punctuation and list order only, it passes any loose substring check and then fails an exact-message contract; and a caller that parses the advertised allowed-value list out of the message to present remediation gets it in an order AWS does not use, so round-tripping the emulator's message against real CloudWatch diverges silently. It also tells the emulator's maintainers that the list can be generated from the model, which is exactly the assumption AWS's ordering disproves.

Fix:      The CloudWatch read-path input validation / refusal-message renderer for the `InvalidParameterValue` family on GetMetricStatistics (the component that formats `The parameter <member> must be a value in the set [ ... ].`), not GetMetricStatistics' datapoint lookup — the storage/read behaviour this check grades around the refusal (three units stored on one metric, one ListMetrics catalogue entry, empty results for valid-but-never-published units) all passed. The renderer needs AWS's padded-bracket form and the per-member recorded value order, rather than deriving the list from the Smithy enum definition. Closely related but NOT covering this: open issue 20261028T043126Z-cloudwatch-enum-constraint-validationerror-messages-are-rendered-from.md documents the same Smithy-declaration-order mechanism for the DIFFERENT `ValidationError` template (`1 validation error detected: ... Member must satisfy enum value set: [...]`) on GetMetricData.ScanBy and PutMetricStream.OutputFormat; this cause is a different error class, a different operation and a different sentence (including the bracket padding that template does not have), so closing that issue does not close this one.

Traces:   cloudwatch.unit-is-part-of-a-datapoints-read-identity

## Evidence
cloudwatch.unit-is-part-of-a-datapoints-read-identity (verdict: mismatch).

Failure site: check_cloudwatch_unit_is_part_of_a_datapoints_read_identity.py line 279, `assert gms_message == (...)`, reached only after the preceding assertion on the error CODE passed:
    gms_code, gms_message = _error_code(lambda: _gms("Furlongs"))
    assert gms_code == "InvalidParameterValue"   # PASSED - class is correct
    assert gms_message == ("The parameter Unit must be a value in the set [ Megabits, Terabits, "
                           "Gigabits, Count, Bytes, Gigabytes, Gigabytes/Second, Kilobytes, "
                           "Kilobits/Second, Terabytes, Terabits/Second, Bytes/Second, Percent, "
                           "Megabytes, Megabits/Second, Milliseconds, Microseconds, "
                           "Kilobytes/Second, Gigabits/Second, Megabytes/Second, Bits, "
                           "Bits/Second, Count/Second, Seconds, Kilobits, Terabytes/Second, "
                           "None ]."), gms_message

Reported AssertionError (the assert's message argument is the emulator's actual string):
    "The parameter Unit must be a value in the set [Seconds, Microseconds, Milliseconds, Bytes, Kilobytes, Megabytes, Gigabytes, Terabytes, Bits, Kilobits, Megabits, Gigabits, Terabits, Percent, Count, Bytes/Second, Kilobytes/Second, Megabytes/Second, Gigabytes/Second, Terabytes/Second, Bits/Second, Kilobits/Second, Megabits/Second, Gigabits/Second, Terabits/Second, Count/Second, None]."

Expected vs actual, isolated:
  status/code : 400 / InvalidParameterValue  ==  400 / InvalidParameterValue   (no divergence)
  fixed text  : "The parameter Unit must be a value in the set [ " ... " ]."  vs  "...set [" ... "]."  -> inner bracket padding dropped
  value list  : AWS's fixed order (Megabits, Terabits, Gigabits, Count, Bytes, ...)  vs  the emulator's (Seconds, Microseconds, Milliseconds, Bytes, ...)
  echoed value: AWS omits the rejected 'Furlongs'; the emulator also omits it   (no divergence)

Smithy cross-check: `smithy_query shape=StandardUnit` on services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/com-amazonaws-cloudwatch-graniteserviceversion20100801.json returns values in the order [Seconds, Microseconds, Milliseconds, Bytes, Kilobytes, Megabytes, Gigabytes, Terabytes, Bits, Kilobits, Megabits, Gigabits, Terabits, Percent, Count, Bytes/Second, Kilobytes/Second, Megabytes/Second, Gigabytes/Second, Terabytes/Second, Bits/Second, Kilobits/Second, Megabits/Second, Gigabits/Second, Terabits/Second, Count/Second, None] — identical, element for element, to the emulator's rendered list. That proves the renderer iterates the model rather than reproducing AWS's recorded order.

Scope control: every arm of this check before line 279 passed, including the three-unit write, the Unit-keyed GetMetricStatistics reads, the single ListMetrics catalogue entry (units are not part of a metric's catalogue identity), and the empty-but-successful reads for the valid-but-never-published units Terabytes/Percent/Count. So the divergence is confined to refusal-message rendering and not to Unit-keyed storage. The check's later arm on GetMetricData's MetricStat.Unit (expecting ValidationError with the `1 validation error detected: ...` template) was never reached and is not implicated here.

## Steps to Reproduce
1. Boot the emulator and point a botocore/AWS SDK CloudWatch client at it.
2. PutMetricData into a fresh unique namespace so the metric exists, e.g. Namespace=<unique>, MetricData=[{MetricName:'Probe', Dimensions:[{Name:'Probe',Value:'unit-identity'}], Value:100.0, Unit:'Bytes'}].
3. Call GetMetricStatistics with that Namespace/MetricName/Dimensions, StartTime a few minutes back, EndTime a few minutes ahead, Period=60, Statistics=['Sum'], and Unit set to any string outside the StandardUnit enum (e.g. 'Furlongs').
4. Catch the ClientError and read Error.Code and Error.Message.
   Observed: Code='InvalidParameterValue' (correct); Message="The parameter Unit must be a value in the set [Seconds, Microseconds, Milliseconds, Bytes, Kilobytes, Megabytes, Gigabytes, Terabytes, Bits, Kilobits, Megabits, Gigabits, Terabits, Percent, Count, Bytes/Second, Kilobytes/Second, Megabytes/Second, Gigabytes/Second, Terabytes/Second, Bits/Second, Kilobits/Second, Megabits/Second, Gigabits/Second, Terabits/Second, Count/Second, None]."
   Expected (AWS): "The parameter Unit must be a value in the set [ Megabits, Terabits, Gigabits, Count, Bytes, Gigabytes, Gigabytes/Second, Kilobytes, Kilobits/Second, Terabytes, Terabits/Second, Bytes/Second, Percent, Megabytes, Megabits/Second, Milliseconds, Microseconds, Kilobytes/Second, Gigabits/Second, Megabytes/Second, Bits, Bits/Second, Count/Second, Seconds, Kilobits, Terabytes/Second, None ]." — same fixed text and member name, but with a space inside each bracket and the allowed values in AWS's fixed order.
5. Confirm the arms that must keep passing: the same GetMetricStatistics with Unit='Bytes' returns Sum=100.0; with Unit='Terabytes', 'Percent' or 'Count' it returns an EMPTY successful Datapoints list (not an error); and ListMetrics for that namespace returns exactly one catalogue entry even though the metric holds data under three different units.

Replay shortcut: re-run the check named in trace_ids.
