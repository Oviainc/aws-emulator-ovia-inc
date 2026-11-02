Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 307ae70e1176afe8148743d9dbd874f3de53a94d

# A credit/{creditId}/status preference is stored for a creditId no credit exists under, although the same loop refuses an account-list key naming an account outside the family

_Filed: 2026-11-02T05:56:47Z_
_Operation: UpdateBillingPreferences | Class: validation_

> _Note: this issue was identified against the code base at commit `307ae70e1176afe8148743d9dbd874f3de53a94d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For `feature = CREDIT_PREFERENCE_OPTIONS` the handler checks only the SHAPE of each key (`credit/{creditId}/{option}`, all keys naming the same `creditId`) and then writes it. The `creditId` segment is never resolved against `state.credits`, the store this crate keeps keyed by exactly that id, so `credit/999/status` — and even `credit/CR-ABC-123/status`, whose id cannot be a modelled `CreditId` (`^[0-9]+$`, length 1..32) at all — is accepted with 200 and persisted as a preference of a credit that does not exist and is in no consolidated billing family.

The same handler, in the very same per-entry loop, DOES resolve the other resource-naming key class against stored state: an account-list key whose trailing segment names an account that is not in `consolidated_billing_family` is refused with `AccessDeniedException`. So one class of key that names a resource is validated against the store and the parallel class is not, even though the modelled sentence the crate quotes beside the constant scopes the credit key to a specific credit's ownership ("for credits they own, or ... for any credit in the family").

Consequence: `GetBillingPreferences(CREDIT_PREFERENCE_OPTIONS)` reports per-credit preferences for credits `GetCredits` knows nothing about, with no error ever telling the caller the credit it named does not exist.

## Evidence

The modelled sentence that scopes the key — `smithy_query(query="doc", shape="UpdateBillingPreferences")`: "The <code>credit/{creditId}/status</code> key may be set by member accounts for credits they own, or by the management account for any credit in the family."
The crate restates it verbatim beside the constant — services/service-com-amazonaws-billing-awsbilling/src/state.rs:230-234:
```
/// The one `CREDIT_PREFERENCE_OPTIONS` option that is NOT a sharing key: the
/// `UpdateBillingPreferences` doc singles out `credit/{creditId}/status` as
/// settable "by member accounts for credits they own, or by the management
/// account for any credit in the family".
pub const CREDIT_STATUS_OPTION: &str = "status";
```
and names the key as identifying one credit — state.rs:222-224: "/// `CREDIT_PREFERENCE_OPTIONS`, the one `BillingFeature` whose keys name a single\n/// credit (`credit/{creditId}/status`, from the `UpdateBillingPreferences` doc)."

The store that holds those credits, keyed by the same id — state.rs:589-592:
```
    /// Every credit held by an account of this emulator's consolidated billing
    /// family, keyed and ordered by `creditId`, so `GetCredits` answers in a
    /// deterministic order. Bounded by [`MAX_CREDITS`].
    pub credits: BTreeMap<String, Credit>,
```
and state.rs:477-479: "/// The modeled `CreditId`: 1..32 characters matching `^[0-9]+$`. This is\n/// the key the credit is stored under.\n    pub credit_id: String,".

The handler's credit-key path, which validates shape only — services/service-com-amazonaws-billing-awsbilling/src/ops/update_billing_preferences.rs:248-272:
```
        if feature == crate::state::CREDIT_PREFERENCE_OPTIONS_FEATURE {
            let mut credit: Option<&str> = None;
            for preference in &input.billing_preferences_per_key {
                let Some((credit_id, _)) = crate::state::credit_key_parts(&preference.key) else {
                    return Err(invalid_field(
                        KEYS_MEMBER,
                        &format!(
                            "key {} is not valid for {}: name a credit as credit/{{creditId}}/{{option}}",
```
`credit_key_parts` accepts ANY non-empty middle segment — state.rs:987-997:
```
pub fn credit_key_parts(key: &str) -> Option<(&str, &str)> {
    let mut segments = key.split('/');
    let prefix = segments.next()?;
    let credit_id = segments.next()?;
    let option = segments.next()?;
    let extra = segments.next();
    if prefix != CREDIT_KEY_PREFIX || credit_id.is_empty() || option.is_empty() || extra.is_some() {
        return None;
    }
```
The parallel check the SAME loop does perform for the other resource-naming key class — update_billing_preferences.rs:308-314:
```
            if let Some(account) = crate::state::account_list_key_account(&preference.key) {
                if state.family_account_name(account).is_none() {
                    return Err(access_denied(format!(
                        "Account {account} is not a member of this consolidated billing family"
                    )));
                }
            }
```
and the write that follows with no credit lookup — update_billing_preferences.rs:334-337:
```
        let now = ::std::time::SystemTime::now();
        for preference in &input.billing_preferences_per_key {
            state.set_preference(feature, &preference.key, &preference.value, now);
        }
```
(`grep -n "state.credits\|credits_for_period\|insert_credit" src/ops/update_billing_preferences.rs` → no match; the only reader of `credits` is `get_credits.rs`.)

Test coverage: the existing tests pin acceptance of ids that are not even modelled `CreditId`s, with no stated reason for skipping the lookup — tests/all/billing_preferences.rs:897-912 (`the_credit_status_key_is_settable_and_read_back`, key `credit/CR-ABC-123/status`, asserting 200) and tests/all/billing_feature_access.rs:54-56 (`CREDIT_KEY: &str = "credit/CR-ACCESS-1/status"`). No test sends a `credit/{creditId}/status` key for a credit actually present in `state.credits`, and none asserts a refusal for one that is absent.

## Steps to Reproduce

1. Boot the emulator and point an awsJson1_0 `billing` client at it (granting CREDIT_PREFERENCE_OPTIONS access as tests/all/billing_preferences.rs:67-82 does if the default gate refuses).
2. Call GetCredits(accountId="000000000000", startDate=<a recent past instant>) — observe `credits: []`, i.e. the account holds no credit at all.
3. Call UpdateBillingPreferences(feature="CREDIT_PREFERENCE_OPTIONS", billingPreferencesPerKey=[{key:"credit/999/status", value:"DISABLED"}]) — observe 200.
4. Call GetBillingPreferences(features=["CREDIT_PREFERENCE_OPTIONS"]) — observe the stored `credit/999/status` = DISABLED for a credit that exists nowhere in the store.
5. Contrast the sibling key class: call UpdateBillingPreferences(feature="CREDIT_SHARING", billingPreferencesPerKey=[{key:"account/999999999999", value:"ENABLED"}]) — observe 403 AccessDeniedException "Account 999999999999 is not a member of this consolidated billing family" (pinned by tests/all/billing_preferences.rs:789-798).
6. Also call UpdateBillingPreferences(feature="CREDIT_PREFERENCE_OPTIONS", billingPreferencesPerKey=[{key:"credit/CR-ABC-123/status", value:"ENABLED"}]) — observe 200 for a creditId that cannot match the modelled `CreditId` pattern `^[0-9]+$`.
