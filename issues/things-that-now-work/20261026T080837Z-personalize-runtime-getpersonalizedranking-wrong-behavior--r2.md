Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 23e63f00b11bb74d67049d5cb37769202febdd46

# GetPersonalizedRanking drops an inputList item the catalog never held when the filter uses NOT IN, contradicting its own stated rule

_Filed: 2026-10-26T08:08:37Z_
_Operation: GetPersonalizedRanking | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `23e63f00b11bb74d67049d5cb37769202febdd46`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler's step 5 states the rule it means to apply to an `inputList` entry that has no record in the campaign's stored Items catalog: the request's filter decides on an EMPTY attribute set, and therefore "an INCLUDE filter cannot keep it, an EXCLUDE filter cannot remove it". The code under that comment calls `BoundFilter::retains(&no_attributes)` and, for a negated `IN` predicate, gets exactly the opposite answer.

`filter::evaluate` computes `matched != *negated` for an `In` predicate. For an entry with no attributes, `stored` is `None`, so `matched == false`; with `negated == true` (`NOT IN`) the clause therefore HOLDS. Consequently:

* `EXCLUDE ItemID WHERE Items.GENRE NOT IN ($GENRE)` (parameter bound) makes `matches()` true for the attribute-less entry, `retains()` returns `!true == false`, and the handler executes `continue` at get_personalized_ranking.rs:284 — the unknown item is dropped from the response entirely, even though the handler's comment and the modelled `inputList` doc say it is appended to the end of the reranked list.
* `INCLUDE ItemID WHERE Items.GENRE NOT IN ($GENRE)` makes `retains()` return true, so the attribute-less entry IS kept by an INCLUDE filter — again the opposite of the stated rule.

The result is that the response is no longer a permutation of `inputList` minus the entries the filter removed by their STORED attributes: entries with no stored record are added or removed purely by the shape (negated vs. non-negated) of the filter predicate, which the handler explicitly claims cannot happen. This is a different defect from the already-fixed score-normalisation issue for this operation (`20261024T020925Z-personalize-runtime-getpersonalizedranking-wrong-behavior.md`): it changes WHICH entries the ranking holds, not what they score.

## Evidence

The rule the handler states — `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/ops/get_personalized_ranking.rs:256-259`:
```
        // 5. Report the entries `inputList` names, and collect the ones the catalog
        //    does not hold. An entry with no catalog record carries no stored
        //    attributes, so the request's filter decides on an empty attribute set: an
        //    INCLUDE filter cannot keep it, an EXCLUDE filter cannot remove it.
```
The code under it — `src/ops/get_personalized_ranking.rs:279-288`:
```
                None => {
                    if request_filter
                        .as_ref()
                        .is_some_and(|filter| !filter.retains(&no_attributes))
                    {
                        continue;
                    }
                    appended.push(item_id.clone());
                }
```
What `retains` actually answers for an empty attribute set and a negated `IN` — `src/state/filter.rs:741-751`:
```
fn evaluate(clause: &BoundClause, attributes: &BTreeMap<String, String>) -> bool {
    let stored = attributes.get(&clause.column);
    match &clause.predicate {
        BoundPredicate::In { values, negated } => {
            let matched = stored.is_some_and(|raw| {
                raw.split('|')
                    .map(str::trim)
                    .any(|held| values.iter().any(|value| value == held))
            });
            matched != *negated
        }
```
and `src/state/filter.rs:703-710`:
```
    pub fn retains(&self, attributes: &BTreeMap<String, String>) -> bool {
        let matched = self.matches(attributes);
        match self.kind {
            FilterKind::Include => matched,
            FilterKind::Exclude => !matched,
        }
    }
```
So with `stored == None` and `negated == true`: `matched == false`, `false != true == true` → the clause holds → an `EXCLUDE` filter removes the entry (`retains == false`) and an `INCLUDE` filter keeps it (`retains == true`), the exact opposite of the quoted comment.

The modelled sentence the drop also contradicts — `smithy_query(query="doc", shape="GetPersonalizedRankingRequest", member="inputList")`: "A list of items (by itemId) to rank. If an item was not included in the training dataset, the item is appended to the end of the reranked list." The handler mirrors it at `src/ops/get_personalized_ranking.rs:313-315`: "If an item was not included in the training dataset, the item is appended to the end of the reranked list".

The `NOT IN` predicate is supported by the parser and reachable from a registered filter — `src/state/filter.rs:389-396` (`Some(Token::Word(word)) if word.eq_ignore_ascii_case("NOT") => { ... Predicate::In { values: self.value_set()?, negated: true } }`), and the crate's own tests register such expressions (`tests/all/action_filter_value_forms.rs:132`: `"INCLUDE ActionID WHERE Actions.CHANNEL NOT IN ($CHANNEL)"`).

Test coverage leaves the case open: `tests/all/inference.rs:902` (`personalized_ranking_is_a_permutation_of_the_input_list`) sends unknown ids (`"an id the training dataset never held"`, `"only unknown ids"`) but names NO `filterArn`; `tests/all/inference.rs:986` (`personalized_ranking_drops_the_items_its_filter_excludes`) uses only catalog ids with `FILTER_EXCLUDE_COMEDY`, registered at `tests/all/inference.rs:130-132` as `"EXCLUDE ItemID WHERE Items.GENRE IN (\"Comedy\")"` — a non-negated predicate. A repo-wide grep for `NOT IN` finds no GetPersonalizedRanking test at all.

## Steps to Reproduce

1. Register a PERSONALIZED_RANKING campaign whose Items catalog carries a GENRE column via the `register_campaign` seam (as `tests/all/inference.rs:91-102` does).
2. Register a filter with `registry::register_filter(engine, arn, "EXCLUDE ItemID WHERE Items.GENRE NOT IN ($GENRE)")`.
3. POST /personalize-ranking with {"campaignArn": <campaign>, "userId": "user-4", "inputList": ["item-5", "no-such-item"], "filterArn": <filter>, "filterValues": {"GENRE": "Comedy"}}.
4. Observed: `personalizedRanking` holds only the catalog entries the filter kept; `no-such-item` is absent, because `retains(&no_attributes)` returned false for the NOT IN clause. Expected per get_personalized_ranking.rs:256-259 ("an EXCLUDE filter cannot remove it") and the `inputList` doc ("the item is appended to the end of the reranked list"): `no-such-item` is appended at the end.
5. Repeat with the filter expression `INCLUDE ItemID WHERE Items.GENRE NOT IN ($GENRE)`. Observed: `no-such-item` IS returned, although the same comment says "an INCLUDE filter cannot keep it".
