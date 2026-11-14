//! A has_many's `build` and `<<` from app code (#1364). Rails 8.1.4 keeps a
//! record built on, or pushed onto, an owner in the association's target:
//! the owner validates it with itself (a has_many's `validate: true`
//! default: "Tasks is invalid") and saves it after itself, with the
//! foreign key. `<<` on a persisted owner sets the key and saves the
//! record at once (an invalid one stays unsaved); on a new owner it is
//! held like a built one. A new owner's reader answers the records it
//! holds.
//!
//! On the ruby family ([`expand`]), each has_many that app code builds on
//! or pushes onto (`<name>.build` / `<name> << record`, by name) gets
//! model source, after its declaration:
//!
//! ```ruby
//! def __hold_tasks(record)        # a lowered `tasks.build(…)` passes its record
//! def __push_tasks(record)        # a lowered `tasks << record`
//! validate :__held_tasks_valid    # each held record valid, else `errors.add(:tasks, :invalid)`
//! after_save :__held_tasks_save   # each held record saved, with the key
//! after_save_commit :__held_tasks_committed  # the held list cleared once the save commits
//! ```
//!
//! `lower::scope_chain` routes the calls there, and the reader's unsaved
//! guard answers the held records, for a model that has them.

use std::collections::BTreeSet;

use crate::app::App;
use crate::dialect::{Association, ModelBodyItem};
use crate::expr::{Expr, ExprNode};

/// The has_many names app code builds on or pushes onto.
fn used_names(app: &App) -> BTreeSet<String> {
    fn walk(e: &Expr, out: &mut BTreeSet<String>) {
        if let ExprNode::Send { recv: Some(recv), method, .. } = &*e.node
            && matches!(method.as_str(), "build" | "<<")
            && let ExprNode::Send { method: name, args, .. } = &*recv.node
            && args.is_empty()
        {
            out.insert(name.as_str().to_string());
        }
        e.node.for_each_child(&mut |c| walk(c, out));
    }
    let mut out = BTreeSet::new();
    for model in &app.models {
        model.methods().for_each(|m| walk(&m.body, &mut out));
    }
    for controller in &app.controllers {
        controller.actions().for_each(|a| walk(&a.body, &mut out));
        controller.class_methods().for_each(|m| walk(&m.body, &mut out));
    }
    for lc in &app.library_classes {
        lc.methods.iter().for_each(|m| walk(&m.body, &mut out));
    }
    for view in &app.views {
        walk(&view.body, &mut out);
    }
    out
}

pub(crate) fn expand(app: &mut App) {
    let used = used_names(app);
    if used.is_empty() {
        return;
    }
    let snapshot = app.models.clone();
    // The generated methods' signatures: their calls are written after
    // analysis (`lower::scope_chain`), so no call site types `record`.
    let mut signatures: Vec<(crate::ident::ClassId, String, crate::ident::ClassId)> = Vec::new();
    for model in &mut app.models {
        let mut inserts: Vec<(usize, Vec<ModelBodyItem>)> = Vec::new();
        for (index, item) in model.body.iter().enumerate() {
            let ModelBodyItem::Association { assoc, .. } = item else { continue };
            let Association::HasMany { name, target, foreign_key, through: None, as_interface, primary_key: None, .. } = assoc
            else {
                continue;
            };
            if !used.contains(name.as_str()) {
                continue;
            }
            // Only an app model: a gem's (`Doorkeeper::Application`) has no
            // `save` here to call.
            if !snapshot.iter().any(|m| &m.name == target) {
                continue;
            }
            let hold = format!("__hold_{}", name.as_str());
            if model.methods().any(|m| m.name.as_str().starts_with(&hold) || m.name.as_str() == format!("__push_{name}")) {
                continue;
            }
            // The target's `belongs_to` on the same key, set to the owner as
            // Rails sets the inverse, so a required one holds before the save.
            let inverse = snapshot.iter().find(|m| &m.name == target).and_then(|t| {
                t.associations().find_map(|a| match a {
                    Association::BelongsTo { name: bt, foreign_key: fk, polymorphic: false, .. } if fk == foreign_key => {
                        Some(bt.as_str().to_string())
                    }
                    _ => None,
                })
            });
            // A held child is checked as Rails checks it with the inverse
            // set: its `belongs_to`'s "must exist" reads the key, which the
            // save sets, so that error alone doesn't make it invalid.
            let counts = match &inverse {
                Some(bt) => format!("      child.errors.each {{ |error| invalid = true unless error.attribute.to_s == \"{bt}\" }}\n"),
                None => "      invalid = true\n".to_string(),
            };
            let set_owner = match &inverse {
                Some(bt) => format!("      record.{bt} = self\n"),
                None => String::new(),
            };
            let set_type = as_interface
                .as_ref()
                .map(|i| format!("      record.{i}_type = \"{}\"\n", model.name.0.as_str()))
                .unwrap_or_default();
            let n = name.as_str();
            let fk = foreign_key.as_str();
            // The model by name, not `self.class`: Spinel loses a local a
            // block assigns when the block raises out of a `self.class` call
            // (matz/spinel#10291).
            let owner = model.name.0.as_str();
            let src = format!(
                "class Generated\n  def __hold_{n}(record)\n    @__held_{n} = [] if @__held_{n}.nil?\n    @__held_{n} << record\n    record\n  end\n\n  def __push_{n}(record)\n    if new_record?\n{set_owner}{set_type}      __hold_{n}(record)\n      {n}\n    else\n      record.{fk} = id\n{set_type}      record.save ? {n} : nil\n    end\n  end\n\n  def __push_all_{n}(records)\n    if new_record?\n      records.each {{ |record| __push_{n}(record) }}\n      return {n}\n    end\n    pushed = true\n    {owner}.transaction do\n      records.each do |record|\n        next unless __push_{n}(record).nil?\n        pushed = false\n        raise ActiveRecord::Rollback\n      end\n    end\n    pushed ? {n} : nil\n  end\n\n  def __held_{n}_list\n    @__held_{n}.nil? ? [] : @__held_{n}\n  end\n\n  def __held_{n}_valid\n    return if @__held_{n}.nil?\n    invalid = false\n    __held_{n}_list.each do |child|\n      next if child.valid?\n{counts}    end\n    errors.add(:{n}, :invalid) if invalid\n  end\n\n  validate :__held_{n}_valid\n\n  def __held_{n}_save\n    return if @__held_{n}.nil? || @__held_{n}_saving\n    @__held_{n}_saving = true\n    begin\n      __held_{n}_list.each do |child|\n        child.{fk} = id\n        child.save!\n      end\n    ensure\n      @__held_{n}_saving = false\n    end\n  end\n\n  after_save :__held_{n}_save\n\n  def __held_{n}_committed\n    @__held_{n} = nil\n  end\n\n  after_save_commit :__held_{n}_committed\nend\n"
            );
            if let Some(items) = super::validates_with::ingest_source(&src, &model.name) {
                inserts.push((index + 1, items));
                signatures.push((model.name.clone(), n.to_string(), target.clone()));
            }
        }
        for (at, items) in inserts.into_iter().rev() {
            for (k, item) in items.into_iter().enumerate() {
                model.body.insert(at + k, item);
            }
        }
    }
    for (model, name, target) in signatures {
        let record = crate::ty::Ty::Class { id: target, args: vec![].into() };
        let takes = |ret: crate::ty::Ty| crate::ty::Ty::Fn(Box::new(crate::ty::FnTy {
            params: vec![crate::ty::Param {
                name: crate::ident::Symbol::from("record"),
                ty: record.clone().into(),
                kind: crate::ty::ParamKind::Required,
            }]
            .into(),
            block: None,
            ret: ret.into(),
            effects: Default::default(),
        }));
        let table = app.rbs_signatures.entry(model).or_default();
        table.insert(crate::ident::Symbol::from(format!("__hold_{name}")), takes(record.clone()));
        // `<<` answers the association; the record is what callers chain
        // on least, so the declared answer is the record's class only
        // where nothing better is known: none is, before analysis. Nil
        // when a persisted owner's save of it fails, as Rails' `<<` is.
        let pushed = crate::ty::Ty::Union { variants: vec![record.clone(), crate::ty::Ty::Nil].into() };
        table.insert(crate::ident::Symbol::from(format!("__push_{name}")), takes(pushed.clone()));
        // `<<` with an Array adds each element (`concat(records.flatten)`).
        table.insert(
            crate::ident::Symbol::from(format!("__push_all_{name}")),
            crate::ty::Ty::Fn(Box::new(crate::ty::FnTy {
                params: vec![crate::ty::Param {
                    name: crate::ident::Symbol::from("records"),
                    ty: crate::ty::Ty::Array { elem: record.clone().into() }.into(),
                    kind: crate::ty::ParamKind::Required,
                }]
                .into(),
                block: None,
                ret: pushed.into(),
                effects: Default::default(),
            })),
        );
        table.insert(
            crate::ident::Symbol::from(format!("__held_{name}_list")),
            crate::ty::Ty::Fn(Box::new(crate::ty::FnTy {
                params: vec![].into(),
                block: None,
                ret: crate::ty::Ty::Array { elem: record.clone().into() }.into(),
                effects: Default::default(),
            })),
        );
    }
}
