/* Generated with google-closure-compiler@20260629.0.0; see generate.mjs. */
/* _GlobalPrefix_ */
"use strict";/*_JS*/
this.default_ClosureProducer=this.default_ClosureProducer||{};
(function(_){var window=this;
/*_M:base*/
try{
_._DumpException=function(error){throw error};
_.beginModule=function(id){_.activeModule=id};
_.endModule=function(){_.activeModule=null};
_._ModuleManager_initialize("base/chunk_final:0/chunk_alpha:0/empty_one:0/empty_two:0/empty_three:0/chunk_beta:2/empty_four:0/empty_five:0",["chunk_alpha","empty_one","empty_two","empty_three","chunk_beta","empty_four","empty_five","chunk_final"]);
var e=typeof Object.defineProperties=="function"?Object.defineProperty:function(a,b,c){if(a==Array.prototype||a==Object.prototype)return a;a[b]=c.value;return a};function g(a){a=["object"==typeof globalThis&&globalThis,a,"object"==typeof window&&window,"object"==typeof self&&self,"object"==typeof global&&global];for(var b=0;b<a.length;++b){var c=a[b];if(c&&c.Math==Math)return c}throw Error("Cannot find global object");}var h=g(this);
function k(a,b){if(b)a:{var c=h;a=a.split(".");for(var d=0;d<a.length-1;d++){var f=a[d];if(!(f in c))break a;c=c[f]}a=a[a.length-1];d=c[a];b=b(d);b!=d&&b!=null&&e(c,a,{configurable:!0,writable:!0,value:b})}}k("globalThis",function(a){return a||h});globalThis.sampleClosureBase=function(a){return a+1};
}catch(e){_._DumpException(e)}
/*_M:chunk_alpha*/
try{
_.beginModule("chunk_alpha");
globalThis.sampleClosureFirst=function(a){return globalThis.sampleClosureBase(a)*2};
_.endModule();
}catch(e){_._DumpException(e)}
/*_M:empty_one*/
/*_M:empty_two*/
/*_M:empty_three*/
/*_M:chunk_beta*/
try{
_.beginModule("chunk_beta");
globalThis.sampleClosureComponent=function(a){return globalThis.sampleClosureFirst(a)+3};
_.endModule();
}catch(e){_._DumpException(e)}
/*_M:empty_four*/
/*_M:empty_five*/
/*_M:chunk_final*/
try{
_.beginModule("chunk_final");
globalThis.sampleClosureLast=function(a){return globalThis.sampleClosureBase(a)-1};
_.endModule();
}catch(e){_._DumpException(e)}
}).call(this,this.default_ClosureProducer);
