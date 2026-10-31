Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: license-manager.concurrent-checkouts-never-exceed-entitlement-max-count
Issue-Shape-Id: com.amazonaws.licensemanager#AWSLicenseManager
Issue-Key: license-manager.concurrent-checkouts-never-exceed-entitlement-max-count|run#11|AssertionError|assert len(granted) == 2, (
Issue-Commit: 30fa5e7e24c2e7ff08a1ee60d32c6c6e65abb481

# CheckoutLicense enforces no entitlement MaxCount under concurrency: six parallel checkouts against MaxCount=2 are all refused NotImplemented and ConsumedValue never moves

> _Note: this issue was identified against the code base at commit `30fa5e7e24c2e7ff08a1ee60d32c6c6e65abb481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CheckoutLicense

## Description
Cause 1 of 1: CheckoutLicense does not serve a checkout at all, so the atomic entitlement-cap contract it is supposed to hold under concurrent callers is unobservable and GetLicenseUsage never reflects any consumption.

Related open issue: `20261030T001530Z-license-manager-checkoutlicense-stub.md` asks for the CheckoutLicense scaffold to be implemented and lists 4 blocked checks; it does NOT carry this check's replay key (`license-manager.concurrent-checkouts-never-exceed-entitlement-max-count`) nor state the concurrency/atomicity requirement, so that requirement is filed here. The two close together: the handler must exist AND its counter update must be atomic.

AWS: Smithy (com.amazonaws.licensemanager#AWSLicenseManager) models CheckoutLicense with required ProductSKU, CheckoutType, KeyFingerprint, Entitlements, ClientToken and output LicenseConsumptionToken / EntitlementsAllowed / SignedToken / IssuedAt / Expiration / LicenseArn, and among its errors `NoEntitlementsAllowedException` (client, HTTP 400). On real AWS a self-managed license whose entitlement has Unit=Count, MaxCount=2, Overage=false grants at most 2 concurrently-held units: a checkout that would push ConsumedValue past MaxCount is refused with NoEntitlementsAllowedException (HTTP 400) — the fixed text of that error class, carrying no values from this request. Each granted checkout mints its own distinct LicenseConsumptionToken. GetLicenseUsage (Smithy: GetLicenseUsage -> LicenseUsage.EntitlementUsages) then reports that entitlement's ConsumedValue as the string count of units currently held ("0" when fresh, "2" when both grants are out, back to "0" after both are checked in) alongside MaxCount="2". The decisive part is that this cap is a single atomic read-modify-write on the entitlement counter: when N callers race, exactly MaxCount of them win and the rest get NoEntitlementsAllowedException — a read-then-write implementation oversells and is a defect even though every individual response is well-shaped.

Emulator: every one of the 6 parallel CheckoutLicense calls is refused with error code `NotImplemented` ("operation CheckoutLicense is scaffolded but not implemented in this emulator"), so 0 of 6 were granted where exactly 2 must be, no consumption token is ever minted, and GetLicenseUsage therefore cannot move ConsumedValue off "0" or back down after a check-in. Replay detail: `6 concurrent checkouts against MaxCount=2 granted 0, not 2 ... Outcomes: [('refused', 'NotImplemented') x6]`.

Why: a client that provisions seats through License Manager gets no entitlement at all — every checkout fails with an error code AWS does not model for this operation, so licensed workloads never start. Once the handler exists, a non-atomic counter is the next failure mode a client sees: it would be granted more units than the entitlement allows (overselling a MaxCount=2 entitlement to 6 holders), and ConsumedValue would settle above MaxCount, which no AWS client is written to defend against. The refusal must be NoEntitlementsAllowedException/400, not a 501 NotImplemented and not success.

Fix: the CheckoutLicense handler and the license-manager entitlement state it writes — the consumption counter update must be a single atomic compare-and-set (or equivalent lock over the entitlement record) that admits at most MaxCount units when Overage is false, mints a fresh LicenseConsumptionToken per grant, and is the same counter GetLicenseUsage/CheckInLicense read and release, so a burst of concurrent checkouts cannot exceed the cap.

Traces: license-manager.concurrent-checkouts-never-exceed-entitlement-max-count

## Evidence
Check: license-manager.concurrent-checkouts-never-exceed-entitlement-max-count (verdict: mismatch)

Setup: CreateLicense with one entitlement {Unit: Count, MaxCount: 2, Overage: false, AllowCheckIn: true}; GetLicenseUsage confirms (ConsumedValue, MaxCount) == ("0", "2"). Then 6 CheckoutLicense calls are fired in parallel on the same (ProductSKU, KeyFingerprint), each asking for Value "1".

Expected (real AWS): exactly 2 outcomes are grants, each with a distinct LicenseConsumptionToken; the other 4 are refused with error code NoEntitlementsAllowedException (HTTP 400); GetLicenseUsage then reports ConsumedValue "2" with MaxCount "2"; after both tokens are checked in it reports "0".

Actual (emulator): all 6 outcomes are refusals with code NotImplemented:
  AssertionError: 6 concurrent checkouts against MaxCount=2 granted 0, not 2 — the cap must hold under concurrency. Outcomes: [('refused', 'NotImplemented'), ('refused', 'NotImplemented'), ('refused', 'NotImplemented'), ('refused', 'NotImplemented'), ('refused', 'NotImplemented'), ('refused', 'NotImplemented')]
  (botocore surfaces: An error occurred (NotImplemented) when calling the CheckoutLicense operation: operation CheckoutLicense is scaffolded but not implemented in this emulator)

Message parts: for the required refusal, `NoEntitlementsAllowedException` is the fixed error code of the class and its message is the fixed text of that class — no part of it is taken from the request. The emulator's `NotImplemented` code and scaffold message are entirely emulator-authored and are modeled nowhere in CheckoutLicense's error list.

## Steps to Reproduce
1. Boot the license-manager emulator.
2. CreateLicense with HomeRegion set to the client region, a unique ProductSKU/Issuer, Validity spanning now..now+1d, Beneficiary = caller account, ConsumptionConfiguration {RenewType: None, ProvisionalConfiguration: {MaxTimeToLiveInMinutes: 60}}, and Entitlements=[{Name: "sim2real-seats", Unit: "Count", MaxCount: 2, Overage: false, AllowCheckIn: true}].
3. GetLicenseUsage(LicenseArn) — observe the entitlement usage reads ConsumedValue "0", MaxCount "2".
4. GetLicense(LicenseArn) to read Issuer.KeyFingerprint.
5. From 6 threads concurrently call CheckoutLicense(ProductSKU, CheckoutType="PROVISIONAL", KeyFingerprint, Entitlements=[{Name: "sim2real-seats", Unit: "Count", Value: "1"}], ClientToken=<unique per call>, Beneficiary=<account>).
6. Observe all 6 are refused with code NotImplemented (expected: exactly 2 grants with distinct LicenseConsumptionToken values, 4 refusals with NoEntitlementsAllowedException / HTTP 400).
7. GetLicenseUsage(LicenseArn) — observe ConsumedValue still "0" (expected "2"); after CheckInLicense on both granted tokens it must return to "0".
