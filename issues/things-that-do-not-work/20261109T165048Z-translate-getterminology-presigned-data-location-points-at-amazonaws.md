Issue-Source: trace-sweep-divergence
Issue-Trace: translate-terminology-import-creates-resource-with-expected-properties
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: translate-terminology-import-creates-resource-with-expected-properties translate-terminology-with-translation
Issue-Shape-Id: com.amazonaws.translate#AWSShineFrontendService_20170701
Issue-Key: translate|GetTerminology|2xx->2xx|$.TerminologyDataLocation.Location
Issue-Related: 20261029T184304Z-translate-success-payloads-carry-wrong-terminology-arn-and.md
Issue-Commit: 67d35df6c1acce919d84587dd79dfbb9f77fdc15

# Translate GetTerminology presigned data location points at amazonaws.com instead of the emulator

> _Note: this issue was identified against the code base at commit `67d35df6c1acce919d84587dd79dfbb9f77fdc15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTerminology

## Description
Two net-new causes in Amazon Translate's GetTerminology 200-response payload, both the SAME shared fault: the TerminologyDataLocation.Location the emulator returns is a presigned URL whose host is a real `*.s3.<region>.amazonaws.com` endpoint, i.e. an address the emulator neither owns nor serves, instead of the data endpoint the request was reached on. Status is 200 on both sides; only the announced location host diverges. A fixer must close BOTH causes (both fixtures/steps below) before submitting.

Cause 1 of 2 - GetTerminology after an ImportTerminology in the same fixture announces a location on an S3 host the emulator does not serve.
AWS: Smithy: GetTerminology output TerminologyDataLocation (members RepositoryType and Location, both @required, String @length 0..10000, @pattern ^[\P{M}\p{M}]{0,10000}$). Location is documented as "The Amazon S3 location of the most recent custom terminology input file that was successfully imported into Amazon Translate. The location is returned as a presigned URL that has a 30-minute expiration." The REQUIREMENT is that the returned URL is a presigned URL that the serving endpoint actually honours, so a client can GET the terminology input file from it; the concrete host, bucket name, object key, expiry and signature are per-environment values and are not asserted by the fixture (expected '<shaped-not-asserted>'). For an emulator that means the announced host must be the endpoint the caller reaches the emulator on, not a public AWS host. Recorded behavior only for the exact URL layout.
Emulator: returns Location = `https://translate-terminology-<accountId>-<region>.s3.<region>.amazonaws.com/<name>/<revision>/<name>.csv?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Expires=1800&X-Amz-SignedHeaders=host&X-Amz-Signature=<sha256>`. The replay classified this as `host_not_own`: the emulator announced an address other than the one it is reached on.
Why: a client that follows the documented workflow - call GetTerminology, then download the input file from the returned presigned URL - is sent off the emulator to the public internet. Either the request escapes the sandbox to real AWS S3 (where the bucket does not exist and the synthetic signature is invalid, so it fails with NoSuchBucket/SignatureDoesNotMatch) or it is refused by egress control. Either way the terminology file can never be retrieved, and the operation's only useful output member is a dead link.
Fix: the location construction in the translate service's state - `presigned_location` builds the URL from a hardcoded `https://{bucket}.s3.{region}.amazonaws.com/...` template; it must announce the emulator's own data endpoint (the host/port the request arrived on) and the emulator must serve that path, so the URL the two `TerminologyDataLocation` helpers (`terminology_data_location`, `auxiliary_data_location`) hand back is fetchable. This is a serializer/state concern, not a per-operation one: both GetTerminology and ImportTerminology report locations through it.
Traces: translate-terminology-import-creates-resource-with-expected-properties#task#2. NOTE: this cluster ALSO carries a $.TerminologyProperties.Arn value_mismatch (trailing `/LATEST` replaced by a generated hex revision). That ARN cause is already owned by open issue 20261029T184304Z-translate-success-payloads-carry-wrong-terminology-arn-and.md (Cause 2 of 3) and is NOT re-filed here; only the Location host is net-new in this cluster.

Cause 2 of 2 - the same malformed location is announced by GetTerminology in the translation fixture, so the defect is in the shared location builder and not in one fixture's setup.
AWS: Smithy: same GetTerminology output member TerminologyDataLocation.Location, same documented contract (a presigned, retrievable S3 location with a 30-minute expiration). Fixture does not assert the URL's value ('<shaped-not-asserted>'); the requirement is that the host is one the responder serves.
Emulator: returns Location = `https://translate-terminology-<accountId>-<region>.s3.<region>.amazonaws.com/<name>/<revision>/<name>.csv?X-Amz-Algorithm=...&X-Amz-Expires=1800&X-Amz-SignedHeaders=host&X-Amz-Signature=...` again - `host_not_own`.
Why: read-after-import on any terminology, not just the one created by a single fixture, yields an unfetchable location, so no client flow that downloads a custom terminology can work against the emulator.
Fix: same `presigned_location` helper in the translate state module (one fix closes both causes, but both steps must be re-replayed to confirm it).
Traces: translate-terminology-with-translation#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, score_pct=36.6, 41 steps, 26 mismatched).

Cause 1 - fixture translate-terminology-import-creates-resource-with-expected-properties, step task#2, operation GetTerminology, status 200 expected / 200 actual.
  $.TerminologyDataLocation.Location: host_not_own - expected '<shaped-not-asserted>' (any location the responder itself serves), got 'https://translate-terminology-000000000000-us-west-2.s3.us-west-2.amazonaws.com/sim2real-term-e4bbe6fc-a046-401b-aaee-c45a106cc8ad/af2a4cf9964a8cbe/sim2real-term-e4bbe6fc-a046-401b-aaee-c45a106cc8ad.csv?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Expires=1800&X-Amz-SignedHeaders=host&X-Amz-Signature=7080e80e...'. The account id, region, terminology name, revision segment and signature in that string are per-run values; the defect is the `*.s3.<region>.amazonaws.com` host, which is not the endpoint the replay reached the emulator on (the data listener that answered the request).
  (Same cluster also reported $.TerminologyProperties.Arn expected '.../LATEST', got '.../af2a4cf9964a8cbe' - already owned by 20261029T184304Z-translate-success-payloads-carry-wrong-terminology-arn-and.md.)

Cause 2 - fixture translate-terminology-with-translation, step task#2, operation GetTerminology, status 200 expected / 200 actual.
  $.TerminologyDataLocation.Location: host_not_own - expected '<shaped-not-asserted>', got 'https://translate-terminology-000000000000-us-west-2.s3.us-west-2.amazonaws.com/sim2real-probe-translate-term-46a0b5f3/d5ac409c343a10ae/sim2real-probe-translate-term-46a0b5f3.csv?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Expires=1800&X-Amz-SignedHeaders=host&X-Amz-Signature=59ec2cfd...'.

Source of the announced host (for orientation only; the checkout may have drifted): services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/src/state.rs lines 488-513 - `presigned_location` formats 'https://{bucket}.s3.{region}.amazonaws.com/{key}?...' and is the single builder used by `terminology_data_location` (state.rs:504) and `auxiliary_data_location` (state.rs:519); GetTerminology calls it at services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/src/ops/get_terminology.rs:193.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: fixture translate-terminology-import-creates-resource-with-expected-properties, step task#2 - ImportTerminology a CSV terminology, then GetTerminology by that name and inspect TerminologyDataLocation.Location. Its host must be the endpoint the request was sent to (the emulator's own data listener), and an HTTP GET of that URL must return the imported terminology file. Observed: the host is '<bucket>.s3.<region>.amazonaws.com', which the emulator does not serve.
4. Cause 2: fixture translate-terminology-with-translation, step task#2 - GetTerminology for the terminology imported earlier in that fixture and inspect TerminologyDataLocation.Location the same way; same amazonaws.com host is announced.
5. In both cases also confirm RepositoryType is still 'S3' and the status stays 200 - only the announced host is at fault.
