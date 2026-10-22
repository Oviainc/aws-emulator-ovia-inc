Issue-Source: trace-sweep-divergence
Issue-Trace: elementalinference-feed-dictionary-subtitling-reference
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elementalinference-feed-dictionary-subtitling-reference elementalinference-dictionary-error-paths elementalinference-feed-association-error-paths elementalinference-list-dictionaries-and-feeds-pagination elementalinference-feed-lifecycle-associate-disassociate
Issue-Shape-Id: com.amazonaws.elementalinference#ElementalInference
Issue-Key: elemental-inference|GetFeed|2xx->2xx|$.dataEndpoints[*]
Issue-Key: elemental-inference|GetFeed|2xx->2xx|$.inputs
Issue-Key: elemental-inference|GetFeed|2xx->2xx|$.outputs[*].outputConfig.subtitling.aspectRatio
Issue-Key: elemental-inference|GetFeed|2xx->2xx|$.outputs[*].outputConfig.subtitling.audioInput
Issue-Key: elemental-inference|GetFeed|2xx->2xx|$.tags
Issue-Key: elemental-inference|CreateDictionary|2xx->2xx|$.tags
Issue-Key: elemental-inference|CreateFeed|2xx->2xx|$.dataEndpoints[*]
Issue-Key: elemental-inference|CreateFeed|2xx->2xx|$.outputs[*].outputConfig.cropping.audioInput
Issue-Key: elemental-inference|CreateFeed|2xx->2xx|$.tags
Issue-Key: elemental-inference|CreateFeed|2xx->2xx|$.outputs[*].outputConfig.subtitling.aspectRatio
Issue-Key: elemental-inference|CreateFeed|2xx->2xx|$.outputs[*].outputConfig.subtitling.audioInput
Issue-Key: elemental-inference|GetFeed|2xx->2xx|$.outputs[*].outputConfig.cropping.audioInput
Issue-Key: elemental-inference|GetFeed|2xx->2xx|$.outputs[*].outputConfig.clipping.audioInput
Issue-Key: elemental-inference|CreateDictionary|2xx->2xx|$.id
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# Feed and dictionary responses drop modeled members and emit non-AWS endpoints and ids

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFeed

## Description
The ElementalInference feed/dictionary response serializers emit a partial resource view: several members AWS always returns are absent, the data endpoint uses a domain template that is not the service's, and the generated resource ids are constants. Five distinct causes, all on the response-construction side of CreateDictionary / CreateFeed / GetFeed / UpdateFeed. All HTTP statuses on these steps already match (202/200), so clients fail on the body, not the envelope.

Cause 1 of 5 - an empty tag set is omitted instead of serialized as an empty map.
AWS: Smithy: CreateDictionaryResponse.tags, CreateFeedResponse.tags, GetFeedResponse.tags, UpdateFeedResponse.tags, GetDictionaryResponse.tags all target TagMap (map of TagKey->TagValue, not required). The model does not state emptiness behavior, so recorded behavior only: every recorded response for an untagged resource carries "tags": {} rather than omitting the member.
Emulator: the member is absent from the JSON body (replay reports `$.tags: missing (expected {}, got None)`).
Why: an SDK/CDK client that reads response.tags gets null/None instead of an empty collection and either NPEs or treats "no tags" as "tags unknown"; diffing tools see a member appear the moment the first tag is added.
Fix: the response serializer for the dictionary and feed shapes - serialize TagMap members that are present-but-empty rather than skipping empty maps.
Traces: elementalinference-dictionary-error-paths#setup#1, elementalinference-list-dictionaries-and-feeds-pagination#setup#1, elementalinference-feed-association-error-paths#task#1, elementalinference-feed-dictionary-subtitling-reference#task#3 and #task#4, elementalinference-feed-dictionary-subtitling-reference#task#6.

Cause 2 of 5 - the feed's default audio input is never materialized, so GetFeed has no `inputs`.
AWS: the on-disk model's GetFeedResponse declares arn/name/id/dataEndpoints/outputs/accessRoleArn/status/association/tags and no `inputs` member, so recorded behavior only: the recorded GetFeed body carries an `inputs` object listing the feed's audio inputs, e.g. {"audio": [{"name": "default-audio"}]}, created implicitly when the feed is created.
Emulator: `$.inputs: missing (expected {'audio': [{'name': 'default-audio'}]}, got None)` on every GetFeed - the feed record has no inputs collection at all.
Why: a client that resolves an output's audio source by looking it up in the feed's input list finds no inputs and cannot address the stream it is supposed to send media to.
Fix: feed state - CreateFeed must create the implicit default audio input and GetFeed/UpdateFeed must serialize it.
Traces: elementalinference-feed-dictionary-subtitling-reference#task#4, elementalinference-feed-lifecycle-associate-disassociate#task#2 and #task#5.

Cause 3 of 5 - output configuration defaults are not applied when an output is created.
AWS: Smithy: GetOutput.outputConfig targets OutputConfig (the union of cropping/clipping/subtitling configs) and is required; the model does not declare the defaults, so recorded behavior only: a created output comes back with its audio source resolved (outputConfig.<kind>.audioInput = the feed's default audio input name) and, for a subtitling output, with the default frame geometry (outputConfig.subtitling.aspectRatio = {"height": 9, "width": 16}) even when the create request omitted both.
Emulator: those members are absent from the created and the re-read output - `$.outputs[0].outputConfig.cropping.audioInput: missing (expected 'default-audio', got None)`, `$.outputs[1].outputConfig.clipping.audioInput: missing`, `$.outputs[0].outputConfig.subtitling.audioInput: missing`, `$.outputs[0].outputConfig.subtitling.aspectRatio: missing (expected {'height': 9, 'width': 16}, got None)`. The emulator echoes only what the request carried.
Why: the audioInput/aspectRatio members are how a client learns what the service actually configured; reading them back as null makes a round-trip (GetFeed -> edit -> UpdateFeed, which is a full-replacement PUT) drop the service-chosen configuration.
Fix: the CreateFeed / AssociateFeed / UpdateFeed handlers - populate output-config defaults when building the stored output, not just on echo.
Traces: elementalinference-feed-association-error-paths#task#1, elementalinference-feed-dictionary-subtitling-reference#task#3 and #task#4, elementalinference-feed-lifecycle-associate-disassociate#task#1, #task#2 and #task#5.

Cause 4 of 5 - dataEndpoints uses the control-plane domain and the feed id instead of the service's data-plane endpoint template.
AWS: Smithy: CreateFeedResponse.dataEndpoints / GetFeedResponse.dataEndpoints target StringList and are required, with no format trait, so recorded behavior only: each recorded entry is a data-plane URL of the form https://<opaque-endpoint-label>.elemental-inference-data.<region>.api.aws/ - a dedicated `elemental-inference-data.<region>.api.aws` domain, with a trailing slash, and a host label that is NOT the feed id.
Emulator: it emits https://<feedId>.data.elemental-inference.<region>.amazonaws.com - the control-plane service domain, no trailing slash, and the feed id as the host label.
Why: the generated label itself is environment-specific and not a requirement, but the domain template is: a client that routes media by matching/parsing the data endpoint's domain suffix, or that concatenates a path onto the returned URL (relying on the trailing slash), builds the wrong target.
Fix: the feed serializer / endpoint construction for dataEndpoints.
Traces: elementalinference-feed-association-error-paths#task#1, elementalinference-feed-dictionary-subtitling-reference#task#3, #task#4 and #task#6, elementalinference-feed-lifecycle-associate-disassociate#task#1, #task#2 and #task#5.

Cause 5 of 5 - generated dictionary (and feed) ids are constants, so distinct resources collide on one id.
AWS: Smithy: CreateDictionaryResponse.id targets DictionaryId (required, @length 1-19, @pattern ^[a-zA-Z0-9]+$) and FeedId is @pattern ^[a-z0-9]{19}$ - an id identifies exactly one resource, so two creates must not return the same id. The concrete value is generated and never a requirement.
Emulator: two CreateDictionary calls in the same fixture both return the same id (replay: `$.id: value_mismatch (expected '<shaped-not-asserted>', got 'PLBpWchG2snckDNKokg')` on the second create; the same id also appears for the dictionary in an unrelated fixture, and every feed in the corpus is id 53prtya0vie7788nfh5), i.e. the id generator is deterministic and not per-resource.
Why: a client that creates two dictionaries gets one id back twice - the second create silently aliases the first, lists/pagination cannot distinguish the resources, and a Get/Update/Delete by id addresses the wrong one.
Fix: dictionary/feed state - derive ids from a per-resource unique source instead of a fixed seed.
Traces: elementalinference-list-dictionaries-and-feeds-pagination#setup#1 (affected also task#2).

## Evidence
Replay of simparity-wire-aws-v1 (8 fixtures, 54 steps, score 37%). Representative step elementalinference-feed-dictionary-subtitling-reference#task#4 (GetFeed), expected 200 / actual 200, body divergences: `$.dataEndpoints[0]: missing (expected 'https://766eefa96a.elemental-inference-data.us-west-2.api.aws/', got 'https://53prtya0vie7788nfh5.data.elemental-inference.us-west-2.amazonaws.com')`; `$.inputs: missing (expected {'audio': [{'name': 'default-audio'}]}, got None)`; `$.outputs[0].outputConfig.subtitling.aspectRatio: missing (expected {'height': 9, 'width': 16}, got None)`; `$.outputs[0].outputConfig.subtitling.audioInput: missing (expected 'default-audio', got None)`; `$.tags: missing (expected {}, got None)`. Same missing-member set on CreateFeed (feed-association-error-paths#task#1, feed-dictionary-subtitling-reference#task#3, feed-lifecycle-associate-disassociate#task#1, 202/202), on GetFeed (feed-lifecycle-associate-disassociate#task#2 and #task#5, 200/200, including `$.outputs[1].outputConfig.clipping.audioInput: missing`), and on CreateDictionary (dictionary-error-paths#setup#1 and list-dictionaries-and-feeds-pagination#setup#1, 202/202, `$.tags: missing`, plus `$.id` repeated across creates).

## Steps to Reproduce
1. Replay fixture elementalinference-feed-dictionary-subtitling-reference, step task#4 (GET /v1/feed/{id}) after its CreateFeed setup: the 200 body has no `inputs`, no `tags`, no outputs[0].outputConfig.subtitling.audioInput/aspectRatio, and dataEndpoints[0] on the *.data.elemental-inference.<region>.amazonaws.com domain.
2. Replay elementalinference-feed-lifecycle-associate-disassociate#task#1/#task#2/#task#5 for the same members on CreateFeed/GetFeed with cropping and clipping outputs.
3. Replay elementalinference-list-dictionaries-and-feeds-pagination#setup#1 and #task#2 (POST /v1/dictionary twice): both responses omit `tags` and return the identical `id`.
