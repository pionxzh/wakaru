import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-enum'

const inlineTest = defineInlineTest(transform)

inlineTest('Numeric enum',
  `
var Direction;
(function (Direction) {
  Direction[Direction["Up"] = 1] = "Up";
  Direction[Direction["Down"] = 2] = "Down";
  Direction[Direction["Left"] = 3] = "Left";
  Direction[Direction["Right"] = -4] = "Right";
})(Direction || (Direction = {}));
`,
  `
var Direction = {
  Up: 1,
  Down: 2,
  Left: 3,
  Right: -4,

  // reverse mapping
  1: "Up",

  2: "Down",
  3: "Left",
  [-4]: "Right"
};
`,
)

inlineTest('String enum',
  `
var Direction;
(function (Direction) {
  Direction["Up"] = "UP";
  Direction["Down"] = "DOWN";
  Direction.Left = "LEFT";
  Direction.Right = "RIGHT";
})(Direction || (Direction = {}));
`,
  `
var Direction = {
  Up: "UP",
  Down: "DOWN",
  Left: "LEFT",
  Right: "RIGHT"
};
`,
)

inlineTest('Heterogeneous enum',
  `
var Response;
(function (Response) {
  Response[Response["No"] = 0] = "No";
  Response["Yes"] = "YES";
})(Response || (Response = {}));
`,
  `
var Response = {
  No: 0,
  Yes: "YES",

  // reverse mapping
  0: "No"
};
`,
)

inlineTest('Enum with computed members',
  `
var FileAccess;
(function (FileAccess) {
  // constant members
  FileAccess[FileAccess["Read"] = 2] = "Read";
  FileAccess[FileAccess["Write"] = 4] = "Write";
  // computed member
  FileAccess[FileAccess["G"] = "123".length] = "G";
})(FileAccess || (FileAccess = {}));
`,
  `
var FileAccess = {
  // constant members
  Read: 2,

  Write: 4,

  // computed member
  G: "123".length,

  // reverse mapping
  2: "Read",

  4: "Write",
  ["123".length]: "G"
};
`,
)

inlineTest('Mangled enum',
  `
var Direction;
(function (i) {
  i[i["Up"] = 1] = "Up";
  i[i["Down"] = 2] = "Down";
  i[i["Left"] = 3] = "Left";
  i[i["Right"] = 4] = "Right";
})(Direction || (Direction = {}));
`,
  `
var Direction = {
  Up: 1,
  Down: 2,
  Left: 3,
  Right: 4,

  // reverse mapping
  1: "Up",

  2: "Down",
  3: "Left",
  4: "Right"
};
`,
)

inlineTest('Enum declaration merging',
  `
var Direction;
(function (Direction) {
  Direction[Direction["Up"] = -1] = "Up";
  Direction["Down"] = "DOWN";
})(Direction || (Direction = {}));
(function (Direction) {
  Direction["Left"] = "LEFT";
  Direction["Right"] = "RIGHT";
})(Direction || (Direction = {}));
`,
  `
var Direction = {
  Up: -1,
  Down: "DOWN",

  // reverse mapping
  [-1]: "Up"
};

Direction = {
  ...(Direction || (Direction = {})),
  Left: "LEFT",
  Right: "RIGHT"
};
`,
)
