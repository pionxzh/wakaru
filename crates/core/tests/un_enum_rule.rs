mod common;

use common::assert_eq_normalized;
use swc_core::common::GLOBALS;
use swc_core::ecma::visit::VisitMutWith;
use wakaru_core::rules::UnEnum;

fn apply(input: &str) -> String {
    GLOBALS.set(&Default::default(), || {
        use swc_core::common::{sync::Lrc, FileName, SourceMap};
        use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
        use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};

        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            FileName::Custom("test.js".to_string()).into(),
            input.to_string(),
        );
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax {
                jsx: true,
                ..Default::default()
            }),
            Default::default(),
            StringInput::from(&*fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);
        let mut module = parser.parse_module().expect("parse failed");

        module.visit_mut_with(&mut UnEnum);

        let mut output = Vec::new();
        {
            let mut emitter = Emitter {
                cfg: Config::default().with_minify(false),
                cm: cm.clone(),
                comments: None,
                wr: JsWriter::new(cm, "\n", &mut output, None),
            };
            emitter.emit_module(&module).expect("emit failed");
        }
        String::from_utf8(output).expect("utf-8")
    })
}

#[test]
fn test_numeric_enum() {
    let input = r#"
var Direction;
(function (Direction) {
  Direction[Direction["Up"] = 1] = "Up";
  Direction[Direction["Down"] = 2] = "Down";
  Direction[Direction["Left"] = 3] = "Left";
  Direction[Direction["Right"] = -4] = "Right";
})(Direction || (Direction = {}));
"#;
    let expected = r#"
var Direction = {
  Up: 1,
  Down: 2,
  Left: 3,
  Right: -4,
  1: "Up",
  2: "Down",
  3: "Left",
  [-4]: "Right"
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_string_enum() {
    let input = r#"
var Direction;
(function (Direction) {
  Direction["Up"] = "UP";
  Direction["Down"] = "DOWN";
  Direction.Left = "LEFT";
  Direction.Right = "RIGHT";
})(Direction || (Direction = {}));
"#;
    let expected = r#"
var Direction = {
  Up: "UP",
  Down: "DOWN",
  Left: "LEFT",
  Right: "RIGHT"
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_mangled_enum() {
    let input = r#"
var Direction;
(function (i) {
  i[i["Up"] = 1] = "Up";
  i[i["Down"] = 2] = "Down";
})(Direction || (Direction = {}));
"#;
    let expected = r#"
var Direction = {
  Up: 1,
  Down: 2,
  1: "Up",
  2: "Down"
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_enum_invalid_identifier_keys() {
    let input = r#"
var RenderMode;
(function (RenderMode) {
  RenderMode[RenderMode["2D"] = 1] = "2D";
  RenderMode[RenderMode["WebGL"] = 2] = "WebGL";
})(RenderMode || (RenderMode = {}));
"#;
    let expected = r#"
var RenderMode = {
  "2D": 1,
  WebGL: 2,
  1: "2D",
  2: "WebGL"
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_noop_not_an_enum() {
    let input = r#"
var x = 1;
console.log(x);
"#;
    let expected = r#"
var x = 1;
console.log(x);
"#;
    assert_eq_normalized(&apply(input), expected);
}
