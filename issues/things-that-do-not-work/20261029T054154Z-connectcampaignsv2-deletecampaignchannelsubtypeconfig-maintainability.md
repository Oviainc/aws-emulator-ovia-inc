Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: be7f7e560904e5f7d0ba877d90da2eae21b18549

# The enum-catalog backstop refusal is hand-rolled in five handlers, although the crate centralizes the identical rule for enum-keyed maps

_Filed: 2026-10-29T05:41:54Z_
_Operation: DeleteCampaignChannelSubtypeConfig | Class: maintainability_

> _Note: this issue was identified against the code base at commit `be7f7e560904e5f7d0ba877d90da2eae21b18549`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Five handlers each hand-roll the same rule: "this scalar member must hold a value from the model's own enum catalog, or the call is a ValidationException naming the member and listing the catalog". The branch, the catalog lookup and the sentence are re-written in each one:

* `DeleteCampaignChannelSubtypeConfig` for `channelSubtype` against `CHANNEL_SUBTYPE_VALUES`
* `DeleteCampaignCommunicationLimits` for `config` against `COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES`
* `DeleteCampaignCommunicationTime` for `config` against `COMMUNICATION_TIME_CONFIG_TYPE_VALUES`
* `DeleteConnectInstanceConfig` for `campaignDeletionPolicy` against `CAMPAIGN_DELETION_POLICY_VALUES`
* `StartInstanceOnboardingJob` for `encryptionConfig.encryptionType` against `ENCRYPTION_TYPE_VALUES`

This crate already centralizes exactly this rule for the one place the boundary walk cannot reach — an enum-keyed MAP — in `crate::state::enum_map_keys_violation`, whose doc calls itself "The one place an enum-keyed map's value set is enforced, shared by every operation that accepts such a map", and which renders the refusal through the framework's own `ViolationKind::EnumValue` + `constraints::envelope`. The five scalar copies bypass it, so the one rule lives in six places.

The copies have ALREADY drifted, which is what makes this a maintainability defect rather than taste: three say "supported types are", one says "supported subtypes are", one says "supported policies are"; four test the member directly while `DeleteConnectInstanceConfig` tests it through `policy.filter(..)` and `StartInstanceOnboardingJob` nests it in a private `check_encryption_config` with its own `refuse` closure. A sixth operation added later will pick whichever copy its author happens to read.

The fix is response-preserving: hoist one helper beside `enum_map_keys_violation` in `state.rs` that takes the member path, the catalog and the value and renders the same sentence the copies render today, and have the five handlers call it. No status, error shape or message text needs to change, and no existing test asserts any of these sentences (see evidence).

## Evidence

The rule the crate already centralizes — services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/state.rs:394-412:

```
/// Why the keys of the MAP at `member` are a bad request, or `None` when every one of
/// them is a value the key shape's catalog (`allowed`) holds.
///
/// The one place an enum-keyed map's value set is enforced, shared by every operation
/// that accepts such a map — a key the walk never validates, refused through
/// [`map_keys_violation`] exactly as a tag key is.
pub fn enum_map_keys_violation<'k>(
    member: &str,
    allowed: &[&str],
    keys: impl IntoIterator<Item = &'k str>,
) -> Option<String> {
    map_keys_violation(member, keys, |key| {
        (!allowed.contains(&key)).then(|| {
            ::smithy_server_runtime::ViolationKind::EnumValue(
                allowed.iter().map(|value| (*value).to_string()).collect(),
            )
        })
    })
}
```

Copy 1 — src/ops/delete_campaign_channel_subtype_config.rs:181-196:

```
        if !CHANNEL_SUBTYPE_VALUES.contains(&input.channel_subtype.as_str()) {
            return Err(
                DeleteCampaignChannelSubtypeConfigErrorModeled::ValidationException(
                    ValidationException {
                        message: format!(
                            "channelSubtype: {} is not a supported channel subtype; \
                             supported subtypes are {}",
                            input.channel_subtype,
                            CHANNEL_SUBTYPE_VALUES.join(", ")
                        ),
```

Copy 2 — src/ops/delete_campaign_communication_limits.rs:184-199:

```
        if !COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES.contains(&input.config.as_str()) {
            return Err(
                DeleteCampaignCommunicationLimitsErrorModeled::ValidationException(
                    ValidationException {
                        message: format!(
                            "config: {} is not a supported communication limits config type; \
                             supported types are {}",
                            input.config,
                            COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES.join(", ")
                        ),
```

Copy 3 — src/ops/delete_campaign_communication_time.rs:184-199 (same shape, `COMMUNICATION_TIME_CONFIG_TYPE_VALUES`, "is not a supported communication time config type").

Copy 4 — src/ops/delete_connect_instance_config.rs:189-213, the same rule written through a different binding form:

```
        let policy = input.campaign_deletion_policy.as_deref();
        if let Some(unknown) =
            policy.filter(|named| !CAMPAIGN_DELETION_POLICY_VALUES.contains(named))
        {
            ...
                    message: format!(
                        "campaignDeletionPolicy: {unknown} is not a supported campaign deletion \
                         policy; supported policies are {}",
                        CAMPAIGN_DELETION_POLICY_VALUES.join(", ")
                    ),
```

Copy 5 — src/ops/start_instance_onboarding_job.rs:151-159, the same rule inside a private fn with its own `refuse` closure:

```
        if let Some(encryption_type) = config.encryption_type.as_deref() {
            if !ENCRYPTION_TYPE_VALUES.contains(&encryption_type) {
                return refuse(format!(
                    "encryptionConfig.encryptionType: {encryption_type} is not a supported encryption \
                     type; supported types are {}",
                    ENCRYPTION_TYPE_VALUES.join(", ")
                ));
```

All five carry the same comment justifying their existence, e.g. src/ops/delete_campaign_communication_time.rs:179-183: "A backstop rather than the first line of defence: `config` targets a modelled enum, so the boundary validator refuses an off-catalog value before `handle` runs — but only this check keeps the handler's own contract if that walk ever widens."

Test coverage, and why centralizing changes nothing a test sees: tests/all/campaign_channel_subtype_config_deletion.rs:500-547 (`a_channel_subtype_outside_the_catalog_is_refused`) states at :509-511 "The status and the error shape are what every row asserts, not a message: an off-catalog enum value is refused by the boundary validator before the handler runs, so which of the two layers composed the text is not part of the rule", and asserts only `status == 400` and `error_code(..).ends_with("ValidationException")` (:531-539). The sibling suites do the same — tests/all/campaign_communication_limits_deletion.rs:458-480, tests/all/campaign_communication_time_deletion.rs:404-405, tests/all/instance_mutation_audit.rs:252-265 ("config delete refused at the boundary: off-catalog campaign deletion policy"). `grep -rn "is not a supported" tests/` returns nothing, so no test pins any of the five sentences.

## Steps to Reproduce

1. Run `grep -rn "is not a supported" services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/` — five handlers match: delete_campaign_channel_subtype_config.rs:186, delete_campaign_communication_limits.rs:189, delete_campaign_communication_time.rs:189, delete_connect_instance_config.rs:205, start_instance_onboarding_job.rs:154.
2. Read each block and confirm all five implement the same rule (catalog membership on a scalar member -> ValidationException naming the member and joining the catalog), already worded three different ways ("supported subtypes are" / "supported types are" / "supported policies are") and structured three different ways.
3. Read src/state.rs:394-412 and confirm the crate already owns this rule once for enum-keyed MAPS (`enum_map_keys_violation`, rendering through `ViolationKind::EnumValue` + `constraints::envelope`), and that nothing in `state.rs` offers the scalar counterpart the five handlers need.
4. Run `grep -rn "is not a supported" services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/tests/` — no matches, so no test asserts any of these sentences; the suites assert only the 400 status and the `ValidationException` shape (tests/all/campaign_channel_subtype_config_deletion.rs:531-539).
5. Hoist one helper beside `enum_map_keys_violation` that renders the existing sentence for a scalar member, call it from all five handlers, and re-run `cargo test -p service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2`: every response and every test stays as it is.
