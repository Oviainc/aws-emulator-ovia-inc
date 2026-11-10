Issue-Severity: MEDIUM
Issue-Gap-Class: error-response

# A locking statement whose row query runs out of its own budget answers `program_limit_exceeded` (`54000`) where PostgreSQL cancels a statement with `57014`

_Filed: 2026-11-10T07:30:00Z_
_Operation: the PostgreSQL engine endpoint of a DB instance (`SELECT … FOR UPDATE`) | Class: error-response_

> _Note: this issue was filed from a code-review finding on the row-locking bound, not from a recorded unit. The behaviour is the current one at the time of filing, so trace it to the code base you are working in._

## Description

A statement carrying a locking clause is served in two steps by the engine endpoint: the
endpoint first runs a ROW QUERY to identify the rows the clause will take
(`InstanceDatabase::row_ids`), then runs the statement itself. Both steps run under the
statement's own deadline — the session's `statement_timeout` clamped by the endpoint's
budget — which is correct and tested.

What is reported when the FIRST step runs out of that budget is not. `row_ids` answers
`None` for every reason a row query did not yield rows, and
`take_the_rows_it_locks` maps that `None` on a statement that carries a clause to
`unservable_clause`, i.e. `EngineErrorKind::LimitExceeded` → SQLSTATE `54000`
(`program_limit_exceeded`), with the message about holding at most a number of rows "and
only where it can identify them". So a client whose `SELECT … FOR UPDATE` was CANCELLED by
its own `statement_timeout` is told its clause is unservable.

PostgreSQL words that condition once, for every statement: `canceling statement due to
statement timeout`, SQLSTATE `57014` (`query_canceled`). The emulator already answers
exactly that for the two other statement forms whose row query it evaluates first — an
`UPDATE` and a `DELETE` with the same waiting predicate are cancelled `57014` — so the
divergence is in the locking-clause path only, and a client cannot tell a cancellation from
a refusal to serve the clause at all. `54000` is also the wrong class to retry on: a
cancelled statement is retryable with a larger `statement_timeout`, an unservable clause is
not.

"The clause cannot be served" remains the right answer for its own cause: a row set the
endpoint genuinely cannot identify, or more rows than it locks. The two causes are
distinguishable at the one call site — the statement's deadline is known there before the
row query starts.

## Evidence

`services/service-com-amazonaws-rds-amazonrdsv19/src/state/data_plane/wire_session.rs`,
in `take_the_rows_it_locks`: every `None` from `row_ids` becomes the unservable-clause
refusal, with no account of whether the budget ran out —

```rust
    let Some(ids) = database.row_ids(target, &server_view(facts), &plan.rows_sql, sql, params)
    else {
        if plan.carries_a_clause() {
            return Err(unservable_clause(&plan));
        }
        …
    };
```

`unservable_clause` builds `EngineErrorKind::LimitExceeded`, which the PostgreSQL wire
renders as `54000`
(`services/service-com-amazonaws-rds-amazonrdsv19/src/state/data_plane/pg_wire.rs`).

The behaviour is currently asserted, for the locking-clause row, by
`services/service-com-amazonaws-rds-amazonrdsv19/tests/all/engine_server_surface.rs`,
`a_waiting_function_in_a_locking_statements_predicate_stops_at_the_deadline`, whose other
two rows assert `57014` for the same condition — so fixing this means changing that row's
expected SQLSTATE to `57014` along with the handler.

## Steps to Reproduce

1. Create a PostgreSQL DB instance and connect to the endpoint it publishes.
2. `CREATE TABLE waits (id int PRIMARY KEY)`, insert a row, then `SET statement_timeout = 100`.
3. Run `SELECT id FROM waits WHERE pg_sleep(3) IS NULL FOR UPDATE`.
4. Observe the refusal comes back promptly (correct) as SQLSTATE `54000` rather than
   `57014` with `canceling statement due to statement timeout`.
5. Run the same predicate as `UPDATE waits SET id = id WHERE pg_sleep(3) IS NULL` and
   observe `57014` for the same condition.
