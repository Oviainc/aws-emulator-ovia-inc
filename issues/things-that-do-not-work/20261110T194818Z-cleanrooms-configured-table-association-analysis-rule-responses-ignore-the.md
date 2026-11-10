Issue-Source: trace-sweep-divergence
Issue-Trace: cleanrooms-configured-table-association-analysis-rule-round-trip
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cleanrooms-configured-table-association-analysis-rule-round-trip
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: cleanrooms|CreateConfiguredTableAssociationAnalysisRule|2xx->2xx|$.analysisRule.configuredTableAssociationArn
Issue-Key: cleanrooms|CreateConfiguredTableAssociationAnalysisRule|2xx->2xx|$.analysisRule.configuredTableAssociationId
Issue-Key: cleanrooms|GetConfiguredTableAssociationAnalysisRule|2xx->2xx|$.analysisRule.configuredTableAssociationArn
Issue-Key: cleanrooms|GetConfiguredTableAssociationAnalysisRule|2xx->2xx|$.analysisRule.configuredTableAssociationId
Issue-Key: cleanrooms|GetConfiguredTableAssociationAnalysisRule|4xx->2xx|$.message
Issue-Key: cleanrooms|GetConfiguredTableAssociationAnalysisRule|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cleanrooms|GetConfiguredTableAssociationAnalysisRule|4xx->2xx|status
Issue-Commit: 472961b7eabfc5bed4558d84ebb9477fe0d8f292

# Configured table association analysis rule responses ignore the request path identifiers

> _Note: this issue was identified against the code base at commit `472961b7eabfc5bed4558d84ebb9477fe0d8f292`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfiguredTableAssociationAnalysisRule

## Description
Three causes, all in the cleanrooms configured-table-association analysis-rule path. The emulator does not carry the identifiers taken from the request URI (membershipIdentifier, configuredTableAssociationIdentifier, analysisRuleType) through into the ConfiguredTableAssociationAnalysisRule it stores and serializes, so the rule it returns identifies the wrong association, and the read does not key on the rule type at all.

Cause 1 of 3: CreateConfiguredTableAssociationAnalysisRule returns a rule whose configuredTableAssociationId / configuredTableAssociationArn are not the association addressed by the request.
AWS: Smithy: CreateConfiguredTableAssociationAnalysisRule POST /memberships/{membershipIdentifier}/configuredTableAssociations/{configuredTableAssociationIdentifier}/analysisRule, both @httpLabel @required; output member analysisRule -> ConfiguredTableAssociationAnalysisRule with @required members membershipIdentifier, configuredTableAssociationId (UUID, @length 36/36, @pattern ^[0-9a-f]{8}-...), configuredTableAssociationArn, policy, type, createTime, updateTime. AWS echoes the configuredTableAssociationIdentifier label from the URI as configuredTableAssociationId, and configuredTableAssociationArn is that association's ARN. Recorded behavior shows the ARN AWS renders for a configured table association in this response is membership-qualified: arn:aws:cleanrooms:<region>:<account>:membership/<membershipId>/configuredtableassociation/<configuredTableAssociationId>, i.e. the membership id from the URI followed by the association id from the URI (the model's ConfiguredTableAssociationArn @pattern is the shorter two-segment form, so the recording is the authority on the rendered layout here).
Emulator: the response's configuredTableAssociationId is a different, unrelated UUID than the one in the request URI, and configuredTableAssociationArn is built as arn:aws:cleanrooms:<region>:<account>:configuredtableassociation/<membershipId>/<that-unrelated-id>: the membership id is placed in the association-id position and the membership/ prefix segment is dropped. Replay: expected configuredTableAssociationId '125a69e9-...' (the URI label), got 'ec4841a3-...'; expected ARN '...:membership/67e9afd6-.../configuredtableassociation/125a69e9-...', got '...:configuredtableassociation/67e9afd6-.../ec4841a3-...'.
Why: a client that creates a collaboration analysis rule and then uses the returned configuredTableAssociationId/Arn to tag, look up or delete that association addresses a resource that does not exist, and ARN-based IAM/policy construction from the response produces a malformed Clean Rooms ARN (wrong resource path). The response cannot be correlated with the association the caller just configured.
Fix: CreateConfiguredTableAssociationAnalysisRule's handler/state: populate the stored rule's membershipIdentifier and configuredTableAssociationId from the request's @httpLabel values rather than from an unrelated/generated id, and fix the ARN builder for ConfiguredTableAssociationArn to render membership/<membershipId>/configuredtableassociation/<configuredTableAssociationId>.
Traces: cleanrooms-configured-table-association-analysis-rule-round-trip#task#11.

Cause 2 of 3: GetConfiguredTableAssociationAnalysisRule returns the same wrong configuredTableAssociationId / configuredTableAssociationArn on read-back.
AWS: Smithy: GetConfiguredTableAssociationAnalysisRule GET /memberships/{membershipIdentifier}/configuredTableAssociations/{configuredTableAssociationIdentifier}/analysisRule/{analysisRuleType}, @readonly; same output shape. The read-back of a rule created on an association reports that association's id and ARN, identical to what Create returned.
Emulator: the read returns the same unrelated id and the same mis-laid-out ARN as Cause 1 (expected configuredTableAssociationId '125a69e9-...', got 'ec4841a3-...'; expected '...:membership/<membershipId>/configuredtableassociation/<associationId>', got '...:configuredtableassociation/<membershipId>/<unrelated-id>').
Why: the defect is persisted, not just a serialization slip on Create: every subsequent read of the rule reports an association that does not exist, so round-trip identity is broken for all readers, not only the creating caller.
Fix: the same state/serializer path used by GetConfiguredTableAssociationAnalysisRule, so the id stored at Create time and the shared ARN renderer are both corrected (one fix should close Causes 1 and 2, but both steps must be re-replayed).
Traces: cleanrooms-configured-table-association-analysis-rule-round-trip#task#12.

Cause 3 of 3: GetConfiguredTableAssociationAnalysisRule answers 200 with a rule for an analysisRuleType that has no rule, instead of 404 ResourceNotFoundException.
AWS: Smithy: GetConfiguredTableAssociationAnalysisRule binds analysisRuleType as an @httpLabel @required path segment and models ResourceNotFoundException (errorKind client, httpError 404, @required members message/resourceId/resourceType) - 'Request references a resource which does not exist.' Asking for a type that has no rule on that association is a 404 whose x-amzn-errortype is the modeled error. The recorded message text is 'No such analysis rule with configuredTableAssociationId: <id> and type <type>': the fixed text is 'No such analysis rule with configuredTableAssociationId: ' and ' and type ', the <id> is the configuredTableAssociationIdentifier from the request URI, and <type> is the analysis-rule type as the request named it in the URI - the recording shows AWS echoing the request's raw type token (recorded: 'IntersectionList'), not a member of the modeled ConfiguredTableAssociationAnalysisRuleType enum (AGGREGATION|LIST|CUSTOM). Do not reuse the recorded id or type literally; both are request-derived values.
Emulator: answers HTTP 200 with an analysisRule body, emitting no message and no x-amzn-errortype header. Replay: expected status 404 with $.message 'No such analysis rule with configuredTableAssociationId: ... and type IntersectionList' and a shaped x-amzn-errortype; got 200 with both absent. The emulator evidently resolves the rule by association alone and ignores the analysisRuleType path segment.
Why: a client probing which collaboration analysis-rule types are configured (or branching on ResourceNotFoundException before creating one) is told every type exists and is handed a rule of the wrong type; error-driven control flow never fires, and the returned policy does not match the type requested. This is the association-side counterpart of open issue 20261031T001457Z-cleanrooms-getconfiguredtableanalysisrule-not-found-message-renders-the-modeled-enum.md, which covers the configured-TABLE operation's message wording only and does not cover this operation or the missing 404.
Fix: GetConfiguredTableAssociationAnalysisRule's handler/lookup: key the lookup on (membershipIdentifier, configuredTableAssociationIdentifier, analysisRuleType) and raise ResourceNotFoundException (404, x-amzn-errortype ResourceNotFoundException) with the message template above, echoing the request's raw analysisRuleType token, when no rule of that type exists.
Traces: cleanrooms-configured-table-association-analysis-rule-round-trip#task#14.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 70.3%, emulator_up=true), fixture cleanrooms-configured-table-association-analysis-rule-round-trip:

Step task#11 CreateConfiguredTableAssociationAnalysisRule - status 200 expected, 200 got; body mismatch:
  $.analysisRule.configuredTableAssociationId: expected '125a69e9-7678-4052-900a-d3cfd55dfbeb', got 'ec4841a3-70ea-4ca8-9ea1-b911fae7a845'
  $.analysisRule.configuredTableAssociationArn: expected 'arn:aws:cleanrooms:us-west-2:000000000000:membership/67e9afd6-1411-49b2-bce5-9acafbd59f69/configuredtableassociation/125a69e9-7678-4052-900a-d3cfd55dfbeb', got 'arn:aws:cleanrooms:us-west-2:000000000000:configuredtableassociation/67e9afd6-1411-49b2-bce5-9acafbd59f69/ec4841a3-70ea-4ca8-9ea1-b911fae7a845'

Step task#12 GetConfiguredTableAssociationAnalysisRule - status 200 expected, 200 got; identical two-member mismatch on read-back (same expected/actual values as task#11).

Step task#14 GetConfiguredTableAssociationAnalysisRule (type with no rule) - expected status 404, got 200:
  $.message: expected 'No such analysis rule with configuredTableAssociationId: 125a69e9-7678-4052-900a-d3cfd55dfbeb and type IntersectionList', got None
  $headers.x-amzn-errortype: expected a shaped error type, got None

(The recorded uuids, region and account above are per-run/per-environment values filling the templates; the requirements are the status, the error type, the message template and the ARN layout.)

## Steps to Reproduce
1. Replay fixture cleanrooms-configured-table-association-analysis-rule-round-trip from trace set simparity-wire-aws-v1 against the emulator.
2. Cause 1 - step task#11: POST /memberships/{membershipId}/configuredTableAssociations/{configuredTableAssociationId}/analysisRule. Compare $.analysisRule.configuredTableAssociationId against the configuredTableAssociationId in the request URI, and $.analysisRule.configuredTableAssociationArn against arn:aws:cleanrooms:<region>:<account>:membership/<membershipId>/configuredtableassociation/<configuredTableAssociationId>.
3. Cause 2 - step task#12: GET /memberships/{membershipId}/configuredTableAssociations/{configuredTableAssociationId}/analysisRule/{type} for the type just created; the same two members are wrong on read-back.
4. Cause 3 - step task#14: GET the same association's analysisRule path with an analysis-rule type for which no rule was created. Expect HTTP 404, x-amzn-errortype ResourceNotFoundException and message 'No such analysis rule with configuredTableAssociationId: <configuredTableAssociationId from the URI> and type <the raw type token from the URI>'; the emulator answers 200 with an analysisRule body.
