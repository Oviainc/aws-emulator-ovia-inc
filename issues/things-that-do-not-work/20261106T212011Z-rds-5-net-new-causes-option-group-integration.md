Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.create-read-replica-cross-engine-family-rejected rds.custom-engine-version-manifest-round-trip rds.delete-integration-boundary rds.describe-db-recommendations rds.default-max-connections-follows-instance-class-memory-formula
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.create-read-replica-cross-engine-family-rejected|run#20|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: rds.custom-engine-version-manifest-round-trip|run#12|AssertionError|assert control_code == "Ec2ImagePropertiesNotSupportedFault", (
Issue-Key: rds.delete-integration-boundary|run#6|AssertionError|assert message.endswith(
Issue-Key: rds.describe-db-recommendations|run#9|AssertionError|assert err.get("Message") == (
Issue-Key: rds.default-max-connections-follows-instance-class-memory-formula|run#16|AssertionError|assert ceiling // 2 <= value <= ceiling, (
Issue-Commit: 14b50dd5b1a37831270f0fabc7b7cd82f4b7ecdf

# rds: 5 net-new causes — option-group/integration/recommendation refusals are emulator-worded, CreateCustomDBEngineVersion validates in the wrong order, and the default parameter group's max_connections formula has no engine effect

> _Note: this issue was identified against the code base at commit `14b50dd5b1a37831270f0fabc7b7cd82f4b7ecdf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCustomDBEngineVersion

## Description
FIVE net-new root causes in the rds service. Each must be closed before this issue is submitted. None is a regression: the operations involved answer, and where a fixed issue previously implemented one of them (20261029T195009Z-rds-describedbrecommendations-stub.md for DescribeDBRecommendations, 20261029T195009Z-rds-6-net-new-root-causes-copydbparametergroup-copydbsnapshot.md for DeleteIntegration) the operation's success path now works in the very same check — only the wording of a refusal AWS also makes diverges.

=== Cause 1 of 5: CreateDBInstanceReadReplica's cross-engine option-group refusal is worded by the emulator ===
AWS: CreateDBInstanceReadReplica with an OptionGroupName whose option group belongs to a different engine than the source instance is refused with InvalidParameterCombination (HTTP 400). The message's fixed text is "The option group <name> is for <option-group engine> <option-group major version>, and your DB instance is <source engine> <source engine major.minor version>." — the option group name, both engine names and both versions are values taken from the request and from the emulator's state; nothing else varies. Smithy: CreateDBInstanceReadReplica input OptionGroupName; the common query-protocol error InvalidParameterCombination (400).
Emulator: the code is correct (InvalidParameterCombination) but the message is the emulator's own sentence: "The option group <name> is an option group of engine postgres, which is not the source DB instance's engine mysql" — it omits both version numbers and uses a clause AWS never emits.
Why: clients and tests that match on RDS's documented sentence (and operators reading it) do not see the engine VERSIONS that tell them which option group would be valid, and message-matching automation breaks.
Fix: the CreateDBInstanceReadReplica handler's option-group validation and its refusal message template, in the rds service crate.
Traces: rds.create-read-replica-cross-engine-family-rejected.

=== Cause 2 of 5: CreateCustomDBEngineVersion checks the image-source combination before the image itself, for a recognized custom engine ===
AWS: for a RECOGNIZED custom engine (e.g. Engine='custom-sqlserver-ee'), CreateCustomDBEngineVersion carrying UseAwsProvidedLatestImage=true together with a named ImageId reaches AMI validation and is refused with Ec2ImagePropertiesNotSupportedFault (HTTP 400, awsQueryError code Ec2ImagePropertiesNotSupportedFault), message fixed text "Image <ImageId> doesn't exist in your account <account id>. Verify <account id> is the owner of the EC2 image." — ImageId comes from the request and the account id from the caller's identity. Smithy: CreateCustomDBEngineVersion errors include Ec2ImagePropertiesNotSupportedFault; input members UseAwsProvidedLatestImage and ImageId.
Emulator: the same call is refused earlier with InvalidParameterValue-class code 'InvalidParameterCombination' and the emulator's own sentence "UseAwsProvidedLatestImage and ImageId cannot both be specified: the service-provided latest AMI and a named AMI are two different images". The unrecognized-engine leg of the same check (Engine='custom-oracle-ee' -> InvalidParameterValue "Invalid DB engine") passes, so only the recognized-engine ordering diverges.
Why: a client gets a different error class for a different reason: it is told its parameters conflict when AWS tells it the AMI is not in its account. Error-class-driven retry/branching and any assertion on Ec2ImagePropertiesNotSupportedFault go the wrong way.
Fix: the CreateCustomDBEngineVersion handler's validation order in the rds service crate — resolve/validate the image (owner/existence) for a recognized custom engine and raise Ec2ImagePropertiesNotSupportedFault, rather than short-circuiting on the UseAwsProvidedLatestImage/ImageId combination.
Traces: rds.custom-engine-version-manifest-round-trip.

=== Cause 3 of 5: DeleteIntegration's IntegrationNotFoundFault message is truncated ===
AWS: DeleteIntegration with an IntegrationIdentifier that does not exist is refused with IntegrationNotFoundFault (HTTP 404, awsQueryError code IntegrationNotFoundFault). The message's fixed text is "Integration with identifier <integration ARN> could not be found. The integration might have been deleted, or you provided an incorrect identifier. Check the integration identifier and try again." — only the ARN (region and account from the caller, the identifier from the request) varies. Smithy: DeleteIntegration input IntegrationIdentifier (required), errors IntegrationNotFoundFault (404).
Emulator: the code is correct but the message stops after the first sentence: "Integration with identifier arn:aws:rds:<region>:<account>:integration:<id> could not be found." — the whole second-and-third-sentence remediation text AWS always appends is missing.
Why: callers that match the full message (and operators) lose the remediation half of the refusal; message assertions against AWS fail.
Fix: the DeleteIntegration handler's not-found refusal message template in the rds service crate.
Traces: rds.delete-integration-boundary.

=== Cause 4 of 5: DescribeDBRecommendations rejects an unknown Filters name with its own prose instead of AWS's validation-error message ===
AWS: DescribeDBRecommendations with Filters[0].Name set to a value outside the member's accepted pattern is refused with InvalidParameterValue (HTTP 400) whose message is the service's standard constraint-violation template: "<N> validation error(s) detected: Value '<value>' at '<member path, e.g. filters.1.member.name>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" — the count, the offending value, the member path and the pattern are all derived from the request and the model; the rest is fixed text. Smithy: DescribeDBRecommendations input Filters -> FilterList of Filter{Name (required), Values (required)}; the operation declares no operation-specific errors, so the refusal is the protocol-common InvalidParameterValue.
Emulator: code InvalidParameterValue is right, message is the emulator's own: "Unrecognized filter name: sim2real-bogus-filter-name".
Why: a client cannot tell which input member failed or what constraint it violated — the member path and the constraint AWS reports are both absent — and message-matching tests fail.
Fix: the DescribeDBRecommendations handler's Filters validation in the rds service crate — raise the shared constraint-violation message builder used for modeled member constraints instead of an ad-hoc sentence.
Traces: rds.describe-db-recommendations.

=== Cause 5 of 5: the default DB parameter group's max_connections formula has no effect on the running engine ===
AWS: a DB instance created with no explicit parameter group uses default.<family>, whose max_connections parameter value is the formula LEAST({DBInstanceClassMemory/9531392},5000) with ApplyType 'static'. The engine evaluates that formula from the instance class's memory, so SHOW max_connections on a 2 GiB class (db.t3.small) reports a value derived from the formula — not stock PostgreSQL's 100 — and pg_settings.source for max_connections is not 'default'. Reference: AWS RDS User Guide, "Maximum number of database connections" (CHAP_Limits.html#RDS_Limits.MaxConnections). Recorded behavior only for the exact integer, which depends on the class's memory.
Emulator: DescribeDBParameters does publish the formula and ApplyType 'static' (those assertions pass), but the engine ignores it: SHOW max_connections returns 100, the stock built-in default, outside the formula's range for the class — "max_connections on a 2 GiB class must be within [112, 225] (memory formula), got 100".
Why: the published configuration and the running engine disagree. A client sized against the parameter group's advertised connection ceiling is throttled at 100 connections, and connection-pool sizing derived from DescribeDBParameters is wrong.
Fix: the DB instance data plane / parameter-group application in the rds service crate — evaluate the default parameter group's DBInstanceClassMemory formulas for the instance's class and apply max_connections to the engine it starts, reporting a non-'default' pg_settings source.
Traces: rds.default-max-connections-follows-instance-class-memory-formula.

## Evidence
Cause 1 — expected message: `The option group <og> is for postgres <pg major>, and your DB instance is mysql <major.minor>.` (code InvalidParameterCombination). Actual: `The option group <og> is an option group of engine postgres, which is not the source DB instance's engine mysql` — `AssertionError: expected message 'The option group sim2real-rds-pgog-9cb8688ae255 is for postgres 17, and your DB instance is mysql 8.4.', got "The option group sim2real-rds-pgog-9cb8688ae255 is an option group of engine postgres, which is not the source DB instance's engine mysql"` (names/versions shown are this run's values filling the template).

Cause 2 — expected: code `Ec2ImagePropertiesNotSupportedFault`, message `Image <ImageId> doesn't exist in your account <account>. Verify <account> is the owner of the EC2 image.` Actual: `AssertionError: expected the recognized engine 'custom-sqlserver-ee' to reach AMI validation instead of an engine-recognition rejection, got 'InvalidParameterCombination': 'UseAwsProvidedLatestImage and ImageId cannot both be specified: the service-provided latest AMI and a named AMI are two different images'`.

Cause 3 — expected message ends with `:integration:<id> could not be found. The integration might have been deleted, or you provided an incorrect identifier. Check the integration identifier and try again.` Actual (code IntegrationNotFoundFault, correct): `Integration with identifier arn:aws:rds:us-west-2:000000000000:integration:0aa3b135-0a43-44ca-ac81-10c688a09a7d could not be found.` — second and third sentences missing (the ARN shown is this run's value).

Cause 4 — expected: `{'Code': 'InvalidParameterValue', 'Message': "1 validation error detected: Value '<name>' at 'filters.1.member.name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z-]+"}`. Actual: `{'Type': 'Sender', 'Code': 'InvalidParameterValue', 'Message': 'Unrecognized filter name: sim2real-bogus-filter-name'}`.

Cause 5 — expected: `SHOW max_connections` on a db.t3.small (2 GiB) instance using default.<family> yields the LEAST({DBInstanceClassMemory/9531392},5000) value for that class, and pg_settings.source != 'default'. Actual: `AssertionError: max_connections on a 2 GiB class must be within [112, 225] (memory formula), got 100` — the stock PostgreSQL default, while DescribeDBParameters for default.<family> correctly publishes the formula with ApplyType 'static'.

## Steps to Reproduce
Cause 1: create a mysql DB instance, create an option group with EngineName='postgres', then rds.create_db_instance_read_replica(SourceDBInstanceIdentifier=<mysql id>, OptionGroupName=<postgres option group>) and read Error.Message.
Cause 2: rds.create_custom_db_engine_version(Engine='custom-sqlserver-ee', EngineVersion='15.00.4073.23.<suffix>', UseAwsProvidedLatestImage=True, ImageId='ami-0000000000sim2real', Description=...) and read Error.Code/Message.
Cause 3: rds.delete_integration(IntegrationIdentifier=<a random uuid that was never created>) and read Error.Message.
Cause 4: rds.describe_db_recommendations(Filters=[{'Name': 'sim2real-bogus-filter-name', 'Values': ['x']}]) and read Error.Message.
Cause 5: rds.create_db_instance(Engine='postgres', DBInstanceClass='db.t3.small', no DBParameterGroupName) ; confirm DescribeDBParameters on default.<family> publishes max_connections = LEAST({DBInstanceClassMemory/9531392},5000) ; connect to the instance endpoint and run `SHOW max_connections` and `SELECT source FROM pg_settings WHERE name='max_connections'`.
