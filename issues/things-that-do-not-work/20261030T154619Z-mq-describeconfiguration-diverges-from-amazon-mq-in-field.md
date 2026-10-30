Issue-Source: trace-sweep-divergence
Issue-Trace: mq-configuration-lifecycle-and-revisions
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mq-configuration-lifecycle-and-revisions mq-error-paths
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq|DescribeConfiguration|2xx->2xx|$.authenticationStrategy
Issue-Key: mq|DescribeConfiguration|2xx->2xx|$.created
Issue-Key: mq|DescribeConfiguration|2xx->2xx|$.engineType
Issue-Key: mq|DescribeConfiguration|2xx->2xx|$.engineVersion
Issue-Key: mq|DescribeConfiguration|2xx->2xx|$.latestRevision.created
Issue-Key: mq|DescribeConfiguration|2xx->2xx|$.latestRevision.description
Issue-Key: mq|DescribeConfiguration|2xx->2xx|$.latestRevision.isInvalid
Issue-Key: mq|DescribeConfiguration|2xx->2xx|$.tags
Issue-Key: mq|DescribeConfiguration|2xx->2xx|$.latestRevision.revision
Issue-Key: mq|DescribeConfiguration|4xx->2xx|$.errorAttribute
Issue-Key: mq|DescribeConfiguration|4xx->2xx|$.message
Issue-Key: mq|DescribeConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mq|DescribeConfiguration|4xx->2xx|status
Issue-Key: mq|DescribeConfiguration|4xx->4xx|$.errorAttribute
Issue-Key: mq|DescribeConfiguration|4xx->4xx|$.message
Issue-Commit: 0da124688ced2f47ce1b8859b8dabf10bd34dedc

# mq DescribeConfiguration diverges from Amazon MQ in field representation, revision tracking and not-found handling

> _Note: this issue was identified against the code base at commit `0da124688ced2f47ce1b8859b8dabf10bd34dedc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConfiguration

## Description
Four distinct causes, all on the mq configuration surface. A fixer must close ALL FOUR and re-replay every fixture#step listed under each before submitting.

Cause 1 of 4: DescribeConfiguration serialises the configuration record in the wrong representation (fixture mq-configuration-lifecycle-and-revisions, step task#2).
AWS: Smithy (services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json) models DescribeConfigurationResponse$Created and ConfigurationRevision$Created as __timestampIso8601 with timestampFormat date-time, i.e. an ISO-8601 date-time STRING on the wire; ConfigurationRevision$Revision and $Created are required. Tags is a __mapOf__string. For engineType/authenticationStrategy the model enumerates EngineType = [ACTIVEMQ, RABBITMQ] and AuthenticationStrategy = [SIMPLE, LDAP, CONFIG_MANAGED], but the RECORDED AWS response for a configuration uses the legacy wire spellings (recorded behavior only): engineType = "ActiveMQ" and authenticationStrategy = "simple". The recording also carries latestRevision.isInvalid = false, a member this model does not declare at all (recorded behavior only), and tags = {} for an untagged configuration — an empty map, not an omitted member.
Emulator: returns engineType "ACTIVEMQ", authenticationStrategy "SIMPLE", created and latestRevision.created as a bare epoch-seconds NUMBER (a float) instead of an ISO-8601 string, omits latestRevision.isInvalid entirely, and omits tags instead of serialising an empty map. It also reports engineVersion "5.18.4" where AWS reports "5.19", and because the default revision description is built from those two values it reports "Auto-generated default for <name> on ACTIVEMQ 5.18.4" where AWS words it "Auto-generated default for <name> on ActiveMQ 5.19" — fixed text "Auto-generated default for ", then the configuration name from the request, then " on ", the engineType and the engineVersion, all from the stored record. Hypothesis: the version skew comes from the crate's hand-maintained published-version list (state.rs, ACTIVEMQ_ENGINE_VERSIONS / published_engine_versions / latest_engine_version), whose newest ActiveMQ entry is 5.18.4, so a create that does not pin EngineVersion defaults to a version AWS no longer defaults to.
Why: a client that parses created with an ISO-8601/date-time parser fails outright on a JSON number; a client that switches on engineType == "ActiveMQ" or authenticationStrategy == "simple" (the values AWS actually sends) takes the wrong branch; code that iterates response.tags without a null check faults on an untagged configuration; and anything that pins an engine version sees a version catalogue that is years stale.
Fix: DescribeConfiguration's handler and its output serialisation — services/service-com-amazonaws-mq-mq/src/ops/describe_configuration.rs (the DescribeConfigurationOutput it builds, lines 190-205, including tags: configuration.optional_tags()), plus the shared record helpers in services/service-com-amazonaws-mq-mq/src/state.rs (ConfigurationRecord, optional_tags, default_configuration_description, published_engine_versions) and whatever honours timestampFormat=date-time for this output schema.
Traces: mq-configuration-lifecycle-and-revisions#task#2.

Cause 2 of 4: a configuration's latestRevision never advances, so DescribeConfiguration keeps reporting revision 1 and the auto-generated description after a revision has been made (fixture mq-configuration-lifecycle-and-revisions, step task#6).
AWS: UpdateConfiguration (Smithy: UpdateConfiguration, PUT /v1/configurations/{ConfigurationId}, output LatestRevision: ConfigurationRevision) makes the NEXT revision of the configuration and carries the request's Description onto it; a later DescribeConfiguration therefore reports latestRevision.revision = 2 with description = the description supplied on the update ("second revision" in this recording), isInvalid = false, and an ISO-8601 created.
Emulator: after the update, DescribeConfiguration still reports latestRevision.revision = 1 with the create-time auto-generated description, i.e. the revision chain never grows. The state record already models a revision list (state.rs ConfigurationRecord.revisions, ConfigurationRevisionRecord, FIRST_CONFIGURATION_REVISION) but nothing appends to it, because UpdateConfiguration is still the unimplemented scaffold (services/service-com-amazonaws-mq-mq/src/ops/update_configuration.rs returns Scaffolded::not_implemented). The same step also shows the Cause 1 representation defects.
Why: a client that updates a configuration and then reads it back cannot tell its change landed; revision-pinned workflows (ListConfigurationRevisions / DescribeConfigurationRevision, and a broker that references a configuration revision) all key off a revision number that never changes, so they silently operate on the wrong revision.
Fix: UpdateConfiguration's handler must append a ConfigurationRevisionRecord (revision = previous + 1, created = now, description = the request's Description) to ConfigurationRecord.revisions, and DescribeConfiguration must report the last one — services/service-com-amazonaws-mq-mq/src/ops/update_configuration.rs plus state.rs ConfigurationRecord::latest_revision. (An unimplemented-operation issue for UpdateConfiguration itself is filed separately; this cause is the state/representation consequence a DescribeConfiguration replay observes.)
Traces: mq-configuration-lifecycle-and-revisions#task#6.

Cause 3 of 4: a configuration that has been deleted is still served with HTTP 200 instead of the modelled NotFoundException (fixture mq-configuration-lifecycle-and-revisions, step teardown#8; also mq-error-paths#teardown#10).
AWS: once DeleteConfiguration has removed a configuration, DescribeConfiguration on that id answers NotFoundException (Smithy: DescribeConfiguration errors include NotFoundException, httpError 404, members message/errorAttribute, plus the x-amzn-errortype header the recording shows). The recorded message is "Configuration ID [<configuration-id>] can't be found. Make sure it exists." — fixed text "Configuration ID [", then the ConfigurationId taken from the request path, then "] can't be found. Make sure it exists." — with errorAttribute = "configuration-id".
Emulator: answers HTTP 200 with the full configuration body, and no message, no errorAttribute and no x-amzn-errortype header; the record is never removed from the store because DeleteConfiguration is the unimplemented scaffold (services/service-com-amazonaws-mq-mq/src/ops/delete_configuration.rs returns Scaffolded::not_implemented), so nothing ever deletes from state.rs ComAmazonawsMqMqState.configurations.
Why: deletion is unobservable — a client cannot verify teardown, a create-after-delete of the same configuration name still collides with the stale record, and cleanup loops that poll for 404 never terminate.
Fix: DeleteConfiguration must remove the record from the configurations map (services/service-com-amazonaws-mq-mq/src/ops/delete_configuration.rs, state.rs ComAmazonawsMqMqState.configurations), after which DescribeConfiguration's existing not-found path (describe_configuration.rs lines 176-185) applies.
Traces: mq-configuration-lifecycle-and-revisions#teardown#8, mq-error-paths#teardown#10.

Cause 4 of 4: the NotFoundException DescribeConfiguration does raise carries a generic message and no errorAttribute (fixture mq-error-paths, step task#3).
AWS: for a ConfigurationId that names nothing, the 404 body is {"message": "Configuration ID [<configuration-id>] can't be found. Make sure it exists.", "errorAttribute": "configuration-id"} — the fixed sentence above with the requested id interpolated from the request, and the member name of the offending input as errorAttribute (Smithy: NotFoundException$ErrorAttribute, "The attribute which caused the error").
Emulator: returns the right status (404) but the body is {"message": "Resource was not found"} with errorAttribute absent — a single constant sentence that interpolates nothing (services/service-com-amazonaws-mq-mq/src/state.rs RESOURCE_NOT_FOUND_MESSAGE, used at describe_configuration.rs line 180 with error_attribute: None).
Why: a client that matches on the message to distinguish a missing configuration from a missing broker or a missing user gets one indistinguishable sentence, and tooling that reports which input was wrong has no errorAttribute to show; the id that failed to resolve is not echoed anywhere.
Fix: DescribeConfiguration's not-found path — build the per-resource sentence from the requested ConfigurationId and set error_attribute = "configuration-id" (services/service-com-amazonaws-mq-mq/src/ops/describe_configuration.rs lines 176-185; the shared constant in state.rs is too coarse for this error class).
Traces: mq-error-paths#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 42.3%, 15 mismatched of 26 steps).

Cause 1 — mq-configuration-lifecycle-and-revisions#task#2, DescribeConfiguration, expected 200 / actual 200:
  $.engineType: expected 'ActiveMQ', got 'ACTIVEMQ'
  $.authenticationStrategy: expected 'simple', got 'SIMPLE'
  $.engineVersion: expected '5.19', got '5.18.4'
  $.created: expected an ISO-8601 date-time string (shape asserted), got 1793374981.5137277
  $.latestRevision.created: expected an ISO-8601 date-time string, got 1793374981.5137277
  $.latestRevision.isInvalid: expected False, got None (member absent)
  $.latestRevision.description: expected 'Auto-generated default for <name> on ActiveMQ 5.19', got 'Auto-generated default for <name> on ACTIVEMQ 5.18.4'
  $.tags: expected {}, got None (member absent)

Cause 2 — mq-configuration-lifecycle-and-revisions#task#6, DescribeConfiguration, expected 200 / actual 200:
  $.latestRevision.revision: expected 2, got 1
  $.latestRevision.description: expected 'second revision', got the create-time auto-generated description
  (plus the same engineType/engineVersion/created/isInvalid/authenticationStrategy/tags divergences as Cause 1)

Cause 3 — mq-configuration-lifecycle-and-revisions#teardown#8 (cluster also covers mq-error-paths#teardown#10), DescribeConfiguration:
  status: expected 404, got 200
  $.message: expected "Configuration ID [<configuration-id>] can't be found. Make sure it exists.", got None
  $.errorAttribute: expected 'configuration-id', got None
  $headers.x-amzn-errortype: expected present, got None

Cause 4 — mq-error-paths#task#3, DescribeConfiguration, expected 404 / actual 404:
  $.message: expected "Configuration ID [<configuration-id>] can't be found. Make sure it exists.", got 'Resource was not found'
  $.errorAttribute: expected 'configuration-id', got None

Source grounding: services/service-com-amazonaws-mq-mq/src/ops/describe_configuration.rs:176-205, services/service-com-amazonaws-mq-mq/src/ops/update_configuration.rs:159-169 (not_implemented), services/service-com-amazonaws-mq-mq/src/ops/delete_configuration.rs:149-159 (not_implemented), services/service-com-amazonaws-mq-mq/src/state.rs (ConfigurationRecord, latest_revision, optional_tags, default_configuration_description, RESOURCE_NOT_FOUND_MESSAGE at line 695, published_engine_versions/latest_engine_version).

Related open issue (different operation, not a duplicate): 20261030T033328Z-mq-describebrokerinstanceoptions-serves-a-hand-built-instance-option.md reports the same stale hand-built engine/instance catalogue on DescribeBrokerInstanceOptions.

## Steps to Reproduce
1. git clone the project repo and build/boot the emulator from the checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: fixture mq-configuration-lifecycle-and-revisions, step task#2 — CreateConfiguration for an ActiveMQ configuration, then GET /v1/configurations/{ConfigurationId}; compare engineType, authenticationStrategy, engineVersion, created, latestRevision.created, latestRevision.isInvalid, latestRevision.description and tags against the recording.
4. Cause 2: same fixture, step task#6 — after PUT /v1/configurations/{ConfigurationId} with description 'second revision', GET the configuration again and read latestRevision.revision/description.
5. Cause 3: same fixture, step teardown#8 (and mq-error-paths#teardown#10) — DELETE the configuration, then GET /v1/configurations/{ConfigurationId}: expect 404 NotFoundException, observe 200 with the full body.
6. Cause 4: fixture mq-error-paths, step task#3 — GET /v1/configurations/c-doesnotexist000000: expect 404 with message "Configuration ID [c-doesnotexist000000] can't be found. Make sure it exists." and errorAttribute 'configuration-id', observe 404 with message 'Resource was not found' and no errorAttribute.
