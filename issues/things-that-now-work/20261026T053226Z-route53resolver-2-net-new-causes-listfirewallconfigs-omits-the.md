Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.server-side-firewall-config-auto-created-on-vpc-association route53resolver.tag-value-unicode-and-empty-round-trip
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.server-side-firewall-config-auto-created-on-vpc-association|run#9|AssertionError|assert any(c["Id"] == baseline["Id"] for c in listed), "default config missing from ListFirewallConfigs"
Issue-Key: route53resolver.tag-value-unicode-and-empty-round-trip|_error#11|AssertionError|raise AssertionError(f"expected a ClientError, the call succeeded and returned {result!r}")
Issue-Commit: 63ca14555026a1bc3ce90b8f8423be0f1109f448

# route53resolver: 2 net-new causes — ListFirewallConfigs omits the per-VPC default FirewallConfig that GetFirewallConfig reports, and TagResource accepts an empty-string tag Value instead of refusing it with InvalidParameterException (RSLVR-00908)

> _Note: this issue was identified against the code base at commit `63ca14555026a1bc3ce90b8f8423be0f1109f448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFirewallConfigs

## Description
Two independent net-new root causes in the Route 53 Resolver emulator, both reached after the previously filed scaffold stubs for these operations were resolved (20261025T175414Z-route53resolver-listfirewallconfigs-stub.md and 20261025T012620Z-route53resolver-tagresource-stub.md are both marked fixed; neither covers the behavior below, and in both causes the operation now answers HTTP 200 — these are fidelity defects in the implemented handlers, not refusals).

=== Cause 1 of 2: ListFirewallConfigs does not report the per-VPC default FirewallConfig that GetFirewallConfig materializes ===

AWS: DNS Firewall maintains a FirewallConfig as a per-VPC singleton the service owns; no operation creates or deletes one. Smithy: GetFirewallConfig(ResourceId: ResourceId required, len 1..64) -> output FirewallConfig{Id, ResourceId, OwnerId, FirewallFailOpen}; ListFirewallConfigs -> output FirewallConfigs: FirewallConfigList, NextToken, paginated{items=FirewallConfigs, pageSize=MaxResults range 5..10}, and its own model documentation says it "Retrieves the firewall configurations that you have defined" for "VPCs from Amazon Virtual Private Cloud". The two reads are therefore views of the same collection: every VPC for which GetFirewallConfig returns a FirewallConfig must appear in ListFirewallConfigs under the same Id, with no explicit UpdateFirewallConfig call and no prior FirewallRuleGroup association required. Recorded behavior confirms the default for a fresh VPC is fail-closed (FirewallFailOpen=DISABLED).

Emulator: for a freshly created VPC, GetFirewallConfig(ResourceId=<vpc-id>) succeeds and returns the expected default (ResourceId == the VPC id, FirewallFailOpen == "DISABLED" — those assertions pass), but ListFirewallConfigs() returns a FirewallConfigs list that contains no element whose Id equals that config's Id. The check fails at `assert any(c["Id"] == baseline["Id"] for c in listed)` with `AssertionError: default config missing from ListFirewallConfigs`. The handler enumerates `store.known_vpc_ids()` and maps each through `store.firewall_config(vpc_id)`, so a VPC the Resolver store has not separately "come to know of" is absent from the listing even though the Get path happily synthesizes its default on demand. Hypothesis: the default config is materialized lazily by GetFirewallConfig but the VPC id is never registered into the collection the list path walks, so the two reads disagree about which VPCs exist.

Why: a client that discovers DNS Firewall posture by listing (the normal way to audit or reconcile every VPC's FirewallFailOpen setting, and the only way a paginator-driven client enumerates it) sees an empty or short list and concludes the account's VPCs have no firewall configuration at all — while a direct Get on the very same VPC shows a fail-closed config that is in force. Reconciliation loops built on the list view will either skip VPCs entirely or repeatedly "create" configuration that already exists.

Fix: the ListFirewallConfigs handler and the Resolver store's VPC registry — services/service-com-amazonaws-route53resolver-route53resolver/src/ops/list_firewall_configs.rs and the `known_vpc_ids`/`firewall_config` accessors in services/service-com-amazonaws-route53resolver-route53resolver/src/state.rs must agree on the same per-VPC singleton collection, so the collection the list walks is exactly the set of VPCs for which Get returns a config. Associating a rule group with the VPC must also leave that pre-existing config in place (same Id, same FirewallFailOpen) rather than re-creating it; the check asserts that too, immediately after the failing assertion.

Traces: route53resolver.server-side-firewall-config-auto-created-on-vpc-association

=== Cause 2 of 2: TagResource accepts an empty-string tag Value with HTTP 200 instead of refusing it ===

AWS: Smithy: TagResource(ResourceArn: Arn required len 1..255; Tags: TagList required len 0..200), errors include InvalidParameterException (HTTP 400, members Message required + FieldName). Tag.Value targets TagValue with @length min 0 max 256, so the MODEL permits an empty string and the framework's constraint validation will not catch it — the rule is a service-side one. Recorded behavior: TagResource with a tag whose Value is "" is refused with HTTP 400, Error.Code `InvalidParameterException` and Error.Message `[RSLVR-00908] One of the input parameters of the tags does not meet the acceptance criteria specified in the restrictions. Please fix the invalid parameter before retrying.` That whole sentence is FIXED text for this error class, including the bracketed `[RSLVR-00908]` service error code prefix that Resolver puts in front of its messages; it interpolates no request value. (The X-Ray ` Trace Id: "..."` suffix the service appends is per-request and is NOT part of the requirement.) The rejected tag must not be persisted: a subsequent independent ListTagsForResource must not contain that key, and tags written by earlier successful calls must be undisturbed.

Emulator: the call SUCCEEDS — `{'ResponseMetadata': {'HTTPStatusCode': 200, ...'content-length': '2'...}}`, i.e. an empty JSON body and no error at all. The handler validates only ARN existence (`resource_tags_mut` -> ResourceNotFoundException) and the 200-tag count bound (`merge_tags` -> LimitExceededException); there is no acceptance-criteria check on individual Key/Value content, so the empty-valued tag is merged into the resource's stored tags. The Unicode round-trip assertions in the same check pass, so the defect is specifically the missing per-tag acceptance validation and its refusal.

Why: a client that relies on AWS refusing malformed tags gets silent acceptance, so a tag AWS would never store appears in ListTagsForResource against the emulator. Code paths that treat an empty value as "unset"/sentinel diverge between emulator and AWS, cost-allocation or policy tooling keyed on tag values sees records real AWS rejects, and negative tests asserting the 400/InvalidParameterException refusal pass in production and fail here (or are written to the emulator's looser contract and then break against AWS).

Fix: input validation for the Tags member in the TagResource handler (services/service-com-amazonaws-route53resolver-route53resolver/src/ops/tag_resource.rs), ideally in the shared tag-validation helper alongside `merge_tags`/`tag_limit_exceeded` in services/service-com-amazonaws-route53resolver-route53resolver/src/state.rs so UntagResource and the Create*/tag-on-create paths enforce the same acceptance criteria. The check must run BEFORE any mutation so the refused write persists nothing.

Traces: route53resolver.tag-value-unicode-and-empty-round-trip

## Evidence
Cause 1 — route53resolver.server-side-firewall-config-auto-created-on-vpc-association [mismatch]
  Expected (AWS): after `ec2.create_vpc(...)`, `get_firewall_config(ResourceId=<vpc-id>)` returns a default FirewallConfig (ResourceId == vpc id, FirewallFailOpen == "DISABLED"), and `list_firewall_configs()["FirewallConfigs"]` contains an entry whose `Id` equals that config's `Id`.
  Actual (emulator): GetFirewallConfig returns the expected default (those assertions pass), but the listing does not contain it:
    File "check_route53resolver_server_side_firewall_config_auto_created_on_vpc_association.py", line 33, in run
      assert any(c["Id"] == baseline["Id"] for c in listed), "default config missing from ListFirewallConfigs"
    AssertionError: default config missing from ListFirewallConfigs
  Handler: list_firewall_configs.rs:178-192 builds the page from `store.known_vpc_ids()`, which does not include a VPC whose default config GetFirewallConfig synthesizes on demand.

Cause 2 — route53resolver.tag-value-unicode-and-empty-round-trip [mismatch]
  Expected (AWS), per the check's own expectation and Resolver's recorded refusal:
    HTTP 400, Error.Code = "InvalidParameterException"
    Error.Message  = "[RSLVR-00908] One of the input parameters of the tags does not meet the acceptance criteria specified in the restrictions. Please fix the invalid parameter before retrying."
    (entire sentence incl. the `[RSLVR-00908]` prefix is fixed text; no request values are interpolated; a trailing ` Trace Id: "..."` is per-request and excluded)
    and the rejected key must be absent from a later ListTagsForResource, with previously written tags unchanged.
  Actual (emulator): HTTP 200 with an empty JSON body — no refusal at all:
    File "check_route53resolver_tag_value_unicode_and_empty_round_trip.py", line 48, in run
      refusal = _error(lambda: r53r.tag_resource(ResourceArn=resource_arn, Tags=[{"Key": "empty-value-key", "Value": ""}]))
    AssertionError: expected a ClientError, the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}, 'RetryAttempts': 0}}
  Handler: tag_resource.rs:163-180 checks only resource existence and the tag-count limit; no per-tag Key/Value acceptance validation exists.
  Model: Tag.Value -> TagValue @length{min:0,max:256} (so framework constraint validation cannot produce this refusal); TagResource models InvalidParameterException (HTTP 400).

## Steps to Reproduce
Cause 1 (ListFirewallConfigs omits the per-VPC default FirewallConfig):
  1. ec2.create_vpc(CidrBlock="10.0.0.0/16") and wait for the vpc_available waiter; note <vpc-id>.
  2. route53resolver.get_firewall_config(ResourceId="<vpc-id>") — succeeds, returns FirewallConfig with ResourceId == <vpc-id> and FirewallFailOpen == "DISABLED"; note its Id.
  3. route53resolver.list_firewall_configs() — observe that no element of FirewallConfigs has that Id (the auto-created default is missing). AWS returns it.
  4. (Second half of the same check:) create a firewall rule group, associate_firewall_rule_group(VpcId="<vpc-id>", Priority=101, MutationProtection="DISABLED"), then get_firewall_config again — the pre-existing config must still be there with the same Id and FirewallFailOpen=DISABLED.

Cause 2 (empty tag Value silently accepted):
  1. arn = route53resolver.create_firewall_domain_list(CreatorRequestId=<unique>, Name=<unique>)["FirewallDomainList"]["Arn"].
  2. route53resolver.tag_resource(ResourceArn=arn, Tags=[{"Key": "环境-key", "Value": "生产-value"}]) and verify list_tags_for_resource round-trips the Unicode value byte-for-byte (this part already passes).
  3. route53resolver.tag_resource(ResourceArn=arn, Tags=[{"Key": "empty-value-key", "Value": ""}]) — the emulator answers HTTP 200; AWS answers HTTP 400 InvalidParameterException with the [RSLVR-00908] acceptance-criteria message.
  4. route53resolver.list_tags_for_resource(ResourceArn=arn) — the rejected key must NOT be present, and the tags from step 2 must be unchanged.

Replay: run_behavioral_suite and re-run the two checks named in trace_ids.
