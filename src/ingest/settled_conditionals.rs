// Not both arms kept: a `defined?` of a class the app carries is settled when the body loads, and Ruby never defines the other arm's methods (et-orbi's tzinfo 1.x fallbacks).
use crate::app::App;
use crate::dialect::{ConditionalDefinitions, LibraryClass};
use crate::expr::ExprNode;

pub(super) fn settle(app: &mut App) {
    let known: crate::det::HashSet<String> = app
        .library_classes
        .iter()
        .map(|c| c.name.0.as_str().to_string())
        .chain(app.models.iter().map(|m| m.name.0.as_str().to_string()))
        .collect();
    for lc in app.library_classes.iter_mut() {
        let records = std::mem::take(&mut lc.conditional_definitions);
        for record in records {
            match settled_arm(&record, &known) {
                Some(kept) => drop_other_arms(lc, &record, kept),
                None => lc.conditional_definitions.push(record),
            }
        }
    }
}

/// The arm Ruby runs, or `usize::MAX` for none (an `if` without `else` whose condition is false).
fn settled_arm(record: &ConditionalDefinitions, known: &crate::det::HashSet<String>) -> Option<usize> {
    if !matches!(record.arms.len(), 1 | 2)
        || record.arms.get(1).is_some_and(|a| a.condition.is_some())
        || record.arms.iter().any(|a| !a.nested.is_empty() || !a.declarations.is_empty())
    {
        return None;
    }
    let holds = settled(record.arms[0].condition.as_ref()?, known)? != record.negated;
    Some(match (holds, record.arms.len()) {
        (true, _) => 0,
        (false, 2) => 1,
        (false, _) => usize::MAX,
    })
}

/// A load-time condition whose answer the running Ruby (3.4 or later) and Spinel give alike.
fn settled(cond: &crate::expr::Expr, known: &crate::det::HashSet<String>) -> Option<bool> {
    match &*cond.node {
        ExprNode::Defined { operand } => {
            let ExprNode::Const { path } = &*operand.node else { return None };
            let name = path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::");
            known.contains(&name).then_some(true)
        }
        ExprNode::Send { recv: Some(r), method, args, .. } if method.as_str() == "!" && args.is_empty() => settled(r, known).map(|b| !b),
        ExprNode::Send { recv: None, method, args, .. } if method.as_str() == "!" && args.len() == 1 => settled(&args[0], known).map(|b| !b),
        ExprNode::BoolOp { op, left, right, .. } => {
            let l = settled(left, known);
            match op {
                crate::expr::BoolOpKind::Or if l == Some(true) => Some(true),
                crate::expr::BoolOpKind::And if l == Some(false) => Some(false),
                crate::expr::BoolOpKind::Or | crate::expr::BoolOpKind::And => {
                    let r = settled(right, known)?;
                    l.map(|_| r)
                }
            }
        }
        ExprNode::Send { recv: Some(r), method, args, .. } if method.as_str() == "respond_to?" => {
            let [arg] = args.as_slice() else { return None };
            let ExprNode::Lit { value: crate::expr::Literal::Sym { value } } = &*arg.node else { return None };
            let class = match &*r.node {
                ExprNode::Array { elements, .. } if elements.is_empty() => "Array",
                ExprNode::Send { recv: Some(c), method, args, .. }
                    if method.as_str() == "new" && args.is_empty() && matches!(&*c.node, ExprNode::Const { path } if path.last().is_some_and(|p| p.as_str() == "Object")) =>
                {
                    "Object"
                }
                _ => return None,
            };
            BUILTIN_ANSWERS.iter().find(|(c, m, _)| *c == class && *m == value.as_str()).map(|(_, _, has)| *has)
        }
        _ => None,
    }
}

/// What Ruby 3.4+ (and Spinel) answers to `respond_to?` for a builtin's method a gem probes for.
const BUILTIN_ANSWERS: &[(&str, &str, bool)] = &[
    ("Array", "bsearch", true),
    ("Array", "bsearch_index", true),
    ("Array", "sum", true),
    ("Array", "filter_map", true),
    // `taint` and its kin were removed in Ruby 3.2.
    ("Object", "untaint", false),
    ("Object", "taint", false),
    ("Object", "tainted?", false),
];

fn drop_other_arms(lc: &mut LibraryClass, record: &ConditionalDefinitions, kept: usize) {
    for (i, arm) in record.arms.iter().enumerate() {
        if i != kept {
            lc.methods.retain(|m| !arm.methods.iter().any(|member| member.is(m)));
        }
    }
}
