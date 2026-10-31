Issue-Source: trace-sweep-divergence
Issue-Trace: customer-profiles-batch-get-profile-mixed-valid-invalid
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: customer-profiles-batch-get-profile-mixed-valid-invalid
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles|BatchGetProfile|2xx->2xx|$.Errors[*].Message
Issue-Commit: 75db209b3b62142531ae7634681767161474a9bf

# customer-profiles BatchGetProfile emits a non-AWS Errors[].Message for an unknown profile id

> _Note: this issue was identified against the code base at commit `75db209b3b62142531ae7634681767161474a9bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetProfile

## Description
Cause 1 of 1: BatchGetProfile's per-profile failure entry carries the wrong Message text when a requested profile id does not exist in the domain.

AWS: BatchGetProfile (Smithy: POST /domains/{DomainName}/batch-get-profiles, http code 200; output BatchGetProfileResponse with Errors -> BatchGetProfileErrorList of BatchGetProfileError{Code, Message, ProfileId}, Message target string1To1000 @length 1..1000, doc "Message describing why a specific profile failed"). A partially-valid batch is NOT an operation-level error: AWS answers HTTP 200, returns the resolvable profiles in Profiles, and one BatchGetProfileError per unresolvable ProfileId. For a profile id that is well-formed (uuid pattern ^[a-f0-9]{32}$) but absent from the domain, the recorded response's Message is the fixed string "Profile was not found in domain" — it is entirely fixed text, with NO interpolated profile id and NO interpolated domain name; the only request-derived value in the entry is ProfileId (and Code). The modeled Message is prose-only beyond the length constraint, so the exact wording here is recorded behavior only, taken from the recorded response.

Emulator: answers HTTP 200 with the right envelope and the right Errors cardinality, but synthesizes its own sentence instead: $.Errors[0].Message = "The profile <profileId> does not exist in domain <domainName>" (replay detail: expected 'Profile was not found in domain', got 'The profile 00000000000000000000000000000000 does not exist in domain sim2real-cp-dom-<id>'). It interpolates the requested profile id and the domain name into a message AWS keeps fixed, so both the fixed text and the set of variable parts are wrong. The id and domain name in the recording are per-run values and are not themselves the requirement; the requirement is that the whole Message be the fixed sentence with no interpolation.

Why: clients and SDK-level test suites that branch on the per-profile failure reason compare or match Errors[].Message against AWS's fixed wording (equality, prefix or regex match on "Profile was not found in domain"). Against the emulator that match never succeeds, so a not-found profile inside an otherwise successful batch is reported as an unrecognized/unclassified failure, or is silently swallowed, even though the surrounding 200 response and ProfileId are correct. Anything that logs or surfaces the reason to an operator shows text AWS never emits, and the leaked domain name is extra information AWS does not put in this field.

Fix: the BatchGetProfile handler's per-profile not-found branch — where the BatchGetProfileError entry is built for an id missing from the domain's profile store. The Message must be emitted as AWS's fixed text; only Code and ProfileId carry request/state-derived values. No other field of the response diverges, so the envelope and Profiles construction are correct and should be left alone.

Related open issues: none of the existing open customer-profiles issues cover this — the bundle 20261029T183204Z-customer-profiles-2-net-new-root-causes-mergeprofiles.md covers MergeProfiles self-merge and PutProfileObjectType TemplateId mixing, and the remaining open items are unimplemented-operation stubs (AddProfileKey, CreateEventStream, CreateRecommenderSchema, CreateUploadJob, ListProfileHistoryRecords, MergeProfiles, PutProfileObjectType, UpdateDomain, UpdateProfile, CreateSegmentDefinition). BatchGetProfile is implemented and answers; this is a fidelity defect in that implementation.

Traces: customer-profiles-batch-get-profile-mixed-valid-invalid#task#3.

## Evidence
Fixture customer-profiles-batch-get-profile-mixed-valid-invalid, step task#3, operation BatchGetProfile.
Verdict: mismatch. Expected status 200, actual status 200 — the divergence is confined to the response body key $.Errors[0].Message.
Expected (recorded AWS): $.Errors[0].Message == "Profile was not found in domain" (fixed text in full; no variable parts).
Actual (emulator): $.Errors[0].Message == "The profile 00000000000000000000000000000000 does not exist in domain sim2real-cp-dom-dbd74c3a-6a2c-4f98-b15b-741eea498b03" — different fixed wording plus two interpolated values (the requested ProfileId and the DomainName) that AWS does not place in this field. The specific id/domain shown are per-run values of this recording, not part of the expected template.
Replay divergence key: service=customer-profiles|BatchGetProfile|status=200->200|keys=$.Errors[0].Message (cluster size 1).
Smithy: BatchGetProfile output BatchGetProfileResponse.Errors -> BatchGetProfileErrorList; BatchGetProfileError.Message required, target string1To1000 @length min 1 max 1000; BatchGetProfileError.ProfileId required, pattern ^[a-f0-9]{32}$; operation http {POST /domains/{DomainName}/batch-get-profiles, code 200}; modeled errors AccessDeniedException(403), BadRequestException(400), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429) — none of which apply, confirming a missing profile is reported in-band at 200.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture customer-profiles-batch-get-profile-mixed-valid-invalid.
2. The fixture creates a customer-profiles domain and at least one real profile, then at step task#3 calls BatchGetProfile on POST /domains/{DomainName}/batch-get-profiles with ProfileIds containing a mix of one existing profile id and one well-formed-but-absent 32-hex-character profile id (e.g. 00000000000000000000000000000000).
3. Observe the 200 response: Profiles contains the existing profile and Errors contains one BatchGetProfileError for the absent id.
4. Read $.Errors[0].Message. AWS returns the fixed text "Profile was not found in domain"; the emulator instead returns "The profile <requested profile id> does not exist in domain <domain name>".
5. Equivalently: replay_wire_corpus() reports the cluster service=customer-profiles|BatchGetProfile|status=200->200|keys=$.Errors[0].Message as a mismatch.
