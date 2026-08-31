---
type: wiki-index
tags:
  - daytrader/wiki
  - maintained-by-llm
updated: 2026-08-31
---

# Daytrader Knowledge Wiki

This index is the content map for the LLM-maintained project wiki. Future Codex and Hermes sessions should read this file first when answering project-history, architecture, strategy, or operations questions.

## Start Here

- [schema](schema.md) - Maintenance rules, page conventions, and workflows.
- [log](log.md) - Append-only timeline of wiki operations.
- [roadmap](roadmap.md) - Potential improvements across reliability, Hermes, strategy, execution, UX, and architecture.
- [urgent-todo](urgent-todo.md) - Short ranked list of verified exposures that should not wait for roadmap sequencing.
- [todo](todo.md) - Open items that are not defects: decided-in-principle work and decisions that are the operator's to make.
- [concepts/llm-maintained-project-wiki](concepts/llm-maintained-project-wiki.md) - How the LLM wiki pattern applies to this repository.
- [concepts/current-system-architecture](concepts/current-system-architecture.md) - Current Rust runtime, advisory inputs, deterministic execution boundary, broker authority, and ownership model.
- [concepts/hermes-self-improvement](concepts/hermes-self-improvement.md) - How Hermes learning, expiring read-only memory, audits, baseline evidence, and strategy experiments connect to the wiki.
- [concepts/markov-regime-model](concepts/markov-regime-model.md) - How the Markov signal is actually computed, why every tuning counts bars rather than days, and which model changes have been tested and rejected.
- [QuiverQuant advisory signals](/Users/lindau/codex/rust_daytrader/docs/quiver-signals.md) - Rust alternative-data signal implementation for decision reports and Hermes.
- [Performance benchmark comparison](/Users/lindau/codex/rust_daytrader/docs/performance-benchmarks.md) - Read-only Saxo-backed ETF-proxy comparison and its limits.

## Source Notes

- [sources/llm-wiki](sources/llm-wiki.md) - Source-note summary for the LLM Wiki pattern.
- [sources/markov-hedge-fund-method](sources/markov-hedge-fund-method.md) - Source-note summary for the Markov regime method.
- [sources/app-economy-insights](sources/app-economy-insights.md) - Public editorial-research source boundary and proposed advisory ingestion shape.

## Concepts

- [roadmap](roadmap.md) - Forward-looking improvement map for product, operations, strategy, AI, and refactoring work.
- [concepts/llm-maintained-project-wiki](concepts/llm-maintained-project-wiki.md) - Persistent, compounding project knowledge layer maintained by agents.
- [concepts/current-system-architecture](concepts/current-system-architecture.md) - Current advisory data flow, execution boundary, protective-stop scope, and service ownership.
- [concepts/hermes-self-improvement](concepts/hermes-self-improvement.md) - Safe loop for Hermes reflections, experiments, and strategy learning.
- [concepts/markov-regime-model](concepts/markov-regime-model.md) - Implemented regime pipeline, bar-count semantics, exchange session lengths, and rejected model extensions.

## Runbooks

- [runbooks/README](runbooks/README.md) - Landing page for operational procedures.
- [runbooks/build-test-deploy](runbooks/build-test-deploy.md) - Build, testing, Saxo SIM, deployment, Hermes smoke, and wiki maintenance checklist.
- [runbooks/k8s-diagnostics](runbooks/k8s-diagnostics.md) - Kubernetes diagnostics, debugging, smoke-test, rollout, CNPG, ngrok, Hermes, and RustFS one-liners.
- [runbooks/backup-restore](runbooks/backup-restore.md) - CloudNativePG and RustFS backup verification and restore rehearsal.

## Decisions

- [decisions/README](decisions/README.md) - Landing page for architecture and workflow decision records.
- [Shadow mid-session Decision Reports and tuning evidence](decisions/2026-08-19-shadow-mid-session-decision-reports.md) - Implemented 14:15 Copenhagen EU and 14:15 New York US shadow pulses, non-execution guarantees, initial typed tuning pulse comparison, and remaining EOD/Hermes/promotion evidence plan.

## Experiments

- [experiments/README](experiments/README.md) - Landing page for Hermes and strategy experiment notes.

## Open Questions

- Which repository pages should be treated as raw sources and which should be folded into generated wiki summaries?
- Should Hermes create wiki pages directly, or should it create database-backed proposals that Codex later files into the wiki?
- What exact qmd collections should be used once the wiki grows past the initial handful of pages?
