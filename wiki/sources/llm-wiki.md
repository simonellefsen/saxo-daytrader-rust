---
type: source-note
tags:
  - daytrader/wiki
  - llm-wiki
updated: 2026-05-23
source: https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f
author: Andrej Karpathy
source_url: https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f
---

# Source Note: LLM Wiki

Source: [karpathy/442a6bf555914893e9891c11519de94f](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f)

Credit: the original LLM Wiki idea file is by Andrej Karpathy. Original gist: [karpathy/442a6bf555914893e9891c11519de94f](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f).

## Summary

The source proposes a persistent LLM-maintained markdown wiki as an alternative to plain RAG. Instead of retrieving raw chunks on every question, the agent incrementally reads sources, extracts durable knowledge, updates interlinked pages, resolves contradictions, and keeps a maintained synthesis layer current.

The key layers are:

- Raw sources: immutable source documents.
- Wiki: LLM-generated markdown summaries, concepts, entity pages, comparisons, and synthesis.
- Schema: instructions that define wiki structure and workflows for future agents.

The key operations are:

- Ingest: process a new source and update all affected wiki pages.
- Query: answer from the wiki and file reusable answers back into it.
- Lint: check contradictions, stale claims, missing links, orphan pages, and gaps.

The recommended navigation files are:

- `index.md`: content-oriented catalog.
- `log.md`: chronological append-only operation log.

Optional tools include qmd for local markdown search and Obsidian for graph/backlink navigation.

## Application To This Repo

For `saxo-rust`, the wiki should preserve project learning that otherwise gets lost across chat sessions:

- Rust migration decisions.
- Saxo order safety and audit lessons.
- Hermes reflection and strategy experiment outcomes.
- Kubernetes and database operational knowledge.
- Strategy hypotheses and their evidence.

The wiki should work alongside [docs/hermes-agent.md](/Users/lindau/codex/rust_daytrader/docs/hermes-agent.md). Hermes can generate reflections and proposals, while Codex maintains the human-readable wiki that future agents can search and update.

## Links

- [concepts/llm-maintained-project-wiki](../concepts/llm-maintained-project-wiki.md)
- [concepts/hermes-self-improvement](../concepts/hermes-self-improvement.md)
