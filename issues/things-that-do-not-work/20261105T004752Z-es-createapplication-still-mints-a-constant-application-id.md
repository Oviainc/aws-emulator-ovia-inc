Issue-Source: trace-sweep-divergence
Issue-Trace: esdef-application-dead-id-probe
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: esdef-application-dead-id-probe
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|CreateApplication|2xx->2xx|$.arn
Issue-Key: es|CreateApplication|2xx->2xx|$.id
Issue-Commit: 322c061674ce7e9dd4c51976f24d3ae59dff0416

# es CreateApplication still mints a constant application id, so the minted id and derived ARN diverge

> _Note: this issue was identified against the code base at commit `322c061674ce7e9dd4c51976f24d3ae59dff0416`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
ONE root cause, reproduced by 1 divergence cluster (2 asserted keys: $.id and $.arn).

Cause 1 of 1: CreateApplication does not mint a per-application identity - it returns a fixed, filler-padded application id, and the application ARN it serializes is derived from that same constant.

AWS: Smithy com.amazonaws.opensearch#CreateApplication (restJson1, POST /2021-01-01/opensearch/application, success code 200) returns output member `id` -> shape `Id` with @pattern ^[a-z0-9]{3,30}$, and `arn` -> shape `ARN` with @length(min 20, max 2048). The model constrains the id's alphabet and length but says nothing about how it is produced; for that part this is recorded behavior only: every recorded CreateApplication answers 200 with a freshly minted id unique to the application just created, and with arn = arn:aws:<namespace>:<region>:<account-id>:application/<that id>. The region, account id and the id itself are per-request/per-environment values, which is exactly why this fixture asserts $.id and $.arn BY SHAPE ('<shaped-not-asserted>') rather than by literal: the requirement is that the served identity has AWS's shape and is newly minted for this application, not that it equals any one recorded string. The status and the body's other members are not part of this divergence - the step expected 200 and got 200, and the ARN's service-namespace token the emulator now emits ('opensearch') matches the recording.

Emulator: the 200 body carries id 'sfi9sv1baaaa' and arn 'arn:aws:opensearch:us-west-2:000000000000:application/sfi9sv1baaaa'; replay reports both keys as value_mismatch against the shape assertion ($.id and $.arn). 'sfi9sv1baaaa' is the identical literal already recorded as the evidence for Cause 1 of the RESOLVED issue 20261029T190009Z-es-application-plane-mints-constant-ids-wrong-arns.md ('CreateApplication mints a constant id ... the same 'sfi9sv1baaaa' reappears for unrelated creates in opensearch-application-capability-lifecycle#setup#1 and opensearch-switch-default-application-between-two#setup#1/#setup#2'). The other eight causes of that resolved issue no longer diverge in this replay - DeleteApplication, PutDefaultApplicationSetting, the duplicate-name and malformed-ARN refusals and DescribeInsightDetails all pass now - so the constant-id generator is the one part of that fix that did not land, and the resolved issue no longer covers it. The trailing 'aaaa' is filler, not entropy: the generator pads a fixed stem to the model's length instead of drawing a fresh id.

Why: a client that creates two applications in one session receives one identity for both. The second create silently aliases the first, and every subsequent GetApplication, UpdateApplication, DeleteApplication, PutDefaultApplicationSetting and StartMigration addressed by that id operates on the wrong object; a default-application setting written for one application reads back as set for the other. An ARN captured from a create and stored in an IAM policy, a cross-service reference or a test fixture collides with the ARN of every other application the emulator ever mints, so no scenario can hold two distinct applications at once.

Fix: the CreateApplication handler's identity minting in the opensearch (es) application state - the generator that assigns Application.id must draw a fresh value per application within the modeled alphabet/length - and the ARN builder that derives `arn` from that id (its namespace token is already correct, so only the id it interpolates is wrong).

Traces: esdef-application-dead-id-probe#task#1 (CreateApplication, status 200 expected and served; keys $.id and $.arn).

## Evidence
Replay detail for esdef-application-dead-id-probe#task#1 (divergence_key service=es|CreateApplication|status=200->200|keys=$.arn,$.id):
  "$.arn: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:opensearch:us-west-2:000000000000:application/sfi9sv1baaaa')"
  "$.id: value_mismatch (expected '<shaped-not-asserted>', got 'sfi9sv1baaaa')"
expected_status 200, actual_status 200 - no status, error-code or message divergence in this cluster; the two asserted identity members are the whole finding.

Fixed text vs variable parts of the served ARN: 'arn:aws:' + the namespace token + ':' + region + ':' + account id + ':application/' + id is the template; 'us-west-2', '000000000000' and the id are per-environment/per-request values and are NOT requirements. The requirement is that the id segment is a freshly minted, per-application value matching com.amazonaws.opensearch#Id @pattern ^[a-z0-9]{3,30}$, and that the ARN interpolates that same fresh id.

Smithy cited: com.amazonaws.opensearch#CreateApplication output members id (target Id, @pattern ^[a-z0-9]{3,30}$) and arn (target ARN, @length 20..2048); service com.amazonaws.opensearch#AmazonOpenSearchService (sdkId OpenSearch, endpointPrefix es, protocol restJson1).

Related: resolved issue 20261029T190009Z-es-application-plane-mints-constant-ids-wrong-arns.md (its Cause 1 is this same generator; it is resolved and therefore does not cover this failure). The replay reports owned_by=[] and stub_path="" for this cluster, so no open issue carries its replay key.

## Steps to Reproduce
1. Replay fixture esdef-application-dead-id-probe, step task#1: POST /2021-01-01/opensearch/application with a valid `name`.
2. Observe the 200 body's `id` is the constant 'sfi9sv1baaaa' and `arn` is 'arn:aws:opensearch:<region>:<account>:application/sfi9sv1baaaa'; both fail the fixture's shape assertion on $.id and $.arn.
3. Confirm the constancy directly: issue two CreateApplication calls with different `name` values in one session (as opensearch-switch-default-application-between-two does at setup#1 and setup#2) and observe both responses carry the same `id` and the same `arn`.
4. Then GetApplication with that id after creating the second application: the record returned is the aliased one, not the application the id was minted for.
