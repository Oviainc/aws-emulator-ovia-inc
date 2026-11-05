Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.postgres-custom-port-is-the-published-and-listening-port
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19

# rds: a DB instance publishes the port its engine listener was assigned, not the `Port` the create asked for

## Operation
CreateDBInstance

## Description
Cause 1 of 1: the port a request names is recorded but never published, because the core listener API assigns the port.
AWS: `CreateDBInstanceMessage$Port` — "The port number on which the database accepts connections" — is published verbatim as `Endpoint.Port`, and a client that asked for `5433` connects to `5433`; `ModifyDBInstance`'s `DBPortNumber` moves it the same way.
Emulator: the rds crate now serves the DB instance data plane on a core-owned listener (`services/service-com-amazonaws-rds-amazonrdsv19/src/state/data_plane.rs`, `open_engine_endpoint`), and `Endpoint.Port` reports THAT listener's port, because `Listeners::tcp_plaintext` (`vendor/server/src/listeners.rs`) binds in the core's own port range and takes no requested port. The requested value is kept on the record (`endpoint_port`, reported as `DbInstancePort` for a cluster member) but a client that asked for a particular port is told a different one. Publishing the reachable port is deliberate — the alternative is an endpoint nothing answers on — so the divergence is in the port VALUE only, not in reachability.
Why: a client that pins its connection string to the port it asked for, or that asserts the published port equals the requested one, diverges even though the endpoint it is told about does serve the engine.
Fix: the core listener API has to accept a requested port. `Listeners::tcp_plaintext` (and the TLS variants beside it) should take an optional port to bind, answering the service's own "port in use" case when it cannot, so `open_engine_endpoint` can ask for `Port`/`DBPortNumber` and the control plane can publish the value the request named; `published_port()` in the rds crate then collapses back to the stored `endpoint_port`. Nothing in the service crate can close this gap on its own: `vendor/` is read-only from a service fix.
Traces: rds.postgres-custom-port-is-the-published-and-listening-port.

## Evidence
`CreateDBInstance` with `Engine=postgres`, `Port=5433`: the emulator answers `Endpoint.Port` with the loopback listener's assigned port (an ephemeral port in the core's range) instead of `5433`, while `Endpoint.Address` is the loopback host the listener is bound to and a PostgreSQL client completes a session on that address and port. Expected: `Endpoint.Port` is `5433`.

## Steps to Reproduce
(1) rds CreateDBInstance with DBInstanceIdentifier=emu-port-probe, Engine=postgres, DBInstanceClass=db.t3.micro, AllocatedStorage=20, MasterUsername/MasterUserPassword and Port=5433; (2) poll rds DescribeDBInstances until DBInstanceStatus=available; (3) read `Endpoint.Port` — it is the core-assigned listener port, not 5433. The reachability half is pinned by `tests/all/db_instance_engine_endpoint.rs::a_requested_port_is_recorded_while_the_published_port_is_the_listener_s`, which opens a session on the published endpoint.
