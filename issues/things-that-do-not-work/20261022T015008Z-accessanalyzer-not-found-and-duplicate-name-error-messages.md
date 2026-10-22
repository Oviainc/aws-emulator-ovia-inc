Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: accessanalyzer.get-analyzer-nonexistent-name-rejected accessanalyzer.delete-analyzer-removes-resource accessanalyzer.create-analyzer-duplicate-name-conflict-rejected accessanalyzer.concurrent-duplicate-analyzer-creation-exactly-one-succeeds
Issue-Shape-Id: com.amazonaws.accessanalyzer#AccessAnalyzer
Issue-Key: accessanalyzer.get-analyzer-nonexistent-name-rejected|run#11|AssertionError|assert message == "Analyzer not found", message
Issue-Key: accessanalyzer.delete-analyzer-removes-resource|run#10|AssertionError|assert message == "Analyzer not found", message
Issue-Key: accessanalyzer.create-analyzer-duplicate-name-conflict-rejected|run#8|AssertionError|assert message == "Analyzer with the same name already exists", message
Issue-Key: accessanalyzer.concurrent-duplicate-analyzer-creation-exactly-one-succeeds|run#12|AssertionError|assert all(detail == expected for _, detail in failures), failures
Issue-Commit: c6f9974c04d0d99d1dbb9b00d2d222f6d449381b

# accessanalyzer not-found and duplicate-name error messages echo the analyzer name instead of AWS's fixed generic message text

> _Note: this issue was identified against the code base at commit `c6f9974c04d0d99d1dbb9b00d2d222f6d449381b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAnalyzer

## Description
The emulator returns the correct error codes on these paths, but builds the message by interpolating the requested analyzer name, where AWS returns a fixed generic message and carries the identifier in the exception's own modeled identifier members. The message rule here IS known from the model: both ResourceNotFoundException and ConflictException declare `resourceId` and `resourceType` as REQUIRED members alongside `message`, i.e. the identifier is a structured member of the error, not part of its prose. Two causes, both in the accessanalyzer service; the same divergence pattern is already filed for other services (e.g. 20261022T003746Z-bcm-data-exports-getexport-s-resourcenotfoundexception-message-echoes-the-requested-arn.md, 20261022T012855Z-launch-wizard-not-found-responses-echo-the-requested.md), none of which covers accessanalyzer.

Cause 1 of 2: ResourceNotFoundException for an absent/deleted analyzer echoes the name.
AWS: Smithy: GetAnalyzer (`GET /analyzer/{analyzerName}`) models `ResourceNotFoundException` (@error client, httpError 404, required members `message`, `resourceId`, `resourceType`; shape doc "The specified resource could not be found."). The refusal is HTTP 404 with Error.Code `ResourceNotFoundException` and a fixed generic message that does not name the requested analyzer — recorded as "Analyzer not found" — with the requested name conveyed in `resourceId` / `resourceType`. This holds both for a name that never existed and for a name re-read after DeleteAnalyzer.
Emulator: same code, but the message is composed from the request — recorded form "Analyzer <analyzerName> does not exist" (e.g. "Analyzer sim2real-aa-missing-d8e73a5b5acb does not exist").
Why: the message is the only place the emulator puts the identifier, so the divergence is twofold: clients and log scrapers that match AWS's fixed not-found text stop matching, and callers that expect the identifier in the structured `resourceId`/`resourceType` members cannot find it. An emulator used to pin error-handling behavior therefore validates a message contract the real service does not honour.
Fix: the ResourceNotFoundException construction used by the GetAnalyzer and DeleteAnalyzer handlers (and the shared accessanalyzer error-building helper they call) in services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/ops/ — emit the fixed generic message and populate the modeled `resourceId`/`resourceType` members instead of interpolating the name into `message`.
Traces: accessanalyzer.get-analyzer-nonexistent-name-rejected, accessanalyzer.delete-analyzer-removes-resource.

Cause 2 of 2: ConflictException for a duplicate analyzer name echoes the name.
AWS: Smithy: CreateAnalyzer (`PUT /analyzer`) models `ConflictException` (@error client, httpError 409, required members `message`, `resourceId`, `resourceType`; shape doc "A conflict exception error."). A second analyzer created with a name already in use is refused with HTTP 409 / Error.Code `ConflictException` and a fixed generic message that does not name the analyzer — recorded as "Analyzer with the same name already exists". The conflict is state, not invalid input: after the original is deleted the identical call succeeds. Under N racing CreateAnalyzer calls for one name, exactly one wins and every loser gets that same ConflictException.
Emulator: same code and the same single-winner outcome, but the message is composed from the request — recorded form "Analyzer <analyzerName> already exists in this account and Region" — identically on the serial path and on all losing racers.
Why: callers that classify conflicts by message text, and tests pinned to AWS's generic conflict wording, diverge; and as in cause 1 the identifier lands in prose instead of the modeled `resourceId`/`resourceType`.
Fix: the ConflictException construction in the CreateAnalyzer handler / shared accessanalyzer error helper (services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/ops/) — fixed generic message, identifier in the modeled members.
Traces: accessanalyzer.create-analyzer-duplicate-name-conflict-rejected, accessanalyzer.concurrent-duplicate-analyzer-creation-exactly-one-succeeds.

## Evidence
Replay of suite simparity-behavioral-aws-v1. In all four checks the HTTP error CODE matched AWS; only the message template diverged, and it diverged by interpolating the request's analyzer name.

1) accessanalyzer.get-analyzer-nonexistent-name-rejected [mismatch]
   check line 33: assert message == "Analyzer not found", message
   AssertionError: Analyzer sim2real-aa-missing-d8e73a5b5acb does not exist
   Expected: Code ResourceNotFoundException (passed) + fixed generic message, identifier in resourceId/resourceType.
   Actual:   Code ResourceNotFoundException + "Analyzer <name> does not exist".

2) accessanalyzer.delete-analyzer-removes-resource [mismatch]
   check line 30: assert message == "Analyzer not found", message
   AssertionError: Analyzer sim2real-aa-delete-c1004398cabd does not exist
   Same template defect on the post-DeleteAnalyzer re-read (the preceding assertion, code == ResourceNotFoundException, passed).

3) accessanalyzer.create-analyzer-duplicate-name-conflict-rejected [mismatch]
   check line 27: assert message == "Analyzer with the same name already exists", message
   AssertionError: Analyzer sim2real-aa-dupe-620c30a18687 already exists in this account and Region
   The preceding assertion code == "ConflictException" passed.

4) accessanalyzer.concurrent-duplicate-analyzer-creation-exactly-one-succeeds [mismatch]
   check line 46: assert all(detail == expected for _, detail in failures), failures
   expected = ("ConflictException", "Analyzer with the same name already exists")
   AssertionError: [('error', ('ConflictException', 'Analyzer sim2real-aa-race-d5663d7f3933 already exists in this account and Region')) x4]
   Exactly-one-winner and loser-count assertions passed; only the message text of each loser diverged, confirming one shared message template rather than a concurrency defect.

## Steps to Reproduce
Boot the emulator and point an accessanalyzer client at it.

Cause 1: (a) call GetAnalyzer(analyzerName=<never created>) and read Error.Message — AWS returns the fixed "analyzer not found" message with the name in resourceId; the emulator returns "Analyzer <name> does not exist". (b) CreateAnalyzer(type="ACCOUNT"), wait for ACTIVE, DeleteAnalyzer, then re-read with GetAnalyzer until it refuses: same divergence. (Replay accessanalyzer.get-analyzer-nonexistent-name-rejected, accessanalyzer.delete-analyzer-removes-resource.)

Cause 2: (a) CreateAnalyzer(analyzerName=N, type="ACCOUNT"), then CreateAnalyzer again with the same N; read Error.Code (409 ConflictException, matches) and Error.Message (diverges: name interpolated). (b) Race 5 concurrent CreateAnalyzer calls for one unused name and inspect each loser's message — all four carry the same interpolated text. (Replay accessanalyzer.create-analyzer-duplicate-name-conflict-rejected, accessanalyzer.concurrent-duplicate-analyzer-creation-exactly-one-succeeds.)
