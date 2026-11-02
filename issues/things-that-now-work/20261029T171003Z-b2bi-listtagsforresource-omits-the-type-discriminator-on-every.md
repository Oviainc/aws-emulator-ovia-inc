Issue-Source: trace-sweep-divergence
Issue-Trace: b2bi-profile-creation-time-tag-visibility
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: b2bi-profile-creation-time-tag-visibility b2bi-tagging-workflow
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi|ListTagsForResource|2xx->2xx|$.Tags[*].__type
Issue-Key: b2bi|ListTagsForResource|2xx->2xx|$.Tags[Key="env"].__type
Issue-Key: b2bi|ListTagsForResource|2xx->2xx|$.Tags[Key="team"].__type
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# b2bi ListTagsForResource omits the __type discriminator on every Tag entry

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Three net-new causes, one fault: the Tags list ListTagsForResource returns carries no per-entry __type discriminator.

AWS: Smithy services/service-com-amazonaws-b2bi-b2bi, protocol awsJson1_0, ListTagsForResource -> ListTagsForResourceResponse with a Tags list of com.amazonaws.b2bi#Tag (members Key and Value, both required). The recorded wire bodies carry a __type member on every Tag object, valued with the service's internal shape id com.amazonaws.visby.common#Tag; the Key/Value pairs themselves match.

Emulator: each Tags entry is serialized as a bare {Key, Value} object with no __type member, for every tag and regardless of how the tag was set (tags supplied at CreateProfile, tags added by TagResource, or the remainder after UntagResource).

Why: a client that discriminates union/structure values on the awsJson __type member — or any consumer comparing recorded bodies byte-for-byte against AWS — sees a different document shape than AWS sends for the same tag set, so tag round-trip verification against the emulator cannot reproduce AWS's body.

Fix: the b2bi response serializer for ListTagsForResourceResponse's Tags member (nested-structure __type emission), not the tag state itself — the Key/Value content is correct in all three causes.

Cause 1 of 3: ListTagsForResource on a profile created with one creation-time tag. AWS: the single Tags[0] object carries __type com.amazonaws.visby.common#Tag. Emulator: __type absent. Trace: b2bi-profile-creation-time-tag-visibility#task#3.

Cause 2 of 3: ListTagsForResource after TagResource added two tags. AWS: both entries (keys "env" and "team" in the recording) carry __type com.amazonaws.visby.common#Tag. Emulator: __type absent on both. Trace: b2bi-tagging-workflow#task#5.

Cause 3 of 3: ListTagsForResource after UntagResource removed one of the two tags. AWS: the surviving entry (key "env" in the recording) carries __type com.amazonaws.visby.common#Tag. Emulator: __type absent. Trace: b2bi-tagging-workflow#task#7. (The tag keys quoted here are request values, not part of the requirement; the requirement is that EVERY Tag entry carries the discriminator.)

## Evidence
Replay of simparity-wire-aws-v1. b2bi-profile-creation-time-tag-visibility#task#3 ListTagsForResource (200/200): $.Tags[0].__type missing, expected 'com.amazonaws.visby.common#Tag'. b2bi-tagging-workflow#task#5 (200/200): $.Tags[Key="env"].__type and $.Tags[Key="team"].__type both missing, expected 'com.amazonaws.visby.common#Tag'. b2bi-tagging-workflow#task#7 (200/200): $.Tags[Key="env"].__type missing, expected 'com.amazonaws.visby.common#Tag'. No other member of these three responses diverged.

## Steps to Reproduce
1. Boot the emulator from mainline. 2. Replay simparity-wire-aws-v1. 3. Replay b2bi-profile-creation-time-tag-visibility#task#3, b2bi-tagging-workflow#task#5 and b2bi-tagging-workflow#task#7. 4. Observe that each returned Tag object lacks the __type member present in the recorded AWS body.
