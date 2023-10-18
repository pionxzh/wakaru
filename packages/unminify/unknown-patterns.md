## Unknown Patterns

These are patterns that are not yet categorized or better understood.

#### Looks like some aggressive assignment merging

```js
var ei;
var eo;

var ec =
  (((ei = ec || {})[(ei.None = 0)] = "None"),
  (ei[(ei.RenderStrategy = 1)] = "RenderStrategy"),
  (ei[(ei.Static = 2)] = "Static"),
  ei);
var ed =
  (((eo = ed || {})[(eo.Unmount = 0)] = "Unmount"),
  (eo[(eo.Hidden = 1)] = "Hidden"),
  eo);
```

```js
let ei;
let eo;

(ei = ec || {})[(ei.None = 0)] = "None";
ei[(ei.RenderStrategy = 1)] = "RenderStrategy";
ei[(ei.Static = 2)] = "Static";
var ec = ei;
(eo = ed || {})[(eo.Unmount = 0)] = "Unmount";
eo[(eo.Hidden = 1)] = "Hidden";
```
