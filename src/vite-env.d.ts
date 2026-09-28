/// <reference types="vite/client" />

/**
 * Build-time constant injected by `vite.config.ts` (`define`). It is replaced
 * with the literal `true` or `false` before bundling, which is what lets rollup
 * drop `src/lib/mock.ts` entirely from a production build.
 */
declare const __ONYX_MOCK__: boolean;
