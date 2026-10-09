//! Object-literal functions constructed from another module stay functions.
//!
//! Wakaru does not rewrite object-literal function values to method
//! shorthand, so a constructor that only another module constructs keeps
//! `[[Construct]]`. These bundles pin that end to end on producer output.

use wakaru_core::driver::test_support::unpack as unpack_bundle;
use wakaru_core::DecompileOptions;

fn assert_keeps_function(source: &str, name: &str) {
    let function_property = format!("{name}: function");
    assert!(
        source.contains(&function_property),
        "{name} must stay a function expression:\n{source}"
    );
}

// Synthetic ES5 CommonJS classes in the `Base.extend({ init: function })`
// style. `pair` is defined through an assignment chain and constructed in
// `use` through a member alias; `Mixer` is defined in its own module and
// constructed by `core`. No module both defines and constructs a class.
const ESBUILD_SPLIT_CLASSES: &str = r#"(()=>{var e=(r,i)=>()=>(i||r((i={exports:{}}).exports,i),i.exports);var t=e((j,o)=>{var C={extend:function(r){var i=Object.create(this);for(var u in r)i[u]=r[u];return i.init.prototype=i,i.$super=this,i},init:function(){}},n={lib:{Base:C},algo:{},x:{}};n.mix=function(r,i){return new n.algo.Mixer.init(r).run(i)};o.exports=n});var h=e((y,l)=>{var s=t(),x=s.algo;x.Mixer=s.lib.Base.extend({init:function(r){this.seed=r},run:function(r){return this.seed*31+r}});l.exports=x.Mixer});var f=e((O,c)=>{var v=t(),b=v.lib.Base,m=v.x.Pair=b.extend({init:function(r,i){this.high=r,this.low=i},sum:function(){return this.high+this.low}});c.exports=m});var g=e(q=>{var B=t();f();var d=B.x,w=d.Pair;q.total=function(r,i){return new w.init(r,i).sum()}});var p=e(a=>{h();var P=g(),_=t();a.total=P.total;a.mix=_.mix});globalThis.lib=p();})();"#;

const WEBPACK_SPLIT_CLASSES: &str = r#"(()=>{var t={574(t,i,n){n(148);var r=n(274),e=n(762);i.total=r.total,i.mix=e.mix},762(t){var i={lib:{Base:{extend:function(t){var i=Object.create(this);for(var n in t)i[n]=t[n];return i.init.prototype=i,i.$super=this,i},init:function(){}}},algo:{},x:{},mix:function(t,n){return new i.algo.Mixer.init(t).run(n)}};t.exports=i},148(t,i,n){var r=n(762),e=r.algo;e.Mixer=r.lib.Base.extend({init:function(t){this.seed=t},run:function(t){return 31*this.seed+t}}),t.exports=e.Mixer},369(t,i,n){var r=n(762),e=r.lib.Base,o=r.x.Pair=e.extend({init:function(t,i){this.high=t,this.low=i},sum:function(){return this.high+this.low}});t.exports=o},274(t,i,n){var r=n(762);n(369);var e=r.x.Pair;i.total=function(t,i){return new e.init(t,i).sum()}}};const i={};globalThis.lib=function n(r){const e=i[r];if(void 0!==e)return e.exports;const o=i[r]={exports:{}};return t[r](o,o.exports,n),o.exports}(574)})();"#;

fn unpack_split_classes(bundle: &str) -> Vec<(String, String)> {
    unpack_bundle(
        bundle,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed")
    .modules
}

fn module_with<'a>(modules: &'a [(String, String)], needle: &str) -> &'a str {
    let matches = modules
        .iter()
        .filter(|(_, source)| source.contains(needle))
        .collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "expected one module containing {needle:?}: {modules:#?}"
    );
    &matches[0].1
}

fn assert_split_classes_stay_constructible(modules: &[(String, String)]) {
    // The class-defining modules contain no `new`; another module
    // constructs their `init`.
    let pair = module_with(modules, "this.high");
    assert!(
        !pair.contains("new "),
        "pair module must not construct:\n{pair}"
    );
    assert_keeps_function(pair, "init");
    assert_keeps_function(pair, "sum");

    let mixer = module_with(modules, "this.seed =");
    assert!(
        !mixer.contains("new "),
        "mixer module must not construct:\n{mixer}"
    );
    assert_keeps_function(mixer, "init");
    assert_keeps_function(mixer, "run");
}

#[test]
fn esbuild_split_commonjs_classes_stay_constructible() {
    // shape: producer esbuild@0.28.0 --bundle --platform=browser --format=iife --minify
    // (sink assignment renamed to `globalThis.lib`).
    let modules = unpack_split_classes(ESBUILD_SPLIT_CLASSES);
    assert_split_classes_stay_constructible(&modules);
}

#[test]
fn webpack_split_commonjs_classes_stay_constructible() {
    // shape: producer webpack@5.111.1 mode=production (Terser)
    // (sink assignment renamed to `globalThis.lib`).
    let modules = unpack_split_classes(WEBPACK_SPLIT_CLASSES);
    assert_split_classes_stay_constructible(&modules);
}
