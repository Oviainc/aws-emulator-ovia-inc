Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medialive.deleted-channel-still-describes-as-deleted-and-deletes-again medialive.deleting-the-channel-detaches-its-input medialive.e2e-create-medialive-channel-lifecycle medialive.channel-omits-input-specification-and-defaults-log-level-disabled medialive.update-input-security-group-replaces-whitelist-and-keeps-tags medialive.url-pull-input-drops-the-security-groups-it-is-given medialive.signal-map-invalid-entry-point-rejected
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive.deleted-channel-still-describes-as-deleted-and-deletes-again|run#8|AssertionError|assert ch["Id"] not in [c["Id"] for c in _all(ctx, "list_channels", "Channels")], (
Issue-Key: medialive.deleting-the-channel-detaches-its-input|run#13|AssertionError|assert ch["Id"] not in listed, f"the deleted channel {ch['Id']!r} is still listed"
Issue-Key: medialive.e2e-create-medialive-channel-lifecycle|phase#9|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: medialive.channel-omits-input-specification-and-defaults-log-level-disabled|run#7|AssertionError|assert got.get("LogLevel") == "DISABLED", f"an omitted LogLevel reads {got.get('LogLevel')!r}"
Issue-Key: medialive.update-input-security-group-replaces-whitelist-and-keeps-tags|run#10|AssertionError|assert tags_now == ({"sim2real": marker}, {"sim2real": marker}), (
Issue-Key: medialive.url-pull-input-drops-the-security-groups-it-is-given|run#5|AssertionError|assert described["SecurityGroups"] == [], (
Issue-Key: medialive.signal-map-invalid-entry-point-rejected|run#7|AssertionError|assert message.startswith(prefix) and message.endswith(suffix), (
Issue-Commit: a03f808379986133e0f00cdb9a2ead2b78e9a26e

# medialive channel tombstones, security-group tags and pull-input security groups diverge: deleted channels stay listed, a re-delete 404s, and update-time tags overwrite

> _Note: this issue was identified against the code base at commit `a03f808379986133e0f00cdb9a2ead2b78e9a26e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteChannel

## Description
MediaLive's representation of deleted channels, of a security group's tags and of a pull input's security groups diverges from AWS, and the CreateSignalMap 404 is worded by the emulator. This bundle has 6 causes; all six must be closed.

Cause 1 of 6 - a deleted channel never leaves ListChannels.
AWS: DeleteChannel (Smithy: DELETE /prod/channels/{ChannelId}, code 200) leaves a tombstone: once the channel settles State "DELETED" it is NO LONGER returned by ListChannels (Smithy: GET /prod/channels, output Channels of ChannelSummary), while DescribeChannel keeps answering 200 with State "DELETED". The two views deliberately disagree.
Emulator: after the channel has settled DELETED, ListChannels still returns its id.
Why: every client that uses ListChannels as "what exists" (inventory sweeps, teardown loops, quota accounting) sees channels AWS has removed, and a teardown loop that lists-then-deletes never terminates.
Fix: the ListChannels handler / channel state - exclude DELETED tombstones from the listing while keeping them describable.
Traces: medialive.deleted-channel-still-describes-as-deleted-and-deletes-again, medialive.deleting-the-channel-detaches-its-input (both fail on "the deleted channel '<channel id>' is still listed" after DescribeChannel reported DELETED).

Cause 2 of 6 - a second DeleteChannel on an already-deleted channel answers NotFoundException instead of 200.
AWS: DeleteChannel on a channel that is already DELETED succeeds - 200 with the channel projection and State "DELETED". NotFoundException (Smithy: NotFoundException httpError 404, modeled on DeleteChannel) is reserved for a channel id that never existed; a re-delete of a tombstone is idempotent.
Emulator: the second DeleteChannel raises NotFoundException (404) even though DescribeChannel on the same id still answers with the channel and its attachments.
Why: a restore/cleanup step that is expected to be re-runnable (the e2e lifecycle runs its post_invoke twice) errors on its second pass, so an idempotent teardown is reported as a failure and per-trial leaks cannot be distinguished from normal convergence.
Fix: DeleteChannel's handler - treat a DELETED channel as a successful no-op returning the tombstone projection, and raise NotFoundException only when no such channel is known.
Traces: medialive.e2e-create-medialive-channel-lifecycle (phase 3/5 post_invoke: second pass reported errors ['delete channel: NotFoundException'] after reading inputs ['<input id>'] from DescribeChannel of the same channel), medialive.deleted-channel-still-describes-as-deleted-and-deletes-again (its second-delete assertion covers the same call).

Cause 3 of 6 - a channel created without LogLevel reports no LogLevel at all.
AWS: LogLevel (Smithy: Channel member LogLevel jsonName "logLevel", enum LogLevel) is defaulted by the service: a channel created with no LogLevel describes as "DISABLED" - the member is always present. The same channel carries no InputSpecification it never declared, a service-assigned weekly Maintenance window (MaintenanceDay one of MONDAY..SUNDAY, MaintenanceStartTime HH:MM) and PipelinesRunningCount 0.
Emulator: DescribeChannel omits LogLevel entirely (the check reads None).
Why: code that reads channel.logLevel to decide whether channel logging is on gets no value where AWS guarantees DISABLED, and SDKs that surface absent enums as null force emulator-specific special cases.
Fix: the CreateChannel handler must persist the service defaults, and the shared Channel serializer must emit them. (The open wire issue 20261102T091340Z-medialive-channel-responses-omit-modeled-empty-lists-and.md names the same serializer defect from a wire fixture; it does not carry this behavioral check's replay key.)
Traces: medialive.channel-omits-input-specification-and-defaults-log-level-disabled.

Cause 4 of 6 - UpdateInputSecurityGroup's Tags member overwrites the group's stored tags.
AWS: UpdateInputSecurityGroup (Smithy: PUT /prod/inputSecurityGroups/{InputSecurityGroupId}, code 200, output SecurityGroup of InputSecurityGroup) replaces WhitelistRules wholesale but leaves the stored tags alone even when the request carries a Tags map - that member is deprecated (Smithy: Tags, deprecated since 2024-11-20, "This API is deprecated. You must use UpdateTagsForResource instead"); tags change only through CreateTags/DeleteTags. After an update carrying Tags, both DescribeInputSecurityGroup.Tags and ListTagsForResource still report the tags the group was created with.
Emulator: the update's Tags map REPLACES the stored tags - the group's tags become exactly the request's map, in both DescribeInputSecurityGroup and ListTagsForResource.
Why: a client that passes Tags on an otherwise unrelated whitelist update silently loses every tag the group carried (cost-allocation and ownership tags included), and a later CreateTags merges onto the wrong base.
Fix: the UpdateInputSecurityGroup handler - apply WhitelistRules only and ignore the deprecated Tags member rather than writing it into state.
Traces: medialive.update-input-security-group-replaces-whitelist-and-keeps-tags.

Cause 5 of 6 - a URL_PULL input records the InputSecurityGroups it was given.
AWS: input security groups apply to push inputs only. CreateInput with Type URL_PULL accepts an InputSecurityGroups member and silently DROPS it: DescribeInput reports SecurityGroups [] and the named group is untouched - State "IDLE" with Inputs []. The same group on an RTMP_PUSH input IS recorded and the group goes IN_USE, so the empty list is a type rule, not a broken group. Recorded behavior only: the model marks InputSecurityGroups optional and states no type-dependent rule.
Emulator: the pull input describes with SecurityGroups ['<group id>'] - the group reference is kept.
Why: a client reading back its pull input sees a security-group binding AWS never made; the group also looks referenced, so the delete-order rules (and the group's IDLE/IN_USE state) report the wrong thing for the rest of the group's life.
Fix: CreateInput's handler/state - persist InputSecurityGroups only for push input types, and take the group reference only then.
Traces: medialive.url-pull-input-drops-the-security-groups-it-is-given.

Cause 6 of 6 - CreateSignalMap's bogus discovery-entry-point refusal carries the emulator's generic resource wording.
AWS: CreateSignalMap (Smithy: POST /prod/signal-maps, code 201; DiscoveryEntryPointArn required; errors include NotFoundException httpError 404) refuses an entry-point ARN naming a channel that does not exist with 404 NotFoundException and the message "Invalid discovery-entry-point-arn: Channel <channel id from the ARN> not found (Service: MediaLive, Status Code: 404, Request ID: <request id of MediaLive's internal downstream call>) (SDK Attempt Count: 1)". Fixed text: the "Invalid discovery-entry-point-arn: Channel ", " not found (Service: MediaLive, Status Code: 404, Request ID: " and ") (SDK Attempt Count: 1)" segments; variable: the channel id from the request's ARN and the embedded request id (per-run, not a requirement). No signal map is listed afterwards.
Emulator: the correct code (NotFoundException) but the message "Resource <the full ARN from the request> not found" - it names the ARN instead of the channel id and drops the entry-point framing entirely.
Why: the message is part of the error contract: a client (or an operator) is told only that some resource is missing, not that the discovery entry point it supplied names a nonexistent channel, which is the one actionable fact.
Fix: CreateSignalMap's entry-point resolution and its error message. (Related open issue 20261029T201850Z-medialive-not-found-refusals-carry-emulator-authored-resource.md describes emulator-authored 404 labels elsewhere in medialive; it does not carry this check's replay key.)
Traces: medialive.signal-map-invalid-entry-point-rejected.

## Evidence
Expected vs actual, from the replay of simparity-behavioral-aws-v1 against this checkout (all verdicts: mismatch, service medialive):

1. ListChannels after DeleteChannel settled DELETED, checks medialive.deleted-channel-still-describes-as-deleted-and-deletes-again and medialive.deleting-the-channel-detaches-its-input: "AssertionError: the deleted channel '1000000' is still listed" - expected the id absent from ListChannels while DescribeChannel still reports State 'DELETED'.
2. DeleteChannel re-issued on a DELETED channel, check medialive.e2e-create-medialive-channel-lifecycle: "[phase 3/5 post_invoke] post_invoke did not converge on its second pass: {'inputs': ['1000000'], 'stopped': False, 'errors': ['delete channel: NotFoundException']}" - expected 200 with State 'DELETED' (errors []); the DescribeChannel in the same pass succeeded and returned the channel's InputAttachments, so the channel is known to the emulator.
3. DescribeChannel of a channel created with no LogLevel, check medialive.channel-omits-input-specification-and-defaults-log-level-disabled: "AssertionError: an omitted LogLevel reads None" - expected 'DISABLED'.
4. UpdateInputSecurityGroup with Tags={'via-update': 'x'} on a group created with Tags={'sim2real': '<marker>'}, check medialive.update-input-security-group-replaces-whitelist-and-keeps-tags: "the update's Tags member changed the stored tags: ({'via-update': 'x'}, {'via-update': 'x'})" (DescribeInputSecurityGroup.Tags, ListTagsForResource.Tags) - expected ({'sim2real': '<marker>'}, {'sim2real': '<marker>'}). The WhitelistRules replacement itself matched.
5. DescribeInput of a URL_PULL input created with InputSecurityGroups=['<group id>'], check medialive.url-pull-input-drops-the-security-groups-it-is-given: "the pull input recorded groups ['3000000']; real AWS drops them" - expected SecurityGroups [] and the group State 'IDLE' with Inputs [].
6. CreateSignalMap with DiscoveryEntryPointArn naming a nonexistent channel, check medialive.signal-map-invalid-entry-point-rejected: expected message "Invalid discovery-entry-point-arn: Channel 999999999 not found (Service: MediaLive, Status Code: 404, Request ID: '<request id>') (SDK Attempt Count: 1)"; got "Resource arn:aws:medialive:<region>:<account>:channel:999999999 not found". The error code NotFoundException matched.

Model references used: DeleteChannel DELETE /prod/channels/{ChannelId} code 200 returning the full Channel projection (State -> ChannelState, LogLevel -> LogLevel, Maintenance -> MaintenanceStatus, PipelinesRunningCount) with NotFoundException httpError 404 modeled; ListChannels GET /prod/channels code 200, output Channels; UpdateInputSecurityGroup PUT /prod/inputSecurityGroups/{InputSecurityGroupId} code 200, input member Tags deprecated since 2024-11-20 in favour of UpdateTagsForResource; CreateInput POST /prod/inputs code 201 with the optional InputSecurityGroups member; CreateSignalMap POST /prod/signal-maps code 201 with NotFoundException 404.

The ids and generated names in the replay output (channel 1000000, input 1000000, group 3000000, the tag marker, the embedded request id) are per-run fillers, not requirements.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite simparity-behavioral-aws-v1; every step below is one of its medialive checks.
2. Cause 1: create a role, an input and a channel, wait for IDLE, DELETE /prod/channels/{id}, wait until DescribeChannel reports State DELETED, then GET /prod/channels -> expect the id absent from Channels; observe it still listed.
3. Cause 2: with that channel still describing as DELETED, DELETE /prod/channels/{id} again -> expect 200 with State 'DELETED'; observe 404 NotFoundException. The e2e lifecycle check reaches this by running its post_invoke restore step twice (phase 3/5).
4. Cause 3: POST /prod/channels with no InputSpecification, no LogLevel and no Maintenance, wait for IDLE, then GET /prod/channels/{id} -> expect logLevel 'DISABLED' (plus an assigned maintenance window and pipelinesRunningCount 0); observe logLevel absent.
5. Cause 4: POST /prod/inputSecurityGroups with Tags={'sim2real': m}, then PUT /prod/inputSecurityGroups/{id} with Tags={'via-update': 'x'} and new WhitelistRules -> expect the whitelist replaced and the tags unchanged in both DescribeInputSecurityGroup and ListTagsForResource; observe the tags replaced by the request's map.
6. Cause 5: POST /prod/inputs with Type URL_PULL, Sources=[...] and InputSecurityGroups=[group] -> expect DescribeInput SecurityGroups [] and the group IDLE with Inputs []; observe the group id recorded on the input. The same group on an RTMP_PUSH input must still be recorded and the group IN_USE.
7. Cause 6: POST /prod/signal-maps with DiscoveryEntryPointArn=arn:aws:medialive:<region>:<account>:channel:999999999 -> expect 404 NotFoundException 'Invalid discovery-entry-point-arn: Channel 999999999 not found (Service: MediaLive, Status Code: 404, Request ID: <id>) (SDK Attempt Count: 1)'; observe 'Resource <arn> not found'.
