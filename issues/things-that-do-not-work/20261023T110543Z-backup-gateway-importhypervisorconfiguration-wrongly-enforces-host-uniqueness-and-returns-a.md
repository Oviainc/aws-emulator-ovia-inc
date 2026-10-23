Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup-gateway.concurrent-import-hypervisor-duplicate-name-host-all-independent
Issue-Shape-Id: com.amazonaws.backupgateway#BackupOnPremises_v20210101
Issue-Key: backup-gateway|ImportHypervisorConfiguration|ConflictException
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# ImportHypervisorConfiguration wrongly enforces Host uniqueness and returns a spurious ConflictException

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportHypervisorConfiguration

## Description
One root cause.

Cause 1 of 1: ImportHypervisorConfiguration rejects a second import that reuses an already-seen Host with ConflictException, but AWS Backup Gateway imposes no uniqueness constraint on Name or Host.

AWS: Smithy (com-amazonaws-backupgateway-backuponpremises-v20210101): ImportHypervisorConfiguration input members Name (@required, @length 1-100, @pattern ^[a-zA-Z0-9-]*$) and Host (@required, @length 3-128, @pattern ^.+$); output HypervisorArn (ServerArn, @pattern ^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}/[a-zA-Z-0-9]+$). The model declares no uniqueness/idempotency trait on either member and no client-request-token member, and the operation is not @idempotent. Recorded behavior: repeated imports of the identical Name+Host pair each succeed and each mints its own, distinct HypervisorArn; GetHypervisor on each ARN returns that ARN's own Name and Host. ConflictException (409) IS modeled on the operation, but recorded behavior shows it is not raised for a duplicate Name+Host.

Emulator: the second and subsequent imports of the same Host fail with HTTP 409 ConflictException (observed message example: "A hypervisor has already been imported for this host."). Under the concurrency check, five racers submitting the identical Name+Host pair produce one success and ConflictException for the losers, so the call raises before any ARN is returned.

Why: a client that legitimately registers several hypervisor configurations pointing at the same host (re-import after credential rotation, multiple configurations of one appliance, or simply a retried/parallel import) gets a hard 409 where AWS returns a new HypervisorArn. Code paths that treat ConflictException as a terminal "already exists, reuse the existing resource" signal silently converge on a single resource that AWS would have kept as independent hypervisors, and concurrent importers lose work entirely. The resource count and the ARN set a caller observes diverge from AWS.

Fix: remove the Host (and Name) uniqueness/dedup check from the ImportHypervisorConfiguration handler and its backing hypervisor state keying in services/service-com-amazonaws-backupgateway-backuponpremises-v20210101 — each accepted import must insert a new hypervisor record keyed by a freshly minted HypervisorArn, with the request's Name and Host stored on that record, and must do so safely under concurrent calls. ConflictException should remain reachable only for the states AWS actually conflicts on, not for duplicate Name/Host.

Traces: backup-gateway.concurrent-import-hypervisor-duplicate-name-host-all-independent (5 concurrent imports of one Name+Host pair; expected 5 distinct HypervisorArns, got ConflictException from the losing racers).

## Evidence
Check backup-gateway.concurrent-import-hypervisor-duplicate-name-host-all-independent [error]

Expected (AWS): all 5 racing ImportHypervisorConfiguration calls with the same Name and Host succeed; len(set(arns)) == 5; GetHypervisor(arn) for each returns Name == the requested name and Host == the requested host.

Actual (emulator): botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the ImportHypervisorConfiguration operation: A hypervisor has already been imported for this host.  (HTTP 409 — the modeled ConflictException, raised where AWS returns a new HypervisorArn. Message text quoted as an observed example; the divergence is the 409/ConflictException itself instead of a 200 with a fresh ARN.)

## Steps to Reproduce
1. Boot the emulator and point a botocore client at it: bg = client("backup-gateway").
2. Pick one pair, e.g. Name="sim2real-bgw-race-1", Host="sim2real-bgw-race-1.example.com".
3. Call bg.import_hypervisor_configuration(Name=Name, Host=Host, Username="sim2real-user", Password="Sim2Real-Passw0rd!") twice (sequentially is enough; the check does it 5-way concurrently via ThreadPoolExecutor).
4. Observe the first call returns a HypervisorArn and every later call raises ConflictException(409). AWS returns a distinct HypervisorArn for every call.
5. For each returned ARN, bg.get_hypervisor(HypervisorArn=arn) must echo the requested Name and Host.
