Issue-Source: trace-sweep-divergence
Issue-Trace: cfetag-public-key-etag-chain
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfetag-public-key-etag-chain cloudfront-field-level-encryption-config-workflow cloudfront-field-level-encryption-lifecycle cloudfront-fle-delete-profile-in-use-error cloudfront-fle-profile-duplicate-caller-reference-error cloudfront-key-group-error-paths cloudfront-key-group-lifecycle-with-members cloudfront-key-group-precondition-and-inuse-errors cloudfront-list-public-keys-and-key-groups-pagination cloudfront-public-key-duplicate-and-notfound-errors cloudfront-public-key-lifecycle cloudfront-public-key-lifecycle-and-errors cloudfront-query-arg-profile-config-lifecycle cfdeep-request-local-limits cloudfront-cache-policy-custom-key-config cloudfront-cache-policy-duplicate-name-error cloudfront-create-origin-request-policy-duplicate-name-error cloudfront-delete-origin-request-policy-wrong-etag-error cloudfront-origin-request-policy-bad-ifmatch-error cloudfront-origin-request-policy-error-paths cloudfront-origin-request-policy-errors cloudfront-origin-request-policy-none-behavior cfetag-field-level-encryption-config-etag-chain cloudfront-content-type-profile-config-lifecycle cloudfront-field-level-encryption-config-lifecycle cfetag-field-level-encryption-profile-etag-chain cfetag-key-group-etag-chain cfetag-key-value-store-etag-chain cfetag-origin-access-control-etag-chain cloudfront-oac-lifecycle-create-get-update-list-delete cfetag-origin-access-identity-etag-chain cloudfront-oai-lifecycle-create-get-delete
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreatePublicKey|2xx->2xx|$headers.location
Issue-Key: cloudfront|CreateCachePolicy|2xx->2xx|$headers.location
Issue-Key: cloudfront|CreateOriginRequestPolicy|2xx->2xx|$headers.location
Issue-Key: cloudfront|CreateFieldLevelEncryptionConfig|2xx->2xx|$headers.location
Issue-Key: cloudfront|CreateFieldLevelEncryptionProfile|2xx->2xx|$headers.location
Issue-Key: cloudfront|CreateKeyGroup|2xx->2xx|$headers.location
Issue-Key: cloudfront|CreateKeyValueStore|2xx->2xx|$headers.location
Issue-Key: cloudfront|CreateOriginAccessControl|2xx->2xx|$headers.location
Issue-Key: cloudfront|CreateCloudFrontOriginAccessIdentity|2xx->2xx|$headers.location
Issue-Related: 20261029T212223Z-cloudfront-generated-identifiers-location-uris-and-post-create.md
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# CloudFront create responses announce a Location URI on a host the emulator does not serve

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePublicKey

## Description
Ten causes, one shared fault: every CloudFront resource-create handler builds its Location response header around the literal production host `cloudfront.amazonaws.com` instead of the endpoint host the request was actually received on, so the create response announces an address other than the one the emulator is reached on. The replay verdict for each of these clusters is `host_not_own` on `$headers.location`. Two of the clusters (causes 9 and 10) are decisive: in cause 10 the recorded and the emitted header are byte-for-byte identical and the step still diverges, which shows the requirement is that the announced host be one the emulator serves, not that the recorded string be echoed.

The open issue 20261029T212223Z-cloudfront-generated-identifiers-location-uris-and-post-create.md covers the Location of CreateFunction/CreateResponseHeadersPolicy and the OAI S3CanonicalUserId constant; it does NOT carry the replay keys for the ten operations below, which is why they are filed here.

Cause 1 of 10: CreatePublicKey's Location names an unserved host.
AWS: 201 with `Location: <service endpoint>/2020-05-31/public-key/<public key id>` - the URI is rooted at the endpoint the request was sent to (Smithy: CreatePublicKey output member Location is bound to the `Location` httpHeader; http {method POST, uri /2020-05-31/public-key, code 201}).
Emulator: emits a Location rooted at `https://cloudfront.amazonaws.com/...`, a host it does not serve.
Why: a client that follows Location to GET/PUT/DELETE the new public key is sent to the real AWS endpoint (or to a name that does not resolve in the test environment) instead of back to the emulator, so the create->address-the-resource round trip breaks.
Fix: CreatePublicKey's response construction (Location derivation - it must use the request's own endpoint/authority).
Traces: cfetag-public-key-etag-chain#task#1 (13 affected steps, incl. cloudfront-public-key-lifecycle#task#1, cloudfront-key-group-lifecycle-with-members#task#1).

Cause 2 of 10: CreateCachePolicy - same fault.
AWS: 201 with Location `<service endpoint>/2020-05-31/cache-policy/<cache policy id>`.
Emulator: Location on `cloudfront.amazonaws.com`.
Why/Fix: as cause 1, in CreateCachePolicy's response construction.
Traces: cfdeep-request-local-limits#task#6 (3 affected steps).

Cause 3 of 10: CreateOriginRequestPolicy - same fault.
AWS: 201 with Location `<service endpoint>/2020-05-31/origin-request-policy/<policy id>`.
Emulator: Location on `cloudfront.amazonaws.com`.
Why/Fix: as cause 1, in CreateOriginRequestPolicy's response construction.
Traces: cloudfront-create-origin-request-policy-duplicate-name-error#setup#1 (6 affected steps).

Cause 4 of 10: CreateFieldLevelEncryptionConfig - same fault.
AWS: 201 with Location `<service endpoint>/2020-05-31/field-level-encryption/<config id>` (Smithy: CreateFieldLevelEncryptionConfig output Location is an httpHeader).
Emulator: Location on `cloudfront.amazonaws.com`.
Why/Fix: as cause 1, in CreateFieldLevelEncryptionConfig's response construction.
Traces: cfetag-field-level-encryption-config-etag-chain#task#3 (5 affected steps).

Cause 5 of 10: CreateFieldLevelEncryptionProfile - same fault.
AWS: 201 with Location `<service endpoint>/2020-05-31/field-level-encryption-profile/<profile id>`.
Emulator: Location on `cloudfront.amazonaws.com`.
Why/Fix: as cause 1, in CreateFieldLevelEncryptionProfile's response construction.
Traces: cfetag-field-level-encryption-profile-etag-chain#task#2 (6 affected steps).

Cause 6 of 10: CreateKeyGroup - same fault.
AWS: 201 with Location `<service endpoint>/2020-05-31/key-group/<key group id>`.
Emulator: Location on `cloudfront.amazonaws.com`.
Why/Fix: as cause 1, in CreateKeyGroup's response construction.
Traces: cfetag-key-group-etag-chain#task#2 (5 affected steps).

Cause 7 of 10: CreateKeyValueStore - same fault.
AWS: 201 with Location `<service endpoint>/2020-05-31/key-value-store/<key value store ARN>`.
Emulator: Location on `cloudfront.amazonaws.com`.
Why/Fix: as cause 1, in CreateKeyValueStore's response construction.
Traces: cfetag-key-value-store-etag-chain#task#1.

Cause 8 of 10: CreateOriginAccessControl - same fault.
AWS: 201 with Location `<service endpoint>/2020-05-31/origin-access-control/<OAC id>`.
Emulator: Location on `cloudfront.amazonaws.com`.
Why/Fix: as cause 1, in CreateOriginAccessControl's response construction.
Traces: cfetag-origin-access-control-etag-chain#task#1 (2 affected steps).

Cause 9 of 10: CreateCloudFrontOriginAccessIdentity - same fault.
AWS: 201 with Location `<service endpoint>/2020-05-31/origin-access-identity/cloudfront/<identity id>`.
Emulator: Location on `cloudfront.amazonaws.com`. (The same cluster also fails on `$.CloudFrontOriginAccessIdentity.S3CanonicalUserId`, which is the constant-canonical-user-id cause already owned by 20261029T212223Z-cloudfront-generated-identifiers-location-uris-and-post-create.md; only the Location cause is net-new here.)
Why/Fix: as cause 1, in CreateCloudFrontOriginAccessIdentity's response construction.
Traces: cfetag-origin-access-identity-etag-chain#task#1.

Cause 10 of 10: CreateCloudFrontOriginAccessIdentity, second cluster - the echoed-recording case.
AWS: as cause 9.
Emulator: the Location it emits is byte-identical to the recorded one and the step still diverges with `host_not_own`, i.e. reproducing AWS's literal production host is itself the defect - the URI must be rooted at the emulator's own endpoint.
Why/Fix: as cause 1; this cause is what tells a fixer that templating the recorded host is not a fix.
Traces: cfetag-origin-access-identity-etag-chain#task#2 (2 affected steps, incl. cloudfront-oai-lifecycle-create-get-delete#task#1).

The ids, ARNs and account numbers in the recorded URIs are per-run values; the requirement is the path shape and that the URI's authority be the endpoint the caller reached.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, /__admin/health 200). All ten clusters are verdict=mismatch, expected_status 201 == actual_status 201, failing key `$headers.location` with verdict `host_not_own`:
1. CreatePublicKey - cfetag-public-key-etag-chain#task#1: got 'https://cloudfront.amazonaws.com/2020-05-31/public-key/K7IYCGPKTANSF3' (13 affected steps).
2. CreateCachePolicy - cfdeep-request-local-limits#task#6: got 'https://cloudfront.amazonaws.com/2020-05-31/cache-policy/f88bb8a8-724c-81ec-1b39-896a51a8749b' (3 affected).
3. CreateOriginRequestPolicy - cloudfront-create-origin-request-policy-duplicate-name-error#setup#1: got 'https://cloudfront.amazonaws.com/2020-05-31/origin-request-policy/e220a839-7b1d-cdaf-6e78-9e6aa1b965f4' (6 affected).
4. CreateFieldLevelEncryptionConfig - cfetag-field-level-encryption-config-etag-chain#task#3: got 'https://cloudfront.amazonaws.com/2020-05-31/field-level-encryption/CVBB9JKPE6KWE0' (5 affected).
5. CreateFieldLevelEncryptionProfile - cfetag-field-level-encryption-profile-etag-chain#task#2: got '.../2020-05-31/field-level-encryption-profile/PJZ50Z92T9DP30' (6 affected).
6. CreateKeyGroup - cfetag-key-group-etag-chain#task#2: got '.../2020-05-31/key-group/06c45d18-8009-454f-f88b-b8a8724c81ec' (5 affected).
7. CreateKeyValueStore - cfetag-key-value-store-etag-chain#task#1: got '.../2020-05-31/key-value-store/arn:aws:cloudfront::000000000000:key-value-store/e220a839-7b1d-cdaf-6e78-9e6aa1b965f4'.
8. CreateOriginAccessControl - cfetag-origin-access-control-etag-chain#task#1: got '.../2020-05-31/origin-access-control/E7IYCGPKTANSF3' (2 affected).
9. CreateCloudFrontOriginAccessIdentity - cfetag-origin-access-identity-etag-chain#task#1: got '.../2020-05-31/origin-access-identity/cloudfront/E7IYCGPKTANSF3', plus $.CloudFrontOriginAccessIdentity.S3CanonicalUserId value_mismatch (that member is owned by an existing issue).
10. CreateCloudFrontOriginAccessIdentity - cfetag-origin-access-identity-etag-chain#task#2: expected 'https://cloudfront.amazonaws.com/2020-05-31/origin-access-identity/cloudfront/E7IYCGPKTANSF3', got the identical string, verdict still host_not_own (2 affected).
No cluster carried a stub_path or an owned_by owner.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it; replay the wire corpus (trace set simparity-wire-aws-v1).
2. Representative step: fixture cfetag-public-key-etag-chain, step task#1 (CreatePublicKey) - read the Location response header and compare its authority with the endpoint the request was sent to.
3. Repeat for each other cause's representative step listed in Traces (CreateCachePolicy, CreateOriginRequestPolicy, CreateFieldLevelEncryptionConfig, CreateFieldLevelEncryptionProfile, CreateKeyGroup, CreateKeyValueStore, CreateOriginAccessControl, CreateCloudFrontOriginAccessIdentity x2).
4. In every case the header is rooted at https://cloudfront.amazonaws.com/ rather than at the emulator's own endpoint.
