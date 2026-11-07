Issue-Source: trace-sweep-divergence
Issue-Trace: iot-event-configuration-and-endpoint-settings
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-event-configuration-and-endpoint-settings iot-topic-rule-create-get-delete iot-topic-rule-disabled-flag iot-topic-rule-duplicate-create-error iot-topic-rule-lifecycle iot-topic-rule-list-and-describe-roundtrip iot-topic-rule-multi-action-with-error-action iot-topic-rule-recreate-conflict-error iot-package-version-lifecycle iot-thing-type-list-lifecycle iot-topic-rule-invalid-sql-error
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|GetIndexingConfiguration|2xx->2xx|$.thingIndexingConfiguration.filter
Issue-Key: iot|GetIndexingConfiguration|2xx->2xx|$.thingGroupIndexingConfiguration.managedFields
Issue-Key: iot|GetIndexingConfiguration|2xx->2xx|$.thingIndexingConfiguration.managedFields
Issue-Key: iot|UpdateIndexingConfiguration|4xx->2xx|$
Issue-Key: iot|UpdateIndexingConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|UpdateIndexingConfiguration|4xx->2xx|status
Issue-Key: iot|GetTopicRule|2xx->2xx|$.rule.properties
Issue-Key: iot|CreatePackageVersion|2xx->2xx|$.attributes
Issue-Key: iot|DescribeThingType|2xx->2xx|$.thingTypeProperties.searchableAttributes[*]
Issue-Key: iot|CreateTopicRule|4xx->2xx|$
Issue-Key: iot|CreateTopicRule|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|CreateTopicRule|4xx->2xx|status
Issue-Commit: ff097a89ab5b098f807ae300b709617042ce117b

# iot read projections: 7 causes — indexing filter and managedFields absent, topic-rule SQL and filter configs unchecked

> _Note: this issue was identified against the code base at commit `ff097a89ab5b098f807ae300b709617042ce117b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIndexingConfiguration

## Description
Several IoT read paths project a different shape from AWS — structures AWS always emits are omitted, a map AWS omits is echoed, and a list AWS returns in request order is reordered — and two write paths accept configuration AWS refuses (an invalid named-shadow filter, and unparseable rule SQL). 7 net-new causes, all in service iot.

Cause 1 of 7: GetIndexingConfiguration omits thingIndexingConfiguration.filter.
AWS: Smithy: ThingIndexingConfiguration has a filter member targeting IndexingFilter (named shadow / geolocation selections). Recorded behavior: AWS always emits filter as a present structure with its members null when nothing is selected — {connectivity: null, geoLocations: null, namedShadowNames: null}.
Emulator: omits the filter member entirely.
Why: a client that reads the current indexing configuration, edits it and writes it back (the documented read-modify-write pattern for UpdateIndexingConfiguration) finds no filter key and cannot distinguish 'no selections' from 'this emulator does not model filters'; code that dereferences the structure fails.
Fix: the GetIndexingConfiguration serializer must emit the filter structure with null members rather than omitting it.
Traces: iot-event-configuration-and-endpoint-settings#task#8 (also iot-event-configuration-and-endpoint-settings#teardown#14).

Cause 2 of 7: GetIndexingConfiguration also omits managedFields for both thing and thing-group indexing.
AWS: Smithy: ThingIndexingConfiguration and ThingGroupIndexingConfiguration each carry a managedFields member (Fields list) — fields the Fleet Indexing service knows the types of, which AWS populates itself. Recorded behavior: thingIndexingConfiguration.managedFields contains the service-managed registry fields (registry.version Number, thingName String, thingId String, registry.thingTypeName String, registry.thingGroupNames String) and thingGroupIndexingConfiguration.managedFields contains the group ones (version Number, thingGroupId String, parentGroupNames String, description String, thingGroupName String); filter is present with its selections ({connectivity: {includeSocketInformation: []}, geoLocations: [], namedShadowNames: []}).
Emulator: all three members absent.
Why: managedFields is how a client discovers which fields it does NOT need to declare as customFields; absent, a caller either re-declares managed fields (which AWS refuses) or cannot build a valid update at all.
Fix: the indexing-configuration state must carry the service-managed field sets and the GetIndexingConfiguration serializer must emit them.
Traces: iot-event-configuration-and-endpoint-settings#task#10.

Cause 3 of 7: UpdateIndexingConfiguration accepts a namedShadowNames filter while named-shadow indexing is off.
AWS: Smithy: UpdateIndexingConfiguration declares InvalidRequestException (httpError 400); ThingIndexingConfiguration.namedShadowIndexingMode is the enum gating named-shadow data and filter.namedShadowNames the selection. Recorded behavior: 400 with x-amzn-errortype 'InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/' and the fixed message 'NamedShadowIndexingMode must be turned ON for providing the Filter for NamedShadowNames' (a constant, no variable part).
Emulator: 200 with an empty body — the inconsistent configuration is stored.
Why: AWS enforces this cross-member precondition at write time; accepting it leaves the account with an indexing configuration that cannot exist in AWS, and a caller gets no signal that its named-shadow selection will never take effect.
Fix: the UpdateIndexingConfiguration handler's validation — reject a namedShadowNames filter unless namedShadowIndexingMode is ON.
Traces: iot-event-configuration-and-endpoint-settings#task#11.

Cause 4 of 7: GetTopicRule omits rule.properties.
AWS: recorded behavior only — the Smithy TopicRule shape in this model declares ruleName, sql, description, createdAt, actions, ruleDisabled, awsIotSqlVersion and errorAction and does NOT declare a properties member, but the recorded AWS response carries rule.properties as an empty list. The requirement is therefore the recording: the member is present and is an empty list when the rule declares no properties.
Emulator: omits rule.properties.
Why: a client deserialising with a newer model than this checkout's expects the member; omitting it is a wire-shape difference on the most-read operation of the topic-rule plane (it affects seven recorded steps).
Fix: the GetTopicRule serializer / the TopicRule projection.
Traces: iot-topic-rule-create-get-delete#task#2 (also iot-topic-rule-disabled-flag#task#2, iot-topic-rule-duplicate-create-error#task#3, iot-topic-rule-lifecycle#task#2, iot-topic-rule-list-and-describe-roundtrip#task#2, iot-topic-rule-multi-action-with-error-action#task#2, iot-topic-rule-recreate-conflict-error#task#3).

Cause 5 of 7: CreatePackageVersion echoes the attributes map AWS withholds.
AWS: Smithy: CreatePackageVersion's attributes member (ResourceAttributes map) is marked @sensitive on both the input and the output. Recorded behavior: the CREATE response omits attributes entirely even when the request supplied them.
Emulator: echoes the submitted attributes map back in the create response.
Why: the member is sensitive — AWS deliberately does not reflect it on the create path, so the emulator writes caller-supplied sensitive values into a response (and any client log of it) that AWS keeps empty.
Fix: the CreatePackageVersion serializer must not populate attributes.
Traces: iot-package-version-lifecycle#task#5.

Cause 6 of 7: DescribeThingType reorders searchableAttributes.
AWS: Smithy: ThingTypeProperties.searchableAttributes targets SearchableAttributes (a list, not a set — no @uniqueItems reordering licence). Recorded behavior: the list is returned in the order it was submitted at create time.
Emulator: returns the two attributes in the opposite order.
Why: list order is part of the wire contract for a non-set list; a client comparing its submitted configuration against the read-back (drift detection, CloudFormation-style diffing) sees a spurious change on every read.
Fix: the thing-type state must preserve the submitted searchableAttributes order (it is presumably stored in an unordered collection) and the DescribeThingType serializer must emit it unchanged.
Traces: iot-thing-type-list-lifecycle#task#2.

Cause 7 of 7: CreateTopicRule accepts unparseable rule SQL.
AWS: Smithy: CreateTopicRule declares SqlParseException (httpError 400, member message) among its errors — 'The Rule-SQL expression can't be parsed correctly.' Recorded behavior: 400 with x-amzn-errortype 'SqlParseException:http://iot.amazonaws.com/doc/2015-10-02/' and a message whose fixed part is the expectation and position report — "Expected 'SELECT'", a blank line, the offending statement echoed, and a caret line giving the 1-based line:column of the failure ('^ at 1:1' for a failure at the first token). Fixed text: "Expected 'SELECT'" and the ' at <line>:<column>' suffix; variable parts: the submitted SQL and the position at which parsing failed.
Emulator: 200 with an empty body and no errortype header — the rule is created with SQL that can never evaluate.
Why: SQL is the whole substance of a topic rule; accepting a statement AWS refuses means the rule is created, matches nothing, and the caller is never told why its messages are not routed. SqlParseException is modelled precisely so a client can surface the parse position to the author.
Fix: the CreateTopicRule handler's SQL validation — parse topicRulePayload.sql and refuse with SqlParseException(400) before persisting.
Traces: iot-topic-rule-invalid-sql-error#task#1.

Related open issues that do NOT cover these: '20261106T201842Z-iot-refusal-fidelity-10-net-new-causes-createpolicy.md', '20261106T201842Z-iot-replacetopicrule-stub.md' and '20261105T035750Z-iot-createpackageversion-duplicate-version-conflictexception-message-is-emulator-authored.md' carry no replay key for any of the seven fixture steps above.

## Evidence
Replay of trace set simparity-wire-aws-v1. Expected = recorded AWS response, Actual = emulator.

1. GetIndexingConfiguration, iot-event-configuration-and-endpoint-settings#task#8 — status 200 both; $.thingIndexingConfiguration.filter expected {'connectivity': None, 'geoLocations': None, 'namedShadowNames': None}, got absent. Cluster of 2.
2. GetIndexingConfiguration, iot-event-configuration-and-endpoint-settings#task#10 — $.thingGroupIndexingConfiguration.managedFields expected [{version,Number},{thingGroupId,String},{parentGroupNames,String},{description,String},{thingGroupName,String}], got absent; $.thingIndexingConfiguration.managedFields expected [{registry.version,Number},{thingName,String},{thingId,String},{registry.thingTypeName,String},{registry.thingGroupNames,String}], got absent; $.thingIndexingConfiguration.filter expected {'connectivity': {'includeSocketInformation': []}, 'geoLocations': [], 'namedShadowNames': []}, got absent.
3. UpdateIndexingConfiguration, iot-event-configuration-and-endpoint-settings#task#11 — status expected 400, got 200; body expected {'message': 'NamedShadowIndexingMode must be turned ON for providing the Filter for NamedShadowNames'}, got {}; x-amzn-errortype expected 'InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/', got absent.
4. GetTopicRule, iot-topic-rule-create-get-delete#task#2 — status 200 both; $.rule.properties expected [], got absent. Cluster of 7 steps.
5. CreatePackageVersion, iot-package-version-lifecycle#task#5 — status 200 both; $.attributes expected absent, got {'arch': 'arm64', 'stage': 'test'} (the values submitted in the request).
6. DescribeThingType, iot-thing-type-list-lifecycle#task#2 — $.thingTypeProperties.searchableAttributes[0] expected 'model', got 'serial'; [1] expected 'serial', got 'model'.
7. CreateTopicRule, iot-topic-rule-invalid-sql-error#task#1 — status expected 400, got 200; body expected {'message': "Expected 'SELECT'\n\nSELEKT !!! FROM WHERE\n^ at 1:1"}, got {}; x-amzn-errortype expected 'SqlParseException:http://iot.amazonaws.com/doc/2015-10-02/', got absent.

(The attribute values, field lists and SQL string above are this recording's values; the requirements are the always-present structures, the omission of the sensitive attributes map, the preserved list order, the fixed refusal texts and the parse-position rendering described per cause.)

## Steps to Reproduce
1. git clone the repo and boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Observe the seven clusters listed under Evidence, starting with fixture iot-event-configuration-and-endpoint-settings step task#8.
4. Equivalently by hand: GET /indexing/config and check thingIndexingConfiguration.filter and both managedFields lists are present; POST /indexing/config with namedShadowIndexingMode OFF and filter.namedShadowNames set (expect 400); POST /rules/<r> with valid SQL then GET /rules/<r> and check rule.properties is []; PUT /packages/<p>/versions/<v> with attributes and check the response omits attributes; create a thing type with searchableAttributes ['model','serial'] then GET /thing-types/<t> and check the order; POST /rules/<r2> with sql 'SELEKT !!! FROM WHERE' (expect 400 SqlParseException).
