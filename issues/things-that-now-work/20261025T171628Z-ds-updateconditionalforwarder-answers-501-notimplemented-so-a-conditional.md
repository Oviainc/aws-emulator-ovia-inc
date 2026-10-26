Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.conditional-forwarder-ip-list-round-trip
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds.conditional-forwarder-ip-list-round-trip|run#17|AssertionError|assert mutation_code == "EntityDoesNotExistException", (
Issue-Commit: 982dbd2447376407a0d79f599aa90f2c7b4f6f2e

# ds UpdateConditionalForwarder answers 501 NotImplemented, so a conditional forwarder can never be mutated and the not-found refusal is never classified

> _Note: this issue was identified against the code base at commit `982dbd2447376407a0d79f599aa90f2c7b4f6f2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConditionalForwarder

## Description
One net-new root cause in the Directory Service crate's conditional-forwarder surface. The sibling stubs CreateConditionalForwarder (20261024T081601Z-ds-createconditionalforwarder-stub.md) and DeleteConditionalForwarder (20261025T030730Z-ds-deleteconditionalforwarder-stub.md) are both already resolved and both now work in this replay — the create, the describe round-trip and the delete all passed. Only the update leg is still a scaffold stub, and no open issue carries it (the closest open ds issues — 20261024T081601Z-ds-directory-service-control-plane-7-behavioral-divergences-error.md and 20261025T030730Z-ds-directory-service-4-behavioral-divergences-no-x-amzn.md — name DisableSso, RemoveRegion, RegisterEventTopic, IP routes, trusts and the RequestId envelope, not UpdateConditionalForwarder). This is not a REOPENS: the operations the fixed issues claimed to fix are not the refused ones here.

Cause 1 of 1: UpdateConditionalForwarder is an unimplemented scaffold stub.

AWS: Smithy (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json) models UpdateConditionalForwarder with required DirectoryId (pattern ^d-[0-9a-f]{10}$) and required RemoteDomainName (len 0..1024, pattern ^([a-zA-Z0-9]+[\.-])+([a-zA-Z0-9])+[.]?$), plus DnsIpAddrs (default []) and DnsIpv6Addrs; output UpdateConditionalForwarderResult; modeled errors ClientException, DirectoryUnavailableException, EntityDoesNotExistException, InvalidParameterException, ServiceException, UnsupportedOperationException. On success AWS replaces the stored ordered DnsIpAddrs for that (DirectoryId, RemoteDomainName) pair and returns HTTP 200 with an empty result. When the (DirectoryId, RemoteDomainName) pair does not exist — e.g. after DeleteConditionalForwarder removed it — AWS refuses with HTTP 400 and error code EntityDoesNotExistException (Smithy: EntityDoesNotExistException, errorKind client, httpError 400, members Message and RequestId; doc "The specified entity could not be found."). Recorded behavior for this check: the delete leg succeeds, a filtered DescribeConditionalForwarders then returns an empty ConditionalForwarders list with no exception, and the subsequent UpdateConditionalForwarder on the same pair is refused with code EntityDoesNotExistException. The message's fixed part is the entity-not-found sentence this service uses for a missing directory entity; its variable parts are the DirectoryId and the RemoteDomainName taken from the request — no value from any single recorded run is part of the requirement.

Emulator: every UpdateConditionalForwarder call returns HTTP 501 with error code 'NotImplemented' and the message "operation UpdateConditionalForwarder is scaffolded but not implemented in this emulator". The handler's handle() body is unconditionally `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateConditionalForwarder"))`, so neither the success path nor any modeled refusal is reachable. The replay asserts on the deleted-forwarder leg and reports: expected EntityDoesNotExistException mutating a deleted forwarder, got code='NotImplemented'.

Why: a client cannot change the DNS IP addresses of an existing conditional forwarder at all — the only mutating operation on that resource is dead, so a forwarder is write-once-then-delete-and-recreate. Worse, the refusal class is wrong in both status and code: callers that branch on EntityDoesNotExistException (400) to detect an already-deleted forwarder — which the cleanup helper in this very check does — instead see a 501 NotImplemented, which botocore surfaces as a non-retryable server fault. Any code that treats 5xx as transient will retry forever, and any code that treats EntityDoesNotExistException as "already gone, continue" will instead abort.

Fix: implement the operation's handler — services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/update_conditional_forwarder.rs is the only file for this operation. Resolve (DirectoryId, RemoteDomainName) against the conditional-forwarder store in crate::state (the same store CreateConditionalForwarder/DescribeConditionalForwarders/DeleteConditionalForwarder already use), replace the stored ordered DnsIpAddrs (and DnsIpv6Addrs) on a hit and return the empty 200 result, and return EntityDoesNotExistException for an unknown directory or an unknown/deleted remote domain, DirectoryUnavailableException for a directory not in a serviceable stage. Then delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line at the top of that file so the stub inventory stays accurate.

Traces: ds.conditional-forwarder-ip-list-round-trip (mismatch) — the create, describe-order round-trip and delete legs pass; the assertion fires on the post-delete UpdateConditionalForwarder.

## Evidence
Failing check: ds.conditional-forwarder-ip-list-round-trip [mismatch]

Assertion from the replay:
  File "check_ds_conditional_forwarder_ip_list_round_trip.py", line 61, in run
    assert mutation_code == "EntityDoesNotExistException", (
  AssertionError: expected EntityDoesNotExistException mutating a deleted forwarder, got code='NotImplemented'

Expected (AWS): HTTP 400, Error.Code = "EntityDoesNotExistException", Message = the service's entity-not-found sentence with the request's DirectoryId and RemoteDomainName filled in. On an existing forwarder: HTTP 200, empty UpdateConditionalForwarderResult, and a following DescribeConditionalForwarders returns the new DnsIpAddrs in the supplied order.

Actual (emulator): HTTP 501, Error.Code = "NotImplemented", Message = "operation UpdateConditionalForwarder is scaffolded but not implemented in this emulator" — for every input, valid or not.

Source confirming the cause, services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/update_conditional_forwarder.rs:
  line 4:   //! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and
  line 164: Err(crate::scaffolded::Scaffolded::not_implemented("UpdateConditionalForwarder"))

Smithy grounding (same crate's smithy-model): UpdateConditionalForwarder errors = ClientException, DirectoryUnavailableException, EntityDoesNotExistException, InvalidParameterException, ServiceException, UnsupportedOperationException; EntityDoesNotExistException { errorKind: client, httpError: 400, members: Message, RequestId }.

Note the preceding legs of the same check succeeded against the emulator (CreateConditionalForwarder, DescribeConditionalForwarders with order preserved, DeleteConditionalForwarder, then an empty filtered describe), so the divergence is isolated to the update operation and is not a harness or connectivity failure.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and point a botocore 'ds' client at it.
2. ec2.create_vpc(CidrBlock="10.67.0.0/16") and two subnets in two AZs.
3. ds.create_microsoft_ad(Name="<unique>.example.com", Password=<strong>, Edition="Standard", VpcSettings={VpcId, SubnetIds}) and wait until DescribeDirectories reports Stage == "Active".
4. ds.create_conditional_forwarder(DirectoryId=<id>, RemoteDomainName="sim2real-forwarder.example.org", DnsIpAddrs=["10.30.40.11","10.30.40.222","10.30.40.3"]).
5. ds.update_conditional_forwarder(DirectoryId=<id>, RemoteDomainName="sim2real-forwarder.example.org", DnsIpAddrs=["10.30.40.99"]) on the EXISTING forwarder — observe HTTP 501 NotImplemented instead of HTTP 200 and a describe that reports the new address list.
6. ds.delete_conditional_forwarder(DirectoryId=<id>, RemoteDomainName="sim2real-forwarder.example.org"), then repeat the update call — observe HTTP 501 with Error.Code "NotImplemented" instead of HTTP 400 with Error.Code "EntityDoesNotExistException".

Or replay the check directly: ds.conditional-forwarder-ip-list-round-trip.
