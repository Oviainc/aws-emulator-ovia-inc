Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-experiment-list-operations
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-experiment-list-operations
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|ListExperimentDefinitions|2xx->2xx|$.Items[Name="sim2real-probe-appconfig-experiment-5538b92e"].CreatedAt
Issue-Key: appconfig|ListExperimentDefinitions|2xx->2xx|$.Items[Name="sim2real-probe-appconfig-experiment-5538b92e"].UpdatedAt
Issue-Commit: 2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf

# ListExperimentDefinitions emits item CreatedAt and UpdatedAt as epoch-float numbers instead of ISO-8601 date-time strings

> _Note: this issue was identified against the code base at commit `2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListExperimentDefinitions

## Description
1 cause.

Cause 1 of 1: ListExperimentDefinitions item timestamp encoding - fixture appconfig-experiment-list-operations, step task#6. The emulator answers 200 with a well-formed page, but every timestamp member inside an item of $.Items is written on the wire as a bare JSON floating-point epoch-seconds number instead of an ISO-8601 date-time string.

AWS: Smithy (services/service-com-amazonaws-appconfig-amazonappconfig): ListExperimentDefinitions GET /experimentdefinitions, output ExperimentDefinitions with member Items -> ExperimentDefinitionList of ExperimentDefinitionSummary. ExperimentDefinitionSummary.CreatedAt and ExperimentDefinitionSummary.UpdatedAt both target com.amazonaws.appconfig#Iso8601DateTime with @timestampFormat("date-time"), documented as "The date and time the experiment definition was created, in ISO 8601 format" and "The date and time the experiment definition was last updated, in ISO 8601 format". So each member is serialized as an RFC-3339/ISO-8601 date-time STRING terminated by 'Z' (recorded responses for the same shape show CreatedAt at millisecond precision and UpdatedAt truncated to whole seconds), never as a JSON number. The instants themselves are per-run clock values and are not part of the requirement; the wire TYPE is.

Emulator: both members of each list item are emitted as JSON numbers (epoch seconds with a fractional part). The fixture does not assert the instants at all - it asserts only that the value is timestamp-shaped ('<shaped-not-asserted>') - and the step still fails, which isolates the divergence to the encoding rather than to a clock difference.

Why: an SDK or client decoding this page against the model's date-time format either errors on the whole ListExperimentDefinitions response or produces a nonsense instant, so a client cannot enumerate, sort, filter or display experiment definitions by creation/update time. A list page is the normal discovery path for experiment definitions, so this breaks clients that never call GetExperimentDefinition.

Fix: the ListExperimentDefinitions response serializer in the appconfig service crate - the code that renders ExperimentDefinitionSummary items must format Iso8601DateTime members as date-time strings, the same way the single-resource (GetExperimentDefinition) path is expected to.

Related open issue: 20261029T190217Z-appconfig-list-operations-epoch-float-timestamps-dropped-zero.md describes the same family of epoch-float list timestamps and even names this fixture, but the replay reports this cluster as unowned (its recorded Issue-Key pins a per-run generated experiment-definition name, so the key does not match this run), so it is filed here rather than assumed covered.

Traces: appconfig-experiment-list-operations#task#6 (ListExperimentDefinitions, 200 expected / 200 actual).

## Evidence
Fixture appconfig-experiment-list-operations, step task#6, operation ListExperimentDefinitions, expected status 200, actual status 200.

  $.Items[Name=<generated experiment-definition name>].CreatedAt: value_mismatch - expected '<shaped-not-asserted>' (any ISO-8601 date-time STRING, e.g. '<date>T<time>.<millis>Z'), got 1793301986.3804874 (a JSON number)
  $.Items[Name=<generated experiment-definition name>].UpdatedAt: value_mismatch - expected '<shaped-not-asserted>' (any ISO-8601 date-time STRING, e.g. '<date>T<time>Z'), got 1793301986.3804874 (a JSON number)

The experiment-definition name and the numeric instant above are per-run values; the asserted requirement that fails is the wire type (string vs number).

Smithy support: ExperimentDefinitionSummary.CreatedAt and .UpdatedAt -> Iso8601DateTime, @timestampFormat("date-time"), docs "... in ISO 8601 format"; ListExperimentDefinitions is @paginated (inputToken NextToken / outputToken NextToken / items Items / pageSize MaxResults) and returns http code 200.

## Steps to Reproduce
1. Replay wire fixture appconfig-experiment-list-operations and inspect step task#6.
2. Equivalently: create an AppConfig application, environment and configuration profile, create one experiment definition under them, then call ListExperimentDefinitions with application_identifier set.
3. Inspect the raw JSON body of the 200 response: AWS renders each $.Items[*].CreatedAt and $.Items[*].UpdatedAt as a quoted ISO-8601 date-time string ending in 'Z'; the emulator renders them as unquoted epoch-seconds floats.
4. Re-replay appconfig-experiment-list-operations#task#6 to confirm the fix.
