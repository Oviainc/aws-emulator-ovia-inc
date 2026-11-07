Issue-Source: trace-sweep-divergence
Issue-Trace: iot-authorizer-duplicate-and-notfound-errors
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-authorizer-duplicate-and-notfound-errors iot-authorizer-error-paths iot-authorizer-lifecycle iot-custom-authorizer-lifecycle iot-certificate-policy-attachment-to-thing iot-command-duplicate-and-execution-errors iot-command-lifecycle iot-job-and-job-execution-error-paths iot-package-duplicate-and-notfound-errors iot-package-error-paths iot-thing-and-group-lifecycle iot-thing-duplicate-and-notfound-errors iot-thing-group-duplicate-and-idempotent-delete iot-thing-group-hierarchy-and-duplicate-error iot-thing-type-and-thing-lifecycle iot-topic-rule-duplicate-create-error iot-topic-rule-lifecycle iot-topic-rule-recreate-conflict-error
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|CreateAuthorizer|4xx->4xx|$.message
Issue-Key: iot|CreatePolicy|4xx->4xx|$.message
Issue-Key: iot|CreateCommand|4xx->4xx|$.message
Issue-Key: iot|CreateCommand|4xx->4xx|$.resourceId
Issue-Key: iot|CreateJob|4xx->4xx|$.message
Issue-Key: iot|CreateJob|4xx->4xx|$.resourceArn
Issue-Key: iot|CreateJob|4xx->4xx|$.resourceId
Issue-Key: iot|CreatePackage|4xx->4xx|$.message
Issue-Key: iot|CreatePackage|4xx->4xx|$.resourceId
Issue-Key: iot|CreateThing|4xx->4xx|$.message
Issue-Key: iot|CreateThing|4xx->4xx|$.resourceArn
Issue-Key: iot|CreateThing|4xx->4xx|$.resourceId
Issue-Key: iot|CreateThingGroup|4xx->4xx|$.message
Issue-Key: iot|CreateThingGroup|4xx->4xx|$.resourceArn
Issue-Key: iot|CreateThingGroup|4xx->4xx|$.resourceId
Issue-Key: iot|CreateThingType|4xx->4xx|$.message
Issue-Key: iot|CreateThingType|4xx->4xx|$.resourceArn
Issue-Key: iot|CreateThingType|4xx->4xx|$.resourceId
Issue-Key: iot|CreateTopicRule|4xx->4xx|$.message
Issue-Key: iot|CreateTopicRule|4xx->4xx|$.resourceArn
Issue-Key: iot|CreateTopicRule|4xx->4xx|$.resourceId
Issue-Commit: ff097a89ab5b098f807ae300b709617042ce117b

# iot already-exists conflicts: 9 causes — reworded ConflictException text and resourceId/resourceArn members AWS does not send

> _Note: this issue was identified against the code base at commit `ff097a89ab5b098f807ae300b709617042ce117b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
Every IoT create-when-already-present refusal in the emulator is built from one generic template, '<Resource> <name> already exists' (sometimes '... with a different configuration'), and several of them additionally populate resourceId / resourceArn members that AWS leaves absent in the 409 body. AWS IoT words this refusal differently per resource family — some sentences mention the account, some are phrased as a conditional-check failure, some end in a period. The status (409) is correct in all nine cases; the body is not. 9 net-new causes, all in service iot.

Cause 1 of 9: CreateAuthorizer on an existing authorizer name.
AWS: Smithy: CreateAuthorizer errors include ResourceAlreadyExistsException (httpError 409); authorizerName is @required, @length(1..128), @pattern ^[\w=,@-]+$. Recorded behavior: 409 whose message is the fixed text 'Authorizer ' + <authorizerName> + ' already exist for this account' — note the non-inflected 'already exist' and the ' for this account' tail; the only variable part is the requested name.
Emulator: 409 'Authorizer <name> already exists'.
Why: a client matching AWS's sentence (or surfacing it to a user) sees text AWS never emits, and loses the account-scope clause that tells the caller the collision is account-wide rather than regional.
Fix: the CreateAuthorizer handler's duplicate-name refusal construction.
Traces: iot-authorizer-duplicate-and-notfound-errors#task#2 (also iot-authorizer-error-paths#task#4, iot-authorizer-lifecycle#task#7, iot-custom-authorizer-lifecycle#task#7).

Cause 2 of 9: CreatePolicy on an existing policy name.
AWS: Smithy: CreatePolicy errors include ResourceAlreadyExistsException (409); policyName @required, @length(1..128), @pattern ^[\w+=,.@-]+$. Recorded behavior: 409 whose message is 'Policy cannot be created - name already exists (name=' + <policyName> + ')' — fixed text 'Policy cannot be created - name already exists (name=' and the closing ')', variable part the requested policyName.
Emulator: 409 'Policy <name> already exists'.
Why: the parenthesised name=<...> form is what AWS emits for this family; clients that extract the colliding name from it get nothing.
Fix: the CreatePolicy handler's duplicate-name refusal construction.
Traces: iot-certificate-policy-attachment-to-thing#task#3.

Cause 3 of 9: CreateCommand on an existing commandId, plus an invented resourceId member.
AWS: Smithy: CreateCommand errors include ConflictException (409); commandId @required, @length(1..64), @pattern ^[a-zA-Z0-9_-]+$. Recorded behavior: 409 whose message is 'Command ' + <commandId> + ' already exists.' (terminating period) and whose body carries NO resourceId member.
Emulator: 409 'Command <id> already exists' (no period) AND a resourceId member set to the commandId.
Why: the extra member changes the response shape — a strict client deserialising ConflictException sees a field AWS never sends, and the message differs by its period.
Fix: the CreateCommand handler's conflict refusal and the ConflictException serializer (do not populate resourceId for this operation).
Traces: iot-command-duplicate-and-execution-errors#task#2 (also iot-command-lifecycle#task#4).

Cause 4 of 9: CreateJob on an existing jobId, plus invented resourceArn/resourceId members.
AWS: recorded behavior: 409 whose message is 'Job ' + <jobId> + ' already exists.' and whose body carries neither resourceArn nor resourceId.
Emulator: 409 'Job <id> already exists' (no period) plus resourceArn (the job ARN it built) and resourceId (the jobId).
Why: same shape divergence plus the missing period.
Fix: the CreateJob handler's conflict refusal construction.
Traces: iot-job-and-job-execution-error-paths#task#3.

Cause 5 of 9: CreatePackage on an existing package name, plus an invented resourceId member.
AWS: recorded behavior: 409 whose message is phrased as a conditional-check failure — the fixed text 'Conditional check failure: Received duplicate request for package ' + <packageName> — and the body carries NO resourceId.
Emulator: 409 'Package <name> already exists' plus resourceId set to the package name.
Why: this family's AWS wording is entirely different in form (it reports a duplicate REQUEST, not an existing resource), so an emulator-authored 'already exists' both misleads the caller about idempotency semantics and fails text matching.
Fix: the CreatePackage handler's conflict refusal construction.
Traces: iot-package-duplicate-and-notfound-errors#task#3 (also iot-package-error-paths#task#5).

Cause 6 of 9: CreateThing with the same name but different attributes.
AWS: Smithy: CreateThing errors include ResourceAlreadyExistsException (409); thingName @required, @length(1..128), @pattern ^[a-zA-Z0-9:_-]+$. Recorded behavior: 409 whose message is 'Thing ' + <thingName> + ' already exists in account with different attributes' — fixed tail ' already exists in account with different attributes' — and NO resourceArn/resourceId members.
Emulator: 409 'Thing <name> already exists with a different configuration' plus resourceArn and resourceId.
Why: AWS names the conflicting dimension ('different attributes') and the account scope; 'different configuration' is the emulator's own prose, and the two extra members change the body shape.
Fix: the CreateThing handler's conflict refusal construction.
Traces: iot-thing-and-group-lifecycle#task#4 (also iot-thing-duplicate-and-notfound-errors#task#4).

Cause 7 of 9: CreateThingGroup with the same name but different properties.
AWS: recorded behavior: 409 whose message is 'Thing Group ' + <thingGroupName> + ' already exists in current account with different properties' — note 'Thing Group' as two capitalised words, 'in current account', and 'properties' — and no resourceArn/resourceId.
Emulator: 409 'Thing group <name> already exists with a different configuration' plus resourceArn and resourceId.
Fix: the CreateThingGroup handler's conflict refusal construction.
Why: same wording and shape divergence; the conflicting dimension for groups is 'properties', not 'configuration'.
Traces: iot-thing-group-duplicate-and-idempotent-delete#task#2 (also iot-thing-group-hierarchy-and-duplicate-error#task#3, iot-thing-group-hierarchy-and-duplicate-error#task#4).

Cause 8 of 9: CreateThingType with the same name but different properties.
AWS: recorded behavior: 409 'Thing Type ' + <thingTypeName> + ' already exists in current account with different properties', no resourceArn/resourceId.
Emulator: 409 'Thing type <name> already exists with a different configuration' plus resourceArn and resourceId.
Fix: the CreateThingType handler's conflict refusal construction.
Why: same as cause 7 for the thing-type family.
Traces: iot-thing-type-and-thing-lifecycle#task#2.

Cause 9 of 9: CreateTopicRule on an existing rule name.
AWS: Smithy: CreateTopicRule errors include ResourceAlreadyExistsException (409) and ConflictingResourceUpdateException (409); ruleName @required, @length(1..128), @pattern ^[a-zA-Z0-9_]+$. Recorded behavior: 409 whose message is the single-quoted form "Topic Rule '" + <ruleName> + "' already exists for account" — fixed text 'Topic Rule ', the surrounding single quotes, and ' already exists for account' — and no resourceArn/resourceId.
Emulator: 409 'Rule with given name already exists (name=<ruleName>)' plus resourceArn and resourceId.
Why: the emulator uses the parenthesised form here and the bare form for policies — exactly inverted from AWS, which uses the parenthesised form for CreatePolicy and the quoted form for CreateTopicRule.
Fix: the CreateTopicRule handler's conflict refusal construction.
Traces: iot-topic-rule-duplicate-create-error#task#2 (also iot-topic-rule-lifecycle#task#3, iot-topic-rule-recreate-conflict-error#task#2).

Related open issue that does NOT cover these: '20261106T201842Z-iot-registry-plane-9-net-new-causes-creatething.md' is a behavioral finding and carries no replay key for any of the nine fixture steps above.

## Evidence
Replay of trace set simparity-wire-aws-v1. Expected = recorded AWS response, Actual = emulator. Status was 409 and matched in all nine clusters; only the body diverged.

1. CreateAuthorizer, iot-authorizer-duplicate-and-notfound-errors#task#2 — $.message expected 'Authorizer sim2real-probe-iot-authorizer-3fd37fcf already exist for this account', got 'Authorizer sim2real-probe-iot-authorizer-3fd37fcf already exists'. Cluster of 4.
2. CreatePolicy, iot-certificate-policy-attachment-to-thing#task#3 — expected 'Policy cannot be created - name already exists (name=sim2real-probe-iot-policy-ab1f033e)', got 'Policy sim2real-probe-iot-policy-ab1f033e already exists'.
3. CreateCommand, iot-command-duplicate-and-execution-errors#task#2 — expected 'Command sim2real-probe-iot-cmd-err-6ae05848 already exists.', got '... already exists'; $.resourceId expected absent, got 'sim2real-probe-iot-cmd-err-6ae05848'. Cluster of 2.
4. CreateJob, iot-job-and-job-execution-error-paths#task#3 — expected 'Job sim2real-probe-iot-iot-errjob-b6a821f9 already exists.', got '... already exists'; $.resourceArn expected absent, got 'arn:aws:iot:us-west-2:000000000000:job/sim2real-probe-iot-iot-errjob-b6a821f9'; $.resourceId expected absent, got the jobId.
5. CreatePackage, iot-package-duplicate-and-notfound-errors#task#3 — expected 'Conditional check failure: Received duplicate request for package sim2real-probe-iot-pkg-err-080fa251', got 'Package sim2real-probe-iot-pkg-err-080fa251 already exists'; $.resourceId expected absent, got the package name. Cluster of 2.
6. CreateThing, iot-thing-and-group-lifecycle#task#4 — expected 'Thing sim2real-probe-iot-thing-14e49689 already exists in account with different attributes', got 'Thing sim2real-probe-iot-thing-14e49689 already exists with a different configuration'; resourceArn/resourceId expected absent, both populated. Cluster of 2.
7. CreateThingGroup, iot-thing-group-duplicate-and-idempotent-delete#task#2 — expected 'Thing Group sim2real-probe-iot-iot-grp-9b258396 already exists in current account with different properties', got 'Thing group ... already exists with a different configuration'; resourceArn/resourceId expected absent, both populated. Cluster of 3.
8. CreateThingType, iot-thing-type-and-thing-lifecycle#task#2 — expected 'Thing Type sim2real-probe-iot-thingtype-1cc9c9fc already exists in current account with different properties', got 'Thing type ... already exists with a different configuration'; resourceArn/resourceId expected absent, both populated.
9. CreateTopicRule, iot-topic-rule-duplicate-create-error#task#2 — expected "Topic Rule 'sim2real_probe_iot_topicruledup_ff7d36ec' already exists for account", got 'Rule with given name already exists (name=sim2real_probe_iot_topicruledup_ff7d36ec)'; resourceArn/resourceId expected absent, both populated. Cluster of 3.

(The names and ARNs above are this recording's values; the requirement is the fixed text plus the request-derived name, and the absence of resourceId/resourceArn.)

## Steps to Reproduce
1. git clone the repo and boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Observe the nine 409 clusters listed under Evidence, starting with fixture iot-authorizer-duplicate-and-notfound-errors step task#2.
4. Equivalently by hand: create each resource (authorizer, policy, command, job, package, thing with attributes, thing group with properties, thing type with properties, topic rule), then issue the same create again with different content, and compare the 409 body's message text and the presence of resourceId/resourceArn against the AWS wording given per cause.
