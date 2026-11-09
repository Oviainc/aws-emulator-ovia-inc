Issue-Source: trace-sweep-divergence
Issue-Trace: lightsail-catalog-and-operations-lifecycle
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lightsail-catalog-and-operations-lifecycle
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail|CreateKeyPair|2xx->2xx|$.keyPair.location.availabilityZone
Issue-Key: lightsail|CreateKeyPair|2xx->2xx|$.operation.location.availabilityZone
Issue-Key: lightsail|CreateKeyPair|2xx->2xx|$.publicKeyBase64
Issue-Key: lightsail|GetOperation|2xx->2xx|$.operation.location.availabilityZone
Issue-Key: lightsail|DeleteKeyPair|2xx->2xx|$.operation.location.availabilityZone
Issue-Key: lightsail|DeleteKeyPair|2xx->2xx|$.operation.operationDetails
Issue-Commit: 4cebb9a061dba8e344ca5830aab422b4a75e10f9

# Lightsail Operation records omit location.availabilityZone, and CreateKeyPair/DeleteKeyPair serialize non-AWS field values

> _Note: this issue was identified against the code base at commit `4cebb9a061dba8e344ca5830aab422b4a75e10f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKeyPair

## Description
Three net-new root causes on the Lightsail key-pair / operation-log wire surface. All three are 200-status responses whose body members diverge from AWS, so each must be closed separately.

Cause 1 of 3: every ResourceLocation the emulator serializes omits the availabilityZone member, so Operation.location and KeyPair.location carry only regionName.
AWS: Smithy com.amazonaws.lightsail#ResourceLocation has members availabilityZone (string) and regionName (RegionName enum); it is the target of Operation.location ("The Amazon Web Services Region and Availability Zone") and of KeyPair.location ("The region name and Availability Zone where the key pair was created"). Recorded behavior: for account-scoped, non-zonal resources and for operation records AWS populates location as {availabilityZone: "all", regionName: <the request's region>} — the literal string "all" is the fixed value for a resource that is not pinned to a single zone; the region name is the value taken from the request/endpoint. Observed on CreateKeyPair (both $.keyPair.location and $.operation.location), on GetOperation ($.operation.location) and on DeleteKeyPair ($.operation.location), i.e. on every Operation the emulator emits, not just one op.
Emulator: availabilityZone is absent from the serialized location object (expected 'all', got None) while regionName is present.
Why: a client reading operation.location.availabilityZone or keyPair.location.availabilityZone gets None instead of a zone string. Code that keys resources by zone, renders the zone, or asserts the account-scoped marker "all" either KeyErrors/NPEs or silently treats the resource as zone-less, and a round-trip through a typed SDK model loses the field entirely.
Fix: the shared ResourceLocation serializer (and whatever constructs the Operation and KeyPair records in the lightsail crate's state/model layer) must always populate availabilityZone — "all" for resources and operations that are not pinned to a single Availability Zone — not just regionName.
Traces: lightsail-catalog-and-operations-lifecycle#setup#1 (CreateKeyPair), lightsail-catalog-and-operations-lifecycle#task#2 (GetOperation), lightsail-catalog-and-operations-lifecycle#teardown#8 (DeleteKeyPair).

Cause 2 of 3: CreateKeyPair returns publicKeyBase64 as raw OpenSSH text instead of a base64 value.
AWS: Smithy com.amazonaws.lightsail#CreateKeyPair output member publicKeyBase64 targets the Base64 shape (as do privateKeyBase64 and KeyPair.fingerprint). A Base64-targeted member carries base64-encoded text, i.e. only the base64 alphabet — no spaces and no key-type prefix. The fixture asserts the member's shape rather than a literal value, since the key material itself is generated per run.
Emulator: the member's value is an OpenSSH authorized_keys line beginning with the literal "ssh-rsa " followed by the key body — it contains a space and the type prefix, so it is not a base64 value and fails the shape assertion.
Why: a client that base64-decodes publicKeyBase64 (the member name and its modeled Base64 target both say to) raises a decode error on the "ssh-rsa " prefix and the embedded space, so key material that AWS hands back usable cannot be recovered at all. The per-run key bytes are not a requirement; the encoding of the member is.
Fix: the CreateKeyPair handler's serializer must base64-encode the public key material it places in publicKeyBase64 (and keep privateKeyBase64 / fingerprint in the same modeled encoding), rather than emitting the OpenSSH text form.
Traces: lightsail-catalog-and-operations-lifecycle#setup#1.

Cause 3 of 3: the DeleteKeyPair Operation record fills operationDetails with the resource name instead of leaving it empty.
AWS: Smithy com.amazonaws.lightsail#Operation member operationDetails (string) is "Details about the operation" and is distinct from resourceName, which already names the affected resource. Recorded behavior: the Operation returned by DeleteKeyPair carries operationDetails as the empty string "" while resourceName carries the deleted key pair's name.
Emulator: operationDetails repeats the key-pair name (expected '', got the requested keyPairName).
Why: operationDetails is free-form detail text a client surfaces to users or matches on; duplicating resourceName into it makes the emulator's operation log report detail AWS does not report, so a client that distinguishes "no details" from "details present" branches the wrong way. The specific name is a per-request value; the requirement is that this operation type leaves the member empty.
Fix: whatever builds the DeleteKeyPair Operation record (the key-pair delete handler / the shared operation-log record constructor) must not default operationDetails to the resource name.
Traces: lightsail-catalog-and-operations-lifecycle#teardown#8.

Related but NOT owning these: open issue 20261029T234852Z-lightsail-key-pair-lifecycle-logs-no-operation-and.md covers the empty GetOperationsForResource list and a DeleteKeyPair that answered 400; on this checkout DeleteKeyPair answers 200 and the divergence is in the record's field values, which that issue's replay keys do not cover.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (score 55.9%, 15/34 steps mismatched).

Cause 1 — lightsail-catalog-and-operations-lifecycle#setup#1, CreateKeyPair, verdict mismatch, status 200 -> 200:
  $.keyPair.location.availabilityZone: missing (expected 'all', got None)
  $.operation.location.availabilityZone: missing (expected 'all', got None)
Cause 1 — lightsail-catalog-and-operations-lifecycle#task#2, GetOperation, verdict mismatch, status 200 -> 200:
  $.operation.location.availabilityZone: missing (expected 'all', got None)
Cause 1 — lightsail-catalog-and-operations-lifecycle#teardown#8, DeleteKeyPair, verdict mismatch, status 200 -> 200:
  $.operation.location.availabilityZone: missing (expected 'all', got None)

Cause 2 — lightsail-catalog-and-operations-lifecycle#setup#1, CreateKeyPair, verdict mismatch, status 200 -> 200:
  $.publicKeyBase64: value_mismatch (expected '<shaped-not-asserted>' — a Base64-shaped value; the key bytes themselves are per-run and not asserted), got an OpenSSH line of the form 'ssh-rsa AAAAB3NzaC1yc2E...' (type prefix + space, not base64).

Cause 3 — lightsail-catalog-and-operations-lifecycle#teardown#8, DeleteKeyPair, verdict mismatch, status 200 -> 200:
  $.operation.operationDetails: value_mismatch (expected '', got the requested keyPairName).

(Region name, key-pair name, operation id, key material and timestamps in these steps are per-run/request values. The requirements are: location.availabilityZone is always present and is 'all' for a non-zonal resource/operation; publicKeyBase64 is base64-encoded; DeleteKeyPair's operationDetails is empty.)

## Steps to Reproduce
Boot the emulator and point a botocore lightsail client at its data listener.

1. r = ls.create_key_pair(keyPairName=N) for a fresh name N.
   - r['keyPair']['location'] and r['operation']['location'] must each be {'availabilityZone': 'all', 'regionName': <client region>}. Observed: availabilityZone absent.
   - base64.b64decode(r['publicKeyBase64'], validate=True) must succeed. Observed: the value is 'ssh-rsa AAAA...' and the decode fails.
2. ls.get_operation(operationId=r['operation']['id'])['operation']['location']['availabilityZone'] must be 'all'. Observed: key absent.
3. d = ls.delete_key_pair(keyPairName=N).
   - d['operation']['location']['availabilityZone'] must be 'all'. Observed: key absent.
   - d['operation']['operationDetails'] must be ''. Observed: it is N.

Re-replay: lightsail-catalog-and-operations-lifecycle#setup#1, lightsail-catalog-and-operations-lifecycle#task#2, lightsail-catalog-and-operations-lifecycle#teardown#8.
