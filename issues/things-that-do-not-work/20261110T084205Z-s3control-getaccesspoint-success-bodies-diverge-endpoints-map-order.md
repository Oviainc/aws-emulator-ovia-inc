Issue-Source: trace-sweep-divergence
Issue-Trace: s3c-access-point-name-validation-splits-into-two-identities
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3c-access-point-name-validation-splits-into-two-identities s3c-access-point-public-access-block-is-replaced-not-merged s3c-account-id-host-prefix-is-authorized-before-the-request s3control-access-point-create-rejections s3control-access-point-lifecycle-and-policy
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|GetAccessPoint|2xx->2xx|$.GetAccessPointResult.Endpoints.entry[*].value
Issue-Key: s3control|GetAccessPoint|4xx->2xx|$.GetAccessPointResult.Endpoints.entry[*].value
Issue-Related: 20261106T121347Z-s3control-access-point-reads-diverge-authorization-and-existence.md
Issue-Commit: 69cb8c4786d799a8a66c2117bf8eebb4fb69db3f

# s3control GetAccessPoint success bodies diverge: Endpoints map order nondeterministic and refusals served as 200

> _Note: this issue was identified against the code base at commit `69cb8c4786d799a8a66c2117bf8eebb4fb69db3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessPoint

## Description
Three independent causes in the s3control GetAccessPoint read path, all net-new in this replay (none is marked owned by an open issue). The shared fault: GetAccessPoint emits a GetAccessPointResult success body where AWS either shapes that body differently or does not serve a body at all. Each cause must be closed separately.

Cause 1 of 3: the Endpoints map of GetAccessPointResult is serialized in a nondeterministic order, not in AWS's fixed order.
AWS: Smithy GetAccessPoint (GET /v20180820/accesspoint/{Name}, success code 200) output GetAccessPointResult member Endpoints -> com.amazonaws.s3control#Endpoints, a map of NonEmptyMaxLength64String keys to NonEmptyMaxLength1024String values, serialized in REST-XML as a sequence of <entry><key/><value/></entry> elements. Recorded behavior: AWS emits the four access-point endpoint entries in one stable order per response -- the plain regional endpoint first, then the FIPS endpoint, then the FIPS+dualstack endpoint, then the dualstack endpoint. The entry values themselves are the derived access-point hostnames for the request's region (the region is a value from the request/environment, the host *forms* and their relative order are the requirement).
Emulator: the same four values come back, but at different entry positions, and the position of a given value CHANGES between responses within the same replay: one response ordered them FIPS+dualstack, dualstack, FIPS, plain; another FIPS, plain, FIPS+dualstack, dualstack; another dualstack, FIPS+dualstack, plain, FIPS. The comparator reports every entry index as diverging, e.g. '$.GetAccessPointResult.Endpoints.entry[0].value: host_not_own (expected the plain regional access-point host, got the FIPS+dualstack one)'.
Why: the response is unstable -- a client (or a recorded-response assertion, or a diff of two reads of the same access point) that reads the endpoint map positionally, or that compares two GetAccessPoint bodies for equality, gets a different document every call even though nothing changed. Hypothesis: the map is held in an unordered hash map and serialized in iteration order.
Fix: the GetAccessPoint serializer / the access-point state record that holds the endpoint map -- it must use an order-preserving map populated in AWS's order so every response for the same access point is byte-identical.
Traces: s3c-access-point-name-validation-splits-into-two-identities#task#3 (representative); the same cluster also fires on s3c-access-point-public-access-block-is-replaced-not-merged#task#3/#5/#7/#9, s3c-account-id-host-prefix-is-authorized-before-the-request#task#3, s3control-access-point-create-rejections#task#3 and s3control-access-point-lifecycle-and-policy#task#3 (8 steps).

Cause 2 of 3: GetAccessPoint discloses the whole access-point configuration, endpoint map included, to a caller whose account identity does not own it.
AWS: Smithy GetAccessPoint input AccountId is @required, bound both to @httpHeader x-amz-account-id and to the endpoint host prefix '{AccountId}.' with pattern ^\d{12}$; the operation declares no modeled errors, so this is recorded behavior: S3 Control authorizes that account identity before resolving the access point and answers HTTP 403 with a REST-XML ErrorResponse whose Error.Code is 'AccessDenied', Error.Message is 'Access Denied' (fixed text) and Error.AccountId is the 12-digit account id taken from the request's host prefix / x-amz-account-id header (a request value, not a literal).
Emulator: answers HTTP 200 with a full GetAccessPointResult and no ErrorResponse member at all -- '$.ErrorResponse: missing', plus four Endpoints entries ('host_not_own', expected None) that should not be in a refusal at all.
Why: cross-account isolation is unobservable: a non-owning account reads the bucket name, VPC configuration, public-access-block and the access point's endpoint hostnames, so any client or test that proves ownership isolation passes on the emulator and fails on AWS.
Fix: the account-identity authorization step in the GetAccessPoint handler, which must run before the state lookup and must serialize the REST-XML AccessDenied error (with the Error.AccountId member) instead of a result body. NOTE: open issue 20261106T121347Z-s3control-access-point-reads-diverge-authorization-and-existence.md describes this authorization gap for the same fixture step, but it does NOT own this divergence: its recorded keys cover only the status and $.ErrorResponse, while this cluster additionally shows the Endpoints map of the wrongly-served success body, so re-replaying this step is required to confirm the refusal path.
Traces: s3c-account-id-host-prefix-is-authorized-before-the-request#task#4.

Cause 3 of 3: GetAccessPoint answers 200 for an access-point name that does not exist.
AWS: recorded behavior only (GetAccessPoint binds no errors in the model; s3control refusals are REST-XML <ErrorResponse><Error>...). A GET of /v20180820/accesspoint/{Name} for a name with no access point in the account is refused with HTTP 404, Error.Code 'NoSuchAccessPoint', Error.Message 'The specified accesspoint does not exist' (both fixed text) and an Error.AccessPointName member carrying the {Name} httpLabel value copied from the request URI.
Emulator: answers HTTP 200 with a GetAccessPointResult and no ErrorResponse ('$.ErrorResponse: missing', status expected 404 got 200), including four Endpoints entries for an access point that was never created.
Why: an existence probe always answers yes, so create/exists/delete flows and NoSuchAccessPoint error handling never run the branch they take on AWS; worse, the emulator hands back endpoint hostnames for a resource with no data path behind it.
Fix: the GetAccessPoint handler's existence lookup and its NoSuchAccessPoint error serialization; it must 404 for any name absent from state regardless of how state got there. NOTE: the preceding CreateAccessPoint acceptance over an absent bucket in this fixture is already filed as 20261030T092534Z-s3control-createaccesspoint-refusals-diverge-absent-bucket-accepted-wrong.md, and 20261106T121347Z-s3control-access-point-reads-diverge-authorization-and-existence.md describes the 404 gap -- neither owns this divergence (the replay reports no owner for it, the observed body now also carries the Endpoints map), so it must be re-replayed here.
Traces: s3control-access-point-create-rejections#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (33 fixtures, 331 steps, 277 passed, 54 mismatched, score 83.7%). All three causes are verdict=mismatch clusters with empty stub_path and empty owned_by.

Cause 1 -- s3c-access-point-name-validation-splits-into-two-identities#task#3, GetAccessPoint, status 200 -> 200, divergence_key 'service=s3control|GetAccessPoint|status=200->200|keys=$.GetAccessPointResult.Endpoints.entry[0].value,...entry[3].value':
  $.GetAccessPointResult.Endpoints.entry[0].value: host_not_own (expected 's3-accesspoint.<region>.amazonaws.com', got 's3-accesspoint-fips.dualstack.<region>.amazonaws.com')
  $.GetAccessPointResult.Endpoints.entry[1].value: host_not_own (expected 's3-accesspoint-fips.<region>.amazonaws.com', got 's3-accesspoint.dualstack.<region>.amazonaws.com')
  $.GetAccessPointResult.Endpoints.entry[2].value: host_not_own (expected 's3-accesspoint-fips.dualstack.<region>.amazonaws.com', got 's3-accesspoint-fips.<region>.amazonaws.com')
  $.GetAccessPointResult.Endpoints.entry[3].value: host_not_own (expected 's3-accesspoint.dualstack.<region>.amazonaws.com', got 's3-accesspoint.<region>.amazonaws.com')
  affected_count 8 (the other members are listed under Traces). The emitted order is not even stable across the replay: the same four values appear as [fips.dualstack, dualstack, fips, plain] here, as [fips, plain, fips.dualstack, dualstack] in s3c-account-id-host-prefix-is-authorized-before-the-request#task#4 and as [dualstack, fips.dualstack, plain, fips] in s3control-access-point-create-rejections#task#6 -- i.e. an unordered map, not AWS's fixed entry order.

Cause 2 -- s3c-account-id-host-prefix-is-authorized-before-the-request#task#4, GetAccessPoint: 'status: expected 403, got 200'; '$.ErrorResponse: missing (expected {"Error": {"Code": "AccessDenied", "Message": "Access Denied", "AccountId": "<the request's 12-digit account id>"}, ...}, got None)'; plus $.GetAccessPointResult.Endpoints.entry[0..3].value host_not_own (expected None, got the four access-point hostnames).

Cause 3 -- s3control-access-point-create-rejections#task#6, GetAccessPoint: 'status: expected 404, got 200'; '$.ErrorResponse: missing (expected {"Error": {"Code": "NoSuchAccessPoint", "Message": "The specified accesspoint does not exist", "AccessPointName": "<the {Name} value from the request URI>"}, ...}, got None)'; plus $.GetAccessPointResult.Endpoints.entry[0..3].value host_not_own (expected None, got the four access-point hostnames).

Model evidence (Smithy, services/service-com-amazonaws-s3control-awss3controlservicev20180820/smithy-model/com-amazonaws-s3control-awss3controlservicev20180820.json): GetAccessPoint GET /v20180820/accesspoint/{Name} code 200, endpoint hostPrefix '{AccountId}.', input AccountId @required @httpHeader x-amz-account-id + hostLabel pattern ^\d{12}$, Name @httpLabel length 3..255, errors [] ; GetAccessPointResult.Endpoints -> Endpoints map (key NonEmptyMaxLength64String 1..64, value NonEmptyMaxLength1024String 1..1024).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: fixture s3c-access-point-name-validation-splits-into-two-identities, step task#3 -- CreateAccessPoint over an existing bucket, then GetAccessPoint(AccountId, Name) and compare the Endpoints map entry order against the recording; then issue the same GetAccessPoint twice and diff the two bodies. AWS emits the entries in one fixed order (plain regional, FIPS, FIPS+dualstack, dualstack) and the two bodies are identical; the emulator permutes them. The same cluster reproduces on s3c-access-point-public-access-block-is-replaced-not-merged#task#3/#5/#7/#9, s3c-account-id-host-prefix-is-authorized-before-the-request#task#3, s3control-access-point-create-rejections#task#3 and s3control-access-point-lifecycle-and-policy#task#3.
4. Cause 2: fixture s3c-account-id-host-prefix-is-authorized-before-the-request, step task#4 -- GetAccessPoint for an existing access point with an accountId host prefix / x-amz-account-id that does not own it. Expect HTTP 403 with Error.Code AccessDenied, Error.Message 'Access Denied' and Error.AccountId = the request's account id; the emulator answers 200 with the full configuration and endpoint map.
5. Cause 3: fixture s3control-access-point-create-rejections, step task#6 -- GET /v20180820/accesspoint/{Name} for a name that was never successfully created. Expect HTTP 404 with Error.Code NoSuchAccessPoint, Error.Message 'The specified accesspoint does not exist' and Error.AccessPointName = the {Name} from the URI; the emulator answers 200.
