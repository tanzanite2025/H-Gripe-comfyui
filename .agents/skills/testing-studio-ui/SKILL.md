---
name: testing-studio-ui
description: Test the H-Gripe Studio desktop UI (apps/desktop-tauri/studio-ui) end-to-end in the browser preview. Use when verifying toolbar/panel/i18n or other studio-ui frontend changes.
---

# Testing H-Gripe Studio UI

`apps/desktop-tauri/studio-ui` is a React + Vite frontend. It can be exercised in a **browser preview** (backend mocked) without the Tauri/Rust desktop build — most UI/UX changes are fully verifiable this way.

## Setup / commands (run in `apps/desktop-tauri/studio-ui`)
- Install: `npm install`
- Typecheck: `npm run typecheck`
- Unit tests: `npx vitest run` (Vitest)
- Build: `npm run build`
- Dev server: `npm run dev` — Vite picks the next free port if 5173/5174/5175 are taken; read the actual URL from its output.

## Browser testing notes
- The app shows `browser preview (backend mocked)` under the toolbar; native-only buttons (Open…/Save As…/Project) are hidden in this mode by design — don't treat their absence as a bug.
- An unsaved-changes guard (`beforeunload`) means **reloading or navigating pops a "Reload site? / Changes you may not be saved" dialog** — click **Reload** to proceed. Expect this on every F5.
- Language preference persists in `localStorage` key `hgripe.studio.lang.v1`; snapshots/auto-snapshot prefs also live in localStorage. To test "fresh defaults" use an incognito window or clear storage.
- The annotated DOM returned alongside screenshots is the fastest way to assert on button labels/tooltips/placeholder text — read `title=` and text content there rather than eyeballing tiny toolbar text.

## Node Editor / graph card testing
- The **Node Editor** tab hosts the React Flow graph. The left **Nodes** palette groups cards by category (Inputs/Generate/Control/Utility/Outputs) and is searchable; the search matches title **and description**, so a query like `Subject` can surface several cards whose descriptions merely mention the word — confirm by the exact title + group header.
- **Executor badge**: cards show a `Local` / `API` / `Local/API` badge next to the title; the `compute` (native-Rust) lane carries **no badge**. Absence of a badge on a `compute` card is correct, not a bug.
- **Click a palette item to add it** to the canvas (drag also works). New nodes spawn near existing ones and may overlap — drag the node to empty canvas before inspecting ports.
- **Counting port handles reliably**: the rendered handles are tiny dots; zooming screenshots is error-prone when an edge wire crosses them. The robust way is the DOM — count `.react-flow__handle` within the node element, split by class `.target` (inputs, left) vs `.source` (outputs, right). Example via `browser_console`:
  `const n=[...document.querySelectorAll('.react-flow__node')].find(e=>e.textContent.includes('<Title>')); console.log(n.querySelectorAll('.react-flow__handle.target').length, n.querySelectorAll('.react-flow__handle.source').length)` — this is a legitimate assertion, not a UI shortcut.
- **Inspector** (right panel) appears when a node is selected and lists every param with its default; the annotated DOM gives exact `value`/`selected`/range for each `<input>`/`<select>` — read defaults there rather than eyeballing sliders.
- **Modal editors / review gates** (e.g. Subject Mask's Preview + Mask-Edit modals): some cards render a lightweight node body (thumbnail/placeholder + action buttons like Auto/Edit/Preview) and open heavy editors in modals. Test the **persistence round-trip** by performing edits (brush stroke, queued ops shown as chips like `wand 24`/`grow 4`, with an "Edits (N)" counter), clicking **Apply**, closing, then **reopening** the editor — the edits must survive (they are committed to a node param such as `edit_paths`). A reopen showing "Edits (0)"/empty chips means the commit/read-back is broken. Also verify a **tool registry** split: enabled tools vs greyed/disabled "soon" tools (planned phases) — planned tools must not be clickable. Modal hand-off (Preview's Edit button → opens Mask-Edit) is wired via shared callbacks; confirm one modal closes and the other opens with state intact. Backend-mocked preview shows a checkerboard underlay and "(not produced yet)" layers — expected, not a bug.

## Environment quirks (Windows test box)
- **Typing a URL with `:` in the Chrome omnibox**: the `type` action may drop the colon (e.g. `localhost:5173` → `localhost5173`, which then triggers a Google search). Type the host, then send the colon as a key (`shift+semicolon`), then the port — e.g. `type "localhost"`, `key shift+semicolon`, `type "5173"`.
- **`upload_attachment` only accepts platform-rooted POSIX paths, where `/tmp` maps to the Windows `C:\tmp`** — it rejects drive paths (`C:\...`/`C:/...` → "must be absolute") and cannot find shell-style `/c/...` paths (the Git-Bash mount is a different view). Reliable recipe: the recording tool returns its mp4 under `/tmp/devin-recordings/<rec-id>/...edited.mp4` and that uploads as-is. For screenshots (saved by the screenshot tool under `C:\Users\...\screenshots\`), **copy them into `C:\tmp\` first** (`cp /c/Users/.../ss_*.png /c/tmp/ssup/`) then upload via the `/tmp/ssup/ss_*.png` path — that succeeds and returns `app.devin.ai/attachments/...` URLs.
- **PR-comment image auto-upload also chokes on `/c/...` / `C:\...` markdown paths** (posts with broken-image warnings). Cleanest flow: `upload_attachment` the screenshots via the `/tmp/...` recipe above to get URLs, post the comment with `git_comment_on_pr`, then `git_edit_comment` to swap in the returned `app.devin.ai/attachments/...` URLs (or just include the URLs from the start).

## i18n specifics
- Strings live in `src/i18n.ts` (`messages` dict, `translate`, `loadLang/saveLang`, `LangContext`, `useT`). The toolbar `中文/EN` button toggles language.
- App "chrome" (toolbar, tooltips, Snapshots/Project/RunLog/search panels) localizes via `src/i18n.ts`.
- **Node cards DO localize now** (since the nodeSpecs i18n work): node title/description, param `label`+`hint`, port labels and select option *labels* are overlaid by `src/graph/nodeSpecsI18n.ts` (`localizeSpec(nodeSpec(kind), lang)`), applied in the node body, Inspector (`Inspector.tsx`), palette and search. So in 中文 a param like Image Enhance's `engine` shows label **引擎** with a zh hint — verify by reading the `<label>` text in the annotated DOM. Note option **ids/values are NOT translated** (e.g. `cpu`/`realesrgan` stay literal); only the chrome around them changes. A missing zh entry is caught by the `nodeSpecsI18n.test.ts` coverage guard (CI red), and at runtime falls back to the English string.
- To test a newly-added node param: search the palette for the card (e.g. "Image Enhance"), click to add it, select it; the param renders both inline on the node body and in the right Inspector. A `select` param's options/default are most reliably read from the annotated DOM (`<select selectedindex=...>` + `<option value=...>`).
- Watch out: a literal `*/` inside a `/** ... */` JSDoc comment (e.g. writing `zh*/en`) terminates the comment early and breaks the TS build. Use `zh / en` instead.

## Conventions
- LF-only line endings are enforced (a check-line-endings CI step). On Windows, verify no `\r` before committing.
- CI does not build the Tauri desktop app, so Rust-only changes can't be verified here; pure-frontend changes are.

## Devin Secrets Needed
- None. Browser-preview testing requires no credentials or secrets.
