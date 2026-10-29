Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7959e6ca7cb81d5c32ad32f8e1e29786f97ad065

# Filter expression subject is parsed but never validated, so a filter declared for the other entity is accepted and applied

_Filed: 2026-10-28T10:03:37Z_
_Operation: GetActionRecommendations | Class: validation_

> _Note: this issue was identified against the code base at commit `7959e6ca7cb81d5c32ad32f8e1e29786f97ad065`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`filter::parse` reads the expression's subject with a plain `word()` call, stores it in `FilterExpression::subject`, and NOTHING in the crate ever reads it again (a crate-wide grep for `subject` finds only the grammar comment, the field, the field's doc, `parse`'s assignment and the struct literal). Two consequences:

1. The registration seam accepts a subject that is neither `ItemID` nor `ActionID` — e.g. `INCLUDE Nonsense WHERE Items.GENRE IN ("Comedy")` registers successfully — although the crate's own grammar comment restricts the slot to those two identifiers, the field doc repeats that restriction, `parse` names it in its own error string, and `register_filter` claims to validate the expression "the way `CreateFilter` does" so that "a stored filter is always one this evaluator can apply". Every other grammar element is validated strictly (the dataset name, the column, the predicate, the combinator), and the crate's own test table of refused expressions covers each of those — the subject is the one element let through unchecked.

2. `filter::bind` decides Items-vs-Actions agreement ONLY from each clause's dataset, so a filter whose own subject declares the OTHER entity is bound and applied: `INCLUDE ItemID WHERE Actions.CHANNEL IN ("App")` is accepted and applied by `GetActionRecommendations` to the Actions catalog (`Entity::Actions`), and symmetrically `INCLUDE ActionID WHERE Items.GENRE IN ("Comedy")` is applied by `GetRecommendations` to the Items catalog. That is exactly the case `bind`'s own doc declares an error — "the caller applied an Items filter to action recommendations, or the reverse" — and it goes undetected because the subject, the part of the expression that states which entity the filter selects, is never consulted.

## Evidence

The grammar the crate documents for the expression it parses — `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/state/filter.rs:13-14`:
```
//! expression := ("INCLUDE" | "EXCLUDE") subject "WHERE" clause { ("AND" | "OR") clause }
//! subject    := identifier                       -- "ItemID" or "ActionID"
```
The field doc restating it — `src/state/filter.rs:171-172`:
```
    /// The subject the expression selects — `ItemID` or `ActionID`.
    pub subject: String,
```
The code that accepts ANY bare word for it — `src/state/filter.rs:463`:
```
    let subject = parser.word("the filtered subject (ItemID or ActionID)")?;
```
(`Parser::word`, `src/state/filter.rs:298-306`, returns `Ok(word)` for every `Token::Word`, with no comparison against either identifier; the value is then stored unexamined at `src/state/filter.rs:487-491`.)

The rule `bind` claims to enforce — `src/state/filter.rs:534-536`:
```
/// A clause over the other dataset cannot be evaluated against this entity's stored
/// attributes at all, which is an error for either kind: the caller applied an Items
/// filter to action recommendations, or the reverse.
```
and the only check it actually makes — `src/state/filter.rs:543-551`:
```
    for (combinator, clause) in &expression.clauses {
        if clause.dataset != entity.dataset() {
            return Err(format!(
                "the filter applies to the {} dataset, which does not describe the {} this \
                 operation recommends",
```
The seam that promises the expression is validated at creation — `src/state.rs:763-768`:
```
/// Register a filter, validating its expression the way `CreateFilter` does.
///
/// `Err` carries the parse error, so an expression the runtime could not apply is
/// refused where the filter is created — never at inference time, and never by
/// quietly filtering nothing.
```
The handler that binds the stored filter for the Actions entity — `src/ops/get_action_recommendations.rs:194-201`:
```
        // 2. The request's filter, over the Actions dataset.
        let request_filter = guard
            .bind_filter(
                registry::present(input.filter_arn.as_deref()),
                Entity::Actions,
                input.filter_values.as_ref(),
            )
            .map_err(deny)?;
```

Test coverage: the crate's refusal table pins every OTHER malformed element and never a bad subject — `tests/all/inference.rs:1656-1674` lists `("no action", …)`, `("no WHERE", …)`, `("no predicate", …)`, `("an unqualified column", …)`, `("a dataset this emulator does not store", "INCLUDE ItemID WHERE Interactions.EVENT_TYPE IN (\"click\")")`, `("an empty value set", …)`, `("an unterminated string", …)`, `("a dangling combinator", …)`; and every accepted expression in the crate pairs its subject with the matching dataset (`tests/all/inference.rs:1641-1646`, `tests/all/absent_attribute_columns.rs:187-214`, `tests/all/action_filter_value_forms.rs:124-136`), so no test exercises a mismatched or nonsense subject.

## Steps to Reproduce

1. Create an emulator and call `registry::register_filter(engine, "arn:aws:personalize:us-east-1:123456789012:filter/subject-mismatch", "INCLUDE ItemID WHERE Actions.CHANNEL IN (\"App\")")` — observe `Ok(true)`: an expression whose subject selects ItemIDs is stored as an Actions filter.
2. Register a PERSONALIZED_ACTIONS campaign with `Campaign::new_actions` (as `tests/all/inference.rs:103-109` does) and POST /action-recommendations with {"campaignArn": <campaign>, "userId": "user-1", "filterArn": "arn:...:filter/subject-mismatch"} — observe HTTP 200 with the filter applied to the Actions catalog, although `src/state/filter.rs:534-536` declares "the caller applied an Items filter to action recommendations" an error for either kind.
3. Symmetrically register "INCLUDE ActionID WHERE Items.GENRE IN (\"Comedy\")" and name it as `filterArn` on POST /recommendations against a USER_PERSONALIZATION campaign — observe HTTP 200 with the filter applied to the Items catalog.
4. Call `registry::register_filter(engine, <arn>, "INCLUDE Nonsense WHERE Items.GENRE IN (\"Comedy\")")` — observe `Ok(true)` and `registry::stored_filter(engine, <arn>)` returning a filter whose `parsed.subject` is "Nonsense", despite the grammar at `src/state/filter.rs:14` and the field doc at `src/state/filter.rs:171`.
