Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6fc58cc31848e5a6d84ee6ef6d43eb64c5590e82

# An SLO `Id` in ARN form resolves whatever ACCOUNT it names, while the tagging plane refuses the same ARN — the account half of the crate's one account rule is skipped on every Id-taking operation

_Filed: 2026-10-28T19:31:34Z_
_Operation: GetServiceLevelObjective | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6fc58cc31848e5a6d84ee6ef6d43eb64c5590e82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` declares ONE place where this service decides whether a request names another account (`names_other_account`), and states that every reader that takes an account id goes through it — explicitly including "the account field of a `ResourceArn`". The ARN form of `ServiceLevelObjectiveId` also carries an account field, and the modelled pattern leaves it completely free (`[^:]*`), but `resolve`/`resolve_mut`/`remove` reach the record through `slo_name_of` -> `slo_name_in_arn`, which only pulls the resource field out of the ARN and never compares the account.

So one and the same string is judged two different ways inside this crate:

* `GET /slo/arn:aws:application-signals:us-east-1:999999999999:slo/<name>` answers HTTP 200 with THIS account's SLO (`get_service_level_objective.rs:154`), and `PATCH`/`DELETE` on the same ARN update and DELETE this account's record (`update_service_level_objective.rs:193`, `delete_service_level_objective.rs:146`). `BatchUpdateExclusionWindows`, `ListServiceLevelObjectiveExclusionWindows` and `BatchGetServiceLevelObjectiveBudgetReport` resolve it the same way.
* `GET /tags?ResourceArn=<that very ARN>` answers HTTP 404, because `tagged_slo_name_in_arn` does run the account field through `names_other_account`.
* `ListServiceLevelObjectives?SloOwnerAwsAccountId=999999999999` answers an empty selection, which `tests/all/account_scoped_reads.rs` pins as "the ONE account rule every account-scoped read of this service holds".

The stated justification for skipping the check on an `Id` is false against the model: `tagged_slo_name_in_arn`'s doc says "A `ServiceLevelObjectiveId` needs neither check: its modelled pattern already fixes the namespace before a handler runs" — the pattern does fix the vendor/namespace, but its ACCOUNT field is `[^:]*`, so nothing before the handler constrains the account at all. The region field is a documented, deliberate exception ("the region in a minted ARN is a rendering of the writer's SigV4 scope rather than part of an SLO's identity"); no such statement exists for the account, which the crate elsewhere treats as part of a resource's identity. The destructive consequence is that a caller can delete or rewrite this account's SLO through an ARN that names a different account.

## Evidence

THE CRATE'S OWN RULE — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:562-576:
```
/// Whether an account id a request supplies names an account OTHER than the one this store
/// holds — the single place every account-scoped read of this service decides that.
...
/// Every reader that takes an account id — a list's own `AwsAccountId`, an SLO list's
/// `SloOwnerAwsAccountId`, the account an audited entity names, and the account field of a
/// `ResourceArn` (always present, so `Some`) — reads it through here, so the reads that must
/// agree about which account a request names cannot drift apart.
pub fn names_other_account(account: Option<&str>) -> bool {
    account.is_some_and(|account| account != ::engine::emulator_account_id())
}
```

SIDE THAT APPLIES IT, AND THE CLAIM THAT EXCUSES THE OTHER SIDE — state.rs:606-624:
```
/// Stricter than [`slo_name_in_arn`], because `ResourceArn` is a bare `AmazonResourceName`:
...
/// — which is what `service_common::arn::parse_of` narrows a parse to — and the account
/// field must be the account this store holds ([`names_other_account`]). A
/// `ServiceLevelObjectiveId` needs neither check: its modelled pattern already fixes the
/// namespace before a handler runs.
pub fn tagged_slo_name_in_arn(resource_arn: &str) -> Option<&str> {
    let arn = ::service_common::arn::parse_of(resource_arn, SIGNING_NAME)?;
    if names_other_account(Some(arn.account)) {
        return None;
    }
    slo_name_in_arn(resource_arn)
}
```

WHAT THE MODEL ACTUALLY FIXES — `smithy_query(query="shape", shape="ServiceLevelObjectiveId")`:
```
"pattern": "^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$|^arn:(aws|aws-us-gov):application-signals:[^:]*:[^:]*:slo/[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$"
```
The fifth field (the account) is `[^:]*` — the pattern fixes the namespace, NOT the account, so the quoted justification does not cover the account check it waives.

SIDE THAT SKIPS IT — state.rs:279-283 and 583-601:
```
    /// The stored SLO an `Id` names, accepting either form the modelled
    /// `ServiceLevelObjectiveId` pattern allows: a bare name or a full `slo/<name>` ARN.
    pub fn resolve(&self, id: &str) -> Option<&ServiceLevelObjective> {
        self.slos.get(slo_name_of(id))
    }
...
pub fn slo_name_of(id: &str) -> &str {
    slo_name_in_arn(id).unwrap_or(id)
}
...
pub fn slo_name_in_arn(arn: &str) -> Option<&str> {
    ::service_common::arn::resource_id_of_type(arn, SLO_ARN_RESOURCE)
}
```
(no account comparison anywhere on this path)

HANDLERS ON THAT PATH — src/ops/get_service_level_objective.rs:153-160:
```
        let store = state.lock();
        match store.resolve(&input.id) {
            Some(slo) => Ok(GetServiceLevelObjectiveOutput { slo: slo.clone() }),
```
and the destructive one, src/ops/delete_service_level_objective.rs:145-153:
```
        let mut store = state.lock();
        if store.remove(&input.id).is_none() {
```

THE REGION EXCEPTION, WHICH IS STATED AND THE ACCOUNT ONE IS NOT — state.rs:616-618:
```
/// The REGION field is not compared, for the reason the store's own doc gives: this process
/// holds one account's and one Region's records, and the region in a minted ARN is a
/// rendering of the writer's SigV4 scope rather than part of an SLO's identity.
```

TEST COVERAGE: tests/all/account_scoped_reads.rs:3-12 states the rule as universal — "End-to-end coverage of the ONE account rule every account-scoped read of this service holds: an account id a request supplies selects this store's records when it names the account this store holds, and nothing when it names any other account" — but drives only `ListServices`, `ListServiceStates`, `ListServiceLevelObjectives`, `ListGroupingAttributeDefinitions` and `ListAuditFindings`. No test addresses an SLO by an `Id` in ARN form carrying a foreign account, so nothing pins the current behaviour; tests/all/resource_tags.rs pins the tagging plane's refusal of a foreign-account ARN.

## Steps to Reproduce

1. Start the emulator with this crate mounted and sign requests for this emulator's own account (engine::emulator_account_id()).
2. POST /slo with {"Name":"cross-account-slo","SliConfig":{"SliMetricConfig":{"KeyAttributes":{"Type":"Service","Name":"checkout","Environment":"prod"}},"MetricThreshold":100},"CreateRecommendedSlo":false} and note the returned Slo.Arn, which carries this emulator's account id.
3. GET /slo/arn%3Aaws%3Aapplication-signals%3Aus-east-1%3A999999999999%3Aslo%2Fcross-account-slo — the same ARN with only the ACCOUNT segment changed to 999999999999. Observe HTTP 200 returning this account's SLO.
4. GET /tags?ResourceArn=arn:aws:application-signals:us-east-1:999999999999:slo/cross-account-slo — the identical ARN through the tagging plane. Observe HTTP 404 ResourceNotFoundException, because tagged_slo_name_in_arn runs the account field through names_other_account.
5. GET /slos?SloOwnerAwsAccountId=999999999999 — observe an empty SloSummaries, the behaviour tests/all/account_scoped_reads.rs pins for every account-scoped read.
6. DELETE /slo/arn%3Aaws%3Aapplication-signals%3Aus-east-1%3A999999999999%3Aslo%2Fcross-account-slo — observe HTTP 200 and that GET /slo/cross-account-slo now answers 404: a request naming another account's resource deleted this account's record.
