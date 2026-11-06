Issue-Source: trace-sweep-divergence
Issue-Trace: s3c-account-id-host-prefix-is-authorized-before-the-request
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3c-account-id-host-prefix-is-authorized-before-the-request s3control-access-point-create-rejections s3control-access-point-listing-pagination-and-bucket-scoping
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|GetAccessPoint|4xx->2xx|$.ErrorResponse
Issue-Key: s3control|GetAccessPoint|4xx->2xx|status
Issue-Key: s3control|GetAccessPoint|4xx->4xx|$.ErrorResponse.Error.AccountId
Issue-Key: s3control|GetAccessPoint|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: s3control|GetAccessPoint|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|GetAccessPoint|4xx->4xx|status
Issue-Key: s3control|ListAccessPoints|4xx->2xx|$.ErrorResponse
Issue-Key: s3control|ListAccessPoints|4xx->2xx|status
Issue-Key: s3control|ListAccessPoints|2xx->4xx|$.ListAccessPointsResult
Issue-Key: s3control|ListAccessPoints|2xx->4xx|status
Issue-Commit: 4274f29ee1bfe9df1e954a309f62e0b76d7a1961

# s3control access-point reads diverge: authorization and existence refusals missing, bucket-scoped listing rejected

> _Note: this issue was identified against the code base at commit `4274f29ee1bfe9df1e954a309f62e0b76d7a1961`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessPoint

## Description
Five independent causes in the s3control access-point READ surface. The shared fault: the handlers skip the refusals AWS performs before serving an access-point read (owner authorization, then existence), and reject a legitimate scoped listing. Each cause must be closed separately.

Cause 1 of 5: GetAccessPoint serves an access point to a caller whose accountId host prefix / x-amz-account-id does not own it.
AWS: Smithy: GetAccessPoint input AccountId is @required, bound to @httpHeader x-amz-account-id AND to the endpoint host prefix "{AccountId}." with pattern ^\d{12}$; S3 Control authorizes that account identity BEFORE resolving the access point. Recorded behavior: HTTP 403 with REST-XML ErrorResponse Error.Code "AccessDenied", Error.Message "Access Denied" and an Error.AccountId member. Message rule: "Access Denied" is fixed text; Error.AccountId is a value taken from the request's account identity (the 12-digit id in the host prefix/header), never a literal from one recording.
Emulator: answers HTTP 200 with a GetAccessPointResult body and no ErrorResponse at all — the access point's configuration is disclosed to a non-owning account.
Why: a client relying on AWS's cross-account isolation sees a foreign account's access point (bucket name, VPC configuration, public-access-block, ARN) instead of AccessDenied, so any test or tool that proves ownership isolation passes against the emulator and fails against AWS.
Fix: the account-identity authorization step shared by the access-point read handlers (GetAccessPoint / ListAccessPoints), which must run before state lookup and must emit the REST-XML AccessDenied error with the Error.AccountId member.
Traces: s3c-account-id-host-prefix-is-authorized-before-the-request#task#4.

Cause 2 of 5: GetAccessPoint orders the checks wrongly — existence is evaluated before authorization.
AWS: recorded behavior only (the model declares no operation errors for GetAccessPoint; s3control uses the common REST-XML error shapes): for a request whose account identity is not authorized, AWS answers HTTP 403 Error.Code "AccessDenied", Error.Message "Access Denied", Error.AccountId = the request's account id — regardless of whether the named access point exists in that account, i.e. authorization is decided first and existence is never leaked.
Emulator: answers HTTP 404 with Error.Code "NoSuchAccessPoint", Error.Message "The specified accesspoint does not exist" and no Error.AccountId member.
Why: the emulator leaks existence information across accounts and gives the client the wrong retry/handling class (a 404 NoSuchAccessPoint is a "create it" signal, a 403 AccessDenied is a credentials/ownership signal), so client error handling branches differently than on AWS.
Fix: same authorization step as Cause 1, evaluated before the access-point existence lookup in the GetAccessPoint handler.
Traces: s3c-account-id-host-prefix-is-authorized-before-the-request#task#5.

Cause 3 of 5: ListAccessPoints does not refuse an unauthorized account identity either.
AWS: Smithy: ListAccessPoints input AccountId is @required, @httpHeader x-amz-account-id + host label, pattern ^\d{12}$; http GET /v20180820/accesspoint, success code 200. Recorded behavior: an unauthorized account identity is refused with HTTP 403, Error.Code "AccessDenied", Error.Message "Access Denied" (fixed text) and Error.AccountId taken from the request.
Emulator: answers HTTP 200 with a ListAccessPointsResult and no ErrorResponse, enumerating access points for an account identity it never authorized.
Why: cross-account enumeration succeeds, so a client cannot distinguish "no access points" from "not allowed to look", and ownership-isolation assertions silently pass.
Fix: apply the same pre-lookup account authorization in the ListAccessPoints handler.
Traces: s3c-account-id-host-prefix-is-authorized-before-the-request#task#8.

Cause 4 of 5: GetAccessPoint returns 200 for an access point that does not exist.
AWS: recorded behavior only: a GET of /v20180820/accesspoint/{Name} for a name with no access point in the account is refused with HTTP 404, REST-XML Error.Code "NoSuchAccessPoint", Error.Message "The specified accesspoint does not exist" and an Error.AccessPointName member. Message rule: the Code and the message text are fixed; Error.AccessPointName is the {Name} httpLabel value copied from the request URI.
Emulator: answers HTTP 200 with a result body and no ErrorResponse for that absent name. (In this fixture the preceding CreateAccessPoint for a non-existent bucket was also wrongly accepted — that acceptance is already filed as 20261030T092534Z-s3control-createaccesspoint-refusals-diverge-absent-bucket-accepted-wrong.md — but this GetAccessPoint cluster is NOT owned by it: the read handler must 404 for any name absent from state, independently of how the state got that way.)
Why: a client's existence probe ("does this access point exist yet?") always answers yes, so create/exists/delete flows and NoSuchAccessPoint error handling never execute the path AWS takes.
Fix: the GetAccessPoint handler's existence lookup and its NoSuchAccessPoint error serialization.
Traces: s3control-access-point-create-rejections#task#6.

Cause 5 of 5: ListAccessPoints rejects a legitimate bucket-scoped listing with 400 instead of returning an empty list.
AWS: Smithy: ListAccessPoints has optional @httpQuery members bucket (BucketName, length 3..255), nextToken, maxResults (range 0..1000), dataSourceId, dataSourceType, and http success code 200; the output's AccessPointList may be empty. A scoped listing that matches nothing is a successful empty page, not an error.
Emulator: answers HTTP 400 and no ListAccessPointsResult at all for the scoped listing, where the recording expects 200 with an empty AccessPointList. Hypothesis: the handler treats the bucket (or pagination) query parameter combination as invalid rather than as a filter that may select zero access points.
Why: a client that lists access points for a bucket receives a client-error instead of an empty page, so pagination/scoping code treats a normal "nothing here" result as a request defect and aborts.
Fix: the ListAccessPoints handler's query-parameter handling and bucket/pagination filtering, which must answer 200 with an empty AccessPointList when the filter selects nothing.
Traces: s3control-access-point-listing-pagination-and-bucket-scoping#task#7.

Related open issues that do NOT cover these causes: 20261102T104655Z-s3control-account-level-publicaccessblock-operations-never-deny-the.md covers the account-level PublicAccessBlock operations only (not the access-point reads above), and 20261024T005340Z-s3control-public-access-block-not-enforced-getaccesspoint-getaccesspointpolicystatus.md covers GetAccessPoint result completeness, not these status/refusal divergences.

## Evidence
Replay of trace set simparity-wire-aws-v1 (s3control, 33 fixtures):

Cause 1 — s3c-account-id-host-prefix-is-authorized-before-the-request#task#4, GetAccessPoint: expected status 403, got 200; '$.ErrorResponse: missing (expected {"Error": {"Code": "AccessDenied", "Message": "Access Denied", "AccountId": "<request account id>"}, ...}, got None)'.

Cause 2 — s3c-account-id-host-prefix-is-authorized-before-the-request#task#5, GetAccessPoint: expected status 403, got 404; Error.Code expected 'AccessDenied', got 'NoSuchAccessPoint'; Error.Message expected 'Access Denied', got 'The specified accesspoint does not exist'; Error.AccountId missing (expected the request's account id, got None).

Cause 3 — s3c-account-id-host-prefix-is-authorized-before-the-request#task#8, ListAccessPoints: expected status 403, got 200; '$.ErrorResponse: missing (expected Error.Code AccessDenied / Error.Message "Access Denied" / Error.AccountId, got None)'.

Cause 4 — s3control-access-point-create-rejections#task#6, GetAccessPoint: expected status 404, got 200; '$.ErrorResponse: missing (expected {"Error": {"Code": "NoSuchAccessPoint", "Message": "The specified accesspoint does not exist", "AccessPointName": "<name from request URI>"}, ...}, got None)'.

Cause 5 — s3control-access-point-listing-pagination-and-bucket-scoping#task#7, ListAccessPoints: expected status 200, got 400; '$.ListAccessPointsResult: missing (expected {"AccessPointList": ""}, got None)'.

Model evidence (Smithy, com-amazonaws-s3control-awss3controlservicev20180820): GetAccessPoint GET /v20180820/accesspoint/{Name} code 200, AccountId @required @httpHeader x-amz-account-id + hostLabel pattern ^\d{12}$; ListAccessPoints GET /v20180820/accesspoint code 200, optional httpQuery bucket/nextToken/maxResults, output AccessPointList (list) + NextToken, @paginated.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1/2/3: fixture s3c-account-id-host-prefix-is-authorized-before-the-request, steps task#4 (GetAccessPoint), task#5 (GetAccessPoint on an absent name under an unauthorized account identity) and task#8 (ListAccessPoints) — each issues the request with an accountId host prefix / x-amz-account-id that does not own the access point; AWS answers 403 AccessDenied, the emulator answers 200, 404 NoSuchAccessPoint and 200 respectively.
4. Cause 4: fixture s3control-access-point-create-rejections, step task#6 — GET /v20180820/accesspoint/{Name} for a name that was never successfully created; AWS answers 404 NoSuchAccessPoint with Error.AccessPointName, the emulator answers 200.
5. Cause 5: fixture s3control-access-point-listing-pagination-and-bucket-scoping, step task#7 — ListAccessPoints scoped by the bucket query parameter where nothing matches; AWS answers 200 with an empty AccessPointList, the emulator answers 400.
