Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-subnet-group-owned-vpc-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-subnet-group-owned-vpc-lifecycle rds-subnet-group-az-coverage-and-naming
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateDBSubnetGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateDBSubnetGroup|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Commit: 7f0eb2b5e815b9fd74c096d9738faf43a70bc9fb

# RDS CreateDBSubnetGroup subnet validation ignores EC2 subnet state

> _Note: this issue was identified against the code base at commit `7f0eb2b5e815b9fd74c096d9738faf43a70bc9fb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBSubnetGroup

## Description
CreateDBSubnetGroup never resolves the subnet ids a request names against EC2 subnet state. It checks only the ids' lexical form and how many DISTINCT ids the list holds, and it derives each subnet's Availability Zone by hashing the id. Two consequences diverge from RDS on the wire: the AZ-coverage refusal names a zone the named subnet does not lie in, and a subnet that does not exist at all is answered with the AZ-coverage fault instead of RDS's invalid-subnet refusal. 2 causes; each must be closed and re-replayed separately.

Cause 1 of 2: the DBSubnetGroupDoesNotCoverEnoughAZs message reports a synthesized Availability Zone instead of the zone the named subnet actually lies in. Fixture rds-db-subnet-group-owned-vpc-lifecycle, step task#6 (also rds-subnet-group-az-coverage-and-naming#task#14 and #task#15).
AWS: Smithy com.amazonaws.rds#CreateDBSubnetGroup models DBSubnetGroupDoesNotCoverEnoughAZs (errorKind client, httpError 400, awsQueryError code DBSubnetGroupDoesNotCoverEnoughAZs); the operation's doc states "DB subnet groups must contain at least one subnet in at least two AZs in the Amazon Web Services Region", and com.amazonaws.rds#Subnet models SubnetIdentifier with SubnetAvailabilityZone, i.e. a subnet's zone is a property of that EC2 subnet. Recorded behavior: HTTP 400, Code DBSubnetGroupDoesNotCoverEnoughAZs, Message "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: <az-list>. Add subnets to cover at least 2 AZs." The fixed text is that sentence exactly; the single variable part is <az-list>, the comma-separated list of the Availability Zones the subnets NAMED IN THE REQUEST actually occupy, read from the subnets' own zone in state.
Emulator: answers 400 with the correct code and the correct fixed sentence, but the AZ list is a value it invents: services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs subnet_availability_zone() computes the zone as region + AVAILABILITY_ZONE_SUFFIXES[sha256(subnet_id)[0] % 3], and check_db_subnet_group_subnets() builds the coverage list from that function. The named subnet's recorded zone is never consulted, so the zone reported is whatever the id's hash lands on. In replay the step's subnet lies in the region's first zone and the emulator reported the second (expected "Current AZ coverage: us-west-2a. ...", got "Current AZ coverage: us-west-2b. ..."); the zone letters are this corpus's values, the requirement is that the list equals the zones of the request's subnets.
Why: the AZ-coverage list is the only thing in the refusal that tells a client WHICH zones it already covers, so a client (or a retry loop that adds a subnet in a zone it is told it is missing) is steered to the wrong zone and keeps getting refused; cross-checking RDS against EC2 DescribeSubnets always disagrees, and the same wrong zone reaches Subnet$SubnetAvailabilityZone on reads of the group.
Fix: CreateDBSubnetGroup's subnet validation — crate::state::check_db_subnet_group_subnets and the subnet_availability_zone helper it calls must resolve each supplied subnet id through EC2 subnet state and take its Availability Zone from that subnet's record, instead of deriving a zone from the id.
Traces: rds-db-subnet-group-owned-vpc-lifecycle#task#6; rds-subnet-group-az-coverage-and-naming#task#14; rds-subnet-group-az-coverage-and-naming#task#15.

Cause 2 of 2: a SubnetIds list naming a subnet that does not exist is refused with the AZ-coverage fault instead of RDS's invalid-subnet refusal, because existence is never checked. Fixture rds-subnet-group-az-coverage-and-naming, step task#13.
AWS: CreateDBSubnetGroup resolves the SubnetIds (required member, com.amazonaws.rds#SubnetIdentifierList) against the account's EC2 subnets BEFORE it evaluates AZ coverage. Recorded behavior: HTTP 400, Code InvalidParameterValue (the query-protocol common client error; the operation's modeled list carries DBSubnetGroupAlreadyExistsFault, DBSubnetGroupDoesNotCoverEnoughAZs, DBSubnetGroupQuotaExceededFault, DBSubnetQuotaExceededFault and InvalidSubnet), Message "Some input subnets in :[<subnet-ids>] are invalid." — fixed text "Some input subnets in :[" ... "] are invalid.", the variable part being the comma-separated list of the request's subnet ids that do not resolve (in this recording the single well-formed-but-nonexistent id the request carried).
Emulator: answers 400 but with Code DBSubnetGroupDoesNotCoverEnoughAZs and Message "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: <hashed-az>. Add subnets to cover at least 2 AZs." (replay: got Code DBSubnetGroupDoesNotCoverEnoughAZs and "... Current AZ coverage: us-west-2c. ..."). check_db_subnet_group_subnets() only applies MAX_SUBNETS_PER_DB_SUBNET_GROUP, check_subnet_id() (the subnet- + 8/17 hex-digit form) and a DISTINCT-id count, so a well-formed id for a subnet nobody created falls through to the coverage branch.
Why: the error code is the contract a client branches on. A client told DBSubnetGroupDoesNotCoverEnoughAZs retries by adding more subnets, which can never succeed, instead of correcting a bad subnet id; and because the two faults carry different codes, negative tests and error-handling paths that pass against AWS fail here. The wrong class also masks the real condition in the message.
Fix: CreateDBSubnetGroup's subnet validation (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_subnet_group.rs and crate::state::check_db_subnet_group_subnets / SubnetListRejection): add an existence resolution step against EC2 subnet state that runs before the AZ-coverage branch, and refuse an unresolvable id with InvalidParameterValue carrying the message template above.
Traces: rds-subnet-group-az-coverage-and-naming#task#13.

Related open issues that do NOT carry either replay key: 20261029T214632Z-rds-db-subnet-group-az-coverage-rule-is.md (its causes are task#8/#11/#12 of the same fixture — create not enforcing coverage at all, and ModifyDBSubnetGroup wording); 20261102T002746Z-rds-describeenginedefaultclusterparameters-and-createdbsubnetgroup-synthesize-data-the-request.md and 20261102T055331Z-rds-2-net-new-root-causes-modifydbclusterparametergroup-accepts.md (the synthesized VpcId / SubnetAvailabilityZone on the SUCCESS path). Neither covers the refusal paths above.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout (emulator_up true, 413/977 steps passed). Both clusters below report stub_path "" and owned_by [] — neither is blocked by an unimplemented operation nor owned by an open issue.

Cause 1 — cluster service=rds|CreateDBSubnetGroup|status=400->400|keys=$.ErrorResponse.Error.Message, representative rds-db-subnet-group-owned-vpc-lifecycle#task#6, affected_count 3 (also rds-subnet-group-az-coverage-and-naming#task#14, rds-subnet-group-az-coverage-and-naming#task#15):
  expected_status 400, actual_status 400.
  $.ErrorResponse.Error.Message: value_mismatch
    expected "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: us-west-2a. Add subnets to cover at least 2 AZs."
    got      "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: us-west-2b. Add subnets to cover at least 2 AZs."
  Code and HTTP status agree; only the AZ-coverage list diverges. The zone letters are this corpus's values — the requirement is that the list be the zones of the subnets the request names.
  Source: services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs — subnet_availability_zone(region, subnet_id) returns format!("{region}{}", AVAILABILITY_ZONE_SUFFIXES[sha256(subnet_id)[0] % 3]), and check_db_subnet_group_subnets() builds the coverage BTreeSet from it.

Cause 2 — cluster service=rds|CreateDBSubnetGroup|status=400->400|keys=$.ErrorResponse.Error.Code,$.ErrorResponse.Error.Message, representative rds-subnet-group-az-coverage-and-naming#task#13, affected_count 1:
  expected_status 400, actual_status 400.
  $.ErrorResponse.Error.Code: value_mismatch — expected 'InvalidParameterValue', got 'DBSubnetGroupDoesNotCoverEnoughAZs'.
  $.ErrorResponse.Error.Message: value_mismatch — expected 'Some input subnets in :[subnet-0123456789abcdef0] are invalid.', got "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: us-west-2c. Add subnets to cover at least 2 AZs."
  The subnet id in the expected message is the id the request carried; the template is "Some input subnets in :[<unresolvable ids>] are invalid."
  Source: create_db_subnet_group.rs maps only SubnetListRejection::{TooManySubnets, NotASubnet, NotEnoughAvailabilityZones}; check_db_subnet_group_subnets() performs no existence lookup at all, so an unknown-but-well-formed subnet id reaches the coverage branch.

Model used: smithy_query(query="operation", operation="CreateDBSubnetGroup", model_path="services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json") — input {DBSubnetGroupName, DBSubnetGroupDescription, SubnetIds required}; errors DBSubnetGroupAlreadyExistsFault/DBSubnetGroupDoesNotCoverEnoughAZs/DBSubnetGroupQuotaExceededFault/DBSubnetQuotaExceededFault/InvalidSubnet, all httpError 400 with awsQueryError codes.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1 — re-replay rds-db-subnet-group-owned-vpc-lifecycle#task#6 (and rds-subnet-group-az-coverage-and-naming#task#14, #task#15). By hand: create a VPC and ONE subnet through EC2 in a known Availability Zone, then aws rds create-db-subnet-group --db-subnet-group-name <name> --db-subnet-group-description d --subnet-ids <that-subnet>. Observe HTTP 400 DBSubnetGroupDoesNotCoverEnoughAZs whose "Current AZ coverage:" names a zone other than the one the subnet was created in.
4. Cause 2 — re-replay rds-subnet-group-az-coverage-and-naming#task#13. By hand: aws rds create-db-subnet-group --db-subnet-group-name <name> --db-subnet-group-description d --subnet-ids subnet-0123456789abcdef0 (a well-formed id for a subnet that was never created). The emulator answers 400 DBSubnetGroupDoesNotCoverEnoughAZs with the AZ-coverage sentence; AWS answers 400 InvalidParameterValue "Some input subnets in :[subnet-0123456789abcdef0] are invalid."
5. Compare $.ErrorResponse.Error.Code and $.ErrorResponse.Error.Message against the recorded responses in each step.
