Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: license-manager.checked-in-consumption-token-is-not-reusable license-manager.checkout-of-undeclared-entitlement-name-rejected license-manager.checkout-past-entitlement-max-count-rejected license-manager.checkout-with-mismatched-entitlement-unit-rejected license-manager.concurrent-checkouts-never-exceed-entitlement-max-count license-manager.entitlement-consumed-count-tracks-checkout-and-checkin license-manager.overage-entitlement-allows-consumption-past-max-count
Issue-Shape-Id: com.amazonaws.licensemanager#AWSLicenseManager
Issue-Key: license-manager.checked-in-consumption-token-is-not-reusable|_make_license#2|KeyError|fingerprint = _rl(
Issue-Key: license-manager.checkout-of-undeclared-entitlement-name-rejected|_make_license#2|KeyError|fingerprint = _rl(
Issue-Key: license-manager.checkout-past-entitlement-max-count-rejected|_make_license#2|KeyError|fingerprint = _rl(
Issue-Key: license-manager.checkout-with-mismatched-entitlement-unit-rejected|_make_license#2|KeyError|fingerprint = _rl(
Issue-Key: license-manager.concurrent-checkouts-never-exceed-entitlement-max-count|_make_license#3|KeyError|fingerprint = _rl(
Issue-Key: license-manager.entitlement-consumed-count-tracks-checkout-and-checkin|_make_license#2|KeyError|fingerprint = _rl(
Issue-Key: license-manager.overage-entitlement-allows-consumption-past-max-count|_make_license#2|KeyError|fingerprint = _rl(
Issue-Commit: b96b6334f7964c8f0db5f0da0179882059e96e50

# License Manager: issuer KeyFingerprint never derived — GetLicense returns License.Issuer without KeyFingerprint, blocking every CheckoutLicense flow

> _Note: this issue was identified against the code base at commit `b96b6334f7964c8f0db5f0da0179882059e96e50`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLicense

## Description
One root cause, reached by all 7 license-manager behavioral checks.

Cause 1 of 1: a license created with CreateLicense has no issuer key fingerprint in the emulator's state, so GetLicense serializes License.Issuer with no KeyFingerprint member. Failing checks: license-manager.checked-in-consumption-token-is-not-reusable, license-manager.checkout-of-undeclared-entitlement-name-rejected, license-manager.checkout-past-entitlement-max-count-rejected, license-manager.checkout-with-mismatched-entitlement-unit-rejected, license-manager.concurrent-checkouts-never-exceed-entitlement-max-count, license-manager.entitlement-consumed-count-tracks-checkout-and-checkin, license-manager.overage-entitlement-allows-consumption-past-max-count.

AWS: Smithy (com.amazonaws.licensemanager): CreateLicense input member Issuer targets shape Issuer, whose members are Name (required) and SignKey — a caller cannot supply a fingerprint. The read model is different: License.Issuer targets IssuerDetails, which adds KeyFingerprint ("Issuer key fingerprint"). So the fingerprint is a SERVICE-DERIVED value: License Manager computes it for the issuer at license creation and returns it on reads of the license (GetLicense/ListLicenses). That derived value is load-bearing, not cosmetic: CheckoutLicense input declares KeyFingerprint as @required alongside ProductSKU, and (ProductSKU, KeyFingerprint) is the key a checkout resolves to a license. Recorded behavior confirms a license created with Issuer={"Name": <name>} and no SignKey still comes back from GetLicense with a non-empty License.Issuer.KeyFingerprint, which the checks then feed straight into CheckoutLicense.

Emulator: CreateLicense is accepted (it returns LicenseArn and Version, and the checks proceed past it), and GetLicense returns a License whose Issuer is present but carries no KeyFingerprint member at all. Every check fails identically at the same line of its inlined helper: `lm.get_license(LicenseArn=arn)["License"]["Issuer"]["KeyFingerprint"]` raises `KeyError: 'KeyFingerprint'`. The failure is in the response shape, not in an assertion — nothing downstream of license creation (checkout, check-in, extend, usage counting, overage) was exercised at all.

Why: a client that creates a license and then checks entitlements out of it — the normal License Manager consumption flow — cannot proceed. It has no way to obtain the KeyFingerprint that CheckoutLicense requires, because the model gives no other source for it: it is not an input to CreateLicense and not in the CreateLicense response. Clients either crash on the missing key (as here) or send an empty/invented fingerprint and get a checkout that cannot be matched to the license. This also silently hides the entire entitlement-counting surface of the emulator behind one missing field.

Fix: the License Manager license state plus the CreateLicense handler — derive a stable per-issuer fingerprint when a license is created (same issuer identity ⇒ same fingerprint, distinct issuers ⇒ distinct fingerprints, so the (ProductSKU, KeyFingerprint) checkout key discriminates) and persist it with the license — and the GetLicense serializer, which must emit it as License.Issuer.KeyFingerprint (IssuerDetails), alongside Name and SignKey. CheckoutLicense's lookup must then resolve the request's required KeyFingerprint against that stored value. The concrete fingerprint string is environment-specific and is not itself a requirement; its presence, stability and use as the checkout key are.

Traces: the 7 check ids listed above, all erroring at `_make_license` -> GetLicense with KeyError: 'KeyFingerprint'.

## Evidence
Suite report (0/7 license-manager checks passed; 7 errored, 0 mismatched). Representative failure, identical in all 7:

  #### [error] license-manager.checked-in-consumption-token-is-not-reusable
  File "check_license_manager_checked_in_consumption_token_is_not_reusable.py", line 30, in run
    sku, fingerprint, arn = _make_license(ctx, lm, max_count=2, overage=False)
  File "...", line 165, in _make_license
    fingerprint = _rl(lambda: lm.get_license(LicenseArn=arn))["License"]["Issuer"]["KeyFingerprint"]
  KeyError: 'KeyFingerprint'

The other six error at the same helper line (lines 147/155/140/138/131/167 of their respective check files) with the same KeyError.

Expected: GetLicense -> 200 with License.Issuer (IssuerDetails) containing Name and a service-derived, non-empty KeyFingerprint usable as CheckoutLicense.KeyFingerprint.
Actual: GetLicense -> 200 with License.Issuer present but KeyFingerprint absent, so the required CheckoutLicense input is unobtainable.

Model citations: Smithy Issuer = {Name (required), SignKey}; Smithy IssuerDetails = {Name, SignKey, KeyFingerprint}; License.Issuer -> IssuerDetails; CheckoutLicense input KeyFingerprint @required (also ProductSKU, CheckoutType, Entitlements, ClientToken required).

## Steps to Reproduce
1. sts.get_caller_identity() to get the account id.
2. license-manager CreateLicense with LicenseName/ProductName/ProductSKU unique, Issuer={"Name": "<issuer-name>"} (no SignKey), HomeRegion=<region>, Validity={Begin,End} ISO8601, Entitlements=[{Name:"sim2real-seats",Unit:"Count",MaxCount:2,Overage:false,AllowCheckIn:true}], Beneficiary=<account>, ConsumptionConfiguration={RenewType:"None",ProvisionalConfiguration:{MaxTimeToLiveInMinutes:60}}, ClientToken=<uuid>. Note the returned LicenseArn/Version.
3. license-manager GetLicense(LicenseArn=<arn>) and inspect License.Issuer.
   Observed: no KeyFingerprint member (KeyError in the client).
   Expected: Issuer.KeyFingerprint present and non-empty.
4. Attempt CheckoutLicense(ProductSKU=<sku>, CheckoutType="PROVISIONAL", KeyFingerprint=<the value from step 3>, Entitlements=[{Name:"sim2real-seats",Unit:"Count",Value:"1"}], ClientToken=<uuid>, Beneficiary=<account>) — unreachable today because step 3 yields no fingerprint.
