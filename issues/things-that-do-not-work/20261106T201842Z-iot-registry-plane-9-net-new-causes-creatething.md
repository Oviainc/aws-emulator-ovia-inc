Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.create-thing-conflicting-attributes-fails iot.create-thing-conflicting-type-fails iot.op-create-thing-invalid-name-rejected iot.op-create-thing-unknown-thing-type-rejected iot.op-delete-thing-stale-expected-version-rejected iot.thing-type-deprecated-blocks-new-thing iot.thing-type-duplicate-name-idempotent iot.op-delete-non-deprecated-thing-type-rejected iot.thing-type-premature-deletion-rejected iot.thing-group-hierarchy-depth-limit-rejected iot.thing-group-hierarchy
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.create-thing-conflicting-attributes-fails|run#7|AssertionError|assert err == ("ResourceAlreadyExistsException", f"Thing {name} already exists in account with different attributes"), err
Issue-Key: iot.create-thing-conflicting-type-fails|run#9|AssertionError|assert err == ("ResourceAlreadyExistsException", f"Thing {name} already exists in account with different attributes"), err
Issue-Key: iot.op-create-thing-invalid-name-rejected|run#9|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code}"
Issue-Key: iot.op-create-thing-unknown-thing-type-rejected|run#15|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iot.op-delete-thing-stale-expected-version-rejected|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iot.thing-type-deprecated-blocks-new-thing|run#14|AssertionError|assert message == f"Can not create new thing with depreated thing type:{type_name}", message
Issue-Key: iot.thing-type-duplicate-name-idempotent|run#10|AssertionError|assert message == f"Thing Type {type_name} already exists in current account with different properties", message
Issue-Key: iot.op-delete-non-deprecated-thing-type-rejected|run#8|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code}"
Issue-Key: iot.thing-type-premature-deletion-rejected|run#7|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code}"
Issue-Key: iot.thing-group-hierarchy-depth-limit-rejected|_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: iot.thing-group-hierarchy|_eventually#11|Timeout|time.sleep(interval_s)
Issue-Commit: 9ad1a007c88406c445bb3f2d55ab006d23610bba

# iot registry plane: 9 net-new causes — CreateThing/CreateThingType/DeleteThing refusals use emulator-authored wording and the wrong error class, DeleteThingType is a stub, and the thing-group hierarchy is neither listed nor depth-capped

> _Note: this issue was identified against the code base at commit `9ad1a007c88406c445bb3f2d55ab006d23610bba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateThing

## Description
Nine net-new root causes in the IoT registry (things, thing types, thing groups). All were found by the behavioral suite under the `iot` service heading. Each cause below is independent and must be closed before submitting.

Cause 1 of 9: CreateThing name-conflict refusal message.
AWS: Smithy CreateThing models ResourceAlreadyExistsException (409). Recorded behavior: re-creating an existing thing with different attributes (or a different thingTypeName) refuses ResourceAlreadyExistsException with the fixed text `Thing <thingName> already exists in account with different attributes`; the only variable part is the thing name from the request.
Emulator: same code, but the message is `Thing <thingName> already exists with a different configuration`.
Why: a client that matches on AWS's wording (or logs/parses it) sees a message AWS never emits, and the conflict reason ("different attributes") is lost.
Fix: CreateThing's handler/refusal wording (services/service-com-amazonaws-iot-awsiotservice/src/ops/create_thing.rs).
Traces: iot.create-thing-conflicting-attributes-fails, iot.create-thing-conflicting-type-fails.

Cause 2 of 9: CreateThing with an invalid thingName answers the wrong error class.
AWS: Smithy CreateThing.thingName is @pattern `^[a-zA-Z0-9:_-]+$`, and the operation models InvalidRequestException (400), ResourceAlreadyExistsException, ResourceNotFoundException, Throttling/Unauthorized/InternalFailure/ServiceUnavailable — it does NOT model ValidationException at all. The refusal is InvalidRequestException with the fixed text `1 validation error detected: Value '<thingName>' at 'thingName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9:_-]+`, where <thingName> is the rejected value from the request.
Emulator: refuses with error code ValidationException — a code this operation does not model — so a client catching iot.exceptions.InvalidRequestException never sees the rejection.
Why: botocore raises an unmodelled error class; error-class-based handling and retry/branching logic break.
Fix: CreateThing's input validation and the error-class mapping it uses (create_thing.rs plus the shared constraint-violation mapping for this service).
Traces: iot.op-create-thing-invalid-name-rejected.

Cause 3 of 9: CreateThing with an unknown thingTypeName — message wording.
AWS: ResourceNotFoundException with the fixed text `Thing Type <account>:<thingTypeName> not found.` (account id and thing-type name are request/state values).
Emulator: `Thing type <thingTypeName> does not exist` — different fixed text, no account qualifier, no trailing period.
Why: the message no longer identifies the account the lookup was scoped to, and differs from AWS's wording.
Fix: CreateThing's thing-type resolution refusal (create_thing.rs).
Traces: iot.op-create-thing-unknown-thing-type-rejected.

Cause 4 of 9: DeleteThing stale expectedVersion — message wording.
AWS: Smithy DeleteThing models VersionConflictException (409) and takes expectedVersion as a query parameter. The refusal message is the fixed sentence `The version for thing <thingName> does not match the expected version.`
Emulator: `expectedVersion 1 does not match the version 2 of thing <thingName>` — emulator-authored prose that also leaks both version numbers, which AWS's message does not contain.
Why: wording clients assert on is wrong; the message template is not AWS's.
Fix: DeleteThing's optimistic-concurrency refusal (delete_thing.rs).
Traces: iot.op-delete-thing-stale-expected-version-rejected.

Cause 5 of 9: CreateThing against a DEPRECATED thing type — message wording.
AWS: InvalidRequestException with the fixed text `Can not create new thing with depreated thing type:<thingTypeName>` — note AWS's own misspelling "depreated", which is part of the fixed text, with only the thing-type name taken from the request.
Emulator: `Can not create new thing with deprecated thing type:<thingTypeName>` (spelling "corrected"), so the string does not match AWS.
Why: exact-match assertions and log scrapers keyed on AWS's literal message fail.
Fix: the deprecated-thing-type guard's message in CreateThing (create_thing.rs).
Traces: iot.thing-type-deprecated-blocks-new-thing.

Cause 6 of 9: CreateThingType conflicting-properties refusal message.
AWS: Smithy CreateThingType models ResourceAlreadyExistsException (409). An identical re-create is idempotent; a re-create of the same name with DIFFERENT thingTypeProperties/tags refuses ResourceAlreadyExistsException with the fixed text `Thing Type <thingTypeName> already exists in current account with different properties`.
Emulator: `Thing type <thingTypeName> already exists with a different configuration`.
Why: the message is not AWS's, and it describes the conflict in terms of "configuration" rather than the "properties" AWS names.
Fix: CreateThingType's conflict refusal (create_thing_type.rs).
Traces: iot.thing-type-duplicate-name-idempotent.

Cause 7 of 9: DeleteThingType is an unimplemented scaffold stub, so both of its state-machine guards are unobservable.
AWS: Smithy DeleteThingType models InvalidRequestException (400). Recorded behavior: deleting a thing type that was never deprecated refuses InvalidRequestException `Cannot delete non deprecated thing type <thingTypeName>`; deleting one inside the 5-minute post-deprecation window refuses InvalidRequestException `Cannot delete thing type : <thingTypeName>. Please wait for 5 minutes after deprecation and then retry`. In both cases the thing type survives, still (un)deprecated.
Emulator: every DeleteThingType call answers error code `NotImplemented` ("operation DeleteThingType is scaffolded but not implemented in this emulator").
Why: a thing type can never be deleted, so teardown and the whole deprecate-then-delete lifecycle are impossible, and the two refusals above can never be observed.
Fix: implement DeleteThingType and its two guards (services/service-com-amazonaws-iot-awsiotservice/src/ops/delete_thing_type.rs plus thing-type state).
Traces: iot.op-delete-non-deprecated-thing-type-rejected, iot.thing-type-premature-deletion-rejected.

Cause 8 of 9: CreateThingGroup does not enforce the 7-level hierarchy depth cap.
AWS: Smithy CreateThingGroup models InvalidRequestException (400). A thing-group hierarchy is capped at 7 levels: creating an 8th nested group refuses InvalidRequestException with the fixed text `Cannot create child group for parent group : <parentGroupName>. Depth of group hierarchy exceeds allowed max depth :7`, and the over-deep group is never created (DescribeThingGroup then refuses ResourceNotFoundException `ThingGroup <account>:<thingGroupName> not found.`).
Emulator: the 8th-level CreateThingGroup is accepted with HTTP 200 and returns a thingGroupName/thingGroupArn/thingGroupId.
Why: the emulator is over-permissive — code that is supposed to fail on an over-deep hierarchy succeeds here and then diverges on real AWS.
Fix: CreateThingGroup's parent-chain depth validation (create_thing_group.rs plus thing-group state).
Traces: iot.thing-group-hierarchy-depth-limit-rejected.

Cause 9 of 9: ListThingGroups(parentGroup=…) never lists a group's children.
AWS: Smithy ListThingGroups takes `parentGroup` as an httpQuery filter and returns the groups whose parent is that group in `thingGroups` (name + ARN).
Emulator: after CreateThingGroup(child, parentGroupName=parent) succeeds and DescribeThingGroup(child) reports the parent, a 60s poll of ListThingGroups(parentGroup=parent) never contains the child, so the check times out ("Check timed out after 60.0s").
Why: the parent/child relation is stored but not queryable, so fleet tooling that enumerates a hierarchy top-down sees an empty tree.
Fix: the parentGroup filter in ListThingGroups and the child index in thing-group state (list_thing_groups.rs).
Traces: iot.thing-group-hierarchy.

## Evidence
Expected vs actual, per cause (all from the replayed suite output):
1. CreateThing conflict — expected ('ResourceAlreadyExistsException', "Thing <name> already exists in account with different attributes"); actual ('ResourceAlreadyExistsException', 'Thing <name> already exists with a different configuration').
2. CreateThing bad name — expected code InvalidRequestException with "1 validation error detected: Value '<name>' at 'thingName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9:_-]+"; actual code ValidationException (AssertionError: expected InvalidRequestException, got ValidationException).
3. CreateThing unknown type — expected 'Thing Type <account>:<name> not found.'; actual 'Thing type <name> does not exist'.
4. DeleteThing stale version — expected 'The version for thing <name> does not match the expected version.'; actual 'expectedVersion 1 does not match the version 2 of thing <name>'.
5. Deprecated type — expected 'Can not create new thing with depreated thing type:<type>'; actual 'Can not create new thing with deprecated thing type:<type>'.
6. CreateThingType conflict — expected 'Thing Type <type> already exists in current account with different properties'; actual 'Thing type <type> already exists with a different configuration'.
7. DeleteThingType — expected InvalidRequestException ('Cannot delete non deprecated thing type <type>' / 'Cannot delete thing type : <type>. Please wait for 5 minutes after deprecation and then retry'); actual code NotImplemented.
8. 8th-level CreateThingGroup — expected InvalidRequestException 'Cannot create child group for parent group : <parent>. Depth of group hierarchy exceeds allowed max depth :7'; actual HTTP 200 with {'thingGroupName': '<base>-L8', 'thingGroupArn': 'arn:aws:iot:<region>:<account>:thinggroup/<base>-L8', 'thingGroupId': '<uuid>'}.
9. ListThingGroups(parentGroup=parent) — expected the child group listed; actual never listed within 60s (check timed out).

## Steps to Reproduce
Replay the listed behavioral checks against the emulator (suite simparity-behavioral-aws-v1):
1. CreateThing(name, attributePayload={'attributes':{'model':'x1'}}) then CreateThing(name, attributePayload={'attributes':{'model':'different'}}) — inspect code+message.
2. CreateThing(thingName='bad name with spaces') — inspect error code.
3. CreateThing(thingName=n, thingTypeName=<missing>) — inspect message.
4. CreateThing(n); UpdateThing(n) to bump version; DeleteThing(n, expectedVersion=1) — inspect message.
5. CreateThingType(t); DeprecateThingType(t); CreateThing(n, thingTypeName=t) — inspect message.
6. CreateThingType(t, tags=X); CreateThingType(t, thingTypeProperties={'thingTypeDescription':'conflicting'}, tags=X) — inspect message.
7. CreateThingType(t); DeleteThingType(t) — observe NotImplemented instead of InvalidRequestException.
8. Create 7 nested thing groups, then an 8th under the deepest — observe HTTP 200.
9. CreateThingGroup(parent); CreateThingGroup(child, parentGroupName=parent); ListThingGroups(parentGroup=parent) — child never appears.
