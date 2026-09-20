var api = require("./variable-factory");
module.exports = { first: api(2), second: api(2), version: api.version };
