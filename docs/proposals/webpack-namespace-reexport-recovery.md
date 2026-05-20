# Webpack Namespace Re-Export Recovery

Status: **Deferred** — revisit after the remaining helper cleanup work is more complete.

## Context

Webpack can lower namespace imports used in object spreads into local namespace objects populated by runtime getters. The current pipeline now removes the direct helper calls:

```js
const tT = {
  get FunctionToString() {
    return t3;
  },
  get InboundFilters() {
    return QD;
  }
};
const tC = {
  get Breadcrumbs() {
    return Oo;
  },
  get Dedupe() {
    return Iq;
  }
};
export let jK = {
  ...de,
  ...tT,
  ...tC
};
```

In the Sentry 7.50.0 browser package, this maps back to `packages/browser/src/index.ts`:

```ts
import { Integrations as CoreIntegrations } from '@sentry/core';
import * as BrowserIntegrations from './integrations';

const INTEGRATIONS = {
  ...windowIntegrations,
  ...CoreIntegrations,
  ...BrowserIntegrations,
};

export { INTEGRATIONS as Integrations };
```

Source reference: https://github.com/getsentry/sentry-javascript/blob/7.50.0/packages/browser/src/index.ts

## Deferred Target

Once lower-level helper recovery is stable, consider folding local getter namespace spreads into the containing object literal when it is semantically safe:

```js
const INTEGRATIONS = {
  ...windowIntegrations,
  get FunctionToString() {
    return t3;
  },
  get InboundFilters() {
    return QD;
  },
  get Breadcrumbs() {
    return Oo;
  },
  get Dedupe() {
    return Iq;
  }
};
export { INTEGRATIONS as Integrations };
```

The more ambitious version would recover namespace import/re-export intent, but that likely needs cross-module facts or stronger provenance tracking.

## Safety Notes

- Do not eagerly rewrite getters to value properties when referenced bindings may be declared later; webpack getters preserve lazy access and live-ish behavior.
- Only inline namespace objects with a single object-spread use, no intervening mutation, and no direct property reads that depend on object identity.
- Preserve spread order. In Sentry's case `...windowIntegrations` intentionally comes before core/browser integrations.
- Keep this separate from `UnWebpackObjectGetters`; that rule should remain a helper-shape cleanup, while this is a higher-level object-spread/namespace recovery.
