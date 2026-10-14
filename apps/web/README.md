# apps/web

Future web app: `rawstock-core` compiled to WebAssembly, rendering with
WebGPU. Phase 6 in [PLAN](../../docs/PLAN.md).

Images are processed entirely client-side and never uploaded to a server.
The Svelte UI lives in [`apps/ui`](../ui) and is shared with `apps/desktop`.
The web shell itself is still Phase 5: nothing here decodes or uploads an image.
