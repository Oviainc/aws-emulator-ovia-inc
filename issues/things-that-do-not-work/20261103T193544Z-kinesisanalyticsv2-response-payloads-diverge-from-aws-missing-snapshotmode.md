Issue-Source: trace-sweep-divergence
Issue-Trace: kinesisanalyticsv2-application-lifecycle
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesisanalyticsv2-application-lifecycle kinesisanalyticsv2-application-version-history kinesisanalyticsv2-kfs-catalog-database-arn-mistakes kinesisanalyticsv2-kfs-execution-role-mistakes
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesisanalyticsv2|DescribeApplicationVersion|2xx->2xx|$.ApplicationVersionDetail.ApplicationConfigurationDescription.ApplicationSnapshotConfigurationDescription.SnapshotMode
Issue-Key: kinesisanalyticsv2|CreateApplication|4xx->4xx|$.message
Issue-Commit: 2b5605b2de23550e6c1f97402ef3aec1933f6882

# kinesisanalyticsv2 response payloads diverge from AWS: missing SnapshotMode and model-trait regex echoed in validation message

> _Note: this issue was identified against the code base at commit `2b5605b2de23550e6c1f97402ef3aec1933f6882`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeApplicationVersion, CreateApplication

## Description
Two net-new wire divergences on kinesisanalyticsv2, both in what the emulator PUTS ON THE WIRE for an otherwise correctly-routed and correctly-statused response. This issue bundles 2 causes; BOTH must be closed before submitting.

Cause 1 of 2 — ApplicationSnapshotConfigurationDescription omits SnapshotMode on the version-describe/describe path (fixture kinesisanalyticsv2-application-lifecycle, step task#5).
AWS: recorded behavior only. Smithy (services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523): DescribeApplicationVersion -> output member ApplicationVersionDetail targets ApplicationDetail, and shape ApplicationSnapshotConfigurationDescription models only the required boolean SnapshotsEnabled — it has NO SnapshotMode member in this model revision. The recordings show the real service nonetheless serializing a SnapshotMode string alongside SnapshotsEnabled inside ApplicationConfigurationDescription.ApplicationSnapshotConfigurationDescription, whose value for an application created without an explicit snapshot mode is the legacy mode marker ('APPLICATION_SNAPSHOTS_LEGACY' in the recording). The requirement is that the member is PRESENT on every describe-shaped response that carries an ApplicationSnapshotConfigurationDescription, carrying the application's effective snapshot mode taken from emulator state (legacy for applications that never had a mode selected); the particular string in any one recording is a state value, not the template.
Emulator: answers 200 with the rest of ApplicationVersionDetail matching, but the key is absent — replay reports '$.ApplicationVersionDetail.ApplicationConfigurationDescription.ApplicationSnapshotConfigurationDescription.SnapshotMode: missing (expected 'APPLICATION_SNAPSHOTS_LEGACY', got None)'. The same gap recurs on the version-history fixture's steps task#5, task#6 and task#10 (same cluster).
Why: a client that reads SnapshotMode to decide whether an application is on legacy or on explicit snapshot semantics gets None/absent and either crashes on the missing key or silently treats every application as having no snapshot mode, so snapshot-mode-dependent branches (restore-from-latest vs. legacy behavior) never run against the emulator.
Fix: the ApplicationDetail/ApplicationSnapshotConfigurationDescription serializer for the kinesisanalyticsv2 describe family (DescribeApplicationVersion and every other op returning ApplicationDetail), plus the application state that must retain the effective snapshot mode so the serializer has something to emit. Note the open issue 'Implement kinesisanalyticsv2 DescribeApplicationVersion: unimplemented scaffold stub' — the operation now answers 200 and is no longer a stub, and that issue does not cover this response-field gap; it is not a duplicate.

Cause 2 of 2 — CreateApplication's validation-error message echoes the Smithy pattern trait verbatim instead of AWS's rendered pattern for the member (fixture kinesisanalyticsv2-kfs-catalog-database-arn-mistakes, step task#5).
AWS: Smithy: GlueDataCatalogConfiguration.DatabaseARN targets DatabaseARN, @length min 1 max 2048, @pattern "^arn:"; CreateApplication models InvalidArgumentException/InvalidRequestException (httpError 400). Per the recording, when applicationConfiguration.zeppelinApplicationConfiguration.catalogConfiguration.glueDataCatalogConfiguration.databaseARN does not look like an ARN, AWS answers HTTP 400 with the standard single-error validation body whose message is the fixed text '1 validation error detected: Value ' + the rejected value in single quotes + ' at ' + the lowerCamel member path in single quotes + ' failed to satisfy constraint: Member must satisfy regular expression pattern: ' + the pattern AS AWS RENDERS IT, which for this member is 'arn:.*'. Fixed text: everything outside the quotes plus the trailing 'Member must satisfy regular expression pattern: '. Variable parts: the rejected value and the member path, both taken from the request. The pattern tail is a per-member constant of the service, NOT the request — and it is not the model's @pattern string.
Emulator: same 400 and same structure, but renders the pattern tail as '^arn:' (the raw Smithy trait), i.e. '... regular expression pattern: ^arn:' where AWS says '... regular expression pattern: arn:.*'. Same cause on kinesisanalyticsv2-kfs-execution-role-mistakes step task#4 (same cluster: ServiceExecutionRole/RoleARN, also @pattern '^arn:').
Why: a client or test that matches on the validation message — the documented way to tell which constraint rejected an input, since all these share one error code — never matches AWS's text, and anything that surfaces the message to a user shows an anchored regex AWS never emits. The message is a requirement of the error class, so this is a defect even though status and code are right.
Fix: the kinesisanalyticsv2 CreateApplication/UpdateApplication input validation layer that formats constraint-violation messages: the per-member pattern rendering must be the service's own pattern text rather than the model trait source, and must stay request-driven only for the value and member path. Related open issue 'kinesisanalyticsv2 refusal responses diverge: wrong error class, wrong HTTP status, emulator-authored messages, and unenforced validations that fail open' owns the applicationName pattern echo, the enum-value-set ordering and the wrong-error-class clusters; it does NOT carry this cluster's replay key, so this cause must be fixed and verified here.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200; 155 steps, 115 passed, score 74.2%). The 2 clusters below are the only ones with empty stub_path and empty owned_by.

Cause 1 — cluster service=kinesisanalyticsv2|DescribeApplicationVersion|status=200->200:
  representative: kinesisanalyticsv2-application-lifecycle#task#5
  expected status 200, actual status 200
  detail: $.ApplicationVersionDetail.ApplicationConfigurationDescription.ApplicationSnapshotConfigurationDescription.SnapshotMode: missing (expected 'APPLICATION_SNAPSHOTS_LEGACY', got None)
  also affected (4): kinesisanalyticsv2-application-lifecycle#task#5, kinesisanalyticsv2-application-version-history#task#5, #task#6, #task#10
  expected: the key is present with the application's effective snapshot mode; actual: the key is absent from the response object.

Cause 2 — cluster service=kinesisanalyticsv2|CreateApplication|status=400->400|keys=$.message:
  representative: kinesisanalyticsv2-kfs-catalog-database-arn-mistakes#task#5
  expected status 400, actual status 400
  detail: $.message: value_mismatch
    expected: "1 validation error detected: Value '<databaseARN value>' at 'applicationConfiguration.zeppelinApplicationConfiguration.catalogConfiguration.glueDataCatalogConfiguration.databaseARN' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:.*"
    actual:   "1 validation error detected: Value '<databaseARN value>' at 'applicationConfiguration.zeppelinApplicationConfiguration.catalogConfiguration.glueDataCatalogConfiguration.databaseARN' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:"
  also affected (2): kinesisanalyticsv2-kfs-catalog-database-arn-mistakes#task#5, kinesisanalyticsv2-kfs-execution-role-mistakes#task#4
  Only the pattern tail differs: 'arn:.*' (AWS) vs '^arn:' (emulator, the Smithy @pattern trait source).

Model evidence: smithy_query shape ApplicationSnapshotConfigurationDescription -> members {SnapshotsEnabled: BooleanObject, required} (no SnapshotMode); shape GlueDataCatalogConfiguration -> DatabaseARN required, length 1..2048, pattern '^arn:'; operation CreateApplication errors include InvalidArgumentException/InvalidRequestException at httpError 400.

## Steps to Reproduce
Cause 1: replay fixture kinesisanalyticsv2-application-lifecycle up to step task#5 (create a Flink application, then DescribeApplicationVersion for version 1) and inspect $.ApplicationVersionDetail.ApplicationConfigurationDescription.ApplicationSnapshotConfigurationDescription — SnapshotMode is absent where the recording has it. Same with kinesisanalyticsv2-application-version-history steps task#5, task#6 and task#10.

Cause 2: replay fixture kinesisanalyticsv2-kfs-catalog-database-arn-mistakes up to step task#5 — CreateApplication for a ZEPPELIN-FLINK Studio notebook whose applicationConfiguration.zeppelinApplicationConfiguration.catalogConfiguration.glueDataCatalogConfiguration.databaseARN is a bare database name rather than an ARN. The response is 400 as expected; compare $.message's trailing pattern text against the recording ('arn:.*' expected, '^arn:' served). Repeat with kinesisanalyticsv2-kfs-execution-role-mistakes step task#4 (non-ARN ServiceExecutionRole).
