use std::collections::HashMap;

use anyhow::Result;
use swc_core::common::{sync::Lrc, SourceMap, SyntaxContext};
use swc_core::ecma::ast::{
    BinaryOp, CallExpr, Callee, Expr, ExprOrSpread, Function, Lit, Pat, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::emitter::{handler_expression, print_template_expression};
use super::roles::{IvyInstruction, IvyRoleTable};
use super::syntax::{binding_key, string_lit, BindingKey};

pub(super) struct RecoveredTemplate {
    pub(super) source: String,
    pub(super) unsupported_instructions: Vec<String>,
}

#[derive(Clone)]
struct InstructionCall {
    instruction: IvyInstruction,
    args: Vec<Box<Expr>>,
}

#[derive(Default)]
struct TemplateProgram {
    create: Vec<InstructionCall>,
    update: Vec<InstructionCall>,
    unsupported_instructions: Vec<String>,
}

#[derive(Clone)]
struct TemplateAttribute {
    name: String,
    value: Option<String>,
}

enum TemplateNodeKind {
    Element {
        tag: String,
        attributes: Vec<TemplateAttribute>,
    },
    Text {
        value: String,
    },
}

struct TemplateNode {
    kind: TemplateNodeKind,
    children: Vec<usize>,
}

#[derive(Default)]
struct TemplateTree {
    nodes: Vec<TemplateNode>,
    roots: Vec<usize>,
    stack: Vec<usize>,
    index_to_node: HashMap<usize, usize>,
    cursor: usize,
}

pub(super) fn recover_template(
    template: &Function,
    constant_table: Option<&Expr>,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
    cm: Lrc<SourceMap>,
) -> Result<RecoveredTemplate> {
    let Some(render_flags) = function_param_binding(template, 0) else {
        return Ok(RecoveredTemplate {
            source: "<!-- Unsupported Ivy template parameters -->".to_string(),
            unsupported_instructions: vec!["template-parameters".to_string()],
        });
    };
    let context = function_param_binding(template, 1);
    let constants = constant_table
        .map(decode_component_constant_table)
        .unwrap_or_default();
    let mut program = TemplateProgram::default();
    if let Some(body) = &template.body {
        collect_statements(
            &body.stmts,
            None,
            &render_flags,
            roles,
            unresolved_ctxt,
            &mut program,
        );
    }

    let mut tree = TemplateTree::default();
    for instruction in &program.create {
        apply_create_instruction(
            instruction,
            &constants,
            context.as_ref(),
            &mut tree,
            cm.clone(),
        )?;
    }
    for instruction in &program.update {
        apply_update_instruction(instruction, context.as_ref(), &mut tree, cm.clone())?;
    }

    let mut source = render_tree(&tree);
    for instruction in &program.unsupported_instructions {
        if !source.is_empty() {
            source.push('\n');
        }
        source.push_str("<!-- Unsupported Ivy instruction: ");
        source.push_str(instruction);
        source.push_str(" -->");
    }
    Ok(RecoveredTemplate {
        source: if source.is_empty() {
            "<!-- Empty Ivy template -->".to_string()
        } else {
            source
        },
        unsupported_instructions: program.unsupported_instructions,
    })
}

pub(super) fn ivy_template_score(
    template: &Function,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
) -> usize {
    struct Counter<'a> {
        roles: &'a IvyRoleTable,
        unresolved_ctxt: SyntaxContext,
        score: usize,
    }

    impl Visit for Counter<'_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            let score = call_chain(call)
                .and_then(|(root, _)| self.roles.instruction_for_expr(root, self.unresolved_ctxt))
                .map(|instruction| match instruction {
                    IvyInstruction::ElementStart
                    | IvyInstruction::Element
                    | IvyInstruction::Text => 3,
                    IvyInstruction::ElementEnd
                    | IvyInstruction::Listener
                    | IvyInstruction::Advance
                    | IvyInstruction::TextInterpolate
                    | IvyInstruction::TextInterpolate1
                    | IvyInstruction::TextInterpolate2
                    | IvyInstruction::TextInterpolate3
                    | IvyInstruction::TextInterpolate4
                    | IvyInstruction::TextInterpolate5
                    | IvyInstruction::TextInterpolate6
                    | IvyInstruction::TextInterpolate7
                    | IvyInstruction::TextInterpolate8
                    | IvyInstruction::Property
                    | IvyInstruction::Attribute
                    | IvyInstruction::ClassProp
                    | IvyInstruction::StyleProp => 1,
                    IvyInstruction::DefineComponent => 0,
                })
                .unwrap_or(0);
            self.score += score;
            call.visit_children_with(self);
        }
    }

    let mut counter = Counter {
        roles,
        unresolved_ctxt,
        score: 0,
    };
    template.visit_with(&mut counter);
    counter.score
}

fn function_param_binding(function: &Function, index: usize) -> Option<BindingKey> {
    let Pat::Ident(binding) = &function.params.get(index)?.pat else {
        return None;
    };
    Some(binding_key(&binding.id))
}

fn collect_statements(
    statements: &[Stmt],
    phase: Option<u8>,
    render_flags: &BindingKey,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
    program: &mut TemplateProgram,
) {
    for statement in statements {
        collect_statement(
            statement,
            phase,
            render_flags,
            roles,
            unresolved_ctxt,
            program,
        );
    }
}

fn collect_statement(
    statement: &Stmt,
    phase: Option<u8>,
    render_flags: &BindingKey,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
    program: &mut TemplateProgram,
) {
    match statement {
        Stmt::Block(block) => collect_statements(
            &block.stmts,
            phase,
            render_flags,
            roles,
            unresolved_ctxt,
            program,
        ),
        Stmt::If(if_statement) => {
            let branch_phase = render_flag_mask(if_statement.test.as_ref(), render_flags).or(phase);
            collect_statement(
                if_statement.cons.as_ref(),
                branch_phase,
                render_flags,
                roles,
                unresolved_ctxt,
                program,
            );
            if let Some(alternate) = &if_statement.alt {
                collect_statement(
                    alternate.as_ref(),
                    phase,
                    render_flags,
                    roles,
                    unresolved_ctxt,
                    program,
                );
            }
        }
        Stmt::Expr(expression) => collect_expression(
            expression.expr.as_ref(),
            phase,
            render_flags,
            roles,
            unresolved_ctxt,
            program,
        ),
        _ => {}
    }
}

fn collect_expression(
    expression: &Expr,
    phase: Option<u8>,
    render_flags: &BindingKey,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
    program: &mut TemplateProgram,
) {
    match expression {
        Expr::Paren(paren) => collect_expression(
            paren.expr.as_ref(),
            phase,
            render_flags,
            roles,
            unresolved_ctxt,
            program,
        ),
        Expr::Seq(sequence) => {
            for expression in &sequence.exprs {
                collect_expression(
                    expression.as_ref(),
                    phase,
                    render_flags,
                    roles,
                    unresolved_ctxt,
                    program,
                );
            }
        }
        Expr::Bin(binary) if binary.op == BinaryOp::LogicalAnd => {
            let branch_phase = render_flag_mask(binary.left.as_ref(), render_flags).or(phase);
            collect_expression(
                binary.right.as_ref(),
                branch_phase,
                render_flags,
                roles,
                unresolved_ctxt,
                program,
            );
        }
        Expr::Call(call) => {
            let Some((root, argument_lists)) = call_chain(call) else {
                return;
            };
            let Some(instruction) = roles.instruction_for_expr(root, unresolved_ctxt) else {
                if let Some(name) = roles.ivy_name_for_expr(root, unresolved_ctxt) {
                    if !program.unsupported_instructions.contains(&name) {
                        program.unsupported_instructions.push(name);
                    }
                }
                return;
            };
            let target = match phase {
                Some(1) => &mut program.create,
                Some(2) => &mut program.update,
                _ => return,
            };
            target.extend(argument_lists.into_iter().map(|args| InstructionCall {
                instruction,
                args: args.iter().map(|arg| arg.expr.clone()).collect(),
            }));
        }
        _ => {}
    }
}

fn render_flag_mask(expression: &Expr, render_flags: &BindingKey) -> Option<u8> {
    let Expr::Bin(binary) = expression else {
        return None;
    };
    if binary.op != BinaryOp::BitAnd {
        return None;
    }
    let (Expr::Ident(ident), Expr::Lit(Lit::Num(mask))) =
        (binary.left.as_ref(), binary.right.as_ref())
    else {
        return None;
    };
    (binding_key(ident) == *render_flags && (mask.value == 1.0 || mask.value == 2.0))
        .then_some(mask.value as u8)
}

fn call_chain(call: &CallExpr) -> Option<(&Expr, Vec<&[ExprOrSpread]>)> {
    let mut argument_lists = vec![call.args.as_slice()];
    let mut callee = &call.callee;
    loop {
        let Callee::Expr(expression) = callee else {
            return None;
        };
        match expression.as_ref() {
            Expr::Call(inner) => {
                argument_lists.push(inner.args.as_slice());
                callee = &inner.callee;
            }
            root => {
                argument_lists.reverse();
                return Some((root, argument_lists));
            }
        }
    }
}

fn apply_create_instruction(
    call: &InstructionCall,
    constants: &[Vec<TemplateAttribute>],
    context: Option<&BindingKey>,
    tree: &mut TemplateTree,
    cm: Lrc<SourceMap>,
) -> Result<()> {
    match call.instruction {
        IvyInstruction::ElementStart => {
            let Some(index) = numeric_arg(&call.args, 0) else {
                return Ok(());
            };
            let Some(tag) = call.args.get(1).and_then(|arg| string_lit(arg.as_ref())) else {
                return Ok(());
            };
            let attributes = numeric_arg(&call.args, 2)
                .and_then(|index| constants.get(index).cloned())
                .unwrap_or_default();
            let node = tree.push_node(index, TemplateNodeKind::Element { tag, attributes });
            tree.stack.push(node);
        }
        IvyInstruction::Element => {
            let Some(index) = numeric_arg(&call.args, 0) else {
                return Ok(());
            };
            let Some(tag) = call.args.get(1).and_then(|arg| string_lit(arg.as_ref())) else {
                return Ok(());
            };
            let attributes = numeric_arg(&call.args, 2)
                .and_then(|index| constants.get(index).cloned())
                .unwrap_or_default();
            tree.push_node(index, TemplateNodeKind::Element { tag, attributes });
        }
        IvyInstruction::ElementEnd => {
            tree.stack.pop();
        }
        IvyInstruction::Text => {
            let Some(index) = numeric_arg(&call.args, 0) else {
                return Ok(());
            };
            let value = call
                .args
                .get(1)
                .and_then(|arg| string_lit(arg.as_ref()))
                .unwrap_or_default();
            tree.push_node(index, TemplateNodeKind::Text { value });
        }
        IvyInstruction::Listener => {
            let Some(node) = tree.stack.last().copied() else {
                return Ok(());
            };
            let Some(event) = call.args.first().and_then(|arg| string_lit(arg.as_ref())) else {
                return Ok(());
            };
            let Some(handler) = call.args.get(1) else {
                return Ok(());
            };
            let expression = handler_expression(handler.as_ref(), context, cm)?;
            tree.add_attribute(
                node,
                TemplateAttribute {
                    name: format!("({event})"),
                    value: Some(expression),
                },
            );
        }
        _ => {}
    }
    Ok(())
}

fn apply_update_instruction(
    call: &InstructionCall,
    context: Option<&BindingKey>,
    tree: &mut TemplateTree,
    cm: Lrc<SourceMap>,
) -> Result<()> {
    match call.instruction {
        IvyInstruction::Advance => {
            let amount = numeric_arg(&call.args, 0).unwrap_or(1);
            tree.cursor = tree.cursor.saturating_add(amount);
        }
        IvyInstruction::TextInterpolate
        | IvyInstruction::TextInterpolate1
        | IvyInstruction::TextInterpolate2
        | IvyInstruction::TextInterpolate3
        | IvyInstruction::TextInterpolate4
        | IvyInstruction::TextInterpolate5
        | IvyInstruction::TextInterpolate6
        | IvyInstruction::TextInterpolate7
        | IvyInstruction::TextInterpolate8 => {
            let value = interpolation_value(call, context, cm)?;
            if let Some(&node) = tree.index_to_node.get(&tree.cursor) {
                if let TemplateNodeKind::Text { value: current } = &mut tree.nodes[node].kind {
                    *current = value;
                }
            }
        }
        IvyInstruction::Property
        | IvyInstruction::Attribute
        | IvyInstruction::ClassProp
        | IvyInstruction::StyleProp => {
            let Some(&node) = tree.index_to_node.get(&tree.cursor) else {
                return Ok(());
            };
            let Some(name) = call.args.first().and_then(|arg| string_lit(arg.as_ref())) else {
                return Ok(());
            };
            let Some(value) = call.args.get(1) else {
                return Ok(());
            };
            let expression = print_template_expression(value.as_ref(), context, cm)?;
            let prefix = match call.instruction {
                IvyInstruction::Property => "",
                IvyInstruction::Attribute => "attr.",
                IvyInstruction::ClassProp => "class.",
                IvyInstruction::StyleProp => "style.",
                _ => unreachable!(),
            };
            tree.add_attribute(
                node,
                TemplateAttribute {
                    name: format!("[{prefix}{name}]"),
                    value: Some(expression),
                },
            );
        }
        _ => {}
    }
    Ok(())
}

fn interpolation_value(
    call: &InstructionCall,
    context: Option<&BindingKey>,
    cm: Lrc<SourceMap>,
) -> Result<String> {
    if call.instruction == IvyInstruction::TextInterpolate {
        let expression = call
            .args
            .first()
            .map(|expr| print_template_expression(expr.as_ref(), context, cm))
            .transpose()?
            .unwrap_or_default();
        return Ok(format!("{{{{ {expression} }}}}"));
    }

    let mut output = String::new();
    for (index, argument) in call.args.iter().enumerate() {
        if index % 2 == 0 {
            output.push_str(&string_lit(argument.as_ref()).unwrap_or_default());
        } else {
            let expression = print_template_expression(argument.as_ref(), context, cm.clone())?;
            output.push_str("{{ ");
            output.push_str(&expression);
            output.push_str(" }}");
        }
    }
    Ok(output)
}

fn numeric_arg(args: &[Box<Expr>], index: usize) -> Option<usize> {
    let Expr::Lit(Lit::Num(number)) = args.get(index)?.as_ref() else {
        return None;
    };
    (number.value >= 0.0 && number.value.fract() == 0.0).then_some(number.value as usize)
}

fn decode_component_constant_table(constants: &Expr) -> Vec<Vec<TemplateAttribute>> {
    let Expr::Array(table) = constants else {
        return Vec::new();
    };
    table
        .elems
        .iter()
        .map(|entry| {
            let Some(entry) = entry else {
                return Vec::new();
            };
            decode_constant_attributes(entry.expr.as_ref())
        })
        .collect()
}

fn decode_constant_attributes(expression: &Expr) -> Vec<TemplateAttribute> {
    let Expr::Array(array) = expression else {
        return Vec::new();
    };
    let values = array
        .elems
        .iter()
        .filter_map(|element| element.as_ref().map(|element| element.expr.as_ref()))
        .collect::<Vec<_>>();
    let mut attributes = Vec::new();
    let mut classes = Vec::new();
    let mut styles = Vec::new();
    let mut index = 0;
    let mut marker = 0usize;
    while index < values.len() {
        if let Expr::Lit(Lit::Num(number)) = values[index] {
            marker = number.value as usize;
            index += 1;
            continue;
        }
        let Some(name) = string_lit(values[index]) else {
            index += 1;
            continue;
        };
        match marker {
            0 => {
                let value = values.get(index + 1).and_then(|value| string_lit(value));
                attributes.push(TemplateAttribute { name, value });
                index += 2;
            }
            1 => {
                classes.push(name);
                index += 1;
            }
            2 => {
                let value = values
                    .get(index + 1)
                    .and_then(|value| string_lit(value))
                    .unwrap_or_default();
                styles.push(format!("{name}: {value}"));
                index += 2;
            }
            _ => {
                // Binding/template markers name non-static attributes.
                index += 1;
            }
        }
    }
    if !classes.is_empty() {
        attributes.push(TemplateAttribute {
            name: "class".to_string(),
            value: Some(classes.join(" ")),
        });
    }
    if !styles.is_empty() {
        attributes.push(TemplateAttribute {
            name: "style".to_string(),
            value: Some(styles.join("; ")),
        });
    }
    attributes
}

impl TemplateTree {
    fn push_node(&mut self, index: usize, kind: TemplateNodeKind) -> usize {
        let node = self.nodes.len();
        self.nodes.push(TemplateNode {
            kind,
            children: Vec::new(),
        });
        self.index_to_node.insert(index, node);
        if let Some(parent) = self.stack.last().copied() {
            self.nodes[parent].children.push(node);
        } else {
            self.roots.push(node);
        }
        node
    }

    fn add_attribute(&mut self, node: usize, attribute: TemplateAttribute) {
        let TemplateNodeKind::Element { attributes, .. } = &mut self.nodes[node].kind else {
            return;
        };
        attributes.push(attribute);
    }
}

fn render_tree(tree: &TemplateTree) -> String {
    tree.roots
        .iter()
        .map(|&node| render_node(tree, node, 0))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_node(tree: &TemplateTree, node: usize, depth: usize) -> String {
    let current = &tree.nodes[node];
    let indent = "  ".repeat(depth);
    match &current.kind {
        TemplateNodeKind::Text { value } => format!("{indent}{}", escape_text(value)),
        TemplateNodeKind::Element { tag, attributes } => {
            let attributes = attributes
                .iter()
                .map(render_attribute)
                .collect::<Vec<_>>()
                .join("");
            if is_void_element(tag) {
                return format!("{indent}<{tag}{attributes} />");
            }
            if current.children.is_empty() {
                return format!("{indent}<{tag}{attributes}></{tag}>");
            }
            if current.children.len() == 1 {
                let child = &tree.nodes[current.children[0]];
                if let TemplateNodeKind::Text { value } = &child.kind {
                    if !value.contains('\n') {
                        return format!(
                            "{indent}<{tag}{attributes}>{}</{tag}>",
                            escape_text(value)
                        );
                    }
                }
            }
            let children = current
                .children
                .iter()
                .map(|&child| render_node(tree, child, depth + 1))
                .collect::<Vec<_>>()
                .join("\n");
            format!("{indent}<{tag}{attributes}>\n{children}\n{indent}</{tag}>")
        }
    }
}

fn render_attribute(attribute: &TemplateAttribute) -> String {
    match &attribute.value {
        Some(value) if !value.is_empty() => {
            format!(" {}=\"{}\"", attribute.name, escape_attribute(value))
        }
        _ => format!(" {}", attribute.name),
    }
}

fn escape_text(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;")
}

fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

fn is_void_element(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}
