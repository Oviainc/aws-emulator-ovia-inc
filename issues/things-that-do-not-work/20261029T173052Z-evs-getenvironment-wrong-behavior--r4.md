Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# A SELF_DEPLOYED environment's licenseInfo is counted as a license-key carrier, so the member the crate says "will have no effect" flips another environment's KEY_REUSE check to FAILED

_Filed: 2026-10-29T17:30:52Z_
_Operation: GetEnvironment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateEnvironment` accepts and stores `licenseInfo` for a `SELF_DEPLOYED` environment, and its handler doc states the reason that is sound: in that mode the member "does nothing" and "will have no effect". But the stored keys are NOT inert. `recompute_environment_checks` gathers the license keys of EVERY stored environment that has not reached `DELETED` — it applies no `vcfVersion` filter — so a `SELF_DEPLOYED` environment becomes a `KeyCarrier` for each key its `licenseInfo` carried. The `KEY_REUSE` check of a genuine, EVS-installed environment that carries the same key then sees a second carrier and turns `FAILED`, and `Environment$environmentStatus` / `EnvironmentSummary$environmentStatus` roll that up to `FAILED`, with an `impairedSince`.

So creating an environment in the one mode where `licenseInfo` is "not supported" (the model's words) silently impairs the reported health of an unrelated environment — the opposite of the "no effect" the handler doc and the crate's own test comment both claim for that member. The same keys also decide the SELF_DEPLOYED environment's OWN `KEY_REUSE` and `KEY_COVERAGE` results (`keys.is_empty()` is false), so those two checks report a verification over a member that mode does not use, instead of the `UNKNOWN` the crate gives a check whose facts the environment does not carry.

Note the carrier set skips only `DELETED`, so even a still-`CREATING` SELF_DEPLOYED environment impairs the other one. The fix belongs in the carrier gathering (or in what a SELF_DEPLOYED record is taken to carry); the create must keep answering 200, which an existing test pins.

## Evidence

The rule the crate states for the five members `SELF_DEPLOYED` does not support — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/create_environment.rs:109-116:
```
//! Supplying a parameter the chosen VCF mode does not support is NOT one of them.
//! `SELF_DEPLOYED` "does not support" `licenseInfo`, `hosts`, `vcfHostnames`, `siteId`
//! and `connectivityInfo` in the sense that it does nothing with them — no hosts are
//! added and no VCF is installed in that mode — and the recorded wire confirms that a
//! request carrying all five of them together with `vcfVersion` `SELF_DEPLOYED` is NOT
//! refused for carrying them: the refusal it drew named the VPC and nothing else. The
//! members are therefore accepted and stored as supplied, exactly as `clientToken` is
//! ("If you supply input for this parameter, it will have no effect").
```
The model text behind it (`smithy_query(query="doc", shape="CreateEnvironmentRequest", member="licenseInfo")`): "Not supported when `vcfVersion` is `SELF_DEPLOYED`." and (member="vcfVersion"): "SELF_DEPLOYED: You install VCF yourself. The licenseInfo, hosts, vcfHostnames, siteId, and connectivityInfo parameters are not supported."

The store writes the member regardless of the mode — create_environment.rs:794:
```
            license_info: input.license_info.unwrap_or_default(),
```
(compare create_environment.rs:850, where `hosts` IS mode-gated: `add_requested_hosts(&*self.engine, stored, &requested_hosts, installs_vcf)` with `if !installs_vcf { return Vec::new(); }` at :897-899.)

The code that gives those stored keys an effect — src/state.rs:2808-2823:
```
    pub fn recompute_environment_checks(&mut self) {
        let mut key_carriers: BTreeMap<String, Vec<KeyCarrier>> = BTreeMap::new();
        for environment in self.environments.values() {
            // A DELETED environment carries its license key no longer, exactly as it
            // carries its name no longer ([`Self::name_is_taken`]): the key is free for
            // another environment again, so it is not counted as a user of it.
            if environment.state == STATE_DELETED {
                continue;
            }
            for key in environment.license_keys() {
                key_carriers.entry(key).or_default().push(KeyCarrier {
                    environment_id: environment.environment_id.clone(),
                    carried_since_ms: environment.created_at_ms,
                });
            }
        }
```
(the only exclusion is `STATE_DELETED`; nothing asks for `vcf_version`), and what another environment then reports — state.rs:2039-2052:
```
        let reused_since_ms: Vec<i64> = keys
            .iter()
            .filter_map(|key| key_carriers.get(key))
            .flatten()
            .filter(|carrier| carrier.environment_id != self.environment_id)
            .map(|carrier| carrier.carried_since_ms)
            .collect();
        let key_reuse = if keys.is_empty() {
            CHECK_UNKNOWN
        } else if reused_since_ms.is_empty() {
            CHECK_PASSED
        } else {
            CHECK_FAILED
        };
```
with `license_keys()` reading the stored member unconditionally — state.rs:1923-1928:
```
    pub fn license_keys(&self) -> BTreeSet<String> {
        self.license_info
            .iter()
            .flat_map(|license| [license.solution_key.clone(), license.vsan_key.clone()])
            .collect()
    }
```
and the roll-up that makes it visible — state.rs:1912-1913 (`environment_status`) plus to_wire/to_summary at state.rs:2162 and :2179 (`environment_status: Some(self.environment_status().to_string())`).

Test coverage: the crate pins only that the create is ACCEPTED, with the same "does nothing" reason — tests/all/environment_lifecycle.rs:710-722:
```
        // The five parameters `SELF_DEPLOYED` "does not support" are ACCEPTED: the mode
        // does nothing with them, and the recorded wire shows AWS refusing nothing for
        // carrying them (see this operation's handler docs).
...
            "SELF_DEPLOYED accepts the install parameters it does not use: licenseInfo",
            json!({ "vcfVersion": "SELF_DEPLOYED", "licenseInfo": license }),
            200,
```
and the KEY_REUSE table has no SELF_DEPLOYED row at all — tests/all/environment_health_checks.rs:344-398 (`KeyReuseRow` cases: "no license key at all", "keys no other environment carries", "a solution key another environment carries", "only the vSAN key is shared", "a key carried as a solution key here and a vSAN key there", "the only other holder has been deleted"), every row created through `create_with` (environment_health_checks.rs:102-117), which merges into `router.create_request(..)` and so uses an installed VCF version. Nothing pins the carrier set, so excluding a SELF_DEPLOYED record leaves every existing assertion standing.

## Steps to Reproduce

1. CreateEnvironment "A" with an installed version (e.g. `vcfVersion` VCF-5.2.2), four hosts and `licenseInfo` [{solutionKey: K1, vsanKey: K2}]; wait past the transition so it is CREATED.
2. GetEnvironment A: `checks` reports KEY_REUSE PASSED (the only other carrier rule is another environment, and there is none).
3. CreateEnvironment "B" with `vcfVersion` SELF_DEPLOYED and the SAME `licenseInfo` [{solutionKey: K1, vsanKey: K2}] — accepted with 200, which tests/all/environment_lifecycle.rs:719-722 pins, on the stated ground that the mode "does nothing with them".
4. GetEnvironment A again (no other call in between): KEY_REUSE is now FAILED with an `impairedSince`, and `environmentStatus` is FAILED; ListEnvironments reports the same FAILED status in A's summary — an effect produced solely by a member the handler doc at src/ops/create_environment.rs:109-116 says "will have no effect" in that mode.
5. GetEnvironment B: its own KEY_REUSE/KEY_COVERAGE are decided from the same unsupported member (state.rs:2033, :2046-2066) rather than reported as UNKNOWN the way a check whose facts the environment does not carry is (state.rs:2013-2015).
