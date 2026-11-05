// Not every class the name could spell: only where the found class is sent a method that only an ActiveJob class answers is the closed set of the app's jobs the right one.
use crate::app::App;
use crate::dialect::MethodDef;
use crate::expr::{Arm, Expr, ExprNode, LValue, Literal, Pattern};
use crate::ident::Symbol;

const JOB_CLASS_ONLY: &[&str] = &["queue_adapter_name", "perform_later", "perform_now", "queue_as", "perform_all_later"];

pub(super) fn rewrite(app: &mut App) {
    let parents: crate::det::HashMap<String, String> = app
        .library_classes
        .iter()
        .filter_map(|lc| lc.parent.as_ref().map(|p| (lc.name.0.as_str().to_string(), p.0.as_str().trim_start_matches("::").to_string())))
        .collect();
    let is_job = |name: &str| {
        let mut at = name.to_string();
        for _ in 0..32 {
            match parents.get(&at) {
                Some(p) if p == "ActiveJob::Base" => return true,
                Some(p) => at = p.clone(),
                None => return false,
            }
        }
        false
    };
    let mut jobs: Vec<String> = app
        .library_classes
        .iter()
        .filter(|lc| !lc.is_module && is_job(lc.name.0.as_str()))
        .map(|lc| lc.name.0.as_str().to_string())
        .collect();
    jobs.sort();
    if jobs.is_empty() {
        return;
    }
    for lc in app.library_classes.iter_mut() {
        let readers = job_class_readers(&lc.methods.iter().collect::<Vec<_>>());
        for m in lc.methods.iter_mut().filter(|m| readers.contains(&m.name)) {
            replace_sites(&mut m.body, &jobs);
        }
    }
    for model in app.models.iter_mut() {
        let readers = job_class_readers(&model.methods().collect::<Vec<_>>());
        for item in model.body.iter_mut() {
            if let crate::dialect::ModelBodyItem::Method { method, .. } = item
                && readers.contains(&method.name)
            {
                replace_sites(&mut method.body, &jobs);
            }
        }
    }
}

/// The class's readers `def r; @x ||= … .safe_constantize …; end` whose answer some method of the class sends a job-only class method.
fn job_class_readers(methods: &[&MethodDef]) -> Vec<Symbol> {
    let candidates: Vec<(Symbol, Symbol)> = methods
        .iter()
        .filter(|m| m.params.is_empty())
        .filter_map(|m| match &*m.body.node {
            ExprNode::OpAssign { target: LValue::Ivar { name }, value, .. } | ExprNode::Assign { target: LValue::Ivar { name }, value }
                if has_dynamic_safe_constantize(value) =>
            {
                Some((m.name.clone(), name.clone()))
            }
            _ => None,
        })
        .collect();
    candidates
        .into_iter()
        .filter(|(reader, ivar)| methods.iter().any(|m| sends_job_only(&m.body, reader, ivar)))
        .map(|(reader, _)| reader)
        .collect()
}

fn has_dynamic_safe_constantize(e: &Expr) -> bool {
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node
        && method.as_str() == "safe_constantize"
        && args.is_empty()
        && !matches!(&*r.node, ExprNode::Lit { .. })
    {
        return true;
    }
    let mut found = false;
    e.node.for_each_child(&mut |c| found = found || has_dynamic_safe_constantize(c));
    found
}

fn sends_job_only(e: &Expr, reader: &Symbol, ivar: &Symbol) -> bool {
    if let ExprNode::Send { recv: Some(r), method, .. } = &*e.node
        && JOB_CLASS_ONLY.contains(&method.as_str())
    {
        let names_it = match &*r.node {
            ExprNode::Send { recv: None, method, args, .. } => method == reader && args.is_empty(),
            ExprNode::Ivar { name } => name == ivar,
            _ => false,
        };
        if names_it {
            return true;
        }
    }
    let mut found = false;
    e.node.for_each_child(&mut |c| found = found || sends_job_only(c, reader, ivar));
    found
}

fn replace_sites(e: &mut Expr, jobs: &[String]) {
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &mut *e.node
        && method.as_str() == "safe_constantize"
        && args.is_empty()
        && !matches!(&*r.node, ExprNode::Lit { .. })
    {
        let span = e.span;
        let scrutinee = r.clone();
        let arms = jobs
            .iter()
            .map(|job| {
                let mut class = Expr::new(span, ExprNode::Const { path: job.split("::").map(Symbol::from).collect() });
                // Not resolved from the source at its span: that text is the receiver's.
                class.decisions |= crate::expr::GENERATED_CONST_REF;
                Arm { pattern: Pattern::Lit { value: Literal::Str { value: job.clone() } }, guard: None, body: class }
            })
            .collect();
        *e.node = ExprNode::Case { scrutinee, arms };
        return;
    }
    e.node.for_each_child_mut(&mut |c| replace_sites(c, jobs));
}

