Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-discoverer-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-discoverer-lifecycle schemas-discoverer-errors
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|CreateDiscoverer|2xx->2xx|$.DiscovererArn
Issue-Key: schemas|CreateDiscoverer|2xx->2xx|$.DiscovererId
Issue-Key: schemas|CreateDiscoverer|4xx->4xx|$.Message
Issue-Key: schemas|DescribeDiscoverer|4xx->4xx|$.Message
Issue-Key: schemas|UpdateDiscoverer|4xx->4xx|$.Message
Issue-Key: schemas|StartDiscoverer|4xx->4xx|$.Message
Issue-Key: schemas|StopDiscoverer|4xx->4xx|$.Message
Issue-Key: schemas|DeleteDiscoverer|4xx->4xx|$.Message
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# Discoverer operations diverge from AWS on DiscovererId derivation and on both error sentences

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDiscoverer

## Description
Every discoverer operation the corpus exercises diverges, on three independent faults in the discoverer handler: the identifier CreateDiscoverer mints, the sentence a conflicting create is refused with, and the sentence the five id-addressed operations refuse a missing discoverer with. Seven causes, all in the schemas discoverer paths.

Cause 1 of 7 - CreateDiscoverer mints an unrelated opaque id. AWS: recorded behavior only - Smithy models DiscovererId as a plain __string with no pattern on CreateDiscoverer (POST /v1/discoverers, 201) and SourceArn as the only required member (__stringMin20Max1600). The recording shows the id derived from the SourceArn the request sent: for SourceArn arn:aws:events:us-west-2:000000000000:event-bus/default the service answered DiscovererId 'events-event-bus-default' and DiscovererArn arn:aws:schemas:<region>:<account>:discoverer/events-event-bus-default - i.e. the source ARN's service token followed by its resource path with the '/' separator rendered as '-'. Emulator: state.rs mint_discoverer_id() (~line 64) returns 32 lower-case hex digits from SHA-256 over a store-wide counter ('schemas-discoverer' || next_discoverer_seq), so the id carries nothing of the source and changes with the order creates happen in; it answered '719a2712e07eb39bd84b546ec26d21c0'. Why: the id is the handle every other discoverer operation addresses the resource by. Because AWS derives it from the source, a client can compute or recognise its own discoverer's id from the event bus it subscribed, and the id is stable across re-creation; against the emulator the same create yields a different id depending on how many discoverers were created before it, so a client that stores or reconstructs the id addresses nothing. Fix: the DiscovererId derivation used by the CreateDiscoverer handler (state.rs mint_discoverer_id and the ARN synthesis that embeds it). Traces: schemas-discoverer-lifecycle#task#1.

Cause 2 of 7 - CreateDiscoverer conflict sentence. AWS: Smithy models ConflictException (httpError 409) on CreateDiscoverer. The recording's 409 body words it 'Discoverer <discovererId> already exists.' - fixed text 'Discoverer ' + ' already exists.', with the one variable part being the DiscovererId of the discoverer already held for that source (the recorded step shows the id minted by the first create). Emulator: answers 409 with 'Discoverer for source <sourceArn> already exists.' - state.rs discoverer_already_exists() (~line 3929) builds the sentence from the request's SourceArn instead of the existing resource's id. Why: the status and code match, but a client that surfaces the message, or parses the id of the colliding resource out of it to switch to a describe-and-reuse path, gets the source ARN it already knew and no id. Fix: discoverer_already_exists() in state.rs, which must take the existing discoverer's id rather than the request's SourceArn. Traces: schemas-discoverer-lifecycle#task#2.

Causes 3-7 of 7 - the not-found sentence for the five id-addressed discoverer operations. AWS: Smithy models NotFoundException (httpError 404) on DescribeDiscoverer, UpdateDiscoverer, StartDiscoverer, StopDiscoverer and DeleteDiscoverer (each addressed by the DiscovererId httpLabel). The recording's 404 body words it 'Discoverer <DiscovererId> does not exist.' - fixed text 'Discoverer ' + ' does not exist.' with the request's DiscovererId as the only variable part. Emulator: answers 404 with 'Discoverer with ID <DiscovererId> does not exist.' - the extra 'with ID ' comes from state.rs discoverer_not_found() (~line 3917), which was written to the service's generic '«Resource» with «identifier» «value» does not exist.' template that registry_not_found/schema_not_found use; AWS does not use that template for a discoverer. Why: the message is part of the contract for this error class; a client or test asserting on the service's refusal text sees a sentence AWS never emits, and the five operations are wrong identically because they all go through this one helper. Cause 3 is DescribeDiscoverer (schemas-discoverer-errors#task#1, which also covers schemas-discoverer-lifecycle#teardown#10), cause 4 UpdateDiscoverer (#task#2), cause 5 StartDiscoverer (#task#3), cause 6 StopDiscoverer (#task#4), cause 7 DeleteDiscoverer (#task#5). Fix: discoverer_not_found() in state.rs - one helper, so one edit closes causes 3-7, but each must be re-replayed.

## Evidence
Replay results, all service=schemas:

CreateDiscoverer, fixture schemas-discoverer-lifecycle step task#1, expected 201 / actual 201: $.DiscovererId value_mismatch (expected 'events-event-bus-default', got '719a2712e07eb39bd84b546ec26d21c0'); $.DiscovererArn value_mismatch (expected 'arn:aws:schemas:us-west-2:000000000000:discoverer/events-event-bus-default', got 'arn:aws:schemas:us-west-2:000000000000:discoverer/719a2712e07eb39bd84b546ec26d21c0').

CreateDiscoverer, fixture schemas-discoverer-lifecycle step task#2, expected 409 / actual 409: $.Message value_mismatch (expected 'Discoverer 719a2712e07eb39bd84b546ec26d21c0 already exists.', got 'Discoverer for source arn:aws:events:us-west-2:000000000000:event-bus/default already exists.').

Fixture schemas-discoverer-errors, expected 404 / actual 404, $.Message value_mismatch in all five - expected 'Discoverer 00000000-0000-0000-0000-000000000000 does not exist.', got 'Discoverer with ID 00000000-0000-0000-0000-000000000000 does not exist.': task#1 DescribeDiscoverer (cluster also contains schemas-discoverer-lifecycle#teardown#10), task#2 UpdateDiscoverer, task#3 StartDiscoverer, task#4 StopDiscoverer, task#5 DeleteDiscoverer.

Code: services/service-com-amazonaws-schemas-schemas/src/state.rs mint_discoverer_id() line ~64 (SHA-256 over 'schemas-discoverer' || next_discoverer_seq, hex of the first 16 bytes); discoverer_not_found() line ~3917 (format!("Discoverer with ID {discoverer_id} does not exist.")); discoverer_already_exists() line ~3929 (format!("Discoverer for source {source_arn} already exists.")).

Smithy (services/service-com-amazonaws-schemas-schemas/smithy-model/com-amazonaws-schemas-schemas.json): CreateDiscoverer POST /v1/discoverers code 201, required SourceArn (__stringMin20Max1600), errors include ConflictException httpError 409; DescribeDiscoverer GET /v1/discoverers/id/{DiscovererId}, errors include NotFoundException httpError 404.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture schemas-discoverer-lifecycle step task#1: POST /v1/discoverers with SourceArn arn:aws:events:<region>:<account>:event-bus/default. Observe 201 whose DiscovererId is 32 hex digits instead of the id derived from the source ARN, and a DiscovererArn built from it.
3. Replay step task#2: repeat the same create. Observe 409 'Discoverer for source <arn> already exists.' instead of 'Discoverer <id> already exists.'
4. Replay fixture schemas-discoverer-errors steps task#1..task#5 (Describe/Update/Start/Stop/DeleteDiscoverer against an id the store does not hold). Observe 404 'Discoverer with ID <id> does not exist.' instead of 'Discoverer <id> does not exist.'
