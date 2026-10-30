Issue-Source: trace-sweep-divergence
Issue-Trace: route53-cidr-collection-change-and-delete
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-cidr-collection-change-and-delete route53-cidr-collection-lifecycle route53-cidr-collection-management route53-cidr-collection-name-conflict-error
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53|CreateCidrCollection|2xx->2xx|$.CreateCidrCollectionResponse.Collection.Arn
Issue-Commit: 45d3476fddc0c303f86586399cfbfaaac87d2cc4

# Route 53 CIDR collection ARNs embed an account id where AWS leaves the account field empty

> _Note: this issue was identified against the code base at commit `45d3476fddc0c303f86586399cfbfaaac87d2cc4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCidrCollection

## Description
Cause 1 of 1: CreateCidrCollection returns a Collection.Arn that carries a populated account-id field, where Route 53 emits a global-service ARN whose region AND account fields are both empty.

AWS: Route 53 is a global (partition-scoped) service, so every Route 53 resource ARN has an empty region segment and an empty account segment. For a CIDR collection the recorded response body is `arn:aws:route53:::cidrcollection/<collection-id>` — the fixed text is `arn:aws:route53:::cidrcollection/`, and the only variable part is the collection's own Id (the UUID the same response returns in Collection.Id). Smithy: route53 CidrCollection.Arn targets ARN with @length min 20 max 2048 and pattern `\S`, documented as "The ARN of the collection. Can be used to reference the collection in IAM policy or in another Amazon Web Services account" — the model constrains the string but not its layout, so the four-colon/no-account layout is recorded behavior only (fixture route53-cidr-collection-change-and-delete, step setup#1, HTTP 201).

Emulator: the same 201 response body carries `arn:aws:route53::000000000000:cidrcollection/<uuid>`, i.e. the account field is filled with the emulator's account number instead of being left empty. The status (201) and every other member of the envelope match; only the ARN's shape diverges. The replay reports it as `$.CreateCidrCollectionResponse.Collection.Arn: value_mismatch (expected 'arn:aws:route53:::cidrcollection/{{cidr_collection_id}}', got 'arn:aws:route53::000000000000:cidrcollection/eb9b0c5a-25f3-e2f7-5bef-4a4920fa58f2')` — the UUID and the account digits are per-run values; the defect is the extra non-empty account segment. The same wrong ARN is produced on all four fixtures that create a CIDR collection, so it is the construction of the ARN, not one fixture's data.

Why: the ARN is the documented handle for referencing the collection from an IAM policy or from another account. A client that copies this ARN into a policy Resource element, or compares it against an ARN it built itself or received from another Route 53 response (hosted-zone and health-check ARNs are formed the same account-less way), gets a value that does not match the real one — IAM statements written against the emulator's ARN do not authorize anything on AWS, and ARN-equality or ARN-parsing checks (splitting on ':' and expecting an empty account field for a global service) fail or mis-read the collection id.

Fix: the CreateCidrCollection handler's response construction / ARN formatting for Route 53 resources — Route 53 ARNs must be rendered with both the region and the account-id fields empty (`arn:<partition>:route53:::cidrcollection/<id>`), rather than through the generic regional/account-qualified ARN builder. Any read path that projects the stored collection (ListCidrCollections / the change and delete paths in the same fixtures) must render it identically.

Traces: route53-cidr-collection-change-and-delete#setup#1 (representative); same cluster also at route53-cidr-collection-lifecycle#setup#1, route53-cidr-collection-management#setup#1, route53-cidr-collection-name-conflict-error#setup#1.

No open issue covers this: the existing route53 wire issues cover authorization/input gates (20261029T220729Z-route53-route-53-accepts-five-requests-aws-refuses-missing.md), refusal wording (…refusal-messages-are-emulator-authored-instead.md), HostedZone.Features/MaxItems envelopes (…response-envelopes-omit-hostedzone-features-and.md), health-check config defaults, static reference catalogs and traffic-policy not-found messages — none of them concerns CIDR collection ARN formatting.

## Evidence
Fixture route53-cidr-collection-change-and-delete, step setup#1 (CreateCidrCollection, HTTP 201 expected and 201 received).
Expected body member $.CreateCidrCollectionResponse.Collection.Arn: 'arn:aws:route53:::cidrcollection/{{cidr_collection_id}}' — fixed text 'arn:aws:route53:::cidrcollection/' plus the collection Id returned in the same response.
Actual: 'arn:aws:route53::000000000000:cidrcollection/eb9b0c5a-25f3-e2f7-5bef-4a4920fa58f2' — an account-id segment ('000000000000') is present where AWS's global-service ARN has an empty field. Verdict: mismatch; divergence_key service=route53|CreateCidrCollection|status=201->201|keys=$.CreateCidrCollectionResponse.Collection.Arn.
Cluster size 4 (affected_count 4): the same divergence on route53-cidr-collection-lifecycle#setup#1, route53-cidr-collection-management#setup#1 and route53-cidr-collection-name-conflict-error#setup#1.
Smithy (services/service-com-amazonaws-route53-awsdnsv20130401): CidrCollection.Arn -> ARN, @length 20..2048, pattern '\S'; no layout constraint, so the account-less layout is taken from the recording.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture route53-cidr-collection-change-and-delete.
2. At step setup#1 the fixture calls Route 53 CreateCidrCollection (POST /2013-04-01/cidrcollection) with a CallerReference and a collection Name.
3. Read Collection.Arn out of the 201 CreateCidrCollectionResponse body.
4. Observe it is 'arn:aws:route53::<account>:cidrcollection/<Collection.Id>'; AWS returns 'arn:aws:route53:::cidrcollection/<Collection.Id>' with the region and account fields both empty.
5. The same ARN layout is returned by route53-cidr-collection-lifecycle#setup#1, route53-cidr-collection-management#setup#1 and route53-cidr-collection-name-conflict-error#setup#1.
