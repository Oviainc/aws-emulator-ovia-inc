Issue-Source: trace-sweep-divergence
Issue-Trace: wafv2-api-key-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wafv2-api-key-lifecycle wafv2-delete-apikey-nonexistent-rejected
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2|ListAPIKeys|2xx->2xx|$.ApplicationIntegrationURL
Issue-Commit: 22941ee6d43df66d29bcadd7a62b7906e5868e84

# wafv2 ListAPIKeys ApplicationIntegrationURL carries a 32-character integration id instead of WAF's 12-character label

> _Note: this issue was identified against the code base at commit `22941ee6d43df66d29bcadd7a62b7906e5868e84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAPIKeys

## Description
Cause 1 of 1 (fixture wafv2-api-key-lifecycle, step task#2): ListAPIKeys answers 200 but the $.ApplicationIntegrationURL it reports is built around an integration identifier of the wrong width, so the URL is not of the form WAF publishes the CAPTCHA JavaScript integration at.

AWS: recorded behavior only for the URL's syntax. Smithy (services/service-com-amazonaws-wafv2-awswaf-20190729/smithy-model/com-amazonaws-wafv2-awswaf-20190729.json): ListAPIKeys -> ListAPIKeysResponse$ApplicationIntegrationURL, target OutputUrl, a plain string with no length/pattern constraint, documented as "The CAPTCHA application integration URL, for use in your JavaScript implementation." The model therefore fixes no syntax and the requirement is the recorded one: WAF answers `https://<id>.<region>.captcha-sdk.awswaf.com/<id>/`, where the fixed text is the `https://` scheme, the `.captcha-sdk.awswaf.com` suffix, the trailing `/` and the repetition of the SAME id in both the leading host label and the single path segment, and the variable parts are the endpoint region and the account's integration id. In the recording that id is TWELVE lowercase hex characters (the recorded value `d1bdd7b0ad06` is this account's own id and is not itself a requirement — only its character class and its 12-character width are).

Emulator: the replayed step returns 200 with an ApplicationIntegrationURL whose host label and path segment are a THIRTY-TWO character lowercase hex id (e.g. `https://<32 hex chars>.us-west-2.captcha-sdk.awswaf.com/<32 hex chars>/`), which the replay reports as $.ApplicationIntegrationURL host_not_own. The scheme, the `.captcha-sdk.awswaf.com` suffix, the region placement and the id repetition are all correct; only the id's width is wrong. The id comes from the first 32 hex characters of a SHA-256 digest over the CAPTCHA domain plus the emulator's account id, i.e. 16 bytes rendered as hex, where WAF's label is 6 bytes rendered as hex.

Why: a client that parses the integration URL to recover WAF's integration id — or that validates the URL against the shape WAF publishes (a 12-character host label), splits the label out to build a script tag or a CSP/host allow-list entry, or asserts the label and path segment have WAF's documented width — succeeds against AWS and fails against the emulator. Because the id is stable per account, every ListAPIKeys answer in the emulator is wrong in the same way, so such a client never recovers.

Fix: the wafv2 service state's CAPTCHA integration helper, not the ListAPIKeys handler — the handler already answers whatever the helper renders and the same helper feeds any other operation reporting this URL. Narrow the integration id the URL is composed from to the width WAF's label has (6 bytes of the existing account-derived digest rendered as 12 lowercase hex characters) and keep the same id in both the host label and the path segment.

Traces: wafv2-api-key-lifecycle#task#2 (representative), wafv2-delete-apikey-nonexistent-rejected#task#1 and wafv2-delete-apikey-nonexistent-rejected#task#3 — all three ListAPIKeys, all expected 200 / got 200 with $.ApplicationIntegrationURL diverging.

## Evidence
Replay cluster service=wafv2|ListAPIKeys|status=200->200|keys=$.ApplicationIntegrationURL, verdict mismatch, affected_count 3.
Expected: status 200, $.ApplicationIntegrationURL = 'https://d1bdd7b0ad06.us-west-2.captcha-sdk.awswaf.com/d1bdd7b0ad06/' — a 12-lowercase-hex-character id repeated as the host label and the path segment (that literal id is this account's value, quoted only to show the FORM).
Actual: status 200, $.ApplicationIntegrationURL = 'https://aa730e0b0523ce83942711e1baf4286e.us-west-2.captcha-sdk.awswaf.com/aa730e0b0523ce83942711e1baf4286e/' — a 32-lowercase-hex-character id in both positions.
Source of the emulator's form: services/service-com-amazonaws-wafv2-awswaf-20190729/src/state/api_keys.rs lines 222-246 (CAPTCHA_INTEGRATION_ID_LENGTH = 32 and `captcha_integration_id`, the account-derived SHA-256 digest truncated to 32 hex chars) and lines 257-260 (`application_integration_url` formats `https://{integration}.{region}.captcha-sdk.awswaf.com/{integration}/`); it is returned unchanged by services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/list_api_keys.rs lines 241-252.
Smithy evidence: ListAPIKeysResponse$ApplicationIntegrationURL -> OutputUrl, a string shape with no length or pattern trait, hence "recorded behavior only" for the URL's syntax.
No open issue owns this cluster (replay reports owned_by: [] and stub_path: ""). The nearest open wafv2 issues cover other surfaces: 20261108T045108Z-wafv2-createapikey-mints-an-api-key-value-in.md covers the minted $.APIKey value's base64 form, 20261029T192556Z-wafv2-request-validation-never-refuses-ten-operations-answer.md covers missing refusals, and 20261029T192556Z-wafv2-managed-rule-catalog-contents-and-wcu-capacity.md covers the managed-rule catalog — none of them mentions ApplicationIntegrationURL.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Fixture wafv2-api-key-lifecycle, step task#2: POST the wafv2 JSON-RPC target AWSWAF_20190729.ListAPIKeys with Scope=REGIONAL against a us-west-2 endpoint.
3. Read $.ApplicationIntegrationURL from the 200 response: the host's leading label and the single path segment are 32 lowercase hex characters, where WAF's recorded URL carries a 12-character label/segment under the same `.<region>.captcha-sdk.awswaf.com` suffix.
4. The same divergence reproduces at wafv2-delete-apikey-nonexistent-rejected#task#1 and #task#3, the other members of the cluster.
