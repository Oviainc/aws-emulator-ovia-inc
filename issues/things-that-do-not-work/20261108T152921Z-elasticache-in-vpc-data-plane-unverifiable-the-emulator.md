Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.auth-token-dual-validity-window elasticache.auth-token-special-characters-preserved elasticache.cache-cluster-resp-protocol-round-trip-over-socket elasticache.e2e-cluster-scale-failover-data-consequence elasticache.e2e-cluster-snapshot-restore-data-consistency elasticache.e2e-cluster-snapshot-restore-data-persists elasticache.e2e-user-group-access-revocation-flow elasticache.e2e-user-group-auth-enforcement-flow elasticache.elasticache-redis-set-get elasticache.elasticache-redis-roundtrip-in-vpc
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.auth-token-dual-validity-window|_probe_call#18|AssertionError|assert "FunctionError" not in resp, f"the in-VPC probe raised: {out!r}"
Issue-Key: elasticache.auth-token-special-characters-preserved|_probe_call#14|AssertionError|assert "FunctionError" not in resp, f"the in-VPC probe raised: {out!r}"
Issue-Key: elasticache.cache-cluster-resp-protocol-round-trip-over-socket|_probe_call#25|AssertionError|assert "FunctionError" not in resp, f"the in-VPC probe raised: {out!r}"
Issue-Key: elasticache.e2e-cluster-scale-failover-data-consequence|_probe_call#13|AssertionError|assert "FunctionError" not in resp, f"the in-VPC probe raised: {out!r}"
Issue-Key: elasticache.e2e-cluster-snapshot-restore-data-consistency|_probe_call#15|AssertionError|assert "FunctionError" not in resp, f"the in-VPC probe raised: {out!r}"
Issue-Key: elasticache.e2e-cluster-snapshot-restore-data-persists|_probe_call#16|AssertionError|assert "FunctionError" not in resp, f"the in-VPC probe raised: {out!r}"
Issue-Key: elasticache.e2e-user-group-access-revocation-flow|_probe_call#25|AssertionError|assert "FunctionError" not in resp, f"the in-VPC probe raised: {out!r}"
Issue-Key: elasticache.e2e-user-group-auth-enforcement-flow|_probe_call#23|AssertionError|assert "FunctionError" not in resp, f"the in-VPC probe raised: {out!r}"
Issue-Key: elasticache.elasticache-redis-set-get|_probe_call#20|AssertionError|assert "FunctionError" not in resp, f"the in-VPC probe raised: {out!r}"
Issue-Key: elasticache.elasticache-redis-roundtrip-in-vpc|run#11|AssertionError|raise _no_executor(
Issue-Commit: 4466ef5882b584b00b87b338b6098c1e65748c9e

# ElastiCache in-VPC data plane unverifiable: the emulator's Lambda Python runtime has no _ssl module and a VPC-attached function has no host network (2 causes)

> _Note: this issue was identified against the code base at commit `4466ef5882b584b00b87b338b6098c1e65748c9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCacheCluster

## Description
Two independent root causes in the emulator's Lambda execution environment stop every ElastiCache in-VPC data-plane check from reaching the cache at all. The subject under test is ElastiCache's RESP endpoint, but the emulator's own in-VPC executor is what fails, so none of these runs measured the cache. Related open issue: 20261105T090541Z-elasticache-every-in-vpc-data-plane-check-is.md reports the SAME family with a different symptom (the runtime's os module lacking register_at_fork) and owns a different set of check ids; the replay report binds none of the ids below to it, and the failure signature here is a missing _ssl extension module, so it is re-grounded.

Cause 1 of 2: the emulator's Lambda Python runtime cannot import the standard-library ssl module, so the in-VPC RESP probe handler dies at import time.
AWS: recorded behavior — a Lambda function created with the python3.12 runtime imports `ssl` (and hence `socket`+TLS) successfully, and Invoke returns the handler's JSON payload with no FunctionError; this is how the probe opens a TLS RESP session to an ElastiCache in-transit-encrypted endpoint from inside the VPC.
Emulator: Invoke answers StatusCode=200 with FunctionError='Unhandled' and body {"errorMessage": "Unable to import module 'index': No module named '_ssl'", "errorType": "Runtime.ImportModuleError", "requestId": "", "stackTrace": []} — the `_ssl` extension module is absent from the runtime image, so any handler that imports `ssl` fails before the handler runs. The reported runtime is python 3.14.0.alpha presenting itself as AWS_Lambda_python3.12.
Why: every ElastiCache check that proves data-plane behavior (SET/GET, AUTH, user-group enforcement, snapshot/restore data persistence, failover consequence, RESP framing over a socket) executes its commands from a Lambda inside the VPC. With no `ssl`, nine checks measure nothing: a client cannot verify its cache serves data, and a TLS-only (transit-encryption-required) endpoint cannot be reached at all.
Fix: the Lambda runtime image/interpreter the emulator executes functions with — it must provide a complete CPython standard library including the _ssl extension (and must match the runtime identifier it advertises).
Traces: elasticache.auth-token-dual-validity-window, elasticache.auth-token-special-characters-preserved, elasticache.cache-cluster-resp-protocol-round-trip-over-socket, elasticache.e2e-cluster-scale-failover-data-consequence, elasticache.e2e-cluster-snapshot-restore-data-consistency, elasticache.e2e-cluster-snapshot-restore-data-persists, elasticache.e2e-user-group-access-revocation-flow, elasticache.e2e-user-group-auth-enforcement-flow, elasticache.elasticache-redis-set-get.

Cause 2 of 2: a Lambda function created with VpcConfig has no host network, so it has no in-VPC vantage point at all.
AWS: recorded behavior — CreateFunction with VpcConfig{SubnetIds, SecurityGroupIds} attaches ENIs in those subnets; the running function's outbound socket toward an in-VPC address reports a source address inside the VPC CIDR, and a DNS query to the VPC resolver at CIDR-base+2 is answered (an address answerable only from inside that VPC).
Emulator: CreateFunction accepts the VpcConfig and the package runs, but every socket operation fails with 'OSError: a Lambda function in this emulator has no host network to reach': the source address toward 10.150.0.2 is None and the VPC resolver never replies, so neither in-VPC signal can fire.
Why: VPC attachment is the whole premise of private-cache access. A function that reports success for CreateFunction+Invoke but has no network cannot reach an ElastiCache node, and a client cannot distinguish 'my security group is wrong' from 'this function has no networking'.
Fix: the Lambda invoke/execution path's VPC attachment in the emulator (give a VpcConfig-attached function a network vantage point inside the named VPC, with the VPC resolver reachable), or refuse the VpcConfig rather than accepting it and running without one.
Traces: elasticache.elasticache-redis-roundtrip-in-vpc.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against this checkout (68/161 passed).
1. Nine checks fail identically with 'AssertionError: the in-VPC probe raised: {'errorMessage': "Unable to import module 'index': No module named '_ssl'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '', 'stackTrace': []}' (elasticache.auth-token-dual-validity-window, elasticache.auth-token-special-characters-preserved, elasticache.cache-cluster-resp-protocol-round-trip-over-socket, elasticache.e2e-cluster-scale-failover-data-consequence, elasticache.e2e-cluster-snapshot-restore-data-consistency, elasticache.e2e-cluster-snapshot-restore-data-persists, elasticache.e2e-user-group-access-revocation-flow, elasticache.e2e-user-group-auth-enforcement-flow, elasticache.elasticache-redis-set-get). The owned siblings show the same body as 'Invoke of <fn> returned StatusCode=200 FunctionError=\'Unhandled\''.
2. elasticache.elasticache-redis-roundtrip-in-vpc: 'AssertionError: the in-VPC executor is unusable, so this run measured nothing: CreateFunction accepted VpcConfig={"SubnetIds": [...], "SecurityGroupIds": [...]} and the package ran, but neither in-VPC signal fired: the source address toward 10.150.0.2 was None (error "OSError: errno=None a Lambda function in this emulator has no host network to reach"), which is not inside the executor VPC CIDR 10.150.0.0/16; and the VPC resolver at 10.150.0.2 did not reply at all (error "OSError: a Lambda function in this emulator has no host network to reach"). Selftest evidence: {... "runtime": {"python": "3.14.0.alpha", "exec_env": "AWS_Lambda_python3.12", "region": "us-west-2"}}' — the selftest digest verified, so the package DID execute; only the network is absent. (Addresses and CIDRs here are per-run values; the requirement is that a VpcConfig-attached function has a source address inside the VPC it named and can reach that VPC's resolver.)

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: CreateFunction with Runtime='python3.12' and a handler whose first line is `import ssl`, then Invoke it -> expect StatusCode=200 with no FunctionError and the handler's payload; observe FunctionError='Unhandled' with Runtime.ImportModuleError "No module named '_ssl'". Re-replay the nine check ids listed under Cause 1.
3. Cause 2: CreateVpc + two subnets, CreateFunction with VpcConfig naming them, Invoke a handler that (a) opens a UDP socket toward <vpc-cidr-base+2>:53 and reports getsockname()[0] and (b) sends a DNS A query to that resolver -> expect a source address inside the VPC CIDR and/or any DNS reply; observe OSError 'a Lambda function in this emulator has no host network to reach' for both. Re-replay elasticache.elasticache-redis-roundtrip-in-vpc.
