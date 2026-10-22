Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.pull-time-update-exclusion-fields-preserved ecr.register-pull-time-exclusion-duplicate-rejected ecr.e2e-registry-policy-cross-account-pull-consequence ecr.e2e-signing-and-status-flow ecr.e2e-signing-configuration-drives-signing-status ecr.signing-configuration-rule-set-round-trip
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr.pull-time-update-exclusion-fields-preserved|run#5|ResponseParserError|role = iam.create_role(
Issue-Key: ecr.register-pull-time-exclusion-duplicate-rejected|_create_iam_role#3|ResponseParserError|resp = iam.create_role(
Issue-Key: ecr.e2e-registry-policy-cross-account-pull-consequence|run#7|ResponseParserError|role = iam.create_role(
Issue-Key: signer|PutSigningProfile|AmbiguousService
Issue-Commit: ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3

# Router refuses every 'iam' and every 'signer' sigv4 request as AmbiguousService (same signing name registered twice), and answers the XML-protocol iam client with a JSON body

## Resolution

Both causes are closed at the request-claiming seam: each emulated service now claims only
the requests addressed to its own endpoint — the routing key must name it (its SigV4 signing
name, or its own endpoint prefix) and the request must address its own URI path space — so
exactly one service claims an `iam`-signed or a `signer`-signed request and no
`AmbiguousService` refusal can be produced for either name. `signer` requests now reach AWS
Signer (PutSigningProfile and the profile reads answer 200) and `GET /revocations` reaches the
Signer data plane that implements it; `iam` requests reach AWS IAM and are answered inside
IAM's own awsQuery XML envelope, so a query/XML client parses the answer instead of raising a
malformed-response error on a JSON `__type` body. Pinned by
`core/router/tests/all/iam_and_signer_signing_name_routing.rs`.

Two parts of the report's "Fix" text are deliberately not covered here, and the three Cause-1
ECR traces (`ecr.pull-time-update-exclusion-fields-preserved`,
`ecr.register-pull-time-exclusion-duplicate-rejected`,
`ecr.e2e-registry-policy-cross-account-pull-consequence`) therefore still fail — now on a
different, separately tracked cause rather than on an unparseable response:

* every AWS IAM operation, `CreateRole` included, is still a scaffold answering HTTP 501
  `NotImplemented` (in IAM's XML envelope). Implementing it belongs to that operation's own
  stub issue, so a check that needs a real role ARN fails there;
* the router's generic `AmbiguousService` envelope is rendered as JSON whatever the target
  protocol is, and that renderer lives in the read-only `vendor/smithy-server-runtime` router,
  which cannot be changed from inside the emulator. With the duplicate claims removed the
  refusal is unreachable for these two signing names, which is what removes the symptom.

> _Note: this issue was identified against the code base at commit `ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRole

## Description
Six ECR behavioral checks never reach ECR because a supporting call is refused by the emulator's request router with a non-AWS `AmbiguousService` error: the router finds more than one emulated service claiming the request's sigv4 signing name and rejects rather than choosing. The duplicate names are reported as identical strings ("iam, iam" and "signer, signer"), i.e. the same signing name is registered by two different service crates. Two causes, both must be closed.

Note: an open issue filed from another sweep, "Router refuses every sigv4 'iam' request as AmbiguousService (iam vs iam-toolbox signing-name collision), in a non-AWS JSON body" (20261021T234042Z-osis-router-refuses-every-sigv4-iam-request-as-ambiguousservice.md), describes the same iam collision from the osis suite; it does not carry these ECR replay keys, so Cause 1 is recorded here with them. The signer collision (Cause 2) is not covered by any open issue.

Cause 1 of 2: 'iam' is claimed twice, so every IAM request is refused - and the refusal is serialized as JSON to an XML-protocol client.
AWS: IAM is reachable and CreateRole (awsQuery protocol, XML responses) returns a CreateRoleResult containing Role.Arn; there is no AWS error named `AmbiguousService`. Recorded behavior only for the router layer: a single emulated service must claim the signing name 'iam' and serve the request.
Emulator: the response body is b'{"__type":"AmbiguousService","message":"2 emulated services all claim requests for \'iam\': iam, iam; the request is rejected rather than served by an arbitrary one"}'. Two problems compound: (a) no IAM operation can be served at all, and (b) the refusal is emitted as a JSON `__type` envelope to a query/XML-protocol client, so botocore cannot even parse it - it raises ResponseParserError ("invalid XML received") instead of surfacing an error code, and the SDK retries it as a transient parse failure.
Why: any workflow that creates an IAM role/user before exercising another service dies at the first IAM call with an unparseable response - the client sees a malformed-response exception rather than a modeled error, so no error handling path can recover. Here it blocks three ECR checks that only need an IAM role ARN as a principal.
Fix: the service registry / request router that resolves a sigv4 signing name to an emulated service (deduplicate the two registrations claiming 'iam' so exactly one serves it), and the router's error serializer, which must answer in the resolved client's protocol (XML for awsQuery services) rather than always emitting a JSON `__type` body.
Traces: ecr.pull-time-update-exclusion-fields-preserved, ecr.register-pull-time-exclusion-duplicate-rejected, ecr.e2e-registry-policy-cross-account-pull-consequence.

Cause 2 of 2: 'signer' is claimed twice, so every AWS Signer request is refused.
AWS: PutSigningProfile is served by AWS Signer (restJson) and returns the profile's ARN/version; there is no AWS error named `AmbiguousService`. Recorded behavior only for the router layer: one emulated service must claim 'signer'.
Emulator: ClientError code `AmbiguousService`, message "2 emulated services all claim requests for 'signer': signer, signer; the request is rejected rather than served by an arbitrary one", and the SDK exhausts its retries (reached max retries: 3) because the code is not classified.
Why: every Signer operation is unreachable, so ECR's signing-configuration behavior (which needs a signing profile to point at) cannot be exercised at all; a client gets an unknown error code after burning its full retry budget instead of a served response or a modeled error.
Fix: the same service registry / router signing-name resolution - two crates currently register the signing name 'signer'; exactly one must claim it.
Traces: ecr.e2e-signing-and-status-flow, ecr.e2e-signing-configuration-drives-signing-status, ecr.signing-configuration-rule-set-round-trip.

## Evidence
Cause 1 (iam) - three ecr checks, identical body:
  botocore.parsers.ResponseParserError: Unable to parse response (not well-formed (invalid token): line 1, column 0), invalid XML received. Further retries may succeed:
  b'{"__type":"AmbiguousService","message":"2 emulated services all claim requests for \'iam\': iam, iam; the request is rejected rather than served by an arbitrary one"}'
  raised from iam.create_role(...) in ecr.pull-time-update-exclusion-fields-preserved (the check only needs the role ARN as principalArn), and from the equivalent IAM setup calls in ecr.register-pull-time-exclusion-duplicate-rejected and ecr.e2e-registry-policy-cross-account-pull-consequence.
  Expected: a parseable XML CreateRoleResponse with Role.Arn (AWS has no AmbiguousService error).
  Actual: a JSON `__type` body delivered to an XML-protocol client -> unparseable, no error code reaches the caller.

Cause 2 (signer) - three ecr checks:
  botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the PutSigningProfile operation (reached max retries: 3): 2 emulated services all claim requests for 'signer': signer, signer; the request is rejected rather than served by an arbitrary one
  in ecr.e2e-signing-and-status-flow, ecr.e2e-signing-configuration-drives-signing-status, ecr.signing-configuration-rule-set-round-trip.
  Expected: PutSigningProfile served by the Signer emulator.
  Actual: error code AmbiguousService after 3 retries; no Signer operation is reachable.

## Steps to Reproduce
Cause 1:
1. Boot the emulator; point a botocore `iam` client at it.
2. iam.create_role(RoleName='sim2real-ecr-ptu-<unique>', AssumeRolePolicyDocument='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ecr.amazonaws.com"},"Action":"sts:AssumeRole"}]}')
3. Observe botocore.parsers.ResponseParserError with the JSON `{"__type":"AmbiguousService"...}` body quoted above, instead of an XML CreateRoleResponse.

Cause 2:
1. Point a botocore `signer` client at the emulator.
2. signer.put_signing_profile(profileName='sim2real<unique>', platformId=<any>)
3. Observe ClientError code AmbiguousService ("2 emulated services all claim requests for 'signer': signer, signer") after the retry budget is exhausted.

Then re-replay the six checks in trace_ids.
