# Native plans

Plans render inline as Markdown. Author a `.grove-plan.json` envelope and link its absolute path in a completed assistant message:

```json
{
  "schema_version": 1,
  "plan_id": "search-performance",
  "revision_id": "r1",
  "title": "Make search responsive",
  "markdown": "# Make search responsive\n\nMove indexing off the input thread.\n\n## Approach\n1. Measure the current path.\n2. Move indexing into a worker.\n3. Ignore stale results.\n\n## Verification\nCompare matching and ranking; measure input-to-paint latency."
}
```

The Markdown field contains the complete readable plan. Use headings, lists, tables, fenced code and file links; scale detail to the task. Ask consequential questions in the document and collect answers through plan feedback. Do not author HTML or claim/exhibit trees for new plans. The envelope retains immutable identity and native review/approval controls. Change the revision ID when content changes. Limits remain 1 MiB per document and 64 KiB per text field.

The conversation renders the complete Markdown. The right-panel **Plan** section lists the latest revision of each plan, including approved plans, as references for current work. Hover opens the same document in an anchored peek; crossing into it preserves the peek, click pins it, and Escape restores source focus. The inline document remains readable while its peek is open.

**Review response** previews exact canonical feedback. Explicit **Send response** sends it to the owning session; feedback never authorizes implementation. **Approve & implement** is a separate explicit action bound to the current immutable revision. Queues, delivery journals, duplicate protection and composer draft preservation remain unchanged.

Existing structured plans remain importable and convert to Markdown for reading. Legacy decisions retain native answer controls and explicit-answer approval gates. Their original stored documents are never rewritten.

Verification must name both **Codex and Claude**. Core tests establish import, persistence, revision and delivery contracts; native fixtures establish only the exercised UI behavior. Neither establishes live provider-boundary behavior. Inspect inline Markdown, latest-revision hover/pinning, source focus, narrow widths and zoom with real native rendering; report unavailable platforms and live checks explicitly.
