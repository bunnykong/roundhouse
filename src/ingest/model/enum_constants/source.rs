//! Prism declaration collection and lexical traversal. Use-context syntax is
//! classified separately; only policy interprets the resulting typed facts.

use super::super::super::util::{constant_id_str, flatten_statements, symbol_or_string_value};
use super::super::super::{expr, library_class, prism};
use super::{
    EnumConstant, EnumConstants, ScopedName, SourceFact, SurfaceUse, ValueContext, context,
};
use crate::expr::Literal;
use ruby_prism::Node;

#[derive(Clone, Copy)]
enum SourceKind {
    Inputs,
    Mutations,
}

impl EnumConstants {
    pub(in crate::ingest) fn record(&mut self, source: &[u8], file: &str) {
        self.collect(source, file, SourceKind::Inputs);
    }

    /// Observe nonlibrary sources without admitting new constant inputs.
    pub(in crate::ingest) fn record_mutations(&mut self, source: &[u8], file: &str) {
        self.collect(source, file, SourceKind::Mutations);
    }

    fn collect(&mut self, source: &[u8], file: &str, kind: SourceKind) {
        struct Collector<'a, 'pr> {
            constants: &'a mut EnumConstants,
            file: &'a str,
            kind: SourceKind,
            nesting: Vec<String>,
            parents: Vec<Node<'pr>>,
            /// Offsets of the plain `NAME = …` writes at a file's or a
            /// body's top level, the one write a constant may have.
            direct_writes: std::collections::HashSet<usize>,
        }
        impl Collector<'_, '_> {
            fn scoped(&self, path: String) -> ScopedName {
                ScopedName {
                    path,
                    owners: self.nesting.iter().rev().cloned().collect(),
                }
            }

            fn record_reference(&mut self, node: &Node<'_>) {
                if node.as_self_node().is_some() {
                    if let Some(owner) = self.nesting.last() {
                        // Uncertain self does not get a guessed identity. It
                        // makes the enclosing candidate vocabulary unprovable.
                        let role = if context::namespace_body(&self.parents) {
                            context::value(&self.parents, node)
                        } else {
                            ValueContext::Unmodeled
                        };
                        self.constants.facts.push(SourceFact::Use {
                            subject: self.scoped(format!("::{owner}")),
                            context: SurfaceUse::Reference(role),
                        });
                    }
                }
                if let Some(call) = node.as_call_node() {
                    let implicit = call.receiver().is_none();
                    // Canonical annotations return their first argument; its
                    // original references/type arguments are still visited.
                    let assertion = expr::sorbet_assertion_argument(node).is_some();
                    // The only implicit declaration exemptions use the shared
                    // literal projection or admitted Rails mapping recognizer.
                    let declaration = implicit
                        && (self.nesting.last().is_some_and(|owner| {
                            matches!(
                                self.constants.values.get(owner),
                                Some(EnumConstant::Sorbet { .. })
                            )
                        }) || super::super::enum_declaration(&call).is_some_and(|declaration| {
                            super::super::serialized_enum_receiver(&declaration.mapping).is_some()
                        }));
                    let path = call
                        .receiver()
                        .and_then(|r| EnumConstants::path(&r))
                        .or_else(|| {
                            (context::namespace_body(&self.parents)
                                && (implicit
                                    || call.receiver().is_some_and(|r| r.as_self_node().is_some())))
                            .then(|| self.nesting.last().map(|owner| format!("::{owner}")))
                            .flatten()
                        });
                    if let Some(path) = path.filter(|_| !assertion && !declaration) {
                        self.constants.facts.push(SourceFact::Use {
                            subject: self.scoped(path),
                            context: SurfaceUse::Call {
                                method: constant_id_str(&call.name()).to_string(),
                                result: context::value(&self.parents, node),
                            },
                        });
                    }
                }
                if let Some((path, role)) = context::reference(&self.parents, node) {
                    self.constants.facts.push(SourceFact::Use {
                        subject: self.scoped(path),
                        context: SurfaceUse::Reference(role),
                    });
                }
            }
        }
        impl<'pr> ruby_prism::Visit<'pr> for Collector<'_, 'pr> {
            fn visit_branch_node_enter(&mut self, node: Node<'pr>) {
                // Classifier receives ancestors only, identically for branches
                // and leaves. Push/pop below is solely the traversal contract.
                self.record_reference(&node);
                let direct = matches!(self.kind, SourceKind::Inputs)
                    && self.parents.iter().all(|parent| {
                        matches!(
                            parent,
                            Node::ProgramNode { .. }
                                | Node::StatementsNode { .. }
                                | Node::ClassNode { .. }
                                | Node::ModuleNode { .. }
                        )
                    });
                self.constants.record_write(&node, &self.nesting, direct);
                if direct && node.as_constant_write_node().is_some() {
                    self.direct_writes.insert(node.location().start_offset());
                }
                // `const_set(:X, …)` / `self.send(:remove_const, :X)` in a
                // body replaces that owner's constants.
                if matches!(self.kind, SourceKind::Inputs)
                    && let Some(call) = node.as_call_node()
                    && call.receiver().is_none_or(|r| r.as_self_node().is_some())
                    && super::super::REFLECTION_METHODS.contains(&constant_id_str(&call.name()).as_ref())
                {
                    // Outside any body the receiver is `main`, whose
                    // `const_get` reaches every owner, as `Object`'s does;
                    // its `const_set(:Name, …)` only the one `Name`.
                    let named = super::super::named_constant_write(&call);
                    let owner = match (self.nesting.last(), &named) {
                        (Some(owner), _) => owner.as_str(),
                        (None, Some(name)) => name.as_str(),
                        (None, None) => "Object",
                    };
                    let last = owner.rsplit("::").next().unwrap_or(owner).to_string();
                    self.constants.reflected.insert(last);
                }
                let path = node
                    .as_class_node()
                    .map(|c| c.constant_path())
                    .or_else(|| node.as_module_node().map(|m| m.constant_path()));
                if let Some(path) = path.and_then(|p| EnumConstants::path(&p)) {
                    let owner = self.nesting.last().map(String::as_str).unwrap_or("");
                    let name = EnumConstants::qualify(owner, &path);
                    let qualified = path.contains("::");
                    if qualified {
                        self.constants.facts.push(SourceFact::Reopen {
                            subject: self.scoped(path),
                            collected: name.clone(),
                        });
                    }
                    let value = node
                        .as_class_node()
                        .filter(|_| direct)
                        .and_then(|class| {
                            let scope = self
                                .nesting
                                .last()
                                .map(|name| {
                                    name.split("::").map(str::to_string).collect::<Vec<_>>()
                                })
                                .unwrap_or_default();
                            Some(EnumConstant::Sorbet {
                                members: library_class::literal_sorbet_members(
                                    &class, &scope, self.file,
                                )?,
                                base: EnumConstants::path(&class.superclass()?)?,
                                owners: self.nesting.iter().rev().cloned().collect(),
                            })
                        })
                        .unwrap_or(EnumConstant::Namespace);
                    if let Some(base) = node
                        .as_class_node()
                        .and_then(|class| class.superclass())
                        .and_then(|base| EnumConstants::path(&base))
                    {
                        let parent = self.scoped(base.clone());
                        self.constants.parents.entry(name.clone()).or_default().push(parent);
                        self.constants.facts.push(SourceFact::Use {
                            subject: self.scoped(base),
                            context: if matches!(value, EnumConstant::Sorbet { .. }) {
                                SurfaceUse::EnumBase
                            } else {
                                SurfaceUse::Inheritance
                            },
                        });
                    }
                    self.constants
                        .values
                        .entry(name.clone())
                        .and_modify(|existing| {
                            // Reopening a declared enum can change its surface.
                            if matches!(existing, EnumConstant::Sorbet { .. }) {
                                *existing = EnumConstant::Unsupported;
                            }
                        })
                        .or_insert(value);
                    // Classes and modules both own lexical constant
                    // lookup — concern `TYPES` inside `included do`
                    // resolves through the module entry the same way a
                    // model class resolves `STATUS` for `enum`.
                    if matches!(self.kind, SourceKind::Inputs) {
                        // `module Wrapper; class Catalog::Rating` may reopen
                        // ::Catalog::Rating: the collected name is not
                        // certain, so its table never folds.
                        let certain = direct && (!qualified || self.nesting.is_empty());
                        let table = certain.then(|| {
                            let body = node
                                .as_class_node()
                                .and_then(|class| class.body())
                                .or_else(|| node.as_module_node().and_then(|module| module.body()));
                            super::super::ClassConsts::collect(
                                &body.map(flatten_statements).unwrap_or_default(),
                                self.file,
                            )
                        });
                        match (self.constants.owner_consts.get_mut(&name), table) {
                            (Some(Some(existing)), Some(table)) => existing.merge(table),
                            (Some(existing), None) => *existing = None,
                            (Some(None), Some(_)) => {}
                            (None, table) => {
                                self.constants.owner_consts.insert(name.clone(), table);
                            }
                        }
                    }
                    if node.as_class_node().is_some() || node.as_module_node().is_some() {
                        self.constants.nesting.insert(
                            (self.file.to_string(), node.location().start_offset()),
                            std::iter::once(name.clone())
                                .chain(self.nesting.iter().rev().cloned())
                                .collect(),
                        );
                    }
                    self.nesting.push(name);
                }
                self.parents.push(node);
            }

            fn visit_branch_node_leave(&mut self) {
                let node = self.parents.pop().unwrap();
                let path = node
                    .as_class_node()
                    .map(|c| c.constant_path())
                    .or_else(|| node.as_module_node().map(|m| m.constant_path()));
                if path.and_then(|p| EnumConstants::path(&p)).is_some() {
                    self.nesting.pop();
                }
            }

            fn visit_leaf_node_enter(&mut self, node: Node<'pr>) {
                self.record_reference(&node);
                self.constants.record_write(&node, &self.nesting, false);
            }
        }
        let result = match kind {
            SourceKind::Inputs => prism::parse(source, file),
            // Already consumed/diagnosed by the real pipeline. Observe the
            // original facts without adding a second set of parse errors.
            SourceKind::Mutations => ruby_prism::parse(source),
        };
        if result.errors().next().is_none() {
            let mut collector = Collector {
                constants: self,
                file,
                kind,
                nesting: Vec::new(),
                parents: Vec::new(),
                direct_writes: Default::default(),
            };
            ruby_prism::Visit::visit(&mut collector, &result.node());
            let direct_writes = collector.direct_writes;
            if matches!(kind, SourceKind::Inputs) {
                let (tainted, reflected) = super::super::ConstTaint::scan_tree(&result.node(), direct_writes);
                self.foreign_taint.extend(tainted);
                self.reflected.extend(reflected);
            }
        }
    }

    fn record_write(&mut self, node: &Node<'_>, nesting: &[String], direct: bool) {
        let owner = nesting.last().map(String::as_str).unwrap_or("");
        let override_receiver = node
            .as_def_node()
            .and_then(|def| def.receiver())
            .or_else(|| {
                node.as_singleton_class_node()
                    .map(|class| class.expression())
            });
        if let Some(path) = override_receiver.and_then(|receiver| Self::path(&receiver)) {
            self.facts.push(SourceFact::Use {
                subject: ScopedName {
                    path,
                    owners: nesting.iter().rev().cloned().collect(),
                },
                context: SurfaceUse::Override,
            });
        }
        if let Some(write) = node.as_constant_write_node() {
            let name = Self::qualify(owner, constant_id_str(&write.name()));
            *self.write_counts.entry(name.clone()).or_default() += 1;
            let value = write.value();
            let literal = match value.as_call_node() {
                Some(call)
                    if constant_id_str(&call.name()) == "freeze"
                        && call.arguments().is_none()
                        && call.block().is_none() =>
                {
                    call.receiver()
                }
                _ => Some(value),
            };
            // `%w[…]` strings and `%i[…]` / `[:A, :B]` symbols — same
            // label list `enum_label_values` admits for class-local consts.
            let labels = literal.filter(|_| direct).and_then(|node| {
                node.as_array_node()?
                    .elements()
                    .iter()
                    .enumerate()
                    .map(|(i, el)| {
                        symbol_or_string_value(&el)
                            .map(|label| (label, Literal::Int { value: i as i64 }))
                    })
                    .collect()
            });
            // Reassignments have load-order-dependent semantics, never choose
            // whichever file happened to be visited last.
            self.values
                .entry(name)
                .and_modify(|value| *value = EnumConstant::Unsupported)
                .or_insert(labels.map_or(EnumConstant::Unsupported, EnumConstant::Mapping));
        } else {
            let bare = node
                .as_constant_and_write_node()
                .map(|w| w.name())
                .or_else(|| node.as_constant_or_write_node().map(|w| w.name()))
                .or_else(|| node.as_constant_operator_write_node().map(|w| w.name()))
                .or_else(|| node.as_constant_target_node().map(|w| w.name()));
            if let Some(name) = bare {
                self.values.insert(
                    Self::qualify(owner, constant_id_str(&name)),
                    EnumConstant::Unsupported,
                );
            }
            // Qualified destinations resolve only after namespaces are complete.
            let target = node
                .as_constant_path_write_node()
                .map(|w| w.target().as_node())
                .or_else(|| {
                    node.as_constant_path_and_write_node()
                        .map(|w| w.target().as_node())
                })
                .or_else(|| {
                    node.as_constant_path_or_write_node()
                        .map(|w| w.target().as_node())
                })
                .or_else(|| {
                    node.as_constant_path_operator_write_node()
                        .map(|w| w.target().as_node())
                })
                .or_else(|| node.as_constant_path_target_node().map(|w| w.as_node()));
            if let Some(path) = target.and_then(|target| Self::path(&target)) {
                self.facts.push(SourceFact::Write(ScopedName {
                    path,
                    owners: nesting.iter().rev().cloned().collect(),
                }));
            }
        }
    }

    /// A static path retaining root qualification, rejecting runtime receivers.
    pub(in crate::ingest) fn path(node: &Node<'_>) -> Option<String> {
        if let Some(parentheses) = node.as_parentheses_node() {
            let statements = flatten_statements(parentheses.body()?);
            return match statements.as_slice() {
                [expression] => Self::path(expression),
                _ => None,
            };
        }
        if let Some(read) = node.as_constant_read_node() {
            return Some(constant_id_str(&read.name()).to_string());
        }
        let (parent, name) = if let Some(path) = node.as_constant_path_node() {
            (path.parent(), path.name()?)
        } else {
            let target = node.as_constant_path_target_node()?;
            (target.parent(), target.name()?)
        };
        let mut path = match parent {
            Some(parent) => format!("{}::", Self::path(&parent)?),
            None if node.location().as_slice().starts_with(b"::") => "::".to_string(),
            None => String::new(),
        };
        path.push_str(constant_id_str(&name));
        Some(path)
    }
}
