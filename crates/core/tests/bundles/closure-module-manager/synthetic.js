"use strict";
this.closureShared = this.closureShared || {};
(function(shared) {
  var window = this;

  /*_M:base*/
  try {
    shared.before("base");
    shared._ModuleManager_initialize(
      "base/feature:0/lazy:0,1",
      ["base", "feature", "lazy"]
    );
    shared.baseValue = 1;
    shared.after();
  } catch (error) {
    shared._DumpException(error);
  }

  /*_M:feature*/
  try {
    shared.before("feature");
    shared.featureValue = shared.baseValue + 1;
    shared.after();
  } catch (error) {
    shared._DumpException(error);
  }

  /*_M:lazy*/
  try {
    shared.before("lazy");
    shared.lazyValue = shared.featureValue + 1;
    shared.after();
  } catch (error) {
    shared._DumpException(error);
  }
}).call(this, this.closureShared);
