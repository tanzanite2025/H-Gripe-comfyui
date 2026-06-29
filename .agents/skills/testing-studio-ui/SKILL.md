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

## i18n specifics
- Strings live in `src/i18n.ts` (`messages` dict, `translate`, `loadLang/saveLang`, `LangContext`, `useT`). The toolbar `中文/EN` button toggles language.
- Scope of translation is the app "chrome" (toolbar, tooltips, Snapshots/Project/RunLog/search panels). The **left node palette names/categories and per-node inspector fields stay English** (graph spec / domain terms) — by design, not a regression.
- Watch out: a literal `*/` inside a `/** ... */` JSDoc comment (e.g. writing `zh*/en`) terminates the comment early and breaks the TS build. Use `zh / en` instead.

## Conventions
- LF-only line endings are enforced (a check-line-endings CI step). On Windows, verify no `\r` before committing.
- CI does not build the Tauri desktop app, so Rust-only changes can't be verified here; pure-frontend changes are.

## Devin Secrets Needed
- None. Browser-preview testing requires no credentials or secrets.
