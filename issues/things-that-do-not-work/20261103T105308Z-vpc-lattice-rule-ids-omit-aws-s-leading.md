Issue-Source: trace-sweep-divergence
Issue-Trace: vpclattice-listener-rule-lifecycle
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: vpclattice-listener-rule-lifecycle
Issue-Shape-Id: com.amazonaws.vpclattice#MercuryControlPlane
Issue-Key: vpc-lattice|CreateRule|2xx->2xx|$.arn
Issue-Key: vpc-lattice|ListRules|2xx->2xx|$.items[*].arn
Issue-Key: vpc-lattice|GetRule|2xx->2xx|$.arn
Issue-Key: vpc-lattice|UpdateRule|2xx->2xx|$.arn
Issue-Commit: 98c28633a50c37c08c8640cea9cea1eb68cf9263

# vpc-lattice rule ids omit AWS's leading-zero hex suffix form, so every rule id and rule ARN diverges

> _Note: this issue was identified against the code base at commit `98c28633a50c37c08c8640cea9cea1eb68cf9263`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRule

## Description
BUNDLE OF 4 CAUSES, one root fault, all four must be closed before submitting.

Shared fault: a VPC Lattice rule id is `rule-` plus a 17-character suffix, and every rule id AWS produced in this corpus has a suffix of the form `0` followed by 16 lowercase-hex digits (a zero-padded 16-nibble value, the same shape the EC2-family ids use). The emulator draws all 17 suffix characters uniformly from the hex alphabet, so the first character is almost never `0` and the minted id is not of the form AWS emits. The wrong form then propagates into every value that embeds the rule id: the rule's own `id`, its `arn` (which nests under the service and listener ARNs), and the same members inside ListRules summaries. One minting site produces all four clusters below: the per-listener DEFAULT rule (minted when the listener is created) and explicitly created rules both come from the same minter.

Shared AWS reference: recorded behavior only. Smithy constrains only the looser outer form — RuleId @length 5..22 @pattern ^rule-[0-9a-z]{17}$; RuleArn @length 20..2048 @pattern ^arn:[a-z0-9\-]+:vpc-lattice:[a-zA-Z0-9\-]+:\d{12}:service/svc-[0-9a-z]{17}/listener/listener-[0-9a-z]{17}/rule/rule-[0-9a-z]{17}$ — so a 17-character all-hex suffix already satisfies the model, which is why the model cannot catch this and only the recordings show it. The recordings show it consistently: the default rule AWS returned is `rule-097e03279b222843a` and the created rule is `rule-05dfcacbf071a1a5c`; the same corpus records `tg-061d6752b311751ac` and `listener-0c26c95e4c2cda1d0`. Every one of those 17-character suffixes begins with `0` and uses only [0-9a-f]. Hypothesis: AWS renders a 64-bit random value as 16 hex digits and prefixes the fixed `0`, giving a 17-character suffix whose first character is always `0`. The requirement here is the TEMPLATE (`rule-` + `0` + 16 chars from [0-9a-f]), never any single recorded id string: the particular ids above are one run's values.

Shared Fix: the id minter in the service's state module — `mint_id(prefix)` builds `prefix + encode_in(fresh_entropy(), ID_SUFFIX_LEN = 17, id_alphabet(prefix))`, and `id_alphabet` already returns the lowercase hex alphabet for PREFIX_RULE; what is missing is the fixed leading `0`, i.e. the suffix must be `0` plus 16 hex characters rather than 17 uniformly drawn hex characters. `rule_arn`, `insert_default_rule` and the Create/Get/Update/List serializers need no change once minting is fixed; they are named per cause only because that is where the wrong form becomes visible on the wire. HTTP statuses, error codes and error messages agree in all four clusters — the divergence is confined to the generated identifier and the values that embed it.

A NOTE FOR THE FIXER on how these render: for the GetRule and UpdateRule clusters the harness has aliased the recorded rule id to the emulator's id for DISPLAY, so the detail prints the expected and actual ARN strings identically while the comparison still fails on the embedded identifier. Do not read those identical-looking strings as a no-op: nothing else in those ARNs differs (same partition, region us-west-2 and account 000000000000 on both sides), and they clear only once rule ids are minted in AWS's form. The service-id and listener-id segments of these ARNs are ALSO wrong (base36 rather than hex) but that is a different fault, owned by an existing issue — see the relation note below.

RELATION TO EXISTING OPEN ISSUES: 20261030T175927Z-vpc-lattice-mints-rule-and-target-group-ids.md reported that rule and target-group suffixes were drawn from base36 instead of hex; the emulator now mints rule suffixes from [0-9a-f], so that part is no longer what fails here. 20261030T043524Z-vpc-lattice-minted-resource-ids-use-a-base36.md owns the svc-/listener-/snsa- base36 fault that explains the other segments of the ARNs quoted below. NEITHER issue carries the replay keys of the four clusters below (replay reports them with an empty owned_by), so those clusters are not exercised by either pointer and are filed here.

Cause 1 of 4 — ListRules, fixture vpclattice-listener-rule-lifecycle step task#5 (HTTP 200 in both).
AWS: lists the listener's rules, each summary carrying `arn`/`id` of the auto-created DEFAULT rule, whose id matches ^rule-0<16 hex>$ (Smithy: ListRules GET /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules, 200, output items -> RuleSummaryList). The recording's default rule is rule-097e03279b222843a.
Emulator: answers 200 with one item, but `$.items[0].arn` embeds a rule id minted as 17 uniform hex characters (observed suffix beginning with a non-zero digit), so it diverges from AWS's form.
Why: a client enumerating a listener's rules and validating or pattern-matching the ids it gets back — or matching them against ids AWS produced — rejects a value AWS would never emit, and it does so for the default rule a listener always has.
Fix: the state module's id minter (the leading `0` of the 17-character suffix), consumed for the default rule via `insert_default_rule` -> `mint_id(PREFIX_RULE)` and `rule_arn`; the list serializer itself is correct.
Traces: vpclattice-listener-rule-lifecycle#task#5.

Cause 2 of 4 — CreateRule, fixture vpclattice-listener-rule-lifecycle step task#6 (HTTP 201 in both).
AWS: returns `id` matching ^rule-0<16 hex>$ and `arn` arn:<partition>:vpc-lattice:<region>:<account>:service/svc-<17>/listener/listener-<17>/rule/rule-0<16 hex> (Smithy: CreateRule POST /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules, success 201). The recording shows rule-05dfcacbf071a1a5c.
Emulator: answers 201 but `$.arn` carries a rule id whose 17-character suffix is uniformly drawn hex and does not start with `0`.
Why: this is the handle the client stores and then passes to GetRule/UpdateRule/DeleteRule and BatchUpdateRule; a handle of a form AWS never produces breaks client-side id validation and any comparison against AWS-produced values.
Fix: same minting change, consumed by CreateRule's handler via `mint_id(PREFIX_RULE)` and `rule_arn`.
Traces: vpclattice-listener-rule-lifecycle#task#6.

Cause 3 of 4 — GetRule, fixture vpclattice-listener-rule-lifecycle step task#7 (HTTP 200 in both; the cluster also covers task#12, the second read of the same rule).
AWS: echoes the stored rule's `arn` exactly as CreateRule returned it, with the leading-zero hex id suffix (Smithy: GetRule GET /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules/{ruleIdentifier}, 200).
Emulator: `$.arn` diverges — the stored ARN embeds the id minted in cause 2 (this is one of the clusters whose expected and actual strings print identically because of the harness's display aliasing).
Why: the read path hands back an identifier a caller cannot match against anything AWS produced, so id validation and recorded comparisons fail on read as well as on create.
Fix: same minting change; GetRule's serializer needs no separate change.
Traces: vpclattice-listener-rule-lifecycle#task#7 (and task#12 in the same cluster).

Cause 4 of 4 — UpdateRule, fixture vpclattice-listener-rule-lifecycle step task#11 (HTTP 200 in both).
AWS: the update response echoes the rule's `arn` with the leading-zero hex suffix (Smithy: UpdateRule PATCH /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules/{ruleIdentifier}, 200, output arn/id/name/isDefault/match/priority/action).
Emulator: `$.arn` diverges on the embedded rule id (again printed identically to the expectation because of display aliasing).
Why: a caller correlating the update acknowledgement with the ARN it holds cannot match it against AWS-shaped values.
Fix: same minting change.
Traces: vpclattice-listener-rule-lifecycle#task#11.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up true; 110 steps, 63 passed, 47 mismatched, 0 errored, 0 blocked; score 57.3%; 17 divergence clusters). Four net-new vpc-lattice mismatch clusters, each with stub_path empty and owned_by empty:

1. ListRules vpclattice-listener-rule-lifecycle#task#5 — expected 200, actual 200. Detail verbatim:
   $.items[0].arn: value_mismatch (expected 'arn:aws:vpc-lattice:us-west-2:000000000000:service/svc-a62vj29cp8odmid3f/listener/listener-23w5jjpn2aharp82y/rule/rule-097e03279b222843a', got 'arn:aws:vpc-lattice:us-west-2:000000000000:service/svc-a62vj29cp8odmid3f/listener/listener-23w5jjpn2aharp82y/rule/rule-51ee495eb6241c277')
2. CreateRule vpclattice-listener-rule-lifecycle#task#6 — expected 201, actual 201. Detail verbatim:
   $.arn: value_mismatch (expected 'arn:aws:vpc-lattice:us-west-2:000000000000:service/svc-a62vj29cp8odmid3f/listener/listener-23w5jjpn2aharp82y/rule/rule-05dfcacbf071a1a5c', got 'arn:aws:vpc-lattice:us-west-2:000000000000:service/svc-a62vj29cp8odmid3f/listener/listener-23w5jjpn2aharp82y/rule/rule-7057f460a64946982')
3. GetRule vpclattice-listener-rule-lifecycle#task#7 — expected 200, actual 200; cluster affected_count 2, also covering vpclattice-listener-rule-lifecycle#task#12. Detail verbatim (the harness has aliased the recorded id to the emulator's for display, so the two strings print alike while the comparison still fails):
   $.arn: value_mismatch (expected 'arn:aws:vpc-lattice:us-west-2:000000000000:service/svc-a62vj29cp8odmid3f/listener/listener-23w5jjpn2aharp82y/rule/rule-7057f460a64946982', got 'arn:aws:vpc-lattice:us-west-2:000000000000:service/svc-a62vj29cp8odmid3f/listener/listener-23w5jjpn2aharp82y/rule/rule-7057f460a64946982')
4. UpdateRule vpclattice-listener-rule-lifecycle#task#11 — expected 200, actual 200. Detail verbatim (same display aliasing):
   $.arn: value_mismatch (expected 'arn:aws:vpc-lattice:us-west-2:000000000000:service/svc-a62vj29cp8odmid3f/listener/listener-23w5jjpn2aharp82y/rule/rule-7057f460a64946982', got 'arn:aws:vpc-lattice:us-west-2:000000000000:service/svc-a62vj29cp8odmid3f/listener/listener-23w5jjpn2aharp82y/rule/rule-7057f460a64946982')

Expected vs actual, stated generally: AWS mints a rule id as `rule-` + `0` + 16 characters from [0-9a-f] (recorded: rule-097e03279b222843a for the default rule, rule-05dfcacbf071a1a5c for the created rule; the same corpus records tg-061d6752b311751ac and listener-0c26c95e4c2cda1d0, all with a leading `0` in the 17-character suffix). The emulator mints `rule-` + 17 characters drawn uniformly from [0-9a-f] (observed: rule-51ee495eb6241c277, rule-7057f460a64946982 — hex, but with a non-zero leading character), so roughly fifteen minted ids in sixteen are not of AWS's form. HTTP status (200/201), error codes and error messages agree in all four clusters; the divergence is confined to the generated rule identifier and the ARNs that embed it. The region (us-west-2), partition and account id (000000000000) are identical on both sides and are not part of the finding. The specific id strings are one run's values, not the requirement — the template is.

Model facts used: RuleId @length 5..22 @pattern ^rule-[0-9a-z]{17}$; RuleArn @length 20..2048 @pattern ^arn:[a-z0-9\-]+:vpc-lattice:[a-zA-Z0-9\-]+:\d{12}:service/svc-[0-9a-z]{17}/listener/listener-[0-9a-z]{17}/rule/rule-[0-9a-z]{17}$; CreateRule http {POST /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules, 201}; GetRule http {GET .../rules/{ruleIdentifier}, 200}; UpdateRule http {PATCH .../rules/{ruleIdentifier}, 200}; ListRules http {GET .../rules, 200}, paginated on items. A suffix of `0` + 16 hex characters keeps every modeled pattern and length satisfied (still 17 characters from [0-9a-z]), so no other member's validation changes — the leading-zero form is RECORDED BEHAVIOR ONLY, not modeled.

Code: services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/state.rs:1510-1529 — `mint_id` is `prefix + encode_in(fresh_entropy(), ID_SUFFIX_LEN, id_alphabet(prefix))` with ID_SUFFIX_LEN = 17 (state.rs:62) and `id_alphabet` already mapping PREFIX_RULE | PREFIX_TARGET_GROUP to HEX_LOWER (state.rs:372, 1524-1529); nothing forces the leading `0`. The default rule is minted through the same call at state.rs:1215 (`insert_default_rule`), and the rule ARN is composed at state.rs:1500-1506 (`rule_arn`).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus), or drive fixture vpclattice-listener-rule-lifecycle by hand against the vpc-lattice endpoint:
   a. POST /services and POST /services/{serviceIdentifier}/listeners to create a service and a listener.
   b. GET /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules (task#5) and inspect items[0].arn: the auto-created default rule's 17-character id suffix is hex but does not begin with `0`, where the recording has rule-097e03279b222843a.
   c. POST /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules with {"name":"<rule name>","priority":<n>,"match":{...},"action":{...}} (task#6) and inspect `id`/`arn`: same non-leading-zero suffix, where the recording has rule-05dfcacbf071a1a5c.
   d. GET .../rules/{ruleIdentifier} (task#7, and again at task#12) and PATCH .../rules/{ruleIdentifier} (task#11): the echoed `arn` carries the same id. Note that the replay report prints expected and actual alike for these two clusters because of display aliasing, so read the id form out of the response itself rather than from the diff.
   e. Repeat the create a few times: with 17 uniformly drawn hex characters only about one id in sixteen happens to begin with `0`, which is the form every recorded id has.
3. Confirm the fix by re-replaying the representative step vpclattice-listener-rule-lifecycle#task#6 and every step listed in trace_ids (task#5, task#7 — whose cluster also covers task#12 — and task#11); all four clusters must come back clean, and for the GetRule/UpdateRule clusters only a clean verdict proves the fix since the report renders expected and actual identically.
