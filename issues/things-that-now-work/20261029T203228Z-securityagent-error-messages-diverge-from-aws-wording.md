Issue-Source: trace-sweep-divergence
Issue-Trace: securityagent-private-connection-error-paths
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityagent-private-connection-error-paths securityagent-provider-registration securityagent-target-domain-tagging-and-errors
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent|CreatePrivateConnection|4xx->4xx|$.message
Issue-Key: securityagent|DescribePrivateConnection|4xx->4xx|$.message
Issue-Key: securityagent|InitiateProviderRegistration|4xx->4xx|$.message
Issue-Key: securityagent|DeleteTargetDomain|4xx->4xx|$.message
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# securityagent error messages diverge from AWS wording

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePrivateConnection

## Description
Four causes, all the same defect class: the emulator reaches the right HTTP status and the right error shape, but writes its own message text instead of the text AWS produces for that error class. The message is part of the contract, so each of the four must be corrected.

Cause 1 of 4 - CreatePrivateConnection validation message renders the resource-configuration pattern differently from AWS.
AWS: Smithy: CreatePrivateConnection.mode (PrivateConnectionMode) -> selfManaged.resourceConfigurationId (ResourceConfigurationId, "The identifier or ARN of a VPC Lattice resource configuration"); modeled error ValidationException (client, HTTP 400). Per the recording the message is the standard smithy-validation template: fixed text "<N> validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", where N, <value> and <memberPath> come from the request and <pattern> is the service's own pattern source text - AWS renders the 12-digit account group as \\d{12}: ((rcfg-[0-9a-z]{17})|(arn:[a-z0-9\\-]+:vpc-lattice:[a-zA-Z0-9\\-]+:\\d{12}:resourceconfiguration/rcfg-[0-9a-z]{17})).
Emulator: same status 400 and same template, but the pattern is re-rendered with the account group expanded to a character class - ...:[0-9]{12}:resourceconfiguration/... - so the quoted pattern is not the service's pattern text.
Why: the pattern in the message is what a client (or a human reading a 400) uses to repair the request, and tests that assert on the documented pattern string fail against the emulator even though the request was rejected for the right reason.
Fix: the CreatePrivateConnection input validation - emit the pattern's source text verbatim rather than a normalized/recompiled form.
Traces: securityagent-private-connection-error-paths#task#1.

Cause 2 of 4 - DescribePrivateConnection not-found message is the emulator's own prose.
AWS: Smithy: DescribePrivateConnection(privateConnectionName) models ResourceNotFoundException (client, HTTP 404) with a required `message`. Recorded message for an unknown connection name is the fixed text "Private Connection not found: null" - no part of it is taken from the request (AWS interpolates an internal identifier that is absent here, hence the literal "null").
Emulator: status 404 is right, but the message is "Private connection '<name>' was not found. Verify that the resource identifier is correct and that the resource exists in the specified agent space or account." - the exception shape's documentation string with the requested name interpolated, not the service's message.
Why: clients and assertions that match on the not-found message text for this operation do not match, and the differing shape of the text (interpolated name vs fixed text) makes the emulator's 404 distinguishable from the real one.
Fix: the DescribePrivateConnection handler's not-found error construction - use the operation's own message text instead of the generic ResourceNotFoundException doc string.
Traces: securityagent-private-connection-error-paths#task#2.

Cause 3 of 4 - InitiateProviderRegistration enum-constraint message lists the allowed values in the wrong order.
AWS: Smithy: InitiateProviderRegistration.provider targets the Provider enum, modeled error ValidationException (client, HTTP 400). Recorded message: "1 validation error detected: Value '<value>' at 'provider' failed to satisfy constraint: Member must satisfy enum value set: [GITHUB, AZURE_DEVOPS, JIRA, BITBUCKET, CONFLUENCE, GITLAB]" - fixed text plus the rejected value from the request; the bracketed value set is itself fixed text in AWS's order, which is NOT the model's declaration order.
Emulator: status 400 and the same template, but the value set is emitted in model declaration order - [GITHUB, GITLAB, BITBUCKET, CONFLUENCE, AZURE_DEVOPS, JIRA] (smithy_query shape=Provider returns exactly that order), i.e. the emulator serializes the enum as declared rather than as the service words it.
Why: the message differs from AWS's for the same rejection, so clients that surface or assert the allowed-value list see a different string; the set is identical, only the fixed ordering of the message text is wrong.
Fix: the provider enum validation's message rendering - emit the value set in the service's fixed order for this constraint message.
Traces: securityagent-provider-registration#task#2.

Cause 4 of 4 - DeleteTargetDomain not-found message is the emulator's own prose.
AWS: Smithy: DeleteTargetDomain(targetDomainId, required) is idempotent and declares no operation-specific errors, but the recording shows a 404 for an unknown targetDomainId whose message is the fixed text "The specified target domain does not exist." - entirely fixed, with no value from the request interpolated.
Emulator: status 404 is right, message is "No target domain found with id <targetDomainId>" - different fixed text plus the requested id interpolated into it.
Why: a client matching on the documented not-found text for target domains fails to recognize the emulator's 404, and the emulator leaks the request id into a message AWS keeps value-free.
Fix: the DeleteTargetDomain handler's not-found error construction - use the service's fixed message text with no interpolation.
Traces: securityagent-target-domain-tagging-and-errors#task#8.

## Evidence
Cause 1 (securityagent-private-connection-error-paths#task#1, CreatePrivateConnection, expected 400 / actual 400): $.message value_mismatch. Expected "1 validation error detected: Value 'not-a-valid-resource-configuration-id' at 'mode.selfManaged.resourceConfigurationId' failed to satisfy constraint: Member must satisfy regular expression pattern: ((rcfg-[0-9a-z]{17})|(arn:[a-z0-9\\-]+:vpc-lattice:[a-zA-Z0-9\\-]+:\\d{12}:resourceconfiguration/rcfg-[0-9a-z]{17}))"; actual identical except the account group is rendered "[0-9]{12}" instead of "\\d{12}". Fixed text = the validation template and the pattern source; variable parts = the error count, the rejected value and the member path, all from the request.

Cause 2 (securityagent-private-connection-error-paths#task#2, DescribePrivateConnection, 404 / 404): $.message expected 'Private Connection not found: null' (wholly fixed text); actual "Private connection 'sim2real-nonexistent-pconn' was not found. Verify that the resource identifier is correct and that the resource exists in the specified agent space or account." - the quoted name is the request's privateConnectionName, the remainder is the ResourceNotFoundException doc string.

Cause 3 (securityagent-provider-registration#task#2, InitiateProviderRegistration, 400 / 400): $.message expected "...Member must satisfy enum value set: [GITHUB, AZURE_DEVOPS, JIRA, BITBUCKET, CONFLUENCE, GITLAB]"; actual "...[GITHUB, GITLAB, BITBUCKET, CONFLUENCE, AZURE_DEVOPS, JIRA]". Fixed text = template + bracketed value set in AWS's order; variable part = the rejected value ('NOT_A_REAL_PROVIDER' here) and the member path 'provider'.

Cause 4 (securityagent-target-domain-tagging-and-errors#task#8, DeleteTargetDomain, 404 / 404): $.message expected 'The specified target domain does not exist.' (wholly fixed text, no request value); actual 'No target domain found with id td-doesnotexist0000' - different fixed text with the request's targetDomainId appended.

In all four the status and error shape match; only the message text diverges. No other open securityagent issue covers them (no existing issues match 'securityagent').

## Steps to Reproduce
1. git clone the repo and boot the emulator from that checkout.
2. Replay fixture securityagent-private-connection-error-paths: at task#1 call CreatePrivateConnection with mode.selfManaged.resourceConfigurationId set to a value that does not match the VPC Lattice resource-configuration pattern and diff $.message against the recording (pattern rendered [0-9]{12} instead of \\d{12}) - cause 1. At task#2 call DescribePrivateConnection with a name that does not exist and diff $.message (emulator prose instead of 'Private Connection not found: null') - cause 2.
3. Replay fixture securityagent-provider-registration to task#2: call InitiateProviderRegistration with provider set to a value outside the Provider enum and diff $.message - the enum value set is listed in model declaration order - cause 3.
4. Replay fixture securityagent-target-domain-tagging-and-errors to task#8: call DeleteTargetDomain with an unknown targetDomainId and diff $.message against 'The specified target domain does not exist.' - cause 4.
