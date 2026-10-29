Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 145e568e7ef9b3115be9abe08039cd724717eead

# ListDiscoveredResources accepts a MemberAccountIds entry that is not an account of the organization, the one half of the member's documentation the handler does not enforce

_Filed: 2026-10-28T17:42:31Z_
_Operation: ListDiscoveredResources | Class: validation_

> _Note: this issue was identified against the code base at commit `145e568e7ef9b3115be9abe08039cd724717eead`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`MemberAccountIds`' own modelled documentation states TWO rules in one sentence pair: only one account per request, AND that the account must be a member of the organization. The handler enforces the first and silently drops the second: it destructures `input.member_account_ids` to exactly one entry, refuses anything else with `InvalidInputException`, and then hands that account straight to `crate::state::discovered_resources` without ever weighing it against the organization's accounts.

The crate already owns the answer to "is this account in the organization": `ComAmazonawsFmsAwsfms20180101State::member_accounts` and `crate::state::organization_accounts`, which the two batch siblings call to refuse a resource whose ARN names an account outside the organization (`NOT_VALID_ACCOUNT_ID`). So an account that `BatchAssociateResource` rejects as not being in the organization is accepted by `ListDiscoveredResources` as a legitimate discovery target, and the caller gets HTTP 200 with an empty `Items` instead of the modelled refusal — indistinguishable from "this member account owns nothing of that type".

## Evidence

Smithy, `smithy_query(query="doc", shape="com.amazonaws.fms#ListDiscoveredResourcesRequest", member="MemberAccountIds")`:

  "The Amazon Web Services account IDs to discover resources in. Only one account is supported per request. The account must be a member of your organization."

The handler quotes the middle sentence of that same doc and enforces only it — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/list_discovered_resources.rs:201-214:

```
        // "Only one account is supported per request" — `MemberAccountIds`' own
        // documentation. The modelled list carries no length constraint, so the
        // boundary passes any number of accounts and the service decides.
        let [account] = input.member_account_ids.as_slice() else {
            return Err(ListDiscoveredResourcesErrorModeled::InvalidInputException(
```

...and the very next use of `account` is the inventory read, with no membership check in between — list_discovered_resources.rs:239-244:

```
        let inventory = crate::state::discovered_resources(
            &*self.engine,
            &region,
            account,
            &input.resource_type,
        );
```

The sibling that DOES apply the organization-membership rule, services/service-com-amazonaws-fms-awsfms-20180101/src/ops/batch_associate_resource.rs:282:

```
        let accounts = crate::state::organization_accounts(&store, discoverable);
```

and services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:2779-2781:

```
    if !arn_in_accounts(&arn, accounts) {
        return Some("NOT_VALID_ACCOUNT_ID");
    }
```

whose helper doc (state.rs:2812-2815) names the shared definition: "The accounts of this organization a resource may belong to: the ones [`...::member_accounts`] reports — the SAME definition `ListMemberAccounts` answers with, so two operations cannot disagree about who is in the organization".

Test coverage of the CURRENT behaviour: services/service-com-amazonaws-fms-awsfms-20180101/tests/all/discovered_resources.rs:938-983 pins a non-member account to a 200 with an empty page —

```
/// RULE: nothing is discovered for an account that owns nothing, or for a
/// resource type no emulated service holds resources of — an empty `Items` and
/// no `NextToken`, never a refusal and never an invented resource.
...
    let cases = [
        (FOREIGN_ACCOUNT, state::RESOURCE_TYPE_EC2_VPC),
```

with `FOREIGN_ACCOUNT` declared at discovered_resources.rs:61-62 as "An account id that is not this emulator's, so nothing it holds belongs to it." That stated reason conflates "an account outside the organization" with "an account that owns nothing"; the modelled sentence above distinguishes them.

## Steps to Reproduce

1. Start the emulator with Firewall Manager and Amazon EC2 mounted and call `AssociateAdminAccount` so the administrator gate passes.
2. Call `ListDiscoveredResources` with `{"MemberAccountIds": ["222233334444"], "ResourceType": "AWS::EC2::VPC"}` — an account id the organization's member set (`ListMemberAccounts`) does not report.
3. Observe HTTP 200 with `Items: []`: the account outside the organization is accepted as a discovery target, although `MemberAccountIds`' documentation says "The account must be a member of your organization."
4. Contrast with `BatchAssociateResource`, which refuses a resource ARN in account 222233334444 as `NOT_VALID_ACCOUNT_ID` by checking the same organization-account set through `crate::state::organization_accounts`.
