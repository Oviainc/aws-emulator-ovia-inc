Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-data-automation-project-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-data-automation-project-lifecycle bedrock-data-automation-project-invalid-name-pattern-rejected bedrock-data-automation-project-with-custom-blueprint
Issue-Shape-Id: com.amazonaws.bedrockdataautomation#AmazonBedrockKeystoneBuildTimeService
Issue-Key: bedrock-data-automation|GetDataAutomationProject|2xx->2xx|$.project.creationTime
Issue-Key: bedrock-data-automation|GetDataAutomationProject|2xx->2xx|$.project.customOutputConfiguration
Issue-Key: bedrock-data-automation|GetDataAutomationProject|2xx->2xx|$.project.dataAutomationLibraryConfiguration
Issue-Key: bedrock-data-automation|GetDataAutomationProject|2xx->2xx|$.project.lastModifiedTime
Issue-Key: bedrock-data-automation|GetDataAutomationProject|2xx->2xx|$.project.overrideConfiguration
Issue-Key: bedrock-data-automation|GetDataAutomationProject|2xx->2xx|$.project.standardOutputConfiguration.audio
Issue-Key: bedrock-data-automation|GetDataAutomationProject|2xx->2xx|$.project.standardOutputConfiguration.image
Issue-Key: bedrock-data-automation|GetDataAutomationProject|2xx->2xx|$.project.standardOutputConfiguration.video
Issue-Key: bedrock-data-automation|GetDataAutomationProject|2xx->2xx|$.project.standardOutputConfiguration.document
Issue-Key: bedrock-data-automation|GetDataAutomationProject|2xx->2xx|$.project.standardOutputConfiguration.document.generativeField.state
Issue-Key: bedrock-data-automation|GetDataAutomationProject|2xx->2xx|$.project.standardOutputConfiguration.document.outputFormat.textFormat.types[*]
Issue-Key: bedrock-data-automation|DeleteDataAutomationProject|2xx->2xx|$.status
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Project reads drop configuration blocks and delete reports COMPLETED

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataAutomationProject

## Description
GetDataAutomationProject never materializes the project's configuration: the per-modality blocks of standardOutputConfiguration and the whole of overrideConfiguration / customOutputConfiguration / dataAutomationLibraryConfiguration come back absent, the timestamps come back as epoch numbers, and one read also reports emulator defaults in place of values the project was configured with. DeleteDataAutomationProject reports a terminal status where the service reports an in-flight one. Five net-new causes. (No open issue covers this service's build-time surface; the bedrock-data-automation-runtime issues such as 20261024T024752Z-bedrock-data-automation-runtime-invokedataautomationasync-event-propagation are a different service.)

=== Cause 1 of 5: GetDataAutomationProject omits the configuration blocks and serves numeric timestamps ===
AWS: Smithy: DataAutomationProject.creationTime and .lastModifiedTime target DateTimestamp with @timestampFormat("date-time"), so under restJson1 they are ISO-8601 date-time STRINGS. Smithy: DataAutomationProject carries standardOutputConfiguration (StandardOutputConfiguration{document,image,video,audio}), customOutputConfiguration, overrideConfiguration (OverrideConfiguration{document,image,video,audio,modalityRouting}) and dataAutomationLibraryConfiguration. Recorded behavior only, for the DEFAULTING rule: the recorded 200 carries all four blocks even for a project whose create set only some of them -- every standardOutputConfiguration modality is present and fully populated with the service's defaults (audio: extraction.category ENABLED with types [TRANSCRIPT] and transcript channel/speaker labeling DISABLED, generativeField DISABLED; document: extraction.boundingBox DISABLED, granularity.types [PAGE, ELEMENT], outputFormat.additionalFileFormat DISABLED, outputFormat.textFormat.types [MARKDOWN]; image and video: extraction.boundingBox ENABLED, extraction.category ENABLED with types [TEXT_DETECTION], generativeField ENABLED with types [IMAGE_SUMMARY] / [VIDEO_SUMMARY, CHAPTER_SUMMARY]), overrideConfiguration is present with its own defaults (audio.languageConfiguration generativeOutputLanguage DEFAULT and identifyMultipleLanguages false, document.splitter DISABLED, image and video present), and customOutputConfiguration and dataAutomationLibraryConfiguration are present as empty structures when nothing is configured.
Emulator: status 200 (correct), but $.project.customOutputConfiguration, $.project.dataAutomationLibraryConfiguration and $.project.overrideConfiguration are null/absent, every standardOutputConfiguration modality the create did not spell out is null/absent, and creationTime/lastModifiedTime are epoch-second numbers.
Why: a client reads the project back to learn the effective configuration it will be billed and processed under -- extraction granularity, bounding boxes, summary generation, splitter state. With the blocks absent it cannot distinguish "off" from "defaulted on", so it either re-sends a configuration it did not intend or treats an enabled modality as disabled. Config-as-code tooling that diffs the read against its desired state sees every default as drift and rewrites the project on every pass. The numeric timestamps are not parseable as the modelled date-time.
Fix: the GetDataAutomationProject handler's project projection -- materialize the stored configuration against the service's defaults before serializing (belongs in the project state's effective-configuration accessor, not in the read handler), plus the date-time timestamp encoding.
Traces: bedrock-data-automation-project-lifecycle#task#2.

=== Cause 2 of 5: the same read after an invalid-name rejection also drops the document block ===
AWS: as Cause 1. The recorded 200 additionally carries the fully defaulted standardOutputConfiguration.document block (extraction.boundingBox DISABLED, granularity.types [PAGE, ELEMENT], generativeField DISABLED, outputFormat.additionalFileFormat DISABLED, outputFormat.textFormat.types [MARKDOWN]).
Emulator: status 200 (correct); document is absent alongside audio, image, video, overrideConfiguration, customOutputConfiguration and dataAutomationLibraryConfiguration, and both timestamps are epoch numbers.
Why: this is the read a client makes right after a failed create to confirm what did and did not take effect; with every modality absent it cannot tell the surviving project's shape.
Fix: same effective-configuration accessor as Cause 1 -- it must cover the document modality too.
Traces: bedrock-data-automation-project-invalid-name-pattern-rejected#task#4.

=== Cause 3 of 5: the read reports emulator defaults instead of the project's configured document values ===
AWS: Smithy: DocumentStandardOutputConfiguration{extraction, generativeField, outputFormat}; the project's own configured values are what the read reports. The recorded 200 reports standardOutputConfiguration.document.generativeField.state ENABLED and document.outputFormat.textFormat.types[0] MARKDOWN for this project.
Emulator: status 200 (correct) but reports document.generativeField.state DISABLED and document.outputFormat.textFormat.types[0] PLAIN_TEXT -- the emulator's own default values in place of the project's -- in addition to the absent blocks and numeric timestamps of Cause 1.
Why: generative-field state and text format decide what the project actually emits. A client that configured MARKDOWN output with summaries reads back PLAIN_TEXT without summaries, so it either disables a feature it paid to enable or loops forever trying to set a value the read never confirms.
Fix: the project state's write path for standardOutputConfiguration.document (the configured value must be persisted and preferred over the default) and the same read projection as Cause 1.
Traces: bedrock-data-automation-project-lifecycle#task#4.

=== Cause 4 of 5: a project created with a custom blueprint reads back without its configuration blocks ===
AWS: as Cause 1. The recorded 200 carries overrideConfiguration, dataAutomationLibraryConfiguration and the audio/image/video standard-output blocks with their defaults; customOutputConfiguration is present here because the project names a custom blueprint.
Emulator: status 200 (correct); dataAutomationLibraryConfiguration, overrideConfiguration and the audio/image/video blocks are absent, and both timestamps are epoch numbers.
Why: for a custom-blueprint project the read is the only way a client confirms the blueprint binding sits alongside the standard-output defaults; an absent block reads as "not configured".
Fix: same effective-configuration accessor as Cause 1, exercised on the custom-blueprint project shape.
Traces: bedrock-data-automation-project-with-custom-blueprint#task#3.

=== Cause 5 of 5: DeleteDataAutomationProject reports COMPLETED instead of IN_PROGRESS ===
AWS: Smithy: DeleteDataAutomationProjectResponse.status targets enum DataAutomationProjectStatus with values [COMPLETED, IN_PROGRESS, FAILED]. The recorded 200 for a delete reports status IN_PROGRESS -- the delete is accepted and the teardown proceeds asynchronously, so the status the delete call returns is the in-flight one, not the terminal one.
Emulator: status 200 (correct) with $.status "COMPLETED".
Why: a client that polls until the project leaves IN_PROGRESS never enters its wait loop, so it proceeds as though the teardown had already settled; a client that asserts the delete is asynchronous (the normal contract for a project delete) sees a status transition that cannot happen on real AWS.
Fix: the DeleteDataAutomationProject handler -- report the deletion's in-flight status from the project state's deletion lifecycle rather than returning a terminal constant.
Traces: bedrock-data-automation-project-lifecycle#teardown#5 (also bedrock-data-automation-project-with-custom-blueprint#teardown#4).

## Evidence
Cause 1 (bedrock-data-automation-project-lifecycle#task#2, GetDataAutomationProject, 200 vs 200): $.project.customOutputConfiguration expected {} got None; $.project.dataAutomationLibraryConfiguration expected {} got None; $.project.overrideConfiguration expected {'audio': {'languageConfiguration': {'generativeOutputLanguage': 'DEFAULT', 'identifyMultipleLanguages': False}}, 'document': {'splitter': {'state': 'DISABLED'}}, 'image': {}, 'video': {}} got None; $.project.standardOutputConfiguration.audio / .image / .video expected fully populated default blocks, got None; $.project.creationTime and $.project.lastModifiedTime expected a date-time-shaped value ('<shaped-not-asserted>'), got 1793293479.3998184.
Cause 2 (bedrock-data-automation-project-invalid-name-pattern-rejected#task#4, GetDataAutomationProject, 200 vs 200): as Cause 1 plus $.project.standardOutputConfiguration.document expected {'extraction': {'boundingBox': {'state': 'DISABLED'}, 'granularity': {'types': ['PAGE', 'ELEMENT']}}, 'generativeField': {'state': 'DISABLED'}, 'outputFormat': {'additionalFileFormat': {'state': 'DISABLED'}, 'textFormat': {'types': ['MARKDOWN']}}} got None; timestamps got 1793293477.7764783.
Cause 3 (bedrock-data-automation-project-lifecycle#task#4, GetDataAutomationProject, 200 vs 200): $.project.standardOutputConfiguration.document.generativeField.state expected 'ENABLED' got 'DISABLED'; $.project.standardOutputConfiguration.document.outputFormat.textFormat.types[0] expected 'MARKDOWN' got 'PLAIN_TEXT'; plus the absent audio/image/video blocks, absent overrideConfiguration / customOutputConfiguration / dataAutomationLibraryConfiguration, and numeric timestamps.
Cause 4 (bedrock-data-automation-project-with-custom-blueprint#task#3, GetDataAutomationProject, 200 vs 200): $.project.dataAutomationLibraryConfiguration expected {} got None; $.project.overrideConfiguration expected the default block got None; $.project.standardOutputConfiguration.audio / .image / .video expected default blocks got None; timestamps got 1793293481.0460007.
Cause 5 (bedrock-data-automation-project-lifecycle#teardown#5, DeleteDataAutomationProject, 200 vs 200): $.status expected 'IN_PROGRESS', got 'COMPLETED'. 2 steps in the cluster (also bedrock-data-automation-project-with-custom-blueprint#teardown#4).
Model citations: Smithy DataAutomationProject.creationTime/lastModifiedTime -> DateTimestamp @timestampFormat("date-time"); DataAutomationProject members include standardOutputConfiguration, customOutputConfiguration, overrideConfiguration, dataAutomationLibraryConfiguration; StandardOutputConfiguration{document,image,video,audio}; OverrideConfiguration{document,image,video,audio,modalityRouting}; DocumentStandardOutputConfiguration{extraction,generativeField,outputFormat}; DeleteDataAutomationProjectResponse.status -> enum DataAutomationProjectStatus [COMPLETED, IN_PROGRESS, FAILED]; protocol restJson1. (Epoch values and generated project names above are this run's values, not requirements.)

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator (replay_wire_corpus).
2. Cause 1: fixture bedrock-data-automation-project-lifecycle, step task#2 -- GetDataAutomationProject; check that $.project.overrideConfiguration, .customOutputConfiguration, .dataAutomationLibraryConfiguration and standardOutputConfiguration.audio/.image/.video are present and defaulted, and that both timestamps are date-time strings.
3. Cause 2: fixture bedrock-data-automation-project-invalid-name-pattern-rejected, step task#4 -- same read; additionally check standardOutputConfiguration.document.
4. Cause 3: fixture bedrock-data-automation-project-lifecycle, step task#4 -- same read; check standardOutputConfiguration.document.generativeField.state and .outputFormat.textFormat.types[0] against what the project was configured with.
5. Cause 4: fixture bedrock-data-automation-project-with-custom-blueprint, step task#3 -- same read on a custom-blueprint project.
6. Cause 5: fixture bedrock-data-automation-project-lifecycle, step teardown#5 -- DeleteDataAutomationProject; check $.status.
