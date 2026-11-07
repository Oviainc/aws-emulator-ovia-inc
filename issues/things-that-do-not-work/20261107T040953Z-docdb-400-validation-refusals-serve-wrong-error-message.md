Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-cluster-create-validation-errors
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-cluster-create-validation-errors docdb-cluster-lifecycle docdb-db-subnet-group-lifecycle
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb|CreateDBCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: docdb|DeleteDBCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: docdb|CreateDBSubnetGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 12bd58142e134c788a6e6e722018c3787d666c1f

# DocDB 400 validation refusals serve wrong Error.Message text and wrong AZ values

> _Note: this issue was identified against the code base at commit `12bd58142e134c788a6e6e722018c3787d666c1f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
This issue bundles THREE causes that share one fault: on docdb's awsQuery 400 refusals the HTTP status and Error.Code are right, but $.ErrorResponse.Error.Message is not what AWS words. In one case the sentence itself is invented; in the other two the sentence's fixed text is right but the variable part (the Availability-Zone list the message is supposed to carry) is built wrongly -- truncated to one entry, or derived from an unresolved subnet so it names the wrong zone. A fixer must close ALL THREE causes below before submitting.

Cause 1 of 3: CreateDBCluster's not-enough-subnets refusal lists only one of the Availability Zones a caller must add subnets in. Fixture docdb-cluster-create-validation-errors, step task#10.
AWS: Smithy (com.amazonaws.docdb#AmazonRDSv19) CreateDBCluster binds DBSubnetGroupDoesNotCoverEnoughAZs (errorKind client, awsQueryError code DBSubnetGroupDoesNotCoverEnoughAZs, httpResponseCode 400), documented 'Subnets in the subnet group should cover at least two Availability Zones unless there is only one Availability Zone.' The model does not state the message, so the wording is recorded behavior only: HTTP 400 with the fixed text "Can't create a Multi-AZ DB cluster because there aren't enough subnets available. At least three subnets with available IP addresses in different Availability Zones are required. You can add subnets to a DB subnet group in the following Availability Zones: " then the variable part, then ".". The variable part is a comma-space separated list of EVERY Availability Zone in the Region that the named DB subnet group does not already cover -- the recording's expected message carries two zone names (one literal, one held by the fixture as a placeholder), so the list is plural whenever more than one zone is uncovered.
Emulator: emits the same fixed text but the variable part holds only a single zone name, dropping the remaining uncovered zones.
Why: the message is the remediation instruction. A client (or operator) that reads it adds a subnet in the one zone named, retries, and is refused again, because the group still does not cover the minimum number of zones. The count of zones in the list is the only thing telling the caller how many subnets to add.
Fix: in CreateDBCluster's subnet-group AZ-coverage validation, build the message's variable part from the full set difference (Region AZs minus the resolved AZs of the subnet group's subnets), joined with ', ', instead of emitting only the first element.

Cause 2 of 3: DeleteDBCluster's missing-final-snapshot refusal uses invented prose instead of AWS's sentence. Fixture docdb-cluster-lifecycle, step task#12.
AWS: Smithy DeleteDBClusterMessage models SkipFinalSnapshot (boolean) and FinalDBSnapshotIdentifier (String), the latter documented as 'The cluster snapshot identifier of the new cluster snapshot created when SkipFinalSnapshot is set to false', so a delete that sets neither is a parameter-combination error. The model does not state the message text; recorded behavior is HTTP 400 whose Error.Message is entirely fixed text with NO variable part: "Please specify FinalDBSnapshotIdentifier or SkipFinalSnapshot" (no trailing period).
Emulator: answers HTTP 400 with Error.Message "FinalDBSnapshotIdentifier is required unless SkipFinalSnapshot is specified." -- a different sentence, differently punctuated.
Why: the message is part of the error contract. SDK retry/classification wrappers, operator runbooks and assertions that match AWS's sentence do not recognise the emulator's text, so a correctly-handled precondition looks like an unknown failure.
Fix: DeleteDBCluster's input validation in the docdb operation handler must emit AWS's fixed sentence verbatim for the neither-parameter-supplied case; it is a constant, so it belongs with the validation that raises the 400, not interpolated from member names.

Cause 3 of 3: CreateDBSubnetGroup's AZ-coverage refusal reports the wrong current-coverage zone, because the requested subnet ids are never resolved to their real Availability Zones. Fixture docdb-db-subnet-group-lifecycle, step task#8.
AWS: same modeled error DBSubnetGroupDoesNotCoverEnoughAZs (400) is bound to CreateDBSubnetGroup, whose input SubnetIds is a required SubnetIdentifierList. Recorded wording (recorded behavior only) is the fixed text "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: " + variable AZ list + ". Add subnets to cover at least " + variable minimum count + " AZs." The AZ list is the set of zones the REQUESTED subnets actually live in, resolved from the account/Region EC2 subnet state; for a request naming subnets that all sit in one zone it is that one zone's name.
Emulator: emits the right fixed text and the right minimum count but a zone name that is not the zone the requested subnet belongs to -- in this step the recorded coverage is the subnet's real zone while the emulator reports a different zone of the same Region. It is reporting a zone it assigned positionally rather than one it looked up, so the value is wrong for any subnet whose assigned slot differs from its real zone.
Why: this message is again the remediation instruction; it tells the caller which zones are already covered so they can add a subnet elsewhere. Pointed at the wrong zone, the caller adds a subnet in a zone that is in fact already covered and is refused again, and any client that asserts the group's current coverage from the refusal records the wrong topology.
Fix: CreateDBSubnetGroup's AZ-coverage validation must derive the message's AZ list from the per-subnet Availability Zone resolved out of the EC2 subnet store (the same resolution the DBSubnetGroup projection needs), not from an index- or hash-assigned zone.
Related open issue: '20261029T193448Z-docdb-subnet-groups-never-resolve-subnet-ids-against' reports the unresolved-subnet fault on the success projections (VpcId, per-subnet AZ) and the unknown-subnet refusal. It does not carry this step's replay key and says nothing about the AZ-coverage error message, so this cause is filed here; fixing the resolution store it describes is likely a prerequisite for this cause.

All three causes are message-only divergences: the HTTP status (400) and the awsQuery Error.Code matched the recording in every one.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 99 steps, 11 mismatched).

Cause 1 - fixture docdb-cluster-create-validation-errors, step task#10, CreateDBCluster, expected 400 / actual 400, single diverging key $.ErrorResponse.Error.Message: value_mismatch. Expected "Can't create a Multi-AZ DB cluster because there aren't enough subnets available. At least three subnets with available IP addresses in different Availability Zones are required. You can add subnets to a DB subnet group in the following Availability Zones: us-west-2c, {{az_4}}." Actual "Can't create a Multi-AZ DB cluster because there aren't enough subnets available. At least three subnets with available IP addresses in different Availability Zones are required. You can add subnets to a DB subnet group in the following Availability Zones: us-west-2c." Everything up to and including 'Availability Zones: ' and the final '.' is fixed text; the zone names between them are values derived from the Region's AZs and the subnet group's resolved coverage (the fixture's second entry is a placeholder, so the specific names are per-environment -- the requirement is that every uncovered zone appears, comma-space separated).

Cause 2 - fixture docdb-cluster-lifecycle, step task#12, DeleteDBCluster, expected 400 / actual 400, single diverging key $.ErrorResponse.Error.Message: value_mismatch. Expected 'Please specify FinalDBSnapshotIdentifier or SkipFinalSnapshot'. Actual 'FinalDBSnapshotIdentifier is required unless SkipFinalSnapshot is specified.' The whole expected string is fixed text; there is no variable part.

Cause 3 - fixture docdb-db-subnet-group-lifecycle, step task#8, CreateDBSubnetGroup, expected 400 / actual 400, single diverging key $.ErrorResponse.Error.Message: value_mismatch. Expected "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: us-west-2a. Add subnets to cover at least 2 AZs." Actual "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: us-west-2b. Add subnets to cover at least 2 AZs." Fixed text: "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: ", ". Add subnets to cover at least ", " AZs." Variable: the AZ list (the zones the requested subnets resolve to) and the minimum zone count. The same zone swap shows up on the success paths of this fixture (Subnets.Subnet[*].SubnetAvailabilityZone expected us-west-2a/us-west-2b, served us-west-2b/us-west-2a), which is why this is a resolution fault and not a per-run value difference.

Model references used: smithy_query operation CreateDBCluster / DeleteDBCluster / CreateDBSubnetGroup and shape DBSubnetGroupDoesNotCoverEnoughAZs, SubnetIdentifierList, plus doc DeleteDBClusterMessage.FinalDBSnapshotIdentifier, from services/service-com-amazonaws-docdb-amazonrdsv19/smithy-model/com-amazonaws-docdb-amazonrdsv19.json.

## Steps to Reproduce
1. Clone this checkout, boot the emulator from it, and replay the wire corpus (replay_wire_corpus on trace set simparity-wire-aws-v1).
2. Cause 1: fixture docdb-cluster-create-validation-errors, step task#10 - POST the awsQuery Action=CreateDBCluster naming a DB subnet group whose subnets do not cover enough Availability Zones for a Multi-AZ cluster. Observed: 400 whose Error.Message names a single zone after 'in the following Availability Zones: '. Expected: the same sentence listing every Region AZ the group does not cover, comma-space separated.
3. Cause 2: fixture docdb-cluster-lifecycle, step task#12 - POST Action=DeleteDBCluster with DBClusterIdentifier only, supplying neither SkipFinalSnapshot nor FinalDBSnapshotIdentifier. Observed: 400 Error.Message 'FinalDBSnapshotIdentifier is required unless SkipFinalSnapshot is specified.' Expected: 400 Error.Message 'Please specify FinalDBSnapshotIdentifier or SkipFinalSnapshot'.
4. Cause 3: fixture docdb-db-subnet-group-lifecycle, step task#8 - POST Action=CreateDBSubnetGroup with SubnetIds naming subnet(s) that all live in one Availability Zone. Observed: 400 DBSubnetGroupDoesNotCoverEnoughAZs whose 'Current AZ coverage:' names a zone other than the one the requested subnet actually belongs to. Expected: the zone(s) the requested subnets resolve to in EC2 state.
5. Re-replay all three fixtures/steps after the fix; all three must report Error.Message as expected while the 400 status and the Error.Code stay unchanged.
