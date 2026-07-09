/* _GlobalPrefix_ */
"use strict";/*_JS*/
this.default_SyntheticSuite=this.default_SyntheticSuite||{};(function(_){var window=this;
/*_M:base*/
try{
_._ModuleManager_initialize=function(graph,loading){_.fixtureGraph=[graph,loading]};
_._ModuleManager_initialize("base/chunk_final/chunk_alpha/empty_one/empty_two/empty_three/empty_four/empty_five/chunk_beta:2,3",["chunk_alpha","empty_one","empty_two","empty_three","empty_four","empty_five","chunk_beta","chunk_final"]);
_.fixtureBase={window:window,ready:true};
}catch(e){_._DumpException(e)}
/*_M:chunk_alpha*/
try{
_.beginModule("chunk_alpha");
_.fixtureFirst=_.fixtureBase.ready;
_.endModule();
}catch(e){_._DumpException(e)}
/*_M:empty_one*/
/*_M:empty_two*/
/*_M:empty_three*/
/*_M:chunk_beta*/
try{
_.beginModule("chunk_beta");
_.fixtureComponent={dependencies:[_.fixtureFirst]};
_.endModule();
}catch(e){_._DumpException(e)}
/*_M:empty_four*/
/*_M:empty_five*/
/*_M:chunk_final*/
try{
_.beginModule("chunk_final");
_.fixtureLast=true;
_.endModule();
}catch(e){_._DumpException(e)}
/* _GlobalSuffix_ */
}).call(this,this.default_SyntheticSuite);
