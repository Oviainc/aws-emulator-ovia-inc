Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: b7068a3aaca27128a9512527e96d46285251fa2e

# DescribeVoices splits the voice catalog into 50-voice pages and returns a NextToken real Polly does not send

_Filed: 2026-10-24T03:33:51Z_
_Operation: DescribeVoices | Class: pagination_

> _Note: this issue was identified against the code base at commit `b7068a3aaca27128a9512527e96d46285251fa2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The emulator's DescribeVoices caps every response at an emulator-chosen page of 50 voices (VOICE_PAGE_SIZE) and mints a NextToken whenever more voices match, so the very first unfiltered call returns only 50 of the 98 seeded voices plus a NextToken.

Expected: Amazon Polly answers an unfiltered DescribeVoices with the complete list of available voices in a single 200 response and no NextToken — the operation is not marked @paginated in the model (only ListSpeechSynthesisTasks is), and the AWS SDKs ship no paginator for it, so a client that calls describe-voices once and reads Voices sees every voice.
Actual: the first page carries 50 voices and a NextToken; a client that does not loop on NextToken (the normal usage) sees a truncated catalog and can conclude a voice such as Zhiyu or Tatyana does not exist.

## Evidence

Smithy model: OPERATIONS.md/smithy_query show DescribeVoices has no @paginated trait (ListSpeechSynthesisTasks is the only paginated operation in this service) and documents NextToken only as "An opaque pagination token"; well-established behavior: `aws polly describe-voices` returns the whole voice list in one response with no NextToken. Handler: src/ops/describe_voices.rs line 142 `pub const VOICE_PAGE_SIZE: usize = 50;` (its own doc comment says the size is "CHOSEN"), lines 296-305 mint a NextToken whenever a 51st match exists. State: src/state.rs VOICE_CATALOG carries 98 voices, so pagination fires on every unfiltered call. Tests ENTRENCH the split rather than cover the gap: tests/all/lexicons_and_speech.rs::a_voice_page_token_only_resumes_its_own_query asserts first["NextToken"] is present with the comment "the seeded catalog spans more than one page".

## Steps to Reproduce

1. Boot the emulator and call GET /v1/voices with no query parameters (botocore: polly.describe_voices())
2. Count the returned Voices: 50, with a NextToken present
3. Compare with real Polly, which returns the full voice list in one response and no NextToken
4. Repeat with ?LanguageCode=en-US to see the same cap applied per filter
