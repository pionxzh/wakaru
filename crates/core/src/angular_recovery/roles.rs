use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Spanned, SyntaxContext};
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, BlockStmtOrExpr, CallExpr, Callee, ClassProp, Expr, ExprOrSpread,
    ImportSpecifier, Module, ModuleDecl, ModuleExportName, ModuleItem, Prop, PropName,
    SimpleAssignTarget, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::syntax::{binding_key, member_prop_name, wtf8_to_string, BindingKey};
use super::workspace::{WorkspaceSymbol, WorkspaceSymbolAlias};
use super::PreparedAngularModule;
use crate::facts::ModuleFactsMap;

mod structural;

const REFERENCE_CANDIDATE_NAME: &str = "__wakaruIvyReferenceCandidate";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum AngularClassApi {
    ContentChild,
    ContentChildren,
    Computed,
    Inject,
    Input,
    Model,
    Output,
    Signal,
    ViewChild,
    ViewChildren,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum AngularListenerTarget {
    Window,
    Document,
    Body,
}

impl AngularListenerTarget {
    fn from_export_name(name: &str) -> Option<Self> {
        Some(match name {
            "ɵɵresolveWindow" => Self::Window,
            "ɵɵresolveDocument" => Self::Document,
            "ɵɵresolveBody" => Self::Body,
            _ => return None,
        })
    }

    pub(super) fn template_name(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Document => "document",
            Self::Body => "body",
        }
    }
}

impl AngularClassApi {
    fn from_export_name(name: &str) -> Option<Self> {
        Some(match name {
            "contentChild" => Self::ContentChild,
            "contentChildren" => Self::ContentChildren,
            "computed" => Self::Computed,
            "inject" => Self::Inject,
            "input" => Self::Input,
            "model" => Self::Model,
            "output" => Self::Output,
            "signal" => Self::Signal,
            "viewChild" => Self::ViewChild,
            "viewChildren" => Self::ViewChildren,
            _ => return None,
        })
    }

    pub(super) fn canonical_export_name(self) -> &'static str {
        match self {
            Self::ContentChild => "contentChild",
            Self::ContentChildren => "contentChildren",
            Self::Computed => "computed",
            Self::Inject => "inject",
            Self::Input => "input",
            Self::Model => "model",
            Self::Output => "output",
            Self::Signal => "signal",
            Self::ViewChild => "viewChild",
            Self::ViewChildren => "viewChildren",
        }
    }

    pub(super) fn is_query(self) -> bool {
        matches!(
            self,
            Self::ContentChild | Self::ContentChildren | Self::ViewChild | Self::ViewChildren
        )
    }

    fn query_initializer(self) -> Option<AngularQueryInitializer> {
        Some(match self {
            Self::ContentChild => AngularQueryInitializer {
                owner: Some(AngularQueryOwner::Content),
                multiple: false,
                required: false,
            },
            Self::ContentChildren => AngularQueryInitializer {
                owner: Some(AngularQueryOwner::Content),
                multiple: true,
                required: false,
            },
            Self::ViewChild => AngularQueryInitializer {
                owner: Some(AngularQueryOwner::View),
                multiple: false,
                required: false,
            },
            Self::ViewChildren => AngularQueryInitializer {
                owner: Some(AngularQueryOwner::View),
                multiple: true,
                required: false,
            },
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum AngularQueryOwner {
    View,
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AngularQueryInitializer {
    pub(super) owner: Option<AngularQueryOwner>,
    pub(super) multiple: bool,
    pub(super) required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum QueryInitializerRole {
    DynamicFactory,
    Fixed { multiple: bool, required: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum IvyInstruction {
    DefineComponent,
    ElementStart,
    ElementEnd,
    Element,
    ElementContainerStart,
    ElementContainerEnd,
    ElementContainer,
    NamespaceHtml,
    NamespaceSvg,
    NamespaceMathMl,
    Text,
    Listener,
    AnimateEnter,
    AnimateEnterListener,
    AnimateLeave,
    AnimateLeaveListener,
    TwoWayProperty,
    TwoWayListener,
    TwoWayBindingSet,
    Template,
    Defer,
    DeferOnIdle,
    Conditional,
    RepeaterCreate,
    Repeater,
    RepeaterTrackByIndex,
    RepeaterTrackByIdentity,
    NextContext,
    GetCurrentView,
    RestoreView,
    ResetView,
    ProjectionDef,
    Projection,
    Reference,
    DeclareLet,
    StoreLet,
    ReadContextLet,
    Pipe,
    PipeBind1,
    PipeBind2,
    PipeBind3,
    PipeBind4,
    PipeBindV,
    PureFunction0,
    PureFunction1,
    PureFunction2,
    PureFunction3,
    PureFunction4,
    PureFunction5,
    PureFunction6,
    PureFunction7,
    PureFunction8,
    PureFunctionV,
    I18n,
    I18nStart,
    I18nEnd,
    I18nExp,
    I18nApply,
    Advance,
    Interpolate,
    Interpolate1,
    Interpolate2,
    Interpolate3,
    Interpolate4,
    Interpolate5,
    Interpolate6,
    Interpolate7,
    Interpolate8,
    InterpolateV,
    TextInterpolate,
    TextInterpolate1,
    TextInterpolate2,
    TextInterpolate3,
    TextInterpolate4,
    TextInterpolate5,
    TextInterpolate6,
    TextInterpolate7,
    TextInterpolate8,
    PropertyInterpolate,
    Property,
    AriaProperty,
    Attribute,
    ClassMap,
    ClassProp,
    StyleMap,
    StyleProp,
    ViewQuerySignal,
    ContentQuerySignal,
}

impl IvyInstruction {
    fn from_export_name(name: &str) -> Option<Self> {
        Some(match name {
            "ɵɵdefineComponent" => Self::DefineComponent,
            "ɵɵelementStart" | "ɵɵdomElementStart" => Self::ElementStart,
            "ɵɵelementEnd" | "ɵɵdomElementEnd" => Self::ElementEnd,
            "ɵɵelement" | "ɵɵdomElement" => Self::Element,
            "ɵɵelementContainerStart" | "ɵɵdomElementContainerStart" => {
                Self::ElementContainerStart
            }
            "ɵɵelementContainerEnd" | "ɵɵdomElementContainerEnd" => Self::ElementContainerEnd,
            "ɵɵelementContainer" | "ɵɵdomElementContainer" => Self::ElementContainer,
            "ɵɵnamespaceHTML" => Self::NamespaceHtml,
            "ɵɵnamespaceSVG" => Self::NamespaceSvg,
            "ɵɵnamespaceMathML" => Self::NamespaceMathMl,
            "ɵɵtext" => Self::Text,
            "ɵɵlistener" | "ɵɵdomListener" => Self::Listener,
            "ɵɵanimateEnter" => Self::AnimateEnter,
            "ɵɵanimateEnterListener" => Self::AnimateEnterListener,
            "ɵɵanimateLeave" => Self::AnimateLeave,
            "ɵɵanimateLeaveListener" => Self::AnimateLeaveListener,
            "ɵɵtwoWayProperty" => Self::TwoWayProperty,
            "ɵɵtwoWayListener" => Self::TwoWayListener,
            "ɵɵtwoWayBindingSet" => Self::TwoWayBindingSet,
            "ɵɵtemplate"
            | "ɵɵdomTemplate"
            | "ɵɵconditionalCreate"
            | "ɵɵconditionalBranchCreate" => Self::Template,
            "ɵɵdefer" => Self::Defer,
            "ɵɵdeferOnIdle" => Self::DeferOnIdle,
            "ɵɵconditional" => Self::Conditional,
            "ɵɵrepeaterCreate" => Self::RepeaterCreate,
            "ɵɵrepeater" => Self::Repeater,
            "ɵɵrepeaterTrackByIndex" => Self::RepeaterTrackByIndex,
            "ɵɵrepeaterTrackByIdentity" => Self::RepeaterTrackByIdentity,
            "ɵɵnextContext" => Self::NextContext,
            "ɵɵgetCurrentView" => Self::GetCurrentView,
            "ɵɵrestoreView" => Self::RestoreView,
            "ɵɵresetView" => Self::ResetView,
            "ɵɵprojectionDef" => Self::ProjectionDef,
            "ɵɵprojection" => Self::Projection,
            "ɵɵreference" => Self::Reference,
            "ɵɵdeclareLet" => Self::DeclareLet,
            "ɵɵstoreLet" => Self::StoreLet,
            "ɵɵreadContextLet" => Self::ReadContextLet,
            "ɵɵpipe" => Self::Pipe,
            "ɵɵpipeBind1" => Self::PipeBind1,
            "ɵɵpipeBind2" => Self::PipeBind2,
            "ɵɵpipeBind3" => Self::PipeBind3,
            "ɵɵpipeBind4" => Self::PipeBind4,
            "ɵɵpipeBindV" => Self::PipeBindV,
            "ɵɵpureFunction0" => Self::PureFunction0,
            "ɵɵpureFunction1" => Self::PureFunction1,
            "ɵɵpureFunction2" => Self::PureFunction2,
            "ɵɵpureFunction3" => Self::PureFunction3,
            "ɵɵpureFunction4" => Self::PureFunction4,
            "ɵɵpureFunction5" => Self::PureFunction5,
            "ɵɵpureFunction6" => Self::PureFunction6,
            "ɵɵpureFunction7" => Self::PureFunction7,
            "ɵɵpureFunction8" => Self::PureFunction8,
            "ɵɵpureFunctionV" => Self::PureFunctionV,
            "ɵɵi18n" => Self::I18n,
            "ɵɵi18nStart" => Self::I18nStart,
            "ɵɵi18nEnd" => Self::I18nEnd,
            "ɵɵi18nExp" => Self::I18nExp,
            "ɵɵi18nApply" => Self::I18nApply,
            "ɵɵadvance" => Self::Advance,
            "ɵɵinterpolate" => Self::Interpolate,
            "ɵɵinterpolate1" => Self::Interpolate1,
            "ɵɵinterpolate2" => Self::Interpolate2,
            "ɵɵinterpolate3" => Self::Interpolate3,
            "ɵɵinterpolate4" => Self::Interpolate4,
            "ɵɵinterpolate5" => Self::Interpolate5,
            "ɵɵinterpolate6" => Self::Interpolate6,
            "ɵɵinterpolate7" => Self::Interpolate7,
            "ɵɵinterpolate8" => Self::Interpolate8,
            "ɵɵinterpolateV" => Self::InterpolateV,
            "ɵɵtextInterpolate" => Self::TextInterpolate,
            "ɵɵtextInterpolate1" => Self::TextInterpolate1,
            "ɵɵtextInterpolate2" => Self::TextInterpolate2,
            "ɵɵtextInterpolate3" => Self::TextInterpolate3,
            "ɵɵtextInterpolate4" => Self::TextInterpolate4,
            "ɵɵtextInterpolate5" => Self::TextInterpolate5,
            "ɵɵtextInterpolate6" => Self::TextInterpolate6,
            "ɵɵtextInterpolate7" => Self::TextInterpolate7,
            "ɵɵtextInterpolate8" => Self::TextInterpolate8,
            "ɵɵpropertyInterpolate" => Self::PropertyInterpolate,
            "ɵɵproperty" | "ɵɵdomProperty" => Self::Property,
            "ɵɵariaProperty" => Self::AriaProperty,
            "ɵɵattribute" => Self::Attribute,
            "ɵɵclassMap" => Self::ClassMap,
            "ɵɵclassProp" => Self::ClassProp,
            "ɵɵstyleMap" => Self::StyleMap,
            "ɵɵstyleProp" => Self::StyleProp,
            "ɵɵviewQuerySignal" => Self::ViewQuerySignal,
            "ɵɵcontentQuerySignal" => Self::ContentQuerySignal,
            _ => return None,
        })
    }

    pub(super) fn canonical_export_name(self) -> &'static str {
        match self {
            Self::DefineComponent => "ɵɵdefineComponent",
            Self::ElementStart => "ɵɵelementStart",
            Self::ElementEnd => "ɵɵelementEnd",
            Self::Element => "ɵɵelement",
            Self::ElementContainerStart => "ɵɵelementContainerStart",
            Self::ElementContainerEnd => "ɵɵelementContainerEnd",
            Self::ElementContainer => "ɵɵelementContainer",
            Self::NamespaceHtml => "ɵɵnamespaceHTML",
            Self::NamespaceSvg => "ɵɵnamespaceSVG",
            Self::NamespaceMathMl => "ɵɵnamespaceMathML",
            Self::Text => "ɵɵtext",
            Self::Listener => "ɵɵlistener",
            Self::AnimateEnter => "ɵɵanimateEnter",
            Self::AnimateEnterListener => "ɵɵanimateEnterListener",
            Self::AnimateLeave => "ɵɵanimateLeave",
            Self::AnimateLeaveListener => "ɵɵanimateLeaveListener",
            Self::TwoWayProperty => "ɵɵtwoWayProperty",
            Self::TwoWayListener => "ɵɵtwoWayListener",
            Self::TwoWayBindingSet => "ɵɵtwoWayBindingSet",
            Self::Template => "ɵɵtemplate",
            Self::Defer => "ɵɵdefer",
            Self::DeferOnIdle => "ɵɵdeferOnIdle",
            Self::Conditional => "ɵɵconditional",
            Self::RepeaterCreate => "ɵɵrepeaterCreate",
            Self::Repeater => "ɵɵrepeater",
            Self::RepeaterTrackByIndex => "ɵɵrepeaterTrackByIndex",
            Self::RepeaterTrackByIdentity => "ɵɵrepeaterTrackByIdentity",
            Self::NextContext => "ɵɵnextContext",
            Self::GetCurrentView => "ɵɵgetCurrentView",
            Self::RestoreView => "ɵɵrestoreView",
            Self::ResetView => "ɵɵresetView",
            Self::ProjectionDef => "ɵɵprojectionDef",
            Self::Projection => "ɵɵprojection",
            Self::Reference => "ɵɵreference",
            Self::DeclareLet => "ɵɵdeclareLet",
            Self::StoreLet => "ɵɵstoreLet",
            Self::ReadContextLet => "ɵɵreadContextLet",
            Self::Pipe => "ɵɵpipe",
            Self::PipeBind1 => "ɵɵpipeBind1",
            Self::PipeBind2 => "ɵɵpipeBind2",
            Self::PipeBind3 => "ɵɵpipeBind3",
            Self::PipeBind4 => "ɵɵpipeBind4",
            Self::PipeBindV => "ɵɵpipeBindV",
            Self::PureFunction0 => "ɵɵpureFunction0",
            Self::PureFunction1 => "ɵɵpureFunction1",
            Self::PureFunction2 => "ɵɵpureFunction2",
            Self::PureFunction3 => "ɵɵpureFunction3",
            Self::PureFunction4 => "ɵɵpureFunction4",
            Self::PureFunction5 => "ɵɵpureFunction5",
            Self::PureFunction6 => "ɵɵpureFunction6",
            Self::PureFunction7 => "ɵɵpureFunction7",
            Self::PureFunction8 => "ɵɵpureFunction8",
            Self::PureFunctionV => "ɵɵpureFunctionV",
            Self::I18n => "ɵɵi18n",
            Self::I18nStart => "ɵɵi18nStart",
            Self::I18nEnd => "ɵɵi18nEnd",
            Self::I18nExp => "ɵɵi18nExp",
            Self::I18nApply => "ɵɵi18nApply",
            Self::Advance => "ɵɵadvance",
            Self::Interpolate => "ɵɵinterpolate",
            Self::Interpolate1 => "ɵɵinterpolate1",
            Self::Interpolate2 => "ɵɵinterpolate2",
            Self::Interpolate3 => "ɵɵinterpolate3",
            Self::Interpolate4 => "ɵɵinterpolate4",
            Self::Interpolate5 => "ɵɵinterpolate5",
            Self::Interpolate6 => "ɵɵinterpolate6",
            Self::Interpolate7 => "ɵɵinterpolate7",
            Self::Interpolate8 => "ɵɵinterpolate8",
            Self::InterpolateV => "ɵɵinterpolateV",
            Self::TextInterpolate => "ɵɵtextInterpolate",
            Self::TextInterpolate1 => "ɵɵtextInterpolate1",
            Self::TextInterpolate2 => "ɵɵtextInterpolate2",
            Self::TextInterpolate3 => "ɵɵtextInterpolate3",
            Self::TextInterpolate4 => "ɵɵtextInterpolate4",
            Self::TextInterpolate5 => "ɵɵtextInterpolate5",
            Self::TextInterpolate6 => "ɵɵtextInterpolate6",
            Self::TextInterpolate7 => "ɵɵtextInterpolate7",
            Self::TextInterpolate8 => "ɵɵtextInterpolate8",
            Self::PropertyInterpolate => "ɵɵpropertyInterpolate",
            Self::Property => "ɵɵproperty",
            Self::AriaProperty => "ɵɵariaProperty",
            Self::Attribute => "ɵɵattribute",
            Self::ClassMap => "ɵɵclassMap",
            Self::ClassProp => "ɵɵclassProp",
            Self::StyleMap => "ɵɵstyleMap",
            Self::StyleProp => "ɵɵstyleProp",
            Self::ViewQuerySignal => "ɵɵviewQuerySignal",
            Self::ContentQuerySignal => "ɵɵcontentQuerySignal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum SymbolIdentity {
    LocalBinding(BindingKey),
    LocalMember { object: BindingKey, property: Atom },
    GlobalBinding(Atom),
    GlobalMember { object: Atom, property: Atom },
}

#[derive(Default)]
pub(super) struct IvyRoleTable {
    ivy_names: HashMap<SymbolIdentity, String>,
    ambiguous_symbols: HashSet<SymbolIdentity>,
    class_api_call_arguments: HashMap<SymbolIdentity, Vec<Box<Expr>>>,
    query_initializer_roles: HashMap<SymbolIdentity, QueryInitializerRole>,
    core_namespaces: HashSet<BindingKey>,
    namespace_state_targets: HashSet<SymbolIdentity>,
    alias_groups: Vec<Vec<SymbolIdentity>>,
    alias_group_by_symbol: HashMap<SymbolIdentity, usize>,
    class_api_argument_alias_groups: HashSet<usize>,
}

impl IvyRoleTable {
    pub(super) fn collect(
        modules: &[PreparedAngularModule],
        module_facts: Option<&ModuleFactsMap>,
    ) -> Self {
        let mut table = Self::default();
        for prepared in modules {
            table.collect_imports(&prepared.module);
        }
        let structural_evidence = structural::StructuralRoleEvidence::collect(modules);
        for (module_index, prepared) in modules.iter().enumerate() {
            table.collect_export_maps(
                &prepared.module,
                prepared.unresolved_ctxt,
                module_index,
                &structural_evidence,
            );
        }
        for (identity, name) in structural_evidence.infer_ivy_roles() {
            table.record_mapping(identity, name.to_string());
        }
        for (identity, name) in structural_evidence.infer_listener_target_roles() {
            table.record_mapping(identity, name.to_string());
        }
        for (identity, name) in structural_evidence.infer_class_api_roles() {
            table.record_mapping(identity, name.to_string());
        }
        for (identity, arguments) in structural_evidence.specialized_class_api_call_arguments() {
            table.record_class_api_call_arguments(identity, arguments);
        }
        for (identity, role) in structural_evidence.infer_query_initializer_roles() {
            table.record_query_initializer_role(identity, role);
        }
        let mut aliases = super::workspace::collect_esm_symbol_aliases(modules);
        if let Some(module_facts) = module_facts {
            aliases.extend(super::workspace::collect_fact_symbol_aliases(
                modules,
                module_facts,
            ));
        }
        table.install_aliases(&aliases);
        table.propagate_aliases();
        for (identity, name) in structural_evidence.infer_template_roles(modules, &table) {
            table.record_mapping(identity, name.to_string());
        }
        table.propagate_aliases();
        table.namespace_state_targets =
            structural_evidence.inferred_namespace_state_targets(&table);
        table
    }

    fn collect_imports(&mut self, module: &Module) {
        for item in &module.body {
            let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
                continue;
            };
            if wtf8_to_string(&import.src.value) != "@angular/core" {
                continue;
            }
            for specifier in &import.specifiers {
                match specifier {
                    ImportSpecifier::Namespace(namespace) => {
                        self.core_namespaces.insert(binding_key(&namespace.local));
                    }
                    ImportSpecifier::Named(named) => {
                        let imported = named
                            .imported
                            .as_ref()
                            .map(module_export_name)
                            .unwrap_or_else(|| named.local.sym.to_string());
                        if imported.starts_with("ɵɵ")
                            || AngularClassApi::from_export_name(&imported).is_some()
                        {
                            self.record_mapping(
                                SymbolIdentity::LocalBinding(binding_key(&named.local)),
                                imported,
                            );
                        }
                    }
                    ImportSpecifier::Default(_) => {}
                }
            }
        }
    }

    fn collect_export_maps(
        &mut self,
        module: &Module,
        unresolved_ctxt: SyntaxContext,
        module_index: usize,
        structural_evidence: &structural::StructuralRoleEvidence,
    ) {
        let mut collector = IvyExportMapCollector {
            unresolved_ctxt,
            module_index,
            mappings: Vec::new(),
        };
        module.visit_with(&mut collector);
        for (identity, name, mapping_module, position) in collector.mappings {
            if structural_evidence.is_stable_export_reference(&identity, mapping_module, position) {
                self.record_mapping(identity, name);
            }
        }
    }

    fn record_mapping(&mut self, identity: SymbolIdentity, name: String) {
        let name = IvyInstruction::from_export_name(&name)
            .map(|instruction| instruction.canonical_export_name().to_string())
            .unwrap_or(name);
        if self.ambiguous_symbols.contains(&identity) {
            return;
        }
        if self
            .ivy_names
            .get(&identity)
            .is_some_and(|existing| existing != &name)
        {
            self.ivy_names.remove(&identity);
            self.class_api_call_arguments.remove(&identity);
            self.query_initializer_roles.remove(&identity);
            self.ambiguous_symbols.insert(identity);
            return;
        }
        self.ivy_names.insert(identity, name);
    }

    fn record_query_initializer_role(
        &mut self,
        identity: SymbolIdentity,
        role: QueryInitializerRole,
    ) {
        if self.ambiguous_symbols.contains(&identity) {
            return;
        }
        if self
            .query_initializer_roles
            .get(&identity)
            .is_some_and(|existing| *existing != role)
        {
            self.ivy_names.remove(&identity);
            self.class_api_call_arguments.remove(&identity);
            self.query_initializer_roles.remove(&identity);
            self.ambiguous_symbols.insert(identity);
            return;
        }
        self.query_initializer_roles.insert(identity, role);
    }

    fn record_class_api_call_arguments(
        &mut self,
        identity: SymbolIdentity,
        arguments: Vec<Box<Expr>>,
    ) {
        if self.ambiguous_symbols.contains(&identity)
            || self
                .ivy_names
                .get(&identity)
                .is_none_or(|name| name != "signal")
            || self.class_api_call_arguments.contains_key(&identity)
        {
            return;
        }
        self.class_api_call_arguments.insert(identity, arguments);
    }

    fn install_aliases(&mut self, aliases: &[WorkspaceSymbolAlias]) {
        let mut adjacency: HashMap<SymbolIdentity, Vec<SymbolIdentity>> = HashMap::new();
        for alias in aliases {
            let left = workspace_symbol_identity(&alias.left);
            let right = workspace_symbol_identity(&alias.right);
            adjacency
                .entry(left.clone())
                .or_default()
                .push(right.clone());
            adjacency.entry(right).or_default().push(left);
        }

        let mut visited = HashSet::new();
        for start in adjacency.keys() {
            if visited.contains(start) {
                continue;
            }
            let mut stack = vec![start.clone()];
            let mut component = Vec::new();
            while let Some(identity) = stack.pop() {
                if !visited.insert(identity.clone()) {
                    continue;
                }
                if let Some(neighbors) = adjacency.get(&identity) {
                    stack.extend(neighbors.iter().cloned());
                }
                component.push(identity);
            }
            let group_index = self.alias_groups.len();
            for identity in &component {
                self.alias_group_by_symbol
                    .insert(identity.clone(), group_index);
            }
            self.alias_groups.push(component);
        }
    }

    fn propagate_aliases(&mut self) {
        for (group_index, component) in self.alias_groups.clone().into_iter().enumerate() {
            let names = component
                .iter()
                .filter_map(|identity| self.ivy_names.get(identity).cloned())
                .collect::<HashSet<_>>();
            let arguments_already_propagated =
                self.class_api_argument_alias_groups.contains(&group_index);
            let call_argument_sources = if arguments_already_propagated {
                Vec::new()
            } else {
                component
                    .iter()
                    .filter_map(|identity| self.class_api_call_arguments.get(identity).cloned())
                    .collect::<Vec<_>>()
            };
            let query_roles = component
                .iter()
                .filter_map(|identity| self.query_initializer_roles.get(identity).copied())
                .collect::<HashSet<_>>();
            let is_ambiguous = component
                .iter()
                .any(|identity| self.ambiguous_symbols.contains(identity))
                || names.len() > 1
                || call_argument_sources.len() > 1
                || query_roles.len() > 1;
            if is_ambiguous {
                self.class_api_argument_alias_groups.remove(&group_index);
                for identity in component {
                    self.ivy_names.remove(&identity);
                    self.class_api_call_arguments.remove(&identity);
                    self.query_initializer_roles.remove(&identity);
                    self.ambiguous_symbols.insert(identity);
                }
            } else {
                let name = names.into_iter().next();
                let call_arguments = call_argument_sources.into_iter().next();
                let query_role = query_roles.into_iter().next();
                for identity in component {
                    if let Some(name) = &name {
                        self.ivy_names.insert(identity.clone(), name.clone());
                    }
                    if let Some(arguments) = &call_arguments {
                        self.class_api_call_arguments
                            .insert(identity.clone(), arguments.clone());
                    }
                    if let Some(role) = query_role {
                        self.query_initializer_roles.insert(identity, role);
                    }
                }
                if call_arguments.is_some() {
                    self.class_api_argument_alias_groups.insert(group_index);
                }
            }
        }
    }

    fn alias_group_index(&self, identity: &SymbolIdentity) -> Option<usize> {
        self.alias_group_by_symbol.get(identity).copied()
    }

    pub(super) fn instruction_for_callee(
        &self,
        callee: &Callee,
        unresolved_ctxt: SyntaxContext,
    ) -> Option<IvyInstruction> {
        let Callee::Expr(expr) = callee else {
            return None;
        };
        self.instruction_for_expr(expr.as_ref(), unresolved_ctxt)
    }

    pub(super) fn class_api_for_callee(
        &self,
        callee: &Callee,
        unresolved_ctxt: SyntaxContext,
    ) -> Option<AngularClassApi> {
        let Callee::Expr(expr) = callee else {
            return None;
        };
        self.class_api_for_expr(expr.as_ref(), unresolved_ctxt)
    }

    pub(super) fn query_initializer_for_call(
        &self,
        call: &CallExpr,
        unresolved_ctxt: SyntaxContext,
    ) -> Option<AngularQueryInitializer> {
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let callee = strip_parenthesized_expr(callee.as_ref());
        if let Expr::Member(member) = callee {
            if member_prop_name(&member.prop).as_deref() == Some("required") {
                let mut initializer = self.query_initializer_for_expr(
                    member.obj.as_ref(),
                    &call.args,
                    unresolved_ctxt,
                )?;
                if initializer.multiple || initializer.required {
                    return None;
                }
                initializer.required = true;
                return Some(initializer);
            }
        }
        self.query_initializer_for_expr(callee, &call.args, unresolved_ctxt)
    }

    fn query_initializer_for_expr(
        &self,
        expression: &Expr,
        arguments: &[ExprOrSpread],
        unresolved_ctxt: SyntaxContext,
    ) -> Option<AngularQueryInitializer> {
        let expression = strip_parenthesized_expr(expression);
        if let Some(initializer) = self
            .class_api_for_expr(expression, unresolved_ctxt)
            .and_then(AngularClassApi::query_initializer)
        {
            return Some(initializer);
        }

        let identity = symbol_identity(expression, unresolved_ctxt)?;
        match self.query_initializer_roles.get(&identity)? {
            QueryInitializerRole::DynamicFactory => {
                let [first_only, required, ..] = arguments else {
                    return None;
                };
                if first_only.spread.is_some() || required.spread.is_some() {
                    return None;
                }
                let first_only = boolean_expression_value(first_only.expr.as_ref())?;
                let required = boolean_expression_value(required.expr.as_ref())?;
                (!(!first_only && required)).then_some(AngularQueryInitializer {
                    owner: None,
                    multiple: !first_only,
                    required,
                })
            }
            QueryInitializerRole::Fixed { multiple, required } => Some(AngularQueryInitializer {
                owner: None,
                multiple: *multiple,
                required: *required,
            }),
        }
    }

    pub(super) fn specialized_class_api_arguments_for_callee(
        &self,
        callee: &Callee,
        unresolved_ctxt: SyntaxContext,
    ) -> Option<Vec<Box<Expr>>> {
        let Callee::Expr(expr) = callee else {
            return None;
        };
        let identity = symbol_identity(expr.as_ref(), unresolved_ctxt)?;
        self.class_api_call_arguments.get(&identity).cloned()
    }

    pub(super) fn class_api_for_expr(
        &self,
        expr: &Expr,
        unresolved_ctxt: SyntaxContext,
    ) -> Option<AngularClassApi> {
        if let Some(name) = self.ivy_name_for_expr(expr, unresolved_ctxt) {
            return AngularClassApi::from_export_name(&name);
        }

        let Expr::Member(member) = expr else {
            return None;
        };
        let Expr::Ident(object) = member.obj.as_ref() else {
            return None;
        };
        if !self.core_namespaces.contains(&binding_key(object)) {
            return None;
        }
        AngularClassApi::from_export_name(member_prop_name(&member.prop)?.as_ref())
    }

    pub(super) fn instruction_for_expr(
        &self,
        expr: &Expr,
        unresolved_ctxt: SyntaxContext,
    ) -> Option<IvyInstruction> {
        self.ivy_name_for_expr(expr, unresolved_ctxt)
            .and_then(|name| IvyInstruction::from_export_name(&name))
    }

    pub(super) fn listener_target_for_expr(
        &self,
        expr: &Expr,
        unresolved_ctxt: SyntaxContext,
    ) -> Option<AngularListenerTarget> {
        self.ivy_name_for_expr(expr, unresolved_ctxt)
            .and_then(|name| AngularListenerTarget::from_export_name(&name))
    }

    pub(super) fn ivy_name_for_expr(
        &self,
        expr: &Expr,
        unresolved_ctxt: SyntaxContext,
    ) -> Option<String> {
        if let Some(identity) = symbol_identity(expr, unresolved_ctxt) {
            if let Some(name) = self.ivy_names.get(&identity) {
                return Some(name.clone());
            }
        }

        match expr {
            Expr::Ident(ident) if ident.ctxt == unresolved_ctxt => {
                ident.sym.starts_with("ɵɵ").then(|| ident.sym.to_string())
            }
            Expr::Member(member) => {
                let Expr::Ident(object) = member.obj.as_ref() else {
                    return None;
                };
                if !self.core_namespaces.contains(&binding_key(object)) {
                    return None;
                }
                let name = member_prop_name(&member.prop)?;
                name.starts_with("ɵɵ").then(|| name.to_string())
            }
            _ => None,
        }
    }

    pub(super) fn is_reference_candidate_expr(
        &self,
        expr: &Expr,
        unresolved_ctxt: SyntaxContext,
    ) -> bool {
        self.ivy_name_for_expr(expr, unresolved_ctxt).as_deref() == Some(REFERENCE_CANDIDATE_NAME)
    }

    pub(super) fn is_namespace_html_reset_assignment(
        &self,
        assignment: &AssignExpr,
        unresolved_ctxt: SyntaxContext,
    ) -> bool {
        if assignment.op != swc_core::ecma::ast::AssignOp::Assign
            || !matches!(
                assignment.right.as_ref(),
                Expr::Lit(swc_core::ecma::ast::Lit::Null(_))
            )
        {
            return false;
        }
        let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assignment.left else {
            return false;
        };
        symbol_identity(&Expr::Member(member.clone()), unresolved_ctxt)
            .is_some_and(|target| self.namespace_state_targets.contains(&target))
    }

    pub(super) fn is_known_runtime_member(
        &self,
        expr: &Expr,
        unresolved_ctxt: SyntaxContext,
    ) -> bool {
        let Some(identity) = symbol_identity(expr, unresolved_ctxt) else {
            return false;
        };
        match identity {
            SymbolIdentity::LocalMember { object, .. } => {
                self.core_namespaces.contains(&object)
                    || self.ivy_names.keys().any(|identity| {
                        matches!(
                            identity,
                            SymbolIdentity::LocalMember {
                                object: known_object,
                                ..
                            } if known_object == &object
                        )
                    })
            }
            SymbolIdentity::GlobalMember { object, .. } => self.ivy_names.keys().any(|identity| {
                matches!(
                    identity,
                    SymbolIdentity::GlobalMember {
                        object: known_object,
                        ..
                    } if known_object == &object
                )
            }),
            SymbolIdentity::LocalBinding(_) | SymbolIdentity::GlobalBinding(_) => false,
        }
    }

    pub(super) fn is_core_namespace_member(
        &self,
        expr: &Expr,
        unresolved_ctxt: SyntaxContext,
    ) -> bool {
        matches!(
            symbol_identity(expr, unresolved_ctxt),
            Some(SymbolIdentity::LocalMember { object, .. })
                if self.core_namespaces.contains(&object)
        )
    }
}

fn workspace_symbol_identity(symbol: &WorkspaceSymbol) -> SymbolIdentity {
    match symbol {
        WorkspaceSymbol::Binding(binding) => SymbolIdentity::LocalBinding(binding.clone()),
        WorkspaceSymbol::Member { object, property } => SymbolIdentity::LocalMember {
            object: object.clone(),
            property: property.clone(),
        },
    }
}

pub(super) struct IvyCallCollector<'a> {
    roles: &'a IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
    pub(super) define_component_calls: Vec<IvyComponentCall>,
}

pub(super) struct IvyComponentCall {
    pub(super) call: CallExpr,
    pub(super) definition_field: Option<Atom>,
}

impl<'a> IvyCallCollector<'a> {
    pub(super) fn new(roles: &'a IvyRoleTable, unresolved_ctxt: SyntaxContext) -> Self {
        Self {
            roles,
            unresolved_ctxt,
            define_component_calls: Vec::new(),
        }
    }
}

impl<'a> Visit for IvyCallCollector<'a> {
    fn visit_class_prop(&mut self, property: &ClassProp) {
        let Some(Expr::Call(call)) = property.value.as_deref() else {
            property.visit_children_with(self);
            return;
        };
        if !property.is_static || !self.is_define_component(call) {
            property.visit_children_with(self);
            return;
        }
        self.define_component_calls.push(IvyComponentCall {
            call: call.clone(),
            definition_field: prop_name_atom(&property.key),
        });
    }

    fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
        let Expr::Call(call) = assignment.right.as_ref() else {
            assignment.visit_children_with(self);
            return;
        };
        if !self.is_define_component(call) {
            assignment.visit_children_with(self);
            return;
        }
        let definition_field = match &assignment.left {
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
                member_prop_name(&member.prop)
            }
            _ => None,
        };
        self.define_component_calls.push(IvyComponentCall {
            call: call.clone(),
            definition_field,
        });
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if self.is_define_component(call) {
            self.define_component_calls.push(IvyComponentCall {
                call: call.clone(),
                definition_field: None,
            });
        }
        call.visit_children_with(self);
    }
}

impl IvyCallCollector<'_> {
    fn is_define_component(&self, call: &CallExpr) -> bool {
        self.roles
            .instruction_for_callee(&call.callee, self.unresolved_ctxt)
            == Some(IvyInstruction::DefineComponent)
    }
}

fn prop_name_atom(name: &PropName) -> Option<Atom> {
    match name {
        PropName::Ident(ident) => Some(ident.sym.clone()),
        PropName::Str(string) => Some(Atom::from(wtf8_to_string(&string.value))),
        _ => None,
    }
}

pub(super) fn symbol_identity(
    expr: &Expr,
    unresolved_ctxt: SyntaxContext,
) -> Option<SymbolIdentity> {
    match expr {
        Expr::Ident(ident) if ident.ctxt == unresolved_ctxt => {
            Some(SymbolIdentity::GlobalBinding(ident.sym.clone()))
        }
        Expr::Ident(ident) => Some(SymbolIdentity::LocalBinding(binding_key(ident))),
        Expr::Member(member) => {
            let property = member_prop_name(&member.prop)?;
            if let Expr::Ident(object) = member.obj.as_ref() {
                if object.ctxt != unresolved_ctxt {
                    return Some(SymbolIdentity::LocalMember {
                        object: binding_key(object),
                        property,
                    });
                }
                Some(SymbolIdentity::GlobalMember {
                    object: object.sym.clone(),
                    property,
                })
            } else {
                Some(SymbolIdentity::GlobalMember {
                    object: global_object_path(member.obj.as_ref(), unresolved_ctxt)?,
                    property,
                })
            }
        }
        _ => None,
    }
}

fn global_object_path(expr: &Expr, unresolved_ctxt: SyntaxContext) -> Option<Atom> {
    match expr {
        Expr::This(_) => Some(Atom::from("this")),
        Expr::Ident(identifier) if identifier.ctxt == unresolved_ctxt => {
            Some(identifier.sym.clone())
        }
        Expr::Member(member) => {
            let object = global_object_path(member.obj.as_ref(), unresolved_ctxt)?;
            let property = member_prop_name(&member.prop)?;
            Some(Atom::from(format!("{object}.{property}")))
        }
        Expr::Paren(paren) => global_object_path(paren.expr.as_ref(), unresolved_ctxt),
        _ => None,
    }
}

struct IvyExportMapCollector {
    unresolved_ctxt: SyntaxContext,
    module_index: usize,
    mappings: Vec<(SymbolIdentity, String, usize, u32)>,
}

impl Visit for IvyExportMapCollector {
    fn visit_prop(&mut self, prop: &Prop) {
        let Prop::KeyValue(key_value) = prop else {
            prop.visit_children_with(self);
            return;
        };
        let Some(name) = ivy_export_prop_name(&key_value.key) else {
            key_value.visit_children_with(self);
            return;
        };
        if !name.starts_with("ɵɵ") {
            key_value.visit_children_with(self);
            return;
        }
        let Some(value) = exported_symbol_expr(key_value.value.as_ref()) else {
            key_value.visit_children_with(self);
            return;
        };
        if let Some(identity) = symbol_identity(value, self.unresolved_ctxt) {
            self.mappings.push((
                identity,
                name,
                self.module_index,
                key_value.value.span().lo.0,
            ));
        }
        key_value.visit_children_with(self);
    }
}

fn ivy_export_prop_name(name: &PropName) -> Option<String> {
    match name {
        PropName::Ident(ident) => Some(ident.sym.to_string()),
        PropName::Str(string) => Some(wtf8_to_string(&string.value)),
        _ => None,
    }
}

fn exported_symbol_expr(expression: &Expr) -> Option<&Expr> {
    match expression {
        Expr::Ident(_) | Expr::Member(_) => Some(expression),
        Expr::Arrow(arrow) if arrow.params.is_empty() => match arrow.body.as_ref() {
            BlockStmtOrExpr::Expr(expression) => exported_symbol_expr(expression),
            BlockStmtOrExpr::BlockStmt(block) => {
                let [Stmt::Return(return_statement)] = block.stmts.as_slice() else {
                    return None;
                };
                exported_symbol_expr(return_statement.arg.as_deref()?)
            }
        },
        Expr::Fn(function) if function.function.params.is_empty() => {
            let [Stmt::Return(return_statement)] =
                function.function.body.as_ref()?.stmts.as_slice()
            else {
                return None;
            };
            exported_symbol_expr(return_statement.arg.as_deref()?)
        }
        Expr::Paren(paren) => exported_symbol_expr(paren.expr.as_ref()),
        _ => None,
    }
}

fn strip_parenthesized_expr(mut expression: &Expr) -> &Expr {
    while let Expr::Paren(parenthesized) = expression {
        expression = parenthesized.expr.as_ref();
    }
    expression
}

fn boolean_expression_value(expression: &Expr) -> Option<bool> {
    match strip_parenthesized_expr(expression) {
        Expr::Lit(swc_core::ecma::ast::Lit::Bool(boolean)) => Some(boolean.value),
        Expr::Unary(unary) if unary.op == swc_core::ecma::ast::UnaryOp::Bang => {
            match strip_parenthesized_expr(unary.arg.as_ref()) {
                Expr::Lit(swc_core::ecma::ast::Lit::Num(number)) => Some(number.value == 0.0),
                _ => None,
            }
        }
        _ => None,
    }
}

fn module_export_name(name: &ModuleExportName) -> String {
    match name {
        ModuleExportName::Ident(ident) => ident.sym.to_string(),
        ModuleExportName::Str(string) => wtf8_to_string(&string.value),
    }
}
