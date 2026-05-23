---
type: wiki-schema
tags:
  - daytrader/wiki
  - maintained-by-llm
updated: 2026-05-23
---

# Wiki Schema

This file is the operating contract for agents maintaining the daytrader knowledge wiki.

## Layer Rules

- Raw sources are immutable. Read them, cite them, and summarize them, but do not rewrite them as part of wiki maintenance.
- The wiki is generated synthesis. Agents may create and update pages under `wiki/`.
- The schema defines conventions. Update it when the workflow changes.

## Page Types

Use YAML frontmatter on maintained wiki pages:

```yaml
---
type: concept
tags:
  - daytrader/wiki
updated: 2026-05-23
sources:
  - wiki/sources/llm-wiki.md
---
```

Recommended `type` values:

- `wiki-index`
- `wiki-log`
- `wiki-schema`
- `source-note`
- `concept`
- `runbook`
- `decision`
- `experiment`
- `capability`

## Link Rules

- Use relative Markdown links for wiki-to-wiki links, for example `[Hermes self-improvement](concepts/hermes-self-improvement.md)`. These work in GitHub previews and remain readable in Obsidian.
- Use Markdown links with absolute local paths for repository files outside `wiki/`.
- Prefer linking source files and code paths over copying long excerpts.
- Avoid unresolved wikilinks unless the missing page is intentionally listed as an open task.

## Ingest Workflow

When processing a new source:

1. Read the source.
2. Create or update a `wiki/sources/<source-name>.md` page.
3. Update affected concept, runbook, decision, or experiment pages.
4. Update `wiki/index.md`.
5. Append one entry to `wiki/log.md`.
6. If the source changes safety or trading behavior, cross-link [docs/hermes-agent.md](/Users/lindau/codex/rust_daytrader/docs/hermes-agent.md) and relevant code files.

## Query Workflow

When answering from the wiki:

1. Search `wiki/index.md` first.
2. Use `rtk qmd search` or `rtk qmd query` when the local index is configured.
3. Retrieve full pages before making claims.
4. Cite wiki pages and raw source files.
5. If the answer produces durable knowledge, update the relevant wiki page and log the change.

## Lint Workflow

Periodically check:

- `wiki/index.md` lists all maintained pages.
- `wiki/log.md` has a chronological entry for each maintenance pass.
- No concept page contradicts newer code, config, or docs.
- Hermes experiment notes reference the active goal contract and baseline.
- No wiki page contains secrets, tokens, account keys, or raw credentials.
- Obsidian reports no important unresolved links or orphan pages.

Useful commands:

```bash
rtk rg -n "\\[\\[|TODO|secret|token|AccountKey|ClientKey" wiki docs
rtk obsidian unresolved total
rtk obsidian orphans total
rtk qmd search "Hermes baseline experiment" -c daytrader-wiki -n 10
```

## Safety Rules

- Never store Saxo tokens, refresh tokens, `ClientKey`, `AccountKey`, client secrets, xAI keys, ngrok keys, database credentials, TradingView credentials, or TOTP seeds in the wiki.
- Broker behavior claims must point to code, docs, source notes, or audited records.
- Strategy learning pages must distinguish hypothesis, evidence, result, and active baseline.
- Hermes may propose changes; only approved code/config/database flows may activate changes.
