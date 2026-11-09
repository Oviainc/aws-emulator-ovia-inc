Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: docdb.cluster-mongo-insert-find-round-trip docdb.mongo-aggregation-pipeline-groups-sums-and-sorts-the-matched-documents docdb.mongo-change-stream-delivers-insert-update-and-delete-events-in-commit-order docdb.mongo-transaction-writes-stay-invisible-until-commit-and-an-abort-discards-them docdb.mongo-unique-index-rejects-a-duplicate-key-and-a-compound-index-serves-a-sorted-query
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb.cluster-mongo-insert-find-round-trip|_probe_call#21|PublishedUrlRejected|host, port = ctx.endpoint_host(host, int(port))
Issue-Key: docdb.mongo-aggregation-pipeline-groups-sums-and-sorts-the-matched-documents|_mongo_session#2|PublishedUrlRejected|host, port = ctx.endpoint_host(host, int(port))
Issue-Key: docdb.mongo-change-stream-delivers-insert-update-and-delete-events-in-commit-order|_mongo_session#2|PublishedUrlRejected|host, port = ctx.endpoint_host(host, int(port))
Issue-Key: docdb.mongo-transaction-writes-stay-invisible-until-commit-and-an-abort-discards-them|_mongo_session#2|PublishedUrlRejected|host, port = ctx.endpoint_host(host, int(port))
Issue-Key: docdb.mongo-unique-index-rejects-a-duplicate-key-and-a-compound-index-serves-a-sorted-query|_mongo_session#2|PublishedUrlRejected|host, port = ctx.endpoint_host(host, int(port))
Issue-Commit: 70b7c68145c683ae49b96a2a3a7ef8edad56294d

# DocDB cluster/instance endpoints are published as real *.docdb.amazonaws.com DNS names the emulator does not serve, so no MongoDB data plane is reachable (1 cause, 5 checks)

> _Note: this issue was identified against the code base at commit `70b7c68145c683ae49b96a2a3a7ef8edad56294d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusters

## Description
Cause 1 of 1: DocDB synthesizes the cluster's connection endpoint as a public AWS DNS name instead of an address the emulator itself answers on, so every data-plane check that connects to the advertised endpoint is refused before it can speak MongoDB. Failing checks: docdb.cluster-mongo-insert-find-round-trip, docdb.mongo-aggregation-pipeline-groups-sums-and-sorts-the-matched-documents, docdb.mongo-change-stream-delivers-insert-update-and-delete-events-in-commit-order, docdb.mongo-transaction-writes-stay-invisible-until-commit-and-an-abort-discards-them, docdb.mongo-unique-index-rejects-a-duplicate-key-and-a-compound-index-serves-a-sorted-query.

AWS: Smithy (com.amazonaws.docdb#AmazonRDSv19) models DBCluster.Endpoint as "Specifies the connection endpoint for the primary instance of the cluster", DBCluster.Port as "Specifies the port that the database engine is listening on", and the Endpoint structure (Address/Port/HostedZoneId) with the service-level documentation "Network information for accessing a cluster or instance. Client programs must specify a valid endpoint to access these Amazon DocumentDB resources." The contract the model states is therefore not merely that an Endpoint string is emitted, but that the emitted host:port is the address at which the engine actually answers the MongoDB wire protocol: a client opens a MongoDB session to DBCluster.Endpoint:DBCluster.Port and authenticates with the cluster's master user. The host's *shape* on real AWS (<cluster-id>.<resource-id>.<region>.docdb.amazonaws.com) is an environment value, not a requirement; what is required is that the published address resolves to, and is served by, the DocumentDB endpoint the caller was handed.

Emulator: DescribeDBClusters (and the instance endpoint) publish a host in AWS's public DNS namespace — of the form <cluster-identifier>.<account-id>.<region>.docdb.amazonaws.com on port 27017 — which is not any host the emulator serves. The replay harness refuses the value outright: "endpoint_host was given <cluster>.<account>.<region>.docdb.amazonaws.com:27017 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address". The emulator announced an address other than the one it is reached on, so all five MongoDB data-plane checks (insert/find round trip, aggregation pipeline, change stream ordering, transaction visibility/abort, unique and compound indexes) stop at the first connect and none of the DocumentDB data-plane semantics they cover is exercised at all.

Why: a client that does what the model documents — read DBCluster.Endpoint/Port and open a MongoDB connection to it — gets a host that either does not resolve or resolves to real AWS, instead of the emulated cluster. Every DocumentDB application workflow (connect, authenticate, CRUD, aggregate, watch a change stream, run a transaction, build an index) is unreachable, and a client cannot tell a non-functional cluster from a network problem of its own.

Fix: the docdb cluster/instance endpoint construction — the state record that stores the cluster's endpoint address and the DBCluster/DBInstance serializer that projects Endpoint, ReaderEndpoint, Endpoint.Address and Port — must publish the emulator's own serving host and the port the emulated MongoDB listener is bound to, rather than a templated amazonaws.com name. If the DocumentDB wire listener itself does not exist, the endpoint must still be published as the emulator's own address and the data-plane listener stood up behind it.

Traces: docdb.cluster-mongo-insert-find-round-trip, docdb.mongo-aggregation-pipeline-groups-sums-and-sorts-the-matched-documents, docdb.mongo-change-stream-delivers-insert-update-and-delete-events-in-commit-order, docdb.mongo-transaction-writes-stay-invisible-until-commit-and-an-abort-discards-them, docdb.mongo-unique-index-rejects-a-duplicate-key-and-a-compound-index-serves-a-sorted-query.

## Evidence
All five docdb checks fail identically at the point where the check takes the cluster endpoint the emulator published and resolves it for a connection:

  File "context.py", line 619, in endpoint_host
    self._published_target(f"{published_host}:{port}", "endpoint_host")
  File "context.py", line 586, in _published_target
    raise PublishedUrlRejected(...)
  simparity.published.PublishedUrlRejected: endpoint_host was given sim2real-behav-docdb-in-9acef9350369.000000000002.us-west-2.docdb.amazonaws.com:27017 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address

Expected (AWS): DescribeDBClusters returns an Endpoint whose host:port is the address the DocumentDB engine answers on, so a MongoDB client connecting to it completes the TLS/SCRAM handshake and can run the session the check performs.
Actual (emulator): the published Endpoint host lives in AWS's public DNS namespace (*.docdb.amazonaws.com) and is not a host the emulator serves; the connection is never attempted against the emulator, so the data plane produces nothing.

The cluster identifier, account id, region and the 27017 port in the captured value are per-run/environment values, not requirements; the requirement is that the published host is the emulator's own serving host.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. ec2 CreateVpc + two CreateSubnet calls in two AZs; docdb CreateDBSubnetGroup over them; ec2 CreateSecurityGroup for the cluster.
3. docdb CreateDBCluster(DBClusterIdentifier=<unique>, Engine='docdb', MasterUsername='dbadmin', MasterUserPassword=<pw>, DBSubnetGroupName=<group>, VpcSecurityGroupIds=[<sg>]) and CreateDBInstance(DBInstanceClass='db.t3.medium', Engine='docdb', DBClusterIdentifier=<cluster>); wait for Status 'available'.
4. docdb DescribeDBClusters(DBClusterIdentifier=<cluster>) and read DBCluster.Endpoint and DBCluster.Port. Observed: Endpoint is '<cluster>.<account>.<region>.docdb.amazonaws.com' with Port 27017 — a host in AWS's public DNS namespace that the emulator does not serve. Expected: the emulator's own serving host (and the port its DocumentDB listener is bound to).
5. Open a MongoDB connection to that host:port (from inside the VPC, as the checks do via an in-VPC Lambda probe). Observed: nothing on the emulator is contacted. Expected: the session authenticates as the master user and serves insert/find, aggregate, change streams, transactions and index operations.
