Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7959e6ca7cb81d5c32ad32f8e1e29786f97ad065

# GetPoliciesStats counts only saml security configurations, so iamfederation/iamidentitycenter configurations are reported in no counter and missing from TotalPolicyCount

_Filed: 2026-10-28T10:01:50Z_
_Operation: GetPoliciesStats | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7959e6ca7cb81d5c32ad32f8e1e29786f97ad065`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`SecurityConfigStats$SamlConfigCount` is documented as "The number of security configurations in the current account" and `TotalPolicyCount` as "The total number of OpenSearch Serverless security policies and configurations in your account". The handler counts only the records whose type is `saml` (`store.security_config_count(crate::state::SECURITY_CONFIG_SAML)`) and then computes `TotalPolicyCount` as the sum of the five typed counters. `CreateSecurityConfig` in this same crate accepts and stores `iamfederation` and `iamidentitycenter` configurations, so a configuration this account demonstrably holds is counted in no counter at all AND is absent from the documented account-wide total — a create that the emulator accepted raises nothing in the next GetPoliciesStats call.

## Evidence

Model text (smithy_query shape=SecurityConfigStats): member `SamlConfigCount` — "The number of security configurations in the current account." Model text (smithy_query doc shape=GetPoliciesStatsResponse member=TotalPolicyCount) — "The total number of OpenSearch Serverless security policies and configurations in your account."

The handler counts one type only and bases the total on its own parts: "counter(store.security_config_count(crate::state::SECURITY_CONFIG_SAML))," — src/ops/get_policies_stats.rs:172 — under the comment "// The response models one counter per type value, so each counter counts exactly its\n        // own type: a configuration of a type the response models no counter for is not\n        // reported under another type's counter." (src/ops/get_policies_stats.rs:157-159), and "// `TotalPolicyCount` is \"the total number of OpenSearch Serverless security\n        // policies and configurations in your account\", which is the sum of the counters\n        // reported beside it" with `let total_policy_count = data_policy_count.saturating_add(...)` — src/ops/get_policies_stats.rs:177-184.

The crate stores the uncounted types: `CreateSecurityConfig` admits `iamfederation` and `iamidentitycenter` (src/ops/create_security_config.rs:195-220) and inserts the record (src/ops/create_security_config.rs:352).

Test coverage: tests/all/policies_stats.rs:250-254 and :275-279 assert the current answer ("an IAM-federation configuration, a type with no counter of its own" leaves every counter and the total unchanged), i.e. the behaviour contradicting the two quoted sentences is pinned rather than derived from them.

## Steps to Reproduce

1. GetPoliciesStats on an empty account → every counter and TotalPolicyCount are 0.
2. CreateSecurityConfig(type="iamfederation", name="fed-one", iamFederationOptions={userAttribute:"Email"}) → 200, the configuration is stored (GetSecurityConfig on the returned id answers it).
3. GetPoliciesStats again → SecurityConfigStats.SamlConfigCount is still 0 and TotalPolicyCount is still 0, although the account now holds one security configuration and TotalPolicyCount is documented as the total number of policies AND configurations in the account.
