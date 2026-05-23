---
type: wiki-log
tags:
  - daytrader/wiki
  - maintained-by-llm
updated: 2026-05-23
---

# Wiki Log

Append-only timeline for project wiki maintenance. Use headings with the format `## [YYYY-MM-DD] kind | summary` so agents and shell tools can parse the log.

## [2026-05-23] ingest | LLM Wiki pattern

- Read [llm-wiki.md](/Users/lindau/codex/rust_daytrader/llm-wiki.md).
- Created the initial project wiki structure under [wiki/](/Users/lindau/codex/rust_daytrader/wiki).
- Added schema, index, source note, and concept pages.
- Added [docs/project-wiki.md](/Users/lindau/codex/rust_daytrader/docs/project-wiki.md) for repo-level workflow documentation.

## [2026-05-23] attribution | LLM Wiki source credit

- Credited Andrej Karpathy as the author of the copied LLM Wiki idea file.
- Added the original gist URL: [karpathy/442a6bf555914893e9891c11519de94f](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f).

## [2026-05-23] implementation | Hermes Kubernetes base

- Added initial Kubernetes support for Hermes Agent in `saxo-rust`.
- Added `hermes-agent`, `hermes-data`, `hermes-gateway`, and `hermes-daytrader-context`.
- Updated deployment scripting to create a separate `hermes-env` secret from a whitelist so Saxo credentials are not passed to Hermes.
- Documented that Hermes is internal-only and not yet connected to a daytrader MCP adapter or strategy promotion flow.
