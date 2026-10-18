# Ondure website

Local React, TypeScript and Vite source. The Rust runtime remains in the Cargo
workspace; the website has its own frozen pnpm lockfile.

```sh
cd apps/website
pnpm install --frozen-lockfile
pnpm dev
```

Open Vite's printed Local URL. Verify and build with:

```sh
pnpm test
pnpm typecheck
pnpm lint
pnpm build
pnpm preview
```

Production assets are emitted into `dist/`. Fonts are bundled locally. The site
has no backend connection; the interactive recovery walkthrough is explicitly a
browser simulation of a committed GET and process interruption.

The page explains the seven bounded agent-runtime features with a readable
example per feature and links to runnable local fixtures. Source links and build
commands use `feat/agent-runtime-website`, since this review series is not merged
or released automatically. Update that reference only when delivery state changes.
The installation flow builds from source and uses a local provider fixture.
No paid credentials, package distribution or measured performance is advertised.

Future streaming, embeddings, model-generated summaries, execution forks, local
inference, OS sandboxing and package registries remain separate from implemented
functionality. The page distinguishes ambiguous effects, configured budget
estimates, trusted local approval and process-crash evidence from stronger claims.

Desktop/mobile browser QA covers layout, navigation/Escape behavior, code copying
and the interruption/recovery controls. Tests also check copy failure fallback
and all example targets. CI checks are independent from local verification.
