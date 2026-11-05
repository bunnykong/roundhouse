//! Call-site parameter rows kept across fixpoint rounds.
//!
//! `unify_params_from_call_sites` rebuilds the parameter table every
//! round from every call site in the app. After the first rounds only a
//! few hundred of tens of thousands of sites change their argument
//! types, yet every site was placed on its `def`'s slots and every row
//! folded again. This keeps what does not change between rounds:
//!
//! - the parameter shapes and the defined-method set, which are
//!   functions of the app's `def`s and are built once per analysis;
//! - each parameter key's observations and the row they folded to, so
//!   a key whose observations are the ones it had last round keeps its
//!   row. A row is a fold of a lattice join (`unify_param_ty`) over the
//!   key's observations, then `fixpoint_bound::bound`, and keys are
//!   independent, so refolding only the changed keys gives the table a
//!   full rebuild gives.
//!
//! `ROUNDHOUSE_INCREMENTAL=0` rebuilds everything every round, as before.
//! `ROUNDHOUSE_INCREMENTAL=check` also rebuilds everything and fails the
//! analysis on any difference.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::det::HashMap;
use crate::ty::Ty;

use super::typing_mode::UnifyScope;
use super::{ParamKey, ParamShape};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Mode {
    /// Rebuild every round.
    Off,
    /// Keep unchanged rows.
    On,
    /// Keep unchanged rows and check them against a full rebuild.
    Check,
}

impl Mode {
    /// A value it does not know stops the analysis: a mistyped `check`
    /// that ran unchecked would read as a check that passed.
    fn from_env() -> Self {
        match std::env::var("ROUNDHOUSE_INCREMENTAL") {
            Err(std::env::VarError::NotPresent) => Mode::On,
            Ok(value) => match value.as_str() {
                "" | "1" | "on" | "true" => Mode::On,
                "0" | "off" | "false" => Mode::Off,
                "check" => Mode::Check,
                other => panic!(
                    "ROUNDHOUSE_INCREMENTAL={other:?} is not one of 0, off, false, 1, on, true, check"
                ),
            },
            Err(std::env::VarError::NotUnicode(value)) => {
                panic!("ROUNDHOUSE_INCREMENTAL={value:?} is not one of 0, off, false, 1, on, true, check")
            }
        }
    }
}

/// The parameter shapes and the defined-method set of one analysis.
pub(super) type ParamTables = Arc<(HashMap<ParamKey, ParamShape>, BTreeSet<ParamKey>)>;

/// One key's observations as last folded, and the row they folded to.
type Folded = HashMap<ParamKey, (Vec<Vec<Ty>>, Vec<Ty>)>;

pub(super) struct UnifyMemo {
    pub(super) mode: Mode,
    tables: Option<ParamTables>,
    production: Folded,
    with_views: Folded,
}

impl Default for UnifyMemo {
    fn default() -> Self {
        Self { mode: Mode::from_env(), tables: None, production: Folded::default(), with_views: Folded::default() }
    }
}

impl UnifyMemo {
    /// Forget everything: a new analysis may see different `def`s.
    pub(super) fn reset(&mut self) {
        self.tables = None;
        self.production = Folded::default();
        self.with_views = Folded::default();
    }

    /// The parameter shapes and defined methods, built by `build` once
    /// per analysis (every call under [`Mode::Off`]; checked against a
    /// rebuild under [`Mode::Check`]).
    pub(super) fn tables(&mut self, build: impl Fn() -> (HashMap<ParamKey, ParamShape>, BTreeSet<ParamKey>)) -> ParamTables {
        if self.mode == Mode::Off {
            return Arc::new(build());
        }
        match &self.tables {
            Some(tables) => {
                if self.mode == Mode::Check {
                    let fresh = build();
                    assert!(
                        fresh.0 == tables.0 && fresh.1 == tables.1,
                        "ROUNDHOUSE_INCREMENTAL=check: the parameter shapes or defined methods changed during the analysis"
                    );
                }
                Arc::clone(tables)
            }
            None => {
                let tables = Arc::new(build());
                self.tables = Some(Arc::clone(&tables));
                tables
            }
        }
    }

    fn folded(&mut self, scope: UnifyScope) -> &mut Folded {
        match scope {
            UnifyScope::Production => &mut self.production,
            UnifyScope::WithViews => &mut self.with_views,
        }
    }

    /// Fold `observations` (in site order) into `rows`, which must not
    /// hold any of their keys yet, reusing each key's row from the last
    /// fold of this scope when its observations are the same.
    pub(super) fn fold(
        &mut self,
        scope: UnifyScope,
        rows: &mut HashMap<ParamKey, Vec<Ty>>,
        observations: Vec<(ParamKey, Vec<Ty>)>,
    ) {
        if self.mode == Mode::Off {
            super::fold_param_observations(rows, observations);
            return;
        }
        let full = (self.mode == Mode::Check).then(|| {
            let mut full = rows.clone();
            super::fold_param_observations(&mut full, observations.clone());
            full
        });
        // Group by key in first-seen order: the rows go in in the order a
        // full fold would insert them.
        let mut order: Vec<ParamKey> = Vec::new();
        let mut groups: HashMap<ParamKey, Vec<Vec<Ty>>> = HashMap::default();
        for (key, tys) in observations {
            match groups.get_mut(&key) {
                Some(group) => group.push(tys),
                None => {
                    order.push(key.clone());
                    groups.insert(key, vec![tys]);
                }
            }
        }
        let previous = std::mem::take(self.folded(scope));
        let mut next = Folded::default();
        let (mut kept, mut refolded) = (0usize, 0usize);
        for key in order {
            let observed = groups.remove(&key).expect("grouped above");
            let row = match previous.get(&key) {
                Some((before, row)) if *before == observed => {
                    kept += 1;
                    row.clone()
                }
                _ => {
                    refolded += 1;
                    super::fold_param_row(&observed)
                }
            };
            rows.insert(key.clone(), row.clone());
            next.insert(key, (observed, row));
        }
        *self.folded(scope) = next;
        crate::timings::note(format_args!("unify params rows: kept {kept}, refolded {refolded}"));
        if let Some(full) = full {
            assert!(
                full == *rows,
                "ROUNDHOUSE_INCREMENTAL=check: the kept parameter rows differ from a full fold"
            );
        }
    }
}
