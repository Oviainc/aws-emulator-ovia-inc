Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: securityagent.add-artifact-invalid-type-rejected securityagent.create-private-connection-nonexistent-self-managed-resource-rejected
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent.add-artifact-invalid-type-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: securityagent.create-private-connection-nonexistent-self-managed-resource-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: b4a0d642bb551c1144d98d8c876f3d4bc2002490

# securityagent error-message text diverges from AWS for two rejections (AddArtifact enum set order, CreatePrivateConnection resource-not-found)

> _Note: this issue was identified against the code base at commit `b4a0d642bb551c1144d98d8c876f3d4bc2002490`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddArtifact

## Description
Two net-new causes, same defect class: the emulator reaches the correct HTTP status and the correct error shape/code, but emits its own message text instead of the text AWS produces for that error class. The message is part of the contract, so both must be corrected. Note: the open issue 20261029T203228Z-securityagent-error-messages-diverge-from-aws-wording.md describes the same defect CLASS for other operations (CreatePrivateConnection ValidationException pattern rendering, DescribePrivateConnection/DeleteTargetDomain not-found text, InitiateProviderRegistration Provider enum order); it does NOT cover either cause below — neither of these two rejection paths (AddArtifact's ArtifactType enum message, and CreatePrivateConnection's ResourceNotFoundException for a missing VPC Lattice resource configuration) is among its causes, and the replay suite did not mark these checks as owned by it.

Cause 1 of 2 — AddArtifact rejection of an out-of-enum artifactType lists the allowed value set in model declaration order instead of AWS's fixed order.
AWS: Smithy: AddArtifact.artifactType is required and targets the enum ArtifactType; modeled error ValidationException (client, HTTP 400, httpError defaulted). The rejection uses the standard smithy-validation template: fixed text "<N> validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must satisfy enum value set: [<values>]", where N, <value> and <memberPath> come from the request. Per the recorded behavior the bracketed value set is itself FIXED text in AWS's own order — [DOCX, YAML, JSON, JPEG, PNG, TXT, PDF, MD, DOC] — which is NOT the model's declaration order (smithy_query shape=ArtifactType returns values TXT, PNG, JPEG, MD, PDF, DOCX, DOC, JSON, YAML).
Emulator: status 400, code ValidationException and the same template are right, but the value set is serialized in model declaration order: "1 validation error detected: Value 'EXE' at 'artifactType' failed to satisfy constraint: Member must satisfy enum value set: [TXT, PNG, JPEG, MD, PDF, DOCX, DOC, JSON, YAML]". Fixed text = the template plus the bracketed set in AWS's order; variable parts = the error count, the rejected value and the member path, all taken from the request.
Why: a client that surfaces, logs or asserts on the allowed-value list for this rejection sees a different string than real AWS returns; the set of values is identical, only the fixed ordering of the message is wrong, so the divergence is invisible until a text comparison fails.
Fix: the AddArtifact input validation / the enum-constraint message rendering for ArtifactType — emit the value set in the service's fixed order for this constraint message rather than the model's declaration order.
Traces: securityagent.add-artifact-invalid-type-rejected.

Cause 2 of 2 — CreatePrivateConnection not-found message appends emulator-specific detail and the generic exception doc string.
AWS: Smithy: CreatePrivateConnection(privateConnectionName required, mode required -> PrivateConnectionMode.selfManaged.resourceConfigurationId) models ResourceNotFoundException (client, HTTP 404) with a required `message`. For a self-managed mode whose resourceConfigurationId names no existing VPC Lattice resource configuration, the recorded message is "A resource referenced by private connection '<privateConnectionName>' was not found. Verify the resource configuration exists and try again." — everything outside the quoted name is fixed text; the only variable part is the privateConnectionName copied from the request. Notably AWS does NOT echo the offending resourceConfigurationId.
Emulator: status 404 and code ResourceNotFoundException are right, but the message is "A resource referenced by private connection '<name>' was not found: mode.selfManaged.resourceConfigurationId '<arn>' does not name a VPC Lattice resource configuration in this account. Verify that the resource identifier is correct and that the resource exists in the specified agent space or account." — i.e. the correct opening clause is terminated with ':' and followed by the member path plus the request's resourceConfigurationId, and the closing sentence is the ResourceNotFoundException shape's documentation string rather than AWS's "Verify the resource configuration exists and try again."
Why: clients and assertions matching the documented not-found text for this operation do not match, and the emulator leaks a request value (the resource-configuration identifier) into a message AWS keeps free of it, making the emulator's 404 trivially distinguishable from the real one.
Fix: the CreatePrivateConnection handler's not-found error construction — build the message from the operation's own fixed text with only privateConnectionName interpolated, instead of concatenating member-path detail and the generic exception doc string.
Traces: securityagent.create-private-connection-nonexistent-self-managed-resource-rejected.

## Evidence
Cause 1 (check securityagent.add-artifact-invalid-type-rejected, AddArtifact, expected 400 ValidationException / actual 400 ValidationException): message value_mismatch.
  expected: "1 validation error detected: Value 'EXE' at 'artifactType' failed to satisfy constraint: Member must satisfy enum value set: [DOCX, YAML, JSON, JPEG, PNG, TXT, PDF, MD, DOC]"
  actual:   "1 validation error detected: Value 'EXE' at 'artifactType' failed to satisfy constraint: Member must satisfy enum value set: [TXT, PNG, JPEG, MD, PDF, DOCX, DOC, JSON, YAML]"
Fixed text = validation template + bracketed value set in AWS's order. Variable text = error count, rejected value ('EXE' in this run) and member path ('artifactType'), all from the request. smithy_query shape=ArtifactType yields exactly the emulator's ordering, confirming the emulator serializes declaration order.

Cause 2 (check securityagent.create-private-connection-nonexistent-self-managed-resource-rejected, CreatePrivateConnection, expected 404 ResourceNotFoundException / actual 404 ResourceNotFoundException): message value_mismatch.
  expected: "A resource referenced by private connection '<privateConnectionName>' was not found. Verify the resource configuration exists and try again."
  actual:   "A resource referenced by private connection '<privateConnectionName>' was not found: mode.selfManaged.resourceConfigurationId '<resourceConfigurationId>' does not name a VPC Lattice resource configuration in this account. Verify that the resource identifier is correct and that the resource exists in the specified agent space or account."
(In the observed run <privateConnectionName> was the harness-generated name and <resourceConfigurationId> a generated arn:aws:vpc-lattice:...:resourceconfiguration/rcfg-... value; those per-run values fill the template and are not part of the requirement. The closing sentence of the actual message is verbatim the ResourceNotFoundException shape's doc string: "The specified resource was not found. Verify that the resource identifier is correct and that the resource exists in the specified agent space or account.")

In both causes the HTTP status and the error code/shape match AWS; only the message text diverges. Both checks also assert the rejection left no state behind and that a positive control succeeded — those assertions were reached only after the message comparison in cause 2's case, so the message is the sole observed divergence.

## Steps to Reproduce
1. git clone the repo and boot the emulator from that checkout (run_behavioral_suite does this).
2. Cause 1: replay check securityagent.add-artifact-invalid-type-rejected — CreateAgentSpace, then AddArtifact(agentSpaceId=<id>, artifactContent=b"malware.exe", artifactType="EXE", fileName="bad.exe"). Catch the ClientError and compare Error.Message against "1 validation error detected: Value 'EXE' at 'artifactType' failed to satisfy constraint: Member must satisfy enum value set: [DOCX, YAML, JSON, JPEG, PNG, TXT, PDF, MD, DOC]"; the emulator returns the same template with the set in model declaration order.
3. Cause 2: replay check securityagent.create-private-connection-nonexistent-self-managed-resource-rejected — call CreatePrivateConnection(privateConnectionName=<fresh name>, mode={"selfManaged": {"resourceConfigurationId": "arn:aws:vpc-lattice:<region>:<account>:resourceconfiguration/rcfg-<17 hex chars>"}}) for a resource configuration that does not exist. Catch the ClientError: code is ResourceNotFoundException (correct) but Error.Message carries the extra ":  mode.selfManaged.resourceConfigurationId '<arn>' does not name ..." clause and the generic doc-string tail instead of AWS's "... was not found. Verify the resource configuration exists and try again.".
