Issue-Source: trace-sweep-divergence
Issue-Trace: kinesisanalyticsv2-application-negative-envelopes
Issue-Step: task#3
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesisanalyticsv2-application-negative-envelopes
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesisanalyticsv2|CreateApplication|4xx->4xx|$.message
Issue-Commit: bb4bfca60cb5e26956e3ac4a914274189a3663a9

# CreateApplication enum validation message lists the wrong RuntimeEnvironment value set

> _Note: this issue was identified against the code base at commit `bb4bfca60cb5e26956e3ac4a914274189a3663a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
Cause 1 of 1: CreateApplication rejects an unmodeled RuntimeEnvironment with the right status and shape but the wrong enumerated value set inside the validation message.

AWS: CreateApplication.RuntimeEnvironment targets com.amazonaws.kinesisanalyticsv2#RuntimeEnvironment, a closed enum, and a value outside it is rejected before the operation runs with HTTP 400 and the framework's constraint-violation envelope. The recorded response for this error class is HTTP 400 with the message template "<n> validation error detected: Value '<rejected value>' at '<lowerCamel member name>' failed to satisfy constraint: Member must satisfy enum value set: [<the enum's accepted values, comma-separated, ascending by string>]". In that template the fixed text is " validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy enum value set: [" and "]"; the variable parts are the violation count, the rejected value and the member name taken from the request, and the accepted-value list taken from the enum the member targets. The recording shows that list as [FLINK-1_11, FLINK-1_13, FLINK-1_15, FLINK-1_6, FLINK-1_8, SQL-1_0, ZEPPELIN-FLINK-1_0, ZEPPELIN-FLINK-2_0, ZEPPELIN-FLINK-3_0] — nine values, i.e. the set the service enumerated for this member — so for this fixture that list is the requirement.

Emulator: status 400 and the envelope are correct, and the fixed text, the violation count, the rejected value and the member name all match. Only the accepted-value list diverges: the emulator emits [FLINK-1_11, FLINK-1_13, FLINK-1_15, FLINK-1_18, FLINK-1_19, FLINK-1_20, FLINK-1_6, FLINK-1_8, FLINK-2_2, FLINK-2_3, SQL-1_0, ZEPPELIN-FLINK-1_0, ZEPPELIN-FLINK-2_0, ZEPPELIN-FLINK-3_0] — fourteen values. Hypothesis: the emulator renders the list from every member of the on-disk Smithy enum (which carries FLINK-1_18, FLINK-1_19, FLINK-1_20, FLINK-2_2 and FLINK-2_3 in addition), rather than from the set this operation actually accepts, so the five extra runtimes leak into the message. The ordering rule (ascending by string, so FLINK-1_11 before FLINK-1_6) is already right.

Why: a client that parses the accepted-value list out of the message to populate a choice, to retry with a supported runtime, or to assert on the error text sees runtimes that CreateApplication does not accept here. It will offer or retry a value that is then rejected again, and any test pinned to the error message fails against the emulator even though the status and error code are correct.

Fix: belongs in CreateApplication's input validation — specifically the enum-constraint violation message builder that formats the accepted-value list for RuntimeEnvironment. It must render the value set the operation accepts for this member rather than the full on-disk enum, keeping the existing count/value/member substitution and the ascending string ordering unchanged.

Traces: kinesisanalyticsv2-application-negative-envelopes#task#3 (CreateApplication, expected 400 / actual 400, $.message value_mismatch).

## Evidence
Fixture kinesisanalyticsv2-application-negative-envelopes, step task#3, CreateApplication with RuntimeEnvironment='FLINK-99_9'. Expected status 400, actual status 400 (status and error envelope agree).

$.message value_mismatch:
  expected: "1 validation error detected: Value 'FLINK-99_9' at 'runtimeEnvironment' failed to satisfy constraint: Member must satisfy enum value set: [FLINK-1_11, FLINK-1_13, FLINK-1_15, FLINK-1_6, FLINK-1_8, SQL-1_0, ZEPPELIN-FLINK-1_0, ZEPPELIN-FLINK-2_0, ZEPPELIN-FLINK-3_0]"
  actual:   "1 validation error detected: Value 'FLINK-99_9' at 'runtimeEnvironment' failed to satisfy constraint: Member must satisfy enum value set: [FLINK-1_11, FLINK-1_13, FLINK-1_15, FLINK-1_18, FLINK-1_19, FLINK-1_20, FLINK-1_6, FLINK-1_8, FLINK-2_2, FLINK-2_3, SQL-1_0, ZEPPELIN-FLINK-1_0, ZEPPELIN-FLINK-2_0, ZEPPELIN-FLINK-3_0]"

The only difference is the accepted-value list: the emulator adds FLINK-1_18, FLINK-1_19, FLINK-1_20, FLINK-2_2 and FLINK-2_3. Smithy (services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523): enum RuntimeEnvironment = SQL-1_0, FLINK-1_6, FLINK-1_8, ZEPPELIN-FLINK-1_0, FLINK-1_11, FLINK-1_13, ZEPPELIN-FLINK-2_0, FLINK-1_15, ZEPPELIN-FLINK-3_0, FLINK-1_18, FLINK-1_19, FLINK-1_20, FLINK-2_2, FLINK-2_3 — i.e. exactly the emulator's fourteen, which is why the full-enum rendering is the suspected source. Not covered by the open issues 20261029T173921Z-kinesisanalyticsv2-applicationdetail-and-applicationsummary-omit-modeled-description-members-applicationmode.md or 20261029T173921Z-kinesisanalyticsv2-deleteapplication-skips-createtimestamp-deserialization-and-updateapplication-never-advances.md, neither of which touches enum validation messages.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture kinesisanalyticsv2-application-negative-envelopes.
2. Observe step task#3: CreateApplication is called with a RuntimeEnvironment value that is not a member of the enum (the fixture uses 'FLINK-99_9'), together with the otherwise valid ApplicationName / ServiceExecutionRole inputs.
3. The response is HTTP 400 with the constraint-violation envelope, as expected.
4. Compare $.message against the recorded message: the fixed text, violation count, rejected value and member name agree, but the bracketed accepted-value list contains five runtimes (FLINK-1_18, FLINK-1_19, FLINK-1_20, FLINK-2_2, FLINK-2_3) that the recorded response does not list.
5. Any enum-constrained member of this service reproduces the same shape of defect: send an off-enum value and compare the bracketed list in $.message with the set the operation actually accepts.
