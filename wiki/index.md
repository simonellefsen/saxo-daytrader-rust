---
type: wiki-index
tags:
  - daytrader/wiki
  - maintained-by-llm
updated: 2026-05-23
---

# Daytrader Knowledge Wiki

This index is the content map for the LLM-maintained project wiki. Future Codex and Hermes sessions should read this file first when answering project-history, architecture, strategy, or operations questions.

## Start Here

- [[schema]] - Maintenance rules, page conventions, and workflows.
- [[log]] - Append-only timeline of wiki operations.
- [[concepts/llm-maintained-project-wiki]] - How the LLM wiki pattern applies to this repository.
- [[concepts/hermes-self-improvement]] - How Hermes learning and strategy experiments connect to the wiki.

## Source Notes

- [[sources/llm-wiki]] - Source-note summary for the LLM Wiki pattern.

## Concepts

- [[concepts/llm-maintained-project-wiki]] - Persistent, compounding project knowledge layer maintained by agents.
- [[concepts/hermes-self-improvement]] - Safe loop for Hermes reflections, experiments, and strategy learning.

## Runbooks

- [[runbooks/README]] - Landing page for operational procedures.
- [[runbooks/build-test-deploy]] - Build, testing, Saxo SIM, deployment, Hermes smoke, and wiki maintenance checklist.

## Decisions

- [[decisions/README]] - Landing page for architecture and workflow decision records.

## Experiments

- [[experiments/README]] - Landing page for Hermes and strategy experiment notes.

## Open Questions

- Which repository pages should be treated as raw sources and which should be folded into generated wiki summaries?
- Should Hermes create wiki pages directly, or should it create database-backed proposals that Codex later files into the wiki?
- What exact qmd collections should be used once the wiki grows past the initial handful of pages?
