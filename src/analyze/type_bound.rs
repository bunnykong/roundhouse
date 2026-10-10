//! Bound inferred structures before another inference round copies them.

use crate::ty::Ty;

// The seven application fixtures reach four container levels and twelve
// type nodes. Double that depth, with over twenty times the size budget,
// so ordinary inference stays precise while recursive types stay finite.
const MAX_DEPTH: usize = 8;
const MAX_NODES: usize = 256;

thread_local! {
    /// Firings by result kind (the backstop's count).
    static FIRINGS: std::cell::RefCell<[u64; 3]> = const { std::cell::RefCell::new([0; 3]) };
}

/// Bound firings so far: (to `Array[untyped]`, to `Hash[untyped, untyped]`, to `untyped`).
pub(super) fn firings() -> serde_json::Value {
    FIRINGS.with(|f| {
        let f = f.borrow();
        serde_json::json!({"array": f[0], "hash": f[1], "untyped": f[2], "total": f[0] + f[1] + f[2]})
    })
}

pub(super) fn bound(ty: Ty) -> Ty {
    let mut nodes = MAX_NODES;
    if fits(&ty, MAX_DEPTH, &mut nodes) {
        return ty;
    }
    let kind = match ty {
        Ty::Array { .. } | Ty::Tuple { .. } => 0,
        Ty::Hash { .. } => 1,
        _ => 2,
    };
    FIRINGS.with(|f| f.borrow_mut()[kind] += 1);
    // A union of the contents can retain the same recursive structure.
    // Keep a collection's runtime shape, but stop expanding its contents.
    match ty {
        Ty::Array { .. } | Ty::Tuple { .. } => Ty::Array {
            elem: std::sync::Arc::new(Ty::Untyped),
        },
        Ty::Hash { .. } => Ty::Hash {
            key: std::sync::Arc::new(Ty::Untyped),
            value: std::sync::Arc::new(Ty::Untyped),
        },
        _ => Ty::Untyped,
    }
}

// Count all structural edges: tuples can recur through arrays, hashes,
// unions or function signatures, and unions can grow in width as well
// as depth. Stop at either budget instead of walking an oversized tree.
fn fits(ty: &Ty, depth: usize, nodes: &mut usize) -> bool {
    if *nodes == 0 {
        return false;
    }
    *nodes -= 1;
    let mut child = |t: &Ty| depth > 0 && fits(t, depth - 1, nodes);
    match ty {
        Ty::Array { elem } => child(elem),
        Ty::Hash { key, value } => child(key) && child(value),
        Ty::Tuple { elems } => depth > 0 && elems.iter().all(child),
        Ty::Union { variants } => depth > 0 && variants.iter().all(child),
        Ty::Record { row } => depth > 0 && row.fields.values().all(child),
        Ty::Class { args, .. } => args.is_empty() || (depth > 0 && args.iter().all(child)),
        Ty::Fn {
            params, block, ret, ..
        } => {
            params.iter().all(|p| child(&p.ty))
                && block.as_ref().is_none_or(|b| child(b))
                && child(ret)
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::super::body::union_of;
    use super::*;

    fn nested(depth: usize, leaf: Ty) -> Ty {
        (0..depth).fold(leaf, |ty, _| Ty::Tuple {
            elems: vec![Ty::Int, ty].into(),
        })
    }

    #[test]
    fn types_within_both_budgets_are_unchanged() {
        for depth in 1..=MAX_DEPTH {
            let ty = nested(depth, Ty::Str);
            assert_eq!(bound(ty.clone()), ty);
        }
        let wide = Ty::Tuple {
            elems: vec![Ty::Str; MAX_NODES - 1].into(),
        };
        assert_eq!(bound(wide.clone()), wide);
    }

    #[test]
    fn joins_bound_tuples_even_when_the_other_arm_is_bottom() {
        let ty = nested(MAX_DEPTH + 1, Ty::Str);
        let widened = Ty::Array {
            elem: std::sync::Arc::new(Ty::Untyped),
        };
        assert_eq!(union_of(ty.clone(), Ty::Bottom), widened);
        assert_eq!(union_of(Ty::Bottom, ty.clone()), widened);
        assert_eq!(union_of(ty.clone(), ty), widened);
    }

    #[test]
    fn wrappers_cannot_hide_recursive_type_depth() {
        let ty = (0..MAX_DEPTH + 1).fold(Ty::Str, |ty, _| Ty::Array { elem: std::sync::Arc::new(ty) });
        assert_eq!(
            bound(ty),
            Ty::Array {
                elem: std::sync::Arc::new(Ty::Untyped)
            }
        );
        let ty = (0..MAX_DEPTH + 1).fold(Ty::Str, |ty, _| Ty::Hash {
            key: std::sync::Arc::new(Ty::Sym),
            value: std::sync::Arc::new(ty),
        });
        assert_eq!(
            bound(ty),
            Ty::Hash {
                key: std::sync::Arc::new(Ty::Untyped),
                value: std::sync::Arc::new(Ty::Untyped)
            }
        );
    }

    #[test]
    fn width_and_join_results_are_bounded_too() {
        let wide = Ty::Tuple {
            elems: vec![Ty::Str; MAX_NODES].into(),
        };
        assert_eq!(
            bound(wide),
            Ty::Array {
                elem: std::sync::Arc::new(Ty::Untyped)
            }
        );
        let a = Ty::Tuple {
            elems: vec![Ty::Str; MAX_NODES / 2].into(),
        };
        let b = Ty::Tuple {
            elems: vec![Ty::Bool; MAX_NODES / 2].into(),
        };
        assert_eq!(union_of(a.clone(), b.clone()), Ty::Untyped);
        assert_eq!(union_of(a.clone(), b.clone()), union_of(b, a));
        assert_eq!(
            bound(bound(nested(MAX_DEPTH + 1, Ty::Str))),
            bound(nested(MAX_DEPTH + 1, Ty::Str))
        );
    }
}
