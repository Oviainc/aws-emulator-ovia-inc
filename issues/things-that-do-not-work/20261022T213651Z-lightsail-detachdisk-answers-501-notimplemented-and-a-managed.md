Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lightsail.disk-detach-not-attached-rejected lightsail.relational-database-native-sql-roundtrip
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail.disk-detach-not-attached-rejected|run#12|AssertionError|assert code == "OperationFailureException", f"unexpected code detaching unattached disk: {code!r}"
Issue-Key: lightsail.relational-database-native-sql-roundtrip|_connect_mysql#14|Timeout|time.sleep(5)
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# Lightsail: DetachDisk answers 501 NotImplemented, and a managed relational database announces an unreachable rds.lightsail.local master endpoint with no MySQL data plane

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetachDisk

## Description
Two distinct net-new root causes in the lightsail emulator, both grounded in failing behavioral checks. A fixer must close BOTH before submitting.

Cause 1 of 2: DetachDisk is an unimplemented scaffold stub, so no disk-detach behaviour (neither the success path nor the not-attached rejection) is served.
AWS: Smithy model com.amazonaws.lightsail#DetachDisk — input diskName (ResourceName, pattern ^\w[\w\-]*\w$, required), output operations (OperationList), modeled errors AccessDeniedException(403), AccountSetupInProgressException(428), InvalidInputException(400), NotFoundException(404), OperationFailureException(400), RegionSetupInProgressException(428), ServiceException(500), UnauthenticatedException(401). Modeled doc: "Detaches a stopped block storage disk from a Lightsail instance." Detaching a disk that is not attached to anything is the service's operation-failure rejection: recorded behavior shows HTTP 400 with wire error code OperationFailureException (recorded message, as an EXAMPLE only: "You can't detach this disk right now. The state of this disk is: available."), and the disk stays state=available / isAttached=false. Detaching a disk that IS attached (instance stopped) succeeds with a 200 carrying operations, and the disk returns to available.
Emulator: every DetachDisk call answers HTTP 501 with error code NotImplemented ("operation DetachDisk is scaffolded but not implemented in this emulator"). services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/detach_disk.rs still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and its handle() body is `Err(crate::scaffolded::Scaffolded::not_implemented("DetachDisk"))` (line 180).
Why: a client cannot detach a Lightsail disk at all, and a client that relies on the documented rejection to discover that a disk is not detachable sees an unmodeled 501/NotImplemented instead of a 400/OperationFailureException, so its error branch (which matches on the modeled Lightsail errors) never fires and the whole attach/detach lifecycle is unusable. Note this is a DIFFERENT operation from the already-filed AttachDisk stub; implementing AttachDisk alone does not fix it.
Fix: implement the DetachDisk handler in services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/detach_disk.rs — resolve the disk from crate::state, reject a disk that is not currently attached with OperationFailureException (400), otherwise clear the attachment and append the operation record, then delete the SCAFFOLDED marker line.
Traces: lightsail.disk-detach-not-attached-rejected (assertion: `unexpected code detaching unattached disk: 'NotImplemented'`).

Cause 2 of 2: a managed relational database reaches state=available and publishes a masterEndpoint, but that address is a synthetic name the emulator does not serve and does not answer on, so no SQL data plane exists.
AWS: Smithy model com.amazonaws.lightsail#GetRelationalDatabase — output relationalDatabase (RelationalDatabase) with masterEndpoint {address, port}. Recorded behavior only for the data plane: a publiclyAccessible Lightsail MySQL database that reports state=available accepts TCP connections on masterEndpoint.address:masterEndpoint.port (3306 for MySQL) and serves the MySQL wire protocol, so CREATE TABLE / INSERT / SELECT round-trips with the master credentials supplied to CreateRelationalDatabase.
Emulator: the control plane is real (state transitions creating -> available after STATE_TRANSITION_MILLIS and the record is stored), but masterEndpoint.address is minted as `ls-<uuid>.<12-char label>.<region>.rds.lightsail.local` (state.rs: relational_database_endpoint_address, RELATIONAL_DATABASE_ENDPOINT_SUFFIX = "rds.lightsail.local") — an address the emulator is not reached on and on which it serves no MySQL protocol at all (state.rs comment: "This emulator serves no MySQL or PostgreSQL wire protocol on it"). A client that connects to the announced endpoint never gets a connection, so the check hangs in its connect-retry loop until the harness 60s timeout.
Why: any client that follows the advertised contract — create database, wait for available, connect to masterEndpoint — blocks and eventually fails on connection error, so nothing that actually uses the database (migrations, ORMs, health checks, the delete-after-use path that follows) can run. It is the emulator's own divergence: it announces an address other than the one it is reachable on.
Fix: serve the relational-database data plane for an available database on an endpoint the emulator actually answers on — the MySQL/PostgreSQL listener alongside the control plane — and announce that reachable host/port from the stored record (the masterEndpoint projection in crate::state::StoredRelationalDatabase::view and the address minted by relational_database_endpoint_address) instead of the unroutable rds.lightsail.local name. This is the relational-database analogue of the already-open issues 20261022T192630Z-lightsail-bucket-object-data-plane-is-not-served.md (bucket/S3 data plane) and 20261022T170908Z-lightsail-container-service-public-endpoint-answers-501-so.md (container endpoint); neither covers the database endpoint.
Traces: lightsail.relational-database-native-sql-roundtrip (verdict mismatch, "Check timed out after 60.0s").

## Evidence
Cause 1 — lightsail.disk-detach-not-attached-rejected:
  expected: DetachDisk on a disk in state available -> HTTP 400, Error.Code = OperationFailureException (example message: "You can't detach this disk right now. The state of this disk is: available."); disk remains state=available, isAttached=false.
  actual:   HTTP 501, Error.Code = NotImplemented, message "operation DetachDisk is scaffolded but not implemented in this emulator".
  assertion: AssertionError: unexpected code detaching unattached disk: 'NotImplemented' (check_lightsail_disk_detach_not_attached_rejected.py line 29).
  source:   services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/detach_disk.rs lines 4 (SCAFFOLDED marker) and 180 (Err(Scaffolded::not_implemented("DetachDisk"))).

Cause 2 — lightsail.relational-database-native-sql-roundtrip:
  expected: after CreateRelationalDatabase(mysql blueprint, publiclyAccessible=true) and GetRelationalDatabase reporting state=available, a TCP+MySQL connection to masterEndpoint.address:masterEndpoint.port succeeds and CREATE TABLE/INSERT/SELECT round-trips.
  actual:   masterEndpoint.address is a synthetic `ls-<id>.<label>.<region>.rds.lightsail.local` host the emulator does not serve; every pymysql connect attempt fails, the check's retry loop never succeeds, and the check is cut off at the harness 60s timeout.
  source:   services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs line 3147 (RELATIONAL_DATABASE_ENDPOINT_SUFFIX = "rds.lightsail.local"), lines 3141-3146 ("This emulator serves no MySQL or PostgreSQL wire protocol on it"), line 3190 (relational_database_endpoint_address), line 3651 (masterEndpoint projection).

## Steps to Reproduce
Cause 1 (lightsail.disk-detach-not-attached-rejected):
1. lightsail CreateDisk(diskName=<name>, availabilityZone=<zone from GetRegions>, sizeInGb=8); poll GetDisk until state == "available".
2. Call DetachDisk(diskName=<name>).
3. Observe HTTP 501 Error.Code=NotImplemented; AWS answers HTTP 400 Error.Code=OperationFailureException and leaves the disk available/unattached.
4. Positive control (also blocked): create an instance, AttachDisk a second disk, StopInstance, DetachDisk -> expected 200 with operations and the disk back to available.

Cause 2 (lightsail.relational-database-native-sql-roundtrip):
1. blueprint = GetRelationalDatabaseBlueprints -> the mysql engine-default blueprintId; bundle = cheapest active GetRelationalDatabaseBundles bundleId.
2. CreateRelationalDatabase(relationalDatabaseName=<name>, relationalDatabaseBlueprintId=blueprint, relationalDatabaseBundleId=bundle, masterDatabaseName=roundtripdb, masterUsername=dbadmin, masterUserPassword=<pw>, publiclyAccessible=true).
3. Poll GetRelationalDatabase until relationalDatabase.state == "available" and read relationalDatabase.masterEndpoint.
4. Connect with a MySQL client to masterEndpoint.address:masterEndpoint.port using the master credentials, then CREATE TABLE / INSERT / SELECT.
5. Observe the connection never establishes (announced host is rds.lightsail.local, which the emulator neither resolves to itself nor serves MySQL on); the client blocks/retries until it gives up.
