Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-ui-customization-and-risk-configuration
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-ui-customization-and-risk-configuration cognito-idp-csv-header-schema-matches-pool-custom-attributes cognito-idp-get-csv-header-matches-user-pool-schema cognito-idp-user-import-readiness-and-lookup
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|CreateUserPool|2xx->4xx|status
Issue-Key: cognito-idp|GetCSVHeader|2xx->2xx|$.CSVHeader[*]
Issue-Commit: c5521ae0ee2e634b97167759371926181b746de0

# cognito-idp: CreateUserPool refuses a valid pool configuration with HTTP 400, and GetCSVHeader omits password_hash and ignores the pool's schema order (3 causes)

> _Note: this issue was identified against the code base at commit `c5521ae0ee2e634b97167759371926181b746de0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserPool

## Description
Three distinct, net-new root causes in the cognito-idp wire surface, all from the simparity-wire-aws-v1 replay. None carries a stub_path and none is marked owned_by any open issue in the replay report. Related-but-different open issues, named so a fixer does not conflate them: 20261029T184956Z-cognito-idp-user-pool-reads-omit-server-set.md covers only the DEFAULT BLOCKS a successful CreateUserPool response omits (EmailConfiguration/KeyConfiguration/LambdaConfig/...), not a CreateUserPool that is refused outright (Cause 1); 20261030T174613Z-cognito-idp-the-user-import-plane-diverges-a.md is a BEHAVIORAL-corpus issue whose Cause 3 describes the same password_hash omission but carries no wire replay key for it and says nothing about column order (Causes 2 and 3).

=== Cause 1 of 3: CreateUserPool answers HTTP 400 for a pool configuration AWS creates, so the whole UI-customization/risk-configuration scenario is unreachable ===
AWS: CreateUserPool "Creates a new Amazon Cognito user pool. This operation sets basic and advanced configuration options. If you don't provide a value for an attribute, Amazon Cognito sets it to its default value" (Smithy: CreateUserPool doc) and answers HTTP 200 with the output member UserPool -> UserPoolType. Its modeled client refusals are InvalidParameterException, LimitExceededException, InvalidEmailRoleAccessPolicyException, InvalidSmsRoleAccessPolicyException, InvalidSmsRoleTrustRelationshipException and UserPoolTaggingException (all HTTP 400), FeatureUnavailableInTierException, NotAuthorizedException and TierChangeNotAllowedException (403) and TooManyRequestsException (429). The recorded response for this step is HTTP 200 with the created pool, so for this request none of those refusals applies: the pool configuration this scenario sends is one AWS accepts.
Emulator: the identical request is answered HTTP 400 — the pool is never created. Because the replay compares status first, the refusal's body (__type / message / x-amzn-errortype) is not carried in the divergence record, so the error code the emulator chose is not quoted here; the requirement is simply that this request succeed with 200 and the stored pool, not that it carry any particular refusal.
Why: a client cannot create a user pool with this (threat-protection + hosted-UI) configuration at all. Every dependent step of the scenario — SetUICustomization/GetUICustomization and SetRiskConfiguration/DescribeRiskConfiguration against that pool — is blocked rather than merely wrong, so a whole feature area of the emulator is unexercisable and a client that creates its pool this way fails at setup.
Fix: CreateUserPool's request-validation path, i.e. the shared input-constraint validation the framework runs before handle() plus services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/create_user_pool.rs. Hypothesis: the refusal comes from the pre-handler constraint validation enforcing a modeled constraint more strictly than AWS on one of the pool-configuration members this scenario supplies (the same validator that renders the "1 validation error detected: Value '...' at '...' failed to satisfy constraint ..." refusals elsewhere in this service), because the only refusals the handler itself can raise are FeatureUnavailableInTierException (HTTP 403, via crate::state::pool_feature_plan_refusal, which fires only for AdvancedSecurityMode=ENFORCED below the Plus tier or a choice-based SignInPolicy below Essentials) and LimitExceededException (HTTP 400, but only at crate::state::MAX_USER_POOLS = 1000 stored pools, far above what this corpus creates). Re-running just this step and reading the 400's __type/message names the member to relax. Note that sibling fixtures prove the near-neighbour configurations are accepted: cognito-idp-risk-configuration-lifecycle and cognito-idp-risk-configuration-invalid-parameter-error both create a pool with UserPoolAddOns (AdvancedSecurityAdditionalFlows CustomAuthMode=AUDIT) and are answered 200, so the refusal is specific to a member this fixture adds.
Traces: cognito-idp-ui-customization-and-risk-configuration#setup#1 (CreateUserPool, expected 200, got 400).

=== Cause 2 of 3: GetCSVHeader omits the password_hash column ===
AWS: Smithy models GetCSVHeader's output as UserPoolId plus CSVHeader (target ListOfStringTypes) and documents nothing about the list's contents, so: recorded behavior only. Both recordings show AWS returning the pool's standard import columns, one custom:<name> column per custom schema attribute, cognito:mfa_enabled and cognito:username, AND a final password_hash column (the column that lets an import job carry pre-hashed passwords). It is the last element in both recordings: index 22 for a pool that has one custom attribute, index 21 for a pool with the default schema.
Emulator: the answered list contains every other column but has no password_hash entry at all, so it is exactly one element short (replay: "$.CSVHeader[22]: missing (expected 'password_hash', got None)" and "$.CSVHeader[21]: missing (expected 'password_hash', got None)"). Status is 200 in both cases; only the body diverges.
Why: a client that builds its import CSV from the pool's own GetCSVHeader template has no password_hash column, so the pre-hashed-password user-import path (CreateUserImportJob with a password hashing algorithm) cannot be driven from the template the service itself hands out, and any code that locates that column by name or index reads the wrong column.
Fix: the GetCSVHeader handler and the CSV-header derivation it reads out of state — services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/get_csv_header.rs and the header-building helper in src/state.rs — must append password_hash to the standard-plus-custom column set it renders, for every pool.
Traces: cognito-idp-user-import-readiness-and-lookup#task#2 (the isolated case: password_hash is the ONLY diff) and cognito-idp-csv-header-schema-matches-pool-custom-attributes#task#3.

=== Cause 3 of 3: CSVHeader column order is a fixed emulator order instead of the pool's own schema order ===
AWS (recorded behavior only — the model documents no ordering for ListOfStringTypes): for the pool created with an explicit Schema, the recorded CSVHeader order is [profile, address, birthdate, gender, preferred_username, updated_at, website, picture, email_verified, phone_number_verified, phone_number, zoneinfo, custom:<custom attribute>, locale, email, given_name, family_name, middle_name, name, nickname, cognito:mfa_enabled, cognito:username, password_hash] — note the custom column appears in the MIDDLE of the standard ones, not appended after them.
Emulator: it renders one fixed canonical order for every pool — [name, given_name, family_name, middle_name, nickname, preferred_username, profile, picture, website, email, email_verified, gender, birthdate, zoneinfo, locale, phone_number, phone_number_verified, address, updated_at, custom:<custom attribute>, cognito:mfa_enabled, cognito:username] — which mismatches 20 of the 22 positions of that recording (only index 7 'picture' and the two trailing cognito: columns land where AWS put them).
Hypothesis: AWS derives the CSVHeader order from the pool's STORED schema-attribute order rather than from a fixed table. The evidence is the second fixture: cognito-idp-user-import-readiness-and-lookup uses a default-schema pool and there the emulator's order matches AWS position for position (its only diff is the missing password_hash of Cause 2), while the pool built from an explicit Schema diverges in nearly every position. The emulator's stored schema order is already known to diverge from AWS's for such pools (see the DescribeUserPool SchemaAttributes[8] 'identities' vs 'picture' divergence of cognito-idp-pool-schema-and-ui-customization, owned by 20261029T184956Z-cognito-idp-user-pool-reads-omit-server-set.md), which is consistent with this root cause.
Why: the header row is the contract between the service and the CSV a client uploads to an import job. A client that writes its rows in the order the service just handed it, or that maps column index to attribute, writes every value into the wrong column against AWS while it appears consistent against the emulator.
Fix: the same GetCSVHeader header derivation (src/ops/get_csv_header.rs plus the header helper in src/state.rs) must emit the columns in the order of the pool's stored schema attributes (and that stored order must be the pool's own, as created), instead of a constant list, with password_hash last.
Traces: cognito-idp-csv-header-schema-matches-pool-custom-attributes#task#3 (also covers cognito-idp-get-csv-header-matches-user-pool-schema#task#2 in the same cluster).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200): 1613 steps, 1237 passed, score 76.7%, 103 divergence clusters — these three are the only clusters with no stub_path and an empty owned_by.

Cause 1 — cluster index 81:
  service=cognito-idp | CreateUserPool | status=200->400 | keys=status
  fixture_id: cognito-idp-ui-customization-and-risk-configuration, step_key: setup#1, affected_count 1
  detail: "status: expected 200, got 400"
  Expected: HTTP 200 with UserPool (the recorded create succeeded). Actual: HTTP 400, pool not created; the dependent UI-customization and risk-configuration steps of the fixture never run (the run reports 28 blocked steps overall).
  Contrast (same operation, accepted): cluster index 65, fixtures cognito-idp-risk-configuration-invalid-parameter-error#setup#1 and cognito-idp-risk-configuration-lifecycle#setup#1 — CreateUserPool with UserPoolAddOns is answered 200->200 there, so only the body diverges.

Cause 2 — cluster index 88:
  service=cognito-idp | GetCSVHeader | status=200->200 | keys=$.CSVHeader[21]
  fixture_id: cognito-idp-user-import-readiness-and-lookup, step_key: task#2, affected_count 1
  detail: "$.CSVHeader[21]: missing (expected 'password_hash', got None)"
  Expected: the pool's header list with password_hash as its last element. Actual: the same list with that element absent.

Cause 3 (and the second instance of Cause 2) — cluster index 39:
  service=cognito-idp | GetCSVHeader | status=200->200 | keys=$.CSVHeader[0],[1],[2],[3],[4],[5],[6],[8],[9],[10],[11],[12],[13],[14],[15],[16],[17],[18],[19],[22]
  fixture_id: cognito-idp-csv-header-schema-matches-pool-custom-attributes, step_key: task#3; affected also cognito-idp-get-csv-header-matches-user-pool-schema#task#2
  detail (verbatim): $.CSVHeader[0]: expected 'profile', got 'name'; [1]: expected 'address', got 'given_name'; [2]: expected 'birthdate', got 'family_name'; [3]: expected 'gender', got 'middle_name'; [4]: expected 'preferred_username', got 'nickname'; [5]: expected 'updated_at', got 'preferred_username'; [6]: expected 'website', got 'profile'; [8]: expected 'email_verified', got 'website'; [9]: expected 'phone_number_verified', got 'email'; [10]: expected 'phone_number', got 'email_verified'; [11]: expected 'zoneinfo', got 'gender'; [12]: expected 'custom:MyOddlyCasedFieldXyz', got 'birthdate'; [13]: expected 'locale', got 'zoneinfo'; [14]: expected 'email', got 'locale'; [15]: expected 'given_name', got 'phone_number'; [16]: expected 'family_name', got 'phone_number_verified'; [17]: expected 'middle_name', got 'address'; [18]: expected 'name', got 'updated_at'; [19]: expected 'nickname', got 'custom:MyOddlyCasedFieldXyz'; [22]: missing (expected 'password_hash', got None).
  Indexes 7, 20 and 21 matched ('picture', 'cognito:mfa_enabled', 'cognito:username'), which is how both the recorded and the served order above are reconstructed.

Code sites consulted: services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/create_user_pool.rs (handle(): the only refusals it can raise are FeatureUnavailableInTierException via crate::state::pool_feature_plan_refusal and LimitExceededException at crate::state::MAX_USER_POOLS) and src/state.rs (MAX_USER_POOLS = 1_000, declared an emulator cap and not an AWS quota; pool_feature_plan_refusal gates only AdvancedSecurityMode=ENFORCED and choice-based sign-in).

## Steps to Reproduce
Cause 1: replay fixture cognito-idp-ui-customization-and-risk-configuration, step setup#1 — CreateUserPool with the scenario's pool configuration (the hosted-UI + threat-protection/risk-configuration setup). Observe HTTP 400 instead of HTTP 200 with the created UserPool; dump the response body to read __type/message and identify which pool-configuration member the emulator's validation refuses. Compare with cognito-idp-risk-configuration-lifecycle#setup#1, whose CreateUserPool (UserPoolAddOns present) is accepted.

Cause 2: replay fixture cognito-idp-user-import-readiness-and-lookup, step task#2 — CreateUserPool with the default schema, then GetCSVHeader(UserPoolId). Observe the returned CSVHeader has no 'password_hash' element (AWS returns it as the last column).

Cause 3: replay fixture cognito-idp-csv-header-schema-matches-pool-custom-attributes, step task#3 — CreateUserPool with an explicit Schema that lists the standard attributes in a non-canonical order plus one custom attribute, then GetCSVHeader(UserPoolId). Observe the columns come back in the emulator's fixed order (name, given_name, family_name, ... , custom:<name>, cognito:mfa_enabled, cognito:username) instead of the pool's own schema order recorded from AWS (profile, address, birthdate, ... with custom:<name> in the middle), and with no trailing password_hash.
