const { getGlobal, GlobalBox, globalState } = require('./global-user');

globalState.current = new GlobalBox(getGlobal());
console.log(globalState.current.value);
