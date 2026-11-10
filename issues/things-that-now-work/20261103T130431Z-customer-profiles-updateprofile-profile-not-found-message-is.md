Issue-Source: trace-sweep-divergence
Issue-Trace: customer-profiles-ccp-traj-wrong-field-then-update
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: customer-profiles-ccp-traj-wrong-field-then-update
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles|UpdateProfile|4xx->4xx|$.Message
Issue-Commit: ed09b88e064ad522a438f1cda8cc30ae911567ef

# customer-profiles UpdateProfile profile-not-found Message is emulator-authored, not AWS wording

> _Note: this issue was identified against the code base at commit `ed09b88e064ad522a438f1cda8cc30ae911567ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateProfile

## Description
One net-new wire cause: UpdateProfile's 404 refusal carries a sentence the emulator composes itself instead of AWS's fixed profile-not-found sentence, and it interpolates a request value AWS does not include.

Cause 1 of 1: UpdateProfile's profile-not-found sentence is reworded and names the domain AWS does not name.
AWS: Smithy (services/service-com-amazonaws-customerprofiles-customerprofiles-20200815): UpdateProfile is PUT /domains/{DomainName}/profiles and models ResourceNotFoundException with httpError 404; ProfileId is a required `uuid` with @pattern ^[a-f0-9]{32}$ and DomainName is the @httpLabel. Recorded response: 404 with body Message 'Profile with profileId <ProfileId> is not found in the domain'. Fixed text: 'Profile with profileId ' + ' is not found in the domain'; the ONLY variable part is the ProfileId taken from the request body. The domain name does NOT appear in AWS's sentence, even though it is in the request path.
Emulator: 404 (status and error class are correct) with 'The profile <ProfileId> does not exist in domain <DomainName>' — a different sentence shape (added article, different verb) plus an extra interpolated request value.
Why: updating a profile that was deleted, or whose id was mistyped/carried over from another domain, is a routine client branch. A client or harness that matches AWS's documented sentence does not match the emulator's, so it misclassifies a genuine 404 (or fails an idempotent update path), and the surplus domain name invites parsers to depend on a field AWS never sends.
Fix: UpdateProfile's profile-lookup refusal in the customer-profiles crate's update_profile handler (sharing the crate's profile-not-found message helper) — emit the fixed sentence with only the ProfileId interpolated and the domain name omitted.
Traces: customer-profiles-ccp-traj-wrong-field-then-update#task#8.

Related but not covering: open issue 20261103T055557Z-customer-profiles-resource-not-found-and-deleteprofile-success.md records the SAME sentence template for DeleteProfile (its Cause 4) and the domain-not-found wording for GetDomain/SearchProfiles/CreateProfile, but UpdateProfile is not among its causes and its replay keys do not include this fixture step, so this 404 path stays unexercised and unfixed under that issue.

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, 165 steps, score 58.8%). Cluster verdict=mismatch, expected_status 404 == actual_status 404, affected_count=1; the only diverging key is $.Message.

fixture customer-profiles-ccp-traj-wrong-field-then-update step task#8, UpdateProfile, 404 -> 404:
  $.Message: value_mismatch (expected 'Profile with profileId <ProfileId> is not found in the domain', got 'The profile <ProfileId> does not exist in domain <DomainName>')

divergence_key: service=customer-profiles|UpdateProfile|status=404->404|keys=$.Message

The profile id and domain name in the recorded strings are that run's fill-ins of the template named above; the fixed text, and which part is variable (ProfileId only), are the requirement.

## Steps to Reproduce
1. CreateDomain(DomainName=D, DefaultExpirationDays>=1).
2. UpdateProfile(DomainName=D, ProfileId=<a 32-char hex id that was never created in D, e.g. one matching ^[a-f0-9]{32}$>).
3. Expect HTTP 404 ResourceNotFoundException with body Message exactly 'Profile with profileId <that ProfileId> is not found in the domain' — no domain name in the sentence.
4. Observe HTTP 404 with Message 'The profile <that ProfileId> does not exist in domain D'.

Re-replay fixture customer-profiles-ccp-traj-wrong-field-then-update (step task#8) to confirm the fix.
