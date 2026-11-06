Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-cluster-create-validation-errors
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-cluster-create-validation-errors docdb-cluster-lifecycle docdb-cluster-parameter-group-lifecycle docdb-db-subnet-group-lifecycle docdb-query-envelope-shared-with-rds
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb|CreateDBCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: docdb|CreateDBCluster|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: docdb|ModifyDBClusterParameterGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: docdb|ResetDBClusterParameterGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: docdb|CreateDBClusterParameterGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: docdb|CreateDBSubnetGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: docdb|DescribeEventCategories|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: b357a994596866b4c3c5b5df5566beb9dd5436ae

# DocDB error responses use emulator-authored messages instead of AWS wording

> _Note: this issue was identified against the code base at commit `b357a994596866b4c3c5b5df5566beb9dd5436ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
This issue bundles EIGHT causes with one root fault: docdb's refusal paths build their own ErrorResponse.Error.Message prose (and in one case the wrong Error.Code) instead of AWS's message templates for the error class. An HTTP status, an error code and an error message are all requirements, so each of these is a defect even where the status matched. A fixer must close ALL EIGHT causes below before submitting.

Cause 1 of 8: CreateDBCluster with an unknown DBSubnetGroupName - wrong message wording.
AWS: Smithy binds DBSubnetGroupNotFoundFault to CreateDBCluster (errorKind client, httpError 404, awsQueryError code DBSubnetGroupNotFoundFault). AWS's message is the fixed text "DB subnet group '" + the requested DBSubnetGroupName (variable, from the request) + "' does not exist."
Emulator: returns the right 404 but the message "DB Subnet group '<name>' not found." - 'Subnet' miscapitalised and "not found." instead of "does not exist."
Why: clients and test harnesses that match on the documented message text (or render it to operators) see wording the service never produces.
Fix: the docdb CreateDBCluster handler's subnet-group lookup must format DBSubnetGroupNotFoundFault with AWS's template.

Cause 2 of 8: CreateDBCluster with a malformed DBClusterIdentifier - wrong message wording.
AWS: an identifier that does not match the service's identifier rule is refused with HTTP 400 and the fixed message "The parameter DBClusterIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens." The only variable part is the member name (DBClusterIdentifier); the offending value is NOT echoed.
Emulator: returns "DBClusterIdentifier must contain only letters, numbers, or hyphens; got '<value>'" - different wording, omits the begin-with-a-letter / no-trailing-hyphen / no-double-hyphen rules, and echoes the rejected value.
Why: the emulator's text states a weaker rule than the service enforces, so a client that surfaces it tells the user a value is acceptable when AWS will reject it.
Fix: the docdb CreateDBCluster identifier validation must emit AWS's fixed identifier-rule message, parameterised only by the member name.

Cause 3 of 8: Multi-AZ CreateDBCluster against a subnet group with too few AZs - wrong Error.Code AND wrong message.
AWS: Smithy binds InvalidVPCNetworkStateFault to CreateDBCluster (client, httpError 400, awsQueryError code InvalidVPCNetworkStateFault). AWS refuses this request with that code and the message whose fixed text is "Can't create a Multi-AZ DB cluster because there aren't enough subnets available. At least three subnets with available IP addresses in different Availability Zones are required. You can add subnets to a DB subnet group in the following Availability Zones: " followed by the variable part - the comma-separated names of the Region's Availability Zones not yet covered by the named subnet group, taken from EC2/subnet-group state - and a terminating ".".
Emulator: returns HTTP 400 with Error.Code 'InvalidParameterCombination' and the message 'AllocatedStorage, DBClusterInstanceClass, Iops, StorageType are required to create a Multi-AZ DB cluster (engine mysql)' - the wrong modeled error for the condition, and prose naming RDS-only members and the mysql engine in a docdb response.
Why: this is a code-level divergence, not just wording: a client branching on InvalidVPCNetworkStateFault to repair the subnet group's AZ coverage never fires, and is instead told to supply members docdb does not accept.
Fix: the docdb CreateDBCluster Multi-AZ precondition must evaluate the named subnet group's AZ coverage and raise the modeled InvalidVPCNetworkStateFault (400) with AWS's template; the mysql/RDS-specific InvalidParameterCombination branch must not be reachable from docdb.

Cause 4 of 8: ModifyDBClusterParameterGroup with a value outside AllowedValues - wrong message wording.
AWS: an out-of-range parameter value is refused with HTTP 400 and a message of the form fixed "Invalid parameter value: " + the rejected value (from the request) + fixed " for: " + the parameter name (from the request) + fixed " allowed values are: " + the parameter's AllowedValues CSV (from the parameter catalog, in the catalog's order).
Emulator: returns 'Value: <value> is outside of range: <allowed values> for parameter: <name>' - a different template, and the allowed-value list is in the emulator's own order (see the catalog issue's tls AllowedValues cause).
Why: an operator or client parsing the refusal cannot extract the parameter name/allowed set from the expected positions, and the quoted allowed list disagrees with AWS.
Fix: the docdb ModifyDBClusterParameterGroup parameter validation must use AWS's template and quote the catalog's AllowedValues string verbatim.

Cause 5 of 8: ResetDBClusterParameterGroup while a parameter has pending changes - wrong message wording.
AWS: the reset is refused with HTTP 400 and the message fixed "Cannot reset the parameter group because the parameter '" + the pending parameter's name (variable, from state) + fixed "' has pending changes."
Emulator: returns 'DB cluster parameter group <group name> has pending changes to <parameter> that require a reboot before it can be reset' - different wording and a different subject (the group rather than the parameter).
Why: the condition a client must clear is the pending parameter; the emulator's prose invents a reboot requirement AWS does not state in this refusal.
Fix: the docdb ResetDBClusterParameterGroup pending-changes check must emit AWS's template, parameterised by the pending parameter name.

Cause 6 of 8: CreateDBClusterParameterGroup with an unknown DBParameterGroupFamily - wrong message wording.
AWS: refused with HTTP 400 and the message fixed "ParameterGroupFamily " + the requested family (variable, from the request) + fixed " is not a valid parameter group family" (no trailing period, no enumeration of valid families).
Emulator: returns "Invalid value '<family>' for DBParameterGroupFamily: the Amazon DocumentDB families are docdb3.6, docdb4.0, docdb5.0" - different template and an invented enumeration.
Why: a client matching the documented wording does not recognise the refusal, and the invented family list becomes a second source of truth that drifts from the engine-version catalog.
Fix: the docdb CreateDBClusterParameterGroup family validation must emit AWS's template.

Cause 7 of 8: CreateDBSubnetGroup for a name that already exists - wrong message wording.
AWS: Smithy binds DBSubnetGroupAlreadyExistsFault (client, httpError 400, awsQueryError code DBSubnetGroupAlreadyExists). AWS's message is fixed "The DB subnet group '" + the requested DBSubnetGroupName (variable) + fixed "' already exists."
Emulator: returns 'DB subnet group already exists: <name>' - the id appended after a colon instead of AWS's quoted-name sentence.
Why: idempotency logic that distinguishes an already-exists refusal by its message text misreads it.
Fix: the docdb CreateDBSubnetGroup duplicate-name check must format DBSubnetGroupAlreadyExistsFault with AWS's template.

Cause 8 of 8: DescribeEventCategories with an invalid SourceType - wrong message wording.
AWS: refused with HTTP 400 and a FIXED message that does not echo the bad value: "Invalid event source type. Feature may be disabled. Valid types are " followed by the service's full quoted valid-source-type list ('db-instance', 'db-security-group', 'db-parameter-group', 'db-snapshot', 'db-cluster', 'db-cluster-snapshot', 'custom-engine-version', 'db-proxy', 'blue-green-deployment', 'db-shard-group', 'zero-etl', 'data-group', 'db-cluster-group', 'integration-s3tables', 'branch', 'db-cluster-group-snapshot').
Emulator: returns 'Invalid event source type: <bad value>. Valid values are: db-instance, db-cluster, db-parameter-group, db-security-group, db-snapshot, db-cluster-snapshot, db-proxy' - echoes the rejected value, drops the "Feature may be disabled." sentence, uses unquoted names in a different order and omits nine of the valid types.
Why: a client that discovers valid source types from the refusal is told a subset, so legitimate subscriptions (custom-engine-version, blue-green-deployment, zero-etl, ...) look unsupported.
Fix: the docdb DescribeEventCategories SourceType validation must emit AWS's fixed message with the full quoted valid-type list (shared with the seeded event-category table).

Traces: docdb-cluster-create-validation-errors#task#6 (Cause 1), docdb-cluster-create-validation-errors#task#9 and docdb-cluster-lifecycle#task#11 (Cause 2), docdb-cluster-create-validation-errors#task#10 (Cause 3), docdb-cluster-parameter-group-lifecycle#task#7 and #task#8 (Cause 4), docdb-cluster-parameter-group-lifecycle#task#9 (Cause 5), docdb-cluster-parameter-group-lifecycle#task#10 (Cause 6), docdb-db-subnet-group-lifecycle#task#7 and #task#8 (Cause 7), docdb-query-envelope-shared-with-rds#task#6 (Cause 8).

## Evidence
Cause 1 - fixture docdb-cluster-create-validation-errors step task#6, CreateDBCluster, expected 404 / actual 404: '$.ErrorResponse.Error.Message: value_mismatch (expected "DB subnet group \'<name>\' does not exist.", got "DB Subnet group \'<name>\' not found.")'. Fixed text: "DB subnet group '" ... "' does not exist."; variable: the requested DBSubnetGroupName.
Cause 2 - fixture docdb-cluster-create-validation-errors step task#9 (also docdb-cluster-lifecycle#task#11), CreateDBCluster, 400/400: expected 'The parameter DBClusterIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.', got "DBClusterIdentifier must contain only letters, numbers, or hyphens; got '<rejected value>'". The whole expected sentence is fixed text; the rejected value must not appear.
Cause 3 - fixture docdb-cluster-create-validation-errors step task#10, CreateDBCluster, 400/400: '$.ErrorResponse.Error.Code: value_mismatch (expected InvalidVPCNetworkStateFault, got InvalidParameterCombination)' and Message expected "Can't create a Multi-AZ DB cluster because there aren't enough subnets available. At least three subnets with available IP addresses in different Availability Zones are required. You can add subnets to a DB subnet group in the following Availability Zones: <uncovered AZ names>.", got 'AllocatedStorage, DBClusterInstanceClass, Iops, StorageType are required to create a Multi-AZ DB cluster (engine mysql)'. The AZ names are values from state; the surrounding sentence is fixed.
Cause 4 - fixture docdb-cluster-parameter-group-lifecycle step task#7 (also #task#8), ModifyDBClusterParameterGroup, 400/400: expected 'Invalid parameter value: <value> for: <parameter> allowed values are: <AllowedValues CSV>', got 'Value: <value> is outside of range: <list> for parameter: <parameter>'.
Cause 5 - fixture docdb-cluster-parameter-group-lifecycle step task#9, ResetDBClusterParameterGroup, 400/400: expected "Cannot reset the parameter group because the parameter '<name>' has pending changes.", got 'DB cluster parameter group <group> has pending changes to <name> that require a reboot before it can be reset'.
Cause 6 - fixture docdb-cluster-parameter-group-lifecycle step task#10, CreateDBClusterParameterGroup, 400/400: expected 'ParameterGroupFamily <family> is not a valid parameter group family', got "Invalid value '<family>' for DBParameterGroupFamily: the Amazon DocumentDB families are docdb3.6, docdb4.0, docdb5.0".
Cause 7 - fixture docdb-db-subnet-group-lifecycle step task#7 (also #task#8), CreateDBSubnetGroup, 400/400: expected "The DB subnet group '<name>' already exists.", got 'DB subnet group already exists: <name>'.
Cause 8 - fixture docdb-query-envelope-shared-with-rds step task#6, DescribeEventCategories, 400/400: expected "Invalid event source type. Feature may be disabled. Valid types are 'db-instance', 'db-security-group', 'db-parameter-group', 'db-snapshot', 'db-cluster', 'db-cluster-snapshot', 'custom-engine-version', 'db-proxy', 'blue-green-deployment', 'db-shard-group', 'zero-etl', 'data-group', 'db-cluster-group', 'integration-s3tables', 'branch', 'db-cluster-group-snapshot'", got 'Invalid event source type: <bad value>. Valid values are: db-instance, db-cluster, db-parameter-group, db-security-group, db-snapshot, db-cluster-snapshot, db-proxy'. The names/values quoted above as <...> are per-request values; everything else is fixed text.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Cause 1: fixture docdb-cluster-create-validation-errors step task#6 - Action=CreateDBCluster with DBSubnetGroupName set to a name that does not exist. Observe 404 with message "DB Subnet group '<name>' not found." instead of "DB subnet group '<name>' does not exist."
3. Cause 2: same fixture step task#9 (and docdb-cluster-lifecycle step task#11) - Action=CreateDBCluster with DBClusterIdentifier containing underscores/uppercase. Observe the emulator's weaker rule text echoing the value.
4. Cause 3: same fixture step task#10 - Action=CreateDBCluster requesting Multi-AZ against a subnet group covering too few AZs. Observe Error.Code InvalidParameterCombination and mysql/RDS-member prose instead of InvalidVPCNetworkStateFault with the AZ-coverage message.
5. Cause 4: fixture docdb-cluster-parameter-group-lifecycle step task#7 - Action=ModifyDBClusterParameterGroup setting tls to a value outside AllowedValues.
6. Cause 5: same fixture step task#9 - Action=ResetDBClusterParameterGroup while tls has pending changes.
7. Cause 6: same fixture step task#10 - Action=CreateDBClusterParameterGroup with DBParameterGroupFamily=<unknown family>.
8. Cause 7: fixture docdb-db-subnet-group-lifecycle step task#7 - Action=CreateDBSubnetGroup reusing an existing DBSubnetGroupName.
9. Cause 8: fixture docdb-query-envelope-shared-with-rds step task#6 - Action=DescribeEventCategories&SourceType=<invalid value>.
10. In each case compare ErrorResponse.Error.Code and Error.Message against the AWS templates quoted in the description (fixed text exactly; variable parts from the request/state).
