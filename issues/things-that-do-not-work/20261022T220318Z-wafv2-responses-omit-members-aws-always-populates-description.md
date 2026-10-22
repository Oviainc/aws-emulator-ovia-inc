Issue-Source: trace-sweep-divergence
Issue-Trace: wafv2-web-acl-lifecycle-and-list
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wafv2-web-acl-lifecycle-and-list wafv2-webacl-description-empty-vs-absent-preserved wafv2-webacl-logging-configuration-setup wafv2-webacl-resource-association-workflow wafv2-ipset-lifecycle-and-errors wafv2-rate-based-rule-analytics-and-sampling wafv2-traffic-path-and-settlement-analytics wafv2-wafv2-error-paths wafv2-webacl-lifecycle-and-list wafv2-webacl-rules-priority-ordering-preserved
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2|GetWebACL|2xx->2xx|$.WebACL.OnSourceDDoSProtectionConfig
Issue-Key: wafv2|GetWebACL|2xx->2xx|$.WebACL.Rules
Issue-Key: wafv2|CreateIPSet|2xx->2xx|$.Summary.Description
Issue-Key: wafv2|CreateWebACL|2xx->2xx|$.Summary.Description
Issue-Key: wafv2|GetWebACL|2xx->2xx|$.WebACL.Description
Issue-Key: wafv2|GetWebACL|2xx->2xx|$.WebACL.Rules[*].Statement.RateBasedStatement.EvaluationWindowSec
Issue-Key: wafv2|GetWebACL|2xx->2xx|$.WebACL.Capacity
Issue-Key: wafv2|ListWebACLs|2xx->2xx|$.NextMarker
Issue-Key: wafv2|ListWebACLs|2xx->2xx|$.WebACLs[Name="sim2real-probe-wafv2-wafv2-webacl-de5a48c2"].Description
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# WAFv2 responses omit members AWS always populates (Description, Rules, defaults, NextMarker)

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWebACL

## Description
This bundle has 9 causes. They share one emulator defect: the wafv2 response serializers emit only members the emulator happens to hold a non-empty value for, and the WebACL state never computes the members AWS derives server-side. AWS populates these members on every successful read, including when they are empty or defaulted, so the emulator's responses are structurally thinner than the wire contract and clients that read them unconditionally break.

Cause 1 of 9: GetWebACL omits WebACL.Rules and WebACL.OnSourceDDoSProtectionConfig. AWS: GetWebACL returns the WebACL shape with Rules present as an empty list [] when the web ACL has no rules, and OnSourceDDoSProtectionConfig always present -- Smithy documents ACTIVE_UNDER_DDOS as enabled by default for ALB-associated web ACLs (OnSourceDDoSProtectionConfig.ALBLowReputationMode, required, enum LowReputationMode), and the recordings show {'ALBLowReputationMode': 'ACTIVE_UNDER_DDOS'} returned for web ACLs created without that input member. Emulator: both members absent (None). Why: a client iterating response.webACL.rules gets null instead of an empty collection, and any client reading the DDoS-protection mode cannot see the account default. Fix: the GetWebACL serializer plus WebACL state construction (default OnSourceDDoSProtectionConfig at create, always serialize Rules). Traces: wafv2-web-acl-lifecycle-and-list#task#2 (also wafv2-webacl-description-empty-vs-absent-preserved#task#4, wafv2-webacl-logging-configuration-setup#teardown#6, wafv2-webacl-resource-association-workflow#teardown#6).

Cause 2 of 9: CreateIPSet response drops Summary.Description. AWS: CreateIPSet returns Summary with Description present as the empty string when no description was supplied (recorded behavior only -- Smithy types EntityDescription with length min 1, but the recording shows '' on the wire). Emulator: $.Summary.Description absent. Why: clients that read the summary's description get null where AWS gives a string, and round-trip equality checks against a recorded create response fail. Fix: the CreateIPSet response serializer / IPSet summary projection. Traces: wafv2-ipset-lifecycle-and-errors#setup#1 (also wafv2-ipset-lifecycle-and-errors#setup#2).

Cause 3 of 9: CreateWebACL response drops Summary.Description. AWS: as cause 2, CreateWebACL's Summary carries Description '' when none was supplied (recorded behavior only). Emulator: absent. Why/Fix: as cause 2, in the CreateWebACL response serializer / WebACLSummary projection. Traces: wafv2-rate-based-rule-analytics-and-sampling#setup#1 (also wafv2-traffic-path-and-settlement-analytics#setup#1, wafv2-wafv2-error-paths#task#1, wafv2-webacl-lifecycle-and-list#setup#1).

Cause 4 of 9: GetWebACL omits Description and OnSourceDDoSProtectionConfig and returns RateBasedStatement.EvaluationWindowSec as 0. AWS: Description '' as in cause 2; OnSourceDDoSProtectionConfig as in cause 1; and for a rate-based rule created without EvaluationWindowSec, WAF stores and returns the documented default -- Smithy RateBasedStatement.EvaluationWindowSec: "Valid settings are 60, 120, 300, and 600 ... Default: 300 (5 minutes)" (the modeled zero-value default of 0 is not a valid setting). Emulator: Description/OnSourceDDoSProtectionConfig absent and EvaluationWindowSec 0. Why: a client reading back its rate-based rule sees an evaluation window of 0, which is outside the documented valid set, so rule round-tripping (get-modify-update) writes an invalid value back. Fix: WebACL rule state at create/update -- apply the 300 default to RateBasedStatement.EvaluationWindowSec when absent. Traces: wafv2-rate-based-rule-analytics-and-sampling#task#3.

Cause 5 of 9: GetWebACL returns WebACL.Capacity as the rule count instead of consumed WCUs. AWS: Capacity is the web ACL capacity units currently consumed (Smithy WebACL.Capacity -> ConsumedCapacity, long, range min 0, "The web ACL capacity units (WCUs) currently being used by this web ACL"); the recordings show 32 for the rate-based web ACL. Emulator: 2 -- i.e. the number of rules, not their WCU cost (same cause as the 30-vs-3 case below). Why: clients that budget against the 1500-WCU web ACL limit, or assert the capacity of a rule set, read a number that is unrelated to WCUs. Fix: WebACL state -- compute Capacity from the rule statements' WCU costs rather than len(Rules). Traces: wafv2-rate-based-rule-analytics-and-sampling#task#7 (also wafv2-rate-based-rule-analytics-and-sampling#teardown#9).

Cause 6 of 9: ListWebACLs omits NextMarker. AWS: ListWebACLs always returns NextMarker (Smithy ListWebACLsResponse.NextMarker -> NextMarker, length 1..256); the recordings show it set to the last returned web ACL's name even when the page is not full. Emulator: $.NextMarker absent. Why: a paginating client cannot continue -- the SDK paginator treats a missing marker as end-of-results, so emulator-backed pagination silently truncates and never exercises the client's loop. Fix: the ListWebACLs handler's pagination -- always emit NextMarker. Traces: wafv2-web-acl-lifecycle-and-list#task#3.

Cause 7 of 9: GetWebACL omits Description '' and OnSourceDDoSProtectionConfig and Rules [] together on a web ACL created with an explicitly empty description. AWS: all three present as in causes 1-3. Emulator: all three absent. Why: this is the empty-vs-absent distinction the fixture exists to pin -- a client cannot tell "no description" from "description cleared". Fix: as causes 1-3, in the GetWebACL serializer. Traces: wafv2-webacl-description-empty-vs-absent-preserved#task#2 (also the same fixture's task#6).

Cause 8 of 9: ListWebACLs omits NextMarker and the per-summary Description. AWS: NextMarker as cause 6, and each WebACLSummary carries Description '' as cause 3. Emulator: both absent. Why/Fix: as causes 3 and 6, in the ListWebACLs handler and its summary projection. Traces: wafv2-webacl-lifecycle-and-list#task#2.

Cause 9 of 9: GetWebACL returns Capacity 30 as 3 and omits Description/OnSourceDDoSProtectionConfig on a multi-rule web ACL. AWS: Capacity is consumed WCUs (recorded 30 for three rules); Description ''/OnSourceDDoSProtectionConfig present. Emulator: Capacity 3, both members absent. Why/Fix: same as causes 1, 3 and 5 -- confirms Capacity tracks rule count, not WCUs, independently of the rate-based case. Traces: wafv2-webacl-rules-priority-ordering-preserved#task#2.

Severity is HIGH because causes 5, 6 and 9 change values and pagination behavior a client acts on, not only presence.

## Evidence
Replay of simparity-wire-aws-v1, 9 divergence clusters, all status-matched 200 -> 200 (missing/mis-valued response members):

1. GetWebACL (wafv2-web-acl-lifecycle-and-list#task#2, 4 affected): $.WebACL.OnSourceDDoSProtectionConfig missing (expected {'ALBLowReputationMode': 'ACTIVE_UNDER_DDOS'}); $.WebACL.Rules missing (expected []).
2. CreateIPSet (wafv2-ipset-lifecycle-and-errors#setup#1, 2 affected): $.Summary.Description missing (expected '').
3. CreateWebACL (wafv2-rate-based-rule-analytics-and-sampling#setup#1, 4 affected): $.Summary.Description missing (expected '').
4. GetWebACL (wafv2-rate-based-rule-analytics-and-sampling#task#3): $.WebACL.Description missing (expected ''); $.WebACL.OnSourceDDoSProtectionConfig missing; $.WebACL.Rules[0].Statement.RateBasedStatement.EvaluationWindowSec expected 300, got 0.
5. GetWebACL (wafv2-rate-based-rule-analytics-and-sampling#task#7, 2 affected): $.WebACL.Capacity expected 32, got 2; plus Description/OnSourceDDoSProtectionConfig missing and EvaluationWindowSec 300 -> 0.
6. ListWebACLs (wafv2-web-acl-lifecycle-and-list#task#3): $.NextMarker missing (expected 'sim2real-probe-wafv2-wafv2-webacl-a6fcc9a5').
7. GetWebACL (wafv2-webacl-description-empty-vs-absent-preserved#task#2, 2 affected): $.WebACL.Description missing (expected ''); OnSourceDDoSProtectionConfig missing; Rules missing (expected []).
8. ListWebACLs (wafv2-webacl-lifecycle-and-list#task#2): $.NextMarker missing; $.WebACLs[Name="...de5a48c2"].Description missing (expected '').
9. GetWebACL (wafv2-webacl-rules-priority-ordering-preserved#task#2): $.WebACL.Capacity expected 30, got 3; Description missing; OnSourceDDoSProtectionConfig missing.

Model grounding: Smithy WebACL.Capacity -> ConsumedCapacity (long, range min 0, "web ACL capacity units (WCUs) currently being used"); OnSourceDDoSProtectionConfig.ALBLowReputationMode required, doc: ACTIVE_UNDER_DDOS "is enabled by default whenever a web ACL is associated with an Application Load Balancer"; RateBasedStatement.EvaluationWindowSec doc: "Valid settings are 60, 120, 300, and 600 ... Default: 300"; ListWebACLsResponse.NextMarker length 1..256. The empty-string Description on the wire is recorded behavior only.

## Steps to Reproduce
1. Replay wafv2-web-acl-lifecycle-and-list#task#2 (GetWebACL on a rule-less web ACL): WebACL.Rules and WebACL.OnSourceDDoSProtectionConfig are absent instead of [] and {'ALBLowReputationMode': 'ACTIVE_UNDER_DDOS'}.
2. Replay wafv2-ipset-lifecycle-and-errors#setup#1 and wafv2-rate-based-rule-analytics-and-sampling#setup#1 (CreateIPSet / CreateWebACL with no Description): Summary.Description is absent instead of ''.
3. Replay wafv2-rate-based-rule-analytics-and-sampling#task#3 and #task#7 (GetWebACL on a rate-based web ACL): EvaluationWindowSec is 0 instead of 300 and Capacity is 2 instead of 32.
4. Replay wafv2-webacl-rules-priority-ordering-preserved#task#2 (GetWebACL, three rules): Capacity is 3 instead of 30.
5. Replay wafv2-web-acl-lifecycle-and-list#task#3 and wafv2-webacl-lifecycle-and-list#task#2 (ListWebACLs): NextMarker is absent, and the summary Description is absent instead of ''.
6. Replay wafv2-webacl-description-empty-vs-absent-preserved#task#2 (GetWebACL after creating with an empty description): Description, Rules and OnSourceDDoSProtectionConfig are all absent.
