Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kendra-ranking.rescore-tokenized-title-body-alternative-input
Issue-Shape-Id: com.amazonaws.kendraranking#AWSKendraRerankingFrontendService
Issue-Key: kendra-ranking.rescore-tokenized-title-body-alternative-input|run#12|AssertionError|assert scores["plain-boosted"] > 0.3, (
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# Rescore does not boost a relevant document above a low OriginalScore: score is dominated by OriginalScore via a stopword-diluted lexical blend

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Rescore

## Description
One root cause, filed alone.

Cause 1 of 1: Rescore's returned Score cannot materially promote a document whose text is relevant to SearchQuery when that document arrives with a deliberately low OriginalScore. Failing check: kendra-ranking.rescore-tokenized-title-body-alternative-input.

AWS: Smithy model, com.amazonaws.kendraranking#Rescore — "Rescores or re-ranks search results from a search service such as OpenSearch (self managed). You use the semantic search capabilities of Amazon Kendra Intelligent Ranking to improve the search service's results."; Document$OriginalScore (Float, required, range -100000..100000) — "The original document score or rank from the search service. Amazon Kendra Intelligent Ranking gives the document a new score or rank based on its intelligent search algorithm"; RescoreResult$ResultItems — "A list of result items for documents with new relevancy scores. The results are in descending order." The model therefore states the new Score is derived from the ranking algorithm's view of relevance, not that it is a bounded perturbation of OriginalScore. The MAGNITUDE of a semantic score is model output and is not a contract; what the recording pins is directional: for a document whose Title/Body plainly answers the query (Title "Electric vehicle batteries", Body "Lithium ion batteries power electric cars.", query "how do electric car batteries work") submitted with OriginalScore 0.2, real AWS returned a Score well above that original score — recorded behavior only, observed in the 0.43-0.45 band across replays, i.e. roughly double the OriginalScore. The check encodes only the directional floor (> 0.3), not the recorded values.

Emulator: the same request returns Score 0.2666668 for that document (and 0.4166667 for the sibling tokenized document submitted with OriginalScore 0.5), so a fully on-topic document is lifted only ~0.067 above its OriginalScore and the batch's order is just the OriginalScore order. The handler computes score = 0.5*OriginalScore + 0.5*(0.7*title_coverage + 0.3*body_coverage), where coverage is the fraction of the query's DISTINCT raw terms present in the field. Two structural consequences: (a) the relevance term is capped at 0.5 and weighted equally with OriginalScore, so a low OriginalScore permanently halves the achievable score; (b) query terms are taken verbatim with no stopword handling, so "how do electric car batteries work" yields 6 terms of which only "electric" and "batteries" can ever match, capping coverage at ~0.33 for a document that answers the query completely. Net: relevance can contribute at most ~0.17 here, and the emulator's "new relevancy score" is in practice OriginalScore with a small nudge.

Why: a client's whole reason to call Intelligent Ranking is to promote results its own search engine under-scored. Against this emulator that promotion never happens: result sets come back in essentially the submitted OriginalScore order, so relevance-threshold logic ("drop items scoring below X", "only show rescored items that beat their original score") and any re-ordering test passes on AWS and silently no-ops or inverts here. It also makes the emulator unusable as a positive control for "is rescoring active at all", because an OriginalScore echo and a real rescore are indistinguishable.

Fix: the Rescore handler's scoring heuristic — the score blend and the query-term extraction it feeds (ORIGINAL_SCORE_WEIGHT / RELEVANCE_WEIGHT, coverage(), distinct_query_terms()/terms_of()). The emulator cannot run a semantic model and does not need to reproduce AWS's numbers, but the heuristic should be shaped so that a document whose text covers the query's content words is boosted materially above its OriginalScore (e.g. measure coverage against content-bearing query terms rather than every whitespace-delimited token, and let the relevance component dominate a low OriginalScore), while keeping the properties already honored: one ResultItem per submitted DocumentId, descending order, Score inside Float's modeled -100000..100000 range, and determinism for a given input.

Traces: kendra-ranking.rescore-tokenized-title-body-alternative-input (the TokenizedTitle/TokenizedBody path itself is accepted and returns a valid in-range Score — only the plain Title/Body positive control fails). Implementation cited above: services/service-com-amazonaws-kendraranking-awskendrarerankingfrontendservice/src/ops/rescore.rs lines 297-314 (scoring/sort) and 338-397 (rescored_score, coverage, distinct_query_terms, terms_of).

## Evidence
Check: kendra-ranking.rescore-tokenized-title-body-alternative-input

Request (after the plan reaches ACTIVE):
  Rescore(RescoreExecutionPlanId=<plan>, SearchQuery="how do electric car batteries work",
    Documents=[
      {Id: "tok-doc", TokenizedTitle: [electric, vehicle, batteries],
       TokenizedBody: [lithium, ion, batteries, power, electric, cars], OriginalScore: 0.5},
      {Id: "plain-boosted", Title: "Electric vehicle batteries",
       Body: "Lithium ion batteries power electric cars.", OriginalScore: 0.2}])

Expected (real AWS, recorded behavior only): the relevant plain Title/Body document is boosted well above its low OriginalScore of 0.2 — observed 0.43-0.45 across replays; the check asserts only the directional floor Score > 0.3.

Actual (emulator): AssertionError: expected relevant content to be boosted well above its low OriginalScore of 0.2: {'tok-doc': 0.4166666865348816, 'plain-boosted': 0.2666666805744171}

Arithmetic confirming the cause (not a recorded value, a derivation from the handler):
  query distinct terms = [how, do, electric, car, batteries, work] (6)
  title/body terms matched = {electric, batteries} -> coverage = 2/6 = 0.3333 for both fields
  relevance = 0.7*0.3333 + 0.3*0.3333 = 0.3333
  score = 0.5*0.2 + 0.5*0.3333 = 0.26667  == the observed 0.2666668
So the observed number is exactly the modeled-free emulator blend, and the relevance term is capped at ~0.33 by counting "how", "do", "car" and "work" in the denominator.

Status/shape are otherwise correct: HTTP 200, one ResultItem per submitted DocumentId, Scores inside Float's -100000..100000 range, descending order.

## Steps to Reproduce
1. CreateRescoreExecutionPlan(Name=<unique>) and poll DescribeRescoreExecutionPlan until Status == ACTIVE.
2. Call Rescore with SearchQuery "how do electric car batteries work" and two documents:
   - Id "tok-doc", TokenizedTitle [electric, vehicle, batteries], TokenizedBody [lithium, ion, batteries, power, electric, cars], OriginalScore 0.5
   - Id "plain-boosted", Title "Electric vehicle batteries", Body "Lithium ion batteries power electric cars.", OriginalScore 0.2
3. Read ResultItems and map DocumentId -> Score.
4. Observe Score("plain-boosted") == 0.2666668, i.e. barely above its OriginalScore of 0.2; on real AWS this document comes back boosted well above 0.3 (observed 0.43-0.45). Equivalently, note that every returned Score equals 0.5*OriginalScore + 0.5*lexical_coverage, so a low OriginalScore can never be overcome by relevance.
5. Cleanup: DeleteRescoreExecutionPlan once the plan is ACTIVE.
