//! `Hash.ruby2_keywords_hash(hash)` →
//! `ActiveJob::Arguments.ruby2_keywords_hash(hash)`.
//!
//! The method answers a copy of the Hash flagged so that a later `*args`
//! splat passes it as keywords. Spinel refuses the call by name and no
//! target carries the flag, so the runtime holds the copies it answered
//! and `ActiveJob::Arguments.serialize` writes such a Hash with
//! `_aj_ruby2_keywords`, as Rails does (solid_queue's
//! `RecurringTask#arguments_with_kwargs` flags a recurring task's last
//! Hash argument). A splat of such a Hash into keyword parameters is not
//! modelled; a job's own keyword parameters ride its payload by name
//! (`lower::job_payload`).

use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub(crate) fn rewrite_node(expr: &mut Expr) {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &mut *expr.node else { return };
    if method.as_str() != "ruby2_keywords_hash" || args.len() != 1 {
        return;
    }
    let ExprNode::Const { path } = &*recv.node else { return };
    let hash = path.iter().filter(|p| !p.as_str().is_empty()).map(Symbol::as_str).eq(["Hash"]);
    if !hash {
        return;
    }
    let span = recv.span;
    *recv = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActiveJob"), Symbol::from("Arguments")] });
}
