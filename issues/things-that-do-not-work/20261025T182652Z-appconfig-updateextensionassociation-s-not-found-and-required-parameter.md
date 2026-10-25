Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.update-extension-association-nonexistent-rejected appconfig.update-extension-association-wrong-resource-type-rejected
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.update-extension-association-nonexistent-rejected|run#21|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: appconfig.update-extension-association-wrong-resource-type-rejected|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 39e54f9f86d0b102f322a2103cd9927458aab9c3

# AppConfig UpdateExtensionAssociation's not-found and required-parameter refusal messages do not use AWS's templates (2 causes)

> _Note: this issue was identified against the code base at commit `39e54f9f86d0b102f322a2103cd9927458aab9c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateExtensionAssociation

## Description
Two distinct net-new root causes on AppConfig's UpdateExtensionAssociation refusal path, each grounded in one replayed behavioral check. In BOTH the HTTP status and the error code now MATCH AWS; the only divergence is the refusal's Message. They are numbered Cause 1 of 2 and Cause 2 of 2 — close BOTH before submitting, and re-replay every check id in trace_ids.

What is already filed and why these are not covered: the open issue 20261025T034405Z-appconfig-updateextensionassociation-answers-501-notimplemented-and-createextensionassociation-s.md lists these two check ids under its Cause 1 ("UpdateExtensionAssociation answers 501 NotImplemented"), but its replay keys for them are the `assert code == ...` lines, and that part is now working: in this run the operation is implemented and answers ResourceNotFoundException / BadRequestException with the right codes, and the checks fail further down on the `assert message == ...` lines, which no open issue carries. The replay report marked neither check ALREADY FILED nor OWNED. The sibling open issue 20261024T131534Z-appconfig-refusal-messages-on-the-tagging-pagination-hosted.md is the same CLASS (request-derived message templates) but names only TagResource/UntagResource/ListTagsForResource, the pagination lists, CreateHostedConfigurationVersion, CreateExperimentDefinition and UpdateExtension — not UpdateExtensionAssociation. The fixed issue 20261025T093741Z-appconfig-updateextensionassociation-stub.md is NOT reopened here: both calls are refused exactly as AWS refuses them (404 and 400 on calls AWS also refuses), so its fix did not leave a call wrongly refused.

=== Cause 1 of 2: the nonexistent-association refusal renders the resource type as two words ("Extension Association") instead of AWS's "ExtensionAssociation" ===
AWS: UpdateExtensionAssociation on an ExtensionAssociationId that satisfies the id pattern but names no association is refused ResourceNotFoundException / HTTP 404 (Smithy: UpdateExtensionAssociation PATCH /extensionassociations/{ExtensionAssociationId}, success 200, input ExtensionAssociationId @httpLabel @required target Id pattern ^[a-z0-9]{4,7}$ plus Parameters (ParameterValueMap, length 0..10); errors BadRequestException 400, ResourceNotFoundException 404, InternalServerException 500). The message is recorded behavior only, but it is a fixed template with one request/state-derived value: the fixed text is "ExtensionAssociation could not be found for account " + <the caller's account id> + "." — the resource-type token is the single CamelCase word "ExtensionAssociation", and the refusal deliberately does NOT echo the submitted association id.
Emulator: the status (404) and the code (ResourceNotFoundException) are correct, and the rejected call correctly leaves the real association untouched, but the resource-type token is rendered as two space-separated words: "Extension Association could not be found for account <accountId>."
Why: this refusal text is the one AppConfig surface that tells a caller which resource kind was missing; a client, log scraper, error classifier or snapshot test that matches AppConfig's sentence does not match the emulator's, and the difference is invisible to inspection because every other part of the template — including the account id — is right.
Fix: the not-found message builder behind the extension-association lookup in the UpdateExtensionAssociation handler (services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/update_extension_association.rs, or the shared appconfig not-found message helper it calls) must emit the single token "ExtensionAssociation"; the account id stays state-derived.
Traces: appconfig.update-extension-association-nonexistent-rejected [mismatch].

=== Cause 2 of 2: the required-parameter refusal uses an invented sentence AND names the extension by Name where AWS names it by extension Id ===
AWS: UpdateExtensionAssociation whose Parameters map drops a value for a parameter the associated extension version's schema declares Required is refused BadRequestException / HTTP 400, and the association's stored Parameters are left unchanged. The message is recorded behavior only, but it is a request/state-derived template: the fixed text is "Parameter " + <the missing parameter's name> + "  is required for extension " + <the extension's Id> + " version " + <the extension version number> + ", but a value has not been supplied." — note AWS renders TWO spaces after the parameter name, states the condition ("but a value has not been supplied"), and identifies the extension by its ID (the ^[a-z0-9]{4,7}$ identifier), not by its Name. The variable parts come from the request's Parameters map and the resolved extension's Id/VersionNumber; the identifier values in the run (parameter "RequiredParam", version 1) are examples, not part of the template.
Emulator: status 400 and code BadRequestException are correct, but the message is a shorter invented sentence that also substitutes the wrong identifier: "Parameter <name> is required by extension <extensionNAME> version <n>." — one space after the parameter name, "is required by" instead of "is required for", no "but a value has not been supplied" clause, and the extension's Name where AWS puts its Id. This is the same per-parameter sentence the emulator already emits on CreateExtensionAssociation (Cause 2 of the open 20261025T034405Z issue), so the two paths share one wrong renderer.
Why: beyond the shape difference, a caller reading the refusal is handed a human-chosen extension Name where AWS hands it the opaque extension Id, so logic or tooling that extracts the identifier from the refusal to re-resolve the extension (GetExtension takes either, but ids and names are not interchangeable in snapshots or correlation) reads a value AppConfig never puts there, and the "a value has not been supplied" distinction — missing value versus wrong value — is lost.
Fix: the required-parameter validation branch of the UpdateExtensionAssociation handler (services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/update_extension_association.rs) must render AWS's template, including the double space and the trailing clause, and must substitute the resolved extension's Id and VersionNumber rather than its Name; the refusal must keep leaving stored Parameters untouched (the emulator already does).
Traces: appconfig.update-extension-association-wrong-resource-type-rejected [mismatch].

## Evidence
Behavioral replay, suite simparity-behavioral-ga-prod-v1, against the booted emulator (bootstrap_signal false — the emulator answered every request, so these are fidelity divergences). Total 43/97 passed; appconfig 38/83. In both failures below the HTTP status and the error code matched and only $.Message diverged. The account id ('000000000000'), generated extension name, extension id and parameter name are the run's values and are EXAMPLES; the requirement in each cause is the template.

Cause 1 — appconfig.update-extension-association-nonexistent-rejected [mismatch]:
  File "check_appconfig_update_extension_association_nonexistent_rejected.py", line 74, in run
    assert message == expected, f"expected message {expected!r}, got {message!r}"
  AssertionError: expected message 'ExtensionAssociation could not be found for account 000000000000.', got 'Extension Association could not be found for account 000000000000.'
The failing call is update_extension_association(ExtensionAssociationId=<the real association id reversed, so it still matches ^[a-z0-9]{4,7}$>, Parameters={}). The preceding assertion that the code is ResourceNotFoundException PASSED.

Cause 2 — appconfig.update-extension-association-wrong-resource-type-rejected [mismatch]:
  File "check_appconfig_update_extension_association_wrong_resource_type_rejected.py", line 50, in run
    assert message == expected, f"expected message {expected!r}, got {message!r}"
  AssertionError: expected message 'Parameter RequiredParam  is required for extension ho65pb1 version 1, but a value has not been supplied.', got 'Parameter RequiredParam is required by extension sim2real-ext-f68fdc1fc110 version 1.'
The failing call is update_extension_association(ExtensionAssociationId=<real id>, Parameters={'OptionalParam': 'opt-value'}) on an association created with Parameters={'RequiredParam': 'initial-value'} against an extension declaring RequiredParam as Required. The preceding assertion that the code is BadRequestException PASSED; note the expected message carries the extension ID ('ho65pb1') while the emulator emitted the extension NAME ('sim2real-ext-f68fdc1fc110'), and the expected text has two spaces after 'RequiredParam'.

Smithy consulted: services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json — UpdateExtensionAssociation PATCH /extensionassociations/{ExtensionAssociationId}, success 200; input ExtensionAssociationId (@httpLabel, @required, pattern ^[a-z0-9]{4,7}$) and Parameters (ParameterValueMap, length 0..10); output ExtensionAssociation (Id, Arn, ExtensionArn, ResourceArn, Parameters, ExtensionVersionNumber); errors BadRequestException (client, 400), ResourceNotFoundException (client, 404), InternalServerException (server, 500). The model fixes the status/code pairs; the message texts are recorded behavior only and are stated above as templates.

## Steps to Reproduce
Boot the emulator and point an appconfig client (plus an sqs client for a queue ARN to use as the extension action Uri) at its data listener. Compare ONLY $.Message of each refusal — status and code already match.

Cause 1:
1. sqs create_queue -> get_queue_attributes(AttributeNames=['QueueArn']) -> queue_arn.
2. appconfig create_extension(Name=N, Actions={'ON_DEPLOYMENT_COMPLETE': [{'Name': 'notify', 'Uri': queue_arn}]}) -> extension_id.
3. create_application(Name=A) -> app_id; build app_arn = arn:aws:appconfig:<region>:<account>:application/<app_id>.
4. create_extension_association(ExtensionIdentifier=extension_id, ResourceIdentifier=app_arn) -> association_id.
5. update_extension_association(ExtensionAssociationId=<association_id reversed, still matching ^[a-z0-9]{4,7}$>, Parameters={}).
   Expected: HTTP 404, ResourceNotFoundException, Message 'ExtensionAssociation could not be found for account <accountId>.'
   Actual:   HTTP 404, ResourceNotFoundException, Message 'Extension Association could not be found for account <accountId>.'
6. Re-replay appconfig.update-extension-association-nonexistent-rejected.

Cause 2:
1. create_extension(Name=N, Actions={'ON_DEPLOYMENT_COMPLETE': [{'Name': 'notify', 'Uri': <queue ARN>}]}, Parameters={'RequiredParam': {'Required': True}, 'OptionalParam': {'Required': False}}) -> extension_id, version 1.
2. create_application + build its ARN; create_extension_association(ExtensionIdentifier=extension_id, ExtensionVersionNumber=1, ResourceIdentifier=<app arn>, Parameters={'RequiredParam': 'initial-value'}) -> association_id.
3. update_extension_association(ExtensionAssociationId=association_id, Parameters={'OptionalParam': 'opt-value'}).
   Expected: HTTP 400, BadRequestException, Message 'Parameter RequiredParam  is required for extension <extensionId> version 1, but a value has not been supplied.' (two spaces after the parameter name; the extension's ID, not its Name).
   Actual:   HTTP 400, BadRequestException, Message 'Parameter RequiredParam is required by extension <extensionName> version 1.'
4. get_extension_association(ExtensionAssociationId=association_id) must still report Parameters == {'RequiredParam': 'initial-value'} (the emulator already satisfies this).
5. Re-replay appconfig.update-extension-association-wrong-resource-type-rejected.
